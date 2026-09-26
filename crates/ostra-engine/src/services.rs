//! What the engine needs from the rest of Ostra. The server implements these; tests use fakes.

use crate::plan::SpawnRequest;
use crate::state::SessionState;
use ostra_core::agent::{AgentName, Capability};
use ostra_core::config::{GlobalConfig, ProjectProfile, ResolvedRoute, WorkspaceSettings};
use ostra_core::exec::{Executor, Usage};
use ostra_core::executor::ExecutorKind;
use ostra_core::model::{Effort, Tier};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Static facts about an agent from its `agent.toml`.
#[derive(Debug, Clone)]
pub struct AgentMeta {
    pub default_tier: Tier,
    pub capabilities: Vec<Capability>,
    pub timeout_secs: u64,
}

/// A spawn rendered for one executor.
#[derive(Debug, Clone)]
pub struct BuiltSpawn {
    pub system_prompt: String,
    /// Spawn block plus repo brief and custom instructions.
    pub first_message: String,
    /// The `Label: value` block alone.
    pub spawn_block: String,
    pub params: Value,
    pub report_file: Option<PathBuf>,
    pub effort: Effort,
}

pub struct SpawnEnv<'a> {
    pub state: &'a SessionState,
    pub executor: ExecutorKind,
    pub settings: &'a WorkspaceSettings,
    pub profile: Option<&'a ProjectProfile>,
    pub inventory: Option<&'a str>,
    pub repo_root: &'a Path,
    /// The project's `CLAUDE.md`, `AGENTS.md`, and `AGENT.md`.
    pub project_docs: &'a [ostra_agents::brief::ProjectDoc],
}

pub trait SpawnFactory: Send + Sync {
    fn agent_meta(&self, agent: AgentName) -> AgentMeta;
    fn build(&self, req: &SpawnRequest, env: &SpawnEnv<'_>) -> Result<BuiltSpawn, String>;
    fn judge_prompt(&self, name: &str) -> Option<String>;
}

/// A push notification the engine asks for. The server decides whether push is on.
#[derive(Debug, Clone)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// Deep link path, for example `/sessions/<id>`.
    pub url: String,
    pub tag: String,
}

#[async_trait::async_trait]
pub trait Services: Send + Sync {
    /// Re-read on every execution, so an edit applies to the next one without a restart.
    fn global(&self) -> GlobalConfig;
    fn workspace(&self) -> WorkspaceSettings;
    fn executor(&self, kind: ExecutorKind) -> Option<Arc<dyn Executor>>;
    fn factory(&self) -> Arc<dyn SpawnFactory>;
    /// One judge call: a forced `decide` tool whose input matches `schema`.
    async fn judge(
        &self,
        route: &ResolvedRoute,
        system: &str,
        user: &str,
        schema: Value,
        effort: Effort,
    ) -> Result<(Value, Usage), String>;
    fn notify(&self, notice: Notice);
    /// Paths no agent may touch: Ostra's binary, config, and databases.
    fn protected_paths(&self) -> Vec<PathBuf>;
    /// The user approved `command` as the format command of the project at `project`.
    fn command_approved(&self, project: &Path, command: &str) -> bool;
    /// Add a rule to the workspace allow list ("always in this workspace").
    fn add_allow_rule(&self, rule: &str);
}
