//! Ostra workspaces: settings and their validation, projects, the approvals that keep folder files
//! from starting programs, creating and deleting a workspace, and the runtime of an open one.

pub mod create;
pub mod host;
pub mod projects;
pub mod runtime;
pub mod settings;
pub mod trust;
pub mod ui_state;

pub use create::{CreateError, Draft, DraftCtx};
pub use host::WorkspaceHost;
pub use runtime::{DeleteError, RemoveProjectError, STARTING, WorkspaceRt, unregister};
