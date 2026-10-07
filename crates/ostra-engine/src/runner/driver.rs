//! The session driver: append events, plan, and perform each step once, under the slot limiter.

use super::*;
use crate::pipeline::{PipelineRef, ProjectFacts};
use crate::plan::{PlanCtx, SpawnRequest, Step, next_steps};
use crate::services::Notice;
use crate::state::{Interrupt, SessionState};
use ostra_core::config::{ProjectProfile, load_toml};
use ostra_core::event::{
    AnswerSource, GateAnswer, GatePayload, SessionEvent, SessionKind, StoredEvent,
};
use ostra_core::exec::{ExecutionResult, ExecutionStatus, Wake};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, GateId, SessionId};
use ostra_core::paths;
use ostra_core::policy::PermissionAnswer;
use ostra_core::workflow::StageRun;
use ostra_store::SessionUpdate;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

impl Inner {
    pub(crate) fn pipeline(&self) -> &PipelineRef {
        &self.pipeline
    }

    pub(crate) fn load(&self, id: &SessionId) -> Result<Arc<Live>, EngineError> {
        if let Some(l) = lock(&self.sessions).get(id) {
            return Ok(l.clone());
        }
        if self.db.get_session(id)?.is_none() {
            return Err(EngineError::NotFound(format!("session {id}")));
        }
        let events = self.db.events(id)?;
        let state = SessionState::fold(self.pipeline.clone(), id.clone(), &events);
        let live = Arc::new(Live {
            state: Mutex::new(state),
            wake: Notify::new(),
            inflight: Mutex::new(HashSet::new()),
            driving: AtomicBool::new(false),
        });
        Ok(lock(&self.sessions)
            .entry(id.clone())
            .or_insert(live)
            .clone())
    }

    /// Append an event: store it, fold it, broadcast it, refresh the session row, wake the driver.
    pub(crate) fn append(
        &self,
        session: &SessionId,
        event: SessionEvent,
    ) -> Result<StoredEvent, EngineError> {
        let live = self.load(session)?;
        let stored = {
            // The state lock serializes appends per session, so seq order is fold order.
            let mut st = lock(&live.state);
            let was_terminal = st.is_terminal();
            // A stop can land while a step is being set up; nothing may start after it, and this
            // check sits under the state lock, so it cannot race the stop.
            if was_terminal
                && matches!(
                    event,
                    SessionEvent::ExecutionStarted { .. }
                        | SessionEvent::ExecutionResumed { .. }
                        | SessionEvent::CommandStarted { .. }
                )
            {
                return Err(EngineError::Ended);
            }
            let stored = self.db.append_event(session, &event)?;
            self.materialize(session, &event)?;
            st.apply(&stored);
            let ended = !was_terminal && st.is_terminal();
            if ended {
                // An ended session runs no more tools, and a later one must not build from its downloads.
                ostra_sandbox::remove_session_cache(session);
            }
            let init_changed = matches!(st.kind, SessionKind::Init { .. })
                && (matches!(event, SessionEvent::SessionCreated { .. }) || ended);
            let cost = lock(&self.judge_cost).get(session).copied().unwrap_or(0.0);
            let summary = self.pipeline().summary(&st, &self.workspace_id, cost);
            let _ = self.db.update_session(
                session,
                &SessionUpdate {
                    category: Some(summary.category),
                    status: Some(summary.status),
                    lane: Some(summary.lane),
                    stage_label: Some(summary.stage_label.clone()),
                    projects: Some(summary.projects.clone()),
                    yolo: Some(summary.yolo),
                    cost_usd: Some(
                        self.db
                            .list_executions(session)
                            .map(|v| v.iter().map(|e| e.usage.cost_usd).sum::<f64>() + cost)
                            .unwrap_or(summary.cost_usd),
                    ),
                    request: None,
                    title: Some(summary.title.clone()),
                },
            );
            if let Ok(Some(s)) = self.db.get_session(session) {
                let _ = self.tx.send(EngineNotice::SessionUpdated { summary: s });
            }
            if init_changed {
                let _ = self.tx.send(EngineNotice::ProjectsChanged);
            }
            stored
        };
        let _ = self.tx.send(EngineNotice::Event {
            session: session.clone(),
            stored: stored.clone(),
        });
        self.bell.notify_waiters();
        self.push_for(session, &event);
        live.wake.notify_one();
        Ok(stored)
    }

    pub(crate) fn materialize(
        &self,
        session: &SessionId,
        event: &SessionEvent,
    ) -> Result<(), EngineError> {
        match event {
            SessionEvent::GateOpened {
                id,
                title,
                explanation,
                payload,
            } => {
                self.db
                    .upsert_gate(session, id, title, explanation, payload)?;
            }
            SessionEvent::GateAnswered {
                id,
                source,
                answer,
                reason,
                ..
            } => {
                let _ = self.db.answer_gate(id, *source, answer, reason.as_deref());
            }
            SessionEvent::DecisionMade {
                id,
                judge,
                subject,
                input_summary,
                output,
                reason,
            } => {
                self.db.insert_decision(
                    session,
                    id,
                    *judge,
                    subject.as_deref(),
                    input_summary,
                    output,
                    reason,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn push_for(&self, session: &SessionId, event: &SessionEvent) {
        let url = format!("/s/{session}");
        let notice = match event {
            SessionEvent::GateOpened { title, payload, .. } => {
                let yolo = self.sessions_yolo(session);
                if yolo && !matches!(payload, GatePayload::HarnessFailure { .. }) {
                    return;
                }
                let body = match payload {
                    GatePayload::HarnessFailure { harness, .. } => {
                        format!("{} needs you to log in.", harness.display_name())
                    }
                    GatePayload::Permission { reason, .. } => reason.clone(),
                    _ => "A step is waiting for your decision.".into(),
                };
                Notice {
                    title: title.clone(),
                    body,
                    url,
                    tag: format!("gate-{session}"),
                }
            }
            SessionEvent::SessionCompleted { summary, .. } => Notice {
                title: "Session complete".into(),
                body: summary.clone(),
                url,
                tag: format!("done-{session}"),
            },
            SessionEvent::SessionFailed { error } => Notice {
                title: "Session stopped".into(),
                body: error.clone(),
                url,
                tag: format!("done-{session}"),
            },
            SessionEvent::PhaseBlocked { phase, reason, .. } => Notice {
                title: format!("Phase {phase} is blocked"),
                body: reason.clone(),
                url,
                tag: format!("blocked-{session}-{phase}"),
            },
            SessionEvent::ContainmentSignal { execution, signal } => {
                let Ok(st) = self.snapshot(session) else {
                    return;
                };
                if st.contained.as_ref() != Some(execution)
                    || st.signals.get(execution).map_or(0, Vec::len)
                        != ostra_core::containment::PAUSE_AFTER
                {
                    return;
                }
                Notice {
                    title: "Session paused".into(),
                    body: format!(
                        "Ostra paused the session because {}. Read the execution's Activity, then continue or stop.",
                        signal.describe()
                    ),
                    url,
                    tag: format!("contained-{session}"),
                }
            }
            _ => return,
        };
        self.services.notify(notice);
    }

    pub(crate) fn sessions_yolo(&self, session: &SessionId) -> bool {
        lock(&self.sessions)
            .get(session)
            .map(|l| lock(&l.state).yolo)
            .unwrap_or(false)
    }

    pub(crate) fn ensure_driver(self: &Arc<Self>, id: &SessionId, live: Arc<Live>) {
        if live.driving.swap(true, Ordering::SeqCst) {
            live.wake.notify_one();
            return;
        }
        let inner = self.clone();
        let id = id.clone();
        tokio::spawn(async move {
            inner.drive(id, live).await;
        });
    }

    pub(crate) async fn drive(self: Arc<Self>, id: SessionId, live: Arc<Live>) {
        loop {
            let steps = {
                let st = lock(&live.state);
                if st.is_terminal() {
                    break;
                }
                let ctx = self.plan_ctx(&st);
                next_steps(&st, &ctx)
            };
            for step in steps {
                let key = step.key();
                if !lock(&live.inflight).insert(key.clone()) {
                    continue;
                }
                let inner = self.clone();
                let sid = id.clone();
                let l = live.clone();
                tokio::spawn(async move {
                    if let Err(e) = inner.perform(&sid, step).await
                        && !matches!(e, EngineError::Ended)
                    {
                        tracing::error!(session = %sid, "step failed: {e}");
                        let _ = inner.append(
                            &sid,
                            SessionEvent::Note {
                                message: format!("A step failed: {e}"),
                            },
                        );
                    }
                    lock(&l.inflight).remove(&key);
                    l.wake.notify_one();
                });
            }
            live.wake.notified().await;
        }
        live.driving.store(false, Ordering::SeqCst);
    }

    pub(crate) fn plan_ctx(&self, st: &SessionState) -> PlanCtx {
        let mut ctx = PlanCtx::default();
        let budget = self.services.workspace().limits.session_budget_usd;
        ctx.budget_usd = (budget > 0.0).then_some(budget);
        for p in &st.projects {
            let profile: ProjectProfile =
                load_toml(&paths::project_profile(&p.path)).unwrap_or_default();
            ctx.format_commands.insert(
                p.key.clone(),
                profile.commands.format.filter(|c| !c.trim().is_empty()),
            );
        }
        ctx
    }

    pub(crate) fn snapshot(&self, session: &SessionId) -> Result<SessionState, EngineError> {
        Ok(lock(&self.load(session)?.state).clone())
    }

    /// Cancel the executions the fold marked as interrupting, and deny their waiting permission
    /// asks, because the run that asked is ending.
    pub(crate) fn interrupt(&self, session: &SessionId, why: Interrupt) -> Result<(), EngineError> {
        let st = self.snapshot(session)?;
        let ids: Vec<&ExecutionId> = st
            .interrupting
            .iter()
            .filter(|(_, w)| **w == why)
            .map(|(id, _)| id)
            .collect();
        for id in &ids {
            if let Some((_, token)) = lock(&self.execs).get(*id) {
                token.cancel();
            }
        }
        let asks: Vec<GateId> = st
            .open_gates()
            .filter(|g| matches!(&g.payload, GatePayload::Permission { execution, .. } if ids.contains(&execution)))
            .map(|g| g.id.clone())
            .collect();
        for g in asks {
            if let Some(tx) = lock(&self.permission_waiters).remove(&g) {
                let _ = tx.send(PermissionAnswer::Deny);
            }
            self.append(
                session,
                SessionEvent::GateAnswered {
                    routed: false,
                    id: g,
                    source: AnswerSource::Engine,
                    answer: GateAnswer::Permission {
                        answer: PermissionAnswer::Deny,
                    },
                    reason: Some(format!(
                        "The execution that asked ended: {}",
                        why.message().to_lowercase()
                    )),
                },
            )?;
        }
        Ok(())
    }

    /// Rule PL8: plugin `plugin`'s checkpoints in `session`.
    pub(crate) fn checkpoints(
        self: &Arc<Self>,
        session: &SessionId,
        plugin: &str,
    ) -> Arc<dyn ostra_core::plugin::Checkpoints> {
        Arc::new(SessionCheckpoints {
            inner: self.clone(),
            session: session.clone(),
            plugin: plugin.to_string(),
        })
    }

    pub(crate) async fn perform(
        self: &Arc<Self>,
        session: &SessionId,
        step: Step,
    ) -> Result<(), EngineError> {
        let step = match self
            .pipeline()
            .perform(&StepHost(self.clone()), session, step)
            .await
        {
            Ok(done) => return done,
            Err(step) => step,
        };
        match step {
            Step::Judge { judge, subject } => self.perform_judge(session, judge, subject).await,
            Step::Spawn(req) => self.perform_spawn(session, *req).await,
            Step::OpenGate {
                title,
                explanation,
                payload,
            } => {
                self.append(
                    session,
                    SessionEvent::GateOpened {
                        id: GateId::new(),
                        title,
                        explanation,
                        payload,
                    },
                )?;
                Ok(())
            }
            Step::YoloAnswer { gate } => self.perform_yolo(session, gate).await,
            Step::WriteBook { book } => self.perform_write_book(session, book),
            Step::Pipeline(p) => Err(EngineError::Invalid(format!(
                "The pipeline did not perform its step `{}`.",
                p.summary
            ))),
            Step::Complete { report_markdown } => {
                let st = self.snapshot(session)?;
                let path = st.session_root.join(paths::report::completion());
                let md = report_markdown.unwrap_or_else(|| "# Session complete\n".into());
                std::fs::write(&path, &md).map_err(|e| EngineError::Invalid(e.to_string()))?;
                if self
                    .pipeline()
                    .completing(&StepHost(self.clone()), session, &st)?
                {
                    return Ok(());
                }
                let summary = md
                    .lines()
                    .map(|l| l.trim_start_matches('#').trim())
                    .find(|l| !l.is_empty())
                    .unwrap_or("Session complete")
                    .to_string();
                self.append(
                    session,
                    SessionEvent::SessionCompleted {
                        report_path: path,
                        summary,
                    },
                )?;
                Ok(())
            }
            Step::Fail { error } => {
                self.append(session, SessionEvent::SessionFailed { error })?;
                Ok(())
            }
            Step::HandleResult { execution } => {
                let st = self.snapshot(session)?;
                let Some((plugin, view)) = st.result_view(&execution) else {
                    return Ok(());
                };
                if st
                    .executions
                    .get(&execution)
                    .is_some_and(|r| r.handled.is_some())
                {
                    return Ok(());
                }
                drop(st);
                let checkpoints = self.checkpoints(session, &plugin);
                let outcome = self
                    .services
                    .handle_result(&plugin, view, checkpoints)
                    .await
                    .unwrap_or_else(|e| ostra_core::submit::CustomSubmit {
                        verdict: ostra_core::submit::StageVerdict::Fail,
                        summary: format!("Plugin `{plugin}` could not handle the result: {e}"),
                        findings: vec![],
                        question: None,
                        options: vec![],
                        report_path: None,
                        data: None,
                    });
                self.append(session, SessionEvent::ResultHandled { execution, outcome })?;
                Ok(())
            }
            Step::DecideStage { node, scope, seq } => {
                let st = self.snapshot(session)?;
                let current = st
                    .stage_track(&node, scope.as_deref())
                    .map(|t| t.decisions.len() as u32)
                    .unwrap_or(0);
                if current != seq {
                    return Ok(());
                }
                let Some((plugin, stage, view)) =
                    crate::plugin_stage::stage_view(&st, &node, scope.as_deref())
                else {
                    return Ok(());
                };
                drop(st);
                let checkpoints = self.checkpoints(session, &plugin);
                let decision = self
                    .services
                    .decide_stage(&plugin, &stage, view, checkpoints)
                    .await
                    .unwrap_or_else(|e| ostra_core::plugin::StageDecision::Fail {
                        summary: format!("Plugin `{plugin}` could not decide: {e}"),
                    });
                self.append(
                    session,
                    SessionEvent::StageDecided {
                        node,
                        scope,
                        decision,
                    },
                )?;
                Ok(())
            }
            Step::SkipStage { node, scope } => {
                self.append(session, SessionEvent::StageSkipped { node, scope })?;
                Ok(())
            }
            Step::RunNode {
                node, scope, round, ..
            } => {
                let st = self.snapshot(session)?;
                let Some(d) = st.workflow.as_ref().and_then(|w| w.stage(&node)).cloned() else {
                    return Ok(());
                };
                let track = st.stage_track(&node, scope.as_deref()).cloned();
                if track.as_ref().is_some_and(|t| t.round >= round) {
                    return Ok(());
                }
                let inputs = st.node_inputs(&d, scope.as_deref());
                let notes = track.map(|t| t.notes).unwrap_or_default();
                let functions = st
                    .workflow
                    .as_ref()
                    .map(|w| w.functions.clone())
                    .unwrap_or_default();
                drop(st);
                let (output, error, cost_usd) = match &d.run {
                    // Rule PL7: `<plugin>:<name>` runs in its plugin's code.
                    StageRun::Transform { function } if function.contains(':') => {
                        let (plugin, name) = function.split_once(':').unwrap_or_default();
                        match self
                            .services
                            .plugin_transform(plugin, name, inputs, d.args.clone())
                            .await
                        {
                            Ok(v) => (Some(v), None, 0.0),
                            Err(e) => (None, Some(format!("Plugin `{plugin}`: {e}")), 0.0),
                        }
                    }
                    StageRun::Transform { function } => {
                        match ostra_core::transform::run_function(
                            function, &inputs, &d.args, &functions,
                        ) {
                            Ok(v) => (Some(v), None, 0.0),
                            Err(e) => (None, Some(e), 0.0),
                        }
                    }
                    StageRun::Prompt { tier, effort } => {
                        self.prompt_node(&d, *tier, *effort, &inputs, &notes).await
                    }
                    _ => return Ok(()),
                };
                self.append(
                    session,
                    SessionEvent::NodeRan {
                        node,
                        scope,
                        round,
                        output,
                        error,
                        cost_usd,
                    },
                )?;
                Ok(())
            }
            Step::ResolveWorkflow { name, category } => {
                self.services.prepare_plugins().await;
                let set = self.services.workflows();
                let resolved = match (&name, category) {
                    (Some(n), _) => set.resolve(n),
                    (None, Some(c)) => set.default_for(c),
                    (None, None) => return Ok(()),
                }
                .and_then(|wf| {
                    crate::workflow::check_runnable(
                        &wf,
                        &self.services.agents(),
                        &self.services.plugin_stages(),
                    )
                    .map(|_| wf)
                });
                let event = match resolved {
                    Ok(workflow) => SessionEvent::WorkflowResolved { workflow },
                    Err(error) => SessionEvent::SessionFailed {
                        error: format!("The session's workflow cannot run. {error}"),
                    },
                };
                self.append(session, event)?;
                Ok(())
            }
        }
    }

    pub(crate) fn project_facts(&self) -> Vec<ProjectFacts> {
        self.services
            .workspace()
            .projects
            .iter()
            .map(|p| {
                let profile: Option<ProjectProfile> =
                    load_toml(&paths::project_profile(&p.path)).ok();
                ProjectFacts {
                    key: p.key.clone(),
                    path: p.path.display().to_string(),
                    initialized: paths::project_inventory(&p.path).exists(),
                    stack: p
                        .stack
                        .clone()
                        .or_else(|| profile.as_ref().and_then(|x| x.stack.clone())),
                    areas: profile
                        .map(|x| {
                            x.module_map
                                .into_iter()
                                .map(|r| format!("{} ({})", r.area, r.glob))
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Rule B5: merge the session's parts into its book and record the write. A failed write is
    /// recorded with its error, and the session goes on.
    pub(crate) fn perform_write_book(
        &self,
        session: &SessionId,
        book: String,
    ) -> Result<(), EngineError> {
        let st = self.snapshot(session)?;
        let update = self.pipeline().book_update(&st);
        let projects = update.parts.iter().map(|p| p.project.clone()).collect();
        let ws = &st.workspace_root;
        let error = ostra_core::book::apply(ws, &book, &update, chrono::Utc::now())
            .err()
            .map(|e| {
                format!(
                    "The book could not be written to {}: {e}",
                    ostra_core::book::book_dir(ws, &book).display()
                )
            });
        self.append(
            session,
            SessionEvent::BookWritten {
                book,
                projects,
                error,
            },
        )?;
        Ok(())
    }

    pub(crate) fn record_denied(
        &self,
        session: &SessionId,
        req: &SpawnRequest,
        executor: ExecutorKind,
        model: String,
        error: String,
    ) -> Result<(), EngineError> {
        let id = ExecutionId::new();
        self.append(
            session,
            SessionEvent::ExecutionStarted {
                projects: req.recorded_projects(),
                id: id.clone(),
                agent: req.agent,
                purpose: req.purpose.clone(),
                stage: req.stage,
                project: req.project.clone(),
                executor,
                model,
                params: Value::Null,
                spawn_block: String::new(),
                report_path: None,
                resumes: None,
                contract: None,
            },
        )?;
        let mut result = ExecutionResult::with_status(ExecutionStatus::Denied);
        result.error = Some(error);
        self.append(session, SessionEvent::ExecutionFinished { id, result })?;
        Ok(())
    }

    /// Wait for a slot under the workspace's parallelism limit, re-read each time so a settings
    /// change applies to waiting spawns.
    /// Rule SM2: hand run `execution` the messages queued for it, and record the hand-over.
    pub(crate) fn take_messages(
        &self,
        session: &SessionId,
        execution: &ExecutionId,
    ) -> Option<Wake> {
        let d = self.snapshot(session).ok()?.next_delivery(execution)?;
        self.append(
            session,
            SessionEvent::MessagesDelivered {
                to: execution.clone(),
                ids: d.ids.clone(),
                notice: d.notice.clone(),
            },
        )
        .ok()?;
        Some(Wake {
            note: d.note,
            owes_reply: !d.owes.is_empty(),
        })
    }

    pub(crate) async fn acquire_slot(self: &Arc<Self>) -> Slot {
        loop {
            let limit = self
                .services
                .workspace()
                .limits
                .max_parallel_executions
                .max(1) as usize;
            {
                let mut n = lock(&self.slots);
                if *n < limit {
                    *n += 1;
                    return Slot(self.clone());
                }
            }
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(2), self.slot_free.notified())
                    .await;
        }
    }
}

#[async_trait::async_trait]
impl ostra_core::plugin::Checkpoints for SessionCheckpoints {
    fn all(&self) -> std::collections::BTreeMap<String, Value> {
        self.inner
            .snapshot(&self.session)
            .ok()
            .and_then(|st| st.plugin_checkpoints.get(&self.plugin).cloned())
            .unwrap_or_default()
    }

    async fn write(&self, key: &str, value: Option<Value>) -> Result<(), String> {
        let st = self
            .inner
            .snapshot(&self.session)
            .map_err(|e| e.to_string())?;
        if st.is_terminal() {
            return Err("The session ended, so it keeps no more checkpoints.".into());
        }
        let kept = st
            .plugin_checkpoints
            .get(&self.plugin)
            .cloned()
            .unwrap_or_default();
        let saves = st.checkpoint_saves.get(&self.plugin).copied().unwrap_or(0);
        drop(st);
        if !ostra_core::plugin::check_checkpoint(&kept, saves, key, value.as_ref())? {
            return Ok(());
        }
        self.inner
            .append(
                &self.session,
                SessionEvent::PluginCheckpoint {
                    plugin: self.plugin.clone(),
                    key: key.to_string(),
                    value,
                },
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}
