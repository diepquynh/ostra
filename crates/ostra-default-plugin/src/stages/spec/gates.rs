//! How the spec's approval gate opens and how its answer applies.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::{FactTarget, GateAnswer};
use ostra_core::ids::GateId;
use ostra_engine::state::*;

/// How the spec's approval gate opens and how its answer applies.
pub trait SpecGates {
    fn spec_approval_opened(&mut self, id: &GateId);

    fn spec_approval_answered(&mut self, id: &GateId, answer: &GateAnswer, routed: bool);
}

impl SpecGates for SessionState {
    fn spec_approval_opened(&mut self, id: &GateId) {
        self.ext.os_mut().spec.approval_gate = Some(id.clone());
        self.ext.os_mut().spec.approval_asked_version = self.ext.os_mut().spec.version;
    }

    fn spec_approval_answered(&mut self, id: &GateId, answer: &GateAnswer, routed: bool) {
        // A change that landed after the gate opened revoked it; its answer is stale.
        let current =
            self.ext.os().spec.approval_gate.as_ref() == Some(id) && !self.ext.os().spec.needs_run;
        self.ext.os_mut().spec.approval_gate = None;
        if routed && let Some((approved, text)) = approval_text(answer) {
            let approve = approved && current && self.ext.os().spec.passed_current();
            self.hold_approval(id, FactTarget::Spec, approve, text);
            return;
        }
        match answer {
            GateAnswer::Approval { approved: true, .. }
                if current && self.ext.os().spec.passed_current() =>
            {
                self.approve_spec()
            }
            GateAnswer::Approval {
                feedback: Some(text),
                ..
            } if !text.trim().is_empty() => {
                self.ext.os_mut().spec.changes.push(text.clone());
                self.ext.os_mut().spec.needs_run = true;
            }
            _ => {}
        }
    }
}
