//! The docs stage's book writer (HANDOVER 8.5, Rule B10): the scan, the survey, the page writers,
//! the fact-checks, and the synthesis rounds. The core holds the book format and writes the book
//! from the update this writer hands it (Rules B5 and B6).

pub mod checks;
pub mod data;
pub mod fold;
pub mod gates;
pub mod planner;
pub mod runs;
pub mod scan;
pub mod track;

use ostra_core::Contract;
use ostra_core::book::{
    DocSection, DocsStep, DocumentationSubmit, GlossaryEntry, InventoryItem, PageUpdate,
    PartUpdate, PlannedPage,
};
use ostra_core::submit::SubmitStatus;

pub use checks::{check_documentation, mechanical_issues, unmentioned_refs};
pub use data::*;
pub use fold::*;
pub use gates::*;
pub use planner::*;
pub use runs::*;
pub use track::DocsTrack;

/// Rule B10: pages one survey may plan, each written by its own writer.
pub const MAX_DOCS_PAGES: usize = 30;
/// Rule B10: synthesis rounds before a gate asks whether to go on.
pub const DOCS_ROUNDS: u32 = 3;

/// Rule B10: install the docs stage's check of the `documentation` contract for the deprecated
/// `ostra_core::book::check_documentation`. Submit tools run it through the pipeline.
pub fn install_checks() {
    ostra_core::submit::install_check(Contract::Documentation, checks::check_value);
}

/// Rule B10: a page under its planned ID and group.
pub fn placed(plan: &PlannedPage, page: &DocSection) -> DocSection {
    let mut page = page.clone();
    page.id = plan.id.clone();
    page.group = plan.group.clone();
    page
}

/// Rule B10: the survey's overview and inventory with the drafts, each page under its planned
/// ID and group, in plan order.
pub fn combine_pages(
    survey: &DocumentationSubmit,
    drafts: &[(&PlannedPage, &DocSection)],
    glossary: Vec<GlossaryEntry>,
    inventory: Vec<InventoryItem>,
) -> DocumentationSubmit {
    DocumentationSubmit {
        status: SubmitStatus::Ok,
        step: DocsStep::Survey,
        summary: survey.summary.clone(),
        overview: survey.overview.clone(),
        sections: drafts.iter().map(|(p, d)| placed(p, d)).collect(),
        inventory,
        pages: survey.pages.clone(),
        edits: vec![],
        checks: vec![],
        done: false,
        glossary,
        stuck: None,
    }
}

/// Rule B6: a project's part as the book receives it. With `plan`, every planned page in order:
/// the page this session wrote, or a page kept from the book. Without one, as a writer of a
/// whole part from an older log wrote it.
pub fn part_update(
    project: &str,
    part: &DocumentationSubmit,
    plan: Option<&[(PlannedPage, Option<DocSection>)]>,
) -> PartUpdate {
    let (pages, inventory) = match plan {
        Some(plan) => (
            plan.iter()
                .map(|(p, page)| match page {
                    Some(page) => PageUpdate::Write(placed(p, page)),
                    None => PageUpdate::Keep {
                        id: p.id.clone(),
                        group: p.group.clone(),
                    },
                })
                .collect(),
            part.inventory.clone(),
        ),
        None => (
            part.sections
                .iter()
                .cloned()
                .map(PageUpdate::Write)
                .collect(),
            vec![],
        ),
    };
    PartUpdate {
        project: project.to_string(),
        overview: part.overview.clone(),
        pages,
        glossary: part.glossary.clone(),
        inventory,
    }
}
