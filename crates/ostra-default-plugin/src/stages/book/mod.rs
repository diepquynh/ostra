//! The docs stage's book writer (HANDOVER 8.5, Rule B10): the scan, the survey, the page writers,
//! the fact-checks, and the synthesis rounds. The core holds the book format and writes the book
//! from the update this writer hands it (Rules B5 and B6).

pub mod checks;
pub mod data;
pub mod fold;
pub mod gates;
pub mod hooks;
pub mod parts;
pub mod planner;
pub mod runs;
pub mod scan;
pub mod track;
pub mod view;

use ostra_core::Contract;
use ostra_core::book::{
    CROSS_PART, DocSection, DocsModule, DocsStep, DocumentationSubmit, GlossaryEntry,
    InventoryItem, PageUpdate, PartUpdate, PlannedPage, RefItem,
};
use ostra_core::submit::SubmitStatus;

pub use checks::{check_documentation, mechanical_issues, unmentioned_refs};
pub use data::*;
pub use fold::*;
pub use gates::*;
pub use planner::*;
pub use runs::*;
pub use track::DocsTrack;

/// Rule B10: the pipeline key a docs run carries: its project, or `_session`.
pub fn docs_key(purpose: &ostra_core::event::ExecPurpose) -> Option<&str> {
    use ostra_core::event::ExecPurpose as P;
    match purpose {
        P::Docs { project, .. }
        | P::DocsSurvey { project }
        | P::DocsCheck { project, .. }
        | P::DocsSynthesis { project, .. } => Some(project),
        _ => None,
    }
}

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
        part_overviews: survey.part_overviews.clone(),
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

/// Rule B11: the part of a planned page in a session-wide book. A page without a part belongs to
/// the first documented project.
pub fn page_part(page: &PlannedPage, projects: &[String]) -> String {
    page.part
        .clone()
        .or_else(|| projects.first().cloned())
        .unwrap_or_default()
}

/// Rule B11: the parts a session-wide book update holds: each part with at least one planned page,
/// the project parts first and the part across projects last. Each part gets its pages in plan
/// order, its overview, and the inventory items its pages own. The first part also carries the
/// glossary and the items out of scope.
pub fn session_parts(
    book: &DocumentationSubmit,
    plan: &[(PlannedPage, Option<DocSection>)],
    projects: &[String],
) -> Vec<PartUpdate> {
    let mut keys: Vec<String> = projects.to_vec();
    for (p, _) in plan {
        let part = page_part(p, projects);
        if part != CROSS_PART && !keys.contains(&part) {
            keys.push(part);
        }
    }
    keys.push(CROSS_PART.into());
    let mut out: Vec<PartUpdate> = vec![];
    for key in keys {
        let mine: Vec<&(PlannedPage, Option<DocSection>)> = plan
            .iter()
            .filter(|(p, _)| page_part(p, projects) == key)
            .collect();
        if mine.is_empty() {
            continue;
        }
        let pages = mine
            .iter()
            .map(|(p, page)| match page {
                Some(page) => PageUpdate::Write(placed(p, page)),
                None => PageUpdate::Keep {
                    id: p.id.clone(),
                    group: p.group.clone(),
                },
            })
            .collect();
        let first = out.is_empty();
        let inventory = book
            .inventory
            .iter()
            .filter(|i| match &i.out_of_scope {
                Some(_) => first,
                None => mine.iter().any(|(p, _)| p.id == i.owner),
            })
            .cloned()
            .collect();
        let overview = book
            .part_overviews
            .iter()
            .find(|o| o.part == key)
            .map(|o| o.overview.clone())
            .unwrap_or_else(|| book.overview.clone());
        out.push(PartUpdate {
            project: key,
            overview,
            pages,
            glossary: if first { book.glossary.clone() } else { vec![] },
            inventory,
        });
    }
    out
}

/// Rule B10: the scans of every project for the session-wide pipeline, in one sheet. Each module
/// glob and each file is written `@<project>/<path>`, the form a session-wide survey writes its
/// sources in, so the coverage check knows the project of each module.
pub fn session_scan(
    tracks: &std::collections::BTreeMap<String, crate::data::ProjectTrack>,
) -> DocsScan {
    let mut out = DocsScan {
        session_wide: true,
        ..Default::default()
    };
    for (key, t) in tracks {
        let Some(scan) = t.book.docs_scan.as_ref().filter(|s| s.session_wide) else {
            continue;
        };
        out.modules.extend(scan.modules.iter().map(|m| {
            DocsModule {
                name: format!("{key}/{}", m.name),
                globs: m
                    .globs
                    .iter()
                    .map(|g| format!("@{key}/{}", g.trim_start_matches("./")))
                    .collect(),
            }
        }));
        out.refs.extend(scan.refs.iter().map(|r| RefItem {
            name: r.name.clone(),
            file: format!("@{key}/{}", r.file),
        }));
    }
    out
}
