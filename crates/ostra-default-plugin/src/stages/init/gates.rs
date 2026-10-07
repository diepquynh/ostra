//! How the init flow's gates open and how their answers apply: the skill approval, and a failed
//! initializer step.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::GateAnswer;
use ostra_core::ids::GateId;
use ostra_engine::state::*;

/// How the init flow's gates open and how their answers apply.
pub trait InitGates {
    fn skill_approval_opened(&mut self, id: &GateId, project: &str);

    fn skill_approval_answered(&mut self, project: &str, answer: &GateAnswer);

    fn init_exec_gate(&mut self, project: &str, gate: &GateId);

    fn init_exec_answered(&mut self, project: &str, retry: bool);
}

impl InitGates for SessionState {
    fn skill_approval_opened(&mut self, id: &GateId, project: &str) {
        if let Some(i) = self.init_track_mut(project) {
            i.approval_gate = Some(id.clone());
        }
    }

    fn skill_approval_answered(&mut self, project: &str, answer: &GateAnswer) {
        if let (Some(i), GateAnswer::Skills { decisions }) = (self.init_track_mut(project), answer)
        {
            i.approval_gate = None;
            i.decisions = Some(
                decisions
                    .iter()
                    .map(|d| (d.name.clone(), d.disposition.clone()))
                    .collect(),
            );
        }
    }

    fn init_exec_gate(&mut self, project: &str, gate: &GateId) {
        if let Some(i) = self.init_track_mut(project) {
            i.failed_gate = Some(gate.clone());
        }
    }

    fn init_exec_answered(&mut self, project: &str, retry: bool) {
        let embedded = self.ext.os().project_inits.contains_key(project);
        let mut abandoned = false;
        if let Some(i) = self.init_track_mut(project) {
            i.failed_gate = None;
            if retry {
                i.escalated = false;
                let key = i.failed.take();
                if let Some((exec, _)) = key {
                    reset_init_item(i, &exec);
                }
            } else if embedded {
                // Rule O4: abandoning a created project's init lets its phases run
                // without it, because the rest of the session still needs them.
                i.finished = true;
                i.note =
                    Some("The user abandoned the init after an initializer step failed.".into());
            } else {
                abandoned = true;
            }
        }
        if abandoned {
            self.failed = Some("The init was abandoned after an initializer step failed.".into());
        }
    }
}
