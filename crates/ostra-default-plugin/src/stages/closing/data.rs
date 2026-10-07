//! The closing stage's state: the path analysis of a phase.

#[allow(unused_imports)]
use crate::data::*;
use ostra_core::ids::{ExecutionId, GateId};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum EpaState {
    NotStarted,
    Running(ExecutionId),
    Done(PathBuf),
    Failed {
        exec: ExecutionId,
        error: String,
        gate: Option<GateId>,
        retries: u32,
    },
    Abandoned,
}
