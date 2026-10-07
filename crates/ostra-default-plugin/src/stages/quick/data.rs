//! The quick answer's state.

#[allow(unused_imports)]
use crate::data::*;
use ostra_core::ids::ExecutionId;
use ostra_core::submit::QuickAnswerSubmit;

#[derive(Debug, Clone, Default)]
pub struct QuickTrack {
    pub exec: Option<ExecutionId>,
    pub running: bool,
    pub answer: Option<QuickAnswerSubmit>,
    pub failed: Option<String>,
}
