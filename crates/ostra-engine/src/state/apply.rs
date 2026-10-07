//! The fold: how one stored event changes a session's state.

use super::*;
use ostra_core::containment::PAUSE_AFTER;
use ostra_core::event::{
    ContextDelivery, GatePayload, ProjectRef, SessionEvent, SessionKind, StoredEvent,
};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};

impl SessionState {
    pub fn apply(&mut self, stored: &StoredEvent) {
        self.last_seq = stored.seq;
        self.updated_at = stored.at;
        let at = stored.at;
        match &stored.event {
            SessionEvent::SessionCreated {
                kind,
                request,
                options,
                projects,
                workspace_root,
                session_root,
                files,
                uploads,
                pinned,
                docs_book,
                workflow,
            } => {
                self.created = true;
                self.workflow_choice = workflow.clone();
                self.pinned = pinned.clone();
                self.docs_book = docs_book.clone();
                self.files = files.clone();
                self.uploads = uploads.clone();
                self.created_at = at;
                self.kind = kind.clone();
                self.request = request.clone();
                self.options = *options;
                self.yolo = options.yolo;
                self.projects = projects.clone();
                self.workspace_root = workspace_root.clone();
                self.session_root = session_root.clone();
                if let SessionKind::Init { project } = kind {
                    self.scope = vec![project.clone()];
                    self.title = Some(format!("Initialize {project}"));
                }
                self.pipeline.clone().created(self);
            }
            SessionEvent::WorkflowResolved { workflow } => self.on_workflow_resolved(workflow),
            SessionEvent::ResultHandled { execution, outcome } => {
                self.on_result_handled(execution, outcome)
            }
            SessionEvent::PluginCheckpoint { plugin, key, value } => {
                *self.checkpoint_saves.entry(plugin.clone()).or_default() += 1;
                let kept = self.plugin_checkpoints.entry(plugin.clone()).or_default();
                match value {
                    Some(v) => {
                        kept.insert(key.clone(), v.clone());
                    }
                    None => {
                        kept.remove(key);
                    }
                }
            }
            SessionEvent::StageDecided {
                node,
                scope,
                decision,
            } => self.on_stage_decided(node, scope.as_deref(), decision),
            SessionEvent::StageSkipped { node, scope } => {
                self.on_stage_skipped(node, scope.as_deref())
            }
            SessionEvent::NodeRan {
                node,
                scope,
                round,
                output,
                error,
                cost_usd,
            } => self.on_node_ran(
                node,
                scope.as_deref(),
                *round,
                output.as_ref(),
                error.as_deref(),
                *cost_usd,
            ),
            SessionEvent::ProjectCreated { project } => {
                // Rule O3: the project joins the session's projects and scope uninitialized.
                if !self.valid_project(&project.key) {
                    self.projects.push(ProjectRef {
                        key: project.key.clone(),
                        path: project.path.clone(),
                    });
                }
                if !self.scope.contains(&project.key) {
                    self.scope.push(project.key.clone());
                }
                self.pipeline.clone().project_created(self, project);
                self.created_projects.push(project.clone());
                // Rule O4: the run that created it stops, and its phase starts again in the new
                // project once the init ends.
                if self
                    .executions
                    .get(&project.execution)
                    .is_some_and(|r| r.result.is_none())
                {
                    self.interrupting
                        .insert(project.execution.clone(), Interrupt::ProjectCreated);
                }
            }
            SessionEvent::RequestAmended {
                text,
                files,
                uploads,
                delivery,
                routed,
            } => {
                let now = *delivery == ContextDelivery::Now;
                if now {
                    // Rule C2: a fresh re-run replaces a correction's resume, which would not see the context.
                    self.interrupting.retain(|_, why| *why != Interrupt::Steer);
                    self.interrupt_running(Interrupt::Context);
                    self.steers.clear();
                }
                // Rule C2: queued context waits for the running executions to finish.
                let held = !now && self.running_executions().next().is_some();
                self.amendments.push(Amendment {
                    text: text.clone(),
                    files: files.clone(),
                    uploads: uploads.clone(),
                    delivery: *delivery,
                    at,
                    routed: *routed,
                    held,
                    withdrawn: false,
                    pending: false,
                    delivered: false,
                });
                if !held {
                    self.release_amendment(self.amendments.len() - 1);
                }
            }
            SessionEvent::AmendmentWithdrawn { index } => {
                if let Some(a) = self.amendments.get_mut(*index as usize).filter(|a| a.held) {
                    a.held = false;
                    a.withdrawn = true;
                }
            }
            SessionEvent::SessionPaused => {
                self.paused = true;
                self.interrupt_running(Interrupt::Pause);
            }
            SessionEvent::SessionResumed => {
                self.paused = false;
                self.contained = None;
            }
            SessionEvent::ContainmentSignal { execution, signal } => {
                let seen = self.signals.entry(execution.clone()).or_default();
                seen.push(signal.clone());
                // Rule P3: the third signal of one execution pauses the session as Rule P1 does.
                if seen.len() == PAUSE_AFTER && !self.paused && !self.is_terminal() {
                    self.paused = true;
                    self.contained = Some(execution.clone());
                    self.interrupt_running(Interrupt::Pause);
                }
            }
            SessionEvent::YoloSet { enabled } => self.yolo = *enabled,
            SessionEvent::DecisionMade {
                id,
                judge,
                subject,
                input_summary,
                output,
                reason,
            } => {
                self.decisions.insert(
                    id.clone(),
                    DecisionRecord {
                        id: id.clone(),
                        judge: *judge,
                        subject: subject.clone(),
                        input_summary: input_summary.clone(),
                        output: output.clone(),
                        reason: reason.clone(),
                        overridden: false,
                        at,
                    },
                );
                self.pipeline
                    .clone()
                    .decision(self, id, *judge, subject.as_deref(), output, false);
            }
            SessionEvent::DecisionOverridden { id, output, reason } => {
                let Some(d) = self.decisions.get_mut(id) else {
                    return;
                };
                d.output = output.clone();
                d.reason = reason.clone();
                d.overridden = true;
                let (judge, subject) = (d.judge, d.subject.clone());
                self.pipeline
                    .clone()
                    .decision(self, id, judge, subject.as_deref(), output, true);
            }
            SessionEvent::ExecutionStarted {
                id,
                agent,
                purpose,
                stage,
                project,
                executor,
                model,
                params,
                spawn_block,
                report_path,
                resumes,
                contract,
                projects,
            } => {
                // Rule CA5: logs from before contracts ran the standard agents.
                let contract = contract.unwrap_or_else(|| self.pipeline.legacy_contract(*agent));
                let loop_key = self.pipeline.clone().loop_key(self, purpose);
                self.executions.insert(
                    id.clone(),
                    ExecRecord {
                        id: id.clone(),
                        agent: *agent,
                        purpose: purpose.clone(),
                        stage: *stage,
                        project: project.clone(),
                        // Rule WD1: a log from before work dirs ran each run in one project.
                        projects: if projects.is_empty() {
                            vec![project.clone()]
                        } else {
                            projects.clone()
                        },
                        report_path: report_path.clone(),
                        params: params.clone(),
                        result: None,
                        resumes: resumes.clone(),
                        loop_key,
                        started_at: at,
                        ended_at: None,
                        executor: *executor,
                        model: model.clone(),
                        spawn_block: spawn_block.clone(),
                        contract,
                        handled: None,
                    },
                );
                self.resume_from.remove(&purpose_key(purpose));
                if let Some(from) = resumes {
                    let root = self.subagent_of(from);
                    self.subagents.insert(id.clone(), root);
                }
                if self.paused {
                    // Started in the window before the pause reached the runner.
                    self.interrupting.insert(id.clone(), Interrupt::Pause);
                }
                self.pipeline
                    .clone()
                    .started(self, id, purpose, loop_key, false);
                self.coord_started(id, purpose);
                self.stage_started(id, purpose);
            }
            SessionEvent::MessageSent { .. }
            | SessionEvent::AgentWaiting { .. }
            | SessionEvent::MessagesDelivered { .. }
            | SessionEvent::AgentAsked { .. }
            | SessionEvent::AgentReplied { .. }
            | SessionEvent::MessageDelivered { .. } => self.on_coord_event(&stored.event, at),
            SessionEvent::ExecutionResumed { id } => {
                let Some(rec) = self.executions.get_mut(id) else {
                    return;
                };
                rec.result = None;
                rec.ended_at = None;
                let (purpose, loop_key) = (rec.purpose.clone(), rec.loop_key);
                self.resume_from.remove(&purpose_key(&purpose));
                self.steers.remove(id);
                self.waiting_keys.retain(|_, x| x != id);
                // Rule P3: continuing is the user's "this was fine", so the count starts again.
                self.signals.remove(id);
                if self.paused {
                    self.interrupting.insert(id.clone(), Interrupt::Pause);
                }
                self.pipeline
                    .clone()
                    .started(self, id, &purpose, loop_key, true);
            }
            SessionEvent::ExecutionSteered { id, text } => {
                if !self.can_steer(id) {
                    return;
                }
                let running = self.executions.get(id).is_some_and(|r| r.result.is_none());
                let steer = self.steers.entry(id.clone()).or_insert(Steer {
                    text: String::new(),
                    queued: true,
                });
                if !steer.text.is_empty() {
                    steer.text.push_str("\n\n");
                }
                steer.text.push_str(text);
                steer.queued &= !running;
                if running {
                    self.interrupting
                        .entry(id.clone())
                        .or_insert(Interrupt::Steer);
                }
            }
            SessionEvent::SteerWithdrawn { id } => {
                if self.steer_queued(id) {
                    self.steers.remove(id);
                }
            }
            SessionEvent::ExecutionSkipped { id } => {
                if self.pipeline.clone().can_skip(self, id) {
                    self.interrupting.insert(id.clone(), Interrupt::Skipped);
                    self.pipeline.clone().skip_task(self, id);
                }
            }
            SessionEvent::ExecutionFinished { id, result } => {
                let Some(rec) = self.executions.get_mut(id) else {
                    return;
                };
                rec.result = Some(result.clone());
                rec.ended_at = Some(at);
                let rec = rec.clone();
                let why = self.interrupting.remove(id);
                let resumes = matches!(why, Some(Interrupt::Pause | Interrupt::Steer))
                    && result.status == ExecutionStatus::Interrupted;
                if resumes {
                    self.resume_from
                        .insert(purpose_key(&rec.purpose), id.clone());
                } else {
                    // Rule U2: a run that ended before the correction stopped it never reads it.
                    self.steers.remove(id);
                }
                if why == Some(Interrupt::ProjectCreated) {
                    self.restart_fresh.insert(id.clone());
                }
                if result.status == ExecutionStatus::Waiting {
                    // Rule H2: the stage sees a paused run, and the planner holds its spawn until
                    // a message wakes this run in place.
                    self.waiting_keys
                        .insert(SessionState::waiting_key(&rec), id.clone());
                    let paused = ExecutionResult {
                        status: ExecutionStatus::Interrupted,
                        ..result.clone()
                    };
                    self.pipeline.clone().finished(self, &rec, &paused);
                } else {
                    self.pipeline.clone().finished(self, &rec, result);
                }
                // Rule U1: the stopped run's finish would put its task back to re-run.
                if why == Some(Interrupt::Skipped) {
                    self.pipeline.clone().skip_task(self, id);
                }
                self.coord_finished(&rec, result);
                self.stage_finished(&rec, result);
                if self.running_executions().next().is_none() {
                    let held: Vec<usize> = self.held_amendments().collect();
                    for i in held {
                        self.release_amendment(i);
                    }
                }
            }
            SessionEvent::GateOpened {
                id,
                title,
                explanation,
                payload,
            } => {
                self.gates.insert(
                    id.clone(),
                    GateRecord {
                        id: id.clone(),
                        title: title.clone(),
                        explanation: explanation.clone(),
                        payload: payload.clone(),
                        answer: None,
                        source: None,
                        reason: None,
                        opened_at: at,
                        answered_at: None,
                    },
                );
                // Pattern 8: the session budget is the engine's, whatever pipeline runs.
                if let GatePayload::BudgetReached { .. } = payload {
                    self.budget_gate = Some(id.clone());
                    return;
                }
                self.pipeline.clone().gate_opened(self, id, payload);
            }
            SessionEvent::GateAnswered {
                id,
                source,
                answer,
                reason,
                routed,
            } => {
                let Some(g) = self.gates.get_mut(id) else {
                    return;
                };
                if g.answer.is_some() {
                    return;
                }
                g.answer = Some(answer.clone());
                g.source = Some(*source);
                g.reason = reason.clone();
                g.answered_at = Some(at);
                let payload = g.payload.clone();
                if let GatePayload::BudgetReached {
                    spent_usd,
                    budget_usd,
                } = payload
                {
                    self.budget_answered(spent_usd, budget_usd, answer);
                    return;
                }
                self.pipeline
                    .clone()
                    .gate_answered(self, id, &payload, answer, *routed);
            }
            SessionEvent::BookWritten {
                book,
                projects,
                error,
            } => {
                self.book_written = Some(BookWrite {
                    book: book.clone(),
                    projects: projects.clone(),
                    error: error.clone(),
                });
            }
            SessionEvent::Note { message } => self.notes.push(message.clone()),
            SessionEvent::SessionCompleted {
                report_path,
                summary,
            } => {
                self.completed = Some((report_path.clone(), summary.clone()));
            }
            SessionEvent::SessionFailed { error } => self.failed = Some(error.clone()),
            // The events only the built-in stages read.
            _ => self.pipeline.clone().event(self, stored),
        }
    }
}
