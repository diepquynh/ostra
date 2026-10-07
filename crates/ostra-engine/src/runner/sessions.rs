//! Creating, starting, and recovering sessions, and stopping one while no server holds it.

use super::*;
use crate::state::{Interrupt, SessionState};
use crate::uploads;
use ostra_core::api::{CreateSession, SessionSummary};
use ostra_core::artifacts;
use ostra_core::event::{
    AnswerSource, ContextFile, GateAnswer, GatePayload, ProjectRef, SessionEvent, SessionKind,
    SessionOptions,
};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, GateId, SessionId};
use ostra_core::paths;
use ostra_core::policy::PermissionAnswer;
use ostra_store::{NewSession, SessionUpdate, WorkspaceDb};
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

impl Engine {
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

    pub(crate) fn start_session(
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
}

/// Rule C1: every attached path is an existing file or folder inside one of the session's projects,
/// or a visible workspace artifact (Rule W2). Returns them with normalized paths, a folder ending in
/// `/`, deduplicated.
pub(crate) fn validate_files(
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
