//! The build stage's answers to the engine's `Pipeline` hooks: phase facts, the review's
//! auto-fixable rules and security block, and the autofix and blocked-phase steps.

use crate::data::AUTO_FIXABLE_PARAM;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::config::ProjectProfile;
use ostra_core::event::{ExecPurpose, SessionEvent};
use ostra_core::exec::ExecutionResult;
use ostra_core::ids::SessionId;
use ostra_engine::pipeline::PhaseFacts;
use ostra_engine::plan::SpawnRequest;
use ostra_engine::runner::{EngineError, StepHost};
use ostra_engine::state::SessionState;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The loop a run belongs to: a prompt generator joins the loop of the run it took over from.
pub(crate) fn loop_key(s: &SessionState, purpose: &ExecPurpose) -> Option<(u32, bool)> {
    match purpose {
        ExecPurpose::PromptGen {
            handoff_for: Some(x),
        } => s.executions.get(x).and_then(|r| r.loop_key),
        ExecPurpose::PromptGen { handoff_for: None } => Some((1, false)),
        other => crate::fold::purpose_loop(other),
    }
}

pub(crate) fn phase(s: &SessionState, id: u32) -> Option<PhaseFacts> {
    s.ext.os().phases.get(&id).map(|p| PhaseFacts {
        info: p.info.clone(),
        implementer_report: p.implementer_report.clone(),
    })
}

pub(crate) fn passed_phases(s: &SessionState) -> Vec<u32> {
    let removed = crate::planner::removed_phases(s);
    s.ext
        .os()
        .phases
        .values()
        .filter(|p| !removed.contains(&p.info.id) && p.impl_loop.is_done())
        .map(|p| p.info.id)
        .collect()
}

pub(crate) fn changed_files(
    s: &SessionState,
    project: &str,
    phase: Option<u32>,
) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    for p in s
        .ext
        .os()
        .phases
        .values()
        .filter(|p| p.info.projects().iter().any(|k| k == project))
        .filter(|p| phase.is_none_or(|n| n == p.info.id))
    {
        let changed = p.impl_loop.changed.iter().chain(&p.test_loop.changed);
        if let Some(mine) = files_by_project(s, &p.info.project, changed).remove(project) {
            files.extend(mine);
        }
    }
    files
}

/// Rule WD2: a multi-project phase's changed files, by the project that holds each one. A
/// relative path is in `main`. An absolute path is in the project whose folder holds it, and is
/// made relative to that folder.
pub(crate) fn files_by_project<'f>(
    s: &SessionState,
    main: &str,
    files: impl IntoIterator<Item = &'f String>,
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in files {
        let path = std::path::Path::new(f);
        if !path.is_absolute() {
            out.entry(main.to_string()).or_default().push(f.clone());
            continue;
        }
        // The deepest folder wins, because one project can sit inside another.
        let owner = s
            .projects
            .iter()
            .filter_map(|p| Some((p, path.strip_prefix(&p.path).ok()?)))
            .max_by_key(|(p, _)| p.path.components().count());
        if let Some((p, rel)) = owner {
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.entry(p.key.clone()).or_default().push(rel);
        }
    }
    out
}

/// A review run records the project's auto-fixable rule IDs, because the fold cannot read
/// `project.toml` (pattern 1).
pub(crate) fn spawn_params(
    req: &SpawnRequest,
    profile: Option<&ProjectProfile>,
    params: &mut Value,
) {
    if matches!(req.purpose, ExecPurpose::Review { .. })
        && let Value::Object(map) = params
    {
        let ids = profile.map(|p| p.auto_fixable_ids()).unwrap_or_default();
        map.insert(
            AUTO_FIXABLE_PARAM.into(),
            serde_json::to_value(ids).unwrap_or_default(),
        );
    }
    // Rule WD3: a plan run records the single-project agents, because the fold and the submit
    // check read no settings.
    if matches!(req.purpose, ExecPurpose::Plan { .. })
        && let Value::Object(map) = params
    {
        let single = crate::inputs::OstraInputs::of(&req.inputs).single_project_agents;
        map.insert(
            crate::data::SINGLE_PROJECT_PARAM.into(),
            serde_json::to_value(single).unwrap_or_default(),
        );
    }
}

/// A review with a BLOCKER finding records a security block.
pub(crate) fn after_run(req: &SpawnRequest, result: &ExecutionResult) -> Vec<SessionEvent> {
    let block = matches!(req.purpose, ExecPurpose::Review { .. })
        .then(|| {
            result.submit.as_ref().and_then(|v| {
                serde_json::from_value::<ostra_core::submit::CodeReviewerSubmit>(v.clone()).ok()
            })
        })
        .flatten()
        .filter(|r| r.security_block);
    match (block, &req.purpose) {
        (Some(review), ExecPurpose::Review { phase, tests, .. }) => {
            vec![SessionEvent::SecurityBlock {
                project: req.project.clone(),
                phase: *phase,
                tests: *tests,
                findings: review
                    .findings
                    .into_iter()
                    .filter(|f| f.severity == ostra_core::submit::Severity::Blocker)
                    .collect(),
            }]
        }
        _ => vec![],
    }
}

pub(crate) async fn autofix(
    host: &StepHost,
    session: &SessionId,
    project: String,
    phase: u32,
    tests: bool,
    findings: Vec<ostra_core::submit::ReviewFinding>,
) -> Result<(), EngineError> {
    let root = host
        .snapshot(session)?
        .project_path(&project)
        .unwrap_or_default();
    let (applied, failed) =
        tokio::task::spawn_blocking(move || super::autofix::apply_findings(&root, &findings))
            .await
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
    host.append(
        session,
        SessionEvent::AutofixApplied {
            project,
            phase,
            tests,
            applied,
            failed,
        },
    )?;
    Ok(())
}

pub(crate) fn announce_blocked(
    host: &StepHost,
    session: &SessionId,
    project: String,
    phase: u32,
    tests: bool,
    reason: String,
) -> Result<(), EngineError> {
    host.append(
        session,
        SessionEvent::PhaseBlocked {
            project,
            phase,
            tests,
            reason,
        },
    )?;
    Ok(())
}
