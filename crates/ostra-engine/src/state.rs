//! A session's state is the fold of its events (HANDOVER 8.1). Everything here is deterministic:
//! the same events always produce the same state, so a restart replays and continues.

use chrono::{DateTime, Utc};
use ostra_core::agent::AgentName;
use ostra_core::containment::{ContainmentSignal, PAUSE_AFTER};
use ostra_core::event::{
    AnswerSource, ContextDelivery, ContextFile, ExecPurpose, GateAnswer, GatePayload, JudgeKind,
    ProjectRef, SessionEvent, SessionKind, SessionOptions, StoredEvent, UploadedFile,
};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId};
use ostra_core::paths;
use ostra_core::pipeline::{Category, StageKind};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Rule B5: the book write that ends the docs stage.
#[derive(Debug, Clone, PartialEq)]
pub struct BookWrite {
    pub book: String,
    pub projects: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExecRecord {
    pub id: ExecutionId,
    pub agent: AgentName,
    pub purpose: ExecPurpose,
    pub stage: StageKind,
    pub project: String,
    pub report_path: Option<PathBuf>,
    pub params: Value,
    pub result: Option<ExecutionResult>,
    pub resumes: Option<ExecutionId>,
    pub loop_key: Option<(u32, bool)>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub executor: ostra_core::ExecutorKind,
    pub model: String,
    pub spawn_block: String,
    /// Rule CA5: the result contract it submits.
    pub contract: ostra_core::Contract,
    /// Rule PL5: what its plugin's handler made of a plugin contract's result.
    pub handled: Option<ostra_core::submit::CustomSubmit>,
}

#[derive(Debug, Clone)]
pub struct GateRecord {
    pub id: GateId,
    pub title: String,
    pub explanation: String,
    pub payload: GatePayload,
    pub answer: Option<GateAnswer>,
    pub source: Option<AnswerSource>,
    pub reason: Option<String>,
    pub opened_at: DateTime<Utc>,
    pub answered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct DecisionRecord {
    pub id: DecisionId,
    pub judge: JudgeKind,
    pub subject: Option<String>,
    pub input_summary: String,
    pub output: Value,
    pub reason: String,
    pub overridden: bool,
    pub at: DateTime<Utc>,
}

/// Context the user added after the start (Rules D2, C2).
#[derive(Debug, Clone)]
pub struct Amendment {
    pub text: String,
    pub files: Vec<ContextFile>,
    pub uploads: Vec<UploadedFile>,
    pub delivery: ContextDelivery,
    pub at: DateTime<Utc>,
    /// Rule C2: the Route answer judge decides what happens to it once it is released.
    pub routed: bool,
    /// Rule C2: queued behind running executions; nothing starts, and the user may withdraw it.
    pub held: bool,
    /// The user withdrew it while it was queued, so no agent or judge reads it.
    pub withdrawn: bool,
    /// Rule C2: the Route answer judge has not decided yet, so nothing starts.
    pub pending: bool,
    /// The context joins the request every later agent reads. Remembered or discarded context does not.
    pub delivered: bool,
}

/// Rule U2: the correction an execution resumes with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Steer {
    pub text: String,
    /// Sent to a paused run and not read yet, so the user may withdraw it. A correction sent to a
    /// running run stopped it and is final.
    pub queued: bool,
}

/// Why the engine interrupted a running execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    /// The session was paused; the execution resumes where it stopped (Rule P2).
    Pause,
    /// Context was sent now; the execution re-runs from its spawn block (Rule C2).
    Context,
    /// Rule O4: the run created its phase's project, which is initialized before the phase starts
    /// again inside it.
    ProjectCreated,
    /// Rule U1: the user skipped the execution's task; it ends without a result.
    Skipped,
    /// Rule U2: the user sent the execution a correction; it resumes in place with it.
    Steer,
}

impl Interrupt {
    pub fn message(self) -> &'static str {
        match self {
            Interrupt::Pause => "The session was paused.",
            Interrupt::Context => "Interrupted to deliver the context the user added.",
            Interrupt::ProjectCreated => {
                "Stopped after creating the project, which Ostra initializes before this phase starts again in it."
            }
            Interrupt::Skipped => {
                "You skipped this task, so Ostra stopped it and moved on without it."
            }
            Interrupt::Steer => "Interrupted to deliver the correction the user sent.",
        }
    }
}

/// Rule C3: each upload reaches the agents as the absolute path of its copy in the session.
pub fn upload_list(heading: &str, uploads: &[UploadedFile]) -> String {
    if uploads.is_empty() {
        return String::new();
    }
    let mut s = format!(
        "\n\n{heading}. Read each one before you start, because the user chose them as context:"
    );
    for u in uploads {
        s.push_str(&format!("\n- `{}`", u.path.display()));
    }
    s
}

/// The resume-map key of an execution purpose: one work loop, review loop, or stage.
pub fn purpose_key(p: &ExecPurpose) -> String {
    p.resume_key()
}

#[derive(Debug, Clone)]
pub struct SessionState {
    /// The pipeline that folds and plans the built-in stages.
    pub pipeline: crate::pipeline::PipelineRef,
    /// The pipeline's own state, which only the pipeline reads.
    pub ext: crate::pipeline::PipelineBox,
    pub id: SessionId,
    pub created: bool,
    pub kind: SessionKind,
    pub request: String,
    /// Files attached to the request at the start (Rule C1).
    pub files: Vec<ContextFile>,
    /// Files uploaded with the request at the start (Rule C3).
    pub uploads: Vec<UploadedFile>,
    pub amendments: Vec<Amendment>,
    pub options: SessionOptions,
    pub yolo: bool,
    pub projects: Vec<ProjectRef>,
    /// Rule O6: the projects the user pinned on the New task form.
    pub pinned: Vec<String>,
    pub workspace_root: PathBuf,
    pub session_root: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    pub classify: Option<DecisionId>,
    pub category: Option<Category>,
    pub scope: Vec<String>,

    /// Rule B6: the book the user picked on the New task form.
    pub docs_book: Option<String>,
    /// Rule B5: set once the engine wrote the session's documentation.
    pub book_written: Option<BookWrite>,
    /// Rule O3: projects agents created in this session, in creation order.
    pub created_projects: Vec<ostra_core::manage::CreatedProject>,
    /// Rule O4: runs stopped because they created their phase's project, whose phase starts over.
    pub restart_fresh: BTreeSet<ExecutionId>,
    /// Agents re-routed to native after a harness failure.
    pub native_fallback: BTreeSet<AgentName>,
    /// Dollars the user added to the session budget.
    pub budget_raised: f64,
    pub budget_gate: Option<GateId>,

    pub executions: BTreeMap<ExecutionId, ExecRecord>,
    pub gates: BTreeMap<GateId, GateRecord>,
    pub decisions: BTreeMap<DecisionId, DecisionRecord>,
    pub completion_decision: Option<DecisionId>,
    pub completed: Option<(PathBuf, String)>,
    pub failed: Option<String>,
    pub notes: Vec<String>,
    /// Rule P1: a paused session starts nothing.
    pub paused: bool,
    /// Rule P3: containment signals per execution.
    pub signals: BTreeMap<ExecutionId, Vec<ContainmentSignal>>,
    /// Rule PL8: each plugin's checkpoints in this session, by key.
    pub plugin_checkpoints: BTreeMap<String, BTreeMap<String, Value>>,
    /// Rule PL8: how often each plugin saved a checkpoint in this session.
    pub checkpoint_saves: BTreeMap<String, u32>,
    /// Rule P3: the execution whose signals paused the session, until it is continued.
    pub contained: Option<ExecutionId>,
    /// Executions the engine is interrupting, and why.
    pub interrupting: BTreeMap<ExecutionId, Interrupt>,
    /// Paused executions to resume, by [`purpose_key`] (Rule P2).
    pub resume_from: BTreeMap<String, ExecutionId>,
    /// Rule U2: corrections the user sent each execution, kept until it resumes with them.
    pub steers: BTreeMap<ExecutionId, Steer>,
    /// Rule H2: native runs that ended waiting for a message, by [`purpose_key`].
    pub waiting_keys: BTreeMap<String, ExecutionId>,
    /// Rule SM8: messages between subagents, in the order they were sent.
    pub messages: Vec<crate::coord::Message>,
    /// Rule SM3: runs that paused themselves and what each waits for.
    pub waits: BTreeMap<ExecutionId, crate::coord::WaitOn>,
    /// Rule WF1: the workflow the session asked for; `None` for logs written before workflows.
    pub workflow_choice: Option<ostra_core::workflow::WorkflowChoice>,
    /// Rule WF1: the workflow the session runs, once resolved.
    pub workflow: Option<ostra_core::workflow::WorkflowDef>,
    /// Rule WF4: custom stage instances by [`crate::workflow::stage_key`].
    pub stages: BTreeMap<String, crate::workflow::StageTrack>,
    /// Rule H1: each execution's subagent ID, for runs that continue another's conversation.
    pub subagents: BTreeMap<ExecutionId, ExecutionId>,
    pub last_seq: i64,
    /// The session's short label: from the Classify decision, or fixed for an init session.
    pub title: Option<String>,
    /// Rule WB3: what prompt nodes spent on their model calls.
    pub node_cost: f64,
}

pub fn parse<T: serde::de::DeserializeOwned>(v: &Option<Value>) -> Option<T> {
    v.as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub fn stage_of(purpose: &ExecPurpose) -> StageKind {
    purpose.stage_kind()
}

impl SessionState {
    pub fn new(pipeline: crate::pipeline::PipelineRef, id: SessionId) -> Self {
        let epoch = DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default();
        SessionState {
            ext: pipeline.new_state(),
            pipeline,
            id,
            created: false,
            kind: SessionKind::Pipeline,
            request: String::new(),
            files: vec![],
            uploads: vec![],
            amendments: vec![],
            options: SessionOptions::default(),
            yolo: false,
            projects: vec![],
            pinned: vec![],
            workspace_root: PathBuf::new(),
            session_root: PathBuf::new(),
            created_at: epoch,
            updated_at: epoch,
            classify: None,
            category: None,
            scope: vec![],
            docs_book: None,
            book_written: None,
            created_projects: vec![],
            restart_fresh: BTreeSet::new(),
            native_fallback: BTreeSet::new(),
            budget_raised: 0.0,
            budget_gate: None,
            executions: BTreeMap::new(),
            gates: BTreeMap::new(),
            decisions: BTreeMap::new(),
            completion_decision: None,
            completed: None,
            failed: None,
            notes: vec![],
            paused: false,
            signals: BTreeMap::new(),
            plugin_checkpoints: BTreeMap::new(),
            checkpoint_saves: BTreeMap::new(),
            contained: None,
            interrupting: BTreeMap::new(),
            resume_from: BTreeMap::new(),
            steers: BTreeMap::new(),
            waiting_keys: BTreeMap::new(),
            messages: vec![],
            waits: BTreeMap::new(),
            workflow_choice: None,
            workflow: None,
            stages: BTreeMap::new(),
            subagents: BTreeMap::new(),
            last_seq: 0,
            title: None,
            node_cost: 0.0,
        }
    }

    pub fn fold(
        pipeline: crate::pipeline::PipelineRef,
        id: SessionId,
        events: &[StoredEvent],
    ) -> Self {
        let mut s = SessionState::new(pipeline, id);
        for e in events {
            s.apply(e);
        }
        s
    }

    pub fn is_terminal(&self) -> bool {
        self.completed.is_some() || self.failed.is_some()
    }

    /// The request as it now stands, with every amendment (Rule D2), attached file (Rule C1), and
    /// upload (Rule C3).
    pub fn full_request(&self) -> String {
        let mut s = self.request.clone();
        s.push_str(&self.file_list(
            "Files and folders the user attached to the request",
            &self.files,
        ));
        s.push_str(&upload_list(
            "Files the user uploaded with the request",
            &self.uploads,
        ));
        for a in self.amendments.iter().filter(|a| a.delivered) {
            s.push_str("\n\nAdded later by the user: ");
            s.push_str(&self.added_part(&a.text, &a.files, &a.uploads));
        }
        s
    }

    /// Rule C2: the context the user added, for a spawn whose task was written without it.
    pub fn added_context(&self) -> String {
        let added: Vec<String> = self
            .amendments
            .iter()
            .filter(|a| a.delivered)
            .map(|a| format!("- {}", self.added_part(&a.text, &a.files, &a.uploads)))
            .collect();
        if added.is_empty() {
            return String::new();
        }
        format!(
            "\n\nThe user added this context to the request while the session ran. Take it into account:\n{}",
            added.join("\n")
        )
    }

    /// Rule C1: each attached file reaches the agents as an absolute path beside its tag.
    pub fn file_list(&self, heading: &str, files: &[ContextFile]) -> String {
        if files.is_empty() {
            return String::new();
        }
        let mut s = format!(
            "\n\n{heading}. Read each file and look through each folder before you start, because the user chose them as context:"
        );
        for f in files {
            let root = if f.project == ostra_core::artifacts::TAG_ROOT {
                Some(ostra_core::artifacts::dir(&self.workspace_root))
            } else {
                self.project_path(&f.project)
            };
            // Forward slashes so the path reads consistently in the prompt on every OS, and never
            // mixes separators when a forward-slash relative path is joined to a Windows root.
            let abs = root
                .map(|p| p.join(&f.path).display().to_string().replace('\\', "/"))
                .unwrap_or_else(|| f.path.clone());
            let kind = if f.is_folder() { "folder, " } else { "" };
            s.push_str(&format!("\n- `{abs}` ({kind}{})", f.tag()));
        }
        s
    }

    pub fn added_part(
        &self,
        text: &str,
        files: &[ContextFile],
        uploads: &[UploadedFile],
    ) -> String {
        format!(
            "{text}{}{}",
            self.file_list("Files and folders attached with this addition", files),
            upload_list("Files uploaded with this addition", uploads)
        )
    }

    pub fn project_path(&self, key: &str) -> Option<PathBuf> {
        self.projects
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.path.clone())
    }

    /// The primary project: the first in scope. Cross-project stages carry its key.
    pub fn primary(&self) -> String {
        self.scope
            .first()
            .cloned()
            .or_else(|| self.projects.first().map(|p| p.key.clone()))
            .unwrap_or_default()
    }

    pub fn project_session_dir(&self, key: &str) -> PathBuf {
        self.session_root.join(key)
    }

    pub fn open_gates(&self) -> impl Iterator<Item = &GateRecord> {
        self.gates.values().filter(|g| g.answer.is_none())
    }

    pub fn running_executions(&self) -> impl Iterator<Item = &ExecRecord> {
        self.executions.values().filter(|e| e.result.is_none())
    }

    /// What finished executions and prompt nodes spent. Running executions are counted when they
    /// finish.
    pub fn spent_usd(&self) -> f64 {
        self.executions
            .values()
            .filter_map(|e| e.result.as_ref())
            .map(|r| r.usage.cost_usd)
            .sum::<f64>()
            + self.node_cost
    }

    // -----------------------------------------------------------------------------------------
    // Fold
    // -----------------------------------------------------------------------------------------

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

    pub fn valid_project(&self, key: &str) -> bool {
        self.projects.iter().any(|p| p.key == key)
    }

    /// Rule C2: a classified session with research or later stages routes added context through
    /// the judge. Before classification the Classify judge reads it with the request.
    pub fn routes_amendments(&self) -> bool {
        self.pipeline.routes_amendments(self)
    }

    /// Rule C2: queued context joins the session, to be routed or delivered. A resumed conversation
    /// would not see it, so each paused run re-runs instead (Rule P2), except a run that holds a
    /// correction, which resumes with it (Rule U2).
    pub fn release_amendment(&mut self, i: usize) {
        let a = &mut self.amendments[i];
        a.held = false;
        a.pending = a.routed;
        a.delivered = !a.routed;
        let routed = a.routed;
        let a = &self.amendments[i];
        let part = self.added_part(&a.text, &a.files, &a.uploads);
        let steers = &self.steers;
        self.resume_from.retain(|_, id| steers.contains_key(id));
        if !routed {
            self.pipeline.clone().amended(self, &part);
        }
    }

    /// Rule C2: queued context the user may still withdraw, by index.
    pub fn held_amendments(&self) -> impl Iterator<Item = usize> + '_ {
        self.amendments
            .iter()
            .enumerate()
            .filter(|(_, a)| a.held)
            .map(|(i, _)| i)
    }

    /// Rule C2: context the user added that waits for the judge, by index.
    pub fn pending_amendments(&self) -> impl Iterator<Item = usize> + '_ {
        self.amendments
            .iter()
            .enumerate()
            .filter(|(_, a)| a.pending)
            .map(|(i, _)| i)
    }

    /// Rule U2: a running execution, or one a pause stopped, can take a correction. A run waiting
    /// on another subagent wakes only with that answer (Rule H2), so it takes none.
    pub fn can_steer(&self, exec: &ExecutionId) -> bool {
        let Some(rec) = self.executions.get(exec) else {
            return false;
        };
        if self.is_terminal()
            || self
                .interrupting
                .get(exec)
                .is_some_and(|w| *w != Interrupt::Steer)
        {
            return false;
        }
        match &rec.result {
            None => true,
            Some(_) => self.resume_from.values().any(|x| x == exec),
        }
    }

    /// Rule U2: a correction waits, withdrawable, while its run is paused. Sent to a running run, it
    /// stops the run at once and cannot be taken back.
    pub fn steer_queued(&self, exec: &ExecutionId) -> bool {
        self.steers.get(exec).is_some_and(|s| s.queued)
            && self.resume_from.values().any(|x| x == exec)
    }

    /// Rule P4: the user stopped this execution, so nothing retries it without them, YOLO included.
    pub fn stopped_by_user(&self, exec: &ExecutionId) -> bool {
        // A pause or an interrupt turns a cancel into Interrupted, and a stopped session ends,
        // so a Cancelled run in a live session is one the user stopped.
        self.executions
            .get(exec)
            .and_then(|r| r.result.as_ref())
            .is_some_and(|r| r.status == ExecutionStatus::Cancelled)
    }

    pub fn interrupt_running(&mut self, why: Interrupt) {
        let running: Vec<ExecutionId> = self.running_executions().map(|r| r.id.clone()).collect();
        for id in running {
            self.interrupting.entry(id).or_insert(why);
        }
    }

    pub fn session_context_path(&self) -> PathBuf {
        self.session_root.join(paths::report::session_context())
    }

    // -----------------------------------------------------------------------------------------
    // Init flow
    // -----------------------------------------------------------------------------------------

    pub fn exec_project(&self, id: &ExecutionId) -> String {
        self.executions
            .get(id)
            .map(|r| r.project.clone())
            .unwrap_or_default()
    }
}

pub fn exec_error(result: &ExecutionResult) -> String {
    match (&result.error, result.status) {
        (Some(e), _) => e.clone(),
        (None, ExecutionStatus::Cancelled) => STOPPED_EXECUTION.into(),
        (None, status) => format!("execution ended with status {status:?}"),
    }
}

pub const STOPPED_EXECUTION: &str = "You stopped this execution.";

/// The Route answer judge's subject for the amendment at `i` (Rule C2).
pub fn amendment_subject(i: usize) -> String {
    format!("amendment:{i}")
}

pub fn amendment_index(subject: &str) -> Option<usize> {
    subject.strip_prefix("amendment:")?.parse().ok()
}

pub fn loop_key_str(key: (u32, bool)) -> String {
    if key.1 {
        format!("phase:{}:tests", key.0)
    } else {
        format!("phase:{}", key.0)
    }
}

pub fn parse_loop_key(s: &str) -> Option<(u32, bool)> {
    let rest = s.strip_prefix("phase:")?;
    match rest.strip_suffix(":tests") {
        Some(n) => Some((n.parse().ok()?, true)),
        None => Some((rest.parse().ok()?, false)),
    }
}
