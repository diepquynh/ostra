//! The effects of the built-in stages' own steps. Each step goes to the stage that owns it; the
//! format and stage commands serve the build and closing stages alike (`commands.rs`).

use crate::stages::{book, build, feedback, init};
use crate::steps::OstraStep;
use ostra_core::ids::SessionId;
use ostra_engine::plan::Step;
use ostra_engine::runner::{EngineError, StepHost};

/// `Pipeline::perform`: the pipeline's own steps, and the steps it prepares before the engine
/// performs them. A step it gives back is the engine's.
pub(crate) async fn perform(
    host: &StepHost,
    session: &SessionId,
    step: Step,
) -> Result<Result<(), EngineError>, Box<Step>> {
    if let Some(own) = OstraStep::of(&step) {
        return Ok(perform_own(host, session, own).await);
    }
    feedback::hooks::perform(host, session, step)
}

async fn perform_own(
    host: &StepHost,
    session: &SessionId,
    step: OstraStep,
) -> Result<(), EngineError> {
    match step {
        OstraStep::Autofix {
            project,
            phase,
            tests,
            findings,
        } => build::hooks::autofix(host, session, project, phase, tests, findings).await,
        OstraStep::AnnounceBlocked {
            project,
            phase,
            tests,
            reason,
        } => build::hooks::announce_blocked(host, session, project, phase, tests, reason),
        OstraStep::FinishInit { project } => init::hooks::finish_init(host, session, project),
        OstraStep::RecordInitProblem {
            project,
            execution,
            error,
        } => init::hooks::record_problem(host, session, project, execution, error),
        OstraStep::ScanDocs { project } => book::hooks::scan_docs(host, session, project).await,
        OstraStep::Command {
            purpose,
            project,
            command,
            files,
        } => {
            super::commands::perform_command(host, session, purpose, &project, command, files).await
        }
    }
}
