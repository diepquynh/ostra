//! The built-in stages' own state: what research, the spec, the plan, the phases, the closing
//! stages, the book, and the init flow hold for one session. The engine keeps it in
//! `SessionState::ext` and never reads it; this crate folds and reads it.

use ostra_core::agent::AgentName;
use ostra_core::book::DocumentationSubmit;
use ostra_core::event::{CommandPurpose, FactTarget, WorkKind};
use ostra_core::ids::{DecisionId, ExecutionId, GateId};
use ostra_core::pipeline::{PhaseInfo, QuestionAnswer, Stakes, Track};
use ostra_core::submit::{
    CodeReviewerSubmit, ExploreSubmit, FactCheckSubmit, GenerateSpecSubmit, HandoffInfo,
    PlanSubmit, QuickAnswerSubmit, ReviewFinding, StuckInfo, Verdict,
};
use ostra_engine::pipeline::PipelineBox;
use ostra_engine::state::SessionState;
use serde::{Deserialize, Serialize};
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
    /// Rule SM7: the research helper a message started; else the research stage's bound agent.
    pub agent: Option<AgentName>,
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

    pub fn start_check(&mut self, id: &ExecutionId) {
        self.check_running = Some(id.clone());
        self.checks.push(FactCheckPass {
            exec: id.clone(),
            version: self.version,
            result: None,
        });
    }

    pub fn revoke_approval(&mut self) {
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
    /// Rule CA5: the contract of its work runs and of its fix runs; the planner binds an agent to
    /// each through the workflow (Rule WF8).
    pub work: ostra_core::Contract,
    pub fix: ostra_core::Contract,
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
    pub fn new(tests: bool, work: ostra_core::Contract, fix: ostra_core::Contract) -> Self {
        WorkLoop {
            tests,
            work,
            fix,
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

/// Rule B10: a fact-check of one draft page.
pub type CheckState = StageRun<FactCheckSubmit>;

/// Rule B10: one planned page: its latest draft and the run that wrote its first draft.
#[derive(Debug, Clone, Default)]
pub struct PageDraft {
    pub run: DocsState,
    pub draft: Option<ostra_core::book::DocSection>,
    /// The draft before the last revision, so a fact-check re-pass checks only what changed.
    pub previous: Option<ostra_core::book::DocSection>,
    pub glossary: Vec<ostra_core::book::GlossaryEntry>,
}

impl Default for DocsState {
    fn default() -> Self {
        DocsState::NotStarted
    }
}

/// Rule B10: what the scan before the survey found on disk.
#[derive(Debug, Clone, Default)]
pub struct DocsScan {
    pub modules: Vec<ostra_core::book::DocsModule>,
    pub refs: Vec<ostra_core::book::RefItem>,
}

/// Rule B10: one synthesis round: a fact-check per changed page, one synthesis pass, then a
/// revision per page that the pass, a failed check, or an engine check names.
#[derive(Debug, Clone, Default)]
pub struct DocsRound {
    pub checks: BTreeMap<String, CheckState>,
    pub synthesis: DocsState,
    /// The revision instructions for each page, fixed when the synthesis pass finishes.
    pub targets: BTreeMap<String, Vec<String>>,
    pub revisions: BTreeMap<String, DocsState>,
}

#[derive(Debug, Clone)]
pub struct ProjectTrack {
    /// `Some(exit code)` once format ran. `Some(None)` means no format command.
    pub format: Option<Option<i32>>,
    /// The command running now, between `CommandStarted` and `CommandRan`.
    pub running: Option<(CommandPurpose, String)>,
    pub closing_gate: Option<GateId>,
    pub closing: Option<(bool, bool)>,
    /// The writer of a whole part, from a log written before topics (Rule B10).
    pub docs: DocsState,
    /// Rule B10: the modules and constants the scan found, before the survey.
    pub docs_scan: Option<DocsScan>,
    /// Rule B10: the survey of what is available, and the page plan.
    pub survey: DocsState,
    /// Rule B10: inventory items the synthesis passes added, latest last.
    pub inventory_added: Vec<ostra_core::book::InventoryItem>,
    /// Rule B10: each planned page, by page ID.
    pub page_docs: BTreeMap<String, PageDraft>,
    /// Rule B10: the synthesis rounds, oldest first.
    pub docs_rounds: Vec<DocsRound>,
    /// Rule B10: the open gate after a multiple of `DOCS_ROUNDS` rounds, the round the user last
    /// chose another round at, and whether the user accepted the book as it is.
    pub docs_gate: Option<GateId>,
    pub docs_continued: u32,
    pub docs_accepted: bool,
}

impl Default for ProjectTrack {
    fn default() -> Self {
        ProjectTrack {
            format: None,
            running: None,
            closing_gate: None,
            closing: None,
            docs: DocsState::NotStarted,
            docs_scan: None,
            survey: DocsState::NotStarted,
            inventory_added: vec![],
            page_docs: BTreeMap::new(),
            docs_rounds: vec![],
            docs_gate: None,
            docs_continued: 0,
            docs_accepted: false,
        }
    }
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

/// The built-in stages' part of one session's state.
#[derive(Debug, Clone)]
pub struct OstraState {
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
    pub quick: QuickTrack,
    pub init: Option<InitTrack>,
    /// Rule O4: the init of each created project, run when the build starts.
    pub project_inits: BTreeMap<String, InitTrack>,
    pub prompt_gens: u32,
    /// Rule J1: spec and plan answers waiting for the Route answer judge.
    pub held_answers: BTreeMap<GateId, HeldAnswer>,
    /// Rule J1: answers the judge kept for later stages.
    pub user_notes: Vec<UserNote>,
}

impl Default for OstraState {
    fn default() -> Self {
        OstraState {
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
            quick: QuickTrack::default(),
            init: None,
            project_inits: BTreeMap::new(),
            prompt_gens: 0,
            held_answers: BTreeMap::new(),
            user_notes: vec![],
        }
    }
}

/// The built-in stages' state inside a session's `ext`.
pub trait OsExt {
    fn os(&self) -> &OstraState;
    fn os_mut(&mut self) -> &mut OstraState;
}

impl OsExt for PipelineBox {
    fn os(&self) -> &OstraState {
        self.get()
    }
    fn os_mut(&mut self) -> &mut OstraState {
        self.get_mut()
    }
}

/// Facts of the built-in state that many parts of this crate read.
pub trait OstraData {
    fn research_docs(&self) -> Vec<PathBuf>;
    fn push_explore(&mut self, project: String, task: String, origin: ExploreOrigin) -> u32;
}

impl OstraData for SessionState {
    /// Research documents, oldest run first, including superseded ones (Rule D2).
    fn research_docs(&self) -> Vec<PathBuf> {
        self.ext
            .os()
            .explore
            .iter()
            .filter_map(|t| t.result.as_ref().map(|r| PathBuf::from(&r.research_path)))
            .collect()
    }

    fn push_explore(&mut self, project: String, task: String, origin: ExploreOrigin) -> u32 {
        let explore = &mut self.ext.os_mut().explore;
        let idx = explore.len() as u32;
        explore.push(ExploreTask {
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
            agent: None,
        });
        idx
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OptsIn {
    #[serde(default)]
    pub tests: bool,
    #[serde(default)]
    pub docs: bool,
}

/// One project a feedback round changes, with the instruction its revision phase gets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackTarget {
    pub project: String,
    pub instruction: String,
}

/// A later stage a remembered note reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteStage {
    Implement,
    Tests,
    Docs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerRoute {
    RequirementChange,
    ImplementationDetail,
    StageChoice,
}
