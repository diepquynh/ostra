//! How the built-in stages' gates open and how their answers apply: a dispatcher to the stage
//! that owns each gate, plus the spec and plan gates that the two stages share.

use super::*;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::stages::{
    book::BookGates, build::BuildGates, closing::ClosingGates, feedback::FeedbackGates,
    init::InitGates, plan::PlanGates, quick::QuickRuns, research::ResearchGates, spec::SpecGates,
};
use ostra_core::event::{ExecPurpose, FactTarget, GateAnswer, GatePayload, WorkKind};
use ostra_core::ids::{ExecutionId, GateId};
use ostra_engine::state::*;

/// A gate's choice answer: the option and its text, when the text is not empty.
pub type Choice<'a> = Option<(&'a str, Option<String>)>;

/// How the built-in stages' gates open and how their answers apply.
pub trait FoldGates {
    fn owner_of_exec_gate(&mut self, exec: &ExecutionId, gate: &GateId);

    fn on_gate_opened(&mut self, id: &GateId, payload: &GatePayload);

    fn on_gate_answered(
        &mut self,
        id: &GateId,
        payload: &GatePayload,
        answer: &GateAnswer,
        routed: bool,
    );

    fn exec_gate_answered(&mut self, execution: &ExecutionId, retry: bool);

    /// The spec's or the plan's open questions.
    fn open_questions_answered(
        &mut self,
        id: &GateId,
        artifact: &str,
        answer: &GateAnswer,
        routed: bool,
    );

    /// The spec's or the plan's recurring fact-check findings.
    fn recurring_answered(
        &mut self,
        id: &GateId,
        target: FactTarget,
        choice: Choice<'_>,
        routed: bool,
    );
}

impl FoldGates for SessionState {
    fn owner_of_exec_gate(&mut self, exec: &ExecutionId, gate: &GateId) {
        let Some(rec) = self.executions.get(exec).cloned() else {
            return;
        };
        if let Some(key) = rec.loop_key {
            if let Some(l) = self.loop_mut(key) {
                l.gate = Some(gate.clone());
            }
            return;
        }
        match &rec.purpose {
            ExecPurpose::Stage { .. } => self.stage_exec_gate(&rec, Some(gate), false),
            ExecPurpose::Explore { task } => self.explore_exec_gate(*task, gate),
            ExecPurpose::Spec { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Spec,
                ..
            } => self.ext.os_mut().spec.failed_gate = Some(gate.clone()),
            ExecPurpose::Plan { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Plan,
                ..
            } => self.ext.os_mut().plan.failed_gate = Some(gate.clone()),
            ExecPurpose::Epa { phase } => self.epa_exec_gate(*phase, gate),
            ExecPurpose::Docs { .. }
            | ExecPurpose::DocsSurvey { .. }
            | ExecPurpose::DocsCheck { .. }
            | ExecPurpose::DocsSynthesis { .. } => self.docs_exec_gate(&rec.purpose, gate),
            ExecPurpose::Init { .. } => self.init_exec_gate(&rec.project, gate),
            _ => {}
        }
    }

    fn on_gate_opened(&mut self, id: &GateId, payload: &GatePayload) {
        match payload {
            GatePayload::StageReview { .. } => self.stage_gate_opened(id, payload),
            GatePayload::OpenQuestions { artifact, .. } => {
                if artifact == "plan" {
                    self.ext.os_mut().plan.questions_gate = Some(id.clone());
                    self.ext.os_mut().plan.questions_asked_version = self.ext.os_mut().plan.version;
                } else {
                    self.ext.os_mut().spec.questions_gate = Some(id.clone());
                    self.ext.os_mut().spec.questions_asked_version = self.ext.os_mut().spec.version;
                }
            }
            GatePayload::SpecApproval { .. } => self.spec_approval_opened(id),
            GatePayload::PlanApproval { .. } => self.plan_approval_opened(id),
            GatePayload::FactCheckRecurring { target, .. } => match target {
                FactTarget::Spec => self.ext.os_mut().spec.recurring_gate = Some(id.clone()),
                FactTarget::Plan => self.ext.os_mut().plan.recurring_gate = Some(id.clone()),
            },
            GatePayload::ReviewCap { phase, tests, .. } => {
                if let Some(l) = self.loop_mut((*phase, *tests)) {
                    l.gate = Some(id.clone());
                }
            }
            GatePayload::PhaseBlocked { phase, .. } => self.phase_blocked_opened(id, *phase),
            GatePayload::Stuck { execution, .. }
            | GatePayload::ExecutionFailed { execution, .. }
            | GatePayload::HarnessFailure { execution, .. } => {
                self.owner_of_exec_gate(execution, id)
            }
            GatePayload::ClosingGate { items } => self.closing_gate_opened(id, items),
            GatePayload::SkillApproval { project, .. } => self.skill_approval_opened(id, project),
            GatePayload::Permission { .. } => {}
            // Pattern 8: the engine folds the budget gate.
            GatePayload::BudgetReached { .. } => {}
            GatePayload::DocsRounds { project, .. } => self.docs_rounds_opened(id, project),
            GatePayload::ImplementationReview { .. } => self.implementation_review_opened(id),
        }
    }

    fn on_gate_answered(
        &mut self,
        id: &GateId,
        payload: &GatePayload,
        answer: &GateAnswer,
        routed: bool,
    ) {
        let choice = match answer {
            GateAnswer::Choice { option, text } => Some((
                option.as_str(),
                text.clone().filter(|t| !t.trim().is_empty()),
            )),
            _ => None,
        };
        match payload {
            GatePayload::StageReview { .. } => self.stage_gate_answered(id, payload, answer),
            GatePayload::OpenQuestions { artifact, .. } => {
                self.open_questions_answered(id, artifact, answer, routed)
            }
            GatePayload::SpecApproval { .. } => self.spec_approval_answered(id, answer, routed),
            GatePayload::PlanApproval { .. } => self.plan_approval_answered(id, answer, routed),
            GatePayload::FactCheckRecurring { target, .. } => {
                self.recurring_answered(id, *target, choice, routed)
            }
            GatePayload::ReviewCap {
                phase,
                tests,
                findings,
                ..
            } => self.review_cap_answered(id, *phase, *tests, findings, choice, routed),
            GatePayload::Stuck { execution, .. } => self.stuck_answered(id, execution, choice),
            GatePayload::PhaseBlocked { phase, .. } => {
                self.phase_blocked_answered(id, *phase, choice)
            }
            GatePayload::ClosingGate { items } => self.closing_gate_answered(items, answer),
            GatePayload::ExecutionFailed { execution, .. }
            | GatePayload::HarnessFailure { execution, .. } => {
                if let GatePayload::HarnessFailure { .. } = payload
                    && let Some(("native", _)) = choice
                    && let Some(r) = self.executions.get(execution)
                {
                    self.native_fallback.insert(r.agent);
                }
                let retry = matches!(choice, Some(("retry" | "native", _)));
                self.exec_gate_answered(execution, retry);
            }
            GatePayload::SkillApproval { project, .. } => {
                self.skill_approval_answered(project, answer)
            }
            GatePayload::Permission { .. } => {}
            GatePayload::ImplementationReview { .. } => {
                self.implementation_review_answered(id, choice, routed)
            }
            GatePayload::DocsRounds {
                project, rounds, ..
            } => self.docs_rounds_answered(project, *rounds, choice),
            GatePayload::BudgetReached { .. } => {}
        }
    }

    fn exec_gate_answered(&mut self, execution: &ExecutionId, retry: bool) {
        let Some(rec) = self.executions.get(execution).cloned() else {
            return;
        };
        if let Some(key) = rec.loop_key {
            if let Some(l) = self.loop_mut(key) {
                l.gate = None;
                if let LoopNext::Failed { .. } = l.next {
                    l.next = if retry {
                        l.error_retries = 0;
                        match rec.purpose {
                            ExecPurpose::Review { .. } => LoopNext::Review,
                            _ => LoopNext::Work {
                                kind: WorkKind::Rerun,
                                instructions: l.rationale.clone(),
                            },
                        }
                    } else {
                        LoopNext::Blocked {
                            reason: format!("{} failed and the user abandoned it.", rec.agent),
                        }
                    };
                }
            }
            return;
        }
        match &rec.purpose {
            ExecPurpose::Stage { .. } => self.stage_exec_gate(&rec, None, retry),
            ExecPurpose::Explore { task } => self.explore_exec_answered(*task, retry),
            ExecPurpose::Spec { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Spec,
                ..
            } => exec_retry(
                &mut self.ext.os_mut().spec,
                retry,
                matches!(rec.purpose, ExecPurpose::Spec { .. }),
            ),
            ExecPurpose::Plan { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Plan,
                ..
            } => exec_retry(
                &mut self.ext.os_mut().plan,
                retry,
                matches!(rec.purpose, ExecPurpose::Plan { .. }),
            ),
            ExecPurpose::Epa { phase } => self.epa_exec_answered(*phase, retry),
            ExecPurpose::Docs { .. }
            | ExecPurpose::DocsSurvey { .. }
            | ExecPurpose::DocsCheck { .. }
            | ExecPurpose::DocsSynthesis { .. } => self.docs_exec_answered(&rec.purpose, retry),
            ExecPurpose::QuickAnswer => self.quick_exec_answered(retry),
            ExecPurpose::Init { .. } => self.init_exec_answered(&rec.project, retry),
            _ => {}
        }
    }

    fn open_questions_answered(
        &mut self,
        id: &GateId,
        artifact: &str,
        answer: &GateAnswer,
        routed: bool,
    ) {
        let answers = match answer {
            GateAnswer::Questions { answers } => answers.clone(),
            _ => vec![],
        };
        let target = if artifact == "plan" {
            FactTarget::Plan
        } else {
            FactTarget::Spec
        };
        if routed && !answers.is_empty() {
            // Rule J1: the judge decides where each answer goes before any agent sees it.
            let t = self.track_mut(target);
            t.clear_questions_gate();
            t.set_routing(Some(id.clone()));
            self.ext
                .os_mut()
                .held_answers
                .insert(id.clone(), HeldAnswer::Questions { target, answers });
            return;
        }
        if artifact == "plan" {
            self.ext.os_mut().plan.questions_gate = None;
            if !answers.is_empty() {
                // After the plan exists, answers go into the spec first (answer routing).
                let text = answers
                    .iter()
                    .map(|a| format!("{} {}\nAnswer: {}", a.id, a.question, a.answer))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.requirement_change(format!(
                    "Answers to the plan's clarifying questions:\n{text}"
                ));
            }
        } else {
            self.ext.os_mut().spec.questions_gate = None;
            // Rule D3: every answer re-runs generate-spec.
            self.ext.os_mut().spec.answers.extend(answers);
            self.ext.os_mut().spec.needs_run = true;
        }
    }

    fn recurring_answered(
        &mut self,
        id: &GateId,
        target: FactTarget,
        choice: Choice<'_>,
        routed: bool,
    ) {
        let track_changes: Option<String>;
        {
            let t_spec;
            let track: &mut dyn RecurringTrack = match target {
                FactTarget::Spec => {
                    t_spec = &mut self.ext.os_mut().spec;
                    t_spec
                }
                FactTarget::Plan => &mut self.ext.os_mut().plan,
            };
            track.clear_recurring_gate();
            track_changes = match choice {
                Some(("stop", _)) => {
                    track.stop();
                    None
                }
                Some((_, text)) => {
                    track.allow_more();
                    text
                }
                None => None,
            };
        }
        if let Some(text) = track_changes {
            if routed {
                self.track_mut(target).set_routing(Some(id.clone()));
                self.ext
                    .os_mut()
                    .held_answers
                    .insert(id.clone(), HeldAnswer::Recurring { target, text });
                return;
            }
            match target {
                FactTarget::Spec => {
                    self.ext.os_mut().spec.changes.push(text);
                    self.ext.os_mut().spec.needs_run = true;
                }
                FactTarget::Plan => self.requirement_change(text),
            }
        }
    }
}
