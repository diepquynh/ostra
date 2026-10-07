//! Rule F1: feedback rounds until the user accepts the implementation.

use crate::planner::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::{GatePayload, JudgeKind};
use ostra_engine::plan::*;

/// Rule F1: feedback rounds until the user accepts the implementation.
pub trait PlannerFeedback<'a> {
    /// Returns true once the user accepted the implementation.
    fn implementation_review(&mut self) -> bool;
}

impl<'a> PlannerFeedback<'a> for Planner<'a> {
    fn implementation_review(&mut self) -> bool {
        let s = self.s;
        let f = &s.ext.os().feedback;
        if f.accepted || s.ext.os().phases.is_empty() {
            return true;
        }
        if f.gate.is_some() || !self.nothing_running() {
            return false;
        }
        let removed = removed_phases(s);
        if !s
            .ext
            .os()
            .phases
            .values()
            .all(|p| removed.contains(&p.info.id) || p.impl_loop.is_terminal())
        {
            return false;
        }
        if let Some(i) = f.rounds.iter().position(|r| r.route.is_none()) {
            self.push(Step::Judge {
                judge: JudgeKind::Feedback,
                subject: Some(i.to_string()),
            });
            return false;
        }
        if f.rounds.iter().any(|r| r.awaiting_spec) {
            return false;
        }
        // Rule F1: every phase finished, so the user reviews the result before the closing stages.
        let blocked = s
            .ext
            .os()
            .phases
            .values()
            .filter_map(|p| match &p.impl_loop.next {
                LoopNext::Blocked { reason } => Some(format!("Phase {}: {reason}", p.info.id)),
                _ => None,
            })
            .collect();
        self.gate(
            "Review the implementation",
            "Every phase is built and reviewed. Try the change, then describe what to change, or accept it. Accepting moves on to formatting, tests, documentation, and the completion report.",
            GatePayload::ImplementationReview {
                round: f.rounds.len() as u32 + 1,
                context_path: s.session_context_path(),
                reports: s
                    .ext.os().phases
                    .values()
                    .filter_map(|p| p.implementer_report.clone())
                    .collect(),
                blocked,
            },
        );
        false
    }
}
