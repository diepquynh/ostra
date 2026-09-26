//! SQLite storage: the workspace database (event log and its materialized tables), the machine
//! registry, and each project's lesson memory.

pub mod memory;
pub mod registry;
pub mod secrets;
mod util;
pub mod workspace;

pub use memory::MemoryStore;
pub use registry::{RegistryDb, StoredPushSubscription, WorkspaceRecord};
pub use workspace::{
    ExecutionOutput, NewExecution, NewSession, ProjectRow, SessionUpdate, StoredMessage, TextHit,
    ToolCallRecord, WorkspaceDb, fts_prefix_query,
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("the registry has no encryption key, so credentials cannot be read or saved")]
    Locked,
}
