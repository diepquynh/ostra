//! The standard pipeline: the engine's `Pipeline` hooks (`hooks.rs`), each answered by the stage
//! that owns it in `stages/<stage>/` or by the shared code here: step effects, the format and
//! stage commands, judges, spawn files, and message partners.

mod commands;
mod effects;
mod hooks;
mod judges;
pub(crate) mod partners;
mod spawn;

pub use commands::FORMAT_NOT_APPROVED;
pub use hooks::OstraPipeline;
