//! The append-only session event log. A session's state is the fold of its events, so a restart
//! replays and continues.

use crate::agent::{AgentName, InitializerMode};
use crate::coord::{AskTarget, DeliveryKind, MessageTarget};
use crate::exec::ExecutionResult;
use crate::executor::{ExecutorKind, HarnessKind};
use crate::ids::{DecisionId, ExecutionId, GateId, MessageId};
use crate::pipeline::{PhaseInfo, Question, QuestionAnswer, StageKind, Track};
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
    /// A track the user forced for an `IMPLEMENT` request. Absent lets the Track judge decide.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub track: Option<Track>,
}

/// A file or folder the user attached as context: a path relative to one of the workspace's
/// projects. A folder's path ends in `/`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ContextFile {
    pub project: String,
    /// Project-relative and `/`-separated.
    pub path: String,
}

impl ContextFile {
    pub fn is_folder(&self) -> bool {
        self.path.ends_with('/')
    }

    /// The `@project/path` tag that names the file or folder in request text.
    pub fn tag(&self) -> String {
        format!("@{}/{}", self.project, self.path)
    }
}

/// A file the user uploaded as context, kept in the session's `uploads/` folder (Rule C3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UploadedFile {
    /// The file name as uploaded, made safe for the file system.
    pub name: String,
    #[ts(type = "string")]
    pub path: PathBuf,
    #[ts(type = "number")]
    pub size: u64,
}

/// When context added mid-session reaches the agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ContextDelivery {
    /// Running executions finish on the old request; the next step sees the new context.
    #[default]
    Queue,
    /// Running executions are interrupted and re-run with the new context.
    Now,
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
    Track,
    RouteAnswer,
    Feedback,
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
            JudgeKind::Track => "track",
            JudgeKind::Feedback => "feedback",
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
    Explore {
        task: u32,
    },
    Spec {
        round: u32,
    },
    FactCheck {
        target: FactTarget,
        pass: u32,
    },
    Plan {
        round: u32,
    },
    Implement {
        phase: u32,
        work: WorkKind,
    },
    /// `tests` marks the closing test loop (`Phase: N-tests`).
    Review {
        phase: u32,
        tests: bool,
        iteration: u32,
    },
    Epa {
        phase: u32,
    },
    WriteTest {
        phase: u32,
        work: WorkKind,
    },
    /// Rule B1: one project's part of the documentation book. Logs written before books say
    /// `module_docs`.
    #[serde(alias = "module_docs")]
    Docs {
        project: String,
    },
    /// Rule B4: the architecture of a book that covers two or more projects.
    Architecture,
    PromptGen {
        handoff_for: Option<ExecutionId>,
    },
    Verify {
        phase: u32,
    },
    QuickAnswer,
    Init {
        mode: InitializerMode,
        item: Option<String>,
    },
    /// The user reopened an ended harness execution's session to read it. Outside the pipeline.
    Inspect {
        of: ExecutionId,
    },
    /// Rule O5: the advisor's `round`-th look at `execution`, a failed step of project `project`.
    Advise {
        project: String,
        execution: ExecutionId,
        round: u32,
    },
    /// Rule O8: the `round`-th implementer the user sent to fix what keeps stuck run `execution` of
    /// phase `phase`'s build or test loop from finishing.
    Unblock {
        phase: u32,
        tests: bool,
        execution: ExecutionId,
        round: u32,
    },
    /// Logs written before messaging: subagent `subagent` answered question `ask` in a run that
    /// continued its conversation.
    Consult {
        subagent: ExecutionId,
        ask: MessageId,
    },
    /// Rule SM4: subagent `subagent` continues its conversation, with its own tools, to act on the
    /// messages sent to it after its run ended, the first of them `first`.
    Message {
        subagent: ExecutionId,
        first: MessageId,
    },
    /// Rule SM7: a custom helper agent `SendMessage` started for message `message`.
    Helper {
        message: MessageId,
    },
    /// Rule WF4: run `round` of workflow node `node`, for `scope`: the session when absent, else
    /// `phase:<n>` or `project:<key>`.
    Stage {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        round: u32,
    },
}

impl ExecPurpose {
    /// A run that covers every project in the session's scope. It still works in the primary
    /// project's folder, but belongs to no single project.
    pub fn spans_session(&self) -> bool {
        matches!(
            self,
            ExecPurpose::Spec { .. }
                | ExecPurpose::FactCheck { .. }
                | ExecPurpose::Plan { .. }
                | ExecPurpose::Architecture
                | ExecPurpose::QuickAnswer
        )
    }

    /// The label of one run within its execution group, before a pass number is added.
    pub fn run_label(&self) -> String {
        let work = |w: &WorkKind| match w {
            WorkKind::Initial | WorkKind::Rerun => "",
            WorkKind::Fix => " · fix pass",
            WorkKind::BlockerFix => " · blocker fix",
            WorkKind::Resume => " · resume",
            WorkKind::Rescue => " · rescue",
        };
        let item = |i: &Option<String>| i.as_deref().unwrap_or_default().trim().to_string();
        match self {
            ExecPurpose::Explore { task } => format!("Research task {}", task + 1),
            ExecPurpose::Spec { .. } => "Spec".into(),
            ExecPurpose::FactCheck {
                target: FactTarget::Spec,
                ..
            } => "Spec check".into(),
            ExecPurpose::FactCheck {
                target: FactTarget::Plan,
                ..
            } => "Plan check".into(),
            ExecPurpose::Plan { .. } => "Plan".into(),
            ExecPurpose::Implement { phase, work: w } => format!("Phase {phase}{}", work(w)),
            ExecPurpose::Review { phase, tests, .. } => {
                format!(
                    "Phase {phase}{} · review pass",
                    if *tests { " tests" } else { "" }
                )
            }
            ExecPurpose::Epa { phase } => format!("Phase {phase}"),
            ExecPurpose::WriteTest { phase, work: w } => format!("Phase {phase}{}", work(w)),
            ExecPurpose::Docs { project } => format!("Docs for {project}"),
            ExecPurpose::Architecture => "System architecture".into(),
            ExecPurpose::PromptGen {
                handoff_for: Some(_),
            } => "Handoff prompt".into(),
            ExecPurpose::PromptGen { handoff_for: None } => "Prompt".into(),
            ExecPurpose::Verify { phase } => format!("Phase {phase} · verification"),
            ExecPurpose::QuickAnswer => "Answer".into(),
            ExecPurpose::Inspect { .. } => "Read-only session".into(),
            ExecPurpose::Advise { project, .. } => format!("Advice for {project}"),
            ExecPurpose::Unblock { phase, .. } => format!("Phase {phase} · unblock"),
            ExecPurpose::Consult { .. } => "Answer".into(),
            ExecPurpose::Message { .. } => "Messages".into(),
            ExecPurpose::Helper { .. } => "Helper".into(),
            ExecPurpose::Stage { node, scope, .. } => match scope {
                Some(s) => format!("Stage {node} · {}", s.replace(':', " ")),
                None => format!("Stage {node}"),
            },
            ExecPurpose::Init { mode, item: i } => match mode {
                InitializerMode::Detect => "Detect the stack".into(),
                InitializerMode::Adopt => "Adopt a bootstrap".into(),
                InitializerMode::Scout => format!("Scout {}", item(i)).trim_end().to_string(),
                InitializerMode::Propose => "Propose skills".into(),
                InitializerMode::GenerateSkill => {
                    format!("Skill {}", item(i)).trim_end().to_string()
                }
                InitializerMode::GenerateInventory => "Inventory".into(),
            },
        }
    }
}

/// `base` for the first run of a label, and `base · pass n` (or `base n` when the label already
/// ends in "pass") for later ones.
pub fn numbered_run_label(base: &str, n: u32) -> String {
    if n <= 1 {
        base.to_string()
    } else if base.ends_with(" pass") {
        format!("{base} {n}")
    } else {
        format!("{base} · pass {n}")
    }
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
    FactCheckRecurring {
        target: FactTarget,
        passes: u32,
        findings: Vec<FactCheckFinding>,
    },
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
    PhaseBlocked {
        project: String,
        phase: u32,
        reason: String,
    },
    ClosingGate {
        items: Vec<ClosingItem>,
    },
    Permission {
        execution: ExecutionId,
        agent: AgentName,
        call: ToolCall,
        reason: String,
        rule: RuleRef,
        /// The rule "always in this workspace" would add.
        suggestion: Option<String>,
    },
    HarnessFailure {
        execution: ExecutionId,
        harness: HarnessKind,
        error: String,
    },
    SkillApproval {
        project: String,
        skills: Vec<SkillProposal>,
    },
    /// An execution failed or was cancelled. Answer `retry` or `abandon`.
    ExecutionFailed {
        execution: ExecutionId,
        agent: AgentName,
        project: String,
        error: String,
    },
    /// Every phase has finished. Answer `feedback` with the change the user wants as `text`, or
    /// `done` to accept the implementation and move on to the closing stages.
    ImplementationReview {
        /// 1 for the first review, then one more after each feedback round is built.
        round: u32,
        /// The engine-written file that lists the request, the artifacts, and every round.
        #[ts(type = "string")]
        context_path: PathBuf,
        /// Implementer reports of every finished phase, oldest first.
        #[ts(type = "string[]")]
        reports: Vec<PathBuf>,
        /// Phases that ended blocked, with the reason.
        blocked: Vec<String>,
    },
    /// Rule WF5: a workflow stage failed or needs the user. For a failure, answer `retry` (with
    /// guidance as `text`), `continue`, or `stop`. For a question, answer with the chosen option, or
    /// `other` with the answer as `text`, or `stop`.
    StageReview {
        /// The workflow node.
        stage: String,
        /// `phase:<n>` or `project:<key>` for a stage that runs per phase or project.
        scope: Option<String>,
        /// The agent whose run raised it; none when a plugin's stage logic asks.
        agent: Option<AgentName>,
        execution: Option<ExecutionId>,
        verdict: crate::submit::StageVerdict,
        summary: String,
        findings: Vec<crate::submit::CustomFinding>,
        question: Option<String>,
        options: Vec<String>,
        round: u32,
        max_rounds: u32,
    },
    /// The session reached its budget. Answer `raise` (with the extra dollars as `text`) or `stop`.
    /// YOLO never answers it, because spending more is the user's decision.
    BudgetReached {
        spent_usd: f64,
        budget_usd: f64,
    },
}

impl GatePayload {
    pub fn stage(&self) -> StageKind {
        match self {
            GatePayload::OpenQuestions { .. } => StageKind::OpenQuestions,
            GatePayload::SpecApproval { .. } => StageKind::SpecApproval,
            GatePayload::PlanApproval { .. } => StageKind::PlanApproval,
            GatePayload::FactCheckRecurring {
                target: FactTarget::Spec,
                ..
            } => StageKind::FactCheckSpec,
            GatePayload::FactCheckRecurring {
                target: FactTarget::Plan,
                ..
            } => StageKind::FactCheckPlan,
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
            GatePayload::ImplementationReview { .. } => StageKind::ImplementationReview,
            GatePayload::StageReview { .. } => StageKind::Custom,
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
            GatePayload::ImplementationReview { .. } => "implementation_review",
            GatePayload::StageReview { .. } => "stage_review",
        }
    }

    /// The execution the gate holds or is about, when it names one.
    pub fn execution(&self) -> Option<&ExecutionId> {
        match self {
            GatePayload::Stuck { execution, .. }
            | GatePayload::Permission { execution, .. }
            | GatePayload::HarnessFailure { execution, .. }
            | GatePayload::ExecutionFailed { execution, .. } => Some(execution),
            _ => None,
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
    Questions {
        answers: Vec<QuestionAnswer>,
    },
    /// `feedback` with `approved: false` is a change request.
    Approval {
        approved: bool,
        feedback: Option<String>,
    },
    /// For review cap (`another-pass`, `stop`), fact-check recurring (`another-round`, `stop`),
    /// stuck (`fact`, `block`), phase blocked (`retry`, `leave`), harness failure (`retry`,
    /// `native`), execution failed (`retry`, `abandon`), budget reached (`raise`, `stop`). `text`
    /// carries a stated fact, instructions, or the extra budget in dollars.
    Choice {
        option: String,
        text: Option<String>,
    },
    Closing {
        items: Vec<ClosingChoice>,
    },
    Permission {
        answer: PermissionAnswer,
    },
    Skills {
        decisions: Vec<SkillDecision>,
    },
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
        /// Files the user attached to the request (Rule C1).
        #[serde(default)]
        files: Vec<ContextFile>,
        /// Files the user uploaded with the request (Rule C3).
        #[serde(default)]
        uploads: Vec<UploadedFile>,
        /// Rule O6: the projects the user pinned, which are then the whole scope.
        #[serde(default)]
        pinned: Vec<String>,
        /// Rule B6: the documentation book the user picked for this session's docs. Absent names
        /// the book after the session's documented projects.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        docs_book: Option<String>,
        /// Rule WF1: the workflow the session asked for. Logs written before workflows have none
        /// and run the built-in pipeline of their category.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        workflow: Option<crate::workflow::WorkflowChoice>,
    },
    /// Rule WF1: the workflow the session runs, resolved from the workspace's files when the
    /// session needed it, so later edits to those files never change a running session.
    WorkflowResolved {
        workflow: crate::workflow::WorkflowDef,
    },
    /// Rule PL3: a plugin's stage logic decided the next step of workflow node `node`.
    StageDecided {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        decision: crate::plugin::StageDecision,
    },
    /// Rule WB5: node `node`'s conditions did not hold once the nodes it waits for were done, so
    /// it was skipped. A skipped node counts as done.
    StageSkipped {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
    /// Rules WB2 and WB3: a transform or prompt node ran in the engine and gave `output`, or
    /// failed with `error`. The output is recorded because a later Ostra may compute it
    /// differently, and the fold must stay a function of the log.
    NodeRan {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        round: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown")]
        output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// A prompt node's model cost.
        #[serde(default)]
        cost_usd: f64,
    },
    /// The user extended or changed the request, or added context (Rules D2, D10, C2).
    RequestAmended {
        text: String,
        #[serde(default)]
        files: Vec<ContextFile>,
        #[serde(default)]
        uploads: Vec<UploadedFile>,
        #[serde(default)]
        delivery: ContextDelivery,
        /// Rule C2: the context waits for the Route answer judge before anything starts. Context
        /// added before the rule has none and folds as it always did.
        #[serde(default)]
        routed: bool,
    },
    /// Rule O3: an agent created a project with `ProjectCreate`, and it joined the session.
    ProjectCreated {
        project: crate::manage::CreatedProject,
    },
    /// Rule O4: a created project's init wrote a usable inventory and profile.
    ProjectInitFinished {
        project: String,
    },
    /// Rule O5: a step of a created project's init left no usable result, found by the engine
    /// rather than by the step itself. `execution` is the step to run again.
    InitStepFailed {
        project: String,
        execution: ExecutionId,
        error: String,
    },
    /// The user paused the session (Rule P1).
    SessionPaused,
    /// The user continued a paused session (Rule P2).
    SessionResumed,
    /// Rule P3: an execution did something that looks like an attempt to leave its sandbox. The
    /// runner records at most [`crate::containment::PAUSE_AFTER`] per execution.
    ContainmentSignal {
        execution: ExecutionId,
        signal: crate::containment::ContainmentSignal,
    },
    YoloSet {
        enabled: bool,
    },
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
        /// Rule CA5: the result contract the run submits. Logs written before contracts have none,
        /// and their runs follow the standard agent's contract.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        contract: Option<crate::contract::Contract>,
    },
    /// Rule PL5: the plugin that owns a run's contract turned its result into this outcome.
    ResultHandled {
        execution: ExecutionId,
        outcome: crate::submit::CustomSubmit,
    },
    /// Rule PL8: plugin `plugin` saved `value` under `key` in this session, or removed the key
    /// (`value` absent). Its later calls read it, so a program that stopped knows where it was.
    PluginCheckpoint {
        plugin: String,
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown")]
        value: Option<serde_json::Value>,
    },
    /// Rule P2: an execution the pause interrupted runs again under its own id, continuing its
    /// conversation, Activity, and usage.
    ExecutionResumed {
        id: ExecutionId,
    },
    /// Rule U1: the user skipped this running execution. It stops, and its task ends without a
    /// result instead of re-running or opening a failure gate.
    ExecutionSkipped {
        id: ExecutionId,
    },
    /// Rule C2: the user withdrew queued context (`index` in the session's additions) before
    /// anything read it.
    AmendmentWithdrawn {
        index: u32,
    },
    /// Rule U2: the user sent this execution a correction. A running execution stops and resumes
    /// in place with `text` as its next message; a paused one takes `text` when it resumes.
    ExecutionSteered {
        id: ExecutionId,
        text: String,
    },
    /// Rule U2: the user withdrew the correction a paused execution had not read yet.
    SteerWithdrawn {
        id: ExecutionId,
    },
    /// Rule SM8: run `from` sent a message. With `wait`, the run pauses until a message arrives.
    MessageSent {
        id: MessageId,
        from: ExecutionId,
        to: MessageTarget,
        text: String,
        #[serde(default)]
        wait: bool,
    },
    /// Rule SM3: run `id` paused itself until a message arrives.
    AgentWaiting {
        id: ExecutionId,
    },
    /// Rule SM8: Ostra handed messages `ids` to run `to` at a turn boundary, with `notice` when
    /// Ostra had to tell a waiting run that no message will come.
    MessagesDelivered {
        to: ExecutionId,
        ids: Vec<MessageId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notice: Option<String>,
    },
    /// Logs written before messaging: a run asked a helper or another subagent, and waited.
    AgentAsked {
        id: MessageId,
        from: ExecutionId,
        target: AskTarget,
        message: String,
    },
    /// Logs written before messaging: a run answered question `ask`.
    AgentReplied {
        ask: MessageId,
        from: ExecutionId,
        message: String,
    },
    /// Logs written before messaging: Ostra handed the question or the answer of `ask` to run `to`.
    MessageDelivered {
        ask: MessageId,
        to: ExecutionId,
        kind: DeliveryKind,
    },
    ExecutionFinished {
        id: ExecutionId,
        result: ExecutionResult,
    },
    GateOpened {
        id: GateId,
        title: String,
        explanation: String,
        payload: GatePayload,
    },
    GateAnswered {
        id: GateId,
        source: AnswerSource,
        answer: GateAnswer,
        /// Why the YOLO judge chose this answer.
        reason: Option<String>,
        /// Rule J1: the answer waits for the Route answer or Feedback judge before it is applied.
        /// Answers recorded before the rule have none and fold as they always did.
        #[serde(default)]
        routed: bool,
    },
    /// Appended before a command process starts, so a long format run shows on the board.
    CommandStarted {
        purpose: CommandPurpose,
        project: String,
        command: String,
    },
    CommandRan {
        purpose: CommandPurpose,
        project: String,
        command: String,
        exit_code: Option<i32>,
        output_tail: String,
    },
    /// Rule B9: logs from before one writer per project split a large project's part into areas.
    /// The fold ignores it.
    DocsPlanned {
        project: String,
    },
    /// Rule B5: the engine wrote the session's documentation into a workspace book. `error` is set
    /// when the files could not be written, and the session goes on without them.
    BookWritten {
        book: String,
        /// The projects whose parts this session wrote.
        projects: Vec<String>,
        #[serde(default)]
        error: Option<String>,
    },
    AutofixApplied {
        project: String,
        phase: u32,
        tests: bool,
        applied: Vec<String>,
        /// Findings that could not be applied mechanically, with the reason.
        failed: Vec<(String, String)>,
    },
    SecurityBlock {
        project: String,
        phase: u32,
        tests: bool,
        findings: Vec<ReviewFinding>,
    },
    PhaseBlocked {
        project: String,
        phase: u32,
        tests: bool,
        reason: String,
    },
    Note {
        message: String,
    },
    SessionCompleted {
        #[ts(type = "string")]
        report_path: PathBuf,
        summary: String,
    },
    SessionFailed {
        error: String,
    },
}

/// A stored event with its sequence number and time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StoredEvent {
    pub seq: i64,
    pub at: chrono::DateTime<chrono::Utc>,
    pub event: SessionEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_labels() {
        let imp = |work| ExecPurpose::Implement { phase: 2, work };
        assert_eq!(imp(WorkKind::Initial).run_label(), "Phase 2");
        assert_eq!(imp(WorkKind::Fix).run_label(), "Phase 2 · fix pass");
        assert_eq!(ExecPurpose::Spec { round: 3 }.run_label(), "Spec");
        assert_eq!(
            ExecPurpose::Review {
                phase: 1,
                tests: true,
                iteration: 2
            }
            .run_label(),
            "Phase 1 tests · review pass"
        );
        assert_eq!(
            ExecPurpose::Explore { task: 0 }.run_label(),
            "Research task 1"
        );
        assert_eq!(
            ExecPurpose::Init {
                mode: InitializerMode::Scout,
                item: Some("api".into())
            }
            .run_label(),
            "Scout api"
        );
        assert_eq!(numbered_run_label("Phase 1", 1), "Phase 1");
        assert_eq!(numbered_run_label("Spec", 2), "Spec · pass 2");
        assert_eq!(
            numbered_run_label("Phase 1 · fix pass", 3),
            "Phase 1 · fix pass 3"
        );
    }
}
