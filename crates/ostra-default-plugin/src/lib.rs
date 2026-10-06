//! Rule PL4: Ostra's own plugin `ostra`. Its definitions (the built-in agents and the default
//! workflows) come from `ostra-standard`, re-exported here. This crate adds the pipeline: the
//! built-in stages' state changes, planning, judges, views, and step effects, which the engine
//! reaches only through its `Pipeline` trait.

pub use ostra_standard::*;

pub mod autofix;
pub mod book;
pub mod context;
pub mod docs_scan;
pub mod factory;
pub mod fold;
pub mod init;
pub mod judge;
pub mod judge_input;
mod pipeline;
pub mod planner;
pub mod view;

pub use pipeline::OstraPipeline;

/// The extension traits that hold the built-in stages' logic over the engine's types.
pub mod prelude {
    pub use crate::fold::{OstraEvents, OstraFold, RecurringTrack, RoutingTrack, WorkLoopFold};
    pub use crate::planner::{OstraFlows, OstraPlanner};
}

/// Install the standard pipeline into the engine. Call it at startup, before any session folds.
/// A second call changes nothing.
pub fn install() {
    book::install_checks();
    ostra_engine::pipeline::install(std::sync::Arc::new(OstraPipeline));
}
