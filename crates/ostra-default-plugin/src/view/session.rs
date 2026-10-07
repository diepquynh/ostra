//! The session's summary, detail, and tree, assembled from the built-in stages' state.

use super::*;
#[allow(unused_imports)]
use crate::book::DocsTrack;
use crate::data::{EpaState, LoopNext};
use crate::planner::removed_phases;
use crate::prelude::*;
use ostra_core::api::{
    ContextAddition, DecisionView, ExecutionGroupView, ExecutionView, FactCheckView, GateView,
    PhaseStatus, PhaseView, SessionDetail, SessionStatus, SessionSummary, StageCard, StageStatus,
    TreeGroup, TreeRun, TreeSession,
};
use ostra_core::event::CommandPurpose;
use ostra_core::exec::ExecutionStatus;
use ostra_core::ids::{ExecutionId, WorkspaceId};
use ostra_core::pipeline::{Lane, StageKind};
use ostra_engine::runner::EngineError;
use ostra_engine::state::SessionState;
use ostra_store::WorkspaceDb;
use std::collections::{HashMap, HashSet};

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
    } else if s.paused {
        SessionStatus::Paused
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
    // Rule P3: say why Ostra paused it, because nothing else on the board does.
    if let Some(rec) = s.contained.as_ref().and_then(|id| s.executions.get(id)) {
        return (
            rec.stage.lane(),
            format!(
                "Paused: read the {} execution's Activity, because it tripped {} containment signals",
                rec.agent.as_str().replace('-', " "),
                ostra_core::containment::PAUSE_AFTER
            ),
        );
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
    if let Some((key, purpose)) = s
        .ext
        .os()
        .project_tracks
        .iter()
        .find_map(|(k, t)| t.running.as_ref().map(|(p, _)| (k, *p)))
    {
        let (stage, label) = match purpose {
            CommandPurpose::Format => (StageKind::Format, format!("Formatting {key}")),
            CommandPurpose::Stage | CommandPurpose::Autofix => (
                StageKind::Staging,
                format!("Staging reviewed files in {key}"),
            ),
        };
        return (stage.lane(), label);
    }
    if s.category.is_none() {
        return (Lane::Research, "Classifying the request".into());
    }
    (Lane::Research, "Working".into())
}

pub(crate) fn first(s: &str) -> String {
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
        Track => "Choose the track",
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
        ImplementationReview => "Review the implementation",
        ClosingGate => "Tests and docs?",
        Epa => "Trace execution paths",
        WriteTest => "Write tests",
        TestReview => "Review tests",
        Documentation => "Write documentation",
        BookWrite => "Write the book",
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
        Custom => "Workflow stage",
    }
}

pub(crate) fn completion_card(s: &SessionState) -> StageCard {
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
    s.ext
        .os()
        .phases
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
                s.executions.get(r).is_some_and(|e| {
                    matches!(e.purpose, ostra_core::event::ExecPurpose::Review { .. })
                })
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
                    project: (!e.spans_session).then(|| e.project.clone()),
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
        files: s.files.clone(),
        uploads: s.uploads.clone(),
        additions: s
            .amendments
            .iter()
            .map(|a| ContextAddition {
                text: a.text.clone(),
                files: a.files.clone(),
                uploads: a.uploads.clone(),
                delivery: a.delivery,
                at: a.at,
                queued: a.held,
                withdrawn: a.withdrawn,
            })
            .collect(),
    })
}

pub fn fact_checks(s: &SessionState) -> Vec<FactCheckView> {
    fn track<T>(t: &crate::data::ArtifactTrack<T>, target: &str) -> Vec<FactCheckView> {
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
    let mut out = track(&s.ext.os().spec, "spec");
    out.extend(track(&s.ext.os().plan, "plan"));
    out
}
