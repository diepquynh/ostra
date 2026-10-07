//! The standard pipeline's own spawn inputs, beyond the generic ones of `SpawnInputs`: what its
//! contracts' spawn blocks need. They travel in `SpawnInputs::extra`, which the engine never
//! reads.

use ostra_core::book::DocsStep;
use ostra_core::event::{FactTarget, WorkKind};
use ostra_core::pipeline::QuestionAnswer;
use ostra_engine::plan::SpawnInputs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OstraInputs {
    /// On a spec revision, the research documents the current spec was written without.
    pub new_research_docs: Vec<PathBuf>,
    pub projects_in_scope: Vec<(String, PathBuf)>,
    pub answers: Vec<QuestionAnswer>,
    pub changes: Vec<String>,
    /// Rules D2a and D4a: the research documents whose code facts the agent gets. One that reads
    /// the documents gets the files that changed since; one that may not gets the facts file.
    pub code_facts: Vec<PathBuf>,
    /// Fact-check findings a generator must resolve, or fix instructions.
    pub findings: Option<String>,
    /// Rule D4b: the plan phases a failed fact-check's findings name.
    pub revise_phases: Vec<u32>,
    pub target_type: Option<FactTarget>,
    pub source_check: Option<String>,
    pub work: Option<WorkKind>,
    pub changed_files: Vec<String>,
    pub rationale: Option<String>,
    pub implementer_reports: Vec<PathBuf>,
    pub epa_report: Option<PathBuf>,
    pub ledger_file: Option<PathBuf>,
    pub target_files: Option<String>,
    pub question: Option<String>,
    /// Earlier implementer reports a revision builds on.
    pub prior_reports: Vec<PathBuf>,
    /// Files the agent reads first, such as the session context.
    pub context_files: Vec<PathBuf>,
    /// The feedback round a revision phase builds.
    pub revision: Option<u32>,
    /// Initializer inputs, by spawn label.
    pub init: BTreeMap<String, String>,
    /// Rule B10: the reference sheet of modules and constants.
    pub docs_reference: Option<PathBuf>,
    /// Rule B10: the docs step a documentation run answers, the page a writer writes with the
    /// whole page plan, the inventory items it owns, its revision instructions or the round's
    /// findings, the synthesis round, and a fact-check of a draft page.
    pub docs_step: Option<DocsStep>,
    pub docs_page: Option<ostra_core::book::PlannedPage>,
    pub docs_pages: Vec<ostra_core::book::PlannedPage>,
    pub docs_inventory: Vec<String>,
    pub docs_instructions: Vec<String>,
    pub docs_round: u32,
    pub docs_check: bool,
    pub init_item: Option<String>,
}

impl OstraInputs {
    /// The pipeline's inputs of a spawn, or the defaults when it has none.
    pub fn of(inputs: &SpawnInputs) -> OstraInputs {
        serde_json::from_value(inputs.extra.clone()).unwrap_or_default()
    }

    pub fn into_value(self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    /// Rules D2a and D4a: an agent that may not read the research documents gets their code
    /// facts as a file the runner writes; one that reads them gets only what changed since.
    pub fn wants_code_facts_file(&self, inputs: &SpawnInputs) -> bool {
        !self.code_facts.is_empty() && inputs.research_docs.is_empty()
    }
}

impl From<OstraInputs> for SpawnInputs {
    fn from(o: OstraInputs) -> SpawnInputs {
        SpawnInputs {
            extra: o.into_value(),
            ..Default::default()
        }
    }
}

/// Read and change the pipeline's inputs of a spawn.
pub trait OstraInputsExt {
    fn ox(&self) -> OstraInputs;
    fn set_ox(&mut self, f: impl FnOnce(&mut OstraInputs));
}

impl OstraInputsExt for SpawnInputs {
    fn ox(&self) -> OstraInputs {
        OstraInputs::of(self)
    }

    fn set_ox(&mut self, f: impl FnOnce(&mut OstraInputs)) {
        let mut o = OstraInputs::of(self);
        f(&mut o);
        self.extra = o.into_value();
    }
}
