//! The contract between the engine and whatever runs an execution (native loop or harness PTY).

use crate::agent::{AgentName, Capability, InitializerMode};
use crate::config::{PermissionMode, PermissionRules, ResolvedRoute, SandboxMode};
use crate::executor::ExecutorKind;
use crate::ids::{ExecutionId, SessionId};
use crate::model::Effort;
use crate::policy::{PermissionAnswer, PolicyDecision, RuleRef, ToolCall};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use ts_rs::TS;

pub use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ExecutionStatus {
    Running,
    Ok,
    Stuck,
    Handoff,
    /// Rule H2: the run asked another subagent and waits for the answer.
    Waiting,
    Error,
    Denied,
    Interrupted,
    Cancelled,
}

impl ExecutionStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, ExecutionStatus::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    /// Every cache write, 5-minute and 1-hour TTL together.
    pub cache_write_tokens: u64,
    /// The part of `cache_write_tokens` written with the 1-hour TTL, which costs more.
    pub cache_write_1h_tokens: u64,
    pub cost_usd: f64,
    pub tool_calls: u64,
    /// Wall time spent in build and test commands, for the build-loop metric.
    pub build_ms: u64,
    /// Size of the latest request's context: its prompt plus its output, which the next request
    /// sends again. The latest value, not a sum.
    pub context_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.cache_write_1h_tokens += other.cache_write_1h_tokens;
        self.cost_usd += other.cost_usd;
        self.tool_calls += other.tool_calls;
        self.build_ms += other.build_ms;
        if other.context_tokens > 0 {
            self.context_tokens = other.context_tokens;
        }
    }
}

/// What the policy layer needs to judge one tool call. Built by the engine per execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecContext {
    pub execution_id: ExecutionId,
    pub session_id: Option<SessionId>,
    pub agent: AgentName,
    pub initializer_mode: Option<InitializerMode>,
    pub executor: ExecutorKind,
    pub workspace_root: PathBuf,
    /// `Repo root:` of the spawn. The agent works from here.
    pub repo_root: PathBuf,
    pub project_key: String,
    /// `Session dir:` of the spawn: the session root, or its per-project subdir.
    pub session_dir: PathBuf,
    /// The session's root dir (owner of every artifact of this session).
    pub session_root: PathBuf,
    /// Declared `Report file:` when the agent has one.
    pub report_file: Option<PathBuf>,
    /// `Phase:` value for review loops (`N`, `N-tests`, `none`).
    pub phase: Option<String>,
    pub yolo: bool,
    pub permission_mode: PermissionMode,
    /// Merged global, workspace, and session rules.
    pub permissions: PermissionRules,
    /// Paths no agent may write, delete, or execute from: Ostra's binary, its config, its
    /// databases, and every engine state dir.
    pub protected_paths: Vec<PathBuf>,
    /// Absolute path of each project's memory database (engine-owned).
    pub memory_db: PathBuf,
    /// The workspace's sandbox mode in place of the global one, when it sets one.
    #[serde(default)]
    pub sandbox_mode: Option<SandboxMode>,
    /// Rule G1: Layer 1 checks where tool calls read and write. Resolved from the workspace and
    /// global `tool_enforcement` when the execution starts.
    #[serde(default)]
    pub enforce_tool_calls: bool,
    /// The workspace's network choice in place of the global one, when it sets one.
    #[serde(default)]
    pub sandbox_network: Option<crate::config::SandboxNetwork>,
    /// The workspace's own allowed hosts, added to the global ones.
    #[serde(default)]
    pub sandbox_allowed_hosts: Vec<String>,
    /// The workspace's own decoy paths, added to the built-in ones.
    #[serde(default)]
    pub sandbox_decoys: Vec<String>,
    /// Credential paths agents may read: the global `[sandbox] extra_readable` and the
    /// workspace's own, merged, because the policy reads no config.
    #[serde(default)]
    pub sandbox_readable: Vec<String>,
    /// The workspace's loopback choice for macOS.
    #[serde(default)]
    pub sandbox_loopback: crate::config::LoopbackAccess,
    /// The workspace's blocked loopback ports for macOS.
    #[serde(default)]
    pub sandbox_blocked_ports: Vec<u16>,
    /// Rule O2: this run's phase is in `project_key`, a project the plan named that does not exist
    /// yet, so it runs from the workspace root and must create that project first.
    #[serde(default)]
    pub creates_project: bool,
    /// Rule SM6: the run starts with messages from subagents that wait for its reply, so its
    /// reminders name `SendMessage` before its submit tool.
    #[serde(default)]
    pub owes_reply: bool,
    /// Rule CA2: where the agent may write. Absent means its session dir only.
    #[serde(default)]
    pub write_scope: Option<crate::agent::WriteScope>,
    /// Rule CA5: the result contract the run submits.
    #[serde(default = "stage_contract")]
    pub contract: crate::contract::Contract,
    /// Rule CA6: the run's capabilities, which grant tools and the files the guards let it write.
    #[serde(default)]
    pub capabilities: Vec<crate::agent::Capability>,
}

fn stage_contract() -> crate::contract::Contract {
    crate::contract::Contract::Stage
}

impl ExecContext {
    /// Rule CA2: the agent's write scope.
    pub fn scope(&self) -> crate::agent::WriteScope {
        self.write_scope.unwrap_or_default()
    }

    /// Rule CA6: the run holds this capability.
    pub fn has(&self, c: crate::agent::Capability) -> bool {
        self.capabilities.contains(&c)
    }

    /// The sandbox settings the workspace keeps in the registry.
    pub fn sandbox(&self) -> crate::config::WorkspaceSandbox {
        crate::config::WorkspaceSandbox {
            mode: self.sandbox_mode,
            network: self.sandbox_network,
            allowed_hosts: self.sandbox_allowed_hosts.clone(),
            readable: self.sandbox_readable.clone(),
            loopback: self.sandbox_loopback,
            blocked_ports: self.sandbox_blocked_ports.clone(),
        }
    }
}

/// Everything needed to run one execution.
#[derive(Debug, Clone)]
pub struct ExecutionSpec {
    pub id: ExecutionId,
    pub agent: AgentName,
    pub route: ResolvedRoute,
    pub effort: Effort,
    /// The agent prompt rendered for this executor's tool names.
    pub system_prompt: String,
    /// The spawn block plus the repo brief and custom instructions.
    pub first_message: String,
    pub capabilities: Vec<Capability>,
    /// Input schema of `submit_<agent>`.
    pub submit_schema: serde_json::Value,
    pub timeout_secs: u64,
    pub ctx: ExecContext,
    /// Present when resuming an earlier execution.
    pub resume: Option<ResumeInfo>,
    /// Session id chosen up front (Claude Code and Grok accept one).
    pub harness_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeInfo {
    /// The execution being resumed.
    pub from: ExecutionId,
    /// Harness session id, for harness resume commands.
    pub native_session_id: Option<String>,
    /// The message the resumed run starts with, in place of the interruption notice.
    #[serde(default)]
    pub note: Option<String>,
    /// Reopen the session for the user to read and ask about: no first prompt, no tools, no
    /// submit, and it ends when the user leaves or stops it.
    #[serde(default)]
    pub inspect: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionResult {
    pub status: ExecutionStatus,
    /// The `submit_<agent>` payload, when the agent called it.
    #[ts(type = "unknown")]
    pub submit: Option<serde_json::Value>,
    /// Last assistant text, kept for display and for the transcript.
    pub final_text: String,
    pub usage: Usage,
    pub native_session_id: Option<String>,
    pub error: Option<String>,
}

impl ExecutionResult {
    pub fn error(message: impl Into<String>) -> Self {
        ExecutionResult {
            status: ExecutionStatus::Error,
            submit: None,
            final_text: String::new(),
            usage: Usage::default(),
            native_session_id: None,
            error: Some(message.into()),
        }
    }

    pub fn with_status(status: ExecutionStatus) -> Self {
        ExecutionResult {
            status,
            ..ExecutionResult::error("")
        }
        .clear_error()
    }

    fn clear_error(mut self) -> Self {
        self.error = None;
        self
    }
}

/// Streamed to the Activity view and persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum ExecutionDelta {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolCall {
        call_id: String,
        call: ToolCall,
    },
    Policy {
        call_id: String,
        decision: PolicyDecision,
    },
    /// Live output of a running shell command.
    ToolOutput {
        call_id: String,
        chunk: String,
    },
    ToolResult {
        call_id: String,
        output: String,
        is_error: bool,
        duration_ms: u64,
    },
    Usage {
        usage: Usage,
    },
    /// One model response of a native run: its own usage and cost, not the running total, and
    /// the tool calls it made. Emitted after the response's thinking and text, before its calls.
    Turn {
        usage: Usage,
        call_ids: Vec<String>,
    },
    Status {
        message: String,
    },
    NativeSessionId {
        id: String,
    },
    /// The egress proxy's first answer for a host and port.
    Egress {
        host: String,
        port: u16,
        allowed: bool,
        /// Why it was refused, with the setting that allows it first.
        reason: Option<String>,
        /// The destination is loopback, private, or link-local.
        local: bool,
    },
    /// A process in the sandbox opened a decoy credential file (Rule P3).
    Decoy {
        path: String,
    },
}

/// Messages handed to a run at a turn boundary, or the message that wakes a waiting run (Rule SM2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wake {
    pub note: String,
    /// Rule SM6: a sender waits for this run's reply.
    pub owes_reply: bool,
}

/// Callbacks an executor uses while it runs. Implemented by the engine.
#[async_trait::async_trait]
pub trait ExecutionHost: Send + Sync {
    fn emit(&self, delta: ExecutionDelta);

    /// A permission ask. Waits for the browser answer, or answers at once under YOLO.
    async fn ask_permission(
        &self,
        call: &ToolCall,
        reason: &str,
        rule: &RuleRef,
    ) -> PermissionAnswer;

    /// Persist one transcript message (native executions), so a Resume can continue from it.
    fn record_message(&self, _role: &str, _content: &serde_json::Value) {}

    /// The transcript of an earlier execution, oldest first, as `(role, content)`.
    fn transcript(&self, _execution: &ExecutionId) -> Vec<(String, serde_json::Value)> {
        vec![]
    }

    /// Whether the session is under YOLO right now (it can be toggled mid-execution).
    fn yolo(&self) -> bool {
        false
    }

    /// Rule SM3: a harness run that paused itself waits here, with its execution slot freed,
    /// until Ostra has the message that wakes it. `None` means no message will come.
    async fn wait_for_wake(&self) -> Option<Wake> {
        None
    }

    /// Rule SM2: the messages queued for this run, handed over now and recorded as delivered.
    /// Executors call it only at a turn boundary, so a message never lands inside a request.
    fn take_messages(&self) -> Option<Wake> {
        None
    }

    /// Rule SM2: messages are queued for this run, without handing them over.
    fn has_messages(&self) -> bool {
        false
    }

    /// Rule SM6: why the run may not submit yet, as the correction for the model.
    fn submit_blocked(&self) -> Option<String> {
        None
    }

    /// Rule PL8: plugin `plugin`'s checkpoints in this run's session.
    fn checkpoints(&self, _plugin: &str) -> Arc<dyn crate::plugin::Checkpoints> {
        Arc::new(crate::plugin::NoCheckpoints)
    }
}

#[async_trait::async_trait]
pub trait Executor: Send + Sync {
    async fn run(
        &self,
        spec: ExecutionSpec,
        host: Arc<dyn ExecutionHost>,
        cancel: CancellationToken,
    ) -> ExecutionResult;
}
