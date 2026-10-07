//! The runner: one driver task per live session. It folds events, asks the planner for the next
//! steps, performs them, and appends what happened. Every state change is an event first.

use crate::pipeline::{PipelineRef, ProjectFacts, YoloPlan};
use crate::plan::{PlanCtx, SpawnRequest, Step, next_steps};
use crate::services::{BuiltSpawn, Notice, Services, SpawnEnv};
use crate::state::{Interrupt, SessionState, purpose_key};
use crate::uploads;
use ostra_core::api::{
    ActivityItem, CreateSession, ExecutionView, GateView, SessionDetail, SessionSummary,
    TreeSession, UploadRef,
};
use ostra_core::artifacts;
use ostra_core::config::{PermissionRules, ProjectProfile, RouteQuery, load_toml, resolve_route};
use ostra_core::event::{
    AnswerSource, CommandPurpose, ContextDelivery, ContextFile, ExecPurpose, GateAnswer,
    GatePayload, JudgeKind, ProjectRef, SessionEvent, SessionKind, SessionOptions, StoredEvent,
};
use ostra_core::exec::{
    CancellationToken, ExecContext, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec,
    ExecutionStatus, ResumeInfo, Usage, Wake,
};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
use ostra_core::model::Tier;
use ostra_core::paths;
use ostra_core::policy::{PermissionAnswer, RuleRef, ToolCall};
use ostra_core::workflow::StageRun;
use ostra_store::{NewExecution, NewSession, SessionUpdate, WorkspaceDb};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, broadcast, oneshot};

#[derive(Debug, Clone)]
pub enum EngineNotice {
    Event {
        session: SessionId,
        stored: StoredEvent,
    },
    Delta {
        execution: ExecutionId,
        item: ActivityItem,
    },
    ExecutionStatus {
        execution: ExecutionId,
        status: ExecutionStatus,
    },
    SessionUpdated {
        summary: SessionSummary,
    },
    /// A project's init status may have changed: an init session started or ended.
    ProjectsChanged,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    /// The session ended before an execution or command it planned could start.
    #[error("The session ended before this step started.")]
    Ended,
    #[error("store: {0}")]
    Store(#[from] ostra_store::StoreError),
}

/// What a new session carries besides its request and options.
#[derive(Default)]
struct Attached<'a> {
    files: &'a [ContextFile],
    uploads: &'a [String],
    docs_book: Option<String>,
    workflow: Option<ostra_core::workflow::WorkflowChoice>,
}

struct Live {
    state: Mutex<SessionState>,
    wake: Notify,
    inflight: Mutex<HashSet<String>>,
    driving: AtomicBool,
}

struct Inner {
    pipeline: PipelineRef,
    workspace_root: PathBuf,
    workspace_id: WorkspaceId,
    db: WorkspaceDb,
    services: Arc<dyn Services>,
    sessions: Mutex<HashMap<SessionId, Arc<Live>>>,
    tx: broadcast::Sender<EngineNotice>,
    execs: Mutex<HashMap<ExecutionId, (Option<SessionId>, CancellationToken)>>,
    permission_waiters: Mutex<HashMap<GateId, oneshot::Sender<PermissionAnswer>>>,
    judge_cost: Mutex<HashMap<SessionId, f64>>,
    resume_hints: Mutex<HashMap<String, ResumeInfo>>,
    /// Executions holding a slot under `limits.max_parallel_executions`.
    slots: Mutex<usize>,
    slot_free: Notify,
    /// Rule H2: messages for harness runs that wait with their process alive.
    /// Rings on every appended event, so a waiting harness run checks for its messages.
    bell: Notify,
}

/// A held execution slot; dropping it frees the slot.
struct Slot(Arc<Inner>);

impl Drop for Slot {
    fn drop(&mut self) {
        let mut n = lock(&self.0.slots);
        *n = n.saturating_sub(1);
        drop(n);
        self.0.slot_free.notify_waiters();
    }
}

/// The output of a format step skipped because its command waits for approval.
pub const FORMAT_NOT_APPROVED: &str = "The format command in project.toml changed outside Ostra, so format was skipped. Approve it in the project's settings to run it next time.";

#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

/// What a pipeline's step effects may use of the runner.
pub struct StepHost(Arc<Inner>);

impl StepHost {
    /// Append an event: store it, fold it, broadcast it, and wake the session's driver.
    pub fn append(
        &self,
        session: &SessionId,
        event: SessionEvent,
    ) -> Result<StoredEvent, EngineError> {
        self.0.append(session, event)
    }

    pub fn snapshot(&self, session: &SessionId) -> Result<SessionState, EngineError> {
        self.0.snapshot(session)
    }

    pub fn pipeline(&self) -> &PipelineRef {
        &self.0.pipeline
    }

    pub fn db(&self) -> &WorkspaceDb {
        &self.0.db
    }

    /// A project's init status changed.
    pub fn projects_changed(&self) {
        let _ = self.0.tx.send(EngineNotice::ProjectsChanged);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Engine {
    /// The pipeline that runs the built-in stages.
    pub fn pipeline(&self) -> &PipelineRef {
        &self.inner.pipeline
    }

    pub fn new(
        workspace_root: PathBuf,
        workspace_id: WorkspaceId,
        db: WorkspaceDb,
        services: Arc<dyn Services>,
    ) -> Self {
        let (tx, _) = broadcast::channel(4096);
        Engine {
            inner: Arc::new(Inner {
                pipeline: services.pipeline().into(),
                workspace_root,
                workspace_id,
                db,
                services,
                sessions: Mutex::new(HashMap::new()),
                tx,
                execs: Mutex::new(HashMap::new()),
                permission_waiters: Mutex::new(HashMap::new()),
                judge_cost: Mutex::new(HashMap::new()),
                resume_hints: Mutex::new(HashMap::new()),
                slots: Mutex::new(0),
                slot_free: Notify::new(),
                bell: Notify::new(),
            }),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineNotice> {
        self.inner.tx.subscribe()
    }

    pub fn db(&self) -> &WorkspaceDb {
        &self.inner.db
    }

    pub fn workspace_root(&self) -> &Path {
        &self.inner.workspace_root
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.inner.workspace_id
    }

    /// Server start: every execution left running becomes interrupted and re-runs with the same
    /// spawn block; permission asks of dead executions are denied (HANDOVER 11.2).
    pub fn recover(&self) -> Result<(), EngineError> {
        let sessions = self.inner.db.list_sessions()?;
        for summary in sessions {
            let live = self.inner.load(&summary.id)?;
            let (running, stale_gates, terminal) = {
                let st = lock(&live.state);
                let running: Vec<(ExecutionId, bool)> = st
                    .running_executions()
                    .map(|r| (r.id.clone(), st.is_waiting(&r.id)))
                    .collect();
                let stale: Vec<GateId> = st
                    .open_gates()
                    .filter(|g| matches!(g.payload, GatePayload::Permission { .. }))
                    .map(|g| g.id.clone())
                    .collect();
                (running, stale, st.is_terminal())
            };
            for (id, waiting) in running {
                // Rule SM3: a harness run that waited with its process alive still waits, and the
                // message that wakes it resumes the harness session.
                let mut result = if waiting {
                    ExecutionResult::with_status(ExecutionStatus::Waiting)
                } else {
                    let mut r = ExecutionResult::with_status(ExecutionStatus::Interrupted);
                    r.error = Some("The server restarted while this execution ran.".into());
                    r
                };
                // Keep what the execution already spent; the usage deltas were stored as it ran.
                if let Ok(Some(view)) = self.inner.db.get_execution(&id) {
                    result.usage = view.usage;
                    result.native_session_id = view.native_session_id;
                }
                let _ = self.inner.db.finish_execution(&id, &result);
                self.inner
                    .append(&summary.id, SessionEvent::ExecutionFinished { id, result })?;
            }
            for g in stale_gates {
                self.inner.append(
                    &summary.id,
                    SessionEvent::GateAnswered {
                        routed: false,
                        id: g,
                        source: AnswerSource::Engine,
                        answer: GateAnswer::Permission {
                            answer: PermissionAnswer::Deny,
                        },
                        reason: Some(
                            "The execution that asked ended when the server restarted.".into(),
                        ),
                    },
                )?;
            }
            if !terminal {
                self.inner.ensure_driver(&summary.id, live);
            }
        }
        let _ = self.inner.db.mark_running_interrupted();
        Ok(())
    }

    pub fn create_session(&self, req: CreateSession) -> Result<SessionSummary, EngineError> {
        if req.request.trim().is_empty() {
            return Err(EngineError::Invalid("Describe the task first.".into()));
        }
        if let Some(b) = req
            .docs_book
            .as_deref()
            .filter(|b| !ostra_core::book::is_book_id(b))
        {
            return Err(EngineError::Invalid(format!(
                "Pick a documentation book from the list instead of `{b}`: a book ID is lowercase letters, digits, dashes, and underscores."
            )));
        }
        self.start_session(
            SessionKind::Pipeline,
            req.request,
            req.options,
            &req.projects,
            Attached {
                files: &req.files,
                uploads: &req.uploads,
                docs_book: req.docs_book.clone(),
                workflow: Some(match req.workflow.as_deref().map(str::trim) {
                    Some(name) if !name.is_empty() => {
                        // Rule WF1: a workflow that cannot resolve is refused before the session exists.
                        let wf = self
                            .inner
                            .services
                            .workflows()
                            .resolve(name)
                            .map_err(EngineError::Invalid)?;
                        crate::workflow::check_runnable(
                            &wf,
                            &self.inner.services.agents(),
                            &self.inner.services.plugin_stages(),
                        )
                        .map_err(EngineError::Invalid)?;
                        ostra_core::workflow::WorkflowChoice::Named { name: name.into() }
                    }
                    _ => ostra_core::workflow::WorkflowChoice::ByCategory,
                }),
            },
        )
    }

    /// Start the init flow for one project (HANDOVER 8.4).
    pub fn create_init_session(
        &self,
        project: &str,
        focus: Option<String>,
    ) -> Result<SessionSummary, EngineError> {
        let settings = self.inner.services.workspace();
        if settings.project(project).is_none() {
            return Err(EngineError::NotFound(format!(
                "No project `{project}` in this workspace."
            )));
        }
        self.start_session(
            SessionKind::Init {
                project: project.into(),
            },
            focus.unwrap_or_default(),
            SessionOptions {
                yolo: settings.yolo.default,
                ..Default::default()
            },
            &[project.to_string()],
            Attached::default(),
        )
    }

    /// Rules SM2 to SM5: one messaging tool call from a running execution. `Err` is a tool error
    /// shown to the model.
    pub fn coordinate(
        &self,
        session: &SessionId,
        execution: &ExecutionId,
        tool: &str,
        input: &Value,
    ) -> Result<ostra_core::coord::CoordReply, String> {
        use ostra_core::coord::{
            CoordReply, LIST_AGENTS, RunEnd, SEND_MESSAGE, WAIT_FOR_MESSAGE, waiting_text,
        };
        let st = self.state(session).map_err(|e| e.to_string())?;
        let harness = st
            .executions
            .get(execution)
            .is_some_and(|r| matches!(r.executor, ExecutorKind::Harness(_)));
        let helpers = self.inner.services.agents().helpers();
        match tool {
            LIST_AGENTS => Ok(CoordReply {
                text: st.list_agents(execution, &helpers),
                end: RunEnd::Continue,
            }),
            SEND_MESSAGE => {
                let (event, who) = st.send_event(execution, input, &helpers)?;
                let wait = matches!(event, SessionEvent::MessageSent { wait: true, .. });
                let id = match &event {
                    SessionEvent::MessageSent { id, .. } => id.to_string(),
                    _ => String::new(),
                };
                self.inner
                    .append(session, event)
                    .map_err(|e| e.to_string())?;
                let queued = format!(
                    "Queued message {id} for {who}. Ostra hands it over at their next turn boundary, or starts or continues them for it."
                );
                Ok(if wait {
                    CoordReply {
                        text: format!("{queued} {}", waiting_text(harness)),
                        end: RunEnd::Wait,
                    }
                } else {
                    CoordReply {
                        text: format!(
                            "{queued} Continue your task: a reply reaches you at your next turn, or wakes you when you wait with {}.",
                            ostra_core::coord::tool_on(
                                WAIT_FOR_MESSAGE,
                                st.executions
                                    .get(execution)
                                    .map(|r| r.executor)
                                    .unwrap_or_default()
                            )
                        ),
                        end: RunEnd::Continue,
                    }
                })
            }
            WAIT_FOR_MESSAGE => {
                let event = st.wait_event(execution)?;
                self.inner
                    .append(session, event)
                    .map_err(|e| e.to_string())?;
                Ok(CoordReply {
                    text: waiting_text(harness),
                    end: RunEnd::Wait,
                })
            }
            other => Err(format!("Unknown tool `{other}`.")),
        }
    }

    /// Rule SM6: why `execution` may not submit yet.
    pub fn submit_blocked(&self, session: &SessionId, execution: &ExecutionId) -> Option<String> {
        self.state(session).ok()?.submit_blocked(execution)
    }

    /// Rule O3: record a project an agent created once the workspace holds it, so it joins the
    /// session's projects and scope.
    pub fn add_created_project(
        &self,
        session: &SessionId,
        project: ostra_core::manage::CreatedProject,
    ) -> Result<(), EngineError> {
        let st = self.state(session)?;
        if let Some(why) =
            self.pipeline()
                .project_creation_refusal(&st, &project.execution, &project.key)
        {
            return Err(EngineError::Invalid(why));
        }
        std::fs::create_dir_all(st.project_session_dir(&project.key))
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        self.inner
            .append(session, SessionEvent::ProjectCreated { project })?;
        // Rule O4: stop the run that created it; its phase starts again after the init.
        self.inner.interrupt(session, Interrupt::ProjectCreated)?;
        Ok(())
    }

    fn start_session(
        &self,
        kind: SessionKind,
        request: String,
        mut options: SessionOptions,
        pinned: &[String],
        attached: Attached<'_>,
    ) -> Result<SessionSummary, EngineError> {
        let Attached {
            files,
            uploads,
            docs_book,
            workflow,
        } = attached;
        let settings = self.inner.services.workspace();
        if settings.yolo.default {
            options.yolo = true;
        }
        let mut projects: Vec<ProjectRef> = settings
            .projects
            .iter()
            .map(|p| ProjectRef {
                key: p.key.clone(),
                path: p.path.clone(),
            })
            .collect();
        if matches!(kind, SessionKind::Pipeline) {
            // No pipeline task may target an uninitialized project (skill-init-guard.js).
            projects.retain(|p| paths::project_inventory(&p.path).exists());
            if projects.is_empty() {
                return Err(EngineError::Invalid(
                    "No project in this workspace is initialized yet. Initialize a project first, because every agent routes its work by the project's inventory.".into(),
                ));
            }
            if !pinned.is_empty() {
                let unknown: Vec<&String> = pinned
                    .iter()
                    .filter(|k| !projects.iter().any(|p| &p.key == *k))
                    .collect();
                if !unknown.is_empty() {
                    return Err(EngineError::Invalid(format!(
                        "Pinned projects are unknown or not initialized: {unknown:?}"
                    )));
                }
                // Rule O6: a pinned session holds only its pinned projects.
                projects.retain(|p| pinned.contains(&p.key));
            }
        }
        let files = validate_files(files, &projects, &self.inner.workspace_root)?;
        uploads::check(&self.inner.workspace_root, uploads)?;
        let id = SessionId::new();
        let session_root = paths::session_root(&self.inner.workspace_root, id.as_str());
        std::fs::create_dir_all(&session_root).map_err(|e| EngineError::Invalid(e.to_string()))?;
        let uploads = uploads::claim(&self.inner.workspace_root, &session_root, uploads)?;
        let gitignore = paths::sessions_root(&self.inner.workspace_root).join(".gitignore");
        if !gitignore.exists() {
            let _ = std::fs::write(gitignore, "*\n");
        }
        for p in &projects {
            let _ = std::fs::create_dir_all(session_root.join(&p.key));
        }
        self.inner.db.create_session(&NewSession {
            id: id.clone(),
            kind: kind.clone(),
            request: request.clone(),
            category: None,
            projects: pinned.to_vec(),
            yolo: options.yolo,
        })?;
        let live = Arc::new(Live {
            state: Mutex::new(SessionState::new(self.pipeline().clone(), id.clone())),
            wake: Notify::new(),
            inflight: Mutex::new(HashSet::new()),
            driving: AtomicBool::new(false),
        });
        lock(&self.inner.sessions).insert(id.clone(), live.clone());
        self.inner.append(
            &id,
            SessionEvent::SessionCreated {
                kind,
                request,
                options,
                projects,
                workspace_root: self.inner.workspace_root.clone(),
                session_root,
                files,
                uploads,
                pinned: pinned.to_vec(),
                docs_book,
                workflow,
            },
        )?;
        self.inner.ensure_driver(&id, live);
        self.inner
            .db
            .get_session(&id)?
            .ok_or_else(|| EngineError::NotFound(id.to_string()))
    }

    pub fn state(&self, session: &SessionId) -> Result<SessionState, EngineError> {
        let live = self.inner.load(session)?;
        Ok(lock(&live.state).clone())
    }

    pub fn detail(&self, session: &SessionId) -> Result<SessionDetail, EngineError> {
        let st = self.state(session)?;
        self.pipeline().detail(
            &st,
            &self.inner.db,
            &self.inner.workspace_id,
            self.live_executions(),
        )
    }

    /// The session's Sessions tree node, built from its stored row. The fold is read in place.
    pub fn tree_session(&self, summary: &SessionSummary) -> Result<TreeSession, EngineError> {
        let executions = self.inner.db.list_executions(&summary.id)?;
        let live = self.inner.load(&summary.id)?;
        let st = lock(&live.state);
        Ok(self.pipeline().tree_session(&st, summary, executions))
    }

    /// Numbered run labels of a session's executions.
    pub fn run_labels(
        &self,
        session: &SessionId,
    ) -> Result<HashMap<ExecutionId, String>, EngineError> {
        let live = self.inner.load(session)?;
        let st = lock(&live.state);
        Ok(self.pipeline().run_labels(&st))
    }

    fn live_executions(&self) -> HashSet<ExecutionId> {
        lock(&self.inner.execs).keys().cloned().collect()
    }

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

    fn summary_of(&self, session: &SessionId) -> Result<SessionSummary, EngineError> {
        self.inner
            .db
            .get_session(session)?
            .ok_or_else(|| EngineError::NotFound(session.to_string()))
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

    /// Reopen an ended harness execution's session so the user can read its trace and ask about
    /// it (HANDOVER 10.2). Runs outside the pipeline, and every tool call in it is refused.
    pub fn inspect_execution(&self, id: &ExecutionId) -> Result<ExecutionId, EngineError> {
        let view = self
            .inner
            .db
            .get_execution(id)?
            .ok_or_else(|| EngineError::NotFound(format!("execution {id}")))?;
        let ExecutorKind::Harness(_) = view.executor else {
            return Err(EngineError::Invalid(
                "Only a harness execution has a session to reopen. Read a native run in its Activity tab.".into(),
            ));
        };
        if matches!(view.purpose, Some(ExecPurpose::Inspect { .. })) {
            return Err(EngineError::Invalid(
                "Reopen the original run instead of a read-only session.".into(),
            ));
        }
        if view.status == ExecutionStatus::Running {
            return Err(EngineError::Invalid(
                "This execution is still running. Watch it in its Terminal tab instead.".into(),
            ));
        }
        let sid = view.native_session_id.clone().ok_or_else(|| {
            EngineError::Invalid(
                "This run recorded no harness session, so there is nothing to reopen.".into(),
            )
        })?;
        let session = view
            .session
            .clone()
            .ok_or_else(|| EngineError::Invalid("This run belongs to no session.".into()))?;
        let st = self.state(&session)?;
        if st.resume_from.values().any(|x| x == id) {
            return Err(EngineError::Invalid(
                "Continue or stop the session first, because the paused agent resumes this conversation and would see what you ask.".into(),
            ));
        }
        let executor = self.inner.services.executor(view.executor).ok_or_else(|| {
            EngineError::Invalid(format!("The {} executor is not available.", view.executor))
        })?;
        let repo_root = st
            .project_path(&view.project)
            .unwrap_or_else(|| self.inner.workspace_root.clone());
        let new = ExecutionId::new();
        let global = self.inner.services.global();
        let settings = self.inner.services.workspace();
        let ctx = ExecContext {
            execution_id: new.clone(),
            session_id: None,
            agent: view.agent,
            initializer_mode: None,
            executor: view.executor,
            workspace_root: self.inner.workspace_root.clone(),
            repo_root: repo_root.clone(),
            project_key: view.project.clone(),
            session_dir: st.project_session_dir(&view.project),
            session_root: st.session_root.clone(),
            report_file: None,
            phase: None,
            yolo: false,
            permission_mode: ostra_core::config::PermissionMode::Plan,
            permissions: PermissionRules::merged(&[
                &global.permissions,
                &settings.permissions.rules(),
            ]),
            protected_paths: self.inner.services.protected_paths(),
            memory_db: paths::project_memory_db(&repo_root),
            sandbox_mode: settings.sandbox_mode,
            enforce_tool_calls: settings.enforces_tool_calls(&global),
            sandbox_network: settings.sandbox_network,
            sandbox_allowed_hosts: settings.sandbox_allowed_hosts.clone(),
            sandbox_decoys: settings.sandbox_decoys.clone(),
            sandbox_readable: settings.readable_paths(&global),
            sandbox_loopback: settings.sandbox_loopback,
            sandbox_blocked_ports: settings.sandbox_blocked_ports.clone(),
            creates_project: false,
            owes_reply: false,
            write_scope: Some(ostra_core::WriteScope::ReadOnly),
            contract: st
                .executions
                .get(&view.id)
                .map(|r| r.contract)
                .unwrap_or(ostra_core::Contract::Stage),
            capabilities: vec![],
        };
        self.inner.db.insert_execution(&NewExecution {
            id: new.clone(),
            session: None,
            agent: view.agent,
            purpose: Some(ExecPurpose::Inspect { of: id.clone() }),
            stage: None,
            project: view.project.clone(),
            executor: view.executor,
            model: view.model.clone(),
            params: serde_json::json!({ "inspects": id }),
            spawn_block: String::new(),
            report_path: None,
            native_session_id: Some(sid.clone()),
        })?;
        let token = CancellationToken::new();
        lock(&self.inner.execs).insert(new.clone(), (None, token.clone()));
        let inner = self.inner.clone();
        let host = Arc::new(EngineHost {
            inner: inner.clone(),
            session: None,
            execution: new.clone(),
            repo_root: Some(repo_root),
            usage_base: Usage::default(),
            slot: Mutex::new(None),
        });
        let spec = ExecutionSpec {
            id: new.clone(),
            agent: view.agent,
            route: ostra_core::config::ResolvedRoute {
                executor: view.executor,
                model: view.model.clone(),
                tier: None,
            },
            effort: ostra_core::model::Effort::Low,
            system_prompt: INSPECT_PROMPT.into(),
            first_message: String::new(),
            capabilities: vec![],
            submit_schema: Value::Null,
            timeout_secs: INSPECT_TIMEOUT_SECS,
            ctx,
            resume: Some(ResumeInfo {
                from: id.clone(),
                native_session_id: Some(sid),
                note: None,
                inspect: true,
            }),
            harness_session_id: None,
        };
        let exec_id = new.clone();
        tokio::spawn(async move {
            let result = executor.run(spec, host, token).await;
            let _ = inner.db.finish_execution(&exec_id, &result);
            lock(&inner.execs).remove(&exec_id);
            let _ = inner.tx.send(EngineNotice::ExecutionStatus {
                execution: exec_id,
                status: result.status,
            });
        });
        Ok(new)
    }

    /// A side-panel question (HANDOVER 12.3). Runs outside the pipeline and changes no state.
    pub async fn ask(
        &self,
        question: String,
        session: Option<SessionId>,
    ) -> Result<ExecutionId, EngineError> {
        let settings = self.inner.services.workspace();
        let project = settings
            .projects
            .first()
            .cloned()
            .ok_or_else(|| EngineError::Invalid("Add a project first.".into()))?;
        let mut context = String::new();
        context.push_str("# Projects in this workspace\n\n");
        for p in &settings.projects {
            context.push_str(&format!("- `{}` at {}\n", p.key, p.path.display()));
        }
        if let Some(sid) = &session
            && let Ok(st) = self.state(sid)
        {
            let d = self.pipeline().artifacts(&st);
            context.push_str("\n# Artifacts of the session this was asked from\n\n");
            for a in d {
                context.push_str(&format!(
                    "- {} ({}): {}\n",
                    a.label,
                    a.kind,
                    a.path.display()
                ));
            }
        }
        let id = ExecutionId::new();
        let executor = self
            .inner
            .services
            .executor(ExecutorKind::Native)
            .ok_or_else(|| EngineError::Invalid("The native executor is not available.".into()))?;
        let inner = self.inner.clone();
        let (token, host, spec) =
            inner
                .clone()
                .prepare_quick(&id, &project.path, &project.key, question, context)?;
        let exec_id = id.clone();
        tokio::spawn(async move {
            let result = executor.run(spec, host, token).await;
            let _ = inner.db.finish_execution(&exec_id, &result);
            lock(&inner.execs).remove(&exec_id);
            let _ = inner.tx.send(EngineNotice::ExecutionStatus {
                execution: exec_id,
                status: result.status,
            });
        });
        Ok(id)
    }
}

pub const STOPPED_BY_USER: &str = "Stopped by the user.";
/// The system prompt of a read-only reopened session.
const INSPECT_PROMPT: &str = "The user reopened this session to read what you did and ask about it. The work has ended. Answer from the conversation above and do not call any tool, because Ostra refuses every tool call in this session. Keep answers short.";
/// A read-only session ends on its own after this long.
const INSPECT_TIMEOUT_SECS: u64 = 4 * 60 * 60;
/// What a paused execution hears when the session continues (Rule P2).
pub const PAUSE_RESUME_NOTE: &str = "Continue the workflow.";

/// Rule U2: the message a run resumes with after the user sent it a correction.
pub fn steer_note(text: &str) -> String {
    format!(
        "The user stopped you to send this correction. Follow it, then continue the workflow from where you stopped, because the rest of your task is unchanged.\n\n{text}"
    )
}
/// The Activity line where a resumed execution picks up (Rule P2).
pub const RESUMED_STATUS: &str = "The session continued, so this run resumes where it stopped.";
/// Files one request or addition may attach (Rule C1).
pub const MAX_CONTEXT_FILES: usize = 50;

/// Rule C1: every attached path is an existing file or folder inside one of the session's projects,
/// or a visible workspace artifact (Rule W2). Returns them with normalized paths, a folder ending in
/// `/`, deduplicated.
fn validate_files(
    files: &[ContextFile],
    projects: &[ProjectRef],
    workspace_root: &Path,
) -> Result<Vec<ContextFile>, EngineError> {
    if files.len() > MAX_CONTEXT_FILES {
        return Err(EngineError::Invalid(format!(
            "Attach at most {MAX_CONTEXT_FILES} files, because each one is read by every agent."
        )));
    }
    let mut out: Vec<ContextFile> = vec![];
    for f in files {
        if f.project == artifacts::TAG_ROOT {
            // Rule W2: a hidden artifact is not in the folder, so it cannot be tagged.
            let file = artifacts::normalize(&f.path)
                .ok()
                .filter(|rel| artifacts::visible_file(workspace_root, rel).is_some())
                .map(|path| ContextFile {
                    project: f.project.clone(),
                    path,
                })
                .ok_or_else(|| {
                    EngineError::Invalid(format!(
                        "Tag a workspace artifact that exists and is not hidden. `{}` is not one.",
                        f.path
                    ))
                })?;
            if !out.contains(&file) {
                out.push(file);
            }
            continue;
        }
        let Some(project) = projects.iter().find(|p| p.key == f.project) else {
            return Err(EngineError::Invalid(format!(
                "Tag files from this session's projects only. `{}` is not one of them.",
                f.project
            )));
        };
        let rel = f.path.trim().replace('\\', "/");
        let rel = rel
            .trim_start_matches("./")
            .trim_end_matches('/')
            .to_string();
        let outside = || {
            EngineError::Invalid(format!(
                "Tag a file or folder inside `{}`. `{}` is not one.",
                f.project, f.path
            ))
        };
        let path = Path::new(&rel);
        if rel.is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(outside());
        }
        let root = ostra_core::paths::canonical(&project.path).map_err(|_| outside())?;
        let full = ostra_core::paths::canonical(root.join(path)).map_err(|_| outside())?;
        let wants_folder = f.path.trim_end().ends_with('/');
        if !full.starts_with(&root)
            || !(full.is_file() || full.is_dir())
            || (wants_folder && !full.is_dir())
        {
            return Err(outside());
        }
        let file = ContextFile {
            project: f.project.clone(),
            path: if full.is_dir() {
                format!("{rel}/")
            } else {
                rel
            },
        };
        if !out.contains(&file) {
            out.push(file);
        }
    }
    Ok(out)
}

/// Stop a session while no server holds it, so a later start does not recover and re-run its
/// executions. Running executions become cancelled and keep what they spent. Returns how many
/// executions were cancelled.
pub fn stop_session_offline(
    pipeline: Arc<dyn crate::pipeline::Pipeline>,
    db: &WorkspaceDb,
    session: &SessionId,
) -> Result<usize, EngineError> {
    let events = db.events(session)?;
    if events.is_empty() {
        return Err(EngineError::NotFound(format!("session {session}")));
    }
    let st = SessionState::fold(pipeline.into(), session.clone(), &events);
    if st.is_terminal() {
        return Err(EngineError::Invalid(
            "This session has already ended.".into(),
        ));
    }
    let running: Vec<ExecutionId> = st.running_executions().map(|r| r.id.clone()).collect();
    for id in &running {
        let mut result = ExecutionResult::with_status(ExecutionStatus::Cancelled);
        result.error = Some(STOPPED_BY_USER.into());
        if let Ok(Some(view)) = db.get_execution(id) {
            result.usage = view.usage;
            result.native_session_id = view.native_session_id;
        }
        let _ = db.finish_execution(id, &result);
        db.append_event(
            session,
            &SessionEvent::ExecutionFinished {
                id: id.clone(),
                result,
            },
        )?;
    }
    db.append_event(
        session,
        &SessionEvent::SessionFailed {
            error: STOPPED_BY_USER.into(),
        },
    )?;
    db.update_session(
        session,
        &SessionUpdate {
            status: Some(ostra_core::api::SessionStatus::Failed),
            lane: Some(ostra_core::pipeline::Lane::Done),
            stage_label: Some(format!("Stopped: {STOPPED_BY_USER}")),
            ..Default::default()
        },
    )?;
    Ok(running.len())
}

fn validate_answer(
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

impl Inner {
    fn pipeline(&self) -> &PipelineRef {
        &self.pipeline
    }

    fn load(&self, id: &SessionId) -> Result<Arc<Live>, EngineError> {
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
    fn append(&self, session: &SessionId, event: SessionEvent) -> Result<StoredEvent, EngineError> {
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

    fn materialize(&self, session: &SessionId, event: &SessionEvent) -> Result<(), EngineError> {
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

    fn push_for(&self, session: &SessionId, event: &SessionEvent) {
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

    fn sessions_yolo(&self, session: &SessionId) -> bool {
        lock(&self.sessions)
            .get(session)
            .map(|l| lock(&l.state).yolo)
            .unwrap_or(false)
    }

    fn ensure_driver(self: &Arc<Self>, id: &SessionId, live: Arc<Live>) {
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

    async fn drive(self: Arc<Self>, id: SessionId, live: Arc<Live>) {
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

    fn plan_ctx(&self, st: &SessionState) -> PlanCtx {
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

    fn snapshot(&self, session: &SessionId) -> Result<SessionState, EngineError> {
        Ok(lock(&self.load(session)?.state).clone())
    }

    /// Cancel the executions the fold marked as interrupting, and deny their waiting permission
    /// asks, because the run that asked is ending.
    fn interrupt(&self, session: &SessionId, why: Interrupt) -> Result<(), EngineError> {
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
    fn checkpoints(
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

    async fn perform(self: &Arc<Self>, session: &SessionId, step: Step) -> Result<(), EngineError> {
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
            Step::Command {
                purpose,
                project,
                command,
                files,
            } => {
                self.perform_command(session, purpose, &project, command, files)
                    .await
            }
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
            other => Err(EngineError::Invalid(format!(
                "The pipeline did not perform step `{}`.",
                other.summary()
            ))),
        }
    }

    fn project_facts(&self) -> Vec<ProjectFacts> {
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

    /// Rule WB3: one model call for a prompt node, on its tier, checked against its output
    /// schema. Returns the output or the error, and what the calls cost.
    async fn prompt_node(
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
    async fn call_judge(
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

    async fn perform_judge(
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
            Ok(_) if kind == JudgeKind::RouteAnswer => self.interrupt(session, Interrupt::Skipped),
            Ok(_) => Ok(()),
            Err(e) => {
                if kind == JudgeKind::Completion {
                    // The session's work is done; a report without prose beats no report.
                    let md = format!(
                        "# Session complete\n\nThe completion judge failed ({e}), so this report lists the state only.\n\n{}",
                        self.pipeline()
                            .judge_input(&st, JudgeKind::Completion, None, &[])
                            .0
                    );
                    self.append(
                        session,
                        SessionEvent::DecisionMade {
                            id: DecisionId::new(),
                            judge: JudgeKind::Completion,
                            subject: None,
                            input_summary: "Fallback after a failed judge call".into(),
                            output: serde_json::json!({"report_markdown": md, "reason": "The completion judge failed."}),
                            reason: "The completion judge failed.".into(),
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

    async fn perform_yolo(
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

    async fn perform_command(
        self: &Arc<Self>,
        session: &SessionId,
        purpose: CommandPurpose,
        project: &str,
        command: Option<String>,
        files: Vec<String>,
    ) -> Result<(), EngineError> {
        let root = self
            .snapshot(session)?
            .project_path(project)
            .unwrap_or_default();
        let (cmd_text, exit, tail) = match purpose {
            CommandPurpose::Format => match command {
                None => (
                    String::new(),
                    None,
                    "No format command in project.toml, so format was skipped.".to_string(),
                ),
                // Rule A1: a format command runs only once the user approved it.
                Some(cmd) if !self.services.command_approved(&root, &cmd) => {
                    (cmd, None, FORMAT_NOT_APPROVED.to_string())
                }
                Some(cmd) => {
                    self.append_command_started(session, purpose, project, &cmd)?;
                    // The project's own program, so it runs under the agent sandbox.
                    let (code, out) = match ostra_sandbox::host_command(
                        "bash",
                        &["-c".into(), cmd.clone()],
                        &root,
                        &[&root],
                        &self.services.workspace().sandbox(),
                    ) {
                        Ok(hc) => run_host(&root, &hc, 600).await,
                        Err(e) => (None, e),
                    };
                    (cmd, code, out)
                }
            },
            CommandPurpose::Stage => {
                if files.is_empty() {
                    (String::new(), Some(0), "No files to stage.".to_string())
                } else {
                    // Staging keeps each review focused on the unstaged diff (Step 2).
                    for p in ostra_sandbox::repair_git_dirs(&ostra_sandbox::git_repos(&[&root])) {
                        tracing::warn!("removed a planted {} before staging", p.display());
                    }
                    let mut args: Vec<String> = ostra_core::git::AUTOMATIC
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    args.extend(ostra_core::git::filter_overrides(&root).await);
                    args.extend([
                        "-C".into(),
                        root.display().to_string(),
                        "add".into(),
                        "-A".into(),
                        "--".into(),
                    ]);
                    args.extend(files.iter().cloned());
                    let text = format!("git {}", args.join(" "));
                    self.append_command_started(session, purpose, project, &text)?;
                    let (code, out) = run_shell(&root, "git", &args, 60).await;
                    (text, code, out)
                }
            }
            CommandPurpose::Autofix => (String::new(), Some(0), String::new()),
        };
        self.append(
            session,
            SessionEvent::CommandRan {
                purpose,
                project: project.into(),
                command: cmd_text,
                exit_code: exit,
                output_tail: tail,
            },
        )?;
        Ok(())
    }

    fn append_command_started(
        &self,
        session: &SessionId,
        purpose: CommandPurpose,
        project: &str,
        command: &str,
    ) -> Result<(), EngineError> {
        self.append(
            session,
            SessionEvent::CommandStarted {
                purpose,
                project: project.into(),
                command: command.into(),
            },
        )?;
        Ok(())
    }

    fn record_denied(
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
    fn take_messages(&self, session: &SessionId, execution: &ExecutionId) -> Option<Wake> {
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

    async fn acquire_slot(self: &Arc<Self>) -> Slot {
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

    async fn perform_spawn(
        self: &Arc<Self>,
        session: &SessionId,
        req: SpawnRequest,
    ) -> Result<(), EngineError> {
        let slot = self.acquire_slot().await;
        let st = self.snapshot(session)?;
        if st.is_terminal() || st.paused {
            return Ok(());
        }
        if self.pipeline().spawn_dropped(&st, &req) {
            return Ok(());
        }
        self.pipeline()
            .before_spawn(&st, &req)
            .map_err(EngineError::Invalid)?;
        // Rule P2: a run the pause interrupted continues under its own id, where it stopped.
        let paused = req
            .resumes
            .as_ref()
            .and_then(|from| st.executions.get(from))
            .filter(|rec| {
                rec.agent == req.agent
                    && rec.result.as_ref().is_some_and(|r| {
                        matches!(
                            r.status,
                            ExecutionStatus::Interrupted | ExecutionStatus::Waiting
                        )
                    })
            });
        // Another step already resumed this run.
        if req.resumes.is_some() && paused.is_none() && req.continues.is_none() {
            return Ok(());
        }
        // Rule SM3: a waiting run wakes only with its messages.
        let wake = match paused {
            Some(rec)
                if rec
                    .result
                    .as_ref()
                    .is_some_and(|r| r.status == ExecutionStatus::Waiting) =>
            {
                match st.next_delivery(&rec.id) {
                    Some(d) => Some(d),
                    None => return Ok(()),
                }
            }
            _ => None,
        };
        // Rules H3 and H5: a new run that continues another run's conversation.
        let continued = match paused {
            None => req.continues.as_ref().and_then(|h| st.executions.get(h)),
            Some(_) => None,
        };
        let global = self.services.global();
        let settings = self.services.workspace();
        let factory = self.services.factory();
        let agents = self.services.agents();
        let Some(meta) = factory.agent_meta(req.agent, &agents) else {
            return self.record_denied(
                session,
                &req,
                ExecutorKind::Native,
                String::new(),
                format!(
                    "The workspace defines no agent `{}` any more. Restore its file in .ostra/agents, or change the workflow.",
                    req.agent
                ),
            );
        };
        let complexity = req.complexity();
        let tier_override = self.pipeline().tier_override(&req);
        let executor_override = self.pipeline().forced_executor(&st, req.agent);
        let mut route = match resolve_route(
            &global,
            &settings,
            RouteQuery {
                key: req.agent.as_str(),
                default_tier: meta.default_tier,
                complexity,
                tier_override,
                executor_override,
            },
        ) {
            Ok(r) => r,
            Err(e) => {
                return self.record_denied(
                    session,
                    &req,
                    ExecutorKind::Native,
                    String::new(),
                    format!("route: {}", e.0),
                );
            }
        };
        // The conversation continues on the executor and model it started on, which also keeps
        // the prompt cache.
        if let Some(rec) = paused.or(continued) {
            route.executor = rec.executor;
            route.model = rec.model.clone();
        }
        // Rule PL2: a programmatic agent runs in its plugin's code, natively, whatever its route
        // says; the route's model serves its model calls.
        if meta.programmatic {
            route.executor = ExecutorKind::Native;
            if let Ok(native) = resolve_route(
                &global,
                &settings,
                RouteQuery {
                    executor_override: Some(ExecutorKind::Native),
                    ..RouteQuery::new(req.agent.as_str(), meta.default_tier)
                },
            ) {
                route.model = native.model;
            }
        }
        let executor = if meta.programmatic {
            self.services.program_executor(req.agent)
        } else {
            self.services.executor(route.executor)
        };
        let Some(executor) = executor else {
            return self.record_denied(
                session,
                &req,
                route.executor,
                route.model,
                format!("The {} executor is not available.", route.executor),
            );
        };
        // Rule SM4: a run that continues an ended subagent starts with the messages sent to it.
        let delivery = match &req.purpose {
            ExecPurpose::Message { subagent, .. } => {
                match st.continuation_delivery(subagent, route.executor) {
                    Some(d) => Some(d),
                    None => return Ok(()),
                }
            }
            _ => None,
        };
        // Rule O2: a phase in a project that does not exist yet runs from its session dir, so its
        // sandbox can write nowhere else until it creates the project.
        // Rule CA6: a run that holds the project tools creates its phase's new project.
        let creates_project = meta
            .capabilities
            .contains(&ostra_core::Capability::ManageProjects)
            && self
                .pipeline()
                .project_to_create(&st, &req.project)
                .is_some();
        let repo_root = if creates_project {
            let _ = std::fs::create_dir_all(&req.session_dir);
            req.session_dir.clone()
        } else {
            st.project_path(&req.project)
                .unwrap_or_else(|| self.workspace_root.clone())
        };
        let profile: Option<ProjectProfile> = load_toml(&paths::project_profile(&repo_root)).ok();
        let inventory = std::fs::read_to_string(paths::project_inventory(&repo_root)).ok();
        let project_docs = ostra_agents::brief::project_docs(&repo_root);
        let _ = std::fs::create_dir_all(&req.session_dir);
        self.pipeline().spawn_files(&st, &req);
        // A paused run, and a run that continues a subagent for its messages, carry on with their
        // own transcript, so they need the prompt and the spawn they had, not a new spawn block:
        // their steps carry no stage inputs (Rules SM3 and SM4).
        let reuse = paused.or(match req.purpose {
            ExecPurpose::Message { .. } => continued,
            _ => None,
        });
        let rebuilt = reuse.map(|rec| -> Result<BuiltSpawn, String> {
            Ok(BuiltSpawn {
                system_prompt: agents
                    .render_prompt(rec.agent, route.executor)
                    .map_err(|e| e.to_string())?,
                first_message: rec.spawn_block.clone(),
                spawn_block: rec.spawn_block.clone(),
                params: rec.params.clone(),
                report_file: rec.report_path.clone(),
                effort: ostra_core::config::resolve_effort(
                    &settings,
                    rec.agent.as_str(),
                    complexity,
                )
                .or_else(|| agents.def(rec.agent).map(|d| d.effort_on(route.executor)))
                .unwrap_or(ostra_core::Effort::High),
            })
        });
        let built = match rebuilt.unwrap_or_else(|| {
            factory.build(
                &req,
                &SpawnEnv {
                    state: &st,
                    executor: route.executor,
                    settings: &settings,
                    profile: profile.as_ref(),
                    inventory: inventory.as_deref(),
                    repo_root: &repo_root,
                    project_docs: &project_docs,
                    agents: &agents,
                },
            )
        }) {
            Ok(b) => b,
            Err(e) => {
                return self.record_denied(
                    session,
                    &req,
                    route.executor,
                    route.model,
                    format!("spawn: {e}"),
                );
            }
        };
        let mut params = built.params.clone();
        self.pipeline()
            .spawn_params(&req, profile.as_ref(), &mut params);
        let hint = lock(&self.resume_hints).remove(&purpose_key(&req.purpose));
        let (id, resume, report_file) = match paused {
            Some(rec) => (
                rec.id.clone(),
                Some(ResumeInfo {
                    from: rec.id.clone(),
                    native_session_id: rec
                        .result
                        .as_ref()
                        .and_then(|r| r.native_session_id.clone()),
                    note: Some(match (&wake, st.steers.get(&rec.id)) {
                        (Some(d), _) => d.note.clone(),
                        (None, Some(steer)) => steer_note(&steer.text),
                        (None, None) => PAUSE_RESUME_NOTE.into(),
                    }),
                    inspect: false,
                }),
                rec.report_path.clone(),
            ),
            None => (
                ExecutionId::new(),
                hint.or_else(|| {
                    continued.map(|rec| ResumeInfo {
                        from: rec.id.clone(),
                        native_session_id: rec
                            .result
                            .as_ref()
                            .and_then(|r| r.native_session_id.clone()),
                        note: Some(match (&req.purpose, &delivery) {
                            (ExecPurpose::Message { .. }, Some(d)) => {
                                crate::coord::message_continuation_note(&d.note)
                            }
                            _ => crate::coord::continuation_note(&built.spawn_block),
                        }),
                        inspect: false,
                    })
                }),
                built.report_file.clone(),
            ),
        };
        let ctx = ExecContext {
            execution_id: id.clone(),
            session_id: Some(session.clone()),
            agent: req.agent,
            initializer_mode: match &req.purpose {
                ExecPurpose::Init { mode, .. } => Some(*mode),
                _ => None,
            },
            executor: route.executor,
            workspace_root: self.workspace_root.clone(),
            repo_root: repo_root.clone(),
            project_key: req.project.clone(),
            session_dir: req.session_dir.clone(),
            session_root: st.session_root.clone(),
            report_file,
            phase: req.inputs.phase_value.clone(),
            yolo: st.yolo,
            permission_mode: settings.permissions.mode,
            permissions: PermissionRules::merged(&[
                &global.permissions,
                &settings.permissions.rules(),
            ]),
            protected_paths: self.services.protected_paths(),
            memory_db: paths::project_memory_db(&repo_root),
            sandbox_mode: settings.sandbox_mode,
            enforce_tool_calls: settings.enforces_tool_calls(&global),
            sandbox_network: settings.sandbox_network,
            sandbox_allowed_hosts: settings.sandbox_allowed_hosts.clone(),
            sandbox_decoys: settings.sandbox_decoys.clone(),
            sandbox_readable: settings.readable_paths(&global),
            sandbox_loopback: settings.sandbox_loopback,
            sandbox_blocked_ports: settings.sandbox_blocked_ports.clone(),
            creates_project,
            owes_reply: wake
                .as_ref()
                .or(delivery.as_ref())
                .is_some_and(|d| !d.owes.is_empty()),
            write_scope: Some(meta.write_scope),
            contract: meta.returns,
            capabilities: meta.capabilities.clone(),
        };
        let usage_base = if paused.is_some() {
            if let Some(d) = &wake {
                self.append(
                    session,
                    SessionEvent::MessagesDelivered {
                        to: id.clone(),
                        ids: d.ids.clone(),
                        notice: d.notice.clone(),
                    },
                )?;
            }
            self.append(session, SessionEvent::ExecutionResumed { id: id.clone() })?;
            self.db.reopen_execution(&id)?.usage
        } else {
            self.append(
                session,
                SessionEvent::ExecutionStarted {
                    id: id.clone(),
                    agent: req.agent,
                    purpose: req.purpose.clone(),
                    stage: req.stage,
                    project: req.project.clone(),
                    executor: route.executor,
                    model: route.model.clone(),
                    params: params.clone(),
                    spawn_block: built.spawn_block.clone(),
                    report_path: built.report_file.clone(),
                    resumes: resume.as_ref().map(|r| r.from.clone()),
                    contract: Some(meta.returns),
                },
            )?;
            if let Some(d) = &delivery {
                self.append(
                    session,
                    SessionEvent::MessagesDelivered {
                        to: id.clone(),
                        ids: d.ids.clone(),
                        notice: None,
                    },
                )?;
            }
            self.db.insert_execution(&NewExecution {
                id: id.clone(),
                session: Some(session.clone()),
                agent: req.agent,
                purpose: Some(req.purpose.clone()),
                stage: Some(req.stage),
                project: req.project.clone(),
                executor: route.executor,
                model: route.model.clone(),
                params,
                spawn_block: built.spawn_block.clone(),
                report_path: built.report_file.clone(),
                native_session_id: None,
            })?;
            Usage::default()
        };
        let _ = self.tx.send(EngineNotice::ExecutionStatus {
            execution: id.clone(),
            status: ExecutionStatus::Running,
        });
        let token = CancellationToken::new();
        lock(&self.execs).insert(id.clone(), (Some(session.clone()), token.clone()));
        // A pause or an interrupt that landed while this run was being set up.
        if self.snapshot(session)?.interrupting.contains_key(&id) {
            token.cancel();
        }
        let host = Arc::new(EngineHost {
            inner: self.clone(),
            session: Some(session.clone()),
            execution: id.clone(),
            repo_root: Some(repo_root.clone()),
            usage_base,
            slot: Mutex::new(Some(slot)),
        });
        if paused.is_some() {
            host.emit(ExecutionDelta::Status {
                message: RESUMED_STATUS.into(),
            });
        }
        let harness_session_id = match route.executor {
            ExecutorKind::Harness(
                ostra_core::HarnessKind::Claude | ostra_core::HarnessKind::Grok,
            ) => Some(
                resume
                    .as_ref()
                    .and_then(|r| r.native_session_id.clone())
                    .unwrap_or_else(uuid_v4),
            ),
            _ => None,
        };
        let spec = ExecutionSpec {
            id: id.clone(),
            agent: req.agent,
            route,
            effort: built.effort,
            system_prompt: built.system_prompt,
            first_message: built.first_message,
            capabilities: meta.capabilities.clone(),
            submit_schema: meta.submit_schema.clone(),
            timeout_secs: meta.timeout_secs,
            ctx,
            resume,
            harness_session_id,
        };
        let mut result = executor.run(spec, host.clone(), token).await;
        lock(&self.execs).remove(&id);
        let mut usage = usage_base;
        usage.add(&result.usage);
        result.usage = usage;
        if result.status == ExecutionStatus::Cancelled
            && let Some(why) = self.snapshot(session)?.interrupting.get(&id).copied()
        {
            result.status = ExecutionStatus::Interrupted;
            result.error = Some(why.message().into());
            host.emit(ExecutionDelta::Status {
                message: why.message().into(),
            });
        }
        let _ = self.db.finish_execution(&id, &result);
        let _ = self.tx.send(EngineNotice::ExecutionStatus {
            execution: id.clone(),
            status: result.status,
        });
        let after = self.pipeline().after_run(&req, &result);
        self.append(session, SessionEvent::ExecutionFinished { id, result })?;
        for event in after {
            self.append(session, event)?;
        }
        drop(lock(&host.slot).take());
        Ok(())
    }

    fn prepare_quick(
        self: Arc<Self>,
        id: &ExecutionId,
        repo_root: &Path,
        project: &str,
        question: String,
        context: String,
    ) -> Result<(CancellationToken, Arc<EngineHost>, ExecutionSpec), EngineError> {
        let global = self.services.global();
        let settings = self.services.workspace();
        let factory = self.services.factory();
        // Rule PL4: the side panel runs the standard agent for answers.
        let agent = self.pipeline().default_agent(ostra_core::Contract::Answer);
        let meta = factory
            .agent_meta(agent, &self.services.agents())
            .ok_or_else(|| EngineError::Invalid(format!("{agent} is missing")))?;
        let route = resolve_route(
            &global,
            &settings,
            RouteQuery {
                executor_override: Some(ExecutorKind::Native),
                ..RouteQuery::new(agent.as_str(), meta.default_tier)
            },
        )
        .map_err(|e| EngineError::Invalid(e.0))?;
        let system = factory
            .judge_prompt(&format!("__agent:{agent}"))
            .ok_or_else(|| EngineError::Invalid(format!("The {agent} prompt is missing.")))?;
        let first = format!("{context}\n# Question\n\n{question}\n");
        let ctx = ExecContext {
            execution_id: id.clone(),
            session_id: None,
            agent,
            initializer_mode: None,
            executor: ExecutorKind::Native,
            workspace_root: self.workspace_root.clone(),
            repo_root: repo_root.to_path_buf(),
            project_key: project.into(),
            session_dir: std::env::temp_dir().join("ostra-quick").join(id.as_str()),
            session_root: std::env::temp_dir().join("ostra-quick").join(id.as_str()),
            report_file: None,
            phase: None,
            yolo: false,
            permission_mode: ostra_core::config::PermissionMode::Plan,
            permissions: PermissionRules::merged(&[
                &global.permissions,
                &settings.permissions.rules(),
            ]),
            protected_paths: self.services.protected_paths(),
            memory_db: paths::project_memory_db(repo_root),
            sandbox_mode: settings.sandbox_mode,
            enforce_tool_calls: settings.enforces_tool_calls(&global),
            sandbox_network: settings.sandbox_network,
            sandbox_allowed_hosts: settings.sandbox_allowed_hosts.clone(),
            sandbox_decoys: settings.sandbox_decoys.clone(),
            sandbox_readable: settings.readable_paths(&global),
            sandbox_loopback: settings.sandbox_loopback,
            sandbox_blocked_ports: settings.sandbox_blocked_ports.clone(),
            creates_project: false,
            owes_reply: false,
            write_scope: Some(meta.write_scope),
            contract: meta.returns,
            capabilities: meta.capabilities.clone(),
        };
        self.db.insert_execution(&NewExecution {
            id: id.clone(),
            session: None,
            agent,
            purpose: Some(ExecPurpose::QuickAnswer),
            stage: None,
            project: project.into(),
            executor: ExecutorKind::Native,
            model: route.model.clone(),
            params: serde_json::json!({"question": question}),
            spawn_block: String::new(),
            report_path: None,
            native_session_id: None,
        })?;
        let token = CancellationToken::new();
        lock(&self.execs).insert(id.clone(), (None, token.clone()));
        let host = Arc::new(EngineHost {
            inner: self.clone(),
            session: None,
            execution: id.clone(),
            repo_root: Some(repo_root.to_path_buf()),
            usage_base: Usage::default(),
            slot: Mutex::new(None),
        });
        let spec = ExecutionSpec {
            id: id.clone(),
            agent,
            route,
            effort: ostra_core::model::Effort::Medium,
            system_prompt: system,
            first_message: first,
            capabilities: meta.capabilities,
            submit_schema: meta.submit_schema.clone(),
            timeout_secs: meta.timeout_secs,
            ctx,
            resume: None,
            harness_session_id: None,
        };
        Ok((token, host, spec))
    }
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

async fn run_shell(
    cwd: &Path,
    program: &str,
    args: &[String],
    timeout_secs: u64,
) -> (Option<i32>, String) {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args);
    run_command(cmd, cwd, program, timeout_secs).await
}

async fn run_host(
    cwd: &Path,
    hc: &ostra_sandbox::HostCommand,
    timeout_secs: u64,
) -> (Option<i32>, String) {
    let mut cmd = tokio::process::Command::new(&hc.program);
    for k in &hc.env_remove {
        cmd.env_remove(k);
    }
    cmd.args(&hc.args).envs(hc.env.iter().map(|(k, v)| (k, v)));
    run_command(cmd, cwd, &hc.program, timeout_secs).await
}

async fn run_command(
    mut cmd: tokio::process::Command,
    cwd: &Path,
    program: &str,
    timeout_secs: u64,
) -> (Option<i32>, String) {
    cmd.current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let fut = cmd.output();
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await {
        Ok(Ok(out)) => {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.code(), tail(&text, 4000))
        }
        Ok(Err(e)) => (None, format!("could not run {program}: {e}")),
        Err(_) => (None, format!("{program} timed out after {timeout_secs} s")),
    }
}

fn tail(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut start = s.len() - n;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", &s[start..])
}

/// Rule PL8: a plugin's checkpoints in one session, read from the fold and saved as events.
struct SessionCheckpoints {
    inner: Arc<Inner>,
    session: SessionId,
    plugin: String,
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

/// Callbacks for one running execution.
pub struct EngineHost {
    inner: Arc<Inner>,
    session: Option<SessionId>,
    execution: ExecutionId,
    /// Tool call paths in the summary line are shown relative to it.
    repo_root: Option<PathBuf>,
    /// What the execution spent before a resume (Rule P2); executors count from zero.
    usage_base: Usage,
    /// The execution slot this run holds, freed while it waits for a message (Rule H2).
    slot: Mutex<Option<Slot>>,
}

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
}

impl Engine {
    pub fn execution(&self, id: &ExecutionId) -> Result<ExecutionView, EngineError> {
        let mut v = self
            .inner
            .db
            .get_execution(id)?
            .ok_or_else(|| EngineError::NotFound(format!("execution {id}")))?;
        v.has_terminal = matches!(v.executor, ExecutorKind::Harness(_))
            && lock(&self.inner.execs).contains_key(id);
        if let Some(session) = v.session.clone() {
            let st = self.state(&session)?;
            self.pipeline()
                .decorate(&st, &self.pipeline().run_labels(&st), &mut v);
        }
        Ok(v)
    }
}
