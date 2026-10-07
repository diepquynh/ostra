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
    /// Rule CA1: the agents the workspace at `root` can run, built-in and custom, with every
    /// problem in a custom definition.
    fn agents(
        &self,
        root: &Path,
    ) -> (
        ostra_agents::AgentCatalog,
        Vec<ostra_agents::catalog::CatalogIssue>,
    ) {
        ostra_agents::AgentCatalog::load(Some(root), &[], &[])
    }
    /// Rule PL3: the plugin stages the workspace at `root` can run, as `(plugin, stage)`.
    fn plugin_stages(&self, _root: &Path) -> Vec<(String, String)> {
        vec![]
    }
    /// Rules PL6 and PL7: the manifests of the plugins that run for the workspace at `root`.
    fn plugin_manifests(&self, _root: &Path) -> Vec<(String, ostra_core::plugin::PluginManifest)> {
        vec![]
    }
    /// Rule AG3: the workspace's plugins, built in and programs, with what each is doing.
    fn plugin_infos(
        &self,
        _root: &Path,
        _settings: &ostra_core::config::WorkspaceSettings,
    ) -> Vec<ostra_core::api::PluginInfo> {
        vec![]
    }
    /// Rule PL1: plugin programs of the workspace at `root` that should run and do not.
    fn plugin_issues(
        &self,
        _root: &Path,
        _settings: &ostra_core::config::WorkspaceSettings,
    ) -> Vec<ostra_core::config::ValidationIssue> {
        vec![]
    }
}
