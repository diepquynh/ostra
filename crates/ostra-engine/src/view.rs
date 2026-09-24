//! Projections of session state for the browser: summary, board cards, phases, artifacts.

use crate::plan::removed_phases;
use crate::runner::EngineError;
use crate::state::{DocsState, EpaState, ExecRecord, LoopNext, SessionState, WorkLoop};
use ostra_core::agent::AgentName;
use ostra_core::api::{
    ArtifactRef, ChangedBy, DecisionView, ExecutionGroupView, ExecutionView, FactCheckView,
    GateView, PendingGate, PhaseStatus, PhaseView, SessionDetail, SessionStatus, SessionSummary,
    StageCard, StageStatus, TreeGroup, TreeRun, TreeSession,
};
use ostra_core::event::{GatePayload, JudgeKind, SessionKind, numbered_run_label};
use ostra_core::exec::ExecutionStatus;
use ostra_core::executor::ExecStream;
use ostra_core::ids::{ExecutionId, WorkspaceId};
use ostra_core::paths;
use ostra_core::pipeline::{Lane, StageKind};
use ostra_core::submit::Verdict;
use ostra_store::WorkspaceDb;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

pub fn summary(s: &SessionState, workspace: &WorkspaceId, judge_cost: f64) -> SessionSummary {
    let cost: f64 = s
        .executions
        .values()
        .filter_map(|e| e.result.as_ref())
        .map(|r| r.usage.cost_usd)
        .sum::<f64>()
        + judge_cost;
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
        title: s.title.clone(),
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
            ostra_core::event::ExecPurpose::Implement { phase, .. } => {
                format!("Implementing phase {phase}")
            }
            ostra_core::event::ExecPurpose::Review {
                phase,
                tests: false,
                iteration,
            } => format!("Reviewing phase {phase}, pass {iteration}"),
            ostra_core::event::ExecPurpose::Review {
                phase,
                tests: true,
                iteration,
            } => format!("Reviewing tests of phase {phase}, pass {iteration}"),
            ostra_core::event::ExecPurpose::WriteTest { phase, .. } => {
                format!("Writing tests for phase {phase}")
            }
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
    s.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect()
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
    s.executions
        .values()
        .filter(|e| f(e))
        .map(|e| e.id.clone())
        .collect()
}

fn gate_for(
    s: &SessionState,
    f: impl Fn(&GatePayload) -> bool,
) -> Option<&crate::state::GateRecord> {
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
        LoopNext::CapReached { .. } | LoopNext::RescueGate { .. } | LoopNext::Failed { .. } => {
            StageStatus::Waiting
        }
        _ => StageStatus::Running,
    }
}

pub fn stages(s: &SessionState) -> Vec<StageCard> {
    let mut out = vec![];
    use ostra_core::event::ExecPurpose as P;
    if let SessionKind::Init { project } = &s.kind {
        if let Some(i) = &s.init {
            let st = |exec: &Option<ExecutionId>, done: bool| {
                if done {
                    StageStatus::Done
                } else if exec.is_some() {
                    StageStatus::Running
                } else {
                    StageStatus::Pending
                }
            };
            let mut c = card(
                StageKind::Detect,
                "Detect the stack".into(),
                st(&i.detect, i.detect_result.is_some()),
            );
            c.project = Some(project.clone());
            c.executions = exec_ids(s, |e| {
                matches!(
                    e.purpose,
                    P::Init {
                        mode: ostra_core::InitializerMode::Detect,
                        ..
                    }
                )
            });
            out.push(c);
            for x in &i.scouts {
                let mut c = card(
                    StageKind::Scout,
                    format!("Scout {}", x.key),
                    st(&x.exec, x.result.is_some()),
                );
                c.executions = x.exec.iter().cloned().collect();
                out.push(c);
            }
            let mut c = card(
                StageKind::Propose,
                "Propose skills".into(),
                st(&i.propose, i.propose_result.is_some()),
            );
            c.executions = i.propose.iter().cloned().collect();
            out.push(c);
            let mut c = card(
                StageKind::SkillApproval,
                "Approve skills".into(),
                if i.decisions.is_some() {
                    StageStatus::Done
                } else if i.approval_gate.is_some() {
                    StageStatus::Waiting
                } else {
                    StageStatus::Pending
                },
            );
            c.gate = i.approval_gate.clone();
            out.push(c);
            for x in &i.generates {
                let mut c = card(
                    StageKind::GenerateSkill,
                    format!("Generate {}", x.key),
                    st(&x.exec, x.result.is_some()),
                );
                c.executions = x.exec.iter().cloned().collect();
                out.push(c);
            }
            let mut c = card(
                StageKind::GenerateInventory,
                "Write the inventory".into(),
                st(&i.inventory, i.inventory_result.is_some()),
            );
            c.executions = i.inventory.iter().cloned().collect();
            out.push(c);
        }
        out.push(completion_card(s));
        return out;
    }
    let classify_status = if s.category.is_some() {
        StageStatus::Done
    } else {
        StageStatus::Running
    };
    let mut c = card(
        StageKind::Classify,
        "Classify the request".into(),
        classify_status,
    );
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
        let mut c = card(
            StageKind::Explore,
            format!("Research: {}", first(&t.task)),
            status,
        );
        c.project = Some(t.project.clone());
        c.executions = t.exec.iter().cloned().collect();
        c.gate = t.gate.clone();
        c.detail = t.result.as_ref().map(|r| {
            format!(
                "{} sources, {} open questions",
                r.sources_retrieved, r.open_questions
            )
        });
        out.push(c);
    }
    let decisions_of = |k: JudgeKind| s.decisions.values().filter(|d| d.judge == k).count();
    if decisions_of(JudgeKind::Sufficiency) > 0 {
        let mut c = card(
            StageKind::Sufficiency,
            "Check research coverage".into(),
            StageStatus::Done,
        );
        c.detail = Some(format!("{} rounds", s.sufficiency_rounds));
        out.push(c);
    }
    let t = &s.spec;
    if !t.runs.is_empty() || t.running.is_some() {
        let mut c = card(
            StageKind::Spec,
            "Write the spec".into(),
            if t.running.is_some() {
                StageStatus::Running
            } else if t.current.is_some() {
                StageStatus::Done
            } else {
                StageStatus::Failed
            },
        );
        c.executions = exec_ids(s, |e| matches!(e.purpose, P::Spec { .. }));
        c.detail = Some(format!("version {}", t.version));
        out.push(c);
        if let Some(g) = gate_for(
            s,
            |p| matches!(p, GatePayload::OpenQuestions { artifact, .. } if artifact == "spec"),
        ) {
            let mut c = card(
                StageKind::OpenQuestions,
                "Open questions".into(),
                if g.answer.is_some() {
                    StageStatus::Done
                } else {
                    StageStatus::Waiting
                },
            );
            c.gate = Some(g.id.clone());
            out.push(c);
        }
        if !t.checks.is_empty() {
            let verdict = t.check_for_current().map(|c| c.verdict);
            let mut c = card(
                StageKind::FactCheckSpec,
                "Fact-check the spec".into(),
                if t.check_running.is_some() {
                    StageStatus::Running
                } else if verdict == Some(Verdict::Pass) {
                    StageStatus::Done
                } else {
                    StageStatus::Running
                },
            );
            c.executions = exec_ids(s, |e| {
                matches!(
                    e.purpose,
                    P::FactCheck {
                        target: ostra_core::event::FactTarget::Spec,
                        ..
                    }
                )
            });
            c.detail = t
                .check_for_current()
                .map(|c| format!("{:?}, {} findings", c.verdict, c.findings.len()));
            out.push(c);
        }
        if t.passed_current() && s.category != Some(ostra_core::pipeline::Category::Spec) {
            let mut c = card(
                StageKind::SpecApproval,
                "Approve the spec".into(),
                if t.approved {
                    StageStatus::Done
                } else {
                    StageStatus::Waiting
                },
            );
            c.gate = t.approval_gate.clone();
            out.push(c);
        }
    }
    if let Some((_, stakes)) = s.stakes {
        let mut c = card(
            StageKind::Stakes,
            "Judge the stakes".into(),
            StageStatus::Done,
        );
        c.detail = Some(format!("{stakes:?}"));
        out.push(c);
        if stakes == ostra_core::pipeline::Stakes::Low {
            out.push(card(
                StageKind::Plan,
                "Plan skipped: low stakes".into(),
                StageStatus::Skipped,
            ));
        }
    }
    let t = &s.plan;
    if !t.runs.is_empty() {
        let mut c = card(
            StageKind::Plan,
            "Write the plan".into(),
            if t.running.is_some() {
                StageStatus::Running
            } else if t.current.is_some() {
                StageStatus::Done
            } else {
                StageStatus::Failed
            },
        );
        c.executions = exec_ids(s, |e| matches!(e.purpose, P::Plan { .. }));
        out.push(c);
        if !t.checks.is_empty() {
            let mut c = card(
                StageKind::FactCheckPlan,
                "Fact-check the plan".into(),
                if t.passed_current() {
                    StageStatus::Done
                } else {
                    StageStatus::Running
                },
            );
            c.executions = exec_ids(s, |e| {
                matches!(
                    e.purpose,
                    P::FactCheck {
                        target: ostra_core::event::FactTarget::Plan,
                        ..
                    }
                )
            });
            c.detail = t
                .check_for_current()
                .map(|c| format!("{:?}, {} findings", c.verdict, c.findings.len()));
            out.push(c);
        }
        if t.passed_current() {
            let mut c = card(
                StageKind::PlanApproval,
                "Approve the plan".into(),
                if t.approved {
                    StageStatus::Done
                } else {
                    StageStatus::Waiting
                },
            );
            c.gate = t.approval_gate.clone();
            out.push(c);
        }
    }
    let removed = removed_phases(s);
    for p in s.phases.values() {
        let id = p.info.id;
        let status = if removed.contains(&id) {
            StageStatus::Skipped
        } else {
            loop_status(&p.impl_loop)
        };
        let mut c = card(
            StageKind::Implement,
            format!("Phase {id}: {}", p.info.title),
            status,
        );
        c.project = Some(p.info.project.clone());
        c.phase = Some(id);
        c.executions = exec_ids(s, |e| e.loop_key == Some((id, false)));
        c.gate = p.impl_loop.gate.clone().or(p.blocked_gate.clone());
        c.detail = Some(format!(
            "{} review passes{}",
            p.impl_loop.iterations,
            if p.impl_loop.blocker_open {
                ", BLOCKER open"
            } else {
                ""
            }
        ));
        out.push(c);
        if !matches!(p.epa, EpaState::NotStarted) || !p.test_loop.is_idle() {
            let mut c = card(
                StageKind::WriteTest,
                format!("Tests for phase {id}"),
                loop_status(&p.test_loop),
            );
            if matches!(p.epa, EpaState::Running(_)) {
                c.status = StageStatus::Running;
                c.stage = StageKind::Epa;
                c.lane = Lane::Test;
            }
            c.project = Some(p.info.project.clone());
            c.phase = Some(id);
            c.executions = exec_ids(s, |e| {
                e.loop_key == Some((id, true))
                    || matches!(e.purpose, P::Epa { phase } if phase == id)
            });
            out.push(c);
        }
    }
    for (key, t) in &s.project_tracks {
        if let Some(f) = t.format {
            let mut c = card(
                StageKind::Format,
                format!("Format {key}"),
                StageStatus::Done,
            );
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
                if t.closing.is_some() {
                    StageStatus::Done
                } else {
                    StageStatus::Waiting
                },
            );
            c.project = Some(key.clone());
            c.gate = t.closing_gate.clone();
            c.detail = t.closing.map(|(a, b)| {
                format!(
                    "tests {}, docs {}",
                    if a { "yes" } else { "no" },
                    if b { "yes" } else { "no" }
                )
            });
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
            c.executions = exec_ids(
                s,
                |e| matches!(&e.purpose, P::ModuleDocs { project } if project == key),
            );
            out.push(c);
        }
    }
    if s.quick.exec.is_some() {
        let mut c = card(
            StageKind::QuickAnswer,
            "Answer".into(),
            if s.quick.answer.is_some() {
                StageStatus::Done
            } else {
                StageStatus::Running
            },
        );
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
            } else if matches!(
                l.next,
                LoopNext::Review | LoopNext::Autofix { .. } | LoopNext::Stage
            ) || l.running.as_ref().is_some_and(|r| {
                s.executions
                    .get(r)
                    .is_some_and(|e| e.agent == ostra_core::AgentName::CodeReviewer)
            }) {
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
            out.push(ArtifactRef {
                path,
                kind: kind.into(),
                label,
                project,
            });
        }
    };
    for t in &s.explore {
        if let Some(r) = &t.result {
            add(
                PathBuf::from(&r.research_path),
                "research",
                format!("Research: {}", first(&t.task)),
                Some(t.project.clone()),
            );
        }
    }
    if let Some(spec) = &s.spec.current {
        add(PathBuf::from(&spec.spec_path), "spec", "Spec".into(), None);
    }
    if let Some(plan) = &s.plan.current {
        add(
            PathBuf::from(&plan.master_plan_path),
            "plan",
            "Master plan".into(),
            None,
        );
        for p in &plan.phases {
            add(
                PathBuf::from(&p.file),
                "phase",
                format!("Phase {}: {}", p.id, p.title),
                Some(p.project.clone()),
            );
        }
    }
    for p in s.phases.values() {
        if let Some(r) = &p.implementer_report {
            add(
                r.clone(),
                "report",
                format!("Implementer report, phase {}", p.info.id),
                Some(p.info.project.clone()),
            );
        }
        for tests in [false, true] {
            let ledger = s.ledger_path(&p.info.project, p.info.id, tests);
            if ledger.exists() {
                add(
                    ledger,
                    "ledger",
                    format!(
                        "Review ledger, phase {}{}",
                        p.info.id,
                        if tests { " tests" } else { "" }
                    ),
                    Some(p.info.project.clone()),
                );
            }
        }
        if let EpaState::Done(path) = &p.epa {
            add(
                path.clone(),
                "report",
                format!("EPA report, phase {}", p.info.id),
                Some(p.info.project.clone()),
            );
        }
        if let Some(r) = &p.test_loop.report {
            add(
                r.clone(),
                "report",
                format!("Test report, phase {}", p.info.id),
                Some(p.info.project.clone()),
            );
        }
    }
    for (key, t) in &s.project_tracks {
        if let DocsState::Done(Some(p)) = &t.docs {
            add(
                p.clone(),
                "report",
                format!("Module docs report, {key}"),
                Some(key.clone()),
            );
        }
    }
    if let Some((path, _)) = &s.completed {
        add(path.clone(), "completion", "Completion report".into(), None);
    }
    out
}

/// Each execution's run label, numbered when the same agent already ran the same label on the
/// same project in this session.
pub fn run_labels(s: &SessionState) -> HashMap<ExecutionId, String> {
    let mut runs: Vec<&ExecRecord> = s.executions.values().collect();
    runs.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut seen: HashMap<(AgentName, &str, String), u32> = HashMap::new();
    runs.into_iter()
        .map(|r| {
            let base = r.purpose.run_label();
            let n = seen
                .entry((r.agent, r.project.as_str(), base.clone()))
                .or_default();
            *n += 1;
            (r.id.clone(), numbered_run_label(&base, *n))
        })
        .collect()
}

/// Fill the fields of an execution view that come from the fold and the session dir.
pub fn decorate(s: &SessionState, labels: &HashMap<ExecutionId, String>, e: &mut ExecutionView) {
    if let Some(label) = labels.get(&e.id) {
        e.run_label = label.clone();
    }
    e.has_transcript = e.stream == ExecStream::Terminal
        && paths::terminal_transcript(&s.session_root, e.id.as_str()).is_file();
    e.repo_root = s.project_path(&e.project);
    e.pending_gate = s
        .open_gates()
        .filter(|g| g.payload.execution() == Some(&e.id))
        .max_by(|a, b| a.opened_at.cmp(&b.opened_at).then_with(|| a.id.cmp(&b.id)))
        .map(|g| PendingGate {
            id: g.id.clone(),
            kind: g.payload.kind_str().to_string(),
            title: g.title.clone(),
        });
}

/// Executions grouped by agent and project, in order of each group's first start.
pub fn execution_groups(executions: &[ExecutionView]) -> Vec<ExecutionGroupView> {
    let mut sorted: Vec<&ExecutionView> = executions.iter().collect();
    sorted.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut out: Vec<ExecutionGroupView> = vec![];
    let mut running: Vec<bool> = vec![];
    for e in sorted {
        let i = match out.iter().position(|g| g.group == e.group) {
            Some(i) => i,
            None => {
                out.push(ExecutionGroupView {
                    group: e.group.clone(),
                    agent: e.agent,
                    project: e.project.clone(),
                    status: e.status,
                    cost_usd: 0.0,
                    executions: vec![],
                });
                running.push(false);
                out.len() - 1
            }
        };
        let g = &mut out[i];
        g.executions.push(e.id.clone());
        g.cost_usd += e.usage.cost_usd;
        running[i] |= e.status == ExecutionStatus::Running;
        // A group is running while any of its runs is; otherwise it shows its latest run.
        g.status = if running[i] {
            ExecutionStatus::Running
        } else {
            e.status
        };
    }
    out
}

/// The session's node in the Sessions tree: its row, its executions grouped with numbered run
/// labels, and its artifacts.
pub fn tree_session(
    s: &SessionState,
    summary: &SessionSummary,
    mut executions: Vec<ExecutionView>,
) -> TreeSession {
    let labels = run_labels(s);
    for e in executions.iter_mut() {
        if let Some(label) = labels.get(&e.id) {
            e.run_label = label.clone();
        }
    }
    let spent: f64 = executions.iter().map(|e| e.usage.cost_usd).sum();
    let by_id: HashMap<&ExecutionId, &ExecutionView> =
        executions.iter().map(|e| (&e.id, e)).collect();
    let groups = execution_groups(&executions)
        .into_iter()
        .map(|g| TreeGroup {
            runs: g
                .executions
                .iter()
                .filter_map(|id| by_id.get(id))
                .map(|e| TreeRun {
                    id: e.id.clone(),
                    run_label: e.run_label.clone(),
                    status: e.status,
                    stream: e.stream,
                    summary: e.summary.clone(),
                })
                .collect(),
            group: g.group,
            agent: g.agent,
            project: g.project,
            status: g.status,
            cost_usd: g.cost_usd,
        })
        .collect();
    TreeSession {
        id: summary.id.clone(),
        title: summary.title.clone(),
        request: summary.request.clone(),
        kind: summary.kind.clone(),
        status: summary.status,
        open_gates: summary.open_gates,
        cost_usd: summary.cost_usd.max(spent),
        updated_at: summary.updated_at,
        groups,
        artifacts: artifacts(s),
    }
}

/// Files the session's work passes reported changing in `project`, keyed by project-relative path,
/// each attributed to the latest finished execution that reported it.
pub fn file_changes(s: &SessionState, project: &str) -> BTreeMap<String, ChangedBy> {
    let root = s.project_path(project);
    let mut out: BTreeMap<String, ChangedBy> = BTreeMap::new();
    for rec in s.executions.values().filter(|r| r.project == project) {
        let (Some((phase, tests)), Some(result)) = (rec.loop_key, rec.result.as_ref()) else {
            continue;
        };
        let Some(files) = result
            .submit
            .as_ref()
            .and_then(|v| v.get("changed_files"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        let staged = s.phases.get(&phase).is_some_and(|p| {
            if tests {
                p.test_loop.staged
            } else {
                p.impl_loop.staged
            }
        });
        let at = rec.ended_at.unwrap_or(rec.started_at);
        for f in files.iter().filter_map(|f| f.as_str()) {
            let Some(path) = project_relative(root.as_deref(), f) else {
                continue;
            };
            if out.get(&path).is_some_and(|c| c.at > at) {
                continue;
            }
            let by = ChangedBy {
                session: s.id.clone(),
                execution: rec.id.clone(),
                agent: rec.agent,
                phase: Some(phase),
                tests,
                staged,
                running: false,
                at,
            };
            out.insert(path, by);
        }
    }
    out
}

/// A reported path as project-relative with `/` separators; `None` when it leaves the project.
fn project_relative(root: Option<&Path>, reported: &str) -> Option<String> {
    let p = Path::new(reported.trim());
    let rel = if p.is_absolute() {
        ostra_core::paths::normalize(p)
            .strip_prefix(ostra_core::paths::normalize(root?))
            .ok()?
            .to_path_buf()
    } else {
        p.to_path_buf()
    };
    let mut parts = vec![];
    for c in rel.components() {
        match c {
            Component::Normal(n) => parts.push(n.to_string_lossy().to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

pub fn detail(
    s: &SessionState,
    db: &WorkspaceDb,
    workspace: &WorkspaceId,
    live: HashSet<ExecutionId>,
) -> Result<SessionDetail, EngineError> {
    let mut executions = db.list_executions(&s.id)?;
    let labels = run_labels(s);
    for e in executions.iter_mut() {
        e.has_terminal =
            matches!(e.executor, ostra_core::ExecutorKind::Harness(_)) && live.contains(&e.id);
        decorate(s, &labels, e);
    }
    let execution_groups = execution_groups(&executions);
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
    let completion = s
        .completed
        .as_ref()
        .and_then(|(p, _)| std::fs::read_to_string(p).ok());
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
        execution_groups,
        fact_checks: fact_checks(s),
    })
}

pub fn fact_checks(s: &SessionState) -> Vec<FactCheckView> {
    fn track<T>(t: &crate::state::ArtifactTrack<T>, target: &str) -> Vec<FactCheckView> {
        t.checks
            .iter()
            .map(|c| FactCheckView {
                execution: c.exec.clone(),
                target: target.into(),
                version: c.version,
                current: c.version == t.version,
                verdict: c.result.as_ref().map(|r| r.verdict),
                findings: c
                    .result
                    .as_ref()
                    .map(|r| r.findings.clone())
                    .unwrap_or_default(),
            })
            .collect()
    }
    let mut out = track(&s.spec, "spec");
    out.extend(track(&s.plan, "plan"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostra_core::ExecutorKind;
    use ostra_core::event::{
        ExecPurpose, ProjectRef, SessionEvent, SessionOptions, StoredEvent, WorkKind,
    };
    use ostra_core::exec::Usage;
    use ostra_core::ids::{DecisionId, GateId, SessionId};
    use serde_json::json;

    fn log(events: Vec<SessionEvent>) -> Vec<StoredEvent> {
        let t0 = chrono::Utc::now();
        events
            .into_iter()
            .enumerate()
            .map(|(i, event)| StoredEvent {
                seq: i as i64 + 1,
                at: t0 + chrono::Duration::seconds(i as i64),
                event,
            })
            .collect()
    }

    fn created(kind: SessionKind) -> SessionEvent {
        SessionEvent::SessionCreated {
            kind,
            request: "Let customers cancel an order".into(),
            options: SessionOptions::default(),
            projects: vec![ProjectRef {
                key: "backend".into(),
                path: PathBuf::from("/code/backend"),
            }],
            workspace_root: PathBuf::from("/ws"),
            session_root: PathBuf::from("/ws/.ostra/sessions/s1"),
        }
    }

    fn classify_output(title: &str) -> serde_json::Value {
        json!({"category": "RESEARCH", "projects": ["backend"], "explore_tasks": [], "opts_in": {"tests": false, "docs": false}, "reason": "r", "title": title})
    }

    fn started(id: &str, agent: AgentName, purpose: ExecPurpose) -> SessionEvent {
        SessionEvent::ExecutionStarted {
            id: ExecutionId::from(id),
            agent,
            purpose,
            stage: StageKind::Implement,
            project: "backend".into(),
            executor: ExecutorKind::Native,
            model: "m".into(),
            params: json!({}),
            spawn_block: String::new(),
            report_path: None,
            resumes: None,
        }
    }

    fn view(
        id: &str,
        agent: AgentName,
        status: ExecutionStatus,
        at: i64,
        cost: f64,
        executor: ExecutorKind,
    ) -> ExecutionView {
        ExecutionView {
            id: ExecutionId::from(id),
            session: Some(SessionId::from("s1")),
            agent,
            purpose: None,
            stage: None,
            project: "backend".into(),
            executor,
            model: "m".into(),
            status,
            started_at: chrono::DateTime::from_timestamp(at, 0).unwrap(),
            ended_at: None,
            usage: Usage {
                cost_usd: cost,
                ..Default::default()
            },
            report_path: None,
            native_session_id: None,
            spawn_block: String::new(),
            error: None,
            can_resume: false,
            has_terminal: false,
            group: ostra_core::api::execution_group(agent, "backend"),
            run_label: agent.to_string(),
            stream: executor.stream(),
            summary: None,
            has_transcript: false,
            pending_gate: None,
            repo_root: None,
        }
    }

    #[test]
    fn titles_come_from_classify_and_init() {
        let id = SessionId::from("s1");
        let init = SessionState::fold(
            id.clone(),
            &log(vec![created(SessionKind::Init {
                project: "web".into(),
            })]),
        );
        assert_eq!(
            summary(&init, &WorkspaceId::from("w"), 0.0)
                .title
                .as_deref(),
            Some("Initialize web")
        );

        let d = DecisionId::new();
        let mut events = vec![created(SessionKind::Pipeline)];
        assert_eq!(
            SessionState::fold(id.clone(), &log(events.clone())).title,
            None
        );
        events.push(SessionEvent::DecisionMade {
            id: d.clone(),
            judge: JudgeKind::Classify,
            subject: None,
            input_summary: String::new(),
            output: classify_output("  \"Order cancellation.\" "),
            reason: "r".into(),
        });
        assert_eq!(
            SessionState::fold(id.clone(), &log(events.clone()))
                .title
                .as_deref(),
            Some("Order cancellation")
        );
        let over = |title: &str| SessionEvent::DecisionOverridden {
            id: d.clone(),
            output: classify_output(title),
            reason: "user".into(),
        };
        events.push(over("Cancel orders"));
        assert_eq!(
            SessionState::fold(id.clone(), &log(events.clone()))
                .title
                .as_deref(),
            Some("Cancel orders")
        );
        events.push(over(""));
        assert_eq!(
            SessionState::fold(id, &log(events)).title.as_deref(),
            Some("Cancel orders"),
            "an override without a title keeps it"
        );
    }

    #[test]
    fn repeated_runs_are_numbered_per_agent_and_label() {
        let fix = || ExecPurpose::Implement {
            phase: 1,
            work: WorkKind::Fix,
        };
        let review = |iteration| ExecPurpose::Review {
            phase: 1,
            tests: false,
            iteration,
        };
        let s = SessionState::fold(
            SessionId::from("s1"),
            &log(vec![
                created(SessionKind::Pipeline),
                started(
                    "x_1",
                    AgentName::GenerateSpec,
                    ExecPurpose::Spec { round: 1 },
                ),
                started(
                    "x_2",
                    AgentName::GenerateSpec,
                    ExecPurpose::Spec { round: 2 },
                ),
                started(
                    "x_3",
                    AgentName::Implementer,
                    ExecPurpose::Implement {
                        phase: 1,
                        work: WorkKind::Initial,
                    },
                ),
                started("x_4", AgentName::CodeReviewer, review(1)),
                started("x_5", AgentName::Implementer, fix()),
                started("x_6", AgentName::CodeReviewer, review(2)),
                started("x_7", AgentName::Implementer, fix()),
                started(
                    "x_8",
                    AgentName::Implementer,
                    ExecPurpose::Implement {
                        phase: 2,
                        work: WorkKind::Initial,
                    },
                ),
            ]),
        );
        let labels = run_labels(&s);
        let got: Vec<&str> = (1..=8)
            .map(|i| labels[&ExecutionId::from(format!("x_{i}"))].as_str())
            .collect();
        assert_eq!(
            got,
            [
                "Spec",
                "Spec · pass 2",
                "Phase 1",
                "Phase 1 · review pass",
                "Phase 1 · fix pass",
                "Phase 1 · review pass 2",
                "Phase 1 · fix pass 2",
                "Phase 2"
            ]
        );
    }

    #[test]
    fn groups_aggregate_status_and_cost_in_start_order() {
        let native = ExecutorKind::Native;
        let groups = execution_groups(&[
            view(
                "x_3",
                AgentName::Implementer,
                ExecutionStatus::Running,
                30,
                0.5,
                native,
            ),
            view(
                "x_1",
                AgentName::Implementer,
                ExecutionStatus::Ok,
                10,
                1.0,
                native,
            ),
            view(
                "x_2",
                AgentName::CodeReviewer,
                ExecutionStatus::Error,
                20,
                0.25,
                native,
            ),
            view(
                "x_4",
                AgentName::CodeReviewer,
                ExecutionStatus::Ok,
                40,
                0.25,
                native,
            ),
        ]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group, "implementer:backend");
        assert_eq!(
            groups[0].executions,
            [ExecutionId::from("x_1"), ExecutionId::from("x_3")]
        );
        assert_eq!(
            groups[0].status,
            ExecutionStatus::Running,
            "a running run makes the group running"
        );
        assert_eq!(groups[0].cost_usd, 1.5);
        assert_eq!(groups[1].agent, AgentName::CodeReviewer);
        assert_eq!(
            groups[1].status,
            ExecutionStatus::Ok,
            "otherwise the latest run decides"
        );
        let groups = execution_groups(&[
            view(
                "x_1",
                AgentName::Implementer,
                ExecutionStatus::Running,
                10,
                0.0,
                native,
            ),
            view(
                "x_2",
                AgentName::Implementer,
                ExecutionStatus::Error,
                20,
                0.0,
                native,
            ),
        ]);
        assert_eq!(groups[0].status, ExecutionStatus::Running);
    }

    #[test]
    fn tree_nodes_group_runs_with_numbered_labels() {
        let s = SessionState::fold(
            SessionId::from("s1"),
            &log(vec![
                created(SessionKind::Pipeline),
                started(
                    "x_1",
                    AgentName::GenerateSpec,
                    ExecPurpose::Spec { round: 1 },
                ),
                started(
                    "x_2",
                    AgentName::Implementer,
                    ExecPurpose::Implement {
                        phase: 1,
                        work: WorkKind::Initial,
                    },
                ),
                started(
                    "x_3",
                    AgentName::GenerateSpec,
                    ExecPurpose::Spec { round: 2 },
                ),
            ]),
        );
        let mut row = summary(&s, &WorkspaceId::from("w"), 0.0);
        row.cost_usd = 0.5;
        row.title = Some("Cancel orders".into());
        let native = ExecutorKind::Native;
        let mut spec2 = view(
            "x_3",
            AgentName::GenerateSpec,
            ExecutionStatus::Running,
            30,
            0.25,
            native,
        );
        spec2.summary = Some("Write spec.md".into());
        let executions = vec![
            view(
                "x_1",
                AgentName::GenerateSpec,
                ExecutionStatus::Ok,
                10,
                1.0,
                native,
            ),
            view(
                "x_2",
                AgentName::Implementer,
                ExecutionStatus::Ok,
                20,
                0.5,
                ExecutorKind::Harness(ostra_core::HarnessKind::Codex),
            ),
            spec2,
        ];
        let node = tree_session(&s, &row, executions);
        assert_eq!(node.title.as_deref(), Some("Cancel orders"));
        assert_eq!(node.request, "Let customers cancel an order");
        assert_eq!(node.cost_usd, 1.75, "the runs' spend wins over a stale row");
        let groups: Vec<(&str, ExecutionStatus, f64)> = node
            .groups
            .iter()
            .map(|g| (g.group.as_str(), g.status, g.cost_usd))
            .collect();
        assert_eq!(
            groups,
            [
                ("generate-spec:backend", ExecutionStatus::Running, 1.25),
                ("implementer:backend", ExecutionStatus::Ok, 0.5)
            ]
        );
        let runs: Vec<(&str, &str, ExecStream, Option<&str>)> = node.groups[0]
            .runs
            .iter()
            .map(|r| {
                (
                    r.id.as_str(),
                    r.run_label.as_str(),
                    r.stream,
                    r.summary.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            runs,
            [
                ("x_1", "Spec", ExecStream::Activity, None),
                (
                    "x_3",
                    "Spec · pass 2",
                    ExecStream::Activity,
                    Some("Write spec.md")
                )
            ]
        );
        assert_eq!(node.groups[1].runs[0].stream, ExecStream::Terminal);
        assert!(node.artifacts.is_empty());
    }

    #[test]
    fn decorate_numbers_labels_and_finds_transcripts() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = SessionState::new(SessionId::from("s1"));
        s.session_root = tmp.path().to_path_buf();
        let harness = ExecutorKind::Harness(ostra_core::HarnessKind::Claude);
        let mut h = view(
            "x_h",
            AgentName::Implementer,
            ExecutionStatus::Ok,
            1,
            0.0,
            harness,
        );
        let mut n = view(
            "x_n",
            AgentName::Implementer,
            ExecutionStatus::Ok,
            2,
            0.0,
            ExecutorKind::Native,
        );
        let labels =
            HashMap::from([(ExecutionId::from("x_h"), "Phase 1 · fix pass 2".to_string())]);
        decorate(&s, &labels, &mut h);
        assert_eq!(h.run_label, "Phase 1 · fix pass 2");
        assert!(!h.has_transcript);
        let path = paths::terminal_transcript(tmp.path(), "x_h");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"\x1b[1mhello").unwrap();
        decorate(&s, &labels, &mut h);
        assert!(h.has_transcript);
        decorate(&s, &labels, &mut n);
        assert_eq!(n.run_label, "implementer");
        assert!(!n.has_transcript);
        assert_eq!((n.repo_root.clone(), n.pending_gate.clone()), (None, None));
    }

    #[test]
    fn decorate_finds_the_project_folder_and_the_open_gate_of_an_execution() {
        let mut s = SessionState::new(SessionId::from("s1"));
        s.projects.push(ProjectRef {
            key: "backend".into(),
            path: PathBuf::from("/code/backend"),
        });
        let t0 = chrono::DateTime::from_timestamp(100, 0).unwrap();
        let gate = |id: &str, execution: &str, at: i64, answered: bool| crate::state::GateRecord {
            id: GateId::from(id),
            title: format!("Gate {id}"),
            explanation: String::new(),
            payload: GatePayload::ExecutionFailed {
                execution: ExecutionId::from(execution),
                agent: AgentName::Implementer,
                project: "backend".into(),
                error: "boom".into(),
            },
            answer: answered.then(|| ostra_core::event::GateAnswer::Choice {
                option: "retry".into(),
                text: None,
            }),
            source: None,
            reason: None,
            opened_at: t0 + chrono::Duration::seconds(at),
            answered_at: None,
        };
        for g in [
            gate("g_old", "x_1", 1, false),
            gate("g_new", "x_1", 2, false),
            gate("g_done", "x_1", 3, true),
            gate("g_other", "x_2", 4, false),
        ] {
            s.gates.insert(g.id.clone(), g);
        }
        let mut e = view(
            "x_1",
            AgentName::Implementer,
            ExecutionStatus::Error,
            1,
            0.0,
            ExecutorKind::Native,
        );
        decorate(&s, &HashMap::new(), &mut e);
        assert_eq!(e.repo_root, Some(PathBuf::from("/code/backend")));
        let pending = e.pending_gate.expect("an open gate names x_1");
        assert_eq!(
            (
                pending.id.as_str(),
                pending.kind.as_str(),
                pending.title.as_str()
            ),
            ("g_new", "execution_failed", "Gate g_new")
        );
        let mut quiet = view(
            "x_3",
            AgentName::Implementer,
            ExecutionStatus::Ok,
            1,
            0.0,
            ExecutorKind::Native,
        );
        decorate(&s, &HashMap::new(), &mut quiet);
        assert_eq!(quiet.pending_gate, None);
    }
}
