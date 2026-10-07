//! The feedback stage's answers to the engine's `Pipeline` hooks: the session context file.

use ostra_core::event::GatePayload;
use ostra_core::ids::SessionId;
use ostra_engine::plan::Step;
use ostra_engine::runner::{EngineError, StepHost};
use ostra_engine::state::SessionState;

/// Rule F2: the session context file is rewritten from the fold before anything reads it.
pub(crate) fn write_session_context(st: &SessionState) -> Result<(), EngineError> {
    let path = st.session_context_path();
    std::fs::write(&path, crate::context::render(st))
        .map_err(|e| EngineError::Invalid(format!("Could not write {}: {e}", path.display())))
}

/// Rule F2: the implementation review gate opens after the context file is current. Returns the
/// step for the engine to open, or the error.
pub(crate) fn perform(
    host: &StepHost,
    session: &SessionId,
    step: Step,
) -> Result<Result<(), EngineError>, Box<Step>> {
    match step {
        Step::OpenGate {
            payload: GatePayload::ImplementationReview { .. },
            ..
        } => match host
            .snapshot(session)
            .and_then(|st| write_session_context(&st))
        {
            Ok(()) => Err(Box::new(step)),
            Err(e) => Ok(Err(e)),
        },
        other => Err(Box::new(other)),
    }
}
