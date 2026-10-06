//! What the engine needs from the rest of Ostra. The server implements these; tests use fakes.

use crate::plan::SpawnRequest;
use crate::state::SessionState;
use ostra_agents::AgentCatalog;
use ostra_core::agent::{AgentName, Capability, WriteScope};
use ostra_core::config::{GlobalConfig, ProjectProfile, ResolvedRoute, WorkspaceSettings};
use ostra_core::exec::{Executor, Usage};
use ostra_core::executor::ExecutorKind;
use ostra_core::model::{Effort, Tier};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Facts about an agent from its definition: `agent.toml` or a custom agent's frontmatter.
#[derive(Debug, Clone)]
pub struct AgentMeta {
    pub default_tier: Tier,
    pub capabilities: Vec<Capability>,
    pub timeout_secs: u64,
    pub write_scope: WriteScope,
    pub submit_schema: Value,
    /// Rule PL2: a plugin runs the agent in code.
    pub programmatic: bool,
    /// Rule CA5: the result contract it submits.
    pub returns: ostra_core::Contract,
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
    pub agents: &'a AgentCatalog,
}

pub trait SpawnFactory: Send + Sync {
    /// `None` when the workspace defines no such agent any more.
    fn agent_meta(&self, agent: AgentName, agents: &AgentCatalog) -> Option<AgentMeta>;
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
    /// Rule CA1: the workspace's agents, built-in and custom, re-read like the settings.
    fn agents(&self) -> AgentCatalog {
        AgentCatalog::builtin()
    }
    /// Rule WF1: the workspace's workflow files, re-read like the settings.
    fn workflows(&self) -> ostra_core::workflow::WorkflowSet {
        crate::pipeline::get().workflow_set()
    }
    /// Rule PL3: the plugin stages the workspace can run, as `(plugin, stage)`.
    fn plugin_stages(&self) -> Vec<(String, String)> {
        vec![]
    }
    /// Rule PL1: start the workspace's plugin programs that should run, so their agents and stages
    /// are known.
    async fn prepare_plugins(&self) {}
    /// Rule PL5: what plugin `plugin`'s handler makes of one result of its contract.
    async fn handle_result(
        &self,
        plugin: &str,
        _result: ostra_core::plugin::ResultView,
        _checkpoints: Arc<dyn ostra_core::plugin::Checkpoints>,
    ) -> Result<ostra_core::submit::CustomSubmit, String> {
        Err(format!("The workspace has no plugin `{plugin}`."))
    }
    /// Rule PL3: one decision of plugin `plugin`'s stage logic `stage`.
    async fn decide_stage(
        &self,
        plugin: &str,
        _stage: &str,
        _view: ostra_core::plugin::StageView,
        _checkpoints: Arc<dyn ostra_core::plugin::Checkpoints>,
    ) -> Result<ostra_core::plugin::StageDecision, String> {
        Err(format!("The workspace has no plugin `{plugin}`."))
    }
    /// Rule PL7: run transform function `name` of plugin `plugin`.
    async fn plugin_transform(
        &self,
        plugin: &str,
        _name: &str,
        _inputs: serde_json::Map<String, Value>,
        _args: serde_json::Map<String, Value>,
    ) -> Result<Value, String> {
        Err(format!("The workspace has no plugin `{plugin}`."))
    }
    /// Rule PL2: the executor of a programmatic agent, which its plugin runs in code.
    fn program_executor(&self, _agent: AgentName) -> Option<Arc<dyn Executor>> {
        None
    }
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
