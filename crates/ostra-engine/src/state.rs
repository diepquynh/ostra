//! A session's state is the fold of its events (HANDOVER 8.1). Everything here is deterministic:
//! the same events always produce the same state, so a restart replays and continues.

use crate::pipeline;
use chrono::{DateTime, Utc};
use ostra_core::agent::AgentName;
use ostra_core::book::DocumentationSubmit;
use ostra_core::containment::{ContainmentSignal, PAUSE_AFTER};
use ostra_core::event::{
    AnswerSource, CommandPurpose, ContextDelivery, ContextFile, ExecPurpose, FactTarget,
    GateAnswer, GatePayload, JudgeKind, ProjectRef, SessionEvent, SessionKind, SessionOptions,
    StoredEvent, UploadedFile, WorkKind,
};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId};
use ostra_core::paths;
use ostra_core::pipeline::{Category, PhaseInfo, QuestionAnswer, StageKind, Stakes, Track};
use ostra_core::submit::{
    CodeReviewerSubmit, ExploreSubmit, FactCheckSubmit, GenerateSpecSubmit, HandoffInfo,
    PlanSubmit, QuickAnswerSubmit, ReviewFinding, StuckInfo, Verdict,
};
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
/// Rule B5: the book write that ends the docs stage.
#[derive(Debug, Clone, PartialEq)]
pub struct BookWrite {
    pub book: String,
    pub projects: Vec<String>,
    pub error: Option<String>,
}

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

impl ProjectTrack {
    /// Rule B10: the survey, once it is done.
    pub fn survey_plan(&self) -> Option<&DocumentationSubmit> {
        match &self.survey {
            DocsState::Done(s) => Some(s),
            _ => None,
        }
    }

    /// Rule B10: the survey's inventory with every item a synthesis pass added; a later item
    /// with the same ID replaces an earlier one.
    pub fn current_inventory(&self) -> Vec<ostra_core::book::InventoryItem> {
        let mut out: Vec<ostra_core::book::InventoryItem> = self
            .survey_plan()
            .map(|s| s.inventory.clone())
            .unwrap_or_default();
        for item in &self.inventory_added {
            out.retain(|i| i.id != item.id);
            out.push(item.clone());
        }
        out
    }

    /// Rule B10: the engine's checks over the current drafts, by page, with the inventory's under
    /// the empty key.
    pub fn engine_issues(&self) -> BTreeMap<String, Vec<String>> {
        let modules = self
            .docs_scan
            .as_ref()
            .map(|s| s.modules.as_slice())
            .unwrap_or_default();
        let kept: Vec<String> = self
            .survey_plan()
            .map(|s| {
                s.pages
                    .iter()
                    .filter(|p| !p.rewrite)
                    .map(|p| p.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        ostra_core::book::mechanical_issues(
            &self.placed_drafts(),
            &kept,
            &self.current_inventory(),
            modules,
        )
    }

    /// Rule B10: the pages the survey plans to write in this session, in plan order.
    pub fn planned_pages(&self) -> Vec<&ostra_core::book::PlannedPage> {
        self.survey_plan()
            .map(|s| s.pages.iter().filter(|p| p.rewrite).collect())
            .unwrap_or_default()
    }

    /// Rule B10: every current draft, in plan order.
    pub fn drafts(
        &self,
    ) -> Vec<(
        &ostra_core::book::PlannedPage,
        &ostra_core::book::DocSection,
    )> {
        self.planned_pages()
            .into_iter()
            .filter_map(|p| {
                self.page_docs
                    .get(&p.id)
                    .and_then(|d| d.draft.as_ref())
                    .map(|d| (p, d))
            })
            .collect()
    }

    /// Rule B10: the drafts placed under their planned IDs and groups, for the engine's checks.
    pub fn placed_drafts(&self) -> Vec<ostra_core::book::DocSection> {
        self.drafts()
            .into_iter()
            .map(|(p, d)| {
                let mut d = d.clone();
                d.id = p.id.clone();
                d.group = p.group.clone();
                d
            })
            .collect()
    }

    /// Rule B10: every first draft run settled.
    pub fn first_drafts_settled(&self) -> bool {
        self.planned_pages().iter().all(|p| {
            self.page_docs
                .get(&p.id)
                .is_some_and(|d| d.run.is_settled())
        })
    }

    /// Rule B10: the pages a round's fact-checks cover: every draft in round 1, then the pages
    /// the previous round revised.
    pub fn pages_to_check(&self, round: u32) -> Vec<String> {
        if round <= 1 {
            return self
                .drafts()
                .into_iter()
                .map(|(p, _)| p.id.clone())
                .collect();
        }
        self.docs_rounds
            .get(round as usize - 2)
            .map(|r| {
                r.revisions
                    .iter()
                    .filter(|(_, st)| matches!(st, DocsState::Done(_)))
                    .map(|(p, _)| p.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Rule B10: the loop is over: the user accepted the book, the last synthesis pass found it
    /// done with no page to revise, or a round revised nothing that needs a check.
    pub fn docs_finished(&self) -> bool {
        if self.docs_accepted {
            return true;
        }
        let Some(last) = self.docs_rounds.last() else {
            return false;
        };
        // Rule B10: a done pass finishes the loop only when no inventory item or module lacks an
        // owner; otherwise the next round runs another synthesis pass.
        if let DocsState::Done(syn) = &last.synthesis
            && syn.done
            && last.targets.is_empty()
            && !self.engine_issues().contains_key("")
        {
            return true;
        }
        let revised = last.synthesis.is_settled()
            && !last.targets.is_empty()
            && last
                .targets
                .keys()
                .all(|p| last.revisions.get(p).is_some_and(|r| r.is_settled()));
        revised
            && self
                .pages_to_check(self.docs_rounds.len() as u32 + 1)
                .is_empty()
    }

    /// The run a docs execution folds into.
    pub fn docs_run(&mut self, page: Option<&str>, round: u32) -> &mut DocsState {
        match page {
            None => &mut self.docs,
            Some(p) if round == 0 => &mut self.page_docs.entry(p.to_string()).or_default().run,
            Some(p) => self
                .round_mut(round)
                .revisions
                .entry(p.to_string())
                .or_default(),
        }
    }

    pub fn round_mut(&mut self, round: u32) -> &mut DocsRound {
        while self.docs_rounds.len() < round as usize {
            self.docs_rounds.push(DocsRound::default());
        }
        &mut self.docs_rounds[round as usize - 1]
    }

    /// The project's docs as one run: a whole-part writer from an older log; else the survey;
    /// else running while any run of the pipeline runs, failed while one waits on its failure,
    /// done with the drafts when the loop is over, and abandoned when no page was written.
    pub fn docs_aggregate(&self) -> DocsState {
        if !matches!(self.docs, DocsState::NotStarted) {
            return self.docs.clone();
        }
        let Some(survey) = self.survey_plan() else {
            return self.survey.clone();
        };
        let mut runs: Vec<&DocsState> = self.page_docs.values().map(|d| &d.run).collect();
        for r in &self.docs_rounds {
            runs.push(&r.synthesis);
            runs.extend(r.revisions.values());
        }
        if let Some(r) = runs.iter().find(|r| matches!(r, DocsState::Running(_))) {
            return (*r).clone();
        }
        for r in &self.docs_rounds {
            if let Some(CheckState::Running(id)) = r
                .checks
                .values()
                .find(|c| matches!(c, CheckState::Running(_)))
            {
                return DocsState::Running(id.clone());
            }
        }
        if let Some(f) = runs.iter().find(|r| matches!(r, DocsState::Failed { .. })) {
            return (*f).clone();
        }
        if !self.first_drafts_settled() {
            return DocsState::NotStarted;
        }
        let drafts = self.drafts();
        if drafts.is_empty() && survey.pages.iter().all(|p| p.rewrite) {
            return DocsState::Abandoned;
        }
        if !self.docs_finished() {
            return DocsState::NotStarted;
        }
        let glossary = self
            .page_docs
            .values()
            .flat_map(|d| d.glossary.iter().cloned())
            .chain(survey.glossary.iter().cloned())
            .collect();
        DocsState::Done(Box::new(ostra_core::book::combine_pages(
            survey,
            &drafts,
            glossary,
            self.current_inventory(),
        )))
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
    /// Rule CA5: the result contract it submits.
    pub contract: ostra_core::Contract,
    /// Rule PL5: what its plugin's handler made of a plugin contract's result.
    pub handled: Option<ostra_core::submit::CustomSubmit>,
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
pub fn upload_list(heading: &str, uploads: &[UploadedFile]) -> String {
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
        ExecPurpose::Stage { node, scope, .. } => {
            format!("stage:{node}:{}", scope.as_deref().unwrap_or_default())
        }
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
    /// Rule PL8: each plugin's checkpoints in this session, by key.
    pub plugin_checkpoints: BTreeMap<String, BTreeMap<String, Value>>,
    /// Rule PL8: how often each plugin saved a checkpoint in this session.
    pub checkpoint_saves: BTreeMap<String, u32>,
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
    /// Rule SM8: messages between subagents, in the order they were sent.
    pub messages: Vec<crate::coord::Message>,
    /// Rule SM3: runs that paused themselves and what each waits for.
    pub waits: BTreeMap<ExecutionId, crate::coord::WaitOn>,
    /// Rule WF1: the workflow the session asked for; `None` for logs written before workflows.
    pub workflow_choice: Option<ostra_core::workflow::WorkflowChoice>,
    /// Rule WF1: the workflow the session runs, once resolved.
    pub workflow: Option<ostra_core::workflow::WorkflowDef>,
    /// Rule WF4: custom stage instances by [`crate::workflow::stage_key`].
    pub stages: BTreeMap<String, crate::workflow::StageTrack>,
    /// Rule H1: each execution's subagent ID, for runs that continue another's conversation.
    pub subagents: BTreeMap<ExecutionId, ExecutionId>,
    pub last_seq: i64,
    /// The session's short label: from the Classify decision, or fixed for an init session.
    pub title: Option<String>,
    /// Rule WB3: what prompt nodes spent on their model calls.
    pub node_cost: f64,
}

pub fn parse<T: serde::de::DeserializeOwned>(v: &Option<Value>) -> Option<T> {
    v.as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub fn stage_of(purpose: &ExecPurpose) -> StageKind {
    match purpose {
        ExecPurpose::Advise { .. } | ExecPurpose::Unblock { .. } => StageKind::Rescue,
        ExecPurpose::Consult { .. } | ExecPurpose::Message { .. } => StageKind::Handoff,
        ExecPurpose::Helper { .. } => StageKind::Explore,
        ExecPurpose::Stage { .. } => StageKind::Custom,
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
        ExecPurpose::Docs { .. }
        | ExecPurpose::DocsSurvey { .. }
        | ExecPurpose::DocsCheck { .. }
        | ExecPurpose::DocsSynthesis { .. } => StageKind::Documentation,
        ExecPurpose::Architecture => StageKind::Documentation,
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
            plugin_checkpoints: BTreeMap::new(),
            checkpoint_saves: BTreeMap::new(),
            contained: None,
            interrupting: BTreeMap::new(),
            resume_from: BTreeMap::new(),
            steers: BTreeMap::new(),
            waiting_keys: BTreeMap::new(),
            messages: vec![],
            waits: BTreeMap::new(),
            workflow_choice: None,
            workflow: None,
            stages: BTreeMap::new(),
            subagents: BTreeMap::new(),
            last_seq: 0,
            title: None,
            node_cost: 0.0,
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
    pub fn file_list(&self, heading: &str, files: &[ContextFile]) -> String {
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

    pub fn added_part(
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

    pub fn push_explore(&mut self, project: String, task: String, origin: ExploreOrigin) -> u32 {
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
            agent: None,
        });
        idx
    }

    pub fn open_gates(&self) -> impl Iterator<Item = &GateRecord> {
        self.gates.values().filter(|g| g.answer.is_none())
    }

    pub fn running_executions(&self) -> impl Iterator<Item = &ExecRecord> {
        self.executions.values().filter(|e| e.result.is_none())
    }

    /// What finished executions and prompt nodes spent. Running executions are counted when they
    /// finish.
    pub fn spent_usd(&self) -> f64 {
        self.executions
            .values()
            .filter_map(|e| e.result.as_ref())
            .map(|r| r.usage.cost_usd)
            .sum::<f64>()
            + self.node_cost
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
                workflow,
            } => {
                self.created = true;
                self.workflow_choice = workflow.clone();
                self.pinned = pinned.clone();
                self.docs_book = docs_book.clone();
                self.files = files.clone();
                self.uploads = uploads.clone();
                self.created_at = at;
                self.kind = kind.clone();
                self.request = request.clone();
                self.options = *options;
                self.yolo = options.yolo;
                self.projects = projects.clone();
                self.workspace_root = workspace_root.clone();
                self.session_root = session_root.clone();
                if let SessionKind::Init { project } = kind {
                    self.scope = vec![project.clone()];
                    self.title = Some(format!("Initialize {project}"));
                }
                pipeline::get().created(self);
            }
            SessionEvent::WorkflowResolved { workflow } => self.on_workflow_resolved(workflow),
            SessionEvent::ResultHandled { execution, outcome } => {
                self.on_result_handled(execution, outcome)
            }
            SessionEvent::PluginCheckpoint { plugin, key, value } => {
                *self.checkpoint_saves.entry(plugin.clone()).or_default() += 1;
                let kept = self.plugin_checkpoints.entry(plugin.clone()).or_default();
                match value {
                    Some(v) => {
                        kept.insert(key.clone(), v.clone());
                    }
                    None => {
                        kept.remove(key);
                    }
                }
            }
            SessionEvent::StageDecided {
                node,
                scope,
                decision,
            } => self.on_stage_decided(node, scope.as_deref(), decision),
            SessionEvent::StageSkipped { node, scope } => {
                self.on_stage_skipped(node, scope.as_deref())
            }
            SessionEvent::NodeRan {
                node,
                scope,
                round,
                output,
                error,
                cost_usd,
            } => self.on_node_ran(
                node,
                scope.as_deref(),
                *round,
                output.as_ref(),
                error.as_deref(),
                *cost_usd,
            ),
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
                pipeline::get().project_created(self, project);
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
            SessionEvent::ProjectInitFinished { .. } | SessionEvent::InitStepFailed { .. } => {
                pipeline::get().event(self, stored)
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
                pipeline::get().decision(self, id, *judge, subject.as_deref(), output, false);
            }
            SessionEvent::DecisionOverridden { id, output, reason } => {
                let Some(d) = self.decisions.get_mut(id) else {
                    return;
                };
                d.output = output.clone();
                d.reason = reason.clone();
                d.overridden = true;
                let (judge, subject) = (d.judge, d.subject.clone());
                pipeline::get().decision(self, id, judge, subject.as_deref(), output, true);
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
                contract,
            } => {
                // Rule CA5: logs from before contracts ran the standard agents.
                let contract = contract.unwrap_or_else(|| crate::workflow::legacy_contract(*agent));
                let loop_key = pipeline::get().loop_key(self, purpose);
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
                        contract,
                        handled: None,
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
                pipeline::get().started(self, id, purpose, loop_key, false);
                self.coord_started(id, purpose);
                self.stage_started(id, purpose);
            }
            SessionEvent::MessageSent { .. }
            | SessionEvent::AgentWaiting { .. }
            | SessionEvent::MessagesDelivered { .. }
            | SessionEvent::AgentAsked { .. }
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
                pipeline::get().started(self, id, &purpose, loop_key, true);
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
                if pipeline::get().can_skip(self, id) {
                    self.interrupting.insert(id.clone(), Interrupt::Skipped);
                    pipeline::get().skip_task(self, id);
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
                    pipeline::get().finished(self, &rec, &paused);
                } else {
                    pipeline::get().finished(self, &rec, result);
                }
                // Rule U1: the stopped run's finish would put its task back to re-run.
                if why == Some(Interrupt::Skipped) {
                    pipeline::get().skip_task(self, id);
                }
                self.coord_finished(&rec, result);
                self.stage_finished(&rec, result);
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
                pipeline::get().gate_opened(self, id, payload);
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
                pipeline::get().gate_answered(self, id, &payload, answer, *routed);
            }
            SessionEvent::CommandStarted { .. }
            | SessionEvent::DocsPlanned { .. }
            | SessionEvent::DocsScanned { .. }
            | SessionEvent::CommandRan { .. }
            | SessionEvent::AutofixApplied { .. }
            | SessionEvent::SecurityBlock { .. }
            | SessionEvent::PhaseBlocked { .. } => pipeline::get().event(self, stored),
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

    pub fn valid_project(&self, key: &str) -> bool {
        self.projects.iter().any(|p| p.key == key)
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
    pub fn release_amendment(&mut self, i: usize) {
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
            pipeline::get().amended(self, &part);
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

    /// Rule P4: the user stopped this execution, so nothing retries it without them, YOLO included.
    pub fn stopped_by_user(&self, exec: &ExecutionId) -> bool {
        // A pause or an interrupt turns a cancel into Interrupted, and a stopped session ends,
        // so a Cancelled run in a live session is one the user stopped.
        self.executions
            .get(exec)
            .and_then(|r| r.result.as_ref())
            .is_some_and(|r| r.status == ExecutionStatus::Cancelled)
    }

    pub fn interrupt_running(&mut self, why: Interrupt) {
        let running: Vec<ExecutionId> = self.running_executions().map(|r| r.id.clone()).collect();
        for id in running {
            self.interrupting.entry(id).or_insert(why);
        }
    }

    pub fn session_context_path(&self) -> PathBuf {
        self.session_root.join(paths::report::session_context())
    }

    // -----------------------------------------------------------------------------------------
    // Init flow
    // -----------------------------------------------------------------------------------------

    pub fn exec_project(&self, id: &ExecutionId) -> String {
        self.executions
            .get(id)
            .map(|r| r.project.clone())
            .unwrap_or_default()
    }
}

pub fn exec_error(result: &ExecutionResult) -> String {
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
