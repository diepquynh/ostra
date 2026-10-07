//! How each judge's decision changes the built-in stages: a dispatcher to the stage that owns
//! the judge.

use super::*;
use crate::judge::RouteAnswerOut;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::stages::{
    build::BuildJudges, feedback::FeedbackGates, research::ResearchJudges, stakes::StakesFold,
    track::TrackFold,
};
use ostra_core::event::JudgeKind;
use ostra_core::ids::{DecisionId, GateId};
use ostra_engine::state::*;
use serde_json::Value;

/// How each judge's decision changes the built-in stages.
pub trait FoldDecisions {
    fn on_decision(
        &mut self,
        id: &DecisionId,
        judge: JudgeKind,
        subject: Option<&str>,
        output: &Value,
        overriding: bool,
    );

    /// Rule J1: the Route answer judge's decision for an amendment or a held answer.
    fn route_answer_decided(&mut self, subject: Option<&str>, output: &Value);
}

impl FoldDecisions for SessionState {
    fn on_decision(
        &mut self,
        id: &DecisionId,
        judge: JudgeKind,
        subject: Option<&str>,
        output: &Value,
        overriding: bool,
    ) {
        match judge {
            JudgeKind::Classify => self.classify_decided(id, output, overriding),
            JudgeKind::Sufficiency => self.sufficiency_decided(subject, output, overriding),
            JudgeKind::Stakes => self.stakes_decided(id, output, overriding),
            JudgeKind::Rescue => self.rescue_decided(subject, output),
            JudgeKind::ResolveReview => self.resolve_review_decided(subject, output),
            JudgeKind::RouteAnswer => self.route_answer_decided(subject, output),
            JudgeKind::Track => self.track_decided(id, output, overriding),
            JudgeKind::Feedback => self.feedback_decided(subject, output),
            JudgeKind::Completion => self.completion_decision = Some(id.clone()),
            JudgeKind::YoloAnswer => {}
        }
    }

    fn route_answer_decided(&mut self, subject: Option<&str>, output: &Value) {
        let Ok(out) = serde_json::from_value::<RouteAnswerOut>(output.clone()) else {
            return;
        };
        if let Some(i) = subject.and_then(amendment_index) {
            self.apply_amendment_route(i, &out);
            return;
        }
        let Some(gate) = subject.map(GateId::from) else {
            return;
        };
        if !self.ext.os().held_answers.contains_key(&gate)
            && !self.ext.os().phases.values().any(|p| {
                [&p.impl_loop, &p.test_loop]
                    .iter()
                    .any(|l| matches!(&l.next, LoopNext::AwaitRoute { gate: g, .. } if *g == gate))
            })
        {
            return;
        }
        self.forget(&out.forget);
        self.skip_research(&out.skip);
        if let Some(held) = self.ext.os_mut().held_answers.remove(&gate) {
            self.apply_held(&gate, held, &out);
            return;
        }
        let target = self
            .ext
            .os()
            .phases
            .iter()
            .flat_map(|(id, p)| [((*id, false), &p.impl_loop), ((*id, true), &p.test_loop)])
            .find_map(|(k, l)| match &l.next {
                LoopNext::AwaitRoute { gate: g, .. } if *g == gate => Some((k, l.next.clone())),
                _ => None,
            });
        if let Some((key, next)) = target {
            self.apply_loop_route(key, &gate, next, &out);
        }
    }
}
