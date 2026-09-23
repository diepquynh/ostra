//! Projections of session state for the browser: summary, board cards, phases, artifacts.

use crate::plan::removed_phases;
use crate::runner::EngineError;
use crate::state::{DocsState, EpaState, LoopNext, SessionState, WorkLoop};
use ostra_core::api::{
    ArtifactRef, DecisionView, GateView, PhaseStatus, PhaseView, SessionDetail, SessionStatus, SessionSummary,
    StageCard, StageStatus,
};
use ostra_core::event::{GatePayload, JudgeKind, SessionKind};
use ostra_core::ids::{ExecutionId, WorkspaceId};
use ostra_core::pipeline::{Lane, StageKind};
use ostra_core::submit::Verdict;
use ostra_store::WorkspaceDb;
use std::collections::HashSet;
use std::path::PathBuf;

pub fn summary(s: &SessionState, workspace: &WorkspaceId, judge_cost: f64) -> SessionSummary {
    let cost: f64 = s.executions.values().filter_map(|e| e.result.as_ref()).map(|r| r.usage.cost_usd).sum::<f64>() + judge_cost;
    let open = s.open_gates().count() as u32;
    let running = s.running_executions().count();
    let status = if s.completed.is_some() {
        SessionStatus::Completed
    } else if s.failed.is_some() {
        SessionStatus::Failed
    } else if open > 0 && running == 0 {
        SessionStatus::Waiting
    } else {
        SessionStatus::Running
    };
    let (lane, label) = inferred_stage(s);
    SessionSummary {
        id: s.id.clone(),
        workspace: workspace.clone(),
        kind: s.kind.clone(),
        request: s.request.clone(),
        category: s.category,
        status,
        lane,
        stage_label: label,
        yolo: s.yolo,
        open_gates: open,
        projects: s.scope.clone(),
        cost_usd: cost,
        created_at: s.created_at,
        updated_at: s.updated_at,
    }
}

/// The hub's `inferStage` idea: the stage the session is at, from its latest activity.
pub fn inferred_stage(s: &SessionState) -> (Lane, String) {
    if s.completed.is_some() {
        return (Lane::Done, "Complete".into());
    }
    if let Some(e) = &s.failed {
        return (Lane::Done, format!("Stopped: {}", first(e)));
    }
    if let Some(g) = s.open_gates().last() {
        return (g.payload.stage().lane(), format!("Waiting: {}", g.title));
    }
    if let Some(r) = s.running_executions().last() {
        let label = match &r.purpose {
            ostra_core::event::ExecPurpose::Implement { phase, .. } => format!("Implementing phase {phase}"),
            ostra_core::event::ExecPurpose::Review { phase, tests: false, iteration } => format!("Reviewing phase {phase}, pass {iteration}"),
            ostra_core::event::ExecPurpose::Review { phase, tests: true, iteration } => format!("Reviewing tests of phase {phase}, pass {iteration}"),
            ostra_core::event::ExecPurpose::WriteTest { phase, .. } => format!("Writing tests for phase {phase}"),
            _ => stage_label(r.stage).to_string(),
        };
        return (r.stage.lane(), label);
    }
    if s.category.is_none() {
        return (Lane::Research, "Classifying the request".into());
    }
    (Lane::Research, "Working".into())
}

fn first(s: &str) -> String {
    s.lines().next().unwrap_or_default().chars().take(120).collect()
}

pub fn stage_label(k: StageKind) -> &'static str {
    use StageKind::*;
    match k {
        Intake => "Intake",
        Classify => "Classify the request",
        Explore => "Research",
        Sufficiency => "Check research coverage",
        Spec => "Write the spec",
        OpenQuestions => "Open questions",
        FactCheckSpec => "Fact-check the spec",
        SpecApproval => "Approve the spec",
        Stakes => "Judge the stakes",
        Plan => "Write the plan",
        FactCheckPlan => "Fact-check the plan",
        PlanApproval => "Approve the plan",
        Implement => "Implement",
        Review => "Code review",
        Autofix => "Apply auto-fixes",
        Staging => "Stage reviewed files",
        Handoff => "Prompt handoff",
        Rescue => "Rescue a stuck agent",
        Format => "Format",
        ClosingGate => "Tests and docs?",
        Epa => "Trace execution paths",
        WriteTest => "Write tests",
        TestReview => "Review tests",
        ModuleDocs => "Module documentation",
        Verify => "Verify",
        PromptGen => "Write prompts",
        QuickAnswer => "Answer",
        Completion => "Completion report",
        Detect => "Detect the stack",
        Scout => "Scout components",
        Propose => "Propose skills",
        SkillApproval => "Approve skills",
        GenerateSkill => "Generate skills",
        GenerateInventory => "Write the inventory",
    }
}

fn exec_ids(s: &SessionState, f: impl Fn(&crate::state::ExecRecord) -> bool) -> Vec<ExecutionId> {
    s.executions.values().filter(|e| f(e)).map(|e| e.id.clone()).collect()
}

fn gate_for(s: &SessionState, f: impl Fn(&GatePayload) -> bool) -> Option<&crate::state::GateRecord> {
    s.gates.values().rev().find(|g| f(&g.payload))
}

fn card(stage: StageKind, label: String, status: StageStatus) -> StageCard {
    StageCard {
        stage,
        lane: stage.lane(),
        label,
        status,
        project: None,
        phase: None,
        executions: vec![],
        gate: None,
        detail: None,
    }
}

fn loop_status(l: &WorkLoop) -> StageStatus {
    if l.running.is_some() {
        return StageStatus::Running;
    }
    match &l.next {
        LoopNext::Idle => StageStatus::Pending,
        LoopNext::Done => StageStatus::Done,
        LoopNext::Blocked { .. } => StageStatus::Blocked,
        LoopNext::CapReached { .. } | LoopNext::RescueGate { .. } | LoopNext::Failed { .. } => StageStatus::Waiting,
        _ => StageStatus::Running,
    }
}

pub fn stages(s: &SessionState) -> Vec<StageCard> {
    let mut out = vec![];
    use ostra_core::event::ExecPurpose as P;
    if let SessionKind::Init { project } = &s.kind {
        if let Some(i) = &s.init {
            let st = |exec: &Option<ExecutionId>, done: bool| {
                if done { StageStatus::Done } else if exec.is_some() { StageStatus::Running } else { StageStatus::Pending }
            };
            let mut c = card(StageKind::Detect, "Detect the stack".into(), st(&i.detect, i.detect_result.is_some()));
            c.project = Some(project.clone());
            c.executions = exec_ids(s, |e| matches!(e.purpose, P::Init { mode: ostra_core::InitializerMode::Detect, .. }));
            out.push(c);
            for x in &i.scouts {
                let mut c = card(StageKind::Scout, format!("Scout {}", x.key), st(&x.exec, x.result.is_some()));
                c.executions = x.exec.iter().cloned().collect();
                out.push(c);
            }
            let mut c = card(StageKind::Propose, "Propose skills".into(), st(&i.propose, i.propose_result.is_some()));
            c.executions = i.propose.iter().cloned().collect();
            out.push(c);
            let mut c = card(
                StageKind::SkillApproval,
                "Approve skills".into(),
                if i.decisions.is_some() { StageStatus::Done } else if i.approval_gate.is_some() { StageStatus::Waiting } else { StageStatus::Pending },
            );
            c.gate = i.approval_gate.clone();
            out.push(c);
            for x in &i.generates {
                let mut c = card(StageKind::GenerateSkill, format!("Generate {}", x.key), st(&x.exec, x.result.is_some()));
                c.executions = x.exec.iter().cloned().collect();
                out.push(c);
            }
            let mut c = card(StageKind::GenerateInventory, "Write the inventory".into(), st(&i.inventory, i.inventory_result.is_some()));
            c.executions = i.inventory.iter().cloned().collect();
            out.push(c);
        }
        out.push(completion_card(s));
        return out;
    }
    let classify_status = if s.category.is_some() { StageStatus::Done } else { StageStatus::Running };
    let mut c = card(StageKind::Classify, "Classify the request".into(), classify_status);
    c.detail = s.category.map(|c| c.to_string());
    out.push(c);
    for t in &s.explore {
        let status = if t.result.is_some() {
            StageStatus::Done
        } else if t.abandoned {
            StageStatus::Skipped
        } else if t.failed.is_some() {
            StageStatus::Failed
        } else if t.running {
            StageStatus::Running
        } else {
            StageStatus::Pending
        };
        let mut c = card(StageKind::Explore, format!("Research: {}", first(&t.task)), status);
        c.project = Some(t.project.clone());
        c.executions = t.exec.iter().cloned().collect();
        c.gate = t.gate.clone();
        c.detail = t.result.as_ref().map(|r| format!("{} sources, {} open questions", r.sources_retrieved, r.open_questions));
        out.push(c);
    }
    let decisions_of = |k: JudgeKind| s.decisions.values().filter(|d| d.judge == k).count();
    if decisions_of(JudgeKind::Sufficiency) > 0 {
        let mut c = card(StageKind::Sufficiency, "Check research coverage".into(), StageStatus::Done);
        c.detail = Some(format!("{} rounds", s.sufficiency_rounds));
        out.push(c);
    }
    let t = &s.spec;
    if !t.runs.is_empty() || t.running.is_some() {
        let mut c = card(
            StageKind::Spec,
            "Write the spec".into(),
            if t.running.is_some() { StageStatus::Running } else if t.current.is_some() { StageStatus::Done } else { StageStatus::Failed },
        );
        c.executions = exec_ids(s, |e| matches!(e.purpose, P::Spec { .. }));
        c.detail = Some(format!("version {}", t.version));
        out.push(c);
        if let Some(g) = gate_for(s, |p| matches!(p, GatePayload::OpenQuestions { artifact, .. } if artifact == "spec")) {
            let mut c = card(StageKind::OpenQuestions, "Open questions".into(), if g.answer.is_some() { StageStatus::Done } else { StageStatus::Waiting });
            c.gate = Some(g.id.clone());
            out.push(c);
        }
        if !t.checks.is_empty() {
            let verdict = t.check_for_current().map(|c| c.verdict);
            let mut c = card(
                StageKind::FactCheckSpec,
                "Fact-check the spec".into(),
                if t.check_running.is_some() { StageStatus::Running } else if verdict == Some(Verdict::Pass) { StageStatus::Done } else { StageStatus::Running },
            );
            c.executions = exec_ids(s, |e| matches!(e.purpose, P::FactCheck { target: ostra_core::event::FactTarget::Spec, .. }));
            c.detail = t.check_for_current().map(|c| format!("{:?}, {} findings", c.verdict, c.findings.len()));
            out.push(c);
        }
        if t.passed_current() && s.category != Some(ostra_core::pipeline::Category::Spec) {
            let mut c = card(StageKind::SpecApproval, "Approve the spec".into(), if t.approved { StageStatus::Done } else { StageStatus::Waiting });
            c.gate = t.approval_gate.clone();
            out.push(c);
        }
    }
    if let Some((_, stakes)) = s.stakes {
        let mut c = card(StageKind::Stakes, "Judge the stakes".into(), StageStatus::Done);
        c.detail = Some(format!("{stakes:?}"));
        out.push(c);
        if stakes == ostra_core::pipeline::Stakes::Low {
            out.push(card(StageKind::Plan, "Plan skipped: low stakes".into(), StageStatus::Skipped));
        }
    }
    let t = &s.plan;
    if !t.runs.is_empty() {
        let mut c = card(
            StageKind::Plan,
            "Write the plan".into(),
            if t.running.is_some() { StageStatus::Running } else if t.current.is_some() { StageStatus::Done } else { StageStatus::Failed },
        );
        c.executions = exec_ids(s, |e| matches!(e.purpose, P::Plan { .. }));
        out.push(c);
        if !t.checks.is_empty() {
            let mut c = card(
                StageKind::FactCheckPlan,
                "Fact-check the plan".into(),
                if t.passed_current() { StageStatus::Done } else { StageStatus::Running },
            );
            c.executions = exec_ids(s, |e| matches!(e.purpose, P::FactCheck { target: ostra_core::event::FactTarget::Plan, .. }));
            c.detail = t.check_for_current().map(|c| format!("{:?}, {} findings", c.verdict, c.findings.len()));
            out.push(c);
        }
        if t.passed_current() {
            let mut c = card(StageKind::PlanApproval, "Approve the plan".into(), if t.approved { StageStatus::Done } else { StageStatus::Waiting });
            c.gate = t.approval_gate.clone();
            out.push(c);
        }
    }
    let removed = removed_phases(s);
    for p in s.phases.values() {
        let id = p.info.id;
        let status = if removed.contains(&id) { StageStatus::Skipped } else { loop_status(&p.impl_loop) };
        let mut c = card(StageKind::Implement, format!("Phase {id}: {}", p.info.title), status);
        c.project = Some(p.info.project.clone());
        c.phase = Some(id);
        c.executions = exec_ids(s, |e| e.loop_key == Some((id, false)));
        c.gate = p.impl_loop.gate.clone().or(p.blocked_gate.clone());
        c.detail = Some(format!(
            "{} review passes{}",
            p.impl_loop.iterations,
            if p.impl_loop.blocker_open { ", BLOCKER open" } else { "" }
        ));
        out.push(c);
        if !matches!(p.epa, EpaState::NotStarted) || !p.test_loop.is_idle() {
            let mut c = card(StageKind::WriteTest, format!("Tests for phase {id}"), loop_status(&p.test_loop));
            if matches!(p.epa, EpaState::Running(_)) {
                c.status = StageStatus::Running;
                c.stage = StageKind::Epa;
                c.lane = Lane::Test;
            }
            c.project = Some(p.info.project.clone());
            c.phase = Some(id);
            c.executions = exec_ids(s, |e| e.loop_key == Some((id, true)) || matches!(e.purpose, P::Epa { phase } if phase == id));
            out.push(c);
        }
    }
    for (key, t) in &s.project_tracks {
        if let Some(f) = t.format {
            let mut c = card(StageKind::Format, format!("Format {key}"), StageStatus::Done);
            c.project = Some(key.clone());
            c.detail = Some(match f {
                None => "no format command".into(),
                Some(code) => format!("exit {code}"),
            });
            out.push(c);
        }
        if t.closing_gate.is_some() || t.closing.is_some() {
            let mut c = card(
                StageKind::ClosingGate,
                format!("Tests and docs for {key}"),
                if t.closing.is_some() { StageStatus::Done } else { StageStatus::Waiting },
            );
            c.project = Some(key.clone());
            c.gate = t.closing_gate.clone();
            c.detail = t.closing.map(|(a, b)| format!("tests {}, docs {}", if a { "yes" } else { "no" }, if b { "yes" } else { "no" }));
            out.push(c);
        }
        let docs_status = match &t.docs {
            DocsState::NotStarted => None,
            DocsState::Running(_) => Some(StageStatus::Running),
            DocsState::Done(_) => Some(StageStatus::Done),
            DocsState::Failed { .. } => Some(StageStatus::Failed),
            DocsState::Abandoned => Some(StageStatus::Skipped),
        };
        if let Some(st) = docs_status {
            let mut c = card(StageKind::ModuleDocs, format!("Module docs for {key}"), st);
            c.project = Some(key.clone());
            c.executions = exec_ids(s, |e| matches!(&e.purpose, P::ModuleDocs { project } if project == key));
            out.push(c);
        }
    }
    if s.quick.exec.is_some() {
        let mut c = card(StageKind::QuickAnswer, "Answer".into(), if s.quick.answer.is_some() { StageStatus::Done } else { StageStatus::Running });
        c.executions = s.quick.exec.iter().cloned().collect();
        out.push(c);
    }
    out.push(completion_card(s));
    out
}

fn completion_card(s: &SessionState) -> StageCard {
    card(
        StageKind::Completion,
        "Completion report".into(),
        if s.completed.is_some() {
            StageStatus::Done
        } else if s.failed.is_some() {
            StageStatus::Failed
        } else {
            StageStatus::Pending
        },
    )
}

pub fn phases(s: &SessionState) -> Vec<PhaseView> {
    let removed = removed_phases(s);
    s.phases
        .values()
        .map(|p| {
            let l = &p.impl_loop;
            let status = if removed.contains(&p.info.id) {
                PhaseStatus::Removed
            } else if l.is_blocked() {
                PhaseStatus::Blocked
            } else if l.is_done() {
                PhaseStatus::Passed
            } else if l.is_idle() {
                PhaseStatus::Queued
            } else if matches!(l.next, LoopNext::Review | LoopNext::Autofix { .. } | LoopNext::Stage)
                || l.running.as_ref().is_some_and(|r| s.executions.get(r).is_some_and(|e| e.agent == ostra_core::AgentName::CodeReviewer))
            {
                PhaseStatus::Reviewing
            } else {
                PhaseStatus::Implementing
            };
            let tests = match (&p.epa, &p.test_loop.next) {
                (EpaState::NotStarted, _) if p.test_loop.is_idle() => "none",
                (EpaState::Abandoned, _) => "skipped",
                (_, LoopNext::Done) => "passed",
                (_, LoopNext::Blocked { .. }) => "blocked",
                (EpaState::Done(_), LoopNext::Idle) => "queued",
                _ => "running",
            };
            PhaseView {
                info: p.info.clone(),
                status,
                review_iterations: l.iterations,
                tests: tests.into(),
                security_block: l.blocker_open || p.test_loop.blocker_open,
            }
        })
        .collect()
}

pub fn artifacts(s: &SessionState) -> Vec<ArtifactRef> {
    let mut out = vec![];
    let mut seen = HashSet::new();
    let mut add = |path: PathBuf, kind: &str, label: String, project: Option<String>| {
        if seen.insert(path.clone()) {
            out.push(ArtifactRef { path, kind: kind.into(), label, project });
        }
    };
    for t in &s.explore {
        if let Some(r) = &t.result {
            add(PathBuf::from(&r.research_path), "research", format!("Research: {}", first(&t.task)), Some(t.project.clone()));
        }
    }
    if let Some(spec) = &s.spec.current {
        add(PathBuf::from(&spec.spec_path), "spec", "Spec".into(), None);
    }
    if let Some(plan) = &s.plan.current {
        add(PathBuf::from(&plan.master_plan_path), "plan", "Master plan".into(), None);
        for p in &plan.phases {
            add(PathBuf::from(&p.file), "phase", format!("Phase {}: {}", p.id, p.title), Some(p.project.clone()));
        }
    }
    for p in s.phases.values() {
        if let Some(r) = &p.implementer_report {
            add(r.clone(), "report", format!("Implementer report, phase {}", p.info.id), Some(p.info.project.clone()));
        }
        for tests in [false, true] {
            let ledger = s.ledger_path(&p.info.project, p.info.id, tests);
            if ledger.exists() {
                add(ledger, "ledger", format!("Review ledger, phase {}{}", p.info.id, if tests { " tests" } else { "" }), Some(p.info.project.clone()));
            }
        }
        if let EpaState::Done(path) = &p.epa {
            add(path.clone(), "report", format!("EPA report, phase {}", p.info.id), Some(p.info.project.clone()));
        }
        if let Some(r) = &p.test_loop.report {
            add(r.clone(), "report", format!("Test report, phase {}", p.info.id), Some(p.info.project.clone()));
        }
    }
    for (key, t) in &s.project_tracks {
        if let DocsState::Done(Some(p)) = &t.docs {
            add(p.clone(), "report", format!("Module docs report, {key}"), Some(key.clone()));
        }
    }
    if let Some((path, _)) = &s.completed {
        add(path.clone(), "completion", "Completion report".into(), None);
    }
    out
}

pub fn detail(
    s: &SessionState,
    db: &WorkspaceDb,
    workspace: &WorkspaceId,
    live: HashSet<ExecutionId>,
) -> Result<SessionDetail, EngineError> {
    let mut executions = db.list_executions(&s.id)?;
    for e in executions.iter_mut() {
        e.has_terminal = matches!(e.executor, ostra_core::ExecutorKind::Harness(_)) && live.contains(&e.id);
    }
    let gates = s
        .gates
        .values()
        .map(|g| GateView {
            id: g.id.clone(),
            session: s.id.clone(),
            title: g.title.clone(),
            explanation: g.explanation.clone(),
            payload: g.payload.clone(),
            answer: g.answer.clone(),
            source: g.source,
            reason: g.reason.clone(),
            opened_at: g.opened_at,
            answered_at: g.answered_at,
        })
        .collect();
    let decisions = s
        .decisions
        .values()
        .map(|d| DecisionView {
            id: d.id.clone(),
            judge: d.judge,
            subject: d.subject.clone(),
            input_summary: d.input_summary.clone(),
            output: d.output.clone(),
            reason: d.reason.clone(),
            overridden: d.overridden,
            can_override: s.can_override(&d.id),
            at: d.at,
        })
        .collect();
    let completion = s.completed.as_ref().and_then(|(p, _)| std::fs::read_to_string(p).ok());
    let cost: f64 = executions.iter().map(|e| e.usage.cost_usd).sum();
    let mut summary = summary(s, workspace, 0.0);
    summary.cost_usd = summary.cost_usd.max(cost);
    Ok(SessionDetail {
        summary,
        stages: stages(s),
        phases: phases(s),
        executions,
        gates,
        decisions,
        artifacts: artifacts(s),
        completion,
        session_root: s.session_root.clone(),
    })
}
