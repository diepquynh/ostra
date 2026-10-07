//! Rules B5 to B10: the docs stage and the book write.

use crate::book::{DOCS_ROUNDS, DocsTrack};
use crate::inputs::OstraInputs;
use crate::judge::NoteStage;
use crate::planner::*;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::steps::OstraStep;
use ostra_core::Contract;
use ostra_core::agent::AgentName;
use ostra_core::book::DocsStep;
use ostra_core::event::{ExecPurpose, GatePayload};
use ostra_core::workflow::BuiltinStage;
use ostra_engine::plan::*;
use ostra_engine::state::*;

pub struct BookProgress {
    /// Projects whose part of the book is written, sorted.
    pub(crate) parts: Vec<String>,
}

/// Rule B10: the session's workflow has a book stage, so the closing stage writes no docs. A
/// session that recorded no workflow keeps the docs in its closing stage, as it started.
pub fn book_node(s: &SessionState) -> bool {
    s.workflow
        .as_ref()
        .is_some_and(|w| w.has_builtin(BuiltinStage::Book))
}

/// Rule B10: the closing gate chose docs for `project`, or the request asked for tests and docs,
/// and no BLOCKER is open (Hard rule 21). `None` while the choice is not made.
pub fn docs_chosen(s: &SessionState, project: &str) -> Option<bool> {
    let track = s.ext.os().project_tracks.get(project)?;
    let passed = passed_phases(s, project);
    if passed.is_empty() {
        return Some(false);
    }
    let docs_on = match track.closing {
        Some(c) => c.1,
        None if s.tests_requested() && s.docs_requested() => true,
        None => return None,
    };
    Some(docs_on && !blocker_open(&passed))
}

/// Rules B5 to B10: the docs stage and the book write.
pub trait PlannerBook<'a> {
    fn docs_stage(&mut self, project: &str, passed: &[&PhaseRun]);

    /// One run of the docs stage: start it under the cap, or open the failure gate.
    fn docs_run<T>(
        &mut self,
        project: &str,
        state: &crate::data::StageRun<T>,
        agent: AgentName,
        purpose: ExecPurpose,
        inputs: SpawnInputs,
    );

    /// Where the book stands once every project's closing stages are known. `None` while a
    /// project may still get documentation.
    fn book_progress(&self) -> Option<BookProgress>;

    /// Rule B5: after every part is written, the book write.
    fn book_stage(&mut self);

    /// Rule B10: the book stage: the docs stage of each project the closing gate chose docs
    /// for, then the book write. Returns `true` when it is done.
    fn book_flow(&mut self) -> bool;
}

impl<'a> PlannerBook<'a> for Planner<'a> {
    fn docs_stage(&mut self, project: &str, passed: &[&PhaseRun]) {
        let s = self.s;
        let track = &s.ext.os().project_tracks[project];
        // Hard rule 21: an open BLOCKER blocks documentation.
        if blocker_open(passed) {
            return;
        }
        let base = SpawnInputs {
            user_notes: s.notes_for(NoteStage::Docs),
            extra: OstraInputs {
                implementer_reports: passed
                    .iter()
                    .filter_map(|p| p.implementer_report.clone())
                    .collect(),
                ..Default::default()
            }
            .into_value(),
            ..Default::default()
        };
        let stage = if book_node(s) {
            BuiltinStage::Book
        } else {
            BuiltinStage::Closing
        };
        let writer = s.agent_for(stage, Contract::Documentation);
        let docs = |page: Option<&str>, round: u32| ExecPurpose::Docs {
            project: project.to_string(),
            page: page.map(str::to_string),
            round,
        };
        // Rule B10: a log from before the pipeline keeps its whole-part writer.
        if !matches!(track.docs, DocsState::NotStarted) {
            self.docs_run(project, &track.docs, writer, docs(None, 0), base);
            return;
        }
        // Rule B10: scan the modules and constants, survey, write a first draft of each page, then
        // synthesis rounds until done. A log whose survey started before the scan keeps going.
        if track.docs_scan.is_none() && matches!(track.survey, DocsState::NotStarted) {
            self.push(Step::from(OstraStep::ScanDocs {
                project: project.to_string(),
            }));
            return;
        }
        let base = SpawnInputs {
            extra: OstraInputs {
                docs_reference: track
                    .docs_scan
                    .as_ref()
                    .map(|_| s.docs_drafts_dir(project).join("reference.md")),
                ..OstraInputs::of(&base)
            }
            .into_value(),
            ..base
        };
        let Some(survey) = track.survey_plan() else {
            let inputs = SpawnInputs {
                extra: OstraInputs {
                    docs_step: Some(DocsStep::Survey),
                    ..OstraInputs::of(&base)
                }
                .into_value(),
                ..base
            };
            let purpose = ExecPurpose::DocsSurvey {
                project: project.to_string(),
            };
            self.docs_run(project, &track.survey, writer, purpose, inputs);
            return;
        };
        let planned = track.planned_pages();
        let inventory = track.current_inventory();
        let page_inputs =
            |page: &ostra_core::book::PlannedPage, instructions: Vec<String>| SpawnInputs {
                extra: OstraInputs {
                    docs_step: Some(DocsStep::Page),
                    docs_page: Some(page.clone()),
                    docs_pages: survey.pages.clone(),
                    docs_inventory: inventory
                        .iter()
                        .filter(|i| i.owner == page.id)
                        .map(|i| {
                            let mut line = format!("{} ({})", i.name, i.sources.join(", "));
                            if !i.settings.is_empty() {
                                line.push_str(&format!(", settings: {}", i.settings.join(", ")));
                            }
                            line
                        })
                        .collect(),
                    docs_instructions: instructions,
                    ..OstraInputs::of(&base.clone())
                }
                .into_value(),
                ..base.clone()
            };
        for page in &planned {
            let Some(d) = track.page_docs.get(&page.id) else {
                continue;
            };
            self.docs_run(
                project,
                &d.run,
                writer,
                docs(Some(&page.id), 0),
                page_inputs(page, vec![]),
            );
        }
        if !track.first_drafts_settled() || track.docs_finished() {
            return;
        }
        let done_rounds = track.docs_rounds.len() as u32;
        let last = track.docs_rounds.last();
        // The round in progress, or the next one when the last round's revisions all settled.
        let round = match last {
            Some(r)
                if !r.synthesis.is_settled()
                    || !r
                        .targets
                        .keys()
                        .all(|p| r.revisions.get(p).is_some_and(|x| x.is_settled())) =>
            {
                done_rounds
            }
            _ => done_rounds + 1,
        };
        let current = track.docs_rounds.get(round as usize - 1);
        // 1. Fact-check each page that changed.
        let checker = s.agent_for(stage, Contract::FactCheck);
        let to_check = track.pages_to_check(round);
        let mut checking = false;
        for page in &to_check {
            let state = current
                .and_then(|r| r.checks.get(page))
                .cloned()
                .unwrap_or(crate::data::StageRun::NotStarted);
            checking |= !state.is_settled();
            let prior = (1..round)
                .rev()
                .find_map(
                    |k| match track.docs_rounds[k as usize - 1].checks.get(page) {
                        Some(crate::data::StageRun::Done(c)) => Some(c.findings_text()),
                        _ => None,
                    },
                )
                .unwrap_or_else(|| "none".into());
            let inputs = SpawnInputs {
                target: Some(s.docs_draft_path(project, page)),
                spec_file: Some(s.docs_inventory_path(project)),
                prior_findings: Some(prior),
                extra: OstraInputs {
                    docs_check: true,
                    source_check: Some("citations".into()),
                    ..Default::default()
                }
                .into_value(),
                ..Default::default()
            };
            let purpose = ExecPurpose::DocsCheck {
                project: project.to_string(),
                page: page.clone(),
                round,
            };
            self.docs_run(project, &state, checker, purpose, inputs);
        }
        if checking {
            return;
        }
        // 2. One synthesis pass over every draft, with the findings and the engine's checks.
        let synthesis = current
            .map(|r| r.synthesis.clone())
            .unwrap_or(DocsState::NotStarted);
        if !matches!(synthesis, DocsState::Done(_)) {
            let mut findings: Vec<String> = current
                .map(|r| {
                    r.checks
                        .iter()
                        .filter_map(|(page, c)| match c {
                            crate::data::StageRun::Done(c) => {
                                Some(format!("{page}: {:?}, {}", c.verdict, c.findings_text()))
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            for (page, issues) in track.engine_issues() {
                let page = if page.is_empty() {
                    "inventory".to_string()
                } else {
                    page
                };
                findings.push(format!("{page} (engine check): {}", issues.join(" ")));
            }
            // Rule B10: the named constants that no page mentions, for the coverage check.
            if let Some(scan) = &track.docs_scan {
                let drafts = track.placed_drafts();
                let missing = crate::book::unmentioned_refs(&drafts, &scan.refs);
                if !missing.is_empty() {
                    let shown: Vec<String> = missing
                        .iter()
                        .take(80)
                        .map(|r| format!("`{}` ({})", r.name, r.file))
                        .collect();
                    findings.push(format!(
                        "reference (engine check): {} of {} named constants appear on no page: {}{}",
                        missing.len(),
                        scan.refs.len(),
                        shown.join(", "),
                        if missing.len() > 80 { ", and more in reference.md" } else { "" }
                    ));
                }
            }
            let inputs = SpawnInputs {
                extra: OstraInputs {
                    docs_step: Some(DocsStep::Synthesis),
                    docs_pages: survey.pages.clone(),
                    docs_instructions: findings,
                    docs_round: round,
                    ..OstraInputs::of(&base)
                }
                .into_value(),
                ..base
            };
            let purpose = ExecPurpose::DocsSynthesis {
                project: project.to_string(),
                round,
            };
            self.docs_run(project, &synthesis, writer, purpose, inputs);
            return;
        }
        let Some(r) = current else {
            return;
        };
        // 3. Rule B10: after each DOCS_ROUNDS rounds without done, the user decides first.
        if round % DOCS_ROUNDS == 0 && track.docs_continued < round {
            if track.docs_gate.is_none() {
                let open: Vec<String> = match &r.synthesis {
                    DocsState::Done(syn) => syn
                        .checks
                        .iter()
                        .filter(|c| !c.passed)
                        .map(|c| format!("{}: {}", c.check, c.note))
                        .collect(),
                    _ => vec![],
                };
                self.gate(
                    format!("Documentation for {project} is not done after {round} rounds"),
                    format!(
                        "{round} synthesis rounds ran, and {} pages still need changes. Run another round, or accept the book as it is.",
                        r.targets.len()
                    ),
                    GatePayload::DocsRounds {
                        project: project.to_string(),
                        rounds: round,
                        open,
                    },
                );
            }
            return;
        }
        // 4. Revise each page that the round names.
        for (page_id, instructions) in &r.targets {
            let Some(page) = planned.iter().find(|p| &p.id == page_id) else {
                continue;
            };
            let state = r
                .revisions
                .get(page_id)
                .cloned()
                .unwrap_or(DocsState::NotStarted);
            let mut inputs = page_inputs(page, instructions.clone());
            inputs.target = Some(s.docs_draft_path(project, page_id));
            self.docs_run(project, &state, writer, docs(Some(page_id), round), inputs);
        }
    }

    fn docs_run<T>(
        &mut self,
        project: &str,
        state: &crate::data::StageRun<T>,
        agent: AgentName,
        purpose: ExecPurpose,
        inputs: SpawnInputs,
    ) {
        let s = self.s;
        match state {
            crate::data::StageRun::NotStarted => {
                // Rule B7: docs runs fan out at once, and the workspace's slot limit bounds them.
                self.spawn(
                    agent,
                    purpose,
                    project,
                    s.project_session_dir(project),
                    inputs,
                );
            }
            crate::data::StageRun::Failed {
                exec,
                error,
                gate: None,
                ..
            } => {
                let (exec, error) = (exec.clone(), error.clone());
                self.exec_failed_gate(&exec, agent, project, &error);
            }
            _ => {}
        }
    }

    fn book_progress(&self) -> Option<BookProgress> {
        let s = self.s;
        let removed = removed_phases(s);
        let mut parts = vec![];
        for key in s.ext.os().project_tracks.keys() {
            if !self.project_code_done(key, &removed) {
                return None;
            }
            let passed: Vec<&PhaseRun> = self
                .project_phases(key)
                .into_iter()
                .filter(|p| p.impl_loop.is_done())
                .collect();
            if passed.is_empty() {
                continue;
            }
            let track = &s.ext.os().project_tracks[key];
            let docs_on = match track.closing {
                Some(c) => c.1,
                None if s.tests_requested() && s.docs_requested() => true,
                None => return None,
            };
            if !docs_on || blocker_open(&passed) {
                continue;
            }
            match track.docs_aggregate() {
                DocsState::Done(_) => parts.push(key.clone()),
                DocsState::Abandoned => {}
                _ => return None,
            }
        }
        Some(BookProgress { parts })
    }

    fn book_stage(&mut self) {
        let s = self.s;
        let Some(progress) = self.book_progress() else {
            return;
        };
        if progress.parts.is_empty() || s.book_written.is_some() {
            return;
        }
        // Rule B6: the book is named after the parts written, so an abandoned part names no book.
        let book = s
            .docs_book
            .clone()
            .filter(|b| ostra_core::book::is_book_id(b))
            .unwrap_or_else(|| ostra_core::book::book_id(&progress.parts));
        self.push(Step::WriteBook { book });
    }

    fn book_flow(&mut self) -> bool {
        let s = self.s;
        // A session without a recorded workflow wrote its docs in the closing stage.
        if !book_node(s) {
            return true;
        }
        let mut settled = true;
        for key in s.ext.os().project_tracks.keys() {
            if docs_chosen(s, key) != Some(true) {
                continue;
            }
            let passed = passed_phases(s, key);
            self.docs_stage(key, &passed);
            settled &= s.ext.os().project_tracks[key].docs_aggregate().is_settled();
        }
        self.book_stage();
        settled
            && match self.book_progress() {
                Some(p) if !p.parts.is_empty() => s.book_written.is_some(),
                Some(_) => true,
                None => false,
            }
    }
}
