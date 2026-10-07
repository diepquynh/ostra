//! The book stage's state: each docs run, page draft, scan, and synthesis round.

#[allow(unused_imports)]
use crate::data::*;
use ostra_core::book::DocumentationSubmit;
use ostra_core::ids::{ExecutionId, GateId};
use ostra_core::submit::FactCheckSubmit;
use std::collections::BTreeMap;

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
    /// The scan serves the session-wide pipeline, not the project's own one.
    pub session_wide: bool,
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

/// Rule B10: one docs pipeline: the scan, the survey, the drafts, the rounds, and the gate.
#[derive(Debug, Clone, Default)]
pub struct DocsPipeline {
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
