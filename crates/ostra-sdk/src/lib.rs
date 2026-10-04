//! Write Ostra plugins in Rust (HANDOVER 10.10).
//!
//! A plugin adds agents and workflow stage logic to Ostra. Implement [`Plugin`], then either
//! build it into your own `ostra` binary with [`Registry`] and `ostra_server::main_with`, or run it
//! as its own program with [`stdio::serve`] and list that program under `[[plugins]]` in the
//! workspace's `workspace.toml`.
//!
//! A plugin can offer these:
//! - Agents with a prompt, which a model runs like a markdown agent of `.ostra/agents/`.
//! - Programmatic agents, which [`Plugin::run_agent`] runs in code through an [`AgentContext`]:
//!   every tool call passes Ostra's policy and sandbox, and model calls run on the agent's route.
//! - Stage logic for workflow nodes `plugin = "<plugin>:<stage>"`, which
//!   [`Plugin::decide_stage`] drives one decision at a time: run an agent, pass, fail, or ask.
//! - Workflows built in code with [`workflow::Workflow`], which a session names as
//!   `<plugin>:<name>`.
//! - Transform functions run in code by [`Plugin::transform`] and described with
//!   [`workflow::TransformFn`], which a workflow node calls as `<plugin>:<name>`.
//!
//! Every stage decision, result handler, and programmatic run gets the plugin's [`Checkpoints`]
//! in the session. Save where the work is there, because a plugin program can stop at any time:
//! Ostra starts it again and calls it once more, and the new process reads what the old one saved.
//!
//! Ostra's own agents and default workflows are written the same way: the crate
//! `ostra-default-plugin` is the plugin `ostra`, which reads each agent's `agent.toml` and prompt
//! with [`definition::parse_toml`] and offers each default workflow as one of its workflows.

pub mod definition;
pub mod stdio;
pub mod workflow;

pub use ostra_core::plugin::{
    AgentCalls, AgentOutcome, AgentTask, Checkpoints, CompleteMessage, CompleteRequest,
    MAX_CHECKPOINT_BYTES, MAX_CHECKPOINT_KEYS, MAX_CHECKPOINT_SAVES, NoCheckpoints, Plugin,
    PluginAgent, PluginContractDef, PluginManifest, PluginStage, PluginWorkflow, ResultView,
    STANDARD_PLUGIN, StageDecision, StageRunView, StageView, ToolReply,
};
pub use ostra_core::submit::{CustomFinding, CustomSubmit, StageVerdict};
pub use ostra_core::transform::{CondOp, TransformInfo, TransformParam, ValueKind};
pub use ostra_core::workflow::{OnFail, StageScope, WorkflowFile};
pub use ostra_core::{AgentName, Capability, Contract, Effort, Tier, WriteScope};

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What a programmatic agent can do while it runs, with helpers for the common tools.
#[derive(Clone)]
pub struct AgentContext {
    calls: Arc<dyn AgentCalls>,
}

impl AgentContext {
    pub fn new(calls: Arc<dyn AgentCalls>) -> Self {
        AgentContext { calls }
    }

    /// Call any tool the agent has, by its native name and input.
    pub async fn tool(&self, name: &str, input: Value) -> ToolReply {
        self.calls.tool(name, input).await
    }

    pub async fn read(&self, path: &str) -> ToolReply {
        self.tool("Read", json!({ "file_path": path })).await
    }

    pub async fn write(&self, path: &str, content: &str) -> ToolReply {
        self.tool("Write", json!({ "file_path": path, "content": content }))
            .await
    }

    pub async fn bash(&self, command: &str) -> ToolReply {
        self.tool("Bash", json!({ "command": command })).await
    }

    pub async fn grep(&self, pattern: &str, path: Option<&str>) -> ToolReply {
        let mut input = json!({ "pattern": pattern });
        if let Some(p) = path {
            input["path"] = json!(p);
        }
        self.tool("Grep", input).await
    }

    /// The session's subagents and the helpers this agent can start.
    pub async fn list_agents(&self) -> ToolReply {
        self.tool("ListAgents", json!({})).await
    }

    /// Queue a message for another subagent. With `wait`, the call returns once a message for
    /// this agent arrives, with that message in its output.
    pub async fn send_message(&self, to: &str, message: &str, wait: bool) -> ToolReply {
        self.tool(
            "SendMessage",
            json!({ "to": to, "message": message, "wait": wait }),
        )
        .await
    }

    /// Start a helper agent with `message` as its task. With `wait`, the call returns with its
    /// result.
    pub async fn start_helper(&self, agent: &str, message: &str, wait: bool) -> ToolReply {
        self.tool(
            "SendMessage",
            json!({ "agent": agent, "message": message, "wait": wait }),
        )
        .await
    }

    /// Pause until a message for this agent arrives, and return it.
    pub async fn wait_for_message(&self) -> ToolReply {
        self.tool("WaitForMessage", json!({})).await
    }

    /// One model call on the agent's route.
    pub async fn complete(&self, system: &str, user: &str) -> Result<String, String> {
        self.calls
            .complete(CompleteRequest {
                system: system.into(),
                messages: vec![CompleteMessage {
                    role: "user".into(),
                    text: user.into(),
                }],
                max_tokens: None,
            })
            .await
    }

    pub async fn complete_with(&self, request: CompleteRequest) -> Result<String, String> {
        self.calls.complete(request).await
    }

    /// A line for the run's Activity view.
    pub fn status(&self, text: &str) {
        self.calls.status(text);
    }
}

/// Rule PL8: checkpoints kept in memory, with Ostra's limits, for a plugin's own tests.
#[derive(Default)]
pub struct MemoryCheckpoints {
    kept: parking_lot::Mutex<(BTreeMap<String, Value>, u32)>,
}

impl MemoryCheckpoints {
    pub fn new(kept: BTreeMap<String, Value>) -> Self {
        MemoryCheckpoints {
            kept: parking_lot::Mutex::new((kept, 0)),
        }
    }
}

#[async_trait::async_trait]
impl Checkpoints for MemoryCheckpoints {
    fn all(&self) -> BTreeMap<String, Value> {
        self.kept.lock().0.clone()
    }

    async fn write(&self, key: &str, value: Option<Value>) -> Result<(), String> {
        let mut kept = self.kept.lock();
        if !ostra_core::plugin::check_checkpoint(&kept.0, kept.1, key, value.as_ref())? {
            return Ok(());
        }
        kept.1 += 1;
        match value {
            Some(v) => kept.0.insert(key.to_string(), v),
            None => kept.0.remove(key),
        };
        Ok(())
    }
}

/// The result of a custom agent: a verdict, a summary, and optional findings and data.
pub fn outcome(submit: CustomSubmit) -> AgentOutcome {
    AgentOutcome {
        submit: serde_json::to_value(submit).unwrap_or(Value::Null),
    }
}

/// A passing result with a summary.
pub fn pass(summary: impl Into<String>) -> AgentOutcome {
    outcome(CustomSubmit {
        verdict: StageVerdict::Pass,
        summary: summary.into(),
        findings: vec![],
        question: None,
        options: vec![],
        report_path: None,
        data: None,
    })
}

/// A failing result with a summary and findings.
pub fn fail(summary: impl Into<String>, findings: Vec<CustomFinding>) -> AgentOutcome {
    outcome(CustomSubmit {
        verdict: StageVerdict::Fail,
        summary: summary.into(),
        findings,
        question: None,
        options: vec![],
        report_path: None,
        data: None,
    })
}

/// Plugins built into one `ostra` binary, by name.
#[derive(Clone, Default)]
pub struct Registry {
    plugins: BTreeMap<String, Arc<dyn Plugin>>,
}

impl Registry {
    pub fn new() -> Self {
        Registry::default()
    }

    /// Add a plugin. A later plugin of the same name replaces the earlier one.
    ///
    /// Panics on a plugin named `ostra`, which is Ostra's own standard plugin (Rule PL4), because
    /// the binary that registers it is built wrong.
    pub fn with(mut self, plugin: Arc<dyn Plugin>) -> Self {
        let name = plugin.manifest().name;
        assert_ne!(
            name, STANDARD_PLUGIN,
            "name the plugin something other than `{STANDARD_PLUGIN}`, which is Ostra's own"
        );
        self.plugins.insert(name, plugin);
        self
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Plugin>> {
        self.plugins.get(name).cloned()
    }

    pub fn all(&self) -> impl Iterator<Item = (&String, &Arc<dyn Plugin>)> {
        self.plugins.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}
