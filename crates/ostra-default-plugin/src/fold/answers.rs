//! Rule J1: answers at the spec and plan gates and the context the user adds, routed by the Route answer judge.

use super::*;
use crate::judge::{
    ANSWER_ITEM, AnswerItem, AnswerRoute, Disposition, NoteStage, RouteAnswerOut, parts_for,
};
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::{FactTarget, GatePayload};
use ostra_core::ids::GateId;
use ostra_core::pipeline::{Category, QuestionAnswer};
use ostra_engine::state::*;

/// Rule J1: answers at the spec and plan gates and the context the user adds, routed by the Route answer judge.
pub trait FoldAnswers {
    /// Rule D10: a requirement change after the spec exists restarts at the spec.
    fn requirement_change(&mut self, text: String);

    fn track_mut(&mut self, target: FactTarget) -> &mut dyn RoutingTrack;

    fn approve_spec(&mut self);

    fn hold_approval(&mut self, id: &GateId, target: FactTarget, approve: bool, text: String);

    /// The implementation review gate a feedback round was answered at, when it was routed.
    fn feedback_gate(&self, round: usize) -> Option<GateId>;

    /// Rule J1: keep every part of an answer the judge named later stages for.
    fn remember_parts(
        &mut self,
        gate: Option<&GateId>,
        items: &[AnswerItem],
        id: &str,
        answer: &str,
    );

    /// Rule J1: keep the part of an answer the judge named later stages for.
    fn remember(&mut self, gate: Option<&GateId>, item: &AnswerItem, answer: &str);

    /// Rule J1: the user took back or replaced notes kept earlier.
    fn forget(&mut self, ids: &[String]);

    /// Rule C2: apply the Route answer judge's decision on context the user added mid-session.
    fn apply_amendment_route(&mut self, i: usize, out: &RouteAnswerOut);

    fn apply_held(&mut self, gate: &GateId, held: HeldAnswer, out: &RouteAnswerOut);

    fn apply_loop_route(
        &mut self,
        key: (u32, bool),
        gate: &GateId,
        next: LoopNext,
        out: &RouteAnswerOut,
    );

    /// Rule J1: once every research task an answer queued has finished, the loop continues with
    /// their documents added to its instructions.
    fn release_answer_research(&mut self, key: (u32, bool));

    /// Rule J1: the notes the judge kept for a later stage, oldest first.
    fn notes_for(&self, stage: NoteStage) -> Vec<String>;

    fn on_amended(&mut self, text: &str);
}

impl FoldAnswers for SessionState {
    fn requirement_change(&mut self, text: String) {
        self.ext.os_mut().spec.changes.push(text);
        self.ext.os_mut().spec.needs_run = true;
        self.ext.os_mut().spec.revoke_approval();
        if self.ext.os().plan.current.is_some()
            || self.ext.os().plan.running.is_some()
            || !self.ext.os().plan.runs.is_empty()
        {
            self.ext.os_mut().plan.revoke_approval();
            self.ext.os_mut().plan.invalidated = true;
        }
    }

    fn track_mut(&mut self, target: FactTarget) -> &mut dyn RoutingTrack {
        match target {
            FactTarget::Spec => &mut self.ext.os_mut().spec,
            FactTarget::Plan => &mut self.ext.os_mut().plan,
        }
    }

    fn approve_spec(&mut self) {
        self.ext.os_mut().spec.approved = true;
        self.ext.os_mut().spec.approved_version = self.ext.os_mut().spec.version;
        if self.ext.os().plan.invalidated {
            self.ext.os_mut().plan.needs_run = true;
        }
        for i in 0..self.ext.os().feedback.rounds.len() {
            if self.ext.os().feedback.rounds[i].awaiting_spec {
                self.ext.os_mut().feedback.rounds[i].awaiting_spec = false;
                self.add_revision_phases(i);
            }
        }
    }

    fn hold_approval(&mut self, id: &GateId, target: FactTarget, approve: bool, text: String) {
        let version = match target {
            FactTarget::Spec => self.ext.os().spec.version,
            FactTarget::Plan => self.ext.os().plan.version,
        };
        self.track_mut(target).set_routing(Some(id.clone()));
        self.ext.os_mut().held_answers.insert(
            id.clone(),
            HeldAnswer::Approval {
                target,
                approve,
                version,
                text,
            },
        );
    }

    fn feedback_gate(&self, round: usize) -> Option<GateId> {
        self.gates
            .values()
            .filter(|g| g.answer.is_some())
            .find(|g| matches!(&g.payload, GatePayload::ImplementationReview { round: r, .. } if *r as usize == round + 1))
            .map(|g| g.id.clone())
    }

    fn remember_parts(
        &mut self,
        gate: Option<&GateId>,
        items: &[AnswerItem],
        id: &str,
        answer: &str,
    ) {
        let parts: Vec<AnswerItem> = parts_for(items, id).into_iter().cloned().collect();
        for p in &parts {
            self.remember(gate, p, answer);
        }
    }

    fn remember(&mut self, gate: Option<&GateId>, item: &AnswerItem, answer: &str) {
        if item.stages.is_empty() || item.disposition == Disposition::Discard {
            return;
        }
        let text = if item.note.trim().is_empty() {
            answer.to_string()
        } else {
            item.note.clone()
        };
        let mut stages = item.stages.clone();
        stages.sort();
        stages.dedup();
        let notes = &mut self.ext.os_mut().user_notes;
        notes.push(UserNote {
            id: format!("N{}", notes.len() + 1),
            forgotten: false,
            stages,
            text,
            gate: gate.cloned(),
        });
    }

    fn forget(&mut self, ids: &[String]) {
        for n in &mut self.ext.os_mut().user_notes {
            if ids.iter().any(|i| i.trim() == n.id) {
                n.forgotten = true;
            }
        }
    }

    fn apply_amendment_route(&mut self, i: usize, out: &RouteAnswerOut) {
        let Some(a) = self.amendments.get(i).filter(|a| a.pending) else {
            return;
        };
        let text = self.added_part(&a.text, &a.files, &a.uploads);
        let item = out.item(ANSWER_ITEM);
        let deliver = item.disposition == Disposition::Deliver;
        self.amendments[i].pending = false;
        self.amendments[i].delivered = deliver;
        self.forget(&out.forget);
        self.skip_research(&out.skip);
        self.remember_parts(None, &out.items, ANSWER_ITEM, &text);
        self.queue_research(&out.research, ExploreOrigin::Amendment);
        // Rule D10: a requirement change after the spec exists restarts at the spec.
        if deliver
            && out.route == AnswerRoute::RequirementChange
            && !self.ext.os().spec.runs.is_empty()
        {
            self.requirement_change(format!("The user extended the request: {text}"));
        }
    }

    fn apply_held(&mut self, gate: &GateId, held: HeldAnswer, out: &RouteAnswerOut) {
        let target = held.target();
        self.track_mut(target).set_routing(None);
        let research = self.queue_research(&out.research, ExploreOrigin::Answer);
        let research_note = (!research.is_empty()).then(|| {
            "The user asked for more research before this step. Fold the new research documents into the spec.".to_string()
        });
        match held {
            HeldAnswer::Questions { target, answers } => {
                let questions = match self.gates.get(gate).map(|g| &g.payload) {
                    Some(GatePayload::OpenQuestions { questions, .. }) => questions.clone(),
                    _ => vec![],
                };
                let mut sent = vec![];
                for a in answers {
                    let item = out.item(&a.id);
                    let in_context = questions
                        .iter()
                        .find(|q| q.id == a.id)
                        .map(|q| q.answer_in_context(&a.answer))
                        .unwrap_or_else(|| a.answer.clone());
                    self.remember_parts(Some(gate), &out.items, &a.id, &in_context);
                    let answer = match item.disposition {
                        Disposition::Deliver => in_context,
                        Disposition::Remember => format!(
                            "The user gave this answer for a later stage, which receives it directly: {}. Remove the question and add no requirement for it.",
                            if item.note.trim().is_empty() { &a.answer } else { &item.note }
                        ),
                        Disposition::Discard => "The user chose not to answer this. Remove the question, and settle the point from the research or record it as an assumption.".into(),
                    };
                    sent.push(QuestionAnswer { answer, ..a });
                }
                match target {
                    FactTarget::Spec => {
                        // Rule D3: every answer re-runs generate-spec.
                        self.ext.os_mut().spec.answers.extend(sent);
                        self.ext.os_mut().spec.needs_run = true;
                    }
                    FactTarget::Plan => {
                        let text = sent
                            .iter()
                            .map(|a| format!("{} {}\nAnswer: {}", a.id, a.question, a.answer))
                            .collect::<Vec<_>>()
                            .join("\n");
                        // After the plan exists, answers go into the spec first (answer routing).
                        self.requirement_change(format!(
                            "Answers to the plan's clarifying questions:\n{text}{}",
                            research_note.map(|n| format!("\n{n}")).unwrap_or_default()
                        ));
                    }
                }
            }
            HeldAnswer::Approval {
                target,
                approve,
                version,
                text,
            } => {
                let item = out.item(ANSWER_ITEM);
                self.remember_parts(Some(gate), &out.items, ANSWER_ITEM, &text);
                let change = match item.disposition {
                    Disposition::Deliver => Some(text),
                    _ => research_note,
                };
                match (target, change) {
                    (FactTarget::Spec, Some(c)) => {
                        self.ext.os_mut().spec.changes.push(c);
                        self.ext.os_mut().spec.needs_run = true;
                    }
                    // Rule D10: a change after the plan exists goes into the spec first.
                    (FactTarget::Plan, Some(c)) => self.requirement_change(c),
                    (FactTarget::Spec, None) => {
                        if approve
                            && self.ext.os().spec.version == version
                            && !self.ext.os().spec.needs_run
                        {
                            self.approve_spec();
                        }
                    }
                    (FactTarget::Plan, None) => {
                        if approve
                            && self.ext.os().plan.version == version
                            && !self.ext.os().plan.needs_run
                            && !self.ext.os().plan.invalidated
                        {
                            self.ext.os_mut().plan.approved = true;
                            self.ext.os_mut().plan.approved_version =
                                self.ext.os_mut().plan.version;
                            self.adopt_plan_phases();
                        }
                    }
                }
            }
            HeldAnswer::Recurring { target, text } => {
                let item = out.item(ANSWER_ITEM);
                self.remember_parts(Some(gate), &out.items, ANSWER_ITEM, &text);
                let change = match item.disposition {
                    Disposition::Deliver => Some(text),
                    _ => research_note,
                };
                match (target, change) {
                    (FactTarget::Spec, Some(c)) => {
                        self.ext.os_mut().spec.changes.push(c);
                        self.ext.os_mut().spec.needs_run = true;
                    }
                    (FactTarget::Plan, Some(c)) => self.requirement_change(c),
                    (_, None) => {}
                }
            }
        }
    }

    fn apply_loop_route(
        &mut self,
        key: (u32, bool),
        gate: &GateId,
        next: LoopNext,
        out: &RouteAnswerOut,
    ) {
        let LoopNext::AwaitRoute {
            text,
            then,
            base,
            stuck,
            fallback,
            ..
        } = next
        else {
            return;
        };
        let item = out.item(ANSWER_ITEM);
        self.remember_parts(Some(gate), &out.items, ANSWER_ITEM, &text);
        let deliver = item.disposition == Disposition::Deliver;
        // With no spec there is nothing to change first, so the answer goes to the phase.
        if deliver
            && out.route == AnswerRoute::RequirementChange
            && self.ext.os().spec.current.is_some()
        {
            // Rule D10: stop the phase and restart at the spec.
            if let Some(l) = self.loop_mut(key) {
                l.next = LoopNext::Blocked {
                    reason: "The user changed a requirement; the spec is being updated (Rule D10)."
                        .into(),
                };
                l.announced_block = true;
                l.block_gate_answered = true;
            }
            self.requirement_change(text);
            return;
        }
        let next = if deliver {
            let instructions = match (&stuck, base) {
                (Some(st), _) => rescue_context(st, &text),
                (None, Some(b)) => format!("{b}\n\nThe user added: {text}"),
                (None, None) => text,
            };
            LoopNext::Work {
                kind: then,
                instructions: Some(instructions),
            }
        } else {
            *fallback
        };
        let tasks = if matches!(next, LoopNext::Work { .. }) {
            self.queue_research(
                &out.research,
                ExploreOrigin::LoopAnswer {
                    phase: key.0,
                    tests: key.1,
                },
            )
        } else {
            vec![]
        };
        if let Some(l) = self.loop_mut(key) {
            if matches!(next, LoopNext::Blocked { .. }) {
                l.block_gate_answered = true;
            }
            l.next = if tasks.is_empty() {
                next
            } else {
                LoopNext::AnswerResearch {
                    tasks,
                    next: Box::new(next),
                }
            };
        }
    }

    fn release_answer_research(&mut self, key: (u32, bool)) {
        let Some(LoopNext::AnswerResearch { tasks, next }) =
            self.loop_ref(key).map(|l| l.next.clone())
        else {
            return;
        };
        let found: Vec<&ExploreTask> = tasks
            .iter()
            .filter_map(|i| self.ext.os().explore.get(*i as usize))
            .collect();
        if found.iter().any(|t| !t.finished()) {
            return;
        }
        let docs: Vec<String> = found
            .iter()
            .filter_map(|t| t.result.as_ref())
            .map(|r| format!("- {}: {}", r.research_path, r.findings_summary))
            .collect();
        let next = match *next {
            LoopNext::Work { kind, instructions } if !docs.is_empty() => LoopNext::Work {
                kind,
                instructions: Some(format!(
                    "{}\n\nResearch the user asked for. Read each document before you start:\n{}",
                    instructions.unwrap_or_default(),
                    docs.join("\n")
                )),
            },
            other => other,
        };
        if let Some(l) = self.loop_mut(key) {
            l.next = next;
        }
    }

    fn notes_for(&self, stage: NoteStage) -> Vec<String> {
        self.ext
            .os()
            .user_notes
            .iter()
            .filter(|n| !n.forgotten && n.stages.contains(&stage))
            .map(|n| n.text.clone())
            .collect()
    }

    fn on_amended(&mut self, text: &str) {
        if self.classify.is_none() {
            return;
        }
        // Rule D2: research the new part before the spec is written again.
        let project = self.primary();
        if matches!(
            self.category,
            Some(Category::Research | Category::Spec | Category::Plan | Category::Implement)
        ) {
            self.push_explore(
                project,
                format!(
                    "The user extended the request. Research the part they added, in the context of the whole request.\nAdded part: {text}\nWhole request: {}",
                    self.request
                ),
                ExploreOrigin::Amendment,
            );
        }
        if !self.ext.os().spec.runs.is_empty() {
            self.requirement_change(format!("The user extended the request: {text}"));
        }
    }
}
