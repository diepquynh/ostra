//! Rule PL4: Ostra's own plugin `ostra`. Its definitions (the built-in agents and the default
//! workflows) come from `ostra-standard`, re-exported here. This crate adds the pipeline: the
//! built-in stages' state, state changes, planning, judges, views, and step effects, which the
//! engine reaches only through its `Pipeline` trait.

pub use ostra_standard::*;

pub mod autofix;
pub mod book;
pub mod context;
pub mod data;
pub mod docs_scan;
pub mod factory;
pub mod fold;
pub mod init;
pub mod inputs;
pub mod judge;
pub mod judge_input;
mod pipeline;
pub mod planner;
pub mod steps;
pub mod view;

pub use pipeline::{FORMAT_NOT_APPROVED, OstraPipeline};

/// The extension traits that hold the built-in stages' logic over the engine's types.
pub mod prelude {
    pub use crate::data::*;
    pub use crate::fold::{OstraEvents, OstraFold, RecurringTrack, RoutingTrack, WorkLoopFold};
    pub use crate::planner::{OstraFlows, OstraPlanner};
}

/// The standard pipeline, for an engine's `Services::pipeline`.
pub fn pipeline() -> std::sync::Arc<dyn ostra_engine::pipeline::Pipeline> {
    std::sync::Arc::new(OstraPipeline)
}

/// Fold a session's events with the standard pipeline.
pub fn fold_session(
    id: ostra_core::ids::SessionId,
    events: &[ostra_core::event::StoredEvent],
) -> ostra_engine::state::SessionState {
    ostra_engine::state::SessionState::fold(pipeline().into(), id, events)
}

/// Install the docs stage's check for the deprecated `ostra_core::book::check_documentation`.
/// Submit tools reach the same check through the pipeline, so nothing else needs this call.
pub fn install() {
    book::install_checks();
}
