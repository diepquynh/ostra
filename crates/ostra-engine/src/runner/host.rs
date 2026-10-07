//! What a running execution asks of the engine: deltas, permissions, messages, and checks.

use super::*;
use crate::state::{Interrupt, SessionState};
use ostra_core::event::{AnswerSource, GateAnswer, GatePayload, SessionEvent};
use ostra_core::exec::{ExecutionDelta, ExecutionHost, Wake};
use ostra_core::ids::{ExecutionId, GateId, SessionId};
use ostra_core::policy::{PermissionAnswer, RuleRef, ToolCall};
use ostra_store::SessionUpdate;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::oneshot;

impl EngineHost {
    /// Rule P3: record a containment signal, at most `PAUSE_AFTER` per execution so a retry loop
    /// cannot flood the log, and interrupt the session when this one paused it.
    fn record_signal(
        &self,
        session: &SessionId,
        signal: ostra_core::containment::ContainmentSignal,
    ) {
        let seen = |st: &SessionState| st.signals.get(&self.execution).map_or(0, Vec::len);
        let Ok(st) = self.inner.snapshot(session) else {
            return;
        };
        if seen(&st) >= ostra_core::containment::PAUSE_AFTER {
            return;
        }
        let event = SessionEvent::ContainmentSignal {
            execution: self.execution.clone(),
            signal,
        };
        if let Err(e) = self.inner.append(session, event) {
            tracing::error!(session = %session, "could not record a containment signal: {e}");
            return;
        }
        // Appends are serialized, so exactly one of them brings the count to `PAUSE_AFTER`.
        if let Ok(st) = self.inner.snapshot(session)
            && st.contained.as_ref() == Some(&self.execution)
            && seen(&st) == ostra_core::containment::PAUSE_AFTER
            && let Err(e) = self.inner.interrupt(session, Interrupt::Pause)
        {
            tracing::error!(session = %session, "could not pause after containment signals: {e}");
        }
    }
}

#[async_trait::async_trait]
impl ExecutionHost for EngineHost {
    fn emit(&self, delta: ExecutionDelta) {
        let summary = match &delta {
            // The submit call ends every run, so the action before it says more.
            ExecutionDelta::ToolCall { call, .. } if !call.tool.contains("submit_") => Some(
                ostra_core::api::tool_summary(call, self.repo_root.as_deref()),
            ),
            ExecutionDelta::Status { message } => Some(ostra_core::api::summary_line(message)),
            _ => None,
        };
        if let Some(line) = summary.filter(|l| !l.is_empty()) {
            let _ = self.inner.db.set_execution_summary(&self.execution, &line);
        }
        if let ExecutionDelta::NativeSessionId { id } = &delta {
            let _ = self.inner.db.set_native_session_id(&self.execution, id);
        }
        let delta = match delta {
            ExecutionDelta::Usage { usage: run } => {
                let mut usage = self.usage_base;
                usage.add(&run);
                ExecutionDelta::Usage { usage }
            }
            other => other,
        };
        if let ExecutionDelta::Usage { usage } = &delta {
            let _ = self.inner.db.update_execution_usage(&self.execution, usage);
            // Keep the session list's cost current while executions run, not only when they end.
            if let Some(session) = &self.session
                && let Ok(execs) = self.inner.db.list_executions(session)
            {
                let judge = lock(&self.inner.judge_cost)
                    .get(session)
                    .copied()
                    .unwrap_or(0.0);
                let cost = execs.iter().map(|e| e.usage.cost_usd).sum::<f64>() + judge;
                if let Ok(summary) = self.inner.db.update_session(
                    session,
                    &SessionUpdate {
                        cost_usd: Some(cost),
                        ..Default::default()
                    },
                ) {
                    let _ = self.inner.tx.send(EngineNotice::SessionUpdated { summary });
                }
            }
        }
        if let Ok(item) = self.inner.db.append_activity(&self.execution, &delta) {
            let _ = self.inner.tx.send(EngineNotice::Delta {
                execution: self.execution.clone(),
                item,
            });
        }
        if let Some(session) = &self.session
            && let Some(signal) = ostra_core::containment::classify(&delta)
        {
            self.record_signal(session, signal);
        }
    }

    async fn ask_permission(
        &self,
        call: &ToolCall,
        reason: &str,
        rule: &RuleRef,
    ) -> PermissionAnswer {
        let Some(session) = &self.session else {
            // Side-panel runs are read-only and never ask.
            return PermissionAnswer::Deny;
        };
        let Ok(st) = self.inner.snapshot(session) else {
            return PermissionAnswer::Deny;
        };
        let agent = st
            .executions
            .get(&self.execution)
            .map(|r| r.agent)
            .unwrap_or_else(|| st.pipeline.default_agent(ostra_core::Contract::Answer));
        let repo = st
            .executions
            .get(&self.execution)
            .and_then(|r| st.project_path(&r.project))
            .unwrap_or_default();
        let gate = GateId::new();
        let payload = GatePayload::Permission {
            execution: self.execution.clone(),
            agent,
            call: call.clone(),
            reason: reason.to_string(),
            rule: rule.clone(),
            suggestion: suggest_rule(call, &repo),
        };
        let title = format!("{agent} asks to run {}", call.tool);
        if st.yolo {
            let _ = self.inner.append(
                session,
                SessionEvent::GateOpened {
                    id: gate.clone(),
                    title,
                    explanation: reason.into(),
                    payload,
                },
            );
            let _ = self.inner.append(
                session,
                SessionEvent::GateAnswered {
                    routed: false,
                    id: gate,
                    source: AnswerSource::Yolo,
                    answer: GateAnswer::Permission {
                        answer: PermissionAnswer::AllowOnce,
                    },
                    reason: Some("Under YOLO every permission ask is allowed.".into()),
                },
            );
            return PermissionAnswer::AllowOnce;
        }
        let (tx, rx) = oneshot::channel();
        lock(&self.inner.permission_waiters).insert(gate.clone(), tx);
        if self
            .inner
            .append(
                session,
                SessionEvent::GateOpened {
                    id: gate.clone(),
                    title,
                    explanation: reason.into(),
                    payload,
                },
            )
            .is_err()
        {
            lock(&self.inner.permission_waiters).remove(&gate);
            return PermissionAnswer::Deny;
        }
        rx.await.unwrap_or(PermissionAnswer::Deny)
    }

    fn record_message(&self, role: &str, content: &Value) {
        let _ = self.inner.db.append_message(&self.execution, role, content);
    }

    fn checkpoints(&self, plugin: &str) -> Arc<dyn ostra_core::plugin::Checkpoints> {
        match &self.session {
            Some(session) => Arc::new(SessionCheckpoints {
                inner: self.inner.clone(),
                session: session.clone(),
                plugin: plugin.to_string(),
            }),
            None => Arc::new(ostra_core::plugin::NoCheckpoints),
        }
    }

    fn transcript(&self, execution: &ExecutionId) -> Vec<(String, Value)> {
        self.inner
            .db
            .messages(execution)
            .map(|m| m.into_iter().map(|m| (m.role, m.content)).collect())
            .unwrap_or_default()
    }

    fn yolo(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| self.inner.sessions_yolo(s))
    }

    async fn wait_for_wake(&self) -> Option<Wake> {
        let session = self.session.clone()?;
        let mut released = false;
        loop {
            let rung = self.inner.bell.notified();
            if let Some(wake) = self.inner.take_messages(&session, &self.execution) {
                if released {
                    let slot = self.inner.acquire_slot().await;
                    *lock(&self.slot) = Some(slot);
                }
                return Some(wake);
            }
            let st = self.inner.snapshot(&session).ok()?;
            if st.is_terminal() || !st.is_waiting(&self.execution) {
                return None;
            }
            drop(st);
            // Rule SM3: a waiting run holds no slot, so the run it waits for can start.
            if !released {
                drop(lock(&self.slot).take());
                released = true;
            }
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), rung).await;
        }
    }

    fn take_messages(&self) -> Option<Wake> {
        self.inner
            .take_messages(self.session.as_ref()?, &self.execution)
    }

    fn has_messages(&self) -> bool {
        self.session.as_ref().is_some_and(|s| {
            self.inner
                .snapshot(s)
                .is_ok_and(|st| st.next_delivery(&self.execution).is_some())
        })
    }

    fn submit_blocked(&self) -> Option<String> {
        self.inner
            .snapshot(self.session.as_ref()?)
            .ok()?
            .submit_blocked(&self.execution)
    }

    fn check_submit(&self, contract: ostra_core::Contract, input: &Value) -> Vec<String> {
        let params = self
            .session
            .as_ref()
            .and_then(|s| self.inner.snapshot(s).ok())
            .and_then(|st| st.executions.get(&self.execution).map(|r| r.params.clone()))
            .unwrap_or_default();
        self.inner.pipeline.check_submit(contract, input, &params)
    }
}
