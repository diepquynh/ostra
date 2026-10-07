//! What the user does to a running session: answer gates, override decisions, add context, pause, stop, skip, and steer.

use super::*;
use crate::state::{Interrupt, SessionState, purpose_key};
use crate::uploads;
use ostra_core::api::{GateView, SessionSummary, UploadRef};
use ostra_core::event::{
    AnswerSource, ContextDelivery, ContextFile, GateAnswer, GatePayload, SessionEvent,
};
use ostra_core::exec::ResumeInfo;
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId};
use ostra_core::policy::{PermissionAnswer, ToolCall};
use serde_json::Value;
use std::path::{Path, PathBuf};

impl Engine {
    pub fn answer_gate(&self, gate: &GateId, answer: GateAnswer) -> Result<GateView, EngineError> {
        let view = self
            .inner
            .db
            .get_gate(gate)?
            .ok_or_else(|| EngineError::NotFound(format!("gate {gate}")))?;
        if view.answer.is_some() {
            return Err(EngineError::Invalid(
                "This gate was already answered.".into(),
            ));
        }
        validate_answer(&self.state(&view.session)?, &view.payload, &answer)?;
        if let (
            GatePayload::Permission { call, .. },
            GateAnswer::Permission {
                answer: PermissionAnswer::AlwaysInWorkspace,
            },
        ) = (&view.payload, &answer)
            && let Some(rule) = suggest_rule(call, &self.inner.workspace_root)
        {
            self.inner.services.add_allow_rule(&rule);
        }
        self.inner.append(
            &view.session,
            SessionEvent::GateAnswered {
                id: gate.clone(),
                source: AnswerSource::User,
                answer: answer.clone(),
                reason: None,
                routed: self.pipeline().answer_needs_route(&view.payload, &answer),
            },
        )?;
        if let GateAnswer::Permission { answer } = answer
            && let Some(tx) = lock(&self.inner.permission_waiters).remove(gate)
        {
            let _ = tx.send(answer);
        }
        self.inner
            .db
            .get_gate(gate)?
            .ok_or_else(|| EngineError::NotFound(gate.to_string()))
    }

    pub fn override_decision(
        &self,
        id: &DecisionId,
        output: Value,
        reason: String,
    ) -> Result<(), EngineError> {
        let session = self
            .inner
            .db
            .decision_session(id)?
            .ok_or_else(|| EngineError::NotFound(format!("decision {id}")))?;
        let st = self.state(&session)?;
        let d = st
            .decisions
            .get(id)
            .ok_or_else(|| EngineError::NotFound(format!("decision {id}")))?;
        if !self.pipeline().can_override(&st, id) {
            return Err(EngineError::Invalid("Work that depends on this decision has already started, so it can no longer be overridden.".into()));
        }
        let valid = self.pipeline().override_ok(d.judge, &output);
        if !valid {
            return Err(EngineError::Invalid(
                "The override does not match the decision's output shape.".into(),
            ));
        }
        self.inner.db.mark_overridden(id, &output, &reason)?;
        self.inner.append(
            &session,
            SessionEvent::DecisionOverridden {
                id: id.clone(),
                output,
                reason,
            },
        )?;
        Ok(())
    }

    pub fn set_yolo(
        &self,
        session: &SessionId,
        enabled: bool,
    ) -> Result<SessionSummary, EngineError> {
        self.inner
            .append(session, SessionEvent::YoloSet { enabled })?;
        if enabled {
            // Takes effect from the next gate or tool call, including asks already waiting.
            let st = self.state(session)?;
            for g in st
                .open_gates()
                .filter(|g| matches!(g.payload, GatePayload::Permission { .. }))
            {
                let _ = self.inner.append(
                    session,
                    SessionEvent::GateAnswered {
                        routed: false,
                        id: g.id.clone(),
                        source: AnswerSource::Yolo,
                        answer: GateAnswer::Permission {
                            answer: PermissionAnswer::AllowOnce,
                        },
                        reason: Some(
                            "YOLO was switched on, so every permission ask is allowed.".into(),
                        ),
                    },
                );
                if let Some(tx) = lock(&self.inner.permission_waiters).remove(&g.id) {
                    let _ = tx.send(PermissionAnswer::AllowOnce);
                }
            }
        }
        self.inner
            .db
            .get_session(session)?
            .ok_or_else(|| EngineError::NotFound(session.to_string()))
    }

    /// Add context to a running session. `Now` interrupts every running execution first, and
    /// each re-runs with the new context (Rule C2).
    pub fn amend(
        &self,
        session: &SessionId,
        text: String,
        files: Vec<ContextFile>,
        uploads: Vec<String>,
        delivery: ContextDelivery,
    ) -> Result<SessionSummary, EngineError> {
        let text = text.trim().to_string();
        if text.is_empty() && files.is_empty() && uploads.is_empty() {
            return Err(EngineError::Invalid(
                "Say what to add or change, tag a file with @, or upload one.".into(),
            ));
        }
        let st = self.state(session)?;
        if st.is_terminal() {
            return Err(EngineError::Invalid(
                "This session has ended. Start a new task instead.".into(),
            ));
        }
        let files = validate_files(&files, &st.projects, &self.inner.workspace_root)?;
        let uploads = uploads::claim(&self.inner.workspace_root, &st.session_root, &uploads)?;
        self.inner.append(
            session,
            SessionEvent::RequestAmended {
                text,
                files,
                uploads,
                delivery,
                // Rule C2: once the request is classified, the judge routes what the user adds.
                routed: st.routes_amendments(),
            },
        )?;
        if delivery == ContextDelivery::Now {
            self.inner.interrupt(session, Interrupt::Context)?;
        }
        self.summary_of(session)
    }

    /// Stage an uploaded file until a new session or an addition claims it (Rule C3).
    pub fn stage_upload(&self, name: &str, bytes: &[u8]) -> Result<UploadRef, EngineError> {
        uploads::stage(&self.inner.workspace_root, name, bytes)
    }

    /// Rule P1: pause a session. Running executions are interrupted and nothing new starts
    /// until [`Engine::resume_session`].
    pub fn pause_session(&self, session: &SessionId) -> Result<SessionSummary, EngineError> {
        let st = self.state(session)?;
        if st.is_terminal() {
            return Err(EngineError::Invalid(
                "This session has already ended.".into(),
            ));
        }
        if st.paused {
            return Err(EngineError::Invalid(
                "This session is already paused.".into(),
            ));
        }
        self.inner.append(session, SessionEvent::SessionPaused)?;
        self.inner.interrupt(session, Interrupt::Pause)?;
        self.summary_of(session)
    }

    /// Rule P2: continue a paused session. Each paused execution resumes where it stopped.
    pub fn resume_session(&self, session: &SessionId) -> Result<SessionSummary, EngineError> {
        let st = self.state(session)?;
        if !st.paused || st.is_terminal() {
            return Err(EngineError::Invalid("This session is not paused.".into()));
        }
        self.inner.append(session, SessionEvent::SessionResumed)?;
        self.summary_of(session)
    }

    /// Stop a session: cancel its running executions, deny its waiting permission asks, and end
    /// it. Nothing the session already did is undone.
    pub fn stop_session(&self, session: &SessionId) -> Result<SessionSummary, EngineError> {
        let st = self.state(session)?;
        if st.is_terminal() {
            return Err(EngineError::Invalid(
                "This session has already ended.".into(),
            ));
        }
        self.inner.append(
            session,
            SessionEvent::SessionFailed {
                error: STOPPED_BY_USER.into(),
            },
        )?;
        for r in st.running_executions() {
            if let Some((_, token)) = lock(&self.inner.execs).get(&r.id) {
                token.cancel();
            }
        }
        for g in st.open_gates() {
            if let Some(tx) = lock(&self.inner.permission_waiters).remove(&g.id) {
                let _ = tx.send(PermissionAnswer::Deny);
            }
        }
        self.inner
            .db
            .get_session(session)?
            .ok_or_else(|| EngineError::NotFound(session.to_string()))
    }

    pub fn cancel_execution(&self, id: &ExecutionId) -> Result<(), EngineError> {
        match lock(&self.inner.execs).get(id) {
            Some((_, token)) => {
                token.cancel();
                Ok(())
            }
            None => Err(EngineError::Invalid(
                "This execution is not running.".into(),
            )),
        }
    }

    /// Rule U1: skip a running execution's task. The run stops, and the session moves on without
    /// its result instead of opening a failure gate.
    pub fn skip_execution(&self, id: &ExecutionId) -> Result<(), EngineError> {
        let session = self
            .inner
            .db
            .get_execution(id)?
            .ok_or_else(|| EngineError::NotFound(format!("execution {id}")))?
            .session
            .ok_or_else(|| {
                EngineError::Invalid("A side-panel answer has no task to skip.".into())
            })?;
        if !self.pipeline().can_skip(&self.state(&session)?, id) {
            return Err(EngineError::Invalid(
                "Only a running research, test analysis, docs, or architecture task can be skipped. Cancel the execution instead.".into(),
            ));
        }
        self.inner
            .append(&session, SessionEvent::ExecutionSkipped { id: id.clone() })?;
        self.inner.interrupt(&session, Interrupt::Skipped)
    }

    /// Rule U2: send one execution a correction. A running run stops and resumes in place with it
    /// as its next message; a paused one reads it when the session continues.
    pub fn steer_execution(&self, id: &ExecutionId, text: String) -> Result<(), EngineError> {
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err(EngineError::Invalid(
                "Say what the agent should do differently.".into(),
            ));
        }
        let session = self
            .inner
            .db
            .get_execution(id)?
            .ok_or_else(|| EngineError::NotFound(format!("execution {id}")))?
            .session
            .ok_or_else(|| {
                EngineError::Invalid("A side-panel answer takes no correction.".into())
            })?;
        if !self.state(&session)?.can_steer(id) {
            return Err(EngineError::Invalid(
                "Only a running or paused execution takes a correction. Add context to the session instead.".into(),
            ));
        }
        self.inner.append(
            &session,
            SessionEvent::ExecutionSteered {
                id: id.clone(),
                text,
            },
        )?;
        self.inner.interrupt(&session, Interrupt::Steer)
    }

    /// Rule U2: take back a correction its paused run has not read yet.
    pub fn withdraw_steer(&self, id: &ExecutionId) -> Result<(), EngineError> {
        let session = self
            .inner
            .db
            .get_execution(id)?
            .and_then(|v| v.session)
            .ok_or_else(|| EngineError::NotFound(format!("execution {id}")))?;
        if !self.state(&session)?.steer_queued(id) {
            return Err(EngineError::Invalid(
                "This correction was already sent, so it cannot be withdrawn. Send another one instead.".into(),
            ));
        }
        self.inner
            .append(&session, SessionEvent::SteerWithdrawn { id: id.clone() })?;
        Ok(())
    }

    /// Rule C2: take back queued context before anything reads it. Context sent now cannot be.
    pub fn withdraw_amendment(
        &self,
        session: &SessionId,
        index: u32,
    ) -> Result<SessionSummary, EngineError> {
        let st = self.state(session)?;
        if !st.held_amendments().any(|i| i == index as usize) {
            return Err(EngineError::Invalid(
                "Only queued context that no step has read yet can be withdrawn. Add a correction instead.".into(),
            ));
        }
        self.inner
            .append(session, SessionEvent::AmendmentWithdrawn { index })?;
        self.summary_of(session)
    }

    /// Resume a failed, cancelled, or interrupted execution: answer its failure gate with retry and
    /// hand the re-run the earlier execution to continue from.
    pub fn resume_execution(&self, id: &ExecutionId) -> Result<(), EngineError> {
        let view = self
            .inner
            .db
            .get_execution(id)?
            .ok_or_else(|| EngineError::NotFound(format!("execution {id}")))?;
        let session = view
            .session
            .clone()
            .ok_or_else(|| EngineError::Invalid("Side-panel answers cannot be resumed.".into()))?;
        let st = self.state(&session)?;
        let gate = st
            .open_gates()
            .find(|g| matches!(&g.payload, GatePayload::ExecutionFailed { execution, .. } | GatePayload::HarnessFailure { execution, .. } if execution == id))
            .map(|g| g.id.clone())
            .ok_or_else(|| EngineError::Invalid("Only a failed or cancelled execution waiting on a decision can be resumed.".into()))?;
        if let Some(purpose) = &view.purpose {
            lock(&self.inner.resume_hints).insert(
                purpose_key(purpose),
                ResumeInfo {
                    from: id.clone(),
                    native_session_id: view.native_session_id.clone(),
                    note: None,
                    inspect: false,
                },
            );
        }
        self.answer_gate(
            &gate,
            GateAnswer::Choice {
                option: "retry".into(),
                text: None,
            },
        )?;
        Ok(())
    }
}

/// Rule U2: the message a run resumes with after the user sent it a correction.
pub fn steer_note(text: &str) -> String {
    format!(
        "The user stopped you to send this correction. Follow it, then continue the workflow from where you stopped, because the rest of your task is unchanged.\n\n{text}"
    )
}

pub(crate) fn validate_answer(
    st: &SessionState,
    payload: &GatePayload,
    answer: &GateAnswer,
) -> Result<(), EngineError> {
    if let Some(r) = st.pipeline.validate_answer(st, payload, answer) {
        return r.map_err(EngineError::Invalid);
    }
    let ok = matches!(
        (payload, answer),
        (
            GatePayload::BudgetReached { .. }
                | GatePayload::HarnessFailure { .. }
                | GatePayload::ExecutionFailed { .. }
                | GatePayload::StageReview { .. },
            GateAnswer::Choice { .. }
        ) | (
            GatePayload::Permission { .. },
            GateAnswer::Permission { .. }
        )
    );
    if !ok {
        return Err(EngineError::Invalid(
            "That answer does not fit this gate.".into(),
        ));
    }
    // Rule WF5: a failed stage takes retry, continue, or stop; a question takes an option, other
    // with text, or stop.
    if let (
        GatePayload::StageReview {
            verdict, options, ..
        },
        GateAnswer::Choice { option, text },
    ) = (payload, answer)
    {
        let has_text = text.as_ref().is_some_and(|t| !t.trim().is_empty());
        let ok = match verdict {
            ostra_core::submit::StageVerdict::Fail => {
                matches!(option.as_str(), "retry" | "continue" | "stop")
            }
            _ => {
                option == "stop"
                    || (option == "other" && has_text)
                    || options.iter().any(|o| o == option)
            }
        };
        if !ok {
            return Err(EngineError::Invalid(match verdict {
                ostra_core::submit::StageVerdict::Fail => "Answer retry, continue, or stop.".into(),
                _ => "Pick one of the stage's options, or other with your answer, or stop.".into(),
            }));
        }
    }
    Ok(())
}

/// The rule "always in this workspace" adds for a call.
pub fn suggest_rule(call: &ToolCall, repo_root: &Path) -> Option<String> {
    match call.tool.as_str() {
        "Bash" => {
            let cmd = call.str_field("command")?.trim();
            let words: Vec<&str> = cmd.split_whitespace().take(2).collect();
            match words.as_slice() {
                [] => None,
                [one] => Some(format!("Bash({one} *)")),
                [a, b] if b.starts_with('-') => Some(format!("Bash({a} *)")),
                [a, b] => Some(format!("Bash({a} {b} *)")),
                _ => None,
            }
        }
        "Write" | "Edit" => {
            let path = PathBuf::from(call.str_field("file_path")?);
            let rel = path
                .strip_prefix(repo_root)
                .ok()
                .map(|p| p.to_path_buf())
                .unwrap_or(path);
            let dir = rel
                .parent()
                .map(|p| p.display().to_string())
                .filter(|d| !d.is_empty());
            Some(match dir {
                Some(d) => format!("Edit({d}/**)"),
                None => "Edit(*)".into(),
            })
        }
        "WebFetch" => {
            let url = call.str_field("url")?;
            let host = url.split("://").nth(1)?.split(['/', ':', '?']).next()?;
            Some(format!("WebFetch(domain:{host})"))
        }
        // Rule O1: no allow rule stands in for the user's answer.
        other if ostra_core::manage::changes_ostra(other) => None,
        other => Some(other.to_string()),
    }
}
