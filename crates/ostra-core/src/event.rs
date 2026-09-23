//! The append-only session event log. A session's state is the fold of its events, so a restart
//! replays and continues.

use crate::agent::{AgentName, InitializerMode};
use crate::exec::ExecutionResult;
use crate::executor::{ExecutorKind, HarnessKind};
use crate::ids::{DecisionId, ExecutionId, GateId};
use crate::pipeline::{PhaseInfo, Question, QuestionAnswer, StageKind};
use crate::policy::{PermissionAnswer, RuleRef, ToolCall};
use crate::submit::{FactCheckFinding, ReviewFinding};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum SessionKind {
    Pipeline,
    Init { project: String },
}

/// Toggles from the New task form. A toggle that is on is an explicit request (Rule T3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(default)]
#[ts(export)]
pub struct SessionOptions {
    pub tests: bool,
    pub docs: bool,
    pub yolo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectRef {
    pub key: String,
    #[ts(type = "string")]
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum JudgeKind {
    Classify,
    Sufficiency,
    Stakes,
    RouteAnswer,
    Rescue,
    ResolveReview,
    YoloAnswer,
    Completion,
}

impl JudgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JudgeKind::Classify => "classify",
            JudgeKind::Sufficiency => "sufficiency",
            JudgeKind::Stakes => "stakes",
            JudgeKind::RouteAnswer => "route-answer",
            JudgeKind::Rescue => "rescue",
            JudgeKind::ResolveReview => "resolve-review",
            JudgeKind::YoloAnswer => "yolo-answer",
            JudgeKind::Completion => "completion",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum FactTarget {
    Spec,
    Plan,
}

impl FactTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            FactTarget::Spec => "spec",
            FactTarget::Plan => "plan",
        }
    }
}

/// Why an implementer or write-test execution ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkKind {
    Initial,
    /// HIGH and MEDIUM findings from review.
    Fix,
    /// BLOCKER findings only, with a removal instruction (Hard rule 21).
    BlockerFix,
    /// Continue after a HANDOFF specialist finished.
    Resume,
    /// Re-run with a stated fact after STUCK.
    Rescue,
    /// Re-run after the server restarted mid-execution.
    Rerun,
}

/// What an execution was for. The engine's fold keys on this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum ExecPurpose {
    Explore { task: u32 },
    Spec { round: u32 },
    FactCheck { target: FactTarget, pass: u32 },
    Plan { round: u32 },
    Implement { phase: u32, work: WorkKind },
    /// `tests` marks the closing test loop (`Phase: N-tests`).
    Review { phase: u32, tests: bool, iteration: u32 },
    Epa { phase: u32 },
    WriteTest { phase: u32, work: WorkKind },
    ModuleDocs { project: String },
    PromptGen { handoff_for: Option<ExecutionId> },
    Verify { phase: u32 },
    QuickAnswer,
    Init { mode: InitializerMode, item: Option<String> },
}

/// A pending gate. The UI renders one card per variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum GatePayload {
    OpenQuestions {
        /// `spec` or `plan`.
        artifact: String,
        #[ts(type = "string")]
        artifact_path: PathBuf,
        questions: Vec<Question>,
    },
    SpecApproval {
        #[ts(type = "string")]
        spec_path: PathBuf,
        summary: String,
        /// LOW findings from the passing fact-check.
        findings: Vec<FactCheckFinding>,
    },
    PlanApproval {
        #[ts(type = "string")]
        plan_path: PathBuf,
        summary: String,
        phases: Vec<PhaseInfo>,
        findings: Vec<FactCheckFinding>,
    },
    /// The same fact-check finding keeps recurring.
    FactCheckRecurring { target: FactTarget, passes: u32, findings: Vec<FactCheckFinding> },
    ReviewCap {
        project: String,
        phase: u32,
        tests: bool,
        iterations: u32,
        findings: Vec<ReviewFinding>,
        #[ts(type = "string")]
        ledger_path: PathBuf,
    },
    Stuck {
        execution: ExecutionId,
        agent: AgentName,
        project: String,
        phase: Option<u32>,
        diagnostic: String,
        need: String,
    },
    PhaseBlocked { project: String, phase: u32, reason: String },
    ClosingGate { items: Vec<ClosingItem> },
    Permission {
        execution: ExecutionId,
        agent: AgentName,
        call: ToolCall,
        reason: String,
        rule: RuleRef,
        /// The rule "always in this workspace" would add.
        suggestion: Option<String>,
    },
    HarnessFailure { execution: ExecutionId, harness: HarnessKind, error: String },
    SkillApproval { project: String, skills: Vec<SkillProposal> },
    /// An execution failed or was cancelled. Answer `retry` or `abandon`.
    ExecutionFailed { execution: ExecutionId, agent: AgentName, project: String, error: String },
    /// The session reached its budget. Answer `raise` (with the extra dollars as `text`) or `stop`.
    /// YOLO never answers it, because spending more is the user's decision.
    BudgetReached { spent_usd: f64, budget_usd: f64 },
}

impl GatePayload {
    pub fn stage(&self) -> StageKind {
        match self {
            GatePayload::OpenQuestions { .. } => StageKind::OpenQuestions,
            GatePayload::SpecApproval { .. } => StageKind::SpecApproval,
            GatePayload::PlanApproval { .. } => StageKind::PlanApproval,
            GatePayload::FactCheckRecurring { target: FactTarget::Spec, .. } => StageKind::FactCheckSpec,
            GatePayload::FactCheckRecurring { target: FactTarget::Plan, .. } => StageKind::FactCheckPlan,
            GatePayload::ReviewCap { tests: false, .. } => StageKind::Review,
            GatePayload::ReviewCap { tests: true, .. } => StageKind::TestReview,
            GatePayload::Stuck { .. } => StageKind::Rescue,
            GatePayload::PhaseBlocked { .. } => StageKind::Implement,
            GatePayload::ClosingGate { .. } => StageKind::ClosingGate,
            GatePayload::Permission { .. } => StageKind::Implement,
            GatePayload::HarnessFailure { .. } => StageKind::Implement,
            GatePayload::SkillApproval { .. } => StageKind::SkillApproval,
            GatePayload::ExecutionFailed { .. } => StageKind::Implement,
            GatePayload::BudgetReached { .. } => StageKind::Intake,
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            GatePayload::OpenQuestions { .. } => "open_questions",
            GatePayload::SpecApproval { .. } => "spec_approval",
            GatePayload::PlanApproval { .. } => "plan_approval",
            GatePayload::FactCheckRecurring { .. } => "fact_check_recurring",
            GatePayload::ReviewCap { .. } => "review_cap",
            GatePayload::Stuck { .. } => "stuck",
            GatePayload::PhaseBlocked { .. } => "phase_blocked",
            GatePayload::ClosingGate { .. } => "closing_gate",
            GatePayload::Permission { .. } => "permission",
            GatePayload::HarnessFailure { .. } => "harness_failure",
            GatePayload::SkillApproval { .. } => "skill_approval",
            GatePayload::ExecutionFailed { .. } => "execution_failed",
            GatePayload::BudgetReached { .. } => "budget_reached",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClosingItem {
    pub project: String,
    pub phases: u32,
    /// False when the request already opted in or out of tests (Rule T3).
    pub ask_tests: bool,
    pub ask_docs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillProposal {
    pub name: String,
    pub kind: String,
    pub description: String,
    /// `generate`, `regenerate`, `reuse`, or `drop`.
    pub disposition: String,
    /// Exemplar files the skill is grounded in.
    #[serde(default)]
    pub exemplars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum GateAnswer {
    Questions { answers: Vec<QuestionAnswer> },
    /// `feedback` with `approved: false` is a change request.
    Approval { approved: bool, feedback: Option<String> },
    /// For review cap (`another-pass`, `stop`), fact-check recurring (`another-round`, `stop`),
    /// stuck (`fact`, `block`), phase blocked (`retry`, `leave`), harness failure (`retry`,
    /// `native`), execution failed (`retry`, `abandon`), budget reached (`raise`, `stop`). `text`
    /// carries a stated fact, instructions, or the extra budget in dollars.
    Choice { option: String, text: Option<String> },
    Closing { items: Vec<ClosingChoice> },
    Permission { answer: PermissionAnswer },
    Skills { decisions: Vec<SkillDecision> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClosingChoice {
    pub project: String,
    pub tests: bool,
    pub docs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillDecision {
    pub name: String,
    pub disposition: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AnswerSource {
    User,
    Yolo,
    /// Answered by the engine with no judgment needed (for example a closing gate with every
    /// stage already requested).
    Engine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CommandPurpose {
    Format,
    Stage,
    Autofix,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum SessionEvent {
    SessionCreated {
        kind: SessionKind,
        request: String,
        options: SessionOptions,
        projects: Vec<ProjectRef>,
        #[ts(type = "string")]
        workspace_root: PathBuf,
        #[ts(type = "string")]
        session_root: PathBuf,
    },
    /// The user extended or changed the request (Rules D2, D10).
    RequestAmended { text: String },
    YoloSet { enabled: bool },
    DecisionMade {
        id: DecisionId,
        judge: JudgeKind,
        /// What the decision is about: a gate id, an execution id, a phase.
        subject: Option<String>,
        input_summary: String,
        #[ts(type = "unknown")]
        output: serde_json::Value,
        reason: String,
    },
    DecisionOverridden {
        id: DecisionId,
        #[ts(type = "unknown")]
        output: serde_json::Value,
        reason: String,
    },
    ExecutionStarted {
        id: ExecutionId,
        agent: AgentName,
        purpose: ExecPurpose,
        stage: StageKind,
        project: String,
        executor: ExecutorKind,
        model: String,
        /// The spawn struct, as JSON.
        #[ts(type = "unknown")]
        params: serde_json::Value,
        /// The rendered `Label: value` block.
        spawn_block: String,
        #[ts(type = "string | null")]
        report_path: Option<PathBuf>,
        /// Execution this one resumes, if any.
        resumes: Option<ExecutionId>,
    },
    ExecutionFinished { id: ExecutionId, result: ExecutionResult },
    GateOpened { id: GateId, title: String, explanation: String, payload: GatePayload },
    GateAnswered {
        id: GateId,
        source: AnswerSource,
        answer: GateAnswer,
        /// Why the YOLO judge chose this answer.
        reason: Option<String>,
    },
    CommandRan {
        purpose: CommandPurpose,
        project: String,
        command: String,
        exit_code: Option<i32>,
        output_tail: String,
    },
    AutofixApplied {
        project: String,
        phase: u32,
        tests: bool,
        applied: Vec<String>,
        /// Findings that could not be applied mechanically, with the reason.
        failed: Vec<(String, String)>,
    },
    SecurityBlock { project: String, phase: u32, tests: bool, findings: Vec<ReviewFinding> },
    PhaseBlocked { project: String, phase: u32, tests: bool, reason: String },
    Note { message: String },
    SessionCompleted {
        #[ts(type = "string")]
        report_path: PathBuf,
        summary: String,
    },
    SessionFailed { error: String },
}

/// A stored event with its sequence number and time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StoredEvent {
    pub seq: i64,
    pub at: chrono::DateTime<chrono::Utc>,
    pub event: SessionEvent,
}
