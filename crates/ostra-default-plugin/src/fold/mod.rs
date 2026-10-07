//! The built-in stages' part of the fold (HANDOVER 8.1): how each event changes the state of
//! research, the spec, the plan, the phases, the closing stages, and the init flow. The engine
//! calls it through `Pipeline` at the point of `SessionState::apply` where it ran before, so the
//! fold stays one deterministic function of the log.

mod answers;
mod book;
mod decisions;
mod events;
mod executions;
mod feedback;
mod gates;
mod init;
mod loops;
mod research;
mod shared;

pub use answers::*;
pub use book::*;
pub use decisions::*;
pub use events::*;
pub use executions::*;
pub use feedback::*;
pub use gates::*;
pub use init::*;
pub use loops::*;
pub use research::*;
pub use shared::*;
