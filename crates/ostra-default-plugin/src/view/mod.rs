//! Projections of session state for the browser: the summary, the board, the artifacts, and the
//! tree, assembled from each stage's `stages/<stage>/view.rs`.

mod board;
mod runs;
mod session;
#[cfg(test)]
mod tests;

pub use crate::stages::build::view::{file_changes, phases};
pub use board::*;
pub use runs::*;
pub use session::*;
