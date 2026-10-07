//! How a failed explore run's gate opens and how its answer applies.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::ids::GateId;
use ostra_engine::state::*;

/// How a failed explore run's gate opens and how its answer applies.
pub trait ResearchGates {
    fn explore_exec_gate(&mut self, task: u32, gate: &GateId);

    fn explore_exec_answered(&mut self, task: u32, retry: bool);
}

impl ResearchGates for SessionState {
    fn explore_exec_gate(&mut self, task: u32, gate: &GateId) {
        if let Some(t) = self.ext.os_mut().explore.get_mut(task as usize) {
            t.gate = Some(gate.clone());
        }
    }

    fn explore_exec_answered(&mut self, task: u32, retry: bool) {
        if let Some(t) = self.ext.os_mut().explore.get_mut(task as usize) {
            t.gate = None;
            if retry {
                t.failed = None;
                t.exec = None;
                t.retries = 0;
            } else {
                t.abandoned = true;
            }
        }
        if let Some(ExploreOrigin::LoopAnswer { phase, tests }) = self
            .ext
            .os()
            .explore
            .get(task as usize)
            .map(|t| t.origin.clone())
        {
            self.release_answer_research((phase, tests));
        }
    }
}
