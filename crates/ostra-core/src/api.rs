//! REST and WebSocket shapes shared with the browser. Exported to `web/src/api/gen/` by ts-rs
//! (`cargo test -p ostra-core export_bindings`).
//!
//! WebSocket `/ws` carries JSON text frames of [`ClientMsg`] and [`ServerMsg`], plus binary PTY
//! frames: one byte holding the execution id length `n`, then `n` bytes of execution id, then raw
//! terminal bytes. The browser sends terminal input as [`ClientMsg::TermInput`].

use crate::agent::AgentName;
use crate::config::{ProjectProfile, ValidationIssue, WorkspaceSettings};
use crate::event::{
    AnswerSource, ExecPurpose, GateAnswer, GatePayload, JudgeKind, SessionEvent, SessionKind, SessionOptions,
};
use crate::exec::{ExecutionDelta, ExecutionStatus, Usage};
use crate::executor::{ExecutorKind, HarnessKind};
use crate::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
use crate::pipeline::{Category, Lane, PhaseInfo, StageKind};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use ts_rs::TS;

// ---------------------------------------------------------------------------------------------
// Workspaces and projects
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceSummary {
    pub id: WorkspaceId,
    pub name: String,
    #[ts(type = "string")]
    pub root: PathBuf,
    pub projects: u32,
    pub active_sessions: u32,
    /// False when the directory or its `workspace.toml` is gone.
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateWorkspace {
    pub name: String,
    #[ts(type = "string")]
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceDetail {
    pub id: WorkspaceId,
    #[ts(type = "string")]
    pub root: PathBuf,
    pub settings: WorkspaceSettings,
    pub projects: Vec<ProjectView>,
    pub harnesses: Vec<HarnessStatus>,
    pub providers: Vec<ProviderStatus>,
    /// Problems with the current settings. Saving refuses settings with problems.
    pub validation: Vec<ValidationIssue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InitStatus {
    Initialized,
    NotInitialized,
    /// An init session is running.
    Initializing,
    /// The folder no longer exists.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectView {
    pub key: String,
    #[ts(type = "string")]
    pub path: PathBuf,
    pub init_status: InitStatus,
    /// `.ultracode/` holds a complete Ultracode bootstrap that could be migrated.
    pub ultracode_bootstrap: bool,
    pub is_git: bool,
    pub stack: Option<String>,
    pub profile: Option<ProjectProfile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportProject {
    #[ts(type = "string")]
    pub path: PathBuf,
    pub key: String,
    pub stack: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HarnessStatus {
    pub harness: HarnessKind,
    pub command: String,
    pub installed: bool,
    pub version: Option<String>,
    /// `None` when Ostra cannot tell.
    pub logged_in: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderStatus {
    pub name: String,
    pub has_key: bool,
    /// Where the key came from: `env:NAME`, `keychain`, or `none`.
    pub source: String,
}

// ---------------------------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateSession {
    pub request: String,
    #[serde(default)]
    pub options: SessionOptions,
    /// Projects the user pinned. Empty lets the Classify judge choose.
    #[serde(default)]
    pub projects: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionStatus {
    Running,
    /// Waiting on at least one gate.
    Waiting,
    Completed,
    Failed,
    /// Nothing is running and no gate is open, but the session is not done.
    Stalled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionSummary {
    pub id: SessionId,
    pub workspace: WorkspaceId,
    pub kind: SessionKind,
    pub request: String,
    pub category: Option<Category>,
    pub status: SessionStatus,
    /// Current lane and a short stage label (the hub's inferred stage).
    pub lane: Lane,
    pub stage_label: String,
    pub yolo: bool,
    pub open_gates: u32,
    pub projects: Vec<String>,
    pub cost_usd: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum StageStatus {
    Pending,
    Running,
    Waiting,
    Done,
    Skipped,
    Failed,
    Blocked,
}

/// One card on the session board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StageCard {
    pub stage: StageKind,
    pub lane: Lane,
    pub label: String,
    pub status: StageStatus,
    pub project: Option<String>,
    pub phase: Option<u32>,
    pub executions: Vec<ExecutionId>,
    pub gate: Option<GateId>,
    /// One-line status detail, for example `FAIL, 2 findings` or `pass 2 of 3`.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PhaseStatus {
    Queued,
    Implementing,
    Reviewing,
    Passed,
    Blocked,
    /// Removed from the queue because a phase it depends on failed (Rule D9).
    Removed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PhaseView {
    pub info: PhaseInfo,
    pub status: PhaseStatus,
    pub review_iterations: u32,
    /// `none`, `queued`, `running`, `passed`, `blocked`, or `skipped`.
    pub tests: String,
    pub security_block: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionView {
    pub id: ExecutionId,
    pub session: Option<SessionId>,
    pub agent: AgentName,
    pub purpose: Option<ExecPurpose>,
    pub stage: Option<StageKind>,
    pub project: String,
    pub executor: ExecutorKind,
    pub model: String,
    pub status: ExecutionStatus,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub usage: Usage,
    #[ts(type = "string | null")]
    pub report_path: Option<PathBuf>,
    pub native_session_id: Option<String>,
    pub spawn_block: String,
    pub error: Option<String>,
    /// A harness execution whose session can be reopened.
    pub can_resume: bool,
    /// A live PTY exists for this execution.
    pub has_terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GateView {
    pub id: GateId,
    pub session: SessionId,
    pub title: String,
    pub explanation: String,
    pub payload: GatePayload,
    pub answer: Option<GateAnswer>,
    pub source: Option<AnswerSource>,
    pub reason: Option<String>,
    pub opened_at: DateTime<Utc>,
    pub answered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionView {
    pub id: DecisionId,
    pub judge: JudgeKind,
    pub subject: Option<String>,
    pub input_summary: String,
    #[ts(type = "unknown")]
    pub output: serde_json::Value,
    pub reason: String,
    pub overridden: bool,
    /// False once work that depends on the decision has started.
    pub can_override: bool,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactRef {
    #[ts(type = "string")]
    pub path: PathBuf,
    /// `research`, `spec`, `plan`, `phase`, `report`, `ledger`, `completion`.
    pub kind: String,
    pub label: String,
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionDetail {
    pub summary: SessionSummary,
    pub stages: Vec<StageCard>,
    pub phases: Vec<PhaseView>,
    pub executions: Vec<ExecutionView>,
    pub gates: Vec<GateView>,
    pub decisions: Vec<DecisionView>,
    pub artifacts: Vec<ArtifactRef>,
    /// Markdown of the completion report, once written.
    pub completion: Option<String>,
    #[ts(type = "string")]
    pub session_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AnswerGate {
    pub answer: GateAnswer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OverrideDecision {
    #[ts(type = "unknown")]
    pub output: serde_json::Value,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetYolo {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AmendRequest {
    pub text: String,
}

/// One persisted Activity item of an execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActivityItem {
    pub seq: i64,
    pub at: DateTime<Utc>,
    pub delta: ExecutionDelta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Artifact {
    #[ts(type = "string")]
    pub path: PathBuf,
    pub content: String,
}

// ---------------------------------------------------------------------------------------------
// Memory, cost, side panel, push
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Lesson {
    pub id: i64,
    pub area: String,
    pub lesson: String,
    pub source: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LessonEdit {
    pub id: Option<i64>,
    pub area: String,
    pub lesson: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CostRow {
    /// Grouping value: a session id, stage, agent, or executor.
    pub key: String,
    pub executions: u32,
    pub usage: Usage,
    /// The metric Ultracode's bench tracks: cache reads divided by tool calls.
    pub cache_reads_per_tool_call: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CostReport {
    pub by_session: Vec<CostRow>,
    pub by_stage: Vec<CostRow>,
    pub by_agent: Vec<CostRow>,
    pub by_executor: Vec<CostRow>,
    pub total: CostRow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AskQuestion {
    pub question: String,
    /// When asked from a session, its artifacts join the context.
    pub session: Option<SessionId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AskStarted {
    /// Stream the answer on `execution:<id>`.
    pub execution: ExecutionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushSubscription {
    pub endpoint: String,
    pub keys: PushKeys,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushKeys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ServerInfo {
    pub version: String,
    /// VAPID public key (URL-safe base64) for push subscriptions.
    pub vapid_public_key: String,
}

/// Directory listing for the folder picker (`GET /api/fs/list?path=`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsListing {
    #[ts(type = "string")]
    pub path: PathBuf,
    #[ts(type = "string | null")]
    pub parent: Option<PathBuf>,
    pub entries: Vec<FsEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    /// The directory holds `.git`.
    pub is_git: bool,
    /// The directory holds `.ostra/INVENTORY.md`.
    pub is_ostra_project: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuthExchange {
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiError {
    pub error: String,
    #[serde(default)]
    pub issues: Vec<ValidationIssue>,
}

// ---------------------------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------------------------

/// Channels: `session:<id>`, `execution:<id>`, `term:<execution>`, `workspace:<id>`, `home`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ClientMsg {
    Subscribe { channel: String },
    Unsubscribe { channel: String },
    TermInput { execution: ExecutionId, data: String },
    TermResize { execution: ExecutionId, cols: u16, rows: u16 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ServerMsg {
    /// An engine event appended to a session.
    SessionEvent { session: SessionId, seq: i64, at: DateTime<Utc>, event: SessionEvent },
    /// Streamed activity of one execution.
    ExecutionDelta { execution: ExecutionId, seq: i64, at: DateTime<Utc>, delta: ExecutionDelta },
    ExecutionStatus { execution: ExecutionId, status: ExecutionStatus },
    SessionUpdated { summary: SessionSummary },
    WorkspaceUpdated { workspace: WorkspaceId },
    HarnessStatus { statuses: Vec<HarnessStatus> },
    Subscribed { channel: String },
    Error { message: String },
}

#[cfg(test)]
mod tests {
    #[test]
    fn export_bindings() {
        // ts-rs writes every `#[ts(export)]` type when its generated tests run; this test exists so
        // `cargo test -p ostra-core export_bindings` is a stable command to regenerate them.
    }
}
