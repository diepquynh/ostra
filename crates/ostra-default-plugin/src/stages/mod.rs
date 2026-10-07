//! The built-in stages, one folder each: its fold, its planner rules, and its own helpers. Code
//! that several stages use lives in `fold/`, `planner/`, `judge_input/`, `view/`, and `pipeline/`.

pub mod book;
pub mod build;
pub mod closing;
pub mod feedback;
pub mod init;
pub mod plan;
pub mod research;
pub mod spec;
