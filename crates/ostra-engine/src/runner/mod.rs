//! The runner: one driver task per live session. It folds events, asks the planner for the next
//! steps, performs them, and appends what happened. Every state change is an event first.

mod api;
mod control;
mod driver;
mod host;
mod inspect;
mod judges;
mod sessions;
mod spawn;

pub use control::*;
pub use sessions::*;
pub use spawn::*;

use crate::pipeline::PipelineRef;
use crate::services::Services;
use crate::state::SessionState;
use ostra_core::api::{ActivityItem, SessionSummary};
use ostra_core::event::{ContextFile, SessionEvent, StoredEvent};
use ostra_core::exec::{CancellationToken, ExecutionStatus, ResumeInfo, Usage};
use ostra_core::ids::{ExecutionId, GateId, SessionId, WorkspaceId};
use ostra_core::policy::PermissionAnswer;
use ostra_store::WorkspaceDb;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
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
pub(crate) struct Attached<'a> {
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

    pub fn services(&self) -> &Arc<dyn Services> {
        &self.0.services
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

pub const STOPPED_BY_USER: &str = "Stopped by the user.";

/// The system prompt of a read-only reopened session.
const INSPECT_PROMPT: &str = "The user reopened this session to read what you did and ask about it. The work has ended. Answer from the conversation above and do not call any tool, because Ostra refuses every tool call in this session. Keep answers short.";

/// A read-only session ends on its own after this long.
const INSPECT_TIMEOUT_SECS: u64 = 4 * 60 * 60;

/// What a paused execution hears when the session continues (Rule P2).
pub const PAUSE_RESUME_NOTE: &str = "Continue the workflow.";

/// The Activity line where a resumed execution picks up (Rule P2).
pub const RESUMED_STATUS: &str = "The session continued, so this run resumes where it stopped.";

/// Files one request or addition may attach (Rule C1).
pub const MAX_CONTEXT_FILES: usize = 50;

/// Rule PL8: a plugin's checkpoints in one session, read from the fold and saved as events.
struct SessionCheckpoints {
    inner: Arc<Inner>,
    session: SessionId,
    plugin: String,
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
