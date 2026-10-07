//! The build stage's state: each phase's work loop and what it does next.

#[allow(unused_imports)]
use crate::data::*;
use ostra_core::event::WorkKind;
use ostra_core::ids::{ExecutionId, GateId};
use ostra_core::submit::{CodeReviewerSubmit, HandoffInfo, ReviewFinding, StuckInfo};
use std::collections::BTreeSet;
use std::path::PathBuf;

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
