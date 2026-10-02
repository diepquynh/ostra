//! A session's state is the fold of its events (HANDOVER 8.1). Everything here is deterministic:
//! the same events always produce the same state, so a restart replays and continues.

use crate::judge::{
    ANSWER_ITEM, AnswerItem, AnswerRoute, ClassifyOut, Disposition, ExploreTaskSpec, FeedbackOut,
    FeedbackTarget, MAX_ANSWER_RESEARCH, MAX_SUFFICIENCY_RESEARCH, NoteStage, OptsIn, RescueAction,
    RescueOut, ResolveAction, ResolveReviewOut, RouteAnswerOut, StakesOut, SufficiencyOut,
    TrackOut, clean_title, item_for, parts_for,
};
use chrono::{DateTime, Utc};
use ostra_core::agent::AgentName;
use ostra_core::book::{ArchitectureSubmit, DocumentationSubmit};
use ostra_core::containment::{ContainmentSignal, PAUSE_AFTER};
use ostra_core::event::{
    AnswerSource, CommandPurpose, ContextDelivery, ContextFile, ExecPurpose, FactTarget,
    GateAnswer, GatePayload, JudgeKind, ProjectRef, SessionEvent, SessionKind, SessionOptions,
    StoredEvent, UploadedFile, WorkKind,
};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId};
use ostra_core::model::Complexity;
use ostra_core::paths;
use ostra_core::pipeline::{
    Category, PhaseInfo, QuestionAnswer, StageKind, Stakes, TestPolicy, Track,
};
use ostra_core::submit::{
    CodeReviewerSubmit, ExploreSubmit, FactCheckSubmit, GenerateSpecSubmit, HandoffInfo,
    ImplementerSubmit, InitializerSubmit, PlanSubmit, QuickAnswerSubmit, ReportSubmit,
    ReviewFinding, Severity, StuckInfo, SubmitStatus, Verdict,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Consecutive fact-check FAILs before the engine stops and asks (Rule D3 step 4).
pub const FACTCHECK_RECURRING_LIMIT: u32 = 3;
/// Review passes per loop before the 4th becomes a gate (Step 4).
pub const REVIEW_CAP: u32 = 3;
/// Review passes per loop under YOLO (review-cap.js).
pub const YOLO_REVIEW_BUDGET: u32 = 10;
/// Automatic retries of an execution that ended in an error, before a gate.
pub const ERROR_RETRIES: u32 = 1;
/// Sufficiency rounds before the engine proceeds to the spec with what it has.
pub const SUFFICIENCY_ROUNDS: u32 = 3;
/// Key in a review execution's params holding the project's auto-fixable rule IDs.
pub const AUTO_FIXABLE_PARAM: &str = "auto_fixable_ids";
/// `Prior findings:` after a pass that found nothing: a re-pass, not a first pass (Rule D3a).
pub const NO_PRIOR_FINDINGS: &str = "no findings on the previous pass";

#[derive(Debug, Clone, PartialEq)]
pub enum ExploreOrigin {
    Classify,
    Sufficiency,
    Amendment,
    Rescue {
        phase: u32,
        tests: bool,
    },
    /// Rule J1: research the Route answer or Feedback judge queued for an answer before the spec.
    Answer,
    /// Rule J1: research for an answer at a phase's gate; only that loop waits for it.
    LoopAnswer {
        phase: u32,
        tests: bool,
    },
    /// Rule H3: a helper a subagent started with `SubagentAsk`; only the asker waits for it.
    Ask {
        ask: ostra_core::ids::MessageId,
    },
}

impl ExploreOrigin {
    /// Research one work loop waits for, which never holds the rest of the session.
    pub fn loop_bound(&self) -> bool {
        matches!(
            self,
            ExploreOrigin::Rescue { .. }
                | ExploreOrigin::LoopAnswer { .. }
                | ExploreOrigin::Ask { .. }
        )
    }
}

/// Rule J1: a spec or plan answer waiting for the Route answer judge.
#[derive(Debug, Clone)]
pub enum HeldAnswer {
    Questions {
        target: FactTarget,
        answers: Vec<QuestionAnswer>,
    },
    Approval {
        target: FactTarget,
        /// The user approved a version that could be approved when they answered.
        approve: bool,
        version: u32,
        text: String,
    },
    Recurring {
        target: FactTarget,
        text: String,
    },
}

impl HeldAnswer {
    pub fn target(&self) -> FactTarget {
        match self {
            HeldAnswer::Questions { target, .. }
            | HeldAnswer::Approval { target, .. }
            | HeldAnswer::Recurring { target, .. } => *target,
        }
    }
}

/// The spec or plan track as Rule J1 routing sees it.
pub trait RoutingTrack {
    fn set_routing(&mut self, gate: Option<GateId>);
    fn clear_questions_gate(&mut self);
}

impl<T> RoutingTrack for ArtifactTrack<T> {
    fn set_routing(&mut self, gate: Option<GateId>) {
        self.routing = gate;
    }
    fn clear_questions_gate(&mut self) {
        self.questions_gate = None;
    }
}

/// An approval answer's decision and its non-empty text, when it has text.
fn approval_text(answer: &GateAnswer) -> Option<(bool, String)> {
    match answer {
        GateAnswer::Approval {
            approved,
            feedback: Some(t),
        } if !t.trim().is_empty() => Some((*approved, t.clone())),
        _ => None,
    }
}

/// Rule J1: whether an answer carries content the judge routes before it is applied. A bare
/// choice, such as approve, stop, retry, or a budget raise, has one meaning and is applied as is.
pub fn answer_needs_route(payload: &GatePayload, answer: &GateAnswer) -> bool {
    let text = |t: &Option<String>| t.as_deref().is_some_and(|t| !t.trim().is_empty());
    match (payload, answer) {
        (GatePayload::OpenQuestions { .. }, GateAnswer::Questions { answers }) => {
            !answers.is_empty()
        }
        (
            GatePayload::SpecApproval { .. } | GatePayload::PlanApproval { .. },
            GateAnswer::Approval { feedback, .. },
        ) => text(feedback),
        (GatePayload::FactCheckRecurring { .. }, GateAnswer::Choice { option, text: t }) => {
            option != "stop" && text(t)
        }
        (GatePayload::ReviewCap { .. }, GateAnswer::Choice { option, text: t }) => {
            option == "another-pass" && text(t)
        }
        (GatePayload::Stuck { .. }, GateAnswer::Choice { option, text: t }) => {
            option == "fact" && text(t)
        }
        (GatePayload::PhaseBlocked { .. }, GateAnswer::Choice { option, text: t }) => {
            option == "retry" && text(t)
        }
        (GatePayload::ImplementationReview { .. }, GateAnswer::Choice { option, text: t }) => {
            option == "feedback" && text(t)
        }
        _ => false,
    }
}

/// Rule J1: part of an answer the judge kept for later stages.
#[derive(Debug, Clone)]
pub struct UserNote {
    /// `N1`, `N2`, ... in the order kept, so a later answer can take one back.
    pub id: String,
    /// A later answer took it back; it stays for traceability and reaches no agent.
    pub forgotten: bool,
    pub stages: Vec<NoteStage>,
    pub text: String,
    /// The gate answered, or none for context the user added (Rule C2).
    pub gate: Option<GateId>,
}

#[derive(Debug, Clone)]
pub struct ExploreTask {
    pub idx: u32,
    pub project: String,
    pub task: String,
    pub origin: ExploreOrigin,
    pub exec: Option<ExecutionId>,
    pub running: bool,
    pub result: Option<ExploreSubmit>,
    /// An error that exhausted retries, waiting for a gate answer.
    pub failed: Option<String>,
    pub abandoned: bool,
    pub retries: u32,
    pub judged: bool,
    pub gate: Option<GateId>,
}

impl ExploreTask {
    pub fn finished(&self) -> bool {
        self.result.is_some() || self.abandoned
    }
}

#[derive(Debug, Clone)]
pub struct FactCheckPass {
    pub exec: ExecutionId,
    pub version: u32,
    pub result: Option<FactCheckSubmit>,
}

/// How much of a generator's accumulated input one run was given.
#[derive(Debug, Clone, Default)]
pub struct InputMark {
    pub answers: usize,
    pub changes: usize,
    pub docs: Vec<PathBuf>,
}

/// Generation and fact-check state of the spec or the plan.
#[derive(Debug, Clone, Default)]
pub struct ArtifactTrack<T> {
    pub runs: Vec<ExecutionId>,
    pub running: Option<ExecutionId>,
    pub current: Option<T>,
    pub version: u32,
    /// An input landed after the last run started: answers, findings, or a change.
    pub needs_run: bool,
    pub answers: Vec<QuestionAnswer>,
    pub changes: Vec<String>,
    /// The input the running generation was given.
    pub sent: InputMark,
    /// The input the current artifact already reflects, so a revision gets only what is new.
    pub applied: InputMark,
    pub pending_findings: Option<String>,
    pub checks: Vec<FactCheckPass>,
    pub check_running: Option<ExecutionId>,
    pub consecutive_fails: u32,
    pub fail_limit_extra: u32,
    pub approved: bool,
    pub approved_version: u32,
    pub approval_gate: Option<GateId>,
    pub approval_asked_version: u32,
    pub questions_gate: Option<GateId>,
    pub questions_asked_version: u32,
    /// Rule J1: an answer about this artifact waits for the Route answer judge.
    pub routing: Option<GateId>,
    pub recurring_gate: Option<GateId>,
    pub failed: Option<String>,
    pub failed_gate: Option<GateId>,
    pub error_retries: u32,
    pub stopped: bool,
    /// The plan must be regenerated once the spec is approved again (Rule D10).
    pub invalidated: bool,
}

impl<T> ArtifactTrack<T> {
    pub fn new() -> Self {
        ArtifactTrack {
            runs: vec![],
            running: None,
            current: None,
            version: 0,
            needs_run: false,
            answers: vec![],
            changes: vec![],
            sent: InputMark::default(),
            applied: InputMark::default(),
            pending_findings: None,
            checks: vec![],
            check_running: None,
            consecutive_fails: 0,
            fail_limit_extra: 0,
            approved: false,
            approved_version: 0,
            approval_gate: None,
            approval_asked_version: 0,
            questions_gate: None,
            questions_asked_version: 0,
            routing: None,
            recurring_gate: None,
            failed: None,
            failed_gate: None,
            error_retries: 0,
            stopped: false,
            invalidated: false,
        }
    }

    /// The finished fact-check pass over the current version, if any.
    pub fn check_for_current(&self) -> Option<&FactCheckSubmit> {
        self.checks
            .iter()
            .rev()
            .find(|c| c.version == self.version && c.result.is_some())
            .and_then(|c| c.result.as_ref())
    }

    pub fn passed_current(&self) -> bool {
        self.check_for_current()
            .is_some_and(|c| c.verdict == Verdict::Pass)
    }

    /// Findings of the previous pass over this artifact, or `none` on its first pass (Rule D3a).
    pub fn prior_findings(&self) -> String {
        match self.checks.iter().rev().find_map(|c| c.result.as_ref()) {
            // A clean pass still leaves a snapshot, so the next pass diffs instead of starting over.
            Some(r) if r.findings.is_empty() => NO_PRIOR_FINDINGS.into(),
            Some(r) => r.findings_text(),
            None => "none".into(),
        }
    }

    pub fn has_pass(&self) -> bool {
        self.checks.iter().any(|c| c.result.is_some())
    }

    /// Answers the current artifact does not reflect yet: all of them before the first artifact.
    pub fn pending_answers(&self) -> Vec<QuestionAnswer> {
        let from = if self.current.is_some() {
            self.applied.answers
        } else {
            0
        };
        self.answers.get(from..).unwrap_or_default().to_vec()
    }

    pub fn pending_changes(&self) -> Vec<String> {
        let from = if self.current.is_some() {
            self.applied.changes
        } else {
            0
        };
        self.changes.get(from..).unwrap_or_default().to_vec()
    }

    pub fn busy(&self) -> bool {
        self.running.is_some() || self.check_running.is_some()
    }

    fn start_check(&mut self, id: &ExecutionId) {
        self.check_running = Some(id.clone());
        self.checks.push(FactCheckPass {
            exec: id.clone(),
            version: self.version,
            result: None,
        });
    }

    fn revoke_approval(&mut self) {
        self.approved = false;
        self.approval_gate = None;
    }
}

/// What a work loop (implement or test, with its review loop) does next.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopNext {
    Idle,
    Work {
        kind: WorkKind,
        instructions: Option<String>,
    },
    Review,
    Autofix {
        apply: Vec<ReviewFinding>,
        remaining: Vec<ReviewFinding>,
    },
    Rescue {
        exec: ExecutionId,
        stuck: StuckInfo,
    },
    RescueExplore {
        task: u32,
        stuck: StuckInfo,
    },
    /// Rule O7: the advisor looks at a stuck run whose failure is in its environment.
    RescueAdvise {
        exec: ExecutionId,
        stuck: StuckInfo,
        advisor: Option<ExecutionId>,
    },
    RescueGate {
        exec: ExecutionId,
        stuck: StuckInfo,
    },
    /// Rule O8: an implementer the user sent fixes what keeps the stuck run `exec` from finishing.
    RescueFix {
        exec: ExecutionId,
        stuck: StuckInfo,
        /// The user's words, or none to work from the diagnostic alone.
        instructions: Option<String>,
        fixer: Option<ExecutionId>,
    },
    /// A gate answer with free text waits for the Route answer judge (Rule J1).
    AwaitRoute {
        gate: GateId,
        /// The user's words.
        text: String,
        then: WorkKind,
        /// Fix instructions the delivered text is added to.
        base: Option<String>,
        /// The STUCK report a delivered fact answers.
        stuck: Option<StuckInfo>,
        /// What the loop does when the judge delivers nothing.
        fallback: Box<LoopNext>,
    },
    /// Rule J1: research the judge queued for an answer; `next` runs when every task finished.
    AnswerResearch {
        tasks: Vec<u32>,
        next: Box<LoopNext>,
    },
    Handoff {
        exec: ExecutionId,
        handoff: HandoffInfo,
    },
    CapReached {
        findings: Vec<ReviewFinding>,
    },
    Resolve {
        findings: Vec<ReviewFinding>,
    },
    Stage,
    Failed {
        exec: ExecutionId,
        error: String,
    },
    Done,
    Blocked {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct WorkLoop {
    pub tests: bool,
    pub work_agent: AgentName,
    pub fix_agent: AgentName,
    /// Whether a review follows each work pass. PROMPT reviews only when code changed.
    pub review: ReviewMode,
    /// Whether passing files are staged with `git add`.
    pub stage: bool,
    pub next: LoopNext,
    pub running: Option<ExecutionId>,
    /// The step the running execution is performing, restored if it is interrupted.
    pub in_flight: Option<LoopNext>,
    pub iterations: u32,
    pub extra_cap: u32,
    pub last_review: Option<CodeReviewerSubmit>,
    pub leftover_low: Vec<ReviewFinding>,
    pub blocker_open: bool,
    pub resolve_rounds: u32,
    pub open_before_resolve: Option<usize>,
    pub error_retries: u32,
    pub changed: BTreeSet<String>,
    pub report: Option<PathBuf>,
    pub gate: Option<GateId>,
    pub rationale: Option<String>,
    pub announced_block: bool,
    pub block_gate_answered: bool,
    pub work_count: u32,
    /// The stage command ran and exited 0 for this loop's changed files.
    pub staged: bool,
    /// Rule O7: guidance the advisor gave this loop's stuck runs, oldest first.
    pub advice: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewMode {
    Always,
    /// Only when a changed file is not an instruction file (PROMPT category).
    IfCodeChanged,
    Never,
}

impl WorkLoop {
    pub fn new(tests: bool, work_agent: AgentName, fix_agent: AgentName) -> Self {
        WorkLoop {
            tests,
            work_agent,
            fix_agent,
            review: ReviewMode::Always,
            stage: true,
            next: LoopNext::Idle,
            running: None,
            in_flight: None,
            iterations: 0,
            extra_cap: 0,
            last_review: None,
            leftover_low: vec![],
            blocker_open: false,
            resolve_rounds: 0,
            open_before_resolve: None,
            error_retries: 0,
            changed: BTreeSet::new(),
            report: None,
            gate: None,
            rationale: None,
            announced_block: false,
            block_gate_answered: false,
            work_count: 0,
            staged: false,
            advice: vec![],
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.next, LoopNext::Done | LoopNext::Blocked { .. }) && self.running.is_none()
    }

    pub fn is_done(&self) -> bool {
        matches!(self.next, LoopNext::Done) && self.running.is_none()
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self.next, LoopNext::Blocked { .. })
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.next, LoopNext::Idle) && self.running.is_none()
    }

    pub fn effective_cap(&self, yolo: bool) -> u32 {
        (if yolo { YOLO_REVIEW_BUDGET } else { REVIEW_CAP }) + self.extra_cap
    }

    /// Decide what follows a review whose HIGH and MEDIUM findings are `remaining`.
    fn after_findings(&mut self, remaining: Vec<ReviewFinding>, yolo: bool, fix_ledger: &str) {
        if remaining.is_empty() {
            self.next = if self.stage {
                LoopNext::Stage
            } else {
                LoopNext::Done
            };
            return;
        }
        if self.iterations >= self.effective_cap(yolo) {
            if yolo {
                // YOLO resolution repeats while it converges (review-cap.js).
                let converging = match self.open_before_resolve {
                    None => true,
                    Some(before) => remaining.len() < before,
                };
                self.next = if self.resolve_rounds == 0 || converging {
                    LoopNext::Resolve {
                        findings: remaining,
                    }
                } else {
                    LoopNext::Blocked {
                        reason: format!(
                            "The review loop stopped converging after {} resolution rounds; {} findings remain open. Ledger: {fix_ledger}",
                            self.resolve_rounds,
                            remaining.len()
                        ),
                    }
                };
            } else {
                self.next = LoopNext::CapReached {
                    findings: remaining,
                };
            }
            return;
        }
        self.next = LoopNext::Work {
            kind: WorkKind::Fix,
            instructions: Some(fix_instructions(&remaining, fix_ledger)),
        };
    }
}

/// HIGH and MEDIUM findings for the fix agent, verbatim, plus the ledger path (Step 4 item 6).
pub fn fix_instructions(findings: &[ReviewFinding], ledger: &str) -> String {
    let mut s = String::from("Fix exactly these review findings, and nothing else:\n");
    for f in findings {
        s.push_str("- ");
        s.push_str(&f.line());
        s.push('\n');
    }
    s.push_str(&format!(
        "\nRecord a FIXED or WONTFIX line with your rationale for each finding in the review ledger at {ledger}, because the reviewer reads it on the next pass."
    ));
    s
}

/// BLOCKER findings only, with a removal instruction (Hard rule 21).
pub fn blocker_instructions(findings: &[ReviewFinding], ledger: &str) -> String {
    let mut s = String::from(
        "The security scan raised BLOCKER findings. Remove the dangerous code each one names. Do not rewrite it to keep its effect, because a secure reimplementation is a separate request the user makes once they understand the risk.\n",
    );
    for f in findings.iter().filter(|f| f.severity == Severity::Blocker) {
        s.push_str("- ");
        s.push_str(&f.line());
        s.push('\n');
    }
    s.push_str(&format!(
        "\nRecord each removal in the review ledger at {ledger}."
    ));
    s
}

#[derive(Debug, Clone, PartialEq)]
pub enum EpaState {
    NotStarted,
    Running(ExecutionId),
    Done(PathBuf),
    Failed {
        exec: ExecutionId,
        error: String,
        gate: Option<GateId>,
        retries: u32,
    },
    Abandoned,
}

#[derive(Debug, Clone)]
pub struct PhaseRun {
    pub info: PhaseInfo,
    pub impl_loop: WorkLoop,
    pub test_loop: WorkLoop,
    pub epa: EpaState,
    pub implementer_report: Option<PathBuf>,
    pub blocked_gate: Option<GateId>,
    /// Set on a phase built from the user's feedback after the implementation.
    pub revision: Option<Revision>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Revision {
    /// 1-based feedback round.
    pub round: u32,
    pub instruction: String,
}

/// The review after every phase finished: feedback rounds until the user accepts.
#[derive(Debug, Clone, Default)]
pub struct FeedbackTrack {
    pub rounds: Vec<FeedbackRound>,
    pub gate: Option<GateId>,
    pub accepted: bool,
}

#[derive(Debug, Clone)]
pub struct FeedbackRound {
    pub text: String,
    /// `None` until the round is routed, by the Feedback judge or directly.
    pub route: Option<AnswerRoute>,
    pub reason: Option<String>,
    pub targets: Vec<FeedbackTarget>,
    /// A requirement change waits for the spec to be approved again before it is built.
    pub awaiting_spec: bool,
    pub phases: Vec<u32>,
}

/// A docs-stage execution: a project's part of the book, or the book's architecture.
#[derive(Debug, Clone, PartialEq)]
pub enum StageRun<T> {
    NotStarted,
    Running(ExecutionId),
    Done(Box<T>),
    Failed {
        exec: ExecutionId,
        error: String,
        gate: Option<GateId>,
        retries: u32,
    },
    Abandoned,
}

impl<T> StageRun<T> {
    pub fn is_settled(&self) -> bool {
        matches!(self, StageRun::Done(_) | StageRun::Abandoned)
    }
}

pub type DocsState = StageRun<DocumentationSubmit>;
pub type ArchitectureState = StageRun<ArchitectureSubmit>;

/// Rule B5: the book write that ends the docs stage.
#[derive(Debug, Clone, PartialEq)]
pub struct BookWrite {
    pub book: String,
    pub projects: Vec<String>,
    pub error: Option<String>,
}

/// Rule B9: how a project's part is split among writers, from `DocsPlanned`.
#[derive(Debug, Clone, PartialEq)]
pub struct DocsPlan {
    /// Empty for one writer.
    pub areas: Vec<ostra_core::book::DocsArea>,
    pub existing: Option<Vec<String>>,
    pub touched: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectTrack {
    /// `Some(exit code)` once format ran. `Some(None)` means no format command.
    pub format: Option<Option<i32>>,
    /// The command running now, between `CommandStarted` and `CommandRan`.
    pub running: Option<(CommandPurpose, String)>,
    pub closing_gate: Option<GateId>,
    pub closing: Option<(bool, bool)>,
    /// The one writer of a part that is not split into areas.
    pub docs: DocsState,
    /// `None` until the docs stage is planned. A log from before Rule B9 has none and one writer.
    pub docs_plan: Option<DocsPlan>,
    /// Rule B9: the writer of each area this session rewrites, by area ID.
    pub area_docs: BTreeMap<String, DocsState>,
}

impl Default for ProjectTrack {
    fn default() -> Self {
        ProjectTrack {
            format: None,
            running: None,
            closing_gate: None,
            closing: None,
            docs: DocsState::NotStarted,
            docs_plan: None,
            area_docs: BTreeMap::new(),
        }
    }
}

impl ProjectTrack {
    /// Rule B9: the planned areas, when the part is split.
    pub fn docs_areas(&self) -> Option<&[ostra_core::book::DocsArea]> {
        self.docs_plan
            .as_ref()
            .map(|p| p.areas.as_slice())
            .filter(|a| !a.is_empty())
    }

    /// The writer state an execution for `area` folds into.
    pub fn docs_mut(&mut self, area: Option<&str>) -> &mut DocsState {
        match area {
            Some(a) => self
                .area_docs
                .entry(a.to_string())
                .or_insert(DocsState::NotStarted),
            None => &mut self.docs,
        }
    }

    /// The project's docs as one run: the writer of an unsplit part, or for a split part, running
    /// while any area runs, failed while one waits on its failure, and done once every rewritten
    /// area settled with at least one written or an area kept from the book, with the rewritten
    /// areas combined.
    pub fn docs_aggregate(&self) -> DocsState {
        let Some(areas) = self.docs_areas() else {
            return self.docs.clone();
        };
        let states: Vec<&DocsState> = self.area_docs.values().collect();
        if let Some(r) = states.iter().find(|s| matches!(s, DocsState::Running(_))) {
            return (*r).clone();
        }
        if let Some(f) = states
            .iter()
            .find(|s| matches!(s, DocsState::Failed { .. }))
        {
            return (*f).clone();
        }
        if states.iter().any(|s| matches!(s, DocsState::NotStarted)) {
            return DocsState::NotStarted;
        }
        let written: Vec<(&ostra_core::book::DocsArea, &DocumentationSubmit)> = areas
            .iter()
            .filter_map(|a| match self.area_docs.get(&a.id) {
                Some(DocsState::Done(d)) => Some((a, &**d)),
                _ => None,
            })
            .collect();
        let kept = areas.iter().any(|a| !self.area_docs.contains_key(&a.id));
        if written.is_empty() && !kept {
            return DocsState::Abandoned;
        }
        DocsState::Done(Box::new(ostra_core::book::combine_areas(&written)))
    }
}

#[derive(Debug, Clone)]
pub struct ExecRecord {
    pub id: ExecutionId,
    pub agent: AgentName,
    pub purpose: ExecPurpose,
    pub stage: StageKind,
    pub project: String,
    pub report_path: Option<PathBuf>,
    pub params: Value,
    pub result: Option<ExecutionResult>,
    pub resumes: Option<ExecutionId>,
    pub loop_key: Option<(u32, bool)>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub executor: ostra_core::ExecutorKind,
    pub model: String,
    pub spawn_block: String,
}

#[derive(Debug, Clone)]
pub struct GateRecord {
    pub id: GateId,
    pub title: String,
    pub explanation: String,
    pub payload: GatePayload,
    pub answer: Option<GateAnswer>,
    pub source: Option<AnswerSource>,
    pub reason: Option<String>,
    pub opened_at: DateTime<Utc>,
    pub answered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct DecisionRecord {
    pub id: DecisionId,
    pub judge: JudgeKind,
    pub subject: Option<String>,
    pub input_summary: String,
    pub output: Value,
    pub reason: String,
    pub overridden: bool,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct QuickTrack {
    pub exec: Option<ExecutionId>,
    pub running: bool,
    pub answer: Option<QuickAnswerSubmit>,
    pub failed: Option<String>,
}

/// State of the init flow (HANDOVER 8.4).
#[derive(Debug, Clone, Default)]
pub struct InitTrack {
    pub project: String,
    pub detect: Option<ExecutionId>,
    pub detect_result: Option<Value>,
    pub scouts: Vec<InitItem>,
    pub propose: Option<ExecutionId>,
    pub propose_result: Option<Value>,
    pub approval_gate: Option<GateId>,
    pub decisions: Option<Vec<(String, String)>>,
    pub generates: Vec<InitItem>,
    pub inventory: Option<ExecutionId>,
    pub inventory_result: Option<Value>,
    pub adopt: Option<ExecutionId>,
    pub adopt_result: Option<Value>,
    pub failed: Option<(ExecutionId, String)>,
    pub failed_gate: Option<GateId>,
    pub retries: BTreeMap<String, u32>,
    /// Rule O4: a created project's init ended. `note` says why it ended without initializing.
    pub finished: bool,
    pub note: Option<String>,
    /// Rule O5: advice rounds per step (`{mode}:{item}`), and every guidance given, oldest first.
    pub advice: BTreeMap<String, Vec<String>>,
    /// The advisor run looking at the current failure.
    pub advising: Option<ExecutionId>,
    /// The advisor asked for the user, or could not help, so the failure goes to the user's gate.
    pub escalated: bool,
}

impl InitTrack {
    /// The spawn's `User focus:` for a project an agent created, from its `ProjectCreate` call.
    pub fn created_focus(p: &ostra_core::manage::CreatedProject) -> String {
        let reqs: Vec<String> = p.requirements.iter().map(|r| format!("- {r}")).collect();
        format!(
            "A new project `{}`, created in this session by the {}. Its folder is empty, so initialize it from the stack and requirements here.\n\nStack: {}\nPurpose: {}\nBase requirements:\n{}",
            p.key,
            p.agent,
            p.stack,
            p.purpose,
            reqs.join("\n")
        )
    }
}

#[derive(Debug, Clone)]
pub struct InitItem {
    pub key: String,
    pub exec: Option<ExecutionId>,
    pub result: Option<Value>,
}

/// Context the user added after the start (Rules D2, C2).
#[derive(Debug, Clone)]
pub struct Amendment {
    pub text: String,
    pub files: Vec<ContextFile>,
    pub uploads: Vec<UploadedFile>,
    pub delivery: ContextDelivery,
    pub at: DateTime<Utc>,
    /// Rule C2: the Route answer judge decides what happens to it once it is released.
    pub routed: bool,
    /// Rule C2: queued behind running executions; nothing starts, and the user may withdraw it.
    pub held: bool,
    /// The user withdrew it while it was queued, so no agent or judge reads it.
    pub withdrawn: bool,
    /// Rule C2: the Route answer judge has not decided yet, so nothing starts.
    pub pending: bool,
    /// The context joins the request every later agent reads. Remembered or discarded context does not.
    pub delivered: bool,
}

/// Rule U2: the correction an execution resumes with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Steer {
    pub text: String,
    /// Sent to a paused run and not read yet, so the user may withdraw it. A correction sent to a
    /// running run stopped it and is final.
    pub queued: bool,
}

/// Why the engine interrupted a running execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    /// The session was paused; the execution resumes where it stopped (Rule P2).
    Pause,
    /// Context was sent now; the execution re-runs from its spawn block (Rule C2).
    Context,
    /// Rule O4: the run created its phase's project, which is initialized before the phase starts
    /// again inside it.
    ProjectCreated,
    /// Rule U1: the user skipped the execution's task; it ends without a result.
    Skipped,
    /// Rule U2: the user sent the execution a correction; it resumes in place with it.
    Steer,
}

impl Interrupt {
    pub fn message(self) -> &'static str {
        match self {
            Interrupt::Pause => "The session was paused.",
            Interrupt::Context => "Interrupted to deliver the context the user added.",
            Interrupt::ProjectCreated => {
                "Stopped after creating the project, which Ostra initializes before this phase starts again in it."
            }
            Interrupt::Skipped => {
                "You skipped this task, so Ostra stopped it and moved on without it."
            }
            Interrupt::Steer => "Interrupted to deliver the correction the user sent.",
        }
    }
}

/// Rule C3: each upload reaches the agents as the absolute path of its copy in the session.
fn upload_list(heading: &str, uploads: &[UploadedFile]) -> String {
    if uploads.is_empty() {
        return String::new();
    }
    let mut s = format!(
        "\n\n{heading}. Read each one before you start, because the user chose them as context:"
    );
    for u in uploads {
        s.push_str(&format!("\n- `{}`", u.path.display()));
    }
    s
}

/// The resume-map key of an execution purpose: one work loop, review loop, or stage.
pub fn purpose_key(p: &ExecPurpose) -> String {
    match p {
        ExecPurpose::Implement { phase, .. } | ExecPurpose::Verify { phase } => {
            format!("work:{phase}:false")
        }
        ExecPurpose::WriteTest { phase, .. } => format!("work:{phase}:true"),
        ExecPurpose::Review { phase, tests, .. } => format!("review:{phase}:{tests}"),
        ExecPurpose::Spec { .. } => "spec".into(),
        ExecPurpose::Plan { .. } => "plan".into(),
        ExecPurpose::FactCheck { target, .. } => format!("fact-check:{}", target.as_str()),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[derive(Debug, Clone)]
pub struct SessionState {
    pub id: SessionId,
    pub created: bool,
    pub kind: SessionKind,
    pub request: String,
    /// Files attached to the request at the start (Rule C1).
    pub files: Vec<ContextFile>,
    /// Files uploaded with the request at the start (Rule C3).
    pub uploads: Vec<UploadedFile>,
    pub amendments: Vec<Amendment>,
    pub options: SessionOptions,
    pub yolo: bool,
    pub projects: Vec<ProjectRef>,
    /// Rule O6: the projects the user pinned on the New task form.
    pub pinned: Vec<String>,
    pub workspace_root: PathBuf,
    pub session_root: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    pub classify: Option<DecisionId>,
    pub category: Option<Category>,
    pub scope: Vec<String>,
    pub opts_in: OptsIn,

    pub explore: Vec<ExploreTask>,
    pub sufficiency_rounds: u32,
    pub spec: ArtifactTrack<GenerateSpecSubmit>,
    pub stakes: Option<(DecisionId, Stakes)>,
    /// The track of an `IMPLEMENT` request: forced from the New task form or decided by the
    /// Track judge after research.
    pub track: Option<Track>,
    pub track_decision: Option<DecisionId>,
    pub feedback: FeedbackTrack,
    pub plan: ArtifactTrack<PlanSubmit>,
    pub phases: BTreeMap<u32, PhaseRun>,
    pub superseded_phases: Vec<PhaseRun>,
    pub project_tracks: BTreeMap<String, ProjectTrack>,
    /// Rule B6: the book the user picked on the New task form.
    pub docs_book: Option<String>,
    /// Rule B4: the architecture of a book of two or more projects.
    pub architecture: ArchitectureState,
    /// Rule B5: set once the engine wrote the session's documentation.
    pub book_written: Option<BookWrite>,
    pub quick: QuickTrack,
    pub init: Option<InitTrack>,
    /// Rule O3: projects agents created in this session, in creation order.
    pub created_projects: Vec<ostra_core::manage::CreatedProject>,
    /// Rule O4: the init of each created project, run when the build starts.
    pub project_inits: BTreeMap<String, InitTrack>,
    /// Rule O4: runs stopped because they created their phase's project, whose phase starts over.
    pub restart_fresh: BTreeSet<ExecutionId>,
    /// Agents re-routed to native after a harness failure.
    pub native_fallback: BTreeSet<AgentName>,
    pub prompt_gens: u32,
    /// Dollars the user added to the session budget.
    pub budget_raised: f64,
    pub budget_gate: Option<GateId>,
    /// Rule J1: spec and plan answers waiting for the Route answer judge.
    pub held_answers: BTreeMap<GateId, HeldAnswer>,
    /// Rule J1: answers the judge kept for later stages.
    pub user_notes: Vec<UserNote>,

    pub executions: BTreeMap<ExecutionId, ExecRecord>,
    pub gates: BTreeMap<GateId, GateRecord>,
    pub decisions: BTreeMap<DecisionId, DecisionRecord>,
    pub completion_decision: Option<DecisionId>,
    pub completed: Option<(PathBuf, String)>,
    pub failed: Option<String>,
    pub notes: Vec<String>,
    /// Rule P1: a paused session starts nothing.
    pub paused: bool,
    /// Rule P3: containment signals per execution.
    pub signals: BTreeMap<ExecutionId, Vec<ContainmentSignal>>,
    /// Rule P3: the execution whose signals paused the session, until it is continued.
    pub contained: Option<ExecutionId>,
    /// Executions the engine is interrupting, and why.
    pub interrupting: BTreeMap<ExecutionId, Interrupt>,
    /// Paused executions to resume, by [`purpose_key`] (Rule P2).
    pub resume_from: BTreeMap<String, ExecutionId>,
    /// Rule U2: corrections the user sent each execution, kept until it resumes with them.
    pub steers: BTreeMap<ExecutionId, Steer>,
    /// Rule H2: native runs that ended waiting for a message, by [`purpose_key`].
    pub waiting_keys: BTreeMap<String, ExecutionId>,
    /// Rule H8: questions between subagents, in the order they were asked.
    pub asks: BTreeMap<ostra_core::ids::MessageId, crate::coord::Ask>,
    /// Rule H1: each execution's subagent ID, for runs that continue another's conversation.
    pub subagents: BTreeMap<ExecutionId, ExecutionId>,
    pub last_seq: i64,
    /// The session's short label: from the Classify decision, or fixed for an init session.
    pub title: Option<String>,
}

/// A docs-stage run ends done only with a submit the engine can read, because the book is built
/// from it.
fn stage_run<T>(
    status: ExecutionStatus,
    submit: Option<T>,
    exec: &ExecutionId,
    error: String,
) -> StageRun<T> {
    match (status, submit) {
        (ExecutionStatus::Ok, Some(t)) => StageRun::Done(Box::new(t)),
        (ExecutionStatus::Interrupted, _) => StageRun::NotStarted,
        (ExecutionStatus::Ok, None) => StageRun::Failed {
            exec: exec.clone(),
            error: "The run ended without a readable submit call, so there is nothing to put in the book.".into(),
            gate: None,
            retries: 1,
        },
        _ => StageRun::Failed {
            exec: exec.clone(),
            error,
            gate: None,
            retries: 1,
        },
    }
}

fn parse<T: serde::de::DeserializeOwned>(v: &Option<Value>) -> Option<T> {
    v.as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

fn is_instruction_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".md")
        || p.ends_with(".mdx")
        || p.ends_with(".txt")
        || p.ends_with(".toml") && p.contains("agent")
}

/// Map an execution purpose to the work loop it belongs to.
fn purpose_loop(purpose: &ExecPurpose) -> Option<(u32, bool)> {
    match purpose {
        ExecPurpose::Implement { phase, .. } | ExecPurpose::Verify { phase } => {
            Some((*phase, false))
        }
        ExecPurpose::WriteTest { phase, .. } => Some((*phase, true)),
        ExecPurpose::Review { phase, tests, .. } => Some((*phase, *tests)),
        _ => None,
    }
}

pub fn stage_of(purpose: &ExecPurpose) -> StageKind {
    match purpose {
        ExecPurpose::Advise { .. } | ExecPurpose::Unblock { .. } => StageKind::Rescue,
        ExecPurpose::Consult { .. } => StageKind::Handoff,
        ExecPurpose::Explore { .. } => StageKind::Explore,
        ExecPurpose::Spec { .. } => StageKind::Spec,
        ExecPurpose::FactCheck {
            target: FactTarget::Spec,
            ..
        } => StageKind::FactCheckSpec,
        ExecPurpose::FactCheck {
            target: FactTarget::Plan,
            ..
        } => StageKind::FactCheckPlan,
        ExecPurpose::Plan { .. } => StageKind::Plan,
        ExecPurpose::Implement { .. } => StageKind::Implement,
        ExecPurpose::Review { tests: false, .. } => StageKind::Review,
        ExecPurpose::Review { tests: true, .. } => StageKind::TestReview,
        ExecPurpose::Epa { .. } => StageKind::Epa,
        ExecPurpose::WriteTest { .. } => StageKind::WriteTest,
        ExecPurpose::Docs { .. } => StageKind::Documentation,
        ExecPurpose::Architecture => StageKind::Architecture,
        ExecPurpose::PromptGen {
            handoff_for: Some(_),
        } => StageKind::Handoff,
        ExecPurpose::PromptGen { handoff_for: None } => StageKind::PromptGen,
        ExecPurpose::Verify { .. } => StageKind::Verify,
        ExecPurpose::QuickAnswer | ExecPurpose::Inspect { .. } => StageKind::QuickAnswer,
        ExecPurpose::Init { mode, .. } => match mode {
            ostra_core::InitializerMode::Detect | ostra_core::InitializerMode::Adopt => {
                StageKind::Detect
            }
            ostra_core::InitializerMode::Scout => StageKind::Scout,
            ostra_core::InitializerMode::Propose => StageKind::Propose,
            ostra_core::InitializerMode::GenerateSkill => StageKind::GenerateSkill,
            ostra_core::InitializerMode::GenerateInventory => StageKind::GenerateInventory,
        },
    }
}

impl SessionState {
    pub fn new(id: SessionId) -> Self {
        let epoch = DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default();
        SessionState {
            id,
            created: false,
            kind: SessionKind::Pipeline,
            request: String::new(),
            files: vec![],
            uploads: vec![],
            amendments: vec![],
            options: SessionOptions::default(),
            yolo: false,
            projects: vec![],
            pinned: vec![],
            workspace_root: PathBuf::new(),
            session_root: PathBuf::new(),
            created_at: epoch,
            updated_at: epoch,
            classify: None,
            category: None,
            scope: vec![],
            opts_in: OptsIn::default(),
            explore: vec![],
            sufficiency_rounds: 0,
            spec: ArtifactTrack::new(),
            stakes: None,
            track: None,
            track_decision: None,
            feedback: FeedbackTrack::default(),
            plan: ArtifactTrack::new(),
            phases: BTreeMap::new(),
            superseded_phases: vec![],
            project_tracks: BTreeMap::new(),
            docs_book: None,
            architecture: ArchitectureState::NotStarted,
            book_written: None,
            quick: QuickTrack::default(),
            init: None,
            created_projects: vec![],
            project_inits: BTreeMap::new(),
            restart_fresh: BTreeSet::new(),
            native_fallback: BTreeSet::new(),
            prompt_gens: 0,
            budget_raised: 0.0,
            budget_gate: None,
            held_answers: BTreeMap::new(),
            user_notes: vec![],
            executions: BTreeMap::new(),
            gates: BTreeMap::new(),
            decisions: BTreeMap::new(),
            completion_decision: None,
            completed: None,
            failed: None,
            notes: vec![],
            paused: false,
            signals: BTreeMap::new(),
            contained: None,
            interrupting: BTreeMap::new(),
            resume_from: BTreeMap::new(),
            steers: BTreeMap::new(),
            waiting_keys: BTreeMap::new(),
            asks: BTreeMap::new(),
            subagents: BTreeMap::new(),
            last_seq: 0,
            title: None,
        }
    }

    pub fn fold(id: SessionId, events: &[StoredEvent]) -> Self {
        let mut s = SessionState::new(id);
        for e in events {
            s.apply(e);
        }
        s
    }

    pub fn is_terminal(&self) -> bool {
        self.completed.is_some() || self.failed.is_some()
    }

    /// The request as it now stands, with every amendment (Rule D2), attached file (Rule C1), and
    /// upload (Rule C3).
    pub fn full_request(&self) -> String {
        let mut s = self.request.clone();
        s.push_str(&self.file_list(
            "Files and folders the user attached to the request",
            &self.files,
        ));
        s.push_str(&upload_list(
            "Files the user uploaded with the request",
            &self.uploads,
        ));
        for a in self.amendments.iter().filter(|a| a.delivered) {
            s.push_str("\n\nAdded later by the user: ");
            s.push_str(&self.added_part(&a.text, &a.files, &a.uploads));
        }
        s
    }

    /// Rule C2: the context the user added, for a spawn whose task was written without it.
    pub fn added_context(&self) -> String {
        let added: Vec<String> = self
            .amendments
            .iter()
            .filter(|a| a.delivered)
            .map(|a| format!("- {}", self.added_part(&a.text, &a.files, &a.uploads)))
            .collect();
        if added.is_empty() {
            return String::new();
        }
        format!(
            "\n\nThe user added this context to the request while the session ran. Take it into account:\n{}",
            added.join("\n")
        )
    }

    /// Rule C1: each attached file reaches the agents as an absolute path beside its tag.
    fn file_list(&self, heading: &str, files: &[ContextFile]) -> String {
        if files.is_empty() {
            return String::new();
        }
        let mut s = format!(
            "\n\n{heading}. Read each file and look through each folder before you start, because the user chose them as context:"
        );
        for f in files {
            let root = if f.project == ostra_core::artifacts::TAG_ROOT {
                Some(ostra_core::artifacts::dir(&self.workspace_root))
            } else {
                self.project_path(&f.project)
            };
            // Forward slashes so the path reads consistently in the prompt on every OS, and never
            // mixes separators when a forward-slash relative path is joined to a Windows root.
            let abs = root
                .map(|p| p.join(&f.path).display().to_string().replace('\\', "/"))
                .unwrap_or_else(|| f.path.clone());
            let kind = if f.is_folder() { "folder, " } else { "" };
            s.push_str(&format!("\n- `{abs}` ({kind}{})", f.tag()));
        }
        s
    }

    pub(crate) fn added_part(
        &self,
        text: &str,
        files: &[ContextFile],
        uploads: &[UploadedFile],
    ) -> String {
        format!(
            "{text}{}{}",
            self.file_list("Files and folders attached with this addition", files),
            upload_list("Files uploaded with this addition", uploads)
        )
    }

    pub fn project_path(&self, key: &str) -> Option<PathBuf> {
        self.projects
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.path.clone())
    }

    /// The primary project: the first in scope. Cross-project stages carry its key.
    pub fn primary(&self) -> String {
        self.scope
            .first()
            .cloned()
            .or_else(|| self.projects.first().map(|p| p.key.clone()))
            .unwrap_or_default()
    }

    pub fn project_session_dir(&self, key: &str) -> PathBuf {
        self.session_root.join(key)
    }

    /// Research documents, oldest run first, including superseded ones (Rule D2).
    pub fn research_docs(&self) -> Vec<PathBuf> {
        self.explore
            .iter()
            .filter_map(|t| t.result.as_ref().map(|r| PathBuf::from(&r.research_path)))
            .collect()
    }

    pub fn open_gates(&self) -> impl Iterator<Item = &GateRecord> {
        self.gates.values().filter(|g| g.answer.is_none())
    }

    pub fn running_executions(&self) -> impl Iterator<Item = &ExecRecord> {
        self.executions.values().filter(|e| e.result.is_none())
    }

    /// What finished executions spent. Running executions are counted when they finish.
    pub fn spent_usd(&self) -> f64 {
        self.executions
            .values()
            .filter_map(|e| e.result.as_ref())
            .map(|r| r.usage.cost_usd)
            .sum()
    }

    pub fn tests_requested(&self) -> bool {
        self.options.tests || self.opts_in.tests
    }

    pub fn docs_requested(&self) -> bool {
        self.options.docs || self.opts_in.docs
    }

    /// Whether a project's closing stages include docs, recorded or implied by an explicit
    /// request for both tests and docs (Rule T3).
    pub fn project_docs_on(&self, key: &str) -> bool {
        self.project_tracks
            .get(key)
            .and_then(|t| t.closing)
            .map(|c| c.1)
            .unwrap_or(self.tests_requested() && self.docs_requested())
    }

    /// Projects whose part of the book this session writes, sorted.
    pub fn docs_projects(&self) -> Vec<String> {
        self.project_tracks
            .keys()
            .filter(|k| {
                self.project_docs_on(k)
                    && self
                        .phases
                        .values()
                        .any(|p| &p.info.project == *k && p.impl_loop.is_done())
            })
            .cloned()
            .collect()
    }

    /// Rule B6: what this session adds to its book, from the docs-stage submits in the log.
    pub fn book_update(&self) -> ostra_core::book::BookUpdate {
        let parts = self
            .project_tracks
            .iter()
            .filter_map(|(k, t)| match t.docs_aggregate() {
                DocsState::Done(d) => Some((k.clone(), *d)),
                _ => None,
            })
            .collect();
        // Rule B9: every planned area, with the submit this session wrote or `None` to keep it.
        let areas = self
            .project_tracks
            .iter()
            .filter(|(_, t)| matches!(t.docs_aggregate(), DocsState::Done(_)))
            .filter_map(|(k, t)| {
                let planned = t.docs_areas()?;
                Some((
                    k.clone(),
                    planned
                        .iter()
                        .map(|a| {
                            let sub = match t.area_docs.get(&a.id) {
                                Some(DocsState::Done(d)) => Some((**d).clone()),
                                _ => None,
                            };
                            (a.clone(), sub)
                        })
                        .collect(),
                ))
            })
            .collect();
        let (architecture, glossary) = match &self.architecture {
            ArchitectureState::Done(a) => (a.architecture.clone(), a.glossary.clone()),
            _ => (None, vec![]),
        };
        ostra_core::book::BookUpdate {
            session: self.id.to_string(),
            parts,
            areas,
            architecture,
            glossary,
        }
    }

    /// Rule B6: the picked book, or the one named after the documented projects.
    pub fn book_id(&self) -> String {
        self.docs_book
            .clone()
            .filter(|b| ostra_core::book::is_book_id(b))
            .unwrap_or_else(|| ostra_core::book::book_id(&self.docs_projects()))
    }

    pub fn ledger_path(&self, project: &str, phase: u32, tests: bool) -> PathBuf {
        let value = if tests {
            format!("{phase}-tests")
        } else {
            phase.to_string()
        };
        self.project_session_dir(project)
            .join(paths::report::review_ledger(&value))
    }

    fn loop_mut(&mut self, key: (u32, bool)) -> Option<&mut WorkLoop> {
        self.phases.get_mut(&key.0).map(|p| {
            if key.1 {
                &mut p.test_loop
            } else {
                &mut p.impl_loop
            }
        })
    }

    pub fn loop_ref(&self, key: (u32, bool)) -> Option<&WorkLoop> {
        self.phases
            .get(&key.0)
            .map(|p| if key.1 { &p.test_loop } else { &p.impl_loop })
    }

    fn auto_fixable(
        &self,
        _project: &str,
        finding: &ReviewFinding,
        autofix_ids: &BTreeSet<String>,
    ) -> bool {
        finding.severity != Severity::Blocker
            && !finding.rule.starts_with("SEC-BLOCK")
            && !finding.rule.starts_with("PHASE-REQ")
            && autofix_ids.contains(&finding.rule)
    }

    // -----------------------------------------------------------------------------------------
    // Fold
    // -----------------------------------------------------------------------------------------

    pub fn apply(&mut self, stored: &StoredEvent) {
        self.last_seq = stored.seq;
        self.updated_at = stored.at;
        let at = stored.at;
        match &stored.event {
            SessionEvent::SessionCreated {
                kind,
                request,
                options,
                projects,
                workspace_root,
                session_root,
                files,
                uploads,
                pinned,
                docs_book,
            } => {
                self.created = true;
                self.pinned = pinned.clone();
                self.docs_book = docs_book.clone();
                self.files = files.clone();
                self.uploads = uploads.clone();
                self.created_at = at;
                self.kind = kind.clone();
                self.request = request.clone();
                self.options = *options;
                self.yolo = options.yolo;
                self.track = options.track;
                self.projects = projects.clone();
                self.workspace_root = workspace_root.clone();
                self.session_root = session_root.clone();
                if let SessionKind::Init { project } = kind {
                    self.scope = vec![project.clone()];
                    self.title = Some(format!("Initialize {project}"));
                    self.init = Some(InitTrack {
                        project: project.clone(),
                        ..Default::default()
                    });
                }
            }
            SessionEvent::ProjectCreated { project } => {
                // Rule O3: the project joins the session's projects and scope uninitialized.
                if !self.valid_project(&project.key) {
                    self.projects.push(ProjectRef {
                        key: project.key.clone(),
                        path: project.path.clone(),
                    });
                }
                if !self.scope.contains(&project.key) {
                    self.scope.push(project.key.clone());
                }
                self.project_inits.insert(
                    project.key.clone(),
                    InitTrack {
                        project: project.key.clone(),
                        ..Default::default()
                    },
                );
                self.created_projects.push(project.clone());
                // Rule O4: the run that created it stops, and its phase starts again in the new
                // project once the init ends.
                if self
                    .executions
                    .get(&project.execution)
                    .is_some_and(|r| r.result.is_none())
                {
                    self.interrupting
                        .insert(project.execution.clone(), Interrupt::ProjectCreated);
                }
            }
            SessionEvent::ProjectInitFinished { project } => {
                if let Some(i) = self.project_inits.get_mut(project) {
                    i.finished = true;
                    i.note = None;
                }
            }
            SessionEvent::InitStepFailed {
                project,
                execution,
                error,
            } => {
                if let Some(i) = self.project_inits.get_mut(project) {
                    clear_init_result(i, execution);
                    i.failed = Some((execution.clone(), error.clone()));
                }
            }
            SessionEvent::RequestAmended {
                text,
                files,
                uploads,
                delivery,
                routed,
            } => {
                let now = *delivery == ContextDelivery::Now;
                if now {
                    // Rule C2: a fresh re-run replaces a correction's resume, which would not see the context.
                    self.interrupting.retain(|_, why| *why != Interrupt::Steer);
                    self.interrupt_running(Interrupt::Context);
                    self.steers.clear();
                }
                // Rule C2: queued context waits for the running executions to finish.
                let held = !now && self.running_executions().next().is_some();
                self.amendments.push(Amendment {
                    text: text.clone(),
                    files: files.clone(),
                    uploads: uploads.clone(),
                    delivery: *delivery,
                    at,
                    routed: *routed,
                    held,
                    withdrawn: false,
                    pending: false,
                    delivered: false,
                });
                if !held {
                    self.release_amendment(self.amendments.len() - 1);
                }
            }
            SessionEvent::AmendmentWithdrawn { index } => {
                if let Some(a) = self.amendments.get_mut(*index as usize).filter(|a| a.held) {
                    a.held = false;
                    a.withdrawn = true;
                }
            }
            SessionEvent::SessionPaused => {
                self.paused = true;
                self.interrupt_running(Interrupt::Pause);
            }
            SessionEvent::SessionResumed => {
                self.paused = false;
                self.contained = None;
            }
            SessionEvent::ContainmentSignal { execution, signal } => {
                let seen = self.signals.entry(execution.clone()).or_default();
                seen.push(signal.clone());
                // Rule P3: the third signal of one execution pauses the session as Rule P1 does.
                if seen.len() == PAUSE_AFTER && !self.paused && !self.is_terminal() {
                    self.paused = true;
                    self.contained = Some(execution.clone());
                    self.interrupt_running(Interrupt::Pause);
                }
            }
            SessionEvent::YoloSet { enabled } => self.yolo = *enabled,
            SessionEvent::DecisionMade {
                id,
                judge,
                subject,
                input_summary,
                output,
                reason,
            } => {
                self.decisions.insert(
                    id.clone(),
                    DecisionRecord {
                        id: id.clone(),
                        judge: *judge,
                        subject: subject.clone(),
                        input_summary: input_summary.clone(),
                        output: output.clone(),
                        reason: reason.clone(),
                        overridden: false,
                        at,
                    },
                );
                self.on_decision(id, *judge, subject.as_deref(), output, false);
            }
            SessionEvent::DecisionOverridden { id, output, reason } => {
                let Some(d) = self.decisions.get_mut(id) else {
                    return;
                };
                d.output = output.clone();
                d.reason = reason.clone();
                d.overridden = true;
                let (judge, subject) = (d.judge, d.subject.clone());
                self.on_decision(id, judge, subject.as_deref(), output, true);
            }
            SessionEvent::ExecutionStarted {
                id,
                agent,
                purpose,
                stage,
                project,
                executor,
                model,
                params,
                spawn_block,
                report_path,
                resumes,
            } => {
                let loop_key = match purpose {
                    ExecPurpose::PromptGen {
                        handoff_for: Some(x),
                    } => self.executions.get(x).and_then(|r| r.loop_key),
                    ExecPurpose::PromptGen { handoff_for: None } => Some((1, false)),
                    other => purpose_loop(other),
                };
                self.executions.insert(
                    id.clone(),
                    ExecRecord {
                        id: id.clone(),
                        agent: *agent,
                        purpose: purpose.clone(),
                        stage: *stage,
                        project: project.clone(),
                        report_path: report_path.clone(),
                        params: params.clone(),
                        result: None,
                        resumes: resumes.clone(),
                        loop_key,
                        started_at: at,
                        ended_at: None,
                        executor: *executor,
                        model: model.clone(),
                        spawn_block: spawn_block.clone(),
                    },
                );
                self.resume_from.remove(&purpose_key(purpose));
                if let Some(from) = resumes {
                    let root = self.subagent_of(from);
                    self.subagents.insert(id.clone(), root);
                }
                if self.paused {
                    // Started in the window before the pause reached the runner.
                    self.interrupting.insert(id.clone(), Interrupt::Pause);
                }
                self.on_started(id, purpose, loop_key, false);
                self.coord_started(id, purpose);
            }
            SessionEvent::AgentAsked { .. }
            | SessionEvent::AgentReplied { .. }
            | SessionEvent::MessageDelivered { .. } => self.on_coord_event(&stored.event, at),
            SessionEvent::ExecutionResumed { id } => {
                let Some(rec) = self.executions.get_mut(id) else {
                    return;
                };
                rec.result = None;
                rec.ended_at = None;
                let (purpose, loop_key) = (rec.purpose.clone(), rec.loop_key);
                self.resume_from.remove(&purpose_key(&purpose));
                self.steers.remove(id);
                self.waiting_keys.retain(|_, x| x != id);
                // Rule P3: continuing is the user's "this was fine", so the count starts again.
                self.signals.remove(id);
                if self.paused {
                    self.interrupting.insert(id.clone(), Interrupt::Pause);
                }
                self.on_started(id, &purpose, loop_key, true);
            }
            SessionEvent::ExecutionSteered { id, text } => {
                if !self.can_steer(id) {
                    return;
                }
                let running = self.executions.get(id).is_some_and(|r| r.result.is_none());
                let steer = self.steers.entry(id.clone()).or_insert(Steer {
                    text: String::new(),
                    queued: true,
                });
                if !steer.text.is_empty() {
                    steer.text.push_str("\n\n");
                }
                steer.text.push_str(text);
                steer.queued &= !running;
                if running {
                    self.interrupting
                        .entry(id.clone())
                        .or_insert(Interrupt::Steer);
                }
            }
            SessionEvent::SteerWithdrawn { id } => {
                if self.steer_queued(id) {
                    self.steers.remove(id);
                }
            }
            SessionEvent::ExecutionSkipped { id } => {
                if self.can_skip(id) {
                    self.interrupting.insert(id.clone(), Interrupt::Skipped);
                    self.skip_task(id);
                }
            }
            SessionEvent::ExecutionFinished { id, result } => {
                let Some(rec) = self.executions.get_mut(id) else {
                    return;
                };
                rec.result = Some(result.clone());
                rec.ended_at = Some(at);
                let rec = rec.clone();
                let why = self.interrupting.remove(id);
                let resumes = matches!(why, Some(Interrupt::Pause | Interrupt::Steer))
                    && result.status == ExecutionStatus::Interrupted;
                if resumes {
                    self.resume_from
                        .insert(purpose_key(&rec.purpose), id.clone());
                } else {
                    // Rule U2: a run that ended before the correction stopped it never reads it.
                    self.steers.remove(id);
                }
                if why == Some(Interrupt::ProjectCreated) {
                    self.restart_fresh.insert(id.clone());
                }
                if result.status == ExecutionStatus::Waiting {
                    // Rule H2: the stage sees a paused run, and the planner holds its spawn until
                    // a message wakes this run in place.
                    self.waiting_keys
                        .insert(SessionState::waiting_key(&rec), id.clone());
                    let paused = ExecutionResult {
                        status: ExecutionStatus::Interrupted,
                        ..result.clone()
                    };
                    self.on_finished(&rec, &paused);
                } else {
                    self.on_finished(&rec, result);
                }
                // Rule U1: the stopped run's finish would put its task back to re-run.
                if why == Some(Interrupt::Skipped) {
                    self.skip_task(id);
                }
                self.coord_finished(&rec, result);
                if self.running_executions().next().is_none() {
                    let held: Vec<usize> = self.held_amendments().collect();
                    for i in held {
                        self.release_amendment(i);
                    }
                }
            }
            SessionEvent::GateOpened {
                id,
                title,
                explanation,
                payload,
            } => {
                self.gates.insert(
                    id.clone(),
                    GateRecord {
                        id: id.clone(),
                        title: title.clone(),
                        explanation: explanation.clone(),
                        payload: payload.clone(),
                        answer: None,
                        source: None,
                        reason: None,
                        opened_at: at,
                        answered_at: None,
                    },
                );
                self.on_gate_opened(id, payload);
            }
            SessionEvent::GateAnswered {
                id,
                source,
                answer,
                reason,
                routed,
            } => {
                let Some(g) = self.gates.get_mut(id) else {
                    return;
                };
                if g.answer.is_some() {
                    return;
                }
                g.answer = Some(answer.clone());
                g.source = Some(*source);
                g.reason = reason.clone();
                g.answered_at = Some(at);
                let payload = g.payload.clone();
                self.on_gate_answered(id, &payload, answer, *routed);
            }
            SessionEvent::CommandStarted {
                purpose,
                project,
                command,
            } => {
                self.project_tracks
                    .entry(project.clone())
                    .or_default()
                    .running = Some((*purpose, command.clone()));
            }
            SessionEvent::DocsPlanned {
                project,
                areas,
                existing,
                touched,
            } => {
                // Rule B9: a DOCS request, a part the book does not record by these areas, or a
                // first split rewrites every area; after a build, only the areas the work touched.
                let every = self.category == Some(Category::Docs)
                    || existing
                        .as_ref()
                        .is_none_or(|e| !areas.iter().all(|a| e.contains(&a.id)));
                let t = self.project_tracks.entry(project.clone()).or_default();
                t.area_docs = areas
                    .iter()
                    .filter(|a| every || touched.contains(&a.id))
                    .map(|a| (a.id.clone(), DocsState::NotStarted))
                    .collect();
                t.docs_plan = Some(DocsPlan {
                    areas: areas.clone(),
                    existing: existing.clone(),
                    touched: touched.clone(),
                });
            }
            SessionEvent::BookWritten {
                book,
                projects,
                error,
            } => {
                self.book_written = Some(BookWrite {
                    book: book.clone(),
                    projects: projects.clone(),
                    error: error.clone(),
                });
            }
            SessionEvent::CommandRan {
                purpose,
                project,
                exit_code,
                ..
            } => {
                if let Some(t) = self.project_tracks.get_mut(project) {
                    t.running = None;
                }
                match purpose {
                    CommandPurpose::Format => {
                        self.project_tracks
                            .entry(project.clone())
                            .or_default()
                            .format = Some(*exit_code);
                    }
                    CommandPurpose::Stage => {
                        let key = self
                            .phases
                            .iter()
                            .flat_map(|(id, p)| {
                                [((*id, false), &p.impl_loop), ((*id, true), &p.test_loop)]
                            })
                            .find(|(k, l)| {
                                self.phases
                                    .get(&k.0)
                                    .is_some_and(|p| &p.info.project == project)
                                    && l.next == LoopNext::Stage
                            })
                            .map(|(k, _)| k);
                        if let Some(key) = key
                            && let Some(l) = self.loop_mut(key)
                        {
                            l.next = LoopNext::Done;
                            l.staged = *exit_code == Some(0);
                        }
                    }
                    CommandPurpose::Autofix => {}
                }
            }
            SessionEvent::AutofixApplied {
                phase,
                tests,
                failed,
                ..
            } => {
                let yolo = self.yolo;
                let project = self
                    .phases
                    .get(phase)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let ledger = self
                    .ledger_path(&project, *phase, *tests)
                    .display()
                    .to_string();
                if let Some(l) = self.loop_mut((*phase, *tests))
                    && let LoopNext::Autofix { apply, remaining } = l.next.clone()
                {
                    let mut rem = remaining;
                    for (rule_line, _) in failed {
                        if let Some(f) = apply.iter().find(|f| &f.line() == rule_line)
                            && matches!(f.severity, Severity::High | Severity::Medium)
                        {
                            rem.push(f.clone());
                        }
                    }
                    l.after_findings(rem, yolo, &ledger);
                }
            }
            SessionEvent::SecurityBlock { .. } => {}
            SessionEvent::PhaseBlocked {
                phase,
                tests,
                reason,
                ..
            } => {
                if let Some(l) = self.loop_mut((*phase, *tests)) {
                    l.announced_block = true;
                    if !l.is_blocked() {
                        l.next = LoopNext::Blocked {
                            reason: reason.clone(),
                        };
                    }
                }
            }
            SessionEvent::Note { message } => self.notes.push(message.clone()),
            SessionEvent::SessionCompleted {
                report_path,
                summary,
            } => {
                self.completed = Some((report_path.clone(), summary.clone()));
            }
            SessionEvent::SessionFailed { error } => self.failed = Some(error.clone()),
        }
    }

    pub(crate) fn push_explore(
        &mut self,
        project: String,
        task: String,
        origin: ExploreOrigin,
    ) -> u32 {
        let idx = self.explore.len() as u32;
        self.explore.push(ExploreTask {
            idx,
            project,
            task,
            origin,
            exec: None,
            running: false,
            result: None,
            failed: None,
            abandoned: false,
            retries: 0,
            judged: false,
            gate: None,
        });
        idx
    }

    /// Why `execution` cannot add project `key` to this session now (Rule O2).
    pub fn project_creation_refusal(&self, execution: &ExecutionId, key: &str) -> Option<String> {
        if !matches!(self.kind, SessionKind::Pipeline) {
            return Some("Create projects only from a pipeline session.".into());
        }
        if self.is_terminal() {
            return Some("This session has ended, so it takes no new project.".into());
        }
        if !self
            .executions
            .get(execution)
            .is_some_and(|r| r.result.is_none())
        {
            return Some("The run that asked for this project has ended.".into());
        }
        if self.valid_project(key) {
            return Some(format!(
                "Work in `{key}` as it is: that project is already in this session."
            ));
        }
        if self.project_to_create(key).is_none() {
            return Some(format!(
                "Create only a project the approved plan names as new: `{key}` is not one."
            ));
        }
        let phase_project = self
            .executions
            .get(execution)
            .and_then(|r| r.loop_key)
            .and_then(|(phase, _)| self.phases.get(&phase))
            .map(|p| p.info.project.as_str());
        if phase_project != Some(key) {
            return Some(format!(
                "Create `{key}` only from a phase the plan puts in it."
            ));
        }
        None
    }

    /// Rule O2: the planned folder of `key` when the approved plan names it as a new project and
    /// it does not exist in this session yet.
    pub fn project_to_create(&self, key: &str) -> Option<PathBuf> {
        let plan = self.plan.current.as_ref()?;
        (plan.new_projects.iter().any(|k| k == key)
            && ostra_core::slug::is_project_key(key)
            && !self.valid_project(key))
        .then(|| self.workspace_root.join(key))
    }

    /// Rule O4: a created project whose init has not ended, so its phases wait.
    pub fn awaiting_init(&self, project: &str) -> bool {
        self.project_inits.get(project).is_some_and(|i| !i.finished)
    }

    fn valid_project(&self, key: &str) -> bool {
        self.projects.iter().any(|p| p.key == key)
    }

    /// Rule D10: a requirement change after the spec exists restarts at the spec.
    fn requirement_change(&mut self, text: String) {
        self.spec.changes.push(text);
        self.spec.needs_run = true;
        self.spec.revoke_approval();
        if self.plan.current.is_some() || self.plan.running.is_some() || !self.plan.runs.is_empty()
        {
            self.plan.revoke_approval();
            self.plan.invalidated = true;
        }
    }

    fn track_mut(&mut self, target: FactTarget) -> &mut dyn RoutingTrack {
        match target {
            FactTarget::Spec => &mut self.spec,
            FactTarget::Plan => &mut self.plan,
        }
    }

    fn approve_spec(&mut self) {
        self.spec.approved = true;
        self.spec.approved_version = self.spec.version;
        if self.plan.invalidated {
            self.plan.needs_run = true;
        }
        for i in 0..self.feedback.rounds.len() {
            if self.feedback.rounds[i].awaiting_spec {
                self.feedback.rounds[i].awaiting_spec = false;
                self.add_revision_phases(i);
            }
        }
    }

    fn hold_approval(&mut self, id: &GateId, target: FactTarget, approve: bool, text: String) {
        let version = match target {
            FactTarget::Spec => self.spec.version,
            FactTarget::Plan => self.plan.version,
        };
        self.track_mut(target).set_routing(Some(id.clone()));
        self.held_answers.insert(
            id.clone(),
            HeldAnswer::Approval {
                target,
                approve,
                version,
                text,
            },
        );
    }

    /// The implementation review gate a feedback round was answered at, when it was routed.
    fn feedback_gate(&self, round: usize) -> Option<GateId> {
        self.gates
            .values()
            .filter(|g| g.answer.is_some())
            .find(|g| matches!(&g.payload, GatePayload::ImplementationReview { round: r, .. } if *r as usize == round + 1))
            .map(|g| g.id.clone())
    }

    /// Rule J1: keep every part of an answer the judge named later stages for.
    fn remember_parts(
        &mut self,
        gate: Option<&GateId>,
        items: &[AnswerItem],
        id: &str,
        answer: &str,
    ) {
        let parts: Vec<AnswerItem> = parts_for(items, id).into_iter().cloned().collect();
        for p in &parts {
            self.remember(gate, p, answer);
        }
    }

    /// Rule J1: keep the part of an answer the judge named later stages for.
    fn remember(&mut self, gate: Option<&GateId>, item: &AnswerItem, answer: &str) {
        if item.stages.is_empty() || item.disposition == Disposition::Discard {
            return;
        }
        let text = if item.note.trim().is_empty() {
            answer.to_string()
        } else {
            item.note.clone()
        };
        let mut stages = item.stages.clone();
        stages.sort();
        stages.dedup();
        self.user_notes.push(UserNote {
            id: format!("N{}", self.user_notes.len() + 1),
            forgotten: false,
            stages,
            text,
            gate: gate.cloned(),
        });
    }

    /// Rule J1: the user took back or replaced notes kept earlier.
    fn forget(&mut self, ids: &[String]) {
        for n in &mut self.user_notes {
            if ids.iter().any(|i| i.trim() == n.id) {
                n.forgotten = true;
            }
        }
    }

    /// Rule J1: queue the research the judge asked for, capped, in projects the session has.
    fn queue_research(&mut self, tasks: &[ExploreTaskSpec], origin: ExploreOrigin) -> Vec<u32> {
        let why = if origin == ExploreOrigin::Amendment {
            "The user asked for this research when they added context to the request"
        } else {
            "The user asked for this research while answering a question"
        };
        let primary = self.primary();
        tasks
            .iter()
            .filter(|t| !t.task.trim().is_empty())
            .take(MAX_ANSWER_RESEARCH)
            .map(|t| {
                let project = if self.valid_project(&t.project) {
                    t.project.clone()
                } else {
                    primary.clone()
                };
                let task = format!(
                    "{}\n\n{why}. The whole request, for context: {}",
                    t.task, self.request
                );
                self.push_explore(project, task, origin.clone())
            })
            .collect()
    }

    /// Rule C2: apply the Route answer judge's decision on context the user added mid-session.
    fn apply_amendment_route(&mut self, i: usize, out: &RouteAnswerOut) {
        let Some(a) = self.amendments.get(i).filter(|a| a.pending) else {
            return;
        };
        let text = self.added_part(&a.text, &a.files, &a.uploads);
        let item = out.item(ANSWER_ITEM);
        let deliver = item.disposition == Disposition::Deliver;
        self.amendments[i].pending = false;
        self.amendments[i].delivered = deliver;
        self.forget(&out.forget);
        self.skip_research(&out.skip);
        self.remember_parts(None, &out.items, ANSWER_ITEM, &text);
        self.queue_research(&out.research, ExploreOrigin::Amendment);
        // Rule D10: a requirement change after the spec exists restarts at the spec.
        if deliver && out.route == AnswerRoute::RequirementChange && !self.spec.runs.is_empty() {
            self.requirement_change(format!("The user extended the request: {text}"));
        }
    }

    /// Rule C2: a classified session with research or later stages routes added context through
    /// the judge. Before classification the Classify judge reads it with the request.
    pub fn routes_amendments(&self) -> bool {
        self.classify.is_some()
            && matches!(
                self.category,
                Some(Category::Research | Category::Spec | Category::Plan | Category::Implement)
            )
    }

    /// Rule C2: queued context joins the session, to be routed or delivered. A resumed conversation
    /// would not see it, so each paused run re-runs instead (Rule P2), except a run that holds a
    /// correction, which resumes with it (Rule U2).
    fn release_amendment(&mut self, i: usize) {
        let a = &mut self.amendments[i];
        a.held = false;
        a.pending = a.routed;
        a.delivered = !a.routed;
        let routed = a.routed;
        let a = &self.amendments[i];
        let part = self.added_part(&a.text, &a.files, &a.uploads);
        let steers = &self.steers;
        self.resume_from.retain(|_, id| steers.contains_key(id));
        if !routed {
            self.on_amended(&part);
        }
    }

    /// Rule C2: queued context the user may still withdraw, by index.
    pub fn held_amendments(&self) -> impl Iterator<Item = usize> + '_ {
        self.amendments
            .iter()
            .enumerate()
            .filter(|(_, a)| a.held)
            .map(|(i, _)| i)
    }

    /// Rule C2: context the user added that waits for the judge, by index.
    pub fn pending_amendments(&self) -> impl Iterator<Item = usize> + '_ {
        self.amendments
            .iter()
            .enumerate()
            .filter(|(_, a)| a.pending)
            .map(|(i, _)| i)
    }

    /// Rule U1: a running execution whose task the session can do without. Research the user or
    /// a judge started, the test analysis, a docs writer, and the architecture overview end
    /// without a result; work, review, spec, plan, and fact-check carry rules a skip would break,
    /// a helper's asker waits for its answer, and a rescue's loop waits for its fact.
    pub fn can_skip(&self, exec: &ExecutionId) -> bool {
        let Some(rec) = self.executions.get(exec) else {
            return false;
        };
        if rec.result.is_some() || rec.loop_key.is_some() || self.is_terminal() {
            return false;
        }
        match &rec.purpose {
            ExecPurpose::Explore { task } => self.explore.get(*task as usize).is_some_and(|t| {
                !t.finished()
                    && !matches!(
                        t.origin,
                        ExploreOrigin::Ask { .. } | ExploreOrigin::Rescue { .. }
                    )
            }),
            ExecPurpose::Epa { .. } | ExecPurpose::Docs { .. } | ExecPurpose::Architecture => true,
            _ => false,
        }
    }

    /// Rule U2: a running execution, or one a pause stopped, can take a correction. A run waiting
    /// on another subagent wakes only with that answer (Rule H2), so it takes none.
    pub fn can_steer(&self, exec: &ExecutionId) -> bool {
        let Some(rec) = self.executions.get(exec) else {
            return false;
        };
        if self.is_terminal()
            || self
                .interrupting
                .get(exec)
                .is_some_and(|w| *w != Interrupt::Steer)
        {
            return false;
        }
        match &rec.result {
            None => true,
            Some(_) => self.resume_from.values().any(|x| x == exec),
        }
    }

    /// Rule U2: a correction waits, withdrawable, while its run is paused. Sent to a running run, it
    /// stops the run at once and cannot be taken back.
    pub fn steer_queued(&self, exec: &ExecutionId) -> bool {
        self.steers.get(exec).is_some_and(|s| s.queued)
            && self.resume_from.values().any(|x| x == exec)
    }

    /// Rule U1: end the execution's task without a result, as abandoning its failure gate does.
    fn skip_task(&mut self, exec: &ExecutionId) {
        self.exec_gate_answered(exec, false);
    }

    /// Rule U1: research tasks the Route answer judge may skip, numbered from 1 as it sees them.
    pub fn skippable_research(&self) -> impl Iterator<Item = &ExploreTask> + '_ {
        self.explore.iter().filter(|t| {
            !t.finished()
                && t.failed.is_none()
                && !matches!(
                    t.origin,
                    ExploreOrigin::Ask { .. } | ExploreOrigin::Rescue { .. }
                )
        })
    }

    /// Rule U1: skip the research tasks the user told the judge to drop. A running one stops.
    fn skip_research(&mut self, numbers: &[u32]) {
        let idx: Vec<u32> = self
            .skippable_research()
            .filter(|t| numbers.contains(&(t.idx + 1)))
            .map(|t| t.idx)
            .collect();
        for i in idx {
            let running = self.explore[i as usize]
                .exec
                .clone()
                .filter(|e| self.executions.get(e).is_some_and(|r| r.result.is_none()));
            match running {
                Some(e) => {
                    self.interrupting.insert(e.clone(), Interrupt::Skipped);
                    self.skip_task(&e);
                }
                None => {
                    let t = &mut self.explore[i as usize];
                    t.abandoned = true;
                    if let ExploreOrigin::LoopAnswer { phase, tests } = t.origin {
                        self.release_answer_research((phase, tests));
                    }
                }
            }
        }
    }

    /// Rule P4: the user stopped this execution, so nothing retries it without them, YOLO included.
    pub fn stopped_by_user(&self, exec: &ExecutionId) -> bool {
        // A pause or an interrupt turns a cancel into Interrupted, and a stopped session ends,
        // so a Cancelled run in a live session is one the user stopped.
        self.executions
            .get(exec)
            .and_then(|r| r.result.as_ref())
            .is_some_and(|r| r.status == ExecutionStatus::Cancelled)
    }

    fn apply_held(&mut self, gate: &GateId, held: HeldAnswer, out: &RouteAnswerOut) {
        let target = held.target();
        self.track_mut(target).set_routing(None);
        let research = self.queue_research(&out.research, ExploreOrigin::Answer);
        let research_note = (!research.is_empty()).then(|| {
            "The user asked for more research before this step. Fold the new research documents into the spec.".to_string()
        });
        match held {
            HeldAnswer::Questions { target, answers } => {
                let questions = match self.gates.get(gate).map(|g| &g.payload) {
                    Some(GatePayload::OpenQuestions { questions, .. }) => questions.clone(),
                    _ => vec![],
                };
                let mut sent = vec![];
                for a in answers {
                    let item = out.item(&a.id);
                    let in_context = questions
                        .iter()
                        .find(|q| q.id == a.id)
                        .map(|q| q.answer_in_context(&a.answer))
                        .unwrap_or_else(|| a.answer.clone());
                    self.remember_parts(Some(gate), &out.items, &a.id, &in_context);
                    let answer = match item.disposition {
                        Disposition::Deliver => in_context,
                        Disposition::Remember => format!(
                            "The user gave this answer for a later stage, which receives it directly: {}. Remove the question and add no requirement for it.",
                            if item.note.trim().is_empty() { &a.answer } else { &item.note }
                        ),
                        Disposition::Discard => "The user chose not to answer this. Remove the question, and settle the point from the research or record it as an assumption.".into(),
                    };
                    sent.push(QuestionAnswer { answer, ..a });
                }
                match target {
                    FactTarget::Spec => {
                        // Rule D3: every answer re-runs generate-spec.
                        self.spec.answers.extend(sent);
                        self.spec.needs_run = true;
                    }
                    FactTarget::Plan => {
                        let text = sent
                            .iter()
                            .map(|a| format!("{} {}\nAnswer: {}", a.id, a.question, a.answer))
                            .collect::<Vec<_>>()
                            .join("\n");
                        // After the plan exists, answers go into the spec first (answer routing).
                        self.requirement_change(format!(
                            "Answers to the plan's clarifying questions:\n{text}{}",
                            research_note.map(|n| format!("\n{n}")).unwrap_or_default()
                        ));
                    }
                }
            }
            HeldAnswer::Approval {
                target,
                approve,
                version,
                text,
            } => {
                let item = out.item(ANSWER_ITEM);
                self.remember_parts(Some(gate), &out.items, ANSWER_ITEM, &text);
                let change = match item.disposition {
                    Disposition::Deliver => Some(text),
                    _ => research_note,
                };
                match (target, change) {
                    (FactTarget::Spec, Some(c)) => {
                        self.spec.changes.push(c);
                        self.spec.needs_run = true;
                    }
                    // Rule D10: a change after the plan exists goes into the spec first.
                    (FactTarget::Plan, Some(c)) => self.requirement_change(c),
                    (FactTarget::Spec, None) => {
                        if approve && self.spec.version == version && !self.spec.needs_run {
                            self.approve_spec();
                        }
                    }
                    (FactTarget::Plan, None) => {
                        if approve
                            && self.plan.version == version
                            && !self.plan.needs_run
                            && !self.plan.invalidated
                        {
                            self.plan.approved = true;
                            self.plan.approved_version = self.plan.version;
                            self.adopt_plan_phases();
                        }
                    }
                }
            }
            HeldAnswer::Recurring { target, text } => {
                let item = out.item(ANSWER_ITEM);
                self.remember_parts(Some(gate), &out.items, ANSWER_ITEM, &text);
                let change = match item.disposition {
                    Disposition::Deliver => Some(text),
                    _ => research_note,
                };
                match (target, change) {
                    (FactTarget::Spec, Some(c)) => {
                        self.spec.changes.push(c);
                        self.spec.needs_run = true;
                    }
                    (FactTarget::Plan, Some(c)) => self.requirement_change(c),
                    (_, None) => {}
                }
            }
        }
    }

    fn apply_loop_route(
        &mut self,
        key: (u32, bool),
        gate: &GateId,
        next: LoopNext,
        out: &RouteAnswerOut,
    ) {
        let LoopNext::AwaitRoute {
            text,
            then,
            base,
            stuck,
            fallback,
            ..
        } = next
        else {
            return;
        };
        let item = out.item(ANSWER_ITEM);
        self.remember_parts(Some(gate), &out.items, ANSWER_ITEM, &text);
        let deliver = item.disposition == Disposition::Deliver;
        // With no spec there is nothing to change first, so the answer goes to the phase.
        if deliver && out.route == AnswerRoute::RequirementChange && self.spec.current.is_some() {
            // Rule D10: stop the phase and restart at the spec.
            if let Some(l) = self.loop_mut(key) {
                l.next = LoopNext::Blocked {
                    reason: "The user changed a requirement; the spec is being updated (Rule D10)."
                        .into(),
                };
                l.announced_block = true;
                l.block_gate_answered = true;
            }
            self.requirement_change(text);
            return;
        }
        let next = if deliver {
            let instructions = match (&stuck, base) {
                (Some(st), _) => rescue_context(st, &text),
                (None, Some(b)) => format!("{b}\n\nThe user added: {text}"),
                (None, None) => text,
            };
            LoopNext::Work {
                kind: then,
                instructions: Some(instructions),
            }
        } else {
            *fallback
        };
        let tasks = if matches!(next, LoopNext::Work { .. }) {
            self.queue_research(
                &out.research,
                ExploreOrigin::LoopAnswer {
                    phase: key.0,
                    tests: key.1,
                },
            )
        } else {
            vec![]
        };
        if let Some(l) = self.loop_mut(key) {
            if matches!(next, LoopNext::Blocked { .. }) {
                l.block_gate_answered = true;
            }
            l.next = if tasks.is_empty() {
                next
            } else {
                LoopNext::AnswerResearch {
                    tasks,
                    next: Box::new(next),
                }
            };
        }
    }

    /// Rule J1: once every research task an answer queued has finished, the loop continues with
    /// their documents added to its instructions.
    fn release_answer_research(&mut self, key: (u32, bool)) {
        let Some(LoopNext::AnswerResearch { tasks, next }) =
            self.loop_ref(key).map(|l| l.next.clone())
        else {
            return;
        };
        let found: Vec<&ExploreTask> = tasks
            .iter()
            .filter_map(|i| self.explore.get(*i as usize))
            .collect();
        if found.iter().any(|t| !t.finished()) {
            return;
        }
        let docs: Vec<String> = found
            .iter()
            .filter_map(|t| t.result.as_ref())
            .map(|r| format!("- {}: {}", r.research_path, r.findings_summary))
            .collect();
        let next = match *next {
            LoopNext::Work { kind, instructions } if !docs.is_empty() => LoopNext::Work {
                kind,
                instructions: Some(format!(
                    "{}\n\nResearch the user asked for. Read each document before you start:\n{}",
                    instructions.unwrap_or_default(),
                    docs.join("\n")
                )),
            },
            other => other,
        };
        if let Some(l) = self.loop_mut(key) {
            l.next = next;
        }
    }

    /// Rule J1: the notes the judge kept for a later stage, oldest first.
    pub fn notes_for(&self, stage: NoteStage) -> Vec<String> {
        self.user_notes
            .iter()
            .filter(|n| !n.forgotten && n.stages.contains(&stage))
            .map(|n| n.text.clone())
            .collect()
    }

    fn interrupt_running(&mut self, why: Interrupt) {
        let running: Vec<ExecutionId> = self.running_executions().map(|r| r.id.clone()).collect();
        for id in running {
            self.interrupting.entry(id).or_insert(why);
        }
    }

    fn on_amended(&mut self, text: &str) {
        if self.classify.is_none() {
            return;
        }
        // Rule D2: research the new part before the spec is written again.
        let project = self.primary();
        if matches!(
            self.category,
            Some(Category::Research | Category::Spec | Category::Plan | Category::Implement)
        ) {
            self.push_explore(
                project,
                format!(
                    "The user extended the request. Research the part they added, in the context of the whole request.\nAdded part: {text}\nWhole request: {}",
                    self.request
                ),
                ExploreOrigin::Amendment,
            );
        }
        if !self.spec.runs.is_empty() {
            self.requirement_change(format!("The user extended the request: {text}"));
        }
    }

    fn apply_classify(&mut self, out: &ClassifyOut) {
        self.category = Some(out.category);
        if let Some(t) = clean_title(&out.title) {
            self.title = Some(t);
        }
        // Rule O6: pinned projects are the whole scope, whatever the judge picked.
        let picked = if self.pinned.is_empty() {
            &out.projects
        } else {
            &self.pinned
        };
        let mut scope: Vec<String> = picked
            .iter()
            .filter(|p| self.valid_project(p))
            .cloned()
            .collect();
        scope.dedup();
        if scope.is_empty()
            && let Some(p) = self.projects.first()
        {
            scope.push(p.key.clone());
        }
        // Rule O3: a created project stays in scope when the request is classified again.
        for c in &self.created_projects {
            if !scope.contains(&c.key) {
                scope.push(c.key.clone());
            }
        }
        self.scope = scope;
        self.opts_in = out.opts_in;
        self.explore.retain(|t| t.origin != ExploreOrigin::Classify);
        self.phases.clear();
        let explores = matches!(
            out.category,
            Category::Research | Category::Spec | Category::Plan | Category::Implement
        );
        if explores {
            let mut tasks: Vec<(String, String)> = out
                .explore_tasks
                .iter()
                .filter(|t| self.valid_project(&t.project) && !t.task.trim().is_empty())
                .map(|t| (t.project.clone(), t.task.clone()))
                .collect();
            if tasks.is_empty() {
                // Rule D1: the spec derives its criteria from research, so there is always one.
                tasks = self
                    .scope
                    .iter()
                    .map(|p| (p.clone(), self.full_request()))
                    .collect();
            }
            for (project, task) in tasks {
                self.push_explore(project, task, ExploreOrigin::Classify);
            }
        }
        let scope = self.scope.clone();
        match out.category {
            Category::Verify => {
                for (i, key) in scope.iter().enumerate() {
                    let id = i as u32 + 1;
                    let mut l =
                        WorkLoop::new(false, AgentName::Implementer, AgentName::Implementer);
                    l.review = ReviewMode::Never;
                    l.stage = false;
                    self.insert_phase(inline_phase(id, key, "Verification", i), l);
                }
            }
            Category::Docs => {
                for (i, key) in scope.iter().enumerate() {
                    let id = i as u32 + 1;
                    let mut l =
                        WorkLoop::new(false, AgentName::Implementer, AgentName::Implementer);
                    l.next = LoopNext::Done;
                    let report = self
                        .project_session_dir(key)
                        .join(paths::report::docs_request());
                    self.insert_phase(
                        inline_phase(id, key, "Documentation requested by the user", i),
                        l,
                    );
                    if let Some(p) = self.phases.get_mut(&id) {
                        p.implementer_report = Some(report);
                        p.info.depends_on = Some(vec![]);
                    }
                    self.project_tracks.entry(key.clone()).or_default().closing =
                        Some((false, true));
                    self.project_tracks.entry(key.clone()).or_default().format = Some(None);
                }
            }
            Category::Test => {
                for (i, key) in scope.iter().enumerate() {
                    let id = i as u32 + 1;
                    let mut l =
                        WorkLoop::new(false, AgentName::Implementer, AgentName::Implementer);
                    l.next = LoopNext::Done;
                    let report = self
                        .project_session_dir(key)
                        .join(paths::report::test_request());
                    self.insert_phase(inline_phase(id, key, "Tests requested by the user", i), l);
                    if let Some(p) = self.phases.get_mut(&id) {
                        p.implementer_report = Some(report);
                        p.info.depends_on = Some(vec![]);
                    }
                    self.project_tracks.entry(key.clone()).or_default().closing =
                        Some((true, false));
                    self.project_tracks.entry(key.clone()).or_default().format = Some(None);
                }
            }
            // Quick change (HANDOVER 8.2): one pass per project, no review, staged when it changed files.
            Category::QuickChange => {
                for (i, key) in scope.iter().enumerate() {
                    let mut l =
                        WorkLoop::new(false, AgentName::Implementer, AgentName::Implementer);
                    l.review = ReviewMode::Never;
                    self.insert_phase(inline_phase(i as u32 + 1, key, "Quick change", i), l);
                }
            }
            Category::Prompt => {
                if let Some(key) = scope.first() {
                    let mut l =
                        WorkLoop::new(false, AgentName::PromptGeneration, AgentName::Implementer);
                    l.review = ReviewMode::IfCodeChanged;
                    self.insert_phase(inline_phase(1, key, "Prompt change", 0), l);
                }
            }
            _ => {}
        }
        if let Some(track) = self.track {
            self.set_track(track);
        }
    }

    /// Light track: one inline phase per project, queued in order (Rule M5), built from the
    /// request and the research. Full track: the spec and plan stages create the phases.
    fn set_track(&mut self, track: Track) {
        self.track = Some(track);
        if self.category != Some(Category::Implement) {
            return;
        }
        self.phases.clear();
        if track == Track::Light {
            let scope = self.scope.clone();
            for (i, key) in scope.iter().enumerate() {
                let l = WorkLoop::new(false, AgentName::Implementer, AgentName::Implementer);
                self.insert_phase(inline_phase(i as u32 + 1, key, "Implementation", i), l);
            }
        }
    }

    fn add_feedback(&mut self, text: String, routed: bool) {
        self.feedback.rounds.push(FeedbackRound {
            text: text.clone(),
            route: None,
            reason: None,
            targets: vec![],
            awaiting_spec: false,
            phases: vec![],
        });
        // Before Rule J1, a round with no spec and one project was built without the judge.
        if !routed && self.spec.current.is_none() && self.scope.len() == 1 {
            let i = self.feedback.rounds.len() - 1;
            let targets = vec![FeedbackTarget {
                project: self.primary(),
                instruction: text,
            }];
            self.route_feedback(i, AnswerRoute::ImplementationDetail, targets, None);
        }
    }

    fn route_feedback(
        &mut self,
        i: usize,
        route: AnswerRoute,
        targets: Vec<FeedbackTarget>,
        reason: Option<String>,
    ) {
        let text = self.feedback.rounds[i].text.clone();
        let mut targets: Vec<FeedbackTarget> = targets
            .into_iter()
            .filter(|t| self.valid_project(&t.project) && !t.instruction.trim().is_empty())
            .collect();
        if targets.is_empty() {
            targets.push(FeedbackTarget {
                project: self.primary(),
                instruction: text.clone(),
            });
        }
        // Rule D10: a requirement change goes into the spec before anything is built from it.
        let spec_first = route == AnswerRoute::RequirementChange && self.spec.current.is_some();
        let r = &mut self.feedback.rounds[i];
        r.route = Some(route);
        r.reason = reason;
        r.targets = targets;
        if spec_first {
            r.awaiting_spec = true;
            self.spec.changes.push(format!(
                "The user reviewed the implementation and asked for: {text}"
            ));
            self.spec.needs_run = true;
            self.spec.revoke_approval();
        } else {
            self.add_revision_phases(i);
        }
    }

    fn add_revision_phases(&mut self, i: usize) {
        let round = i as u32 + 1;
        let targets = self.feedback.rounds[i].targets.clone();
        for t in targets {
            let id = self
                .phases
                .keys()
                .chain(self.superseded_phases.iter().map(|p| &p.info.id))
                .max()
                .copied()
                .unwrap_or(0)
                + 1;
            let info = PhaseInfo {
                id,
                deliverable: None,
                project: t.project.clone(),
                title: format!("Revision {round}"),
                complexity: Complexity::Low,
                test_policy: TestPolicy::Required,
                depends_on: Some(vec![]),
                file: None,
                test_rationale: None,
            };
            self.insert_phase(
                info,
                WorkLoop::new(false, AgentName::Implementer, AgentName::Implementer),
            );
            if let Some(p) = self.phases.get_mut(&id) {
                p.revision = Some(Revision {
                    round,
                    instruction: t.instruction,
                });
            }
            self.feedback.rounds[i].phases.push(id);
        }
    }

    pub fn session_context_path(&self) -> PathBuf {
        self.session_root.join(paths::report::session_context())
    }

    /// Rule D4a: where the runner writes the code facts before a spawn that gets them.
    pub fn code_facts_path(&self) -> PathBuf {
        self.session_root.join(paths::report::code_facts())
    }

    fn insert_phase(&mut self, info: PhaseInfo, impl_loop: WorkLoop) {
        self.project_tracks.entry(info.project.clone()).or_default();
        self.phases.insert(
            info.id,
            PhaseRun {
                info,
                impl_loop,
                test_loop: WorkLoop::new(true, AgentName::WriteTest, AgentName::WriteTest),
                epa: EpaState::NotStarted,
                implementer_report: None,
                blocked_gate: None,
                revision: None,
            },
        );
    }

    fn any_explore_started(&self) -> bool {
        self.explore.iter().any(|t| t.exec.is_some())
    }

    fn any_phase_started(&self) -> bool {
        self.phases
            .values()
            .any(|p| !p.impl_loop.is_idle() && p.impl_loop.work_count > 0)
    }

    /// Whether overriding a decision can still change what happens (HANDOVER 8.3 override button).
    pub fn can_override(&self, id: &DecisionId) -> bool {
        let Some(d) = self.decisions.get(id) else {
            return false;
        };
        if self.is_terminal() {
            return false;
        }
        match d.judge {
            JudgeKind::Classify => {
                !self.any_explore_started()
                    && self.phases.values().all(|p| p.impl_loop.work_count == 0)
            }
            JudgeKind::Stakes => self.plan.runs.is_empty() && !self.any_phase_started(),
            JudgeKind::Track => self.spec.runs.is_empty() && !self.any_phase_started(),
            JudgeKind::Sufficiency => self.spec.runs.is_empty(),
            _ => false,
        }
    }

    fn on_decision(
        &mut self,
        id: &DecisionId,
        judge: JudgeKind,
        subject: Option<&str>,
        output: &Value,
        overriding: bool,
    ) {
        match judge {
            JudgeKind::Classify => {
                if overriding && !self.can_override_classify_now() {
                    return;
                }
                if let Ok(out) = serde_json::from_value::<ClassifyOut>(output.clone()) {
                    self.classify = Some(id.clone());
                    self.apply_classify(&out);
                } else if !overriding {
                    self.classify = Some(id.clone());
                    self.failed = Some(
                        "The Classify judge returned output that does not match its schema.".into(),
                    );
                }
            }
            JudgeKind::Sufficiency => {
                let covered: Vec<u32> = subject
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if !overriding {
                    self.sufficiency_rounds += 1;
                }
                for t in self.explore.iter_mut().filter(|t| covered.contains(&t.idx)) {
                    t.judged = true;
                }
                if overriding {
                    self.explore
                        .retain(|t| !(t.origin == ExploreOrigin::Sufficiency && t.exec.is_none()));
                }
                if let Ok(out) = serde_json::from_value::<SufficiencyOut>(output.clone()) {
                    let mut added = 0;
                    for item in out.items.into_iter().filter(|i| i.needed) {
                        if added == MAX_SUFFICIENCY_RESEARCH {
                            break;
                        }
                        let (project, task) = match item.task {
                            Some(t) if self.valid_project(&t.project) => (t.project, t.task),
                            _ => (self.primary(), item.item.clone()),
                        };
                        // Rule D2: the judge often maps several items to one task; research it once.
                        if self
                            .explore
                            .iter()
                            .any(|t| !t.finished() && t.project == project && t.task == task)
                        {
                            continue;
                        }
                        self.push_explore(project, task, ExploreOrigin::Sufficiency);
                        added += 1;
                    }
                }
            }
            JudgeKind::Stakes => {
                if let Ok(out) = serde_json::from_value::<StakesOut>(output.clone()) {
                    if overriding && !(self.plan.runs.is_empty() && !self.any_phase_started()) {
                        return;
                    }
                    self.stakes = Some((id.clone(), out.stakes));
                    if overriding {
                        self.phases.clear();
                    }
                    if out.stakes == Stakes::Low && self.category == Some(Category::Implement) {
                        // Plan skipped for a lower-stakes request: inline phases, one per project,
                        // queued in order because there is no graph to read (Rule M5).
                        let scope = self.scope.clone();
                        for (i, key) in scope.iter().enumerate() {
                            let l = WorkLoop::new(
                                false,
                                AgentName::Implementer,
                                AgentName::Implementer,
                            );
                            self.insert_phase(
                                inline_phase(i as u32 + 1, key, "Implementation", i),
                                l,
                            );
                        }
                    }
                }
            }
            JudgeKind::Rescue => {
                let Some(exec) = subject.map(ExecutionId::from) else {
                    return;
                };
                let Ok(out) = serde_json::from_value::<RescueOut>(output.clone()) else {
                    return;
                };
                let key = self.executions.get(&exec).and_then(|r| r.loop_key);
                let Some(key) = key else { return };
                let project = self
                    .phases
                    .get(&key.0)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let next = self.loop_ref(key).map(|l| l.next.clone());
                let Some(LoopNext::Rescue {
                    exec: stuck_exec,
                    stuck,
                }) = next
                else {
                    return;
                };
                if stuck_exec != exec {
                    return;
                }
                let new_next = match out.action {
                    RescueAction::Explore => {
                        let (p, task) = match out.explore_task {
                            Some(t) if self.valid_project(&t.project) => (t.project, t.task),
                            _ => (
                                project,
                                format!(
                                    "Find the fact this agent needs.\nNeed: {}\nDiagnostic:\n{}",
                                    stuck.need, stuck.diagnostic
                                ),
                            ),
                        };
                        let idx = self.push_explore(
                            p,
                            task,
                            ExploreOrigin::Rescue {
                                phase: key.0,
                                tests: key.1,
                            },
                        );
                        LoopNext::RescueExplore { task: idx, stuck }
                    }
                    RescueAction::Rerun => LoopNext::Work {
                        kind: WorkKind::Rescue,
                        instructions: Some(rescue_context(
                            &stuck,
                            out.fact.as_deref().unwrap_or(&out.reason),
                        )),
                    },
                    // Rule O7: at most MAX_ADVICE advisor rounds per loop before the user is asked.
                    RescueAction::Advise
                        if self
                            .loop_ref(key)
                            .is_some_and(|l| l.advice.len() < crate::init::MAX_ADVICE) =>
                    {
                        LoopNext::RescueAdvise {
                            exec,
                            stuck,
                            advisor: None,
                        }
                    }
                    RescueAction::Advise | RescueAction::Gate => {
                        LoopNext::RescueGate { exec, stuck }
                    }
                };
                if let Some(l) = self.loop_mut(key) {
                    l.next = new_next;
                }
            }
            JudgeKind::ResolveReview => {
                let Some(key) = subject.and_then(parse_loop_key) else {
                    return;
                };
                let Ok(out) = serde_json::from_value::<ResolveReviewOut>(output.clone()) else {
                    return;
                };
                let project = self
                    .phases
                    .get(&key.0)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let ledger = self
                    .ledger_path(&project, key.0, key.1)
                    .display()
                    .to_string();
                let Some(l) = self.loop_mut(key) else { return };
                let LoopNext::Resolve { findings } = l.next.clone() else {
                    return;
                };
                match out.action {
                    ResolveAction::Fix => {
                        l.resolve_rounds += 1;
                        l.open_before_resolve = Some(findings.len());
                        // One verification pass per resolution round (review-cap.js).
                        l.extra_cap += 1;
                        let mut text = String::from(
                            "Resolve these review findings with the instructions given for each:\n",
                        );
                        for i in &out.instructions {
                            text.push_str(&format!(
                                "- Finding: {}\n  Instruction: {}\n",
                                i.finding, i.instruction
                            ));
                        }
                        if out.instructions.is_empty() {
                            text = fix_instructions(&findings, &ledger);
                        } else {
                            text.push_str(&format!("\nRecord a FIXED or WONTFIX line for each in the review ledger at {ledger}."));
                        }
                        l.next = LoopNext::Work {
                            kind: WorkKind::Fix,
                            instructions: Some(text),
                        };
                    }
                    ResolveAction::Block => {
                        l.next = LoopNext::Blocked {
                            reason: format!(
                                "The engine could not resolve the open review findings: {}. Ledger: {ledger}",
                                out.reason
                            ),
                        };
                    }
                }
            }
            JudgeKind::RouteAnswer => {
                let Ok(out) = serde_json::from_value::<RouteAnswerOut>(output.clone()) else {
                    return;
                };
                if let Some(i) = subject.and_then(amendment_index) {
                    self.apply_amendment_route(i, &out);
                    return;
                }
                let Some(gate) = subject.map(GateId::from) else {
                    return;
                };
                if !self.held_answers.contains_key(&gate)
                    && !self.phases.values().any(|p| {
                        [&p.impl_loop, &p.test_loop].iter().any(
                        |l| matches!(&l.next, LoopNext::AwaitRoute { gate: g, .. } if *g == gate),
                    )
                    })
                {
                    return;
                }
                self.forget(&out.forget);
                self.skip_research(&out.skip);
                if let Some(held) = self.held_answers.remove(&gate) {
                    self.apply_held(&gate, held, &out);
                    return;
                }
                let target = self
                    .phases
                    .iter()
                    .flat_map(|(id, p)| [((*id, false), &p.impl_loop), ((*id, true), &p.test_loop)])
                    .find_map(|(k, l)| match &l.next {
                        LoopNext::AwaitRoute { gate: g, .. } if *g == gate => {
                            Some((k, l.next.clone()))
                        }
                        _ => None,
                    });
                if let Some((key, next)) = target {
                    self.apply_loop_route(key, &gate, next, &out);
                }
            }
            JudgeKind::Track => {
                let Ok(out) = serde_json::from_value::<TrackOut>(output.clone()) else {
                    return;
                };
                if overriding && !self.can_override(id) {
                    return;
                }
                self.track_decision = Some(id.clone());
                self.set_track(out.track);
            }
            JudgeKind::Feedback => {
                let Some(i) = subject.and_then(|s| s.parse::<usize>().ok()) else {
                    return;
                };
                let Ok(out) = serde_json::from_value::<FeedbackOut>(output.clone()) else {
                    return;
                };
                if self
                    .feedback
                    .rounds
                    .get(i)
                    .is_none_or(|r| r.route.is_some())
                {
                    return;
                }
                let Some(gate) = self.feedback_gate(i) else {
                    self.route_feedback(i, out.route, out.targets, Some(out.reason));
                    return;
                };
                let item = item_for(&out.items, ANSWER_ITEM);
                let text = self.feedback.rounds[i].text.clone();
                self.forget(&out.forget);
                self.remember_parts(Some(&gate), &out.items, ANSWER_ITEM, &text);
                self.queue_research(&out.research, ExploreOrigin::Answer);
                match item.disposition {
                    Disposition::Deliver => {
                        self.route_feedback(i, out.route, out.targets, Some(out.reason))
                    }
                    Disposition::Remember | Disposition::Discard => {
                        let r = &mut self.feedback.rounds[i];
                        r.route = Some(out.route);
                        r.reason = Some(out.reason);
                        // Keeping the feedback only for later stages accepts what was built.
                        if item.disposition == Disposition::Remember {
                            self.feedback.accepted = true;
                        }
                    }
                }
            }
            JudgeKind::Completion => self.completion_decision = Some(id.clone()),
            JudgeKind::YoloAnswer => {}
        }
    }

    /// The executor an agent must run on regardless of routing: native after a harness failure,
    /// native for a quick change, which skips the harness startup cost, and native for a quick
    /// answer, which runs the side panel's agent (HANDOVER 12.3).
    pub fn forced_executor(&self, agent: AgentName) -> Option<ostra_core::ExecutorKind> {
        (self.native_fallback.contains(&agent)
            || self.category == Some(Category::QuickChange)
            || agent == AgentName::QuickAnswer)
            .then_some(ostra_core::ExecutorKind::Native)
    }

    fn can_override_classify_now(&self) -> bool {
        !self.any_explore_started() && self.phases.values().all(|p| p.impl_loop.work_count == 0)
    }

    fn on_started(
        &mut self,
        id: &ExecutionId,
        purpose: &ExecPurpose,
        loop_key: Option<(u32, bool)>,
        resumed: bool,
    ) {
        // A resumed run is the same run: it adds no run, pass, or work count, and keeps the
        // inputs its conversation already saw.
        match purpose {
            ExecPurpose::Explore { task } => {
                if let Some(t) = self.explore.get_mut(*task as usize) {
                    t.exec = Some(id.clone());
                    t.running = true;
                    t.failed = None;
                }
            }
            ExecPurpose::Spec { .. } => {
                let docs = self.research_docs();
                let t = &mut self.spec;
                t.running = Some(id.clone());
                t.needs_run = false;
                if !resumed {
                    t.runs.push(id.clone());
                    t.sent = InputMark {
                        answers: t.answers.len(),
                        changes: t.changes.len(),
                        docs,
                    };
                }
            }
            ExecPurpose::Plan { .. } => {
                self.plan.running = Some(id.clone());
                if !resumed {
                    self.plan.runs.push(id.clone());
                }
                self.plan.needs_run = false;
                // Rule D10: the plan is revised in place against the changed spec, not replaced.
                if self.plan.invalidated {
                    self.plan.invalidated = false;
                    self.plan.pending_findings = None;
                    self.plan.consecutive_fails = 0;
                }
            }
            ExecPurpose::FactCheck { target, .. } => match target {
                FactTarget::Spec => self.spec.start_check(id),
                FactTarget::Plan => self.plan.start_check(id),
            },
            ExecPurpose::Epa { phase } => {
                if let Some(p) = self.phases.get_mut(phase) {
                    p.epa = EpaState::Running(id.clone());
                }
            }
            ExecPurpose::Docs { project, area } => {
                *self
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_mut(area.as_deref()) = DocsState::Running(id.clone());
            }
            ExecPurpose::Architecture => {
                self.architecture = ArchitectureState::Running(id.clone());
            }
            ExecPurpose::QuickAnswer => {
                self.quick.exec = Some(id.clone());
                self.quick.running = true;
            }
            ExecPurpose::Init { mode, item } => self.init_started(id, *mode, item.clone()),
            ExecPurpose::Advise {
                project, execution, ..
            } => {
                if let Some(l) = self.stuck_loop_mut(execution)
                    && let LoopNext::RescueAdvise { advisor, .. } = &mut l.next
                {
                    *advisor = Some(id.clone());
                } else if let Some(i) = self.project_inits.get_mut(project) {
                    i.advising = Some(id.clone());
                }
            }
            ExecPurpose::Unblock { execution, .. } => {
                if let Some(l) = self.fixing_loop_mut(execution)
                    && let LoopNext::RescueFix { fixer, .. } = &mut l.next
                {
                    *fixer = Some(id.clone());
                }
            }
            ExecPurpose::PromptGen { handoff_for: None } if !resumed => self.prompt_gens += 1,
            _ => {}
        }
        if let Some(key) = loop_key {
            let is_handoff = matches!(
                purpose,
                ExecPurpose::PromptGen {
                    handoff_for: Some(_)
                }
            );
            if matches!(purpose, ExecPurpose::PromptGen { .. }) && !resumed {
                self.prompt_gens += u32::from(is_handoff);
            }
            if let Some(l) = self.loop_mut(key) {
                l.running = Some(id.clone());
                l.in_flight = Some(l.next.clone());
                if !resumed
                    && matches!(
                        purpose,
                        ExecPurpose::Implement { .. }
                            | ExecPurpose::WriteTest { .. }
                            | ExecPurpose::Verify { .. }
                    )
                    || matches!(purpose, ExecPurpose::PromptGen { handoff_for: None })
                {
                    l.work_count += 1;
                }
            }
        }
    }

    fn on_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult) {
        let status = result.status;
        let error = exec_error(result);
        match &rec.purpose {
            ExecPurpose::Explore { task } => {
                let idx = *task as usize;
                let parsed: Option<ExploreSubmit> = parse(&result.submit);
                let Some(t) = self.explore.get_mut(idx) else {
                    return;
                };
                t.running = false;
                match (status, parsed) {
                    (ExecutionStatus::Ok, Some(sub)) => t.result = Some(sub),
                    (ExecutionStatus::Interrupted, _) => t.exec = None,
                    (_, _) => {
                        if t.retries < ERROR_RETRIES && status == ExecutionStatus::Error {
                            t.retries += 1;
                            t.exec = None;
                        } else {
                            t.failed = Some(missing_submit(status, &error, result));
                        }
                    }
                }
                let origin = t.origin.clone();
                let summary = t.result.clone();
                if let ExploreOrigin::LoopAnswer { phase, tests } = origin {
                    self.release_answer_research((phase, tests));
                }
                if let (ExploreOrigin::Rescue { phase, tests }, Some(sub)) = (origin, summary) {
                    let task_idx = *task;
                    if let Some(l) = self.loop_mut((phase, tests))
                        && let LoopNext::RescueExplore {
                            task: waiting,
                            stuck,
                        } = l.next.clone()
                        && waiting == task_idx
                    {
                        let fact = format!(
                            "A targeted explore found: {}\nResearch document: {}",
                            sub.findings_summary, sub.research_path
                        );
                        l.next = LoopNext::Work {
                            kind: WorkKind::Rescue,
                            instructions: Some(rescue_context(&stuck, &fact)),
                        };
                    }
                }
            }
            ExecPurpose::Spec { .. } => {
                self.spec.running = None;
                match (status, parse::<GenerateSpecSubmit>(&result.submit)) {
                    (ExecutionStatus::Ok, Some(sub)) => {
                        self.spec.current = Some(sub);
                        self.spec.applied = self.spec.sent.clone();
                        self.spec.version += 1;
                        self.spec.pending_findings = None;
                        self.spec.revoke_approval();
                        self.spec.error_retries = 0;
                    }
                    (ExecutionStatus::Interrupted, _) => self.spec.needs_run = true,
                    _ => artifact_error(
                        &mut self.spec,
                        status,
                        missing_submit(status, &error, result),
                    ),
                }
            }
            ExecPurpose::Plan { .. } => {
                self.plan.running = None;
                match (status, parse::<PlanSubmit>(&result.submit)) {
                    (ExecutionStatus::Ok, Some(sub)) => {
                        self.plan.current = Some(sub);
                        self.plan.version += 1;
                        self.plan.pending_findings = None;
                        self.plan.revoke_approval();
                        self.plan.error_retries = 0;
                    }
                    (ExecutionStatus::Interrupted, _) => self.plan.needs_run = true,
                    _ => artifact_error(
                        &mut self.plan,
                        status,
                        missing_submit(status, &error, result),
                    ),
                }
            }
            ExecPurpose::FactCheck { target, .. } => {
                let parsed: Option<FactCheckSubmit> = parse(&result.submit);
                let (checks, failed_msg) = (parsed, missing_submit(status, &error, result));
                match target {
                    FactTarget::Spec => {
                        fact_finished(&mut self.spec, &rec.id, status, checks, failed_msg)
                    }
                    FactTarget::Plan => {
                        fact_finished(&mut self.plan, &rec.id, status, checks, failed_msg)
                    }
                }
            }
            ExecPurpose::Epa { phase } => {
                let report = parse::<ReportSubmit>(&result.submit)
                    .map(|r| PathBuf::from(r.report_path))
                    .or_else(|| rec.report_path.clone());
                if let Some(p) = self.phases.get_mut(phase) {
                    let retries = match &p.epa {
                        EpaState::Failed { retries, .. } => *retries,
                        _ => 0,
                    };
                    p.epa = match status {
                        ExecutionStatus::Ok => EpaState::Done(report.unwrap_or_default()),
                        ExecutionStatus::Interrupted => EpaState::NotStarted,
                        ExecutionStatus::Error if retries < ERROR_RETRIES => EpaState::NotStarted,
                        _ => EpaState::Failed {
                            exec: rec.id.clone(),
                            error: error.clone(),
                            gate: None,
                            retries: retries + 1,
                        },
                    };
                }
            }
            ExecPurpose::Docs { project, area } => {
                let t = self.project_tracks.entry(project.clone()).or_default();
                *t.docs_mut(area.as_deref()) = stage_run(
                    status,
                    parse::<DocumentationSubmit>(&result.submit),
                    &rec.id,
                    error,
                );
            }
            ExecPurpose::Architecture => {
                self.architecture = stage_run(
                    status,
                    parse::<ArchitectureSubmit>(&result.submit),
                    &rec.id,
                    error,
                );
            }
            ExecPurpose::QuickAnswer => {
                self.quick.running = false;
                match (status, parse::<QuickAnswerSubmit>(&result.submit)) {
                    (ExecutionStatus::Ok, Some(a)) => self.quick.answer = Some(a),
                    (ExecutionStatus::Ok, None) if !result.final_text.trim().is_empty() => {
                        self.quick.answer = Some(QuickAnswerSubmit {
                            answer: result.final_text.clone(),
                            sources: vec![],
                        })
                    }
                    (ExecutionStatus::Interrupted, _) => self.quick.exec = None,
                    _ => self.quick.failed = Some(error),
                }
            }
            ExecPurpose::Init { mode, item } => {
                self.init_finished(&rec.id, *mode, item.clone(), status, result, error)
            }
            ExecPurpose::Unblock { execution, .. } => {
                self.loop_fix_finished(execution, &rec.id, status, result, error)
            }
            ExecPurpose::Advise {
                project, execution, ..
            } => {
                if self.stuck_loop_mut(execution).is_some() {
                    self.loop_advice_finished(execution, status, result, error);
                } else {
                    self.advice_finished(project, execution, status, result, error);
                }
            }
            _ => {}
        }
        if let Some(key) = rec.loop_key {
            self.loop_finished(key, rec, result);
        }
    }

    fn loop_finished(&mut self, key: (u32, bool), rec: &ExecRecord, result: &ExecutionResult) {
        let yolo = self.yolo;
        let fresh = self.restart_fresh.remove(&rec.id);
        let project = self
            .phases
            .get(&key.0)
            .map(|p| p.info.project.clone())
            .unwrap_or_default();
        let ledger = self
            .ledger_path(&project, key.0, key.1)
            .display()
            .to_string();
        // Recorded at spawn from the project's Review Rule Set, so the fold stays a pure function
        // of the event log.
        let autofix_ids: BTreeSet<String> = rec
            .params
            .get(AUTO_FIXABLE_PARAM)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let status = result.status;
        let error = exec_error(result);
        let is_review = matches!(rec.purpose, ExecPurpose::Review { .. });
        let is_handoff = matches!(
            rec.purpose,
            ExecPurpose::PromptGen {
                handoff_for: Some(_)
            }
        );
        let review_findings: Option<CodeReviewerSubmit> = if is_review {
            parse(&result.submit)
        } else {
            None
        };
        let auto: Vec<bool> = review_findings
            .as_ref()
            .map(|r| {
                r.findings
                    .iter()
                    .map(|f| self.auto_fixable(&project, f, &autofix_ids))
                    .collect()
            })
            .unwrap_or_default();
        let Some(phase) = self.phases.get_mut(&key.0) else {
            return;
        };
        let l = if key.1 {
            &mut phase.test_loop
        } else {
            &mut phase.impl_loop
        };
        if l.running.as_ref() != Some(&rec.id) {
            return;
        }
        l.running = None;
        let in_flight = l.in_flight.take().unwrap_or(LoopNext::Idle);

        if status == ExecutionStatus::Interrupted && fresh {
            // Rule O4: the phase starts over inside the project the run created.
            l.next = LoopNext::Work {
                kind: WorkKind::Initial,
                instructions: None,
            };
            return;
        }
        if status == ExecutionStatus::Interrupted {
            // Re-run with the same spawn block (HANDOVER 11.2).
            l.next = match in_flight {
                LoopNext::Work { instructions, .. } => LoopNext::Work {
                    kind: WorkKind::Rerun,
                    instructions,
                },
                LoopNext::Idle => LoopNext::Work {
                    kind: WorkKind::Rerun,
                    instructions: None,
                },
                other => other,
            };
            return;
        }
        let failed_hard = !matches!(
            status,
            ExecutionStatus::Ok | ExecutionStatus::Stuck | ExecutionStatus::Handoff
        );
        if failed_hard {
            if status == ExecutionStatus::Error
                && l.error_retries < ERROR_RETRIES
                && !error.starts_with("harness-")
            {
                l.error_retries += 1;
                l.next = match in_flight {
                    LoopNext::Idle => LoopNext::Work {
                        kind: WorkKind::Rerun,
                        instructions: None,
                    },
                    LoopNext::Work { instructions, .. } => LoopNext::Work {
                        kind: WorkKind::Rerun,
                        instructions,
                    },
                    other => other,
                };
            } else {
                l.next = LoopNext::Failed {
                    exec: rec.id.clone(),
                    error,
                };
            }
            return;
        }

        if is_review {
            let Some(review) = review_findings else {
                l.next = LoopNext::Failed {
                    exec: rec.id.clone(),
                    error: missing_submit(status, &error, result),
                };
                return;
            };
            l.error_retries = 0;
            l.iterations += 1;
            l.last_review = Some(review.clone());
            if review.security_block
                || review
                    .findings
                    .iter()
                    .any(|f| f.severity == Severity::Blocker)
            {
                // Hard rule 21: BLOCKER findings go alone to the fix agent, with no cap.
                l.blocker_open = true;
                l.next = LoopNext::Work {
                    kind: WorkKind::BlockerFix,
                    instructions: Some(blocker_instructions(&review.findings, &ledger)),
                };
                return;
            }
            l.blocker_open = false;
            let mut apply = vec![];
            let mut remaining = vec![];
            let mut low = vec![];
            for (f, is_auto) in review.findings.iter().zip(auto) {
                if is_auto {
                    apply.push(f.clone());
                } else if matches!(f.severity, Severity::High | Severity::Medium) {
                    remaining.push(f.clone());
                } else {
                    low.push(f.clone());
                }
            }
            l.leftover_low = low;
            if !apply.is_empty() {
                l.next = LoopNext::Autofix { apply, remaining };
            } else {
                l.after_findings(remaining, yolo, &ledger);
            }
            return;
        }

        if is_handoff {
            l.next = match (status, in_flight) {
                (ExecutionStatus::Ok, LoopNext::Handoff { handoff, .. }) => LoopNext::Work {
                    kind: WorkKind::Resume,
                    instructions: Some(format!(
                        "prompt-generation finished the handoff you asked for ({}). Continue: {}",
                        handoff.request, handoff.resume_instructions
                    )),
                },
                _ => LoopNext::Failed {
                    exec: rec.id.clone(),
                    error: missing_submit(status, &error, result),
                },
            };
            return;
        }

        // A work pass: implementer, write-test, prompt-generation, or verify.
        let sub: Option<ImplementerSubmit> = parse(&result.submit);
        let report_sub: Option<ReportSubmit> = parse(&result.submit);
        let (sub_status, changed, report, stuck, handoff) = match (sub, report_sub) {
            (Some(s), _) => (
                s.status,
                s.changed_files,
                Some(s.report_path),
                s.stuck,
                s.handoff,
            ),
            (None, Some(r)) => (
                r.status,
                r.changed_files,
                Some(r.report_path),
                r.stuck,
                None,
            ),
            (None, None) => {
                l.next = LoopNext::Failed {
                    exec: rec.id.clone(),
                    error: missing_submit(status, &error, result),
                };
                return;
            }
        };
        l.error_retries = 0;
        l.changed.extend(changed.iter().cloned());
        if let Some(r) = report.filter(|r| !r.is_empty()) {
            l.report = Some(PathBuf::from(&r));
            if !key.1 {
                phase.implementer_report = Some(PathBuf::from(r));
            }
        } else if !key.1 && rec.report_path.is_some() {
            phase.implementer_report = rec.report_path.clone();
        }
        let l = if key.1 {
            &mut phase.test_loop
        } else {
            &mut phase.impl_loop
        };
        match sub_status {
            SubmitStatus::Stuck => {
                let stuck = stuck.unwrap_or(StuckInfo {
                    diagnostic: result.final_text.clone(),
                    need: "The agent reported STUCK without a diagnostic.".into(),
                });
                l.next = LoopNext::Rescue {
                    exec: rec.id.clone(),
                    stuck,
                };
            }
            SubmitStatus::Handoff => match handoff {
                Some(h) => {
                    l.next = LoopNext::Handoff {
                        exec: rec.id.clone(),
                        handoff: h,
                    }
                }
                None => {
                    l.next = LoopNext::Failed {
                        exec: rec.id.clone(),
                        error: "The agent reported HANDOFF without saying what it needs.".into(),
                    }
                }
            },
            SubmitStatus::Ok => {
                let review = match l.review {
                    ReviewMode::Always => true,
                    ReviewMode::Never => false,
                    ReviewMode::IfCodeChanged => l.changed.iter().any(|f| !is_instruction_file(f)),
                };
                l.rationale = match &in_flight {
                    LoopNext::Work {
                        instructions: Some(i),
                        kind,
                    } if *kind != WorkKind::Initial => Some(i.clone()),
                    _ => None,
                };
                l.next = if review {
                    LoopNext::Review
                } else if l.stage && !l.changed.is_empty() && l.review != ReviewMode::IfCodeChanged
                {
                    LoopNext::Stage
                } else {
                    LoopNext::Done
                };
            }
        }
    }

    fn owner_of_exec_gate(&mut self, exec: &ExecutionId, gate: &GateId) {
        let Some(rec) = self.executions.get(exec).cloned() else {
            return;
        };
        if let Some(key) = rec.loop_key {
            if let Some(l) = self.loop_mut(key) {
                l.gate = Some(gate.clone());
            }
            return;
        }
        match &rec.purpose {
            ExecPurpose::Explore { task } => {
                if let Some(t) = self.explore.get_mut(*task as usize) {
                    t.gate = Some(gate.clone());
                }
            }
            ExecPurpose::Spec { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Spec,
                ..
            } => self.spec.failed_gate = Some(gate.clone()),
            ExecPurpose::Plan { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Plan,
                ..
            } => self.plan.failed_gate = Some(gate.clone()),
            ExecPurpose::Epa { phase } => {
                if let Some(p) = self.phases.get_mut(phase)
                    && let EpaState::Failed { gate: g, .. } = &mut p.epa
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::Docs { project, area } => {
                if let DocsState::Failed { gate: g, .. } = self
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_mut(area.as_deref())
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::Architecture => {
                if let ArchitectureState::Failed { gate: g, .. } = &mut self.architecture {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::Init { .. } => {
                if let Some(i) = self.init_track_mut(&rec.project) {
                    i.failed_gate = Some(gate.clone());
                }
            }
            _ => {}
        }
    }

    fn on_gate_opened(&mut self, id: &GateId, payload: &GatePayload) {
        match payload {
            GatePayload::OpenQuestions { artifact, .. } => {
                if artifact == "plan" {
                    self.plan.questions_gate = Some(id.clone());
                    self.plan.questions_asked_version = self.plan.version;
                } else {
                    self.spec.questions_gate = Some(id.clone());
                    self.spec.questions_asked_version = self.spec.version;
                }
            }
            GatePayload::SpecApproval { .. } => {
                self.spec.approval_gate = Some(id.clone());
                self.spec.approval_asked_version = self.spec.version;
            }
            GatePayload::PlanApproval { .. } => {
                self.plan.approval_gate = Some(id.clone());
                self.plan.approval_asked_version = self.plan.version;
            }
            GatePayload::FactCheckRecurring { target, .. } => match target {
                FactTarget::Spec => self.spec.recurring_gate = Some(id.clone()),
                FactTarget::Plan => self.plan.recurring_gate = Some(id.clone()),
            },
            GatePayload::ReviewCap { phase, tests, .. } => {
                if let Some(l) = self.loop_mut((*phase, *tests)) {
                    l.gate = Some(id.clone());
                }
            }
            GatePayload::PhaseBlocked { phase, .. } => {
                if let Some(p) = self.phases.get_mut(phase) {
                    p.blocked_gate = Some(id.clone());
                }
            }
            GatePayload::Stuck { execution, .. }
            | GatePayload::ExecutionFailed { execution, .. }
            | GatePayload::HarnessFailure { execution, .. } => {
                self.owner_of_exec_gate(execution, id)
            }
            GatePayload::ClosingGate { items } => {
                for item in items {
                    self.project_tracks
                        .entry(item.project.clone())
                        .or_default()
                        .closing_gate = Some(id.clone());
                }
            }
            GatePayload::SkillApproval { project, .. } => {
                if let Some(i) = self.init_track_mut(project) {
                    i.approval_gate = Some(id.clone());
                }
            }
            GatePayload::Permission { .. } => {}
            GatePayload::BudgetReached { .. } => self.budget_gate = Some(id.clone()),
            GatePayload::ImplementationReview { .. } => self.feedback.gate = Some(id.clone()),
        }
    }

    fn on_gate_answered(
        &mut self,
        id: &GateId,
        payload: &GatePayload,
        answer: &GateAnswer,
        routed: bool,
    ) {
        let choice = match answer {
            GateAnswer::Choice { option, text } => Some((
                option.as_str(),
                text.clone().filter(|t| !t.trim().is_empty()),
            )),
            _ => None,
        };
        match payload {
            GatePayload::OpenQuestions { artifact, .. } => {
                let answers = match answer {
                    GateAnswer::Questions { answers } => answers.clone(),
                    _ => vec![],
                };
                let target = if artifact == "plan" {
                    FactTarget::Plan
                } else {
                    FactTarget::Spec
                };
                if routed && !answers.is_empty() {
                    // Rule J1: the judge decides where each answer goes before any agent sees it.
                    let t = self.track_mut(target);
                    t.clear_questions_gate();
                    t.set_routing(Some(id.clone()));
                    self.held_answers
                        .insert(id.clone(), HeldAnswer::Questions { target, answers });
                    return;
                }
                if artifact == "plan" {
                    self.plan.questions_gate = None;
                    if !answers.is_empty() {
                        // After the plan exists, answers go into the spec first (answer routing).
                        let text = answers
                            .iter()
                            .map(|a| format!("{} {}\nAnswer: {}", a.id, a.question, a.answer))
                            .collect::<Vec<_>>()
                            .join("\n");
                        self.requirement_change(format!(
                            "Answers to the plan's clarifying questions:\n{text}"
                        ));
                    }
                } else {
                    self.spec.questions_gate = None;
                    // Rule D3: every answer re-runs generate-spec.
                    self.spec.answers.extend(answers);
                    self.spec.needs_run = true;
                }
            }
            GatePayload::SpecApproval { .. } => {
                // A change that landed after the gate opened revoked it; its answer is stale.
                let current = self.spec.approval_gate.as_ref() == Some(id) && !self.spec.needs_run;
                self.spec.approval_gate = None;
                if routed && let Some((approved, text)) = approval_text(answer) {
                    let approve = approved && current && self.spec.passed_current();
                    self.hold_approval(id, FactTarget::Spec, approve, text);
                    return;
                }
                match answer {
                    GateAnswer::Approval { approved: true, .. }
                        if current && self.spec.passed_current() =>
                    {
                        self.approve_spec()
                    }
                    GateAnswer::Approval {
                        feedback: Some(text),
                        ..
                    } if !text.trim().is_empty() => {
                        self.spec.changes.push(text.clone());
                        self.spec.needs_run = true;
                    }
                    _ => {}
                }
            }
            GatePayload::PlanApproval { .. } => {
                let current = self.plan.approval_gate.as_ref() == Some(id)
                    && !self.plan.needs_run
                    && !self.plan.invalidated;
                self.plan.approval_gate = None;
                if routed && let Some((approved, text)) = approval_text(answer) {
                    let approve = approved && current && self.plan.passed_current();
                    self.hold_approval(id, FactTarget::Plan, approve, text);
                    return;
                }
                match answer {
                    GateAnswer::Approval { approved: true, .. }
                        if current && self.plan.passed_current() =>
                    {
                        self.plan.approved = true;
                        self.plan.approved_version = self.plan.version;
                        self.adopt_plan_phases();
                    }
                    GateAnswer::Approval {
                        feedback: Some(text),
                        ..
                    } if !text.trim().is_empty() => {
                        // Rule D10: a change after the plan exists goes into the spec first.
                        self.requirement_change(text.clone());
                    }
                    _ => {}
                }
            }
            GatePayload::FactCheckRecurring { target, .. } => {
                let track_changes: Option<String>;
                {
                    let t_spec;
                    let track: &mut dyn RecurringTrack = match target {
                        FactTarget::Spec => {
                            t_spec = &mut self.spec;
                            t_spec
                        }
                        FactTarget::Plan => &mut self.plan,
                    };
                    track.clear_recurring_gate();
                    track_changes = match choice {
                        Some(("stop", _)) => {
                            track.stop();
                            None
                        }
                        Some((_, text)) => {
                            track.allow_more();
                            text
                        }
                        None => None,
                    };
                }
                if let Some(text) = track_changes {
                    if routed {
                        self.track_mut(*target).set_routing(Some(id.clone()));
                        self.held_answers.insert(
                            id.clone(),
                            HeldAnswer::Recurring {
                                target: *target,
                                text,
                            },
                        );
                        return;
                    }
                    match target {
                        FactTarget::Spec => {
                            self.spec.changes.push(text);
                            self.spec.needs_run = true;
                        }
                        FactTarget::Plan => self.requirement_change(text),
                    }
                }
            }
            GatePayload::ReviewCap {
                phase,
                tests,
                findings,
                ..
            } => {
                let project = self
                    .phases
                    .get(phase)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let ledger = self
                    .ledger_path(&project, *phase, *tests)
                    .display()
                    .to_string();
                if let Some(l) = self.loop_mut((*phase, *tests)) {
                    l.gate = None;
                    match choice {
                        Some(("another-pass", text)) => {
                            l.extra_cap += 1;
                            let mut instr = fix_instructions(findings, &ledger);
                            match text {
                                Some(t) if routed => {
                                    l.next = LoopNext::AwaitRoute {
                                        gate: id.clone(),
                                        text: t,
                                        then: WorkKind::Fix,
                                        base: Some(instr.clone()),
                                        stuck: None,
                                        fallback: Box::new(LoopNext::Work {
                                            kind: WorkKind::Fix,
                                            instructions: Some(instr),
                                        }),
                                    };
                                }
                                text => {
                                    if let Some(t) = text {
                                        instr.push_str(&format!("\n\nThe user added: {t}"));
                                    }
                                    l.next = LoopNext::Work {
                                        kind: WorkKind::Fix,
                                        instructions: Some(instr),
                                    };
                                }
                            }
                        }
                        _ => {
                            l.next = LoopNext::Blocked {
                                reason: format!(
                                    "The review loop reached its cap with {} findings open. Ledger: {ledger}",
                                    findings.len()
                                ),
                            };
                            l.announced_block = false;
                            l.block_gate_answered = true;
                        }
                    }
                }
            }
            GatePayload::Stuck { execution, .. } => {
                let key = self.executions.get(execution).and_then(|r| r.loop_key);
                if let Some(key) = key
                    && let Some(l) = self.loop_mut(key)
                {
                    l.gate = None;
                    if let LoopNext::RescueGate { exec, stuck } = l.next.clone() {
                        match choice {
                            // Rule O8: the user's words are the implementer's task, not a fact
                            // for the stuck agent, so they skip the Route answer judge.
                            Some(("fix", text)) => {
                                l.next = LoopNext::RescueFix {
                                    exec,
                                    stuck,
                                    instructions: text,
                                    fixer: None,
                                };
                            }
                            Some(("fact", Some(text))) => {
                                l.next = LoopNext::AwaitRoute {
                                    gate: id.clone(),
                                    text,
                                    then: WorkKind::Rescue,
                                    base: None,
                                    fallback: Box::new(LoopNext::Blocked {
                                        reason: format!("Stuck: {}", stuck.need),
                                    }),
                                    stuck: Some(stuck),
                                }
                            }
                            _ => {
                                l.next = LoopNext::Blocked {
                                    reason: format!("Stuck: {}", stuck.need),
                                };
                                l.block_gate_answered = true;
                            }
                        }
                    }
                }
            }
            GatePayload::PhaseBlocked { phase, .. } => {
                let tests = self
                    .phases
                    .get(phase)
                    .is_some_and(|p| p.test_loop.is_blocked() && !p.impl_loop.is_blocked());
                if let Some(p) = self.phases.get_mut(phase) {
                    p.blocked_gate = None;
                }
                if let Some(l) = self.loop_mut((*phase, tests)) {
                    l.block_gate_answered = true;
                    if let Some(("retry", text)) = choice {
                        let iterations = l.iterations;
                        l.extra_cap = (iterations + REVIEW_CAP).saturating_sub(REVIEW_CAP);
                        l.announced_block = false;
                        l.block_gate_answered = false;
                        l.resolve_rounds = 0;
                        l.open_before_resolve = None;
                        let plain = LoopNext::Work {
                            kind: WorkKind::Fix,
                            instructions: l.last_review.as_ref().map(|r| {
                                let hm: Vec<ReviewFinding> = r
                                    .findings
                                    .iter()
                                    .filter(|f| {
                                        matches!(
                                            f.severity,
                                            Severity::High | Severity::Medium | Severity::Blocker
                                        )
                                    })
                                    .cloned()
                                    .collect();
                                fix_instructions(&hm, "the review ledger")
                            }),
                        };
                        l.next = match text {
                            Some(t) => LoopNext::AwaitRoute {
                                gate: id.clone(),
                                text: t,
                                then: WorkKind::Fix,
                                base: None,
                                stuck: None,
                                fallback: Box::new(plain),
                            },
                            None => plain,
                        };
                    }
                }
            }
            GatePayload::ClosingGate { items } => {
                let choices = match answer {
                    GateAnswer::Closing { items } => items.clone(),
                    _ => vec![],
                };
                let (opt_tests, opt_docs) = (self.tests_requested(), self.docs_requested());
                for item in items {
                    let c = choices.iter().find(|c| c.project == item.project);
                    let tests = if item.ask_tests {
                        c.is_some_and(|c| c.tests)
                    } else {
                        opt_tests
                    };
                    let docs = if item.ask_docs {
                        c.is_some_and(|c| c.docs)
                    } else {
                        opt_docs
                    };
                    let t = self.project_tracks.entry(item.project.clone()).or_default();
                    t.closing = Some((tests, docs));
                    t.closing_gate = None;
                }
            }
            GatePayload::ExecutionFailed { execution, .. }
            | GatePayload::HarnessFailure { execution, .. } => {
                if let GatePayload::HarnessFailure { .. } = payload
                    && let Some(("native", _)) = choice
                    && let Some(r) = self.executions.get(execution)
                {
                    self.native_fallback.insert(r.agent);
                }
                let retry = matches!(choice, Some(("retry" | "native", _)));
                self.exec_gate_answered(execution, retry);
            }
            GatePayload::SkillApproval { project, .. } => {
                if let (Some(i), GateAnswer::Skills { decisions }) =
                    (self.init_track_mut(project), answer)
                {
                    i.approval_gate = None;
                    i.decisions = Some(
                        decisions
                            .iter()
                            .map(|d| (d.name.clone(), d.disposition.clone()))
                            .collect(),
                    );
                }
            }
            GatePayload::Permission { .. } => {}
            GatePayload::ImplementationReview { .. } => {
                if self.feedback.gate.as_ref() != Some(id) {
                    return;
                }
                self.feedback.gate = None;
                match choice {
                    Some(("feedback", Some(text))) => self.add_feedback(text, routed),
                    _ => self.feedback.accepted = true,
                }
            }
            GatePayload::BudgetReached {
                spent_usd,
                budget_usd,
            } => {
                self.budget_gate = None;
                match choice {
                    Some(("raise", text)) => {
                        // The default raise is the budget again, so the session can spend as much
                        // once more before the next pause.
                        let extra = text
                            .and_then(|t| t.trim().trim_start_matches('$').parse::<f64>().ok())
                            .filter(|v| v.is_finite() && *v > 0.0)
                            .unwrap_or(budget_usd.max(1.0));
                        self.budget_raised += extra + (spent_usd - budget_usd).max(0.0);
                    }
                    _ => {
                        self.failed = Some(format!(
                            "Stopped at the session budget after spending ${spent_usd:.2}."
                        ))
                    }
                }
            }
        }
    }

    fn exec_gate_answered(&mut self, execution: &ExecutionId, retry: bool) {
        let Some(rec) = self.executions.get(execution).cloned() else {
            return;
        };
        if let Some(key) = rec.loop_key {
            if let Some(l) = self.loop_mut(key) {
                l.gate = None;
                if let LoopNext::Failed { .. } = l.next {
                    l.next = if retry {
                        l.error_retries = 0;
                        match rec.purpose {
                            ExecPurpose::Review { .. } => LoopNext::Review,
                            _ => LoopNext::Work {
                                kind: WorkKind::Rerun,
                                instructions: l.rationale.clone(),
                            },
                        }
                    } else {
                        LoopNext::Blocked {
                            reason: format!("{} failed and the user abandoned it.", rec.agent),
                        }
                    };
                }
            }
            return;
        }
        match &rec.purpose {
            ExecPurpose::Explore { task } => {
                if let Some(t) = self.explore.get_mut(*task as usize) {
                    t.gate = None;
                    if retry {
                        t.failed = None;
                        t.exec = None;
                        t.retries = 0;
                    } else {
                        t.abandoned = true;
                    }
                }
                if let Some(ExploreOrigin::LoopAnswer { phase, tests }) =
                    self.explore.get(*task as usize).map(|t| t.origin.clone())
                {
                    self.release_answer_research((phase, tests));
                }
            }
            ExecPurpose::Spec { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Spec,
                ..
            } => exec_retry(
                &mut self.spec,
                retry,
                matches!(rec.purpose, ExecPurpose::Spec { .. }),
            ),
            ExecPurpose::Plan { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Plan,
                ..
            } => exec_retry(
                &mut self.plan,
                retry,
                matches!(rec.purpose, ExecPurpose::Plan { .. }),
            ),
            ExecPurpose::Epa { phase } => {
                if let Some(p) = self.phases.get_mut(phase) {
                    p.epa = if retry {
                        EpaState::NotStarted
                    } else {
                        EpaState::Abandoned
                    };
                }
            }
            ExecPurpose::Docs { project, area } => {
                *self
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_mut(area.as_deref()) = if retry {
                    DocsState::NotStarted
                } else {
                    DocsState::Abandoned
                };
            }
            ExecPurpose::Architecture => {
                self.architecture = if retry {
                    ArchitectureState::NotStarted
                } else {
                    ArchitectureState::Abandoned
                };
            }
            ExecPurpose::QuickAnswer => {
                if retry {
                    self.quick.failed = None;
                    self.quick.exec = None;
                }
            }
            ExecPurpose::Init { .. } => {
                let embedded = self.project_inits.contains_key(&rec.project);
                let mut abandoned = false;
                if let Some(i) = self.init_track_mut(&rec.project) {
                    i.failed_gate = None;
                    if retry {
                        i.escalated = false;
                        let key = i.failed.take();
                        if let Some((exec, _)) = key {
                            reset_init_item(i, &exec);
                        }
                    } else if embedded {
                        // Rule O4: abandoning a created project's init lets its phases run
                        // without it, because the rest of the session still needs them.
                        i.finished = true;
                        i.note = Some(
                            "The user abandoned the init after an initializer step failed.".into(),
                        );
                    } else {
                        abandoned = true;
                    }
                }
                if abandoned {
                    self.failed =
                        Some("The init was abandoned after an initializer step failed.".into());
                }
            }
            _ => {}
        }
    }

    /// Replace the phase set with the approved plan's phases (Rules D6, D7).
    fn adopt_plan_phases(&mut self) {
        let Some(plan) = self.plan.current.clone() else {
            return;
        };
        let old = std::mem::take(&mut self.phases);
        self.superseded_phases
            .extend(old.into_values().filter(|p| p.impl_loop.work_count > 0));
        for p in &plan.phases {
            let complexity = p
                .complexity
                .parse::<Complexity>()
                .unwrap_or(Complexity::Medium);
            let test_policy = if p.test_policy.trim().eq_ignore_ascii_case("skip") {
                TestPolicy::Skip
            } else {
                TestPolicy::Required
            };
            // Rule M5: a dependency on a phase that does not exist is unreadable, so it depends
            // on every earlier phase.
            let ids: BTreeSet<u32> = plan.phases.iter().map(|x| x.id).collect();
            let depends_on = if p.depends_on.iter().all(|d| ids.contains(d) && *d != p.id) {
                Some(p.depends_on.clone())
            } else {
                None
            };
            let info = PhaseInfo {
                id: p.id,
                deliverable: Some(p.deliverable.clone()).filter(|d| !d.is_empty()),
                project: p.project.clone(),
                title: p.title.clone(),
                complexity,
                test_policy,
                depends_on,
                file: Some(PathBuf::from(&p.file)),
                test_rationale: p.test_rationale.clone(),
            };
            let valid = self.valid_project(&info.project)
                || self.project_to_create(&info.project).is_some();
            self.insert_phase(
                info,
                WorkLoop::new(false, AgentName::Implementer, AgentName::Implementer),
            );
            if !valid && let Some(ph) = self.phases.get_mut(&p.id) {
                ph.impl_loop.next = LoopNext::Blocked {
                    reason: format!(
                        "The plan names project `{}`, which is not in this workspace.",
                        ph.info.project
                    ),
                };
            }
        }
        self.project_tracks
            .retain(|k, _| self.phases.values().any(|p| &p.info.project == k));
    }

    // -----------------------------------------------------------------------------------------
    // Init flow
    // -----------------------------------------------------------------------------------------

    /// The init track an initializer execution belongs to: the init session's own, or a created
    /// project's (Rule O4).
    fn init_track_mut(&mut self, project: &str) -> Option<&mut InitTrack> {
        if self.init.as_ref().is_some_and(|i| i.project == project) {
            return self.init.as_mut();
        }
        self.project_inits.get_mut(project)
    }

    fn exec_project(&self, id: &ExecutionId) -> String {
        self.executions
            .get(id)
            .map(|r| r.project.clone())
            .unwrap_or_default()
    }

    fn init_started(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
    ) {
        use ostra_core::InitializerMode as M;
        let project = self.exec_project(id);
        let Some(i) = self.init_track_mut(&project) else {
            return;
        };
        match mode {
            M::Detect => i.detect = Some(id.clone()),
            M::Adopt => i.adopt = Some(id.clone()),
            M::Propose => i.propose = Some(id.clone()),
            M::GenerateInventory => i.inventory = Some(id.clone()),
            M::Scout | M::GenerateSkill => {
                let key = item.unwrap_or_default();
                let list = if mode == M::Scout {
                    &mut i.scouts
                } else {
                    &mut i.generates
                };
                match list.iter_mut().find(|x| x.key == key) {
                    Some(x) => x.exec = Some(id.clone()),
                    None => list.push(InitItem {
                        key,
                        exec: Some(id.clone()),
                        result: None,
                    }),
                }
            }
        }
    }

    fn init_finished(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    ) {
        use ostra_core::InitializerMode as M;
        let parsed: Option<InitializerSubmit> = parse(&result.submit);
        let project = self.exec_project(id);
        let Some(i) = self.init_track_mut(&project) else {
            return;
        };
        let ok = status == ExecutionStatus::Ok
            && parsed
                .as_ref()
                .is_some_and(|p| p.status == SubmitStatus::Ok);
        if !ok {
            let retry_key = format!("{mode}:{}", item.clone().unwrap_or_default());
            let retries = i.retries.entry(retry_key).or_insert(0);
            if (status == ExecutionStatus::Interrupted)
                || (status == ExecutionStatus::Error && *retries < ERROR_RETRIES)
            {
                if status == ExecutionStatus::Error {
                    *retries += 1;
                }
                reset_init_item(i, id);
            } else {
                let msg = match parsed {
                    Some(p) if p.status == SubmitStatus::Stuck => {
                        stuck_problem(&p.summary, &p.stuck.map(|s| s.need).unwrap_or_default())
                    }
                    _ => missing_submit(status, &error, result),
                };
                i.failed = Some((id.clone(), msg));
            }
            return;
        }
        // Models sometimes send the result object JSON-encoded as a string.
        let value = match parsed.map(|p| p.result).unwrap_or(Value::Null) {
            Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
            v => v,
        };
        match mode {
            M::Detect => i.detect_result = Some(value),
            M::Adopt => i.adopt_result = Some(value),
            M::Propose => i.propose_result = Some(value),
            M::GenerateInventory => i.inventory_result = Some(value),
            M::Scout | M::GenerateSkill => {
                let list = if mode == M::Scout {
                    &mut i.scouts
                } else {
                    &mut i.generates
                };
                if let Some(x) = list.iter_mut().find(|x| x.exec.as_ref() == Some(id)) {
                    x.result = Some(value);
                }
            }
        }
    }
}

impl SessionState {
    /// Rule O5: the step key an initializer execution's advice is counted under.
    pub fn init_step_key(&self, exec: &ExecutionId) -> String {
        match self.executions.get(exec).map(|r| &r.purpose) {
            Some(ExecPurpose::Init { mode, item }) => {
                format!("{mode}:{}", item.clone().unwrap_or_default())
            }
            _ => String::new(),
        }
    }

    /// The work loop waiting on the advisor's look at its stuck run `exec` (Rule O7).
    fn stuck_loop_mut(&mut self, exec: &ExecutionId) -> Option<&mut WorkLoop> {
        let key = self.executions.get(exec).and_then(|r| r.loop_key)?;
        self.loop_mut(key)
            .filter(|l| matches!(&l.next, LoopNext::RescueAdvise { exec: e, .. } if e == exec))
    }

    /// The work loop whose stuck run `exec` an implementer the user sent is fixing (Rule O8).
    fn fixing_loop_mut(&mut self, exec: &ExecutionId) -> Option<&mut WorkLoop> {
        let key = self.executions.get(exec).and_then(|r| r.loop_key)?;
        self.loop_mut(key)
            .filter(|l| matches!(&l.next, LoopNext::RescueFix { exec: e, .. } if e == exec))
    }

    /// Rule O8: a fix that finished sends the stuck run back to work with what changed; one that
    /// did not reopens the stuck gate with the reason.
    fn loop_fix_finished(
        &mut self,
        stuck_exec: &ExecutionId,
        fixer_exec: &ExecutionId,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    ) {
        let parsed: Option<ImplementerSubmit> = parse(&result.submit);
        let Some(l) = self.fixing_loop_mut(stuck_exec) else {
            return;
        };
        let LoopNext::RescueFix {
            exec,
            stuck,
            instructions,
            fixer,
        } = l.next.clone()
        else {
            return;
        };
        if fixer.as_ref() != Some(fixer_exec) {
            return;
        }
        if status == ExecutionStatus::Interrupted {
            l.next = LoopNext::RescueFix {
                exec,
                stuck,
                instructions,
                fixer: None,
            };
            return;
        }
        l.next = match parsed {
            Some(f) if status == ExecutionStatus::Ok && f.status == SubmitStatus::Ok => {
                l.changed.extend(f.changed_files.iter().cloned());
                let asked = instructions
                    .map(|i| format!("The user asked it to: {i}\n"))
                    .unwrap_or_default();
                let files = if f.changed_files.is_empty() {
                    "none".to_string()
                } else {
                    f.changed_files.join(", ")
                };
                let fact = format!(
                    "The user sent another implementer to fix what stopped you, and it finished.\n{asked}Its summary: {}\nFiles it changed: {files}\nIts report: {}\nContinue your work from where you stopped.",
                    f.summary, f.report_path
                );
                LoopNext::Work {
                    kind: WorkKind::Rescue,
                    instructions: Some(rescue_context(&stuck, &fact)),
                }
            }
            other => {
                let why = match other {
                    Some(f) => format!("The implementer you sent did not fix it: {}", f.summary),
                    None => format!("The implementer you sent did not finish: {error}"),
                };
                LoopNext::RescueGate {
                    exec,
                    stuck: StuckInfo {
                        diagnostic: stuck.diagnostic,
                        need: format!("{}\n\n{why}", stuck.need),
                    },
                }
            }
        };
    }

    fn loop_advice_finished(
        &mut self,
        failed: &ExecutionId,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    ) {
        let parsed: Option<ostra_core::submit::AdvisorSubmit> = parse(&result.submit);
        let Some(l) = self.stuck_loop_mut(failed) else {
            return;
        };
        let LoopNext::RescueAdvise { exec, stuck, .. } = l.next.clone() else {
            return;
        };
        if status == ExecutionStatus::Interrupted {
            l.next = LoopNext::RescueAdvise {
                exec,
                stuck,
                advisor: None,
            };
            return;
        }
        l.next = match parsed {
            Some(a)
                if status == ExecutionStatus::Ok
                    && a.action == ostra_core::submit::AdviceAction::Retry =>
            {
                l.advice.push(a.guidance.clone());
                let fact = format!(
                    "An advisor looked at this failure and says how to get past it:\n{}",
                    a.guidance
                );
                LoopNext::Work {
                    kind: WorkKind::Rescue,
                    instructions: Some(rescue_context(&stuck, &fact)),
                }
            }
            other => {
                let why = match other {
                    Some(a) => format!("Advisor: {}", a.reason),
                    None => format!("The advisor could not help: {error}"),
                };
                LoopNext::RescueGate {
                    exec,
                    stuck: StuckInfo {
                        diagnostic: stuck.diagnostic,
                        need: format!("{}\n\n{why}", stuck.need),
                    },
                }
            }
        };
    }

    fn advice_finished(
        &mut self,
        project: &str,
        failed: &ExecutionId,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    ) {
        let key = self.init_step_key(failed);
        let parsed: Option<ostra_core::submit::AdvisorSubmit> = parse(&result.submit);
        let Some(i) = self.project_inits.get_mut(project) else {
            return;
        };
        i.advising = None;
        if status == ExecutionStatus::Interrupted {
            return;
        }
        match parsed {
            Some(a)
                if status == ExecutionStatus::Ok
                    && a.action == ostra_core::submit::AdviceAction::Retry =>
            {
                i.advice.entry(key).or_default().push(a.guidance);
                clear_init_result(i, failed);
                reset_init_item(i, failed);
                i.failed = None;
            }
            other => {
                i.escalated = true;
                let why = match other {
                    Some(a) => format!("Advisor: {}", a.reason),
                    None => format!("The advisor could not help: {error}"),
                };
                if let Some((_, msg)) = i.failed.as_mut() {
                    msg.push_str("\n\n");
                    msg.push_str(&why);
                }
            }
        }
    }
}

/// Drop the result of the step `exec` ran, so the step runs again.
fn clear_init_result(i: &mut InitTrack, exec: &ExecutionId) {
    if i.detect.as_ref() == Some(exec) {
        i.detect_result = None;
    } else if i.propose.as_ref() == Some(exec) {
        i.propose_result = None;
    } else if i.inventory.as_ref() == Some(exec) {
        i.inventory_result = None;
    } else {
        for x in i.scouts.iter_mut().chain(i.generates.iter_mut()) {
            if x.exec.as_ref() == Some(exec) {
                x.result = None;
            }
        }
    }
}

fn reset_init_item(i: &mut InitTrack, exec: &ExecutionId) {
    if i.detect.as_ref() == Some(exec) {
        i.detect = None;
    } else if i.adopt.as_ref() == Some(exec) {
        i.adopt = None;
    } else if i.propose.as_ref() == Some(exec) {
        i.propose = None;
    } else if i.inventory.as_ref() == Some(exec) {
        i.inventory = None;
    } else {
        for x in i.scouts.iter_mut().chain(i.generates.iter_mut()) {
            if x.exec.as_ref() == Some(exec) {
                x.exec = None;
            }
        }
    }
}

trait RecurringTrack {
    fn clear_recurring_gate(&mut self);
    fn stop(&mut self);
    fn allow_more(&mut self);
}

impl<T> RecurringTrack for ArtifactTrack<T> {
    fn clear_recurring_gate(&mut self) {
        self.recurring_gate = None;
    }
    fn stop(&mut self) {
        self.stopped = true;
    }
    fn allow_more(&mut self) {
        self.fail_limit_extra = self.consecutive_fails;
    }
}

fn artifact_error<T>(t: &mut ArtifactTrack<T>, status: ExecutionStatus, msg: String) {
    if status == ExecutionStatus::Error && t.error_retries < ERROR_RETRIES {
        t.error_retries += 1;
        t.needs_run = true;
    } else {
        t.failed = Some(msg);
    }
}

fn exec_retry<T>(t: &mut ArtifactTrack<T>, retry: bool, generation: bool) {
    t.failed_gate = None;
    if retry {
        t.failed = None;
        t.error_retries = 0;
        if generation {
            t.needs_run = true;
        }
    } else {
        t.stopped = true;
    }
}

fn fact_finished<T>(
    t: &mut ArtifactTrack<T>,
    exec: &ExecutionId,
    status: ExecutionStatus,
    result: Option<FactCheckSubmit>,
    failed_msg: String,
) {
    t.check_running = None;
    let valid = result.filter(|r| status == ExecutionStatus::Ok && r.verdict != Verdict::Error);
    match valid {
        Some(r) => {
            if let Some(pass) = t.checks.iter_mut().find(|c| &c.exec == exec) {
                pass.result = Some(r.clone());
            }
            t.error_retries = 0;
            match r.verdict {
                Verdict::Pass => t.consecutive_fails = 0,
                _ => {
                    // FAIL goes back to the owning agent with the findings (HANDOVER 8.1).
                    t.consecutive_fails += 1;
                    t.pending_findings = Some(r.findings_text());
                    t.needs_run = true;
                }
            }
        }
        None => {
            t.checks.retain(|c| &c.exec != exec);
            if status == ExecutionStatus::Interrupted {
                return;
            }
            if status == ExecutionStatus::Error && t.error_retries < ERROR_RETRIES {
                t.error_retries += 1;
            } else {
                t.failed = Some(failed_msg);
            }
        }
    }
}

/// What a stuck init step's failure says: its summary, then the fact or decision it needs.
pub fn stuck_problem(summary: &str, need: &str) -> String {
    let summary = summary.trim().trim_end_matches('.');
    let need = need.trim();
    if need.is_empty() {
        format!("{summary}.")
    } else {
        format!("{summary}. It needs: {need}")
    }
}

fn exec_error(result: &ExecutionResult) -> String {
    match (&result.error, result.status) {
        (Some(e), _) => e.clone(),
        (None, ExecutionStatus::Cancelled) => STOPPED_EXECUTION.into(),
        (None, status) => format!("execution ended with status {status:?}"),
    }
}

pub const STOPPED_EXECUTION: &str = "You stopped this execution.";

/// The Route answer judge's subject for the amendment at `i` (Rule C2).
pub fn amendment_subject(i: usize) -> String {
    format!("amendment:{i}")
}

pub fn amendment_index(subject: &str) -> Option<usize> {
    subject.strip_prefix("amendment:")?.parse().ok()
}

fn missing_submit(status: ExecutionStatus, error: &str, result: &ExecutionResult) -> String {
    match status {
        ExecutionStatus::Ok if result.submit.is_none() => {
            "The agent finished without calling its submit tool, so Ostra has no result to read."
                .into()
        }
        ExecutionStatus::Ok => "The agent's submit payload did not match its schema.".into(),
        _ => error.to_string(),
    }
}

/// Rescue context: the diagnostic verbatim plus the stated fact (Rule D9, STUCK handling).
pub fn rescue_context(stuck: &StuckInfo, fact: &str) -> String {
    format!(
        "Your previous attempt stopped with STUCK. Its diagnostic, verbatim:\n{}\nIt needed: {}\nWhat changed since then: {}\nUse this fact. Do not repeat the approach that failed.",
        stuck.diagnostic, stuck.need, fact
    )
}

pub fn loop_key_str(key: (u32, bool)) -> String {
    if key.1 {
        format!("phase:{}:tests", key.0)
    } else {
        format!("phase:{}", key.0)
    }
}

pub fn parse_loop_key(s: &str) -> Option<(u32, bool)> {
    let rest = s.strip_prefix("phase:")?;
    match rest.strip_suffix(":tests") {
        Some(n) => Some((n.parse().ok()?, true)),
        None => Some((rest.parse().ok()?, false)),
    }
}

fn inline_phase(id: u32, project: &str, title: &str, index: usize) -> PhaseInfo {
    PhaseInfo {
        id,
        deliverable: None,
        project: project.to_string(),
        title: title.to_string(),
        complexity: Complexity::Low,
        test_policy: TestPolicy::Required,
        depends_on: Some(if index == 0 { vec![] } else { vec![id - 1] }),
        file: None,
        test_rationale: None,
    }
}
