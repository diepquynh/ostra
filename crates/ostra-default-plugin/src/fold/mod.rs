//! The built-in stages' part of the fold (HANDOVER 8.1): the dispatchers that hand each event to
//! its stage in `stages/`, and the helpers several stages use. The engine calls it through
//! `Pipeline` at the point of `SessionState::apply` where it ran before, so the fold stays one
//! deterministic function of the log.

mod answers;
mod decisions;
mod events;
mod executions;
mod gates;
mod shared;

pub use crate::stages::book::fold::*;
pub use crate::stages::build::loops::*;
pub use crate::stages::feedback::fold::*;
pub use crate::stages::init::fold::*;
pub use crate::stages::research::fold::*;
pub use answers::*;
pub use decisions::*;
pub use events::*;
pub use executions::*;
pub use gates::*;
pub use shared::*;
