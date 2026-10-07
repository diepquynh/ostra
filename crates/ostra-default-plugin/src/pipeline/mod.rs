//! The standard pipeline: the engine's `Pipeline` hooks, each answered by the built-in stages'
//! code in this crate.

mod effects;
mod hooks;

pub use effects::*;
pub use hooks::*;
