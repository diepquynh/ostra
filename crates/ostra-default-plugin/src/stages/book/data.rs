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
