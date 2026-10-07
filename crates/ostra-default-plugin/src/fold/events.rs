//! The events only the built-in stages read.

use super::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::{CommandPurpose, SessionEvent, StoredEvent};
use ostra_core::submit::Severity;
use ostra_engine::state::*;

pub trait OstraEvents {
    /// The fold of the events only the built-in stages read.
    fn fold_event(&mut self, stored: &StoredEvent);
}

impl OstraEvents for SessionState {
    fn fold_event(&mut self, stored: &StoredEvent) {
        match &stored.event {
            SessionEvent::CommandRan {
                purpose,
                project,
                exit_code,
                ..
            } => {
                if let Some(t) = self.ext.os_mut().project_tracks.get_mut(project) {
                    t.running = None;
                }
                match purpose {
                    CommandPurpose::Format => {
                        self.ext
                            .os_mut()
                            .project_tracks
                            .entry(project.clone())
                            .or_default()
                            .format = Some(*exit_code);
                    }
                    CommandPurpose::Stage => {
                        let key = self
                            .ext
                            .os()
                            .phases
                            .iter()
                            .flat_map(|(id, p)| {
                                [((*id, false), &p.impl_loop), ((*id, true), &p.test_loop)]
                            })
                            .find(|(k, l)| {
                                self.ext
                                    .os()
                                    .phases
                                    .get(&k.0)
                                    .is_some_and(|p| &p.info.project == project)
                                    && l.next == LoopNext::Stage
                            })
                            .map(|(k, _)| k);
                        if let Some(key) = key
                            && let Some(l) = self.loop_mut(key)
                        {
                            l.next = LoopNext::Done;
                            l.staged = *exit_code == Some(0);
                        }
                    }
                    CommandPurpose::Autofix => {}
                }
            }
            SessionEvent::AutofixApplied {
                phase,
                tests,
                failed,
                ..
            } => {
                let yolo = self.yolo;
                let project = self
                    .ext
                    .os()
                    .phases
                    .get(phase)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let ledger = self
                    .ledger_path(&project, *phase, *tests)
                    .display()
                    .to_string();
                if let Some(l) = self.loop_mut((*phase, *tests))
                    && let LoopNext::Autofix { apply, remaining } = l.next.clone()
                {
                    let mut rem = remaining;
                    for (rule_line, _) in failed {
                        if let Some(f) = apply.iter().find(|f| &f.line() == rule_line)
                            && matches!(f.severity, Severity::High | Severity::Medium)
                        {
                            rem.push(f.clone());
                        }
                    }
                    l.after_findings(rem, yolo, &ledger);
                }
            }
            SessionEvent::SecurityBlock { .. } => {}
            SessionEvent::PhaseBlocked {
                phase,
                tests,
                reason,
                ..
            } => {
                if let Some(l) = self.loop_mut((*phase, *tests)) {
                    l.announced_block = true;
                    if !l.is_blocked() {
                        l.next = LoopNext::Blocked {
                            reason: reason.clone(),
                        };
                    }
                }
            }

            SessionEvent::CommandStarted {
                purpose,
                project,
                command,
            } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .running = Some((*purpose, command.clone()));
            }
            SessionEvent::DocsPlanned { .. } => {}
            SessionEvent::DocsScanned {
                project,
                modules,
                refs,
            } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_scan = Some(DocsScan {
                    modules: modules.clone(),
                    refs: refs.clone(),
                });
            }
            SessionEvent::ProjectInitFinished { project } => {
                if let Some(i) = self.ext.os_mut().project_inits.get_mut(project) {
                    i.finished = true;
                    i.note = None;
                }
            }
            SessionEvent::InitStepFailed {
                project,
                execution,
                error,
            } => {
                if let Some(i) = self.ext.os_mut().project_inits.get_mut(project) {
                    clear_init_result(i, execution);
                    i.failed = Some((execution.clone(), error.clone()));
                }
            }
            _ => {}
        }
    }
}
