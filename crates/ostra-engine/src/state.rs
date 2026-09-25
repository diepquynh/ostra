//! A session's state is the fold of its events (HANDOVER 8.1). Everything here is deterministic:
//! the same events always produce the same state, so a restart replays and continues.

use crate::judge::{
    AnswerRoute, ClassifyOut, OptsIn, RescueAction, RescueOut, ResolveAction, ResolveReviewOut,
    RouteAnswerOut, StakesOut, SufficiencyOut, clean_title,
};
use chrono::{DateTime, Utc};
use ostra_core::agent::AgentName;
use ostra_core::event::{
    AnswerSource, CommandPurpose, ExecPurpose, FactTarget, GateAnswer, GatePayload, JudgeKind,
    ProjectRef, SessionEvent, SessionKind, SessionOptions, StoredEvent, WorkKind,
};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId};
use ostra_core::model::Complexity;
use ostra_core::paths;
use ostra_core::pipeline::{Category, PhaseInfo, QuestionAnswer, StageKind, Stakes, TestPolicy};
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
    Rescue { phase: u32, tests: bool },
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
    RescueGate {
        exec: ExecutionId,
        stuck: StuckInfo,
    },
    /// A gate answer with free text waits for the Route answer judge.
    AwaitRoute {
        gate: GateId,
        text: String,
        then: WorkKind,
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum DocsState {
    NotStarted,
    Running(ExecutionId),
    Done(Option<PathBuf>),
    Failed {
        exec: ExecutionId,
        error: String,
        gate: Option<GateId>,
        retries: u32,
    },
    Abandoned,
}

#[derive(Debug, Clone)]
pub struct ProjectTrack {
    /// `Some(exit code)` once format ran. `Some(None)` means no format command.
    pub format: Option<Option<i32>>,
    /// The command running now, between `CommandStarted` and `CommandRan`.
    pub running: Option<(CommandPurpose, String)>,
    pub closing_gate: Option<GateId>,
    pub closing: Option<(bool, bool)>,
    pub docs: DocsState,
}

impl Default for ProjectTrack {
    fn default() -> Self {
        ProjectTrack {
            format: None,
            running: None,
            closing_gate: None,
            closing: None,
            docs: DocsState::NotStarted,
        }
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
}

#[derive(Debug, Clone)]
pub struct InitItem {
    pub key: String,
    pub exec: Option<ExecutionId>,
    pub result: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct SessionState {
    pub id: SessionId,
    pub created: bool,
    pub kind: SessionKind,
    pub request: String,
    pub amendments: Vec<String>,
    pub options: SessionOptions,
    pub yolo: bool,
    pub projects: Vec<ProjectRef>,
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
    pub plan: ArtifactTrack<PlanSubmit>,
    pub phases: BTreeMap<u32, PhaseRun>,
    pub superseded_phases: Vec<PhaseRun>,
    pub project_tracks: BTreeMap<String, ProjectTrack>,
    pub quick: QuickTrack,
    pub init: Option<InitTrack>,
    /// Agents re-routed to native after a harness failure.
    pub native_fallback: BTreeSet<AgentName>,
    pub prompt_gens: u32,
    /// Dollars the user added to the session budget.
    pub budget_raised: f64,
    pub budget_gate: Option<GateId>,

    pub executions: BTreeMap<ExecutionId, ExecRecord>,
    pub gates: BTreeMap<GateId, GateRecord>,
    pub decisions: BTreeMap<DecisionId, DecisionRecord>,
    pub completion_decision: Option<DecisionId>,
    pub completed: Option<(PathBuf, String)>,
    pub failed: Option<String>,
    pub notes: Vec<String>,
    pub last_seq: i64,
    /// The session's short label: from the Classify decision, or fixed for an init session.
    pub title: Option<String>,
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
        ExecPurpose::ModuleDocs { .. } => StageKind::ModuleDocs,
        ExecPurpose::PromptGen {
            handoff_for: Some(_),
        } => StageKind::Handoff,
        ExecPurpose::PromptGen { handoff_for: None } => StageKind::PromptGen,
        ExecPurpose::Verify { .. } => StageKind::Verify,
        ExecPurpose::QuickAnswer => StageKind::QuickAnswer,
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
            amendments: vec![],
            options: SessionOptions::default(),
            yolo: false,
            projects: vec![],
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
            plan: ArtifactTrack::new(),
            phases: BTreeMap::new(),
            superseded_phases: vec![],
            project_tracks: BTreeMap::new(),
            quick: QuickTrack::default(),
            init: None,
            native_fallback: BTreeSet::new(),
            prompt_gens: 0,
            budget_raised: 0.0,
            budget_gate: None,
            executions: BTreeMap::new(),
            gates: BTreeMap::new(),
            decisions: BTreeMap::new(),
            completion_decision: None,
            completed: None,
            failed: None,
            notes: vec![],
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

    /// The request as it now stands, with every amendment (Rule D2).
    pub fn full_request(&self) -> String {
        let mut s = self.request.clone();
        for a in &self.amendments {
            s.push_str("\n\nAdded later by the user: ");
            s.push_str(a);
        }
        s
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
            } => {
                self.created = true;
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
                    self.init = Some(InitTrack {
                        project: project.clone(),
                        ..Default::default()
                    });
                }
            }
            SessionEvent::RequestAmended { text } => self.on_amended(text),
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
                self.on_started(id, purpose, loop_key);
            }
            SessionEvent::ExecutionFinished { id, result } => {
                let Some(rec) = self.executions.get_mut(id) else {
                    return;
                };
                rec.result = Some(result.clone());
                rec.ended_at = Some(at);
                let rec = rec.clone();
                self.on_finished(&rec, result);
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
                self.on_gate_answered(id, &payload, answer);
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

    fn push_explore(&mut self, project: String, task: String, origin: ExploreOrigin) -> u32 {
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

    fn on_amended(&mut self, text: &str) {
        self.amendments.push(text.to_string());
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
        let mut scope: Vec<String> = out
            .projects
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
            Category::UnitTest => {
                for (i, key) in scope.iter().enumerate() {
                    let id = i as u32 + 1;
                    let mut l =
                        WorkLoop::new(false, AgentName::Implementer, AgentName::Implementer);
                    l.next = LoopNext::Done;
                    let report = self
                        .project_session_dir(key)
                        .join(paths::report::unit_test_request());
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
                    for item in out.items.into_iter().filter(|i| i.needed) {
                        let (project, task) = match item.task {
                            Some(t) if self.valid_project(&t.project) => (t.project, t.task),
                            _ => (self.primary(), item.item.clone()),
                        };
                        self.push_explore(project, task, ExploreOrigin::Sufficiency);
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
                    RescueAction::Gate => LoopNext::RescueGate { exec, stuck },
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
                let Some(gate) = subject.map(GateId::from) else {
                    return;
                };
                let Ok(out) = serde_json::from_value::<RouteAnswerOut>(output.clone()) else {
                    return;
                };
                let target = self
                    .phases
                    .iter()
                    .flat_map(|(id, p)| [((*id, false), &p.impl_loop), ((*id, true), &p.test_loop)])
                    .find_map(|(k, l)| match &l.next {
                        LoopNext::AwaitRoute {
                            gate: g,
                            text,
                            then,
                        } if *g == gate => Some((k, text.clone(), *then)),
                        _ => None,
                    });
                let Some((key, text, then)) = target else {
                    return;
                };
                match out.route {
                    AnswerRoute::RequirementChange => {
                        // Rule D10: stop the phase and restart at the spec.
                        if let Some(l) = self.loop_mut(key) {
                            l.next = LoopNext::Blocked { reason: "The user changed a requirement; the spec is being updated (Rule D10).".into() };
                            l.announced_block = true;
                            l.block_gate_answered = true;
                        }
                        self.requirement_change(text);
                    }
                    AnswerRoute::ImplementationDetail | AnswerRoute::StageChoice => {
                        if let Some(l) = self.loop_mut(key) {
                            l.next = LoopNext::Work {
                                kind: then,
                                instructions: Some(text),
                            };
                        }
                    }
                }
            }
            JudgeKind::Completion => self.completion_decision = Some(id.clone()),
            JudgeKind::YoloAnswer => {}
        }
    }

    /// The executor an agent must run on regardless of routing: native after a harness failure,
    /// and native for a quick change, which skips the harness startup cost.
    pub fn forced_executor(&self, agent: AgentName) -> Option<ostra_core::ExecutorKind> {
        (self.native_fallback.contains(&agent) || self.category == Some(Category::QuickChange))
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
    ) {
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
                t.runs.push(id.clone());
                t.needs_run = false;
                t.sent = InputMark {
                    answers: t.answers.len(),
                    changes: t.changes.len(),
                    docs,
                };
            }
            ExecPurpose::Plan { .. } => {
                self.plan.running = Some(id.clone());
                self.plan.runs.push(id.clone());
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
            ExecPurpose::ModuleDocs { project } => {
                self.project_tracks.entry(project.clone()).or_default().docs =
                    DocsState::Running(id.clone());
            }
            ExecPurpose::QuickAnswer => {
                self.quick.exec = Some(id.clone());
                self.quick.running = true;
            }
            ExecPurpose::Init { mode, item } => self.init_started(id, *mode, item.clone()),
            ExecPurpose::PromptGen { handoff_for: None } => self.prompt_gens += 1,
            _ => {}
        }
        if let Some(key) = loop_key {
            let is_handoff = matches!(
                purpose,
                ExecPurpose::PromptGen {
                    handoff_for: Some(_)
                }
            );
            if matches!(purpose, ExecPurpose::PromptGen { .. }) {
                self.prompt_gens += u32::from(is_handoff);
            }
            if let Some(l) = self.loop_mut(key) {
                l.running = Some(id.clone());
                l.in_flight = Some(l.next.clone());
                if matches!(
                    purpose,
                    ExecPurpose::Implement { .. }
                        | ExecPurpose::WriteTest { .. }
                        | ExecPurpose::Verify { .. }
                ) || matches!(purpose, ExecPurpose::PromptGen { handoff_for: None })
                {
                    l.work_count += 1;
                }
            }
        }
    }

    fn on_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult) {
        let status = result.status;
        let error = result
            .error
            .clone()
            .unwrap_or_else(|| format!("execution ended with status {status:?}"));
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
            ExecPurpose::ModuleDocs { project } => {
                let t = self.project_tracks.entry(project.clone()).or_default();
                t.docs = match status {
                    ExecutionStatus::Ok => DocsState::Done(rec.report_path.clone()),
                    ExecutionStatus::Interrupted => DocsState::NotStarted,
                    _ => DocsState::Failed {
                        exec: rec.id.clone(),
                        error,
                        gate: None,
                        retries: 1,
                    },
                };
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
            _ => {}
        }
        if let Some(key) = rec.loop_key {
            self.loop_finished(key, rec, result);
        }
    }

    fn loop_finished(&mut self, key: (u32, bool), rec: &ExecRecord, result: &ExecutionResult) {
        let yolo = self.yolo;
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
        let error = result
            .error
            .clone()
            .unwrap_or_else(|| format!("execution ended with status {status:?}"));
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
            ExecPurpose::ModuleDocs { project } => {
                if let DocsState::Failed { gate: g, .. } =
                    &mut self.project_tracks.entry(project.clone()).or_default().docs
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::Init { .. } => {
                if let Some(i) = self.init.as_mut() {
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
            GatePayload::SkillApproval { .. } => {
                if let Some(i) = self.init.as_mut() {
                    i.approval_gate = Some(id.clone());
                }
            }
            GatePayload::Permission { .. } => {}
            GatePayload::BudgetReached { .. } => self.budget_gate = Some(id.clone()),
        }
    }

    fn on_gate_answered(&mut self, id: &GateId, payload: &GatePayload, answer: &GateAnswer) {
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
                match answer {
                    GateAnswer::Approval { approved: true, .. }
                        if current && self.spec.passed_current() =>
                    {
                        self.spec.approved = true;
                        self.spec.approved_version = self.spec.version;
                        if self.plan.invalidated {
                            self.plan.needs_run = true;
                        }
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
                            if let Some(t) = text {
                                instr.push_str(&format!("\n\nThe user added: {t}"));
                            }
                            l.next = LoopNext::Work {
                                kind: WorkKind::Fix,
                                instructions: Some(instr),
                            };
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
                    if let LoopNext::RescueGate { stuck, .. } = l.next.clone() {
                        match choice {
                            Some(("fact", Some(text))) => {
                                l.next = LoopNext::AwaitRoute {
                                    gate: id.clone(),
                                    text: rescue_context(&stuck, &text),
                                    then: WorkKind::Rescue,
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
                        l.next = match text {
                            Some(t) => LoopNext::AwaitRoute {
                                gate: id.clone(),
                                text: t,
                                then: WorkKind::Fix,
                            },
                            None => LoopNext::Work {
                                kind: WorkKind::Fix,
                                instructions: l.last_review.as_ref().map(|r| {
                                    let hm: Vec<ReviewFinding> = r
                                        .findings
                                        .iter()
                                        .filter(|f| {
                                            matches!(
                                                f.severity,
                                                Severity::High
                                                    | Severity::Medium
                                                    | Severity::Blocker
                                            )
                                        })
                                        .cloned()
                                        .collect();
                                    fix_instructions(&hm, "the review ledger")
                                }),
                            },
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
            GatePayload::SkillApproval { .. } => {
                if let (Some(i), GateAnswer::Skills { decisions }) = (self.init.as_mut(), answer) {
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
            ExecPurpose::ModuleDocs { project } => {
                self.project_tracks.entry(project.clone()).or_default().docs = if retry {
                    DocsState::NotStarted
                } else {
                    DocsState::Abandoned
                };
            }
            ExecPurpose::QuickAnswer => {
                if retry {
                    self.quick.failed = None;
                    self.quick.exec = None;
                }
            }
            ExecPurpose::Init { .. } => {
                let mut abandoned = false;
                if let Some(i) = self.init.as_mut() {
                    i.failed_gate = None;
                    if retry {
                        let key = i.failed.take();
                        if let Some((exec, _)) = key {
                            reset_init_item(i, &exec);
                        }
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
            let valid = self.valid_project(&info.project);
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

    fn init_started(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
    ) {
        use ostra_core::InitializerMode as M;
        let Some(i) = self.init.as_mut() else { return };
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
        let Some(i) = self.init.as_mut() else { return };
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
                        format!(
                            "{}: {}",
                            p.summary,
                            p.stuck.map(|s| s.need).unwrap_or_default()
                        )
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
