//! How a docs-stage run that starts or ends changes the book stage (Rule B10).

use crate::book::DocsTrack;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::book::{DocsStep, DocumentationSubmit};
use ostra_core::event::ExecPurpose;
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::ExecutionId;
use ostra_core::submit::FactCheckSubmit;
use ostra_engine::state::*;
use std::collections::{BTreeMap, BTreeSet};

/// A docs-stage run ends done only with a submit the engine can read, because the book is built
/// from it.
pub fn stage_run<T>(
    status: ExecutionStatus,
    submit: Option<T>,
    exec: &ExecutionId,
    error: String,
) -> StageRun<T> {
    match (status, submit) {
        (ExecutionStatus::Ok, Some(t)) => StageRun::Done(Box::new(t)),
        (ExecutionStatus::Interrupted, _) => StageRun::NotStarted,
        (ExecutionStatus::Ok, None) => StageRun::Failed {
            exec: exec.clone(),
            error: "The run ended without a readable submit call, so there is nothing to put in the book.".into(),
            gate: None,
            retries: 1,
        },
        _ => StageRun::Failed {
            exec: exec.clone(),
            error,
            gate: None,
            retries: 1,
        },
    }
}

/// Rule B10: a docs run that answered another step of the pipeline failed.
pub fn expect_step(run: DocsState, step: DocsStep, exec: &ExecutionId) -> DocsState {
    match run {
        DocsState::Done(d) if d.step != step => DocsState::Failed {
            exec: exec.clone(),
            error: format!(
                "The run answered the `{}` step, but it was started for the `{}` step. Call the submit tool again with `step: {}` and the fields of that step.",
                d.step.as_str(),
                step.as_str(),
                step.as_str()
            ),
            gate: None,
            retries: 1,
        },
        other => other,
    }
}

/// How a docs-stage run that starts or ends changes the book stage.
pub trait BookRuns {
    /// A run of `Docs`, `DocsSurvey`, `DocsCheck`, or `DocsSynthesis` started.
    fn docs_started(&mut self, id: &ExecutionId, purpose: &ExecPurpose);

    /// A run of `Docs`, `DocsSurvey`, `DocsCheck`, or `DocsSynthesis` ended.
    fn docs_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult, error: String);
}

impl BookRuns for SessionState {
    fn docs_started(&mut self, id: &ExecutionId, purpose: &ExecPurpose) {
        match purpose {
            ExecPurpose::Docs {
                project,
                page,
                round,
            } => {
                *self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_run(page.as_deref(), *round) = DocsState::Running(id.clone());
            }
            ExecPurpose::DocsSurvey { project } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .survey = DocsState::Running(id.clone());
            }
            ExecPurpose::DocsCheck {
                project,
                page,
                round,
            } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .checks
                    .insert(page.clone(), CheckState::Running(id.clone()));
            }
            ExecPurpose::DocsSynthesis { project, round } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .synthesis = DocsState::Running(id.clone());
            }
            _ => {}
        }
    }

    fn docs_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult, error: String) {
        let status = result.status;
        match &rec.purpose {
            ExecPurpose::Docs {
                project,
                page,
                round,
            } => {
                let t = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default();
                let mut run = stage_run(
                    status,
                    parse::<DocumentationSubmit>(&result.submit),
                    &rec.id,
                    error,
                );
                if page.is_some() {
                    run = expect_step(run, DocsStep::Page, &rec.id);
                }
                if let (Some(p), DocsState::Done(sub)) = (page, &run) {
                    // Rule B10: a first draft or a revision replaces the page's draft.
                    let d = t.page_docs.entry(p.clone()).or_default();
                    d.previous = d.draft.take();
                    d.draft = sub.sections.first().cloned();
                    d.glossary = sub.glossary.clone();
                }
                *t.docs_run(page.as_deref(), *round) = run;
            }
            ExecPurpose::DocsSurvey { project } => {
                let t = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default();
                t.survey = expect_step(
                    stage_run(
                        status,
                        parse::<DocumentationSubmit>(&result.submit),
                        &rec.id,
                        error,
                    ),
                    DocsStep::Survey,
                    &rec.id,
                );
                // Rule B10: each page the survey plans to write gets a writer.
                t.page_docs = match &t.survey {
                    DocsState::Done(sv) => sv
                        .pages
                        .iter()
                        .filter(|p| p.rewrite)
                        .map(|p| (p.id.clone(), PageDraft::default()))
                        .collect(),
                    _ => BTreeMap::new(),
                };
                t.docs_rounds.clear();
            }
            ExecPurpose::DocsCheck {
                project,
                page,
                round,
            } => {
                let run = stage_run(
                    status,
                    parse::<FactCheckSubmit>(&result.submit),
                    &rec.id,
                    error,
                );
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .checks
                    .insert(page.clone(), run);
            }
            ExecPurpose::DocsSynthesis { project, round } => {
                let t = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default();
                let run = expect_step(
                    stage_run(
                        status,
                        parse::<DocumentationSubmit>(&result.submit),
                        &rec.id,
                        error,
                    ),
                    DocsStep::Synthesis,
                    &rec.id,
                );
                // Rule B10: the pages to revise are fixed now: the pass's edits, each failed
                // fact-check, each owner of an inventory item the pass added, and each page the
                // engine's own checks name.
                let mut targets: BTreeMap<String, Vec<String>> = BTreeMap::new();
                if let DocsState::Done(syn) = &run {
                    let pages: BTreeSet<String> = t.page_docs.keys().cloned().collect();
                    for item in &syn.inventory {
                        let owned = pages.contains(&item.owner);
                        if !owned && item.out_of_scope.is_none() {
                            continue;
                        }
                        t.inventory_added.retain(|i| i.id != item.id);
                        t.inventory_added.push(item.clone());
                        if owned {
                            targets.entry(item.owner.clone()).or_default().push(format!(
                                "Cover the new inventory item `{}` ({}), from {}.",
                                item.id,
                                item.name,
                                item.sources.join(", ")
                            ));
                        }
                    }
                    for e in &syn.edits {
                        if pages.contains(&e.page) {
                            targets
                                .entry(e.page.clone())
                                .or_default()
                                .extend(e.instructions.clone());
                        }
                    }
                    for (page, check) in &t.round_mut(*round).checks {
                        if let CheckState::Done(c) = check
                            && c.verdict != ostra_core::submit::Verdict::Pass
                        {
                            targets.entry(page.clone()).or_default().push(format!(
                                "Fix each fact-check finding: {}",
                                c.findings_text()
                            ));
                        }
                    }
                    for (page, issues) in t.engine_issues() {
                        if pages.contains(&page) {
                            targets.entry(page).or_default().extend(issues);
                        }
                    }
                }
                let r = t.round_mut(*round);
                r.synthesis = run;
                r.targets = targets;
            }
            _ => {}
        }
    }
}
