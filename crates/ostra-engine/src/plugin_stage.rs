//! Plugin stages (HANDOVER 10.10): a plugin's stage logic decides each next step of its workflow
//! stage, one decision at a time. The planner asks for a decision with `Step::DecideStage`, the
//! runner records the plugin's answer as a `StageDecided` event, and the fold applies it.

use crate::plan::{Planner, SpawnRequest, Step};
use crate::state::SessionState;
use crate::workflow::{StageNext, StageOutcome, StageTrack, stage_scopes};
use ostra_core::event::GatePayload;
use ostra_core::exec::ExecutionStatus;
use ostra_core::plugin::{StageRunView, StageView};
use ostra_core::submit::StageVerdict;
use ostra_core::workflow::{StageDef, StageRun, WorkflowDef};

/// Rule PL3: the next step of each instance of a plugin stage, or `true` when all are done.
pub fn plan(p: &mut Planner<'_>, wf: &WorkflowDef, d: &StageDef) -> bool {
    let s = p.session();
    let mut done = true;
    for scope in stage_scopes(s, d) {
        let t = s
            .stage_track(&d.id, scope.as_deref())
            .cloned()
            .unwrap_or_default();
        match &t.outcome {
            Some(StageOutcome::Passed | StageOutcome::FailedOn | StageOutcome::Skipped) => {
                continue;
            }
            Some(StageOutcome::Stopped(why)) => {
                p.push_step(Step::Fail { error: why.clone() });
                done = false;
                continue;
            }
            None => {}
        }
        done = false;
        // Rule WB5: a plugin stage whose conditions do not hold is never asked.
        if let Some(step) = crate::workflow::skip_step(s, d, scope.as_deref()) {
            p.push_step(step);
            continue;
        }
        if t.gate.is_some() {
            continue;
        }
        let live = t
            .runs
            .last()
            .and_then(|id| s.executions.get(id))
            .is_some_and(|r| {
                r.result
                    .as_ref()
                    .is_none_or(|x| x.status == ExecutionStatus::Waiting)
                    // Rule PL5: a plugin contract's result is read only once its plugin handled it.
                    || s.results_due().contains(&r.id)
            });
        if live {
            continue;
        }
        if t.next == Some(StageNext::Gate) {
            p.push_step(gate(d, scope.as_deref(), &t));
            continue;
        }
        if let Some((agent, instructions)) = &t.pending_run {
            match agent.parse() {
                Ok(agent) => p.push_step(Step::Spawn(Box::new(spawn(
                    s,
                    wf,
                    d,
                    agent,
                    scope.as_deref(),
                    t.runs.len() as u32 + 1,
                    instructions.clone(),
                    &t,
                )))),
                Err(_) => p.push_step(Step::Fail {
                    error: format!(
                        "Stage `{}`: the plugin named agent `{agent}`, which is not an agent name.",
                        d.id
                    ),
                }),
            }
            continue;
        }
        let needs = t.decisions.is_empty()
            || t.runs.len() > t.decided_runs
            || matches!(t.next, Some(StageNext::Retry(_)));
        if needs {
            p.push_step(Step::DecideStage {
                node: d.id.clone(),
                scope: scope.clone(),
                seq: t.decisions.len() as u32,
            });
        }
    }
    done
}

fn gate(d: &StageDef, scope: Option<&str>, t: &StageTrack) -> Step {
    let (verdict, question, options, summary) = match &t.question {
        Some((q, o)) => (
            StageVerdict::NeedsUser,
            Some(q.clone()),
            o.clone(),
            q.clone(),
        ),
        None => (
            StageVerdict::Fail,
            None,
            vec![],
            t.last
                .as_ref()
                .map(|c| c.summary.clone())
                .unwrap_or_else(|| "The stage failed.".into()),
        ),
    };
    let last = t.runs.last().cloned();
    Step::OpenGate {
        title: match verdict {
            StageVerdict::NeedsUser => format!("Stage {} asks you", d.id),
            _ => format!("Stage {} failed", d.id),
        },
        explanation: match verdict {
            StageVerdict::NeedsUser => format!(
                "The stage's plugin needs a decision before the workflow goes on: {summary}"
            ),
            _ => format!(
                "The stage's plugin reports a failure. Let it decide again, continue the workflow without the stage, or stop the session. {summary}"
            ),
        },
        payload: GatePayload::StageReview {
            stage: d.id.clone(),
            scope: scope.map(String::from),
            agent: None,
            execution: last,
            verdict,
            summary,
            findings: t
                .last
                .as_ref()
                .map(|c| c.findings.clone())
                .unwrap_or_default(),
            question,
            options,
            round: t.runs.len() as u32,
            max_rounds: d.max_rounds,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn(
    s: &SessionState,
    wf: &WorkflowDef,
    d: &StageDef,
    agent: ostra_core::AgentName,
    scope: Option<&str>,
    round: u32,
    instructions: Option<String>,
    t: &StageTrack,
) -> SpawnRequest {
    let mut req = crate::workflow::stage_request(s, wf, d, agent, scope, round, None, &t.notes);
    req.inputs.instructions = match (instructions, &d.instructions) {
        (Some(i), Some(n)) => Some(format!("{n}\n\n{i}")),
        (Some(i), None) => Some(i),
        (None, n) => n.clone(),
    };
    req
}

/// Rule PL3: what the plugin is shown when it decides.
pub fn stage_view(
    s: &SessionState,
    node: &str,
    scope: Option<&str>,
) -> Option<(String, String, StageView)> {
    let wf = s.workflow.as_ref()?;
    let d = wf.stage(node)?;
    let StageRun::Plugin { plugin, stage } = &d.run else {
        return None;
    };
    let t = s.stage_track(node, scope).cloned().unwrap_or_default();
    let runs = t
        .runs
        .iter()
        .filter_map(|id| s.executions.get(id))
        .map(|r| StageRunView {
            execution: r.id.clone(),
            agent: r.agent.to_string(),
            status: r
                .result
                .as_ref()
                .map(|x| x.status)
                .unwrap_or(ExecutionStatus::Running),
            submit: r.result.as_ref().and_then(|x| x.submit.clone()),
            error: r.result.as_ref().and_then(|x| x.error.clone()),
            handled: r
                .handled
                .as_ref()
                .and_then(|h| serde_json::to_value(h).ok()),
        })
        .collect();
    let req: SpawnRequest = crate::workflow::stage_request(
        s,
        wf,
        d,
        s.default_agent(ostra_core::Contract::Research),
        scope,
        1,
        None,
        &t.notes,
    );
    Some((
        plugin.clone(),
        stage.clone(),
        StageView {
            session: s.id.clone(),
            node: node.into(),
            stage: stage.clone(),
            scope: scope.map(String::from),
            request: s.full_request(),
            workspace_root: s.workspace_root.clone(),
            projects: s
                .projects
                .iter()
                .filter(|p| s.scope.contains(&p.key))
                .map(|p| (p.key.clone(), p.path.clone()))
                .collect(),
            instructions: d.instructions.clone(),
            inputs: s.node_inputs(d, scope),
            decisions: t.decisions.clone(),
            runs,
            answers: t.notes.clone(),
            earlier_stages: req.inputs.earlier_stages,
            spec_file: req.inputs.spec_file,
            master_plan: req.inputs.target,
        },
    ))
}
