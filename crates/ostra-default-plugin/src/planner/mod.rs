//! The built-in stages' part of the planner: research, the Track and Stakes judges, the spec and
//! plan with their fact-checks and approvals, the build's phase loops, the user's review, the
//! closing stages, the init flow, and completion. The engine's planner calls it through
//! `Pipeline`; it stays a pure function of the session state.

mod book;
mod build;
mod closing;
mod feedback;
mod plan;
mod research;
mod shared;
mod spec;

pub use book::*;
pub use build::*;
pub use closing::*;
pub use feedback::*;
pub use plan::*;
pub use research::*;
pub use shared::*;
pub use spec::*;
