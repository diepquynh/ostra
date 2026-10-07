//! How the plan's approval gate opens and how its answer applies.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::{FactTarget, GateAnswer};
use ostra_core::ids::GateId;
use ostra_engine::state::*;

/// How the plan's approval gate opens and how its answer applies.
pub trait PlanGates {
    fn plan_approval_opened(&mut self, id: &GateId);

    fn plan_approval_answered(&mut self, id: &GateId, answer: &GateAnswer, routed: bool);
}

impl PlanGates for SessionState {
    fn plan_approval_opened(&mut self, id: &GateId) {
        self.ext.os_mut().plan.approval_gate = Some(id.clone());
        self.ext.os_mut().plan.approval_asked_version = self.ext.os_mut().plan.version;
    }

    fn plan_approval_answered(&mut self, id: &GateId, answer: &GateAnswer, routed: bool) {
        let current = self.ext.os().plan.approval_gate.as_ref() == Some(id)
            && !self.ext.os().plan.needs_run
            && !self.ext.os().plan.invalidated;
        self.ext.os_mut().plan.approval_gate = None;
        if routed && let Some((approved, text)) = approval_text(answer) {
            let approve = approved && current && self.ext.os().plan.passed_current();
            self.hold_approval(id, FactTarget::Plan, approve, text);
            return;
        }
        match answer {
            GateAnswer::Approval { approved: true, .. }
                if current && self.ext.os().plan.passed_current() =>
            {
                self.ext.os_mut().plan.approved = true;
                self.ext.os_mut().plan.approved_version = self.ext.os_mut().plan.version;
                self.adopt_plan_phases();
            }
            GateAnswer::Approval {
                feedback: Some(text),
                ..
            } if !text.trim().is_empty() => {
                // Rule D10: a change after the plan exists goes into the spec first.
                self.requirement_change(text.clone());
            }
            _ => {}
        }
    }
}
