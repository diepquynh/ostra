//! The init flow's answers to the engine's `Pipeline` hooks: the init of a created project, the
//! hold on a project until its init ends, and the init's end.

use crate::data::InitTrack;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::steps::OstraStep;
use ostra_core::config::ProjectProfile;
use ostra_core::event::{ExecPurpose, SessionEvent, SessionKind};
use ostra_core::ids::{ExecutionId, SessionId};
use ostra_core::manage::CreatedProject;
use ostra_core::paths;
use ostra_engine::plan::{SpawnRequest, Step};
use ostra_engine::runner::{EngineError, StepHost};
use ostra_engine::state::SessionState;
use std::path::Path;

/// An init session starts with its project's init track.
pub(crate) fn created(s: &mut SessionState) {
    if let SessionKind::Init { project } = &s.kind {
        s.ext.os_mut().init = Some(InitTrack {
            project: project.clone(),
            ..Default::default()
        });
    }
}

/// Rule O4: a project an agent created gets an init, run when the build starts.
pub(crate) fn project_created(s: &mut SessionState, project: &CreatedProject) {
    s.ext.os_mut().project_inits.insert(
        project.key.clone(),
        InitTrack {
            project: project.key.clone(),
            ..Default::default()
        },
    );
}

pub(crate) fn holds(s: &SessionState, step: &Step) -> bool {
    // Rule O4: nothing but its init and the advisor runs in a created project until the init
    // ends, because every other agent routes its work by the project's inventory and profile.
    match step {
        Step::Spawn(r) => {
            !matches!(
                r.purpose,
                ExecPurpose::Init { .. } | ExecPurpose::Advise { .. }
            ) && s.awaiting_init(&r.project)
        }
        _ => OstraStep::of(step)
            .and_then(|o| o.project().map(|p| s.awaiting_init(p)))
            .unwrap_or(false),
    }
}

/// An init session ends only with a usable inventory and profile.
pub(crate) fn completing(
    host: &StepHost,
    session: &SessionId,
    st: &SessionState,
) -> Result<bool, EngineError> {
    if let SessionKind::Init { project } = &st.kind {
        // Later executions route by these two files, so a broken one fails the init
        // here rather than silently giving every agent an empty profile.
        let root = st.project_path(project).unwrap_or_default();
        if let Some(p) = init_problem(&root) {
            host.append(
                session,
                SessionEvent::SessionFailed {
                    error: format!("Init did not finish: {p}. Run init again."),
                },
            )?;
            return Ok(true);
        }
        let _ = host
            .db()
            .set_project_init_status(project, ostra_core::api::InitStatus::Initialized);
    }
    Ok(false)
}

/// A generated skill runs on the advanced tier.
pub(crate) fn tier_override(req: &SpawnRequest) -> Option<ostra_core::model::Tier> {
    matches!(
        req.purpose,
        ExecPurpose::Init {
            mode: ostra_core::agent::InitializerMode::GenerateSkill,
            ..
        }
    )
    .then_some(ostra_core::model::Tier::Advanced)
}

/// Rule O4: a created project's init ends, or records why it cannot.
pub(crate) fn finish_init(
    host: &StepHost,
    session: &SessionId,
    project: String,
) -> Result<(), EngineError> {
    let st = host.snapshot(session)?;
    let root = st.project_path(&project).unwrap_or_default();
    let inventory = st
        .ext
        .os()
        .project_inits
        .get(&project)
        .and_then(|i| i.inventory.clone());
    match (init_problem(&root), inventory) {
        (Some(p), Some(execution)) => {
            host.append(
                session,
                SessionEvent::InitStepFailed {
                    project,
                    execution,
                    error: format!("The init did not finish: {p}."),
                },
            )?;
        }
        _ => {
            let _ = host
                .db()
                .set_project_init_status(&project, ostra_core::api::InitStatus::Initialized);
            host.projects_changed();
            host.append(session, SessionEvent::ProjectInitFinished { project })?;
        }
    }
    Ok(())
}

pub(crate) fn record_problem(
    host: &StepHost,
    session: &SessionId,
    project: String,
    execution: ExecutionId,
    error: String,
) -> Result<(), EngineError> {
    host.append(
        session,
        SessionEvent::InitStepFailed {
            project,
            execution,
            error,
        },
    )?;
    Ok(())
}

/// Why an init left the project without a usable inventory or profile.
pub(crate) fn init_problem(root: &Path) -> Option<String> {
    if !paths::project_inventory(root).exists() {
        return Some("the initializer did not write .ostra/INVENTORY.md".into());
    }
    ostra_core::config::load_toml_required::<ProjectProfile>(&paths::project_profile(root))
        .err()
        .map(|e| format!("the generated .ostra/project.toml is not valid: {e}"))
}
