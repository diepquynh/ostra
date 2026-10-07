//! Plugins (HANDOVER 10.10): code that adds agents and workflow stage logic to Ostra, in process
//! (a binary built with `ostra-sdk`) or out of process (a program speaking JSON-RPC over stdio).
//! These are the shapes both sides exchange and the `Plugin` and `AgentCalls` traits; `ostra-sdk`
//! re-exports them with its helpers.

use crate::agent::{Capability, WriteScope};
use crate::ids::{ExecutionId, SessionId};
use crate::model::{Effort, Tier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use ts_rs::TS;

/// Rule PL1: one out-of-process plugin in `workspace.toml`: `[[plugins]]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PluginConfig {
    pub name: String,
    /// Program and arguments. It starts in the workspace folder and speaks JSON-RPC on stdio.
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Seconds one stage decision may take.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_enabled() -> bool {
    true
}

fn default_timeout() -> u64 {
    120
}

pub const MAX_PLUGIN_TIMEOUT_SECS: u64 = 3600;

/// What a plugin offers, from its `initialize` answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub agents: Vec<PluginAgent>,
    #[serde(default)]
    pub stages: Vec<PluginStage>,
    /// Rule PL5: result contracts of its own, which its agents return and it handles.
    #[serde(default)]
    pub contracts: Vec<PluginContractDef>,
    /// Rule PL6: workflows it builds in code. A session names one as `<plugin>:<name>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<PluginWorkflow>,
    /// Rule PL7: transform functions it runs in code. A node names one as `<plugin>:<name>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<crate::transform::TransformInfo>,
}

/// Rule PL6: a workflow a plugin builds in code, in the shape a workflow file has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PluginWorkflow {
    /// Its name inside the plugin, in lowercase kebab-case.
    pub name: String,
    pub workflow: crate::workflow::WorkflowFile,
}

/// Rule PL5: a result contract a plugin defines: the schema its agents submit, which the plugin's
/// `handle_result` turns into the outcome a workflow reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PluginContractDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// JSON Schema (subset) of the submit.
    #[ts(type = "unknown")]
    pub schema: serde_json::Value,
}

/// Rule PL5: one result of a plugin contract, as its plugin's handler sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResultView {
    pub session: SessionId,
    pub execution: ExecutionId,
    pub agent: String,
    /// The contract's name inside the plugin.
    pub contract: String,
    /// The workflow node the run served, when it served one.
    pub stage: Option<String>,
    pub scope: Option<String>,
    #[ts(type = "unknown")]
    pub submit: serde_json::Value,
}

/// Rule PL2: an agent a plugin defines. With `prompt`, a model runs it on the executor its route
/// picks, like a markdown agent. Without one, the plugin's `run_agent` runs it in code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct PluginAgent {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_tier: Option<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<Capability>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_scope: Option<WriteScope>,
    #[serde(default)]
    pub effort: BTreeMap<String, Effort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// JSON Schema (subset) of the submit's `data`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub data_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub helper: bool,
    /// Rule CA5: the result contract it submits: `stage` (the default), a built-in contract such as
    /// `spec` or `review`, or a plugin's own `<plugin>:<contract>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    /// Rule CA6: the repo brief sections it gets (`stack`, `commands`, `testing`, `skills`,
    /// `conventions`, `review`, `modules`). Absent gives every section but `testing` and `review`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<Vec<String>>,
}

impl PluginAgent {
    /// An agent with a name and a description; the builder methods set the rest.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        PluginAgent {
            name: name.into(),
            description: description.into(),
            ..Default::default()
        }
    }

    /// The instructions a model runs it with. Without a prompt, the plugin runs it in code.
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    pub fn tier(mut self, tier: Tier) -> Self {
        self.default_tier = Some(tier);
        self
    }

    pub fn capabilities(mut self, caps: impl IntoIterator<Item = Capability>) -> Self {
        self.capabilities = Some(caps.into_iter().collect());
        self
    }

    pub fn write_scope(mut self, scope: WriteScope) -> Self {
        self.write_scope = Some(scope);
        self
    }

    /// Reasoning effort on one executor table (`native`, `claude`, `codex`, `grok`, `agy`).
    pub fn effort(mut self, executor: impl Into<String>, effort: Effort) -> Self {
        self.effort.insert(executor.into(), effort);
        self
    }

    pub fn timeout_seconds(mut self, secs: u64) -> Self {
        self.timeout_seconds = Some(secs);
        self
    }

    /// JSON Schema (subset) of the submit's `data`.
    pub fn data_schema(mut self, schema: serde_json::Value) -> Self {
        self.data_schema = Some(schema);
        self
    }

    pub fn helper(mut self, helper: bool) -> Self {
        self.helper = helper;
        self
    }

    /// The result contract it submits, such as `review` or `acme:release-note`.
    pub fn returns(mut self, contract: impl Into<String>) -> Self {
        self.returns = Some(contract.into());
        self
    }

    /// The repo brief sections it gets.
    pub fn brief(mut self, sections: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.brief = Some(sections.into_iter().map(Into::into).collect());
        self
    }
}

/// Rule PL4: the name of Ostra's standard plugin, which no other plugin may take.
pub const STANDARD_PLUGIN: &str = "ostra";

/// Rule PL3: stage logic a plugin serves for workflow nodes `plugin = "<plugin>:<name>"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PluginStage {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// One run a plugin stage started, as the plugin sees it when it decides again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StageRunView {
    pub execution: ExecutionId,
    pub agent: String,
    pub status: crate::exec::ExecutionStatus,
    #[serde(default)]
    #[ts(type = "unknown")]
    pub submit: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<String>,
    /// Rule PL5: what the plugin's handler made of a plugin contract's result.
    #[serde(default)]
    #[ts(type = "unknown")]
    pub handled: Option<serde_json::Value>,
}

/// Rule PL3: what a plugin's stage logic is given each time it decides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StageView {
    pub session: SessionId,
    /// The workflow node.
    pub node: String,
    /// The plugin's stage name.
    pub stage: String,
    pub scope: Option<String>,
    pub request: String,
    #[ts(type = "string")]
    pub workspace_root: PathBuf,
    /// Projects in scope: key and path.
    #[ts(type = "Array<[string, string]>")]
    pub projects: Vec<(String, PathBuf)>,
    pub instructions: Option<String>,
    /// Rule WB4: the node's inputs from earlier nodes.
    #[serde(default)]
    #[ts(type = "Record<string, unknown>")]
    pub inputs: serde_json::Map<String, serde_json::Value>,
    /// Decisions it made for this stage before, oldest first.
    pub decisions: Vec<StageDecision>,
    /// The runs those decisions started, oldest first.
    pub runs: Vec<StageRunView>,
    /// What the user answered at this stage's gates, oldest first.
    pub answers: Vec<String>,
    /// One line per earlier custom stage: id, verdict, summary.
    pub earlier_stages: Vec<String>,
    #[ts(type = "string | null")]
    pub spec_file: Option<PathBuf>,
    #[ts(type = "string | null")]
    pub master_plan: Option<PathBuf>,
}

/// Rule PL3: one decision of a plugin's stage logic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum StageDecision {
    /// Run this agent (any agent of the workspace's catalog) with these instructions.
    Run {
        agent: String,
        #[serde(default)]
        instructions: Option<String>,
    },
    /// The stage passed.
    Pass { summary: String },
    /// The stage failed; its `on_fail` decides what follows.
    Fail { summary: String },
    /// Ask the user, recommended option first.
    Ask {
        question: String,
        #[serde(default)]
        options: Vec<String>,
    },
}

/// Rule PL2: what a programmatic agent is given when it starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTask {
    pub execution: ExecutionId,
    pub session: Option<SessionId>,
    pub agent: String,
    /// The spawn block, the repo brief, and the custom instructions.
    pub first_message: String,
    pub repo_root: PathBuf,
    pub session_dir: PathBuf,
    pub workspace_root: PathBuf,
    /// The model its `complete` calls run on.
    pub model: String,
    /// The tools it may call, by native name.
    pub tools: Vec<String>,
    /// The schema its result must match.
    pub submit_schema: serde_json::Value,
    /// Rule PL8: the run continues an earlier start of the same execution, because the plugin
    /// program stopped or the session was paused. Its checkpoints say where it was.
    #[serde(default)]
    pub resumed: bool,
}

/// One message of a `complete` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteMessage {
    /// `user` or `assistant`.
    pub role: String,
    pub text: String,
}

/// Rule PL2: a model call a programmatic agent makes on its route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteRequest {
    pub system: String,
    pub messages: Vec<CompleteMessage>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// The answer of one tool call a programmatic agent makes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReply {
    pub output: String,
    pub is_error: bool,
}

/// How a programmatic agent ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentOutcome {
    /// The submit payload, checked against `AgentTask::submit_schema`.
    pub submit: serde_json::Value,
}

/// Rule PL1: plugin names are lowercase kebab-case, like agent names.
pub fn valid_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Rule PL1: the problems in a workspace's `[[plugins]]`.
pub fn validate(plugins: &[PluginConfig], issues: &mut Vec<crate::config::ValidationIssue>) {
    let mut seen = std::collections::BTreeSet::new();
    for (i, p) in plugins.iter().enumerate() {
        let at = format!("plugins[{i}]");
        let mut push = |path: String, message: String| {
            issues.push(crate::config::ValidationIssue { path, message })
        };
        if !valid_plugin_name(&p.name) {
            push(
                format!("{at}.name"),
                format!(
                    "Name the plugin in lowercase kebab-case, starting with a letter: `{}` is not.",
                    p.name
                ),
            );
        }
        if p.name == STANDARD_PLUGIN {
            push(
                format!("{at}.name"),
                format!("Give the plugin another name: `{STANDARD_PLUGIN}` is Ostra's own."),
            );
        }
        if !seen.insert(p.name.clone()) {
            push(
                format!("{at}.name"),
                format!("Give each plugin its own name: `{}` is used twice.", p.name),
            );
        }
        if p.command.first().is_none_or(|c| c.trim().is_empty()) {
            push(
                format!("{at}.command"),
                "Give the program to start, then its arguments.".into(),
            );
        }
        if p.timeout_secs == 0 || p.timeout_secs > MAX_PLUGIN_TIMEOUT_SECS {
            push(
                format!("{at}.timeout_secs"),
                format!("Use a timeout from 1 to {MAX_PLUGIN_TIMEOUT_SECS} seconds."),
            );
        }
    }
}

/// Rule PL8: the most keys one plugin keeps in one session.
pub const MAX_CHECKPOINT_KEYS: usize = 256;
/// Rule PL8: the largest value, as JSON.
pub const MAX_CHECKPOINT_BYTES: usize = 64 * 1024;
/// Rule PL8: the most saves one plugin makes in one session, because each is an event.
pub const MAX_CHECKPOINT_SAVES: u32 = 5000;
/// Rule PL8: how often Ostra starts a stopped plugin program again within one call or run.
pub const PLUGIN_RESTARTS: u32 = 2;

/// Rule PL8: whether a plugin may write `value` (`None` removes the key) under `key`, given what
/// it keeps now and how often it saved. `Ok(false)` means the write changes nothing.
pub fn check_checkpoint(
    current: &BTreeMap<String, serde_json::Value>,
    saves: u32,
    key: &str,
    value: Option<&serde_json::Value>,
) -> Result<bool, String> {
    if key.is_empty() || key.len() > 200 || key.chars().any(char::is_control) {
        return Err(
            "Name the checkpoint with 1 to 200 characters and no control characters.".into(),
        );
    }
    if current.get(key) == value {
        return Ok(false);
    }
    if saves >= MAX_CHECKPOINT_SAVES {
        return Err(format!(
            "Save less often: this plugin saved {MAX_CHECKPOINT_SAVES} checkpoints in this session, the limit."
        ));
    }
    if let Some(v) = value {
        let bytes = serde_json::to_string(v).map_or(0, |s| s.len());
        if bytes > MAX_CHECKPOINT_BYTES {
            return Err(format!(
                "Keep a checkpoint under {MAX_CHECKPOINT_BYTES} bytes of JSON: this one has {bytes}. Write large data to a file and save its path."
            ));
        }
        if !current.contains_key(key) && current.len() >= MAX_CHECKPOINT_KEYS {
            return Err(format!(
                "Remove a checkpoint first: this plugin keeps {MAX_CHECKPOINT_KEYS} in this session, the limit."
            ));
        }
    }
    Ok(true)
}

/// Rule PL8: a plugin's working metadata in one session, kept in the session's event log, so a
/// plugin program that stopped and started again reads where its work was. Keys are the
/// plugin's own; each plugin sees only its keys.
#[async_trait::async_trait]
pub trait Checkpoints: Send + Sync {
    /// Every value the plugin keeps in the session, by key: those saved before the call started,
    /// and the call's own saves.
    fn all(&self) -> BTreeMap<String, serde_json::Value>;
    /// Save `value` under `key`, or remove the key with `None`. The save is in the log when this
    /// returns.
    async fn write(&self, key: &str, value: Option<serde_json::Value>) -> Result<(), String>;

    fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.all().remove(key)
    }

    async fn save(&self, key: &str, value: serde_json::Value) -> Result<(), String> {
        self.write(key, Some(value)).await
    }

    async fn remove(&self, key: &str) -> Result<(), String> {
        self.write(key, None).await
    }
}

/// Rule PL8: the checkpoints of a call that belongs to no session: none, and no saves.
pub struct NoCheckpoints;

#[async_trait::async_trait]
impl Checkpoints for NoCheckpoints {
    fn all(&self) -> BTreeMap<String, serde_json::Value> {
        BTreeMap::new()
    }

    async fn write(&self, _: &str, _: Option<serde_json::Value>) -> Result<(), String> {
        Err("This call belongs to no session, so it keeps no checkpoints.".into())
    }
}

/// Rule PL2: what a programmatic agent can ask of Ostra while it runs. Every tool call passes the
/// same policy, sandbox, and permission asks as a model's.
#[async_trait::async_trait]
pub trait AgentCalls: Send + Sync {
    /// Call a tool by its native name, such as `Read`, `Bash`, or `SendMessage`. Messages queued
    /// for the run follow the tool's output.
    async fn tool(&self, name: &str, input: serde_json::Value) -> ToolReply;
    /// One model call on the agent's route, billed to the run.
    async fn complete(&self, request: CompleteRequest) -> Result<String, String>;
    /// A line for the run's Activity view.
    fn status(&self, text: &str);
}

/// Rule PL1: a plugin, in process or behind the stdio host.
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;

    /// Rule PL8: whether the plugin can still answer. A program that stopped cannot, and Ostra
    /// starts it again.
    fn is_alive(&self) -> bool {
        true
    }

    /// Rule PL3: decide the next step of stage `stage`. Rule PL8: `checkpoints` holds what the
    /// plugin saved in the session.
    async fn decide_stage(
        &self,
        stage: &str,
        _view: StageView,
        _checkpoints: std::sync::Arc<dyn Checkpoints>,
    ) -> Result<StageDecision, String> {
        Err(format!("This plugin serves no stage `{stage}`."))
    }

    /// Rule PL5: turn a result of one of this plugin's contracts into the outcome the workflow
    /// reads: a verdict, a summary, findings, and a question for the user.
    async fn handle_result(
        &self,
        contract: &str,
        _result: ResultView,
        _checkpoints: std::sync::Arc<dyn Checkpoints>,
    ) -> Result<crate::submit::CustomSubmit, String> {
        Err(format!("This plugin handles no contract `{contract}`."))
    }

    /// Rule PL7: run transform function `name` of this plugin. Ostra records the output, so the
    /// function may read anything, but a workflow is easier to follow when it gives the same output
    /// for the same inputs.
    async fn transform(
        &self,
        name: &str,
        _inputs: serde_json::Map<String, serde_json::Value>,
        _args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        Err(format!("This plugin runs no transform `{name}`."))
    }

    /// Rule PL2: run programmatic agent `agent` to its result. Rule PL8: a run that saves its
    /// progress in `checkpoints` can resume from it when its program stopped.
    async fn run_agent(
        &self,
        agent: &str,
        _task: AgentTask,
        _calls: std::sync::Arc<dyn AgentCalls>,
        _checkpoints: std::sync::Arc<dyn Checkpoints>,
    ) -> Result<AgentOutcome, String> {
        Err(format!("This plugin runs no agent `{agent}` in code."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pl8_checkpoint_limits() {
        let mut kept = BTreeMap::new();
        assert_eq!(
            check_checkpoint(&kept, 0, "step", Some(&json!(1))),
            Ok(true)
        );
        kept.insert("step".to_string(), json!(1));
        assert_eq!(
            check_checkpoint(&kept, 1, "step", Some(&json!(1))),
            Ok(false),
            "the same value again is no save"
        );
        assert_eq!(check_checkpoint(&kept, 1, "gone", None), Ok(false));
        assert!(check_checkpoint(&kept, 1, "", Some(&json!(1))).is_err());
        assert!(check_checkpoint(&kept, 1, "a\nb", Some(&json!(1))).is_err());
        let big = json!("x".repeat(MAX_CHECKPOINT_BYTES));
        assert!(check_checkpoint(&kept, 1, "big", Some(&big)).is_err());
        assert!(check_checkpoint(&kept, MAX_CHECKPOINT_SAVES, "step", None).is_err());
        let full: BTreeMap<String, serde_json::Value> = (0..MAX_CHECKPOINT_KEYS)
            .map(|i| (format!("k{i}"), json!(i)))
            .collect();
        assert!(check_checkpoint(&full, 0, "new", Some(&json!(0))).is_err());
        assert_eq!(
            check_checkpoint(&full, 0, "k0", Some(&json!("changed"))),
            Ok(true),
            "a kept key can still change at the key limit"
        );
        assert_eq!(check_checkpoint(&full, 0, "k0", None), Ok(true));
    }
}
