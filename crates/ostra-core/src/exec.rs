//! The contract between the engine and whatever runs an execution (native loop or harness PTY).

use crate::agent::{AgentName, Capability, InitializerMode};
use crate::config::{PermissionMode, PermissionRules, ResolvedRoute};
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
        ExecutionResult { status, ..ExecutionResult::error("") }.clear_error()
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
    Text { text: String },
    Thinking { text: String },
    ToolCall { call_id: String, call: ToolCall },
    Policy { call_id: String, decision: PolicyDecision },
    /// Live output of a running shell command.
    ToolOutput { call_id: String, chunk: String },
    ToolResult { call_id: String, output: String, is_error: bool, duration_ms: u64 },
    Usage { usage: Usage },
    Status { message: String },
    NativeSessionId { id: String },
}

/// Callbacks an executor uses while it runs. Implemented by the engine.
#[async_trait::async_trait]
pub trait ExecutionHost: Send + Sync {
    fn emit(&self, delta: ExecutionDelta);

    /// A permission ask. Waits for the browser answer, or answers at once under YOLO.
    async fn ask_permission(&self, call: &ToolCall, reason: &str, rule: &RuleRef) -> PermissionAnswer;

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
