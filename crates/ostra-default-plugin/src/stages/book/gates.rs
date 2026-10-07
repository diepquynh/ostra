//! How the book stage's gates open and how their answers apply: a failed docs run, and the
//! `DocsRounds` gate (Rule B10).

use crate::book::DocsTrack;
use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::ExecPurpose;
use ostra_core::ids::GateId;
use ostra_engine::state::*;

/// How the book stage's gates open and how their answers apply.
pub trait BookGates {
    /// The failure gate of a docs run opened.
    fn docs_exec_gate(&mut self, purpose: &ExecPurpose, gate: &GateId);

    /// The user answered the failure gate of a docs run.
    fn docs_exec_answered(&mut self, purpose: &ExecPurpose, retry: bool);

    fn docs_rounds_opened(&mut self, id: &GateId, project: &str);

    fn docs_rounds_answered(&mut self, project: &str, rounds: u32, choice: Choice<'_>);
}

impl BookGates for SessionState {
    fn docs_exec_gate(&mut self, purpose: &ExecPurpose, gate: &GateId) {
        match purpose {
            ExecPurpose::Docs {
                project,
                page,
                round,
            } => {
                if let DocsState::Failed { gate: g, .. } = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_run(page.as_deref(), *round)
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::DocsSurvey { project } => {
                if let DocsState::Failed { gate: g, .. } = &mut self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .survey
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::DocsCheck {
                project,
                page,
                round,
            } => {
                if let Some(CheckState::Failed { gate: g, .. }) = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .checks
                    .get_mut(page)
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::DocsSynthesis { project, round } => {
                if let DocsState::Failed { gate: g, .. } = &mut self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .synthesis
                {
                    *g = Some(gate.clone());
                }
            }
            _ => {}
        }
    }

    fn docs_exec_answered(&mut self, purpose: &ExecPurpose, retry: bool) {
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
                    .docs_run(page.as_deref(), *round) = if retry {
                    DocsState::NotStarted
                } else {
                    DocsState::Abandoned
                };
            }
            ExecPurpose::DocsSurvey { project } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .survey = if retry {
                    DocsState::NotStarted
                } else {
                    DocsState::Abandoned
                };
            }
            ExecPurpose::DocsCheck {
                project,
                page,
                round,
            } => {
                let st = if retry {
                    CheckState::NotStarted
                } else {
                    CheckState::Abandoned
                };
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .checks
                    .insert(page.clone(), st);
            }
            ExecPurpose::DocsSynthesis { project, round } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .synthesis = if retry {
                    DocsState::NotStarted
                } else {
                    DocsState::Abandoned
                };
            }
            _ => {}
        }
    }

    fn docs_rounds_opened(&mut self, id: &GateId, project: &str) {
        self.ext
            .os_mut()
            .project_tracks
            .entry(project.to_string())
            .or_default()
            .docs_gate = Some(id.clone());
    }

    fn docs_rounds_answered(&mut self, project: &str, rounds: u32, choice: Choice<'_>) {
        // Rule B10: another round, or the book as it is.
        let t = self
            .ext
            .os_mut()
            .project_tracks
            .entry(project.to_string())
            .or_default();
        t.docs_gate = None;
        match choice {
            Some(("accept", _)) => t.docs_accepted = true,
            _ => t.docs_continued = rounds,
        }
    }
}
