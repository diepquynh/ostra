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

    /// Rule B10: the log ran a docs pipeline per project, as logs did before the session-wide
    /// pipeline, so the session keeps that path.
    fn docs_per_project(&self) -> bool;

    /// Rule B10: the docs pipeline a run's key names: the session's for `_session`, else the
    /// project's own.
    fn docs_pipe(&self, key: &str) -> Option<&crate::data::DocsPipeline>;

    fn docs_pipe_mut(&mut self, key: &str) -> &mut crate::data::DocsPipeline;

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

    fn docs_per_project(&self) -> bool {
        // A log that recorded no workflow is from before the session-wide pipeline.
        self.workflow.is_none()
            || self.ext.os().project_tracks.values().any(|t| {
                let b = &t.book;
                !matches!(b.docs, DocsState::NotStarted)
                    || !matches!(b.survey, DocsState::NotStarted)
                    || !b.page_docs.is_empty()
                    || !b.docs_rounds.is_empty()
                    || b.docs_gate.is_some()
                    || b.docs_accepted
                    || b.docs_scan.as_ref().is_some_and(|s| !s.session_wide)
            })
    }

    fn docs_pipe(&self, key: &str) -> Option<&crate::data::DocsPipeline> {
        if key == ostra_core::book::SESSION_DOCS {
            return Some(&self.ext.os().session_book);
        }
        self.ext.os().project_tracks.get(key).map(|t| &t.book)
    }

    fn docs_pipe_mut(&mut self, key: &str) -> &mut crate::data::DocsPipeline {
        let os = self.ext.os_mut();
        if key == ostra_core::book::SESSION_DOCS {
            return &mut os.session_book;
        }
        &mut os.project_tracks.entry(key.to_string()).or_default().book
    }

    fn book_update(&self) -> ostra_core::book::BookUpdate {
        if !self.docs_per_project() {
            // Rule B11: one session-wide pipeline, its pages split into the book's parts.
            let t = &self.ext.os().session_book;
            let parts = match t.docs_aggregate() {
                DocsState::Done(d) => {
                    let plan: Vec<_> = t
                        .survey_plan()
                        .map(|survey| {
                            survey
                                .pages
                                .iter()
                                .map(|p| {
                                    let draft =
                                        t.page_docs.get(&p.id).and_then(|d| d.draft.clone());
                                    (p.clone(), if p.rewrite { draft } else { None })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    crate::book::session_parts(&d, &plan, &self.docs_projects())
                }
                _ => vec![],
            };
            return ostra_core::book::BookUpdate {
                session: self.id.to_string(),
                parts,
            };
        }
        // Rule B10: a log that ran the pipeline per project writes one part per project.
        let parts = self
            .ext
            .os()
            .project_tracks
            .iter()
            .filter_map(|(k, t)| match t.book.docs_aggregate() {
                DocsState::Done(d) => {
                    // Rule B10: every planned page, with the draft this session wrote or `None`
                    // to keep it.
                    let t = &t.book;
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
