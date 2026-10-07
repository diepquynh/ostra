//! The spec stage: the spec, its fact-check, and its approval (Rules D3 to D5).

pub mod gates;
pub mod planner;
pub mod runs;
pub mod view;

pub use gates::*;
pub use planner::*;
pub use runs::*;
