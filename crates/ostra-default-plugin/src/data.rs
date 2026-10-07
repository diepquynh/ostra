//! The built-in stages' own state: what research, the spec, the plan, the phases, the closing
//! stages, the book, and the init flow hold for one session. The engine keeps it in
//! `SessionState::ext` and never reads it; this crate folds and reads it.

use ostra_core::event::{CommandPurpose, FactTarget};
use ostra_core::ids::{DecisionId, ExecutionId, GateId};
use ostra_core::pipeline::{PhaseInfo, QuestionAnswer, Stakes, Track};
use ostra_core::submit::{FactCheckSubmit, GenerateSpecSubmit, PlanSubmit, Verdict};
use ostra_engine::pipeline::PipelineBox;
use ostra_engine::state::SessionState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub use crate::stages::book::data::*;
pub use crate::stages::build::data::*;
pub use crate::stages::closing::data::*;
pub use crate::stages::feedback::data::*;
pub use crate::stages::init::data::*;
pub use crate::stages::quick::data::*;
pub use crate::stages::research::data::*;

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
/// Rule WD3: key in a plan execution's params holding the `(agent, executor)` pairs that a
/// multi-project phase would run and that work in one project.
pub const SINGLE_PROJECT_PARAM: &str = "single_project_agents";
/// `Prior findings:` after a pass that found nothing: a re-pass, not a first pass (Rule D3a).
pub const NO_PRIOR_FINDINGS: &str = "no findings on the previous pass";

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

#[derive(Debug, Clone, Default)]
pub struct ProjectTrack {
    /// `Some(exit code)` once format ran. `Some(None)` means no format command.
    pub format: Option<Option<i32>>,
    /// The command running now, between `CommandStarted` and `CommandRan`.
    pub running: Option<(CommandPurpose, String)>,
    pub closing_gate: Option<GateId>,
    pub closing: Option<(bool, bool)>,
    /// Rule B10: the project's docs pipeline.
    pub book: DocsPipeline,
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
    /// Rule WD3: the single-project agents the current plan's run was told about.
    pub plan_single_project: Vec<(String, String)>,
    pub phases: BTreeMap<u32, PhaseRun>,
    pub superseded_phases: Vec<PhaseRun>,
    pub project_tracks: BTreeMap<String, ProjectTrack>,
    /// Rule B10: the session-wide docs pipeline. A log whose docs ran per project leaves it empty.
    pub session_book: DocsPipeline,
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
            plan_single_project: vec![],
            phases: BTreeMap::new(),
            superseded_phases: vec![],
            project_tracks: BTreeMap::new(),
            session_book: DocsPipeline::default(),
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
