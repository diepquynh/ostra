//! HANDOVER 10.5: how YOLO answers each built-in gate, with a fixed answer or the YOLO judge.

#[allow(unused_imports)]
use crate::book::DocsTrack;
use crate::prelude::*;
use ostra_core::event::{GateAnswer, GatePayload};
use ostra_core::ids::GateId;
use ostra_core::pipeline::QuestionAnswer;
pub use ostra_engine::pipeline::YoloPlan;
use ostra_engine::state::SessionState;
use serde_json::{Value, json};

/// Gates YOLO never answers: spending more, and retrying an execution the user stopped (Rule P4).
pub fn yolo_leaves_open(s: &SessionState, payload: &GatePayload) -> bool {
    match payload {
        GatePayload::BudgetReached { .. } => true,
        GatePayload::ExecutionFailed { execution, .. } => s.stopped_by_user(execution),
        // Rule WF5: a stage that failed every round it may run waits for the user.
        GatePayload::StageReview {
            verdict: ostra_core::submit::StageVerdict::Fail,
            round,
            max_rounds,
            ..
        } => round >= max_rounds,
        _ => false,
    }
}

pub fn yolo_plan(s: &SessionState, gate: &GateId) -> Option<YoloPlan> {
    let g = s.gates.get(gate)?;
    let choice = |option: &str, reason: &str| YoloPlan::Fixed {
        answer: GateAnswer::Choice {
            option: option.into(),
            text: None,
        },
        reason: reason.into(),
    };
    Some(match &g.payload {
        GatePayload::OpenQuestions { questions, .. } => YoloPlan::Judge {
            schema: json!({
                "type": "object",
                "properties": {
                    "kind": {"const": "questions"},
                    "answers": {"type": "array", "items": {
                        "type": "object",
                        "properties": {"id": {"type": "string"}, "question": {"type": "string"}, "answer": {"type": "string", "description": "The chosen option's label, or free text."}},
                        "required": ["id", "question", "answer"]
                    }, "minItems": questions.len(), "maxItems": questions.len()}
                },
                "required": ["kind", "answers"]
            }),
        },
        GatePayload::SpecApproval { .. } | GatePayload::PlanApproval { .. } => YoloPlan::Judge {
            schema: json!({
                "type": "object",
                "properties": {
                    "kind": {"const": "approval"},
                    "approved": {"type": "boolean"},
                    "feedback": {"type": ["string", "null"], "description": "What to change, when not approved."}
                },
                "required": ["kind", "approved"]
            }),
        },
        // Rule O8: under YOLO every stuck run gets an implementer sent to fix its cause.
        GatePayload::Stuck { .. } => choice(
            "fix",
            "Under YOLO an implementer is sent to fix what stopped the agent, so the work continues instead of waiting for you.",
        ),
        GatePayload::ImplementationReview { .. } => choice(
            "done",
            "Under YOLO the implementation is accepted as built, because only the user can say what they want changed.",
        ),
        GatePayload::ClosingGate { items } => YoloPlan::Fixed {
            answer: GateAnswer::Closing {
                items: items
                    .iter()
                    .map(|i| ostra_core::event::ClosingChoice {
                        project: i.project.clone(),
                        tests: !i.ask_tests && s.tests_requested(),
                        docs: !i.ask_docs && s.docs_requested(),
                    })
                    .collect(),
            },
            reason: "Under YOLO the closing gate takes its recommended defaults, no tests and no docs, unless the request already asked for them (Rules T2, T3).".into(),
        },
        GatePayload::FactCheckRecurring { passes, .. } => {
            if *passes < 6 {
                choice("another-round", "Under YOLO another fact-check round runs while the budget allows.")
            } else {
                choice("stop", "The fact-check failed six times in a row, so the engine stopped rather than guess.")
            }
        }
        GatePayload::ReviewCap { .. } => choice("another-pass", "Under YOLO the review loop keeps its larger budget."),
        GatePayload::DocsRounds { .. } => choice("continue", "Under YOLO the docs loop runs another round, and the session budget bounds it (Rule B10)."),
        GatePayload::PhaseBlocked { .. } => choice("leave", "Under YOLO a blocked phase is recorded and independent work continues (Rule D9)."),
        GatePayload::ExecutionFailed { .. } if yolo_leaves_open(s, &g.payload) => return None,
        GatePayload::ExecutionFailed { execution, .. } => {
            let agent = s.executions.get(execution).map(|r| r.agent);
            let prior = s
                .gates
                .values()
                .filter(|x| x.answer.is_some())
                .filter(|x| matches!(&x.payload, GatePayload::ExecutionFailed { execution: e, .. } if s.executions.get(e).map(|r| r.agent) == agent))
                .count();
            if prior < 2 {
                choice("retry", "Under YOLO a failed execution is retried.")
            } else {
                choice("abandon", "This agent failed three times, so the engine abandoned the step instead of retrying again.")
            }
        }
        GatePayload::HarnessFailure { .. } => choice("native", "Under YOLO a harness failure re-routes the execution to the native executor."),
        GatePayload::SkillApproval { skills, .. } => YoloPlan::Fixed {
            answer: GateAnswer::Skills {
                decisions: skills
                    .iter()
                    .map(|k| ostra_core::event::SkillDecision { name: k.name.clone(), disposition: k.disposition.clone() })
                    .collect(),
            },
            reason: "Under YOLO the proposal's default dispositions are used.".into(),
        },
        GatePayload::Permission { .. } => YoloPlan::Fixed {
            answer: GateAnswer::Permission { answer: ostra_core::policy::PermissionAnswer::AllowOnce },
            reason: "Under YOLO every permission ask is allowed.".into(),
        },
        // Spending more is the user's decision, so YOLO leaves a budget gate open.
        GatePayload::BudgetReached { .. } => return None,
        GatePayload::StageReview { .. } if yolo_leaves_open(s, &g.payload) => return None,
        // Rule WF5: options come recommended first, so YOLO takes the first.
        GatePayload::StageReview {
            verdict: ostra_core::submit::StageVerdict::NeedsUser,
            options,
            ..
        } => match options.first() {
            Some(o) => choice(o, "Under YOLO the stage's recommended answer is taken."),
            None => YoloPlan::Fixed {
                answer: GateAnswer::Choice {
                    option: "other".into(),
                    text: Some("Decide as you recommend and go on.".into()),
                },
                reason: "Under YOLO the stage decides for itself, because it offered no options.".into(),
            },
        },
        GatePayload::StageReview { .. } => choice(
            "retry",
            "Under YOLO a failed stage runs again with its findings while it has rounds left.",
        ),
    })
}

/// Turn the YOLO judge's `answer` into a gate answer, enforcing what must stay true: approval
/// only with a fact-check PASS, and every question answered.
pub fn yolo_answer_from_judge(
    s: &SessionState,
    gate: &GateId,
    answer: &Value,
) -> Result<GateAnswer, String> {
    let g = s.gates.get(gate).ok_or("unknown gate")?;
    match &g.payload {
        GatePayload::OpenQuestions { questions, .. } => {
            let answers: Vec<QuestionAnswer> =
                serde_json::from_value(answer.get("answers").cloned().unwrap_or(Value::Null))
                    .map_err(|e| format!("answers: {e}"))?;
            let complete: Vec<QuestionAnswer> = questions
                .iter()
                .map(|q| {
                    answers
                        .iter()
                        .find(|a| a.id == q.id)
                        .cloned()
                        .unwrap_or_else(|| QuestionAnswer {
                            id: q.id.clone(),
                            question: q.question.clone(),
                            answer: q
                                .options
                                .get(q.recommended)
                                .map(|o| o.label.clone())
                                .unwrap_or_default(),
                        })
                })
                .collect();
            Ok(GateAnswer::Questions { answers: complete })
        }
        GatePayload::SpecApproval { .. } | GatePayload::PlanApproval { .. } => {
            let approved = answer
                .get("approved")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let feedback = answer
                .get("feedback")
                .and_then(|v| v.as_str())
                .map(String::from)
                .filter(|f| !f.trim().is_empty());
            let passed = match &g.payload {
                GatePayload::SpecApproval { .. } => s.ext.os().spec.passed_current(),
                _ => s.ext.os().plan.passed_current(),
            };
            if approved && !passed {
                return Err("approval requires a fact-check PASS".into());
            }
            if !approved && feedback.is_none() {
                return Err("a rejection needs feedback saying what to change".into());
            }
            Ok(GateAnswer::Approval { approved, feedback })
        }
        _ => Err("this gate is answered without the judge".into()),
    }
}
