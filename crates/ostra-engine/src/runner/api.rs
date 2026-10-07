//! The engine's read side: its parts, session state, views, and executions.

use super::*;
use crate::pipeline::PipelineRef;
use crate::state::SessionState;
use ostra_core::api::{ExecutionView, SessionDetail, SessionSummary, TreeSession};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, SessionId, WorkspaceId};
use ostra_store::WorkspaceDb;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, broadcast};

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

    pub(crate) fn live_executions(&self) -> HashSet<ExecutionId> {
        lock(&self.inner.execs).keys().cloned().collect()
    }

    pub(crate) fn summary_of(&self, session: &SessionId) -> Result<SessionSummary, EngineError> {
        self.inner
            .db
            .get_session(session)?
            .ok_or_else(|| EngineError::NotFound(session.to_string()))
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
