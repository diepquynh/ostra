//! Rules D8 and T1 to T7: format, the closing gate, and the tests.

use crate::book::DocsTrack;
use crate::judge::NoteStage;
use crate::planner::*;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::steps::OstraStep;
use ostra_core::Contract;
use ostra_core::event::{ClosingItem, CommandPurpose, ExecPurpose, GatePayload, WorkKind};
use ostra_core::paths::report;
use ostra_core::pipeline::TestPolicy;
use ostra_core::workflow::BuiltinStage;
use ostra_engine::plan::*;

/// Rules D8 and T1 to T7: format, the closing gate, and the tests.
pub trait PlannerClosing<'a> {
    fn closing_stages(&mut self);

    /// Returns true when the project's test stage is finished.
    fn test_stage(&mut self, project: &str, passed: &[&'a PhaseRun]) -> bool;

    fn all_implement_done(&self) -> bool;
}

impl<'a> PlannerClosing<'a> for Planner<'a> {
    fn closing_stages(&mut self) {
        let s = self.s;
        let removed = removed_phases(s);
        let projects: Vec<String> = s.ext.os().project_tracks.keys().cloned().collect();
        let mut closing_items = vec![];
        let mut tested: Vec<&String> = vec![];
        for key in &projects {
            if !self.project_code_done(key, &removed) {
                continue;
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
            // Rule D8: format runs once per project after its last phase, not gated.
            if track.format.is_none() {
                let command = self.ctx.format_commands.get(key).cloned().flatten();
                self.push(Step::from(OstraStep::Command {
                    purpose: CommandPurpose::Format,
                    project: key.clone(),
                    command,
                    files: vec![],
                }));
                continue;
            }
            let closing = match track.closing {
                Some(c) => c,
                None => {
                    // Rule T3: an explicit request replaces the gate.
                    let (ask_tests, ask_docs) = (!s.tests_requested(), !s.docs_requested());
                    if !ask_tests && !ask_docs {
                        (true, true)
                    } else {
                        if track.closing_gate.is_none() {
                            closing_items.push(ClosingItem {
                                project: key.clone(),
                                phases: passed.len() as u32,
                                ask_tests,
                                ask_docs,
                            });
                        }
                        continue;
                    }
                }
            };
            let tests_done = if closing.0 {
                self.test_stage(key, &passed)
            } else {
                true
            };
            if tests_done {
                tested.push(key);
            }
            // Rule B10: a workflow with a book stage writes the docs there, after closing.
            if closing.1 && tests_done && !book_node(s) && s.docs_per_project() {
                self.docs_stage(key, &passed);
            }
        }
        // Rule B10: without a book stage, the session-wide docs start when every documented
        // project's tests are done.
        if !book_node(s)
            && !s.docs_per_project()
            && let Some(docs) = self.docs_choices()
            && !docs.is_empty()
            && docs.iter().all(|k| tested.contains(&k))
        {
            self.docs_session_stage(&docs);
        }
        if !closing_items.is_empty() {
            // Rule T6: projects reaching the gate together are asked in one batch.
            let n: u32 = closing_items.iter().map(|i| i.phases).sum();
            self.gate(
                "Tests and documentation",
                format!(
                    "All {n} phases are implemented and reviewed. Writing tests and writing the documentation book are optional. Neither changes the requirements (Rule T5)."
                ),
                GatePayload::ClosingGate { items: closing_items },
            );
        }
        if !book_node(s) {
            self.book_stage();
        }
    }

    fn test_stage(&mut self, project: &str, passed: &[&'a PhaseRun]) -> bool {
        let s = self.s;
        // Rule T4: Required phases are covered; no plan means the whole change is covered.
        let covered: Vec<&PhaseRun> = passed
            .iter()
            .copied()
            .filter(|p| p.info.file.is_none() || p.info.test_policy == TestPolicy::Required)
            .collect();
        let mut all_epa_done = true;
        for p in &covered {
            match &p.epa {
                EpaState::NotStarted => {
                    all_epa_done = false;
                    // Rule WD2: a phase in several projects gets one analysis, from its main project.
                    let dir = s.project_session_dir(&p.info.project);
                    let inputs = SpawnInputs {
                        phase: Some(p.info.clone()),
                        implementer_report: p.implementer_report.clone(),
                        report_file: Some(dir.join(report::epa(&p.info.id.to_string()))),
                        user_notes: s.notes_for(NoteStage::Tests),
                        ..Default::default()
                    };
                    self.spawn(
                        s.agent_for(BuiltinStage::Closing, Contract::PathAnalysis),
                        ExecPurpose::Epa { phase: p.info.id },
                        &p.info.project,
                        dir,
                        inputs,
                    );
                }
                EpaState::Running(_) => all_epa_done = false,
                EpaState::Failed {
                    exec, error, gate, ..
                } => {
                    all_epa_done = false;
                    if gate.is_none() {
                        self.exec_failed_gate(
                            exec,
                            s.agent_for(BuiltinStage::Closing, Contract::PathAnalysis),
                            project,
                            error,
                        );
                    }
                }
                EpaState::Done(_) | EpaState::Abandoned => {}
            }
        }
        if !all_epa_done {
            return false;
        }
        // Analyze in parallel, write serially, in phase order (Rule T4).
        for p in &covered {
            if matches!(p.epa, EpaState::Abandoned) {
                continue;
            }
            let l = &p.test_loop;
            if l.is_blocked() {
                // Rule D9: a blocked test phase is announced, and asked about outside YOLO, before
                // the completion report, which waits for the announcement.
                self.loop_steps(p, true);
                if l.announced_block && (s.yolo || l.block_gate_answered) {
                    continue;
                }
                return false;
            }
            if l.is_terminal() {
                continue;
            }
            if l.is_idle() {
                self.loop_work(p, true, WorkKind::Initial, None);
            } else {
                self.loop_steps(p, true);
            }
            return false;
        }
        true
    }

    fn all_implement_done(&self) -> bool {
        let s = self.s;
        if !self.nothing_running() {
            return false;
        }
        let removed = removed_phases(s);
        for key in s.ext.os().project_tracks.keys() {
            if !self.project_code_done(key, &removed) {
                return false;
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
            if track.format.is_none() {
                return false;
            }
            let Some((tests, docs)) = track
                .closing
                .or_else(|| (s.tests_requested() && s.docs_requested()).then_some((true, true)))
            else {
                return false;
            };
            if tests {
                let covered = passed.iter().filter(|p| {
                    p.info.file.is_none() || p.info.test_policy == TestPolicy::Required
                });
                for p in covered {
                    let epa_ok = matches!(p.epa, EpaState::Done(_) | EpaState::Abandoned);
                    if !epa_ok
                        || (!matches!(p.epa, EpaState::Abandoned) && !p.test_loop.is_terminal())
                    {
                        return false;
                    }
                }
            }
            if docs
                && !book_node(s)
                && !blocker_open(&passed)
                && !if s.docs_per_project() {
                    track.book.docs_aggregate().is_settled()
                } else {
                    s.ext.os().session_book.docs_aggregate().is_settled()
                }
            {
                return false;
            }
        }
        if book_node(s) {
            return true;
        }
        match self.book_progress() {
            Some(p) if !p.parts.is_empty() => s.book_written.is_some(),
            Some(_) => true,
            None => false,
        }
    }
}
