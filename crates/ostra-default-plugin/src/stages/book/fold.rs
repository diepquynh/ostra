//! Rules B5, B6, and B10: the docs stage's paths and what a session adds to its book.

use crate::book::DocsTrack;
use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::paths;
use ostra_engine::state::*;
use std::path::PathBuf;

/// Rules B5, B6, and B10: the docs stage's paths and what a session adds to its book.
pub trait FoldBook {
    /// Whether a project's closing stages include docs, recorded or implied by an explicit
    /// request for both tests and docs (Rule T3).
    fn project_docs_on(&self, key: &str) -> bool;

    /// Projects whose part of the book this session writes, sorted.
    fn docs_projects(&self) -> Vec<String>;

    /// Rule B6: what this session adds to its book, from the docs-stage submits in the log.
    fn book_update(&self) -> ostra_core::book::BookUpdate;

    /// Rule B6: the picked book, or the one named after the documented projects.
    fn book_id(&self) -> String;

    /// Rule B10: where the runner writes a project's docs drafts.
    fn docs_drafts_dir(&self, project: &str) -> PathBuf;

    fn docs_draft_path(&self, project: &str, page: &str) -> PathBuf;

    fn docs_inventory_path(&self, project: &str) -> PathBuf;
}

impl FoldBook for SessionState {
    fn project_docs_on(&self, key: &str) -> bool {
        self.ext
            .os()
            .project_tracks
            .get(key)
            .and_then(|t| t.closing)
            .map(|c| c.1)
            .unwrap_or(self.tests_requested() && self.docs_requested())
    }

    fn docs_projects(&self) -> Vec<String> {
        self.ext
            .os()
            .project_tracks
            .keys()
            .filter(|k| {
                self.project_docs_on(k)
                    && self
                        .ext
                        .os()
                        .phases
                        .values()
                        .any(|p| p.info.projects().contains(k) && p.impl_loop.is_done())
            })
            .cloned()
            .collect()
    }

    fn book_update(&self) -> ostra_core::book::BookUpdate {
        let parts = self
            .ext
            .os()
            .project_tracks
            .iter()
            .filter_map(|(k, t)| match t.docs_aggregate() {
                DocsState::Done(d) => {
                    // Rule B10: every planned page, with the draft this session wrote or `None`
                    // to keep it.
                    let plan: Option<Vec<_>> = t.survey_plan().map(|survey| {
                        survey
                            .pages
                            .iter()
                            .map(|p| {
                                let draft = t.page_docs.get(&p.id).and_then(|d| d.draft.clone());
                                (p.clone(), if p.rewrite { draft } else { None })
                            })
                            .collect()
                    });
                    Some(crate::book::part_update(k, &d, plan.as_deref()))
                }
                _ => None,
            })
            .collect();
        ostra_core::book::BookUpdate {
            session: self.id.to_string(),
            parts,
        }
    }

    fn book_id(&self) -> String {
        self.docs_book
            .clone()
            .filter(|b| ostra_core::book::is_book_id(b))
            .unwrap_or_else(|| ostra_core::book::book_id(&self.docs_projects()))
    }

    fn docs_drafts_dir(&self, project: &str) -> PathBuf {
        self.session_root
            .join(paths::report::docs_drafts())
            .join(project)
    }

    fn docs_draft_path(&self, project: &str, page: &str) -> PathBuf {
        self.docs_drafts_dir(project).join(format!("{page}.md"))
    }

    fn docs_inventory_path(&self, project: &str) -> PathBuf {
        self.docs_drafts_dir(project).join("inventory.md")
    }
}
