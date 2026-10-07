//! Judge calls, prompt nodes, and YOLO answers.

use super::*;
use crate::pipeline::YoloPlan;
use crate::state::Interrupt;
use ostra_core::config::{RouteQuery, resolve_route};
use ostra_core::event::{AnswerSource, JudgeKind, SessionEvent};
use ostra_core::ids::{DecisionId, GateId, SessionId};
use ostra_core::model::Tier;
use serde_json::Value;
use std::sync::Arc;

impl Inner {
    /// Rule WB3: one model call for a prompt node, on its tier, checked against its output
    /// schema. Returns the output or the error, and what the calls cost.
    pub(crate) async fn prompt_node(
        &self,
        d: &ostra_core::workflow::StageDef,
        tier: Option<Tier>,
        effort: Option<ostra_core::model::Effort>,
        inputs: &serde_json::Map<String, Value>,
        notes: &[String],
    ) -> (Option<Value>, Option<String>, f64) {
        let Some(system) = self.services.factory().judge_prompt("prompt-node") else {
            return (None, Some("Ostra has no prompt-node prompt.".into()), 0.0);
        };
        let schema = d.output_schema.clone().unwrap_or(Value::Null);
        let tier = tier.unwrap_or(Tier::Balanced);
        let route = match resolve_route(
            &self.services.global(),
            &self.services.workspace(),
            RouteQuery {
                tier_override: Some(tier),
                ..RouteQuery::new(ostra_core::agent::JUDGE_ROUTE, tier)
            },
        ) {
            Ok(r) => r,
            Err(e) => return (None, Some(e.0), 0.0),
        };
        let mut user = format!(
            "## Prompt\n\n{}\n\n## Inputs\n\n{}\n",
            d.instructions.as_deref().unwrap_or_default(),
            serde_json::to_string_pretty(inputs).unwrap_or_default()
        );
        if !notes.is_empty() {
            user.push_str("\n## Your earlier answers were refused\n\n");
            for n in notes {
                user.push_str(&format!("- {n}\n"));
            }
        }
        let mut cost = 0.0;
        let mut last = String::new();
        for _ in 0..2 {
            match self
                .services
                .judge(
                    &route,
                    &system,
                    &user,
                    schema.clone(),
                    effort.unwrap_or(ostra_core::model::Effort::Medium),
                )
                .await
            {
                Ok((value, usage)) => {
                    cost += usage.cost_usd;
                    let issues = ostra_core::schema_check::check(&schema, &value, "");
                    if issues.is_empty() {
                        return (Some(value), None, cost);
                    }
                    last = format!(
                        "the answer did not match the output schema: {}",
                        issues.join("; ")
                    );
                }
                Err(e) => last = e,
            }
        }
        (
            None,
            Some(format!("The prompt node's model call failed: {last}")),
            cost,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn call_judge(
        &self,
        session: &SessionId,
        kind: JudgeKind,
        subject: Option<String>,
        input: String,
        summary: String,
        schema: Value,
        validate: impl Fn(&Value) -> bool,
    ) -> Result<Value, EngineError> {
        let factory = self.services.factory();
        let system = factory.judge_prompt(kind.as_str()).ok_or_else(|| {
            EngineError::Invalid(format!("No prompt for the {} judge.", kind.as_str()))
        })?;
        let global = self.services.global();
        let settings = self.services.workspace();
        let route = resolve_route(
            &global,
            &settings,
            RouteQuery::new(ostra_core::agent::JUDGE_ROUTE, Tier::Advanced),
        )
        .map_err(|e| EngineError::Invalid(e.0))?;
        let mut last = String::new();
        for attempt in 0..3 {
            match self
                .services
                .judge(
                    &route,
                    &system,
                    &input,
                    schema.clone(),
                    ostra_core::model::Effort::Low,
                )
                .await
            {
                Ok((value, usage)) => {
                    *lock(&self.judge_cost).entry(session.clone()).or_insert(0.0) += usage.cost_usd;
                    if validate(&value) {
                        let reason = self.pipeline().judge_reason(&value);
                        self.append(
                            session,
                            SessionEvent::DecisionMade {
                                id: DecisionId::new(),
                                judge: kind,
                                subject: subject.clone(),
                                input_summary: summary.clone(),
                                output: value.clone(),
                                reason,
                            },
                        )?;
                        return Ok(value);
                    }
                    last = "the answer did not match the schema".into();
                }
                Err(e) => last = e,
            }
            tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1))).await;
        }
        Err(EngineError::Invalid(format!(
            "The {} judge call failed: {last}",
            kind.as_str()
        )))
    }

    pub(crate) async fn perform_judge(
        self: &Arc<Self>,
        session: &SessionId,
        kind: JudgeKind,
        subject: Option<String>,
    ) -> Result<(), EngineError> {
        let st = self.snapshot(session)?;
        let (input, summary) =
            self.pipeline()
                .judge_input(&st, kind, subject.as_deref(), &self.project_facts());
        let schema = self.pipeline().judge_schema(kind, None);
        let pipeline = self.pipeline().clone();
        let validate = move |v: &Value| -> bool { pipeline.judge_output_ok(kind, v) };
        match self
            .call_judge(session, kind, subject, input, summary, schema, validate)
            .await
        {
            // Rule U1: research the decision skipped stops now.
            Ok(_) if self.pipeline().judge_skips_work(kind) => {
                self.interrupt(session, Interrupt::Skipped)
            }
            Ok(_) => Ok(()),
            Err(e) => {
                if let Some(output) = self.pipeline().judge_fallback(&st, kind, &e.to_string()) {
                    let reason = self.pipeline().judge_reason(&output);
                    self.append(
                        session,
                        SessionEvent::DecisionMade {
                            id: DecisionId::new(),
                            judge: kind,
                            subject: None,
                            input_summary: "Fallback after a failed judge call".into(),
                            output,
                            reason,
                        },
                    )?;
                    return Ok(());
                }
                self.append(
                    session,
                    SessionEvent::SessionFailed {
                        error: format!(
                            "{e}. Check that the judge route has a provider with a working API key."
                        ),
                    },
                )?;
                Ok(())
            }
        }
    }

    pub(crate) async fn perform_yolo(
        self: &Arc<Self>,
        session: &SessionId,
        gate: GateId,
    ) -> Result<(), EngineError> {
        let st = self.snapshot(session)?;
        let Some(plan) = self.pipeline().yolo_plan(&st, &gate) else {
            return Ok(());
        };
        let (answer, reason) = match plan {
            YoloPlan::Fixed { answer, reason } => (answer, reason),
            YoloPlan::Judge { schema } => {
                let (input, summary) = self.pipeline().judge_input(
                    &st,
                    JudgeKind::YoloAnswer,
                    Some(gate.as_str()),
                    &[],
                );
                let full = self
                    .pipeline()
                    .judge_schema(JudgeKind::YoloAnswer, Some(schema));
                let value = match self
                    .call_judge(
                        session,
                        JudgeKind::YoloAnswer,
                        Some(gate.to_string()),
                        input,
                        summary,
                        full,
                        |_| true,
                    )
                    .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        self.append(
                            session,
                            SessionEvent::Note {
                                message: format!(
                                    "YOLO could not answer \"{}\": {e}. The gate waits for you.",
                                    st.gates
                                        .get(&gate)
                                        .map(|g| g.title.as_str())
                                        .unwrap_or("a gate")
                                ),
                            },
                        )?;
                        return Ok(());
                    }
                };
                let reason = self.pipeline().judge_reason(&value);
                let st = self.snapshot(session)?;
                match self.pipeline().yolo_answer_from_judge(
                    &st,
                    &gate,
                    value.get("answer").unwrap_or(&Value::Null),
                ) {
                    Ok(a) => (a, reason),
                    Err(e) => {
                        self.append(
                            session,
                            SessionEvent::Note {
                                message: format!(
                                    "The YOLO answer was refused: {e}. The gate waits for you."
                                ),
                            },
                        )?;
                        return Ok(());
                    }
                }
            }
        };
        let st = self.snapshot(session)?;
        let Some(g) = st.gates.get(&gate).filter(|g| g.answer.is_none()) else {
            return Ok(());
        };
        let routed = self.pipeline().answer_needs_route(&g.payload, &answer);
        self.append(
            session,
            SessionEvent::GateAnswered {
                id: gate,
                source: AnswerSource::Yolo,
                answer,
                reason: Some(reason),
                routed,
            },
        )?;
        Ok(())
    }
}
