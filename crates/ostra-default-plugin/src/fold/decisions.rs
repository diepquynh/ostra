//! How each judge's decision changes the built-in stages.

use super::*;
use crate::judge::{
    ANSWER_ITEM, ClassifyOut, Disposition, FeedbackOut, MAX_SUFFICIENCY_RESEARCH, RescueAction,
    RescueOut, ResolveAction, ResolveReviewOut, RouteAnswerOut, StakesOut, SufficiencyOut,
    TrackOut, item_for,
};
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::Contract;
use ostra_core::event::{JudgeKind, WorkKind};
use ostra_core::ids::{DecisionId, ExecutionId, GateId};
use ostra_core::pipeline::{Category, Stakes};
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
            JudgeKind::Classify => {
                if overriding && !self.can_override_classify_now() {
                    return;
                }
                if let Ok(out) = serde_json::from_value::<ClassifyOut>(output.clone()) {
                    self.classify = Some(id.clone());
                    self.apply_classify(&out);
                } else if !overriding {
                    self.classify = Some(id.clone());
                    self.failed = Some(
                        "The Classify judge returned output that does not match its schema.".into(),
                    );
                }
            }
            JudgeKind::Sufficiency => {
                let covered: Vec<u32> = subject
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if !overriding {
                    self.ext.os_mut().sufficiency_rounds += 1;
                }
                for t in self
                    .ext
                    .os_mut()
                    .explore
                    .iter_mut()
                    .filter(|t| covered.contains(&t.idx))
                {
                    t.judged = true;
                }
                if overriding {
                    self.ext
                        .os_mut()
                        .explore
                        .retain(|t| !(t.origin == ExploreOrigin::Sufficiency && t.exec.is_none()));
                }
                if let Ok(out) = serde_json::from_value::<SufficiencyOut>(output.clone()) {
                    let mut added = 0;
                    for item in out.items.into_iter().filter(|i| i.needed) {
                        if added == MAX_SUFFICIENCY_RESEARCH {
                            break;
                        }
                        let (project, task) = match item.task {
                            Some(t) if self.valid_project(&t.project) => (t.project, t.task),
                            _ => (self.primary(), item.item.clone()),
                        };
                        // Rule D2: the judge often maps several items to one task; research it once.
                        if self
                            .ext
                            .os()
                            .explore
                            .iter()
                            .any(|t| !t.finished() && t.project == project && t.task == task)
                        {
                            continue;
                        }
                        self.push_explore(project, task, ExploreOrigin::Sufficiency);
                        added += 1;
                    }
                }
            }
            JudgeKind::Stakes => {
                if let Ok(out) = serde_json::from_value::<StakesOut>(output.clone()) {
                    if overriding
                        && !(self.ext.os().plan.runs.is_empty() && !self.any_phase_started())
                    {
                        return;
                    }
                    self.ext.os_mut().stakes = Some((id.clone(), out.stakes));
                    if overriding {
                        self.ext.os_mut().phases.clear();
                    }
                    if out.stakes == Stakes::Low && self.category == Some(Category::Implement) {
                        // Plan skipped for a lower-stakes request: inline phases, one per project,
                        // queued in order because there is no graph to read (Rule M5).
                        let scope = self.scope.clone();
                        for (i, key) in scope.iter().enumerate() {
                            let l = WorkLoop::new(
                                false,
                                Contract::Implementation,
                                Contract::Implementation,
                            );
                            self.insert_phase(
                                inline_phase(i as u32 + 1, key, "Implementation", i),
                                l,
                            );
                        }
                    }
                }
            }
            JudgeKind::Rescue => {
                let Some(exec) = subject.map(ExecutionId::from) else {
                    return;
                };
                let Ok(out) = serde_json::from_value::<RescueOut>(output.clone()) else {
                    return;
                };
                let key = self.executions.get(&exec).and_then(|r| r.loop_key);
                let Some(key) = key else { return };
                let project = self
                    .ext
                    .os()
                    .phases
                    .get(&key.0)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let next = self.loop_ref(key).map(|l| l.next.clone());
                let Some(LoopNext::Rescue {
                    exec: stuck_exec,
                    stuck,
                }) = next
                else {
                    return;
                };
                if stuck_exec != exec {
                    return;
                }
                let new_next = match out.action {
                    RescueAction::Explore => {
                        let (p, task) = match out.explore_task {
                            Some(t) if self.valid_project(&t.project) => (t.project, t.task),
                            _ => (
                                project,
                                format!(
                                    "Find the fact this agent needs.\nNeed: {}\nDiagnostic:\n{}",
                                    stuck.need, stuck.diagnostic
                                ),
                            ),
                        };
                        let idx = self.push_explore(
                            p,
                            task,
                            ExploreOrigin::Rescue {
                                phase: key.0,
                                tests: key.1,
                            },
                        );
                        LoopNext::RescueExplore { task: idx, stuck }
                    }
                    RescueAction::Rerun => LoopNext::Work {
                        kind: WorkKind::Rescue,
                        instructions: Some(rescue_context(
                            &stuck,
                            out.fact.as_deref().unwrap_or(&out.reason),
                        )),
                    },
                    // Rule O7: at most MAX_ADVICE advisor rounds per loop before the user is asked.
                    RescueAction::Advise
                        if self
                            .loop_ref(key)
                            .is_some_and(|l| l.advice.len() < crate::init::MAX_ADVICE) =>
                    {
                        LoopNext::RescueAdvise {
                            exec,
                            stuck,
                            advisor: None,
                        }
                    }
                    RescueAction::Advise | RescueAction::Gate => {
                        LoopNext::RescueGate { exec, stuck }
                    }
                };
                if let Some(l) = self.loop_mut(key) {
                    l.next = new_next;
                }
            }
            JudgeKind::ResolveReview => {
                let Some(key) = subject.and_then(parse_loop_key) else {
                    return;
                };
                let Ok(out) = serde_json::from_value::<ResolveReviewOut>(output.clone()) else {
                    return;
                };
                let project = self
                    .ext
                    .os()
                    .phases
                    .get(&key.0)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let ledger = self
                    .ledger_path(&project, key.0, key.1)
                    .display()
                    .to_string();
                let Some(l) = self.loop_mut(key) else { return };
                let LoopNext::Resolve { findings } = l.next.clone() else {
                    return;
                };
                match out.action {
                    ResolveAction::Fix => {
                        l.resolve_rounds += 1;
                        l.open_before_resolve = Some(findings.len());
                        // One verification pass per resolution round (review-cap.js).
                        l.extra_cap += 1;
                        let mut text = String::from(
                            "Resolve these review findings with the instructions given for each:\n",
                        );
                        for i in &out.instructions {
                            text.push_str(&format!(
                                "- Finding: {}\n  Instruction: {}\n",
                                i.finding, i.instruction
                            ));
                        }
                        if out.instructions.is_empty() {
                            text = fix_instructions(&findings, &ledger);
                        } else {
                            text.push_str(&format!("\nRecord a FIXED or WONTFIX line for each in the review ledger at {ledger}."));
                        }
                        l.next = LoopNext::Work {
                            kind: WorkKind::Fix,
                            instructions: Some(text),
                        };
                    }
                    ResolveAction::Block => {
                        l.next = LoopNext::Blocked {
                            reason: format!(
                                "The engine could not resolve the open review findings: {}. Ledger: {ledger}",
                                out.reason
                            ),
                        };
                    }
                }
            }
            JudgeKind::RouteAnswer => {
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
                        [&p.impl_loop, &p.test_loop].iter().any(
                        |l| matches!(&l.next, LoopNext::AwaitRoute { gate: g, .. } if *g == gate),
                    )
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
                        LoopNext::AwaitRoute { gate: g, .. } if *g == gate => {
                            Some((k, l.next.clone()))
                        }
                        _ => None,
                    });
                if let Some((key, next)) = target {
                    self.apply_loop_route(key, &gate, next, &out);
                }
            }
            JudgeKind::Track => {
                let Ok(out) = serde_json::from_value::<TrackOut>(output.clone()) else {
                    return;
                };
                if overriding && !self.can_override(id) {
                    return;
                }
                self.ext.os_mut().track_decision = Some(id.clone());
                self.set_track(out.track);
            }
            JudgeKind::Feedback => {
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
            JudgeKind::Completion => self.completion_decision = Some(id.clone()),
            JudgeKind::YoloAnswer => {}
        }
    }
}
