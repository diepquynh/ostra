//! What a workspace needs from the process that opens it.

use ostra_core::api::{HarnessStatus, ProviderStatus};
use ostra_core::config::{Environment, GlobalConfig};
use ostra_core::ids::WorkspaceId;
use ostra_engine::Services;
use ostra_store::RegistryDb;
use std::path::Path;
use std::sync::Arc;

pub trait WorkspaceHost: Send + Sync {
    /// Holds approvals, the permission mode, YOLO, and sealed secrets, which never come from a
    /// folder file.
    fn registry(&self) -> &RegistryDb;
    /// Re-read on every call, so edits apply without a restart.
    fn global(&self) -> GlobalConfig;
    /// Machine facts that settings validation needs.
    fn environment(&self) -> Environment;
    fn providers(&self) -> Vec<ProviderStatus>;
    fn harnesses(&self) -> Vec<HarnessStatus>;
    /// The engine's services for the workspace at `root`.
    fn services(self: Arc<Self>, id: &WorkspaceId, root: &Path) -> Arc<dyn Services>;
}
