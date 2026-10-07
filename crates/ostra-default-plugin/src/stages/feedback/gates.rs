//! How the implementation review gate (Rule F1) opens and how its answer applies, and how the
//! Feedback judge's decision routes the user's feedback.

use crate::fold::*;
use crate::judge::{ANSWER_ITEM, Disposition, FeedbackOut, item_for};
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::ids::GateId;
use ostra_engine::state::*;
use serde_json::Value;

/// How the implementation review gate opens and how its answer and its judge apply.
pub trait FeedbackGates {
    fn implementation_review_opened(&mut self, id: &GateId);

    fn implementation_review_answered(&mut self, id: &GateId, choice: Choice<'_>, routed: bool);

    fn feedback_decided(&mut self, subject: Option<&str>, output: &Value);
}

impl FeedbackGates for SessionState {
    fn implementation_review_opened(&mut self, id: &GateId) {
        self.ext.os_mut().feedback.gate = Some(id.clone())
    }

    fn implementation_review_answered(&mut self, id: &GateId, choice: Choice<'_>, routed: bool) {
        if self.ext.os().feedback.gate.as_ref() != Some(id) {
            return;
        }
        self.ext.os_mut().feedback.gate = None;
        match choice {
            Some(("feedback", Some(text))) => self.add_feedback(text, routed),
            _ => self.ext.os_mut().feedback.accepted = true,
        }
    }

    fn feedback_decided(&mut self, subject: Option<&str>, output: &Value) {
        let Some(i) = subject.and_then(|s| s.parse::<usize>().ok()) else {
            return;
        };
        let Ok(out) = serde_json::from_value::<FeedbackOut>(output.clone()) else {
            return;
        };
        if self
            .ext
            .os()
            .feedback
            .rounds
            .get(i)
            .is_none_or(|r| r.route.is_some())
        {
            return;
        }
        let Some(gate) = self.feedback_gate(i) else {
            self.route_feedback(i, out.route, out.targets, Some(out.reason));
            return;
        };
        let item = item_for(&out.items, ANSWER_ITEM);
        let text = self.ext.os().feedback.rounds[i].text.clone();
        self.forget(&out.forget);
        self.remember_parts(Some(&gate), &out.items, ANSWER_ITEM, &text);
        self.queue_research(&out.research, ExploreOrigin::Answer);
        match item.disposition {
            Disposition::Deliver => {
                self.route_feedback(i, out.route, out.targets, Some(out.reason))
            }
            Disposition::Remember | Disposition::Discard => {
                let r = &mut self.ext.os_mut().feedback.rounds[i];
                r.route = Some(out.route);
                r.reason = Some(out.reason);
                // Keeping the feedback only for later stages accepts what was built.
                if item.disposition == Disposition::Remember {
                    self.ext.os_mut().feedback.accepted = true;
                }
            }
        }
    }
}
