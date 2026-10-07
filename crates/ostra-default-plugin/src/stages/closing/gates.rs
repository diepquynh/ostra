//! How the closing stage's gates open and how their answers apply: the closing gate, and a failed
//! path analysis.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::{ClosingItem, GateAnswer};
use ostra_core::ids::GateId;
use ostra_engine::state::*;

/// How the closing stage's gates open and how their answers apply.
pub trait ClosingGates {
    fn closing_gate_opened(&mut self, id: &GateId, items: &[ClosingItem]);

    fn closing_gate_answered(&mut self, items: &[ClosingItem], answer: &GateAnswer);

    fn epa_exec_gate(&mut self, phase: u32, gate: &GateId);

    fn epa_exec_answered(&mut self, phase: u32, retry: bool);
}

impl ClosingGates for SessionState {
    fn closing_gate_opened(&mut self, id: &GateId, items: &[ClosingItem]) {
        for item in items {
            self.ext
                .os_mut()
                .project_tracks
                .entry(item.project.clone())
                .or_default()
                .closing_gate = Some(id.clone());
        }
    }

    fn closing_gate_answered(&mut self, items: &[ClosingItem], answer: &GateAnswer) {
        let choices = match answer {
            GateAnswer::Closing { items } => items.clone(),
            _ => vec![],
        };
        let (opt_tests, opt_docs) = (self.tests_requested(), self.docs_requested());
        for item in items {
            let c = choices.iter().find(|c| c.project == item.project);
            let tests = if item.ask_tests {
                c.is_some_and(|c| c.tests)
            } else {
                opt_tests
            };
            let docs = if item.ask_docs {
                c.is_some_and(|c| c.docs)
            } else {
                opt_docs
            };
            let t = self
                .ext
                .os_mut()
                .project_tracks
                .entry(item.project.clone())
                .or_default();
            t.closing = Some((tests, docs));
            t.closing_gate = None;
        }
    }

    fn epa_exec_gate(&mut self, phase: u32, gate: &GateId) {
        if let Some(p) = self.ext.os_mut().phases.get_mut(&phase)
            && let EpaState::Failed { gate: g, .. } = &mut p.epa
        {
            *g = Some(gate.clone());
        }
    }

    fn epa_exec_answered(&mut self, phase: u32, retry: bool) {
        if let Some(p) = self.ext.os_mut().phases.get_mut(&phase) {
            p.epa = if retry {
                EpaState::NotStarted
            } else {
                EpaState::Abandoned
            };
        }
    }
}
