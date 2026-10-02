//! `<workspace>/.ostra/workspace.db`: the session event log (source of truth) and the tables the
//! engine materializes beside it for the UI.
//!
//! Each table has a repository in its own module that holds its SQL and row mapping.
//! `WorkspaceDb` owns the connection and decides which calls share a transaction.

mod activity;
mod costs;
mod decisions;
mod events;
mod executions;
mod gates;
mod messages;
mod meta;
mod projects;
mod schema;
mod search;
mod sessions;
mod tool_calls;

pub use executions::{ExecutionOutput, NewExecution};
pub use messages::StoredMessage;
pub use projects::ProjectRow;
pub use search::{TextHit, fts_prefix_query};
pub use sessions::{NewSession, SessionUpdate};
pub use tool_calls::{TOOL_OUTPUT_LIMIT, ToolCallRecord};

use crate::StoreError;
use crate::sqlite::{Db, migrate, open_file};
use activity::Activity;
use chrono::{DateTime, Utc};
use costs::Costs;
use decisions::Decisions;
use events::Events;
use executions::Executions;
use gates::Gates;
use messages::Messages;
use meta::Meta;
use ostra_core::api::{
    ActivityItem, CostReport, DecisionView, ExecutionView, GateView, InitStatus, SessionSummary,
};
use ostra_core::event::{
    AnswerSource, GateAnswer, GatePayload, JudgeKind, SessionEvent, StoredEvent,
};
use ostra_core::exec::{ExecutionDelta, ExecutionResult, ExecutionStatus, Usage};
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
use projects::Projects;
use rusqlite::Connection;
use schema::MIGRATIONS;
use search::Search;
use sessions::Sessions;
use std::collections::HashMap;
use std::path::Path;
use tool_calls::ToolCalls;

#[derive(Debug, Clone)]
pub struct WorkspaceDb {
    db: Db,
}

fn found<T>(row: Option<T>, id: &impl ToString) -> Result<T, StoreError> {
    row.ok_or_else(|| StoreError::NotFound(id.to_string()))
}

impl WorkspaceDb {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = open_file(path, MIGRATIONS)?;
        // The event log keeps every tool output, so a `git add -A` in the workspace root must
        // not publish it.
        if let Some(dir) = path.parent() {
            let ignore = dir.join(".gitignore");
            if !ignore.exists() {
                std::fs::write(
                    &ignore,
                    "# Ostra's local state: the event log with every tool output, sessions, and uploads.\n\
                     workspace.db*\nsessions/\nuploads/\n",
                )?;
            }
        }
        Ok(WorkspaceDb { db: Db::new(conn) })
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn, MIGRATIONS)?;
        Ok(WorkspaceDb { db: Db::new(conn) })
    }

    // -- meta ---------------------------------------------------------------------------------

    pub fn set_workspace_id(&self, id: &WorkspaceId) -> Result<(), StoreError> {
        self.db.run(|c| Meta(c).set_workspace_id(id))
    }

    pub fn workspace_id(&self) -> Result<WorkspaceId, StoreError> {
        self.db.run(|c| Meta(c).workspace_id())
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.db.run(|c| Meta(c).get(key))
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.db.run(|c| Meta(c).set(key, value))
    }

    // -- projects -----------------------------------------------------------------------------

    pub fn upsert_project(&self, row: &ProjectRow) -> Result<(), StoreError> {
        self.db.run(|c| Projects(c).upsert(row))
    }

    pub fn set_project_init_status(
        &self,
        key: &str,
        status: InitStatus,
    ) -> Result<bool, StoreError> {
        self.db.run(|c| Projects(c).set_init_status(key, status))
    }

    pub fn delete_project(&self, key: &str) -> Result<bool, StoreError> {
        self.db.run(|c| Projects(c).delete(key))
    }

    pub fn get_project(&self, key: &str) -> Result<Option<ProjectRow>, StoreError> {
        self.db.run(|c| Projects(c).get(key))
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>, StoreError> {
        self.db.run(|c| Projects(c).list())
    }

    // -- sessions -----------------------------------------------------------------------------

    pub fn create_session(&self, new: &NewSession) -> Result<SessionSummary, StoreError> {
        self.db.run(|c| {
            let sessions = Sessions(c);
            sessions.insert(new)?;
            found(sessions.get(&new.id)?, &new.id)
        })
    }

    pub fn update_session(
        &self,
        id: &SessionId,
        update: &SessionUpdate,
    ) -> Result<SessionSummary, StoreError> {
        self.db.transaction(|c| {
            let sessions = Sessions(c);
            sessions.update(id, update)?;
            found(sessions.get(id)?, id)
        })
    }

    pub fn get_session(&self, id: &SessionId) -> Result<Option<SessionSummary>, StoreError> {
        self.db.run(|c| Sessions(c).get(id))
    }

    /// Newest first.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>, StoreError> {
        self.db.run(|c| Sessions(c).list())
    }

    // -- events -------------------------------------------------------------------------------

    /// Append one event, assigning the next sequence number for the session.
    pub fn append_event(
        &self,
        session: &SessionId,
        event: &SessionEvent,
    ) -> Result<StoredEvent, StoreError> {
        self.db.transaction(|c| {
            let stored = Events(c).append(session, event)?;
            Sessions(c).touch(session, stored.at)?;
            Ok(stored)
        })
    }

    pub fn events(&self, session: &SessionId) -> Result<Vec<StoredEvent>, StoreError> {
        self.events_after(session, 0)
    }

    pub fn events_after(
        &self,
        session: &SessionId,
        after: i64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        self.db.run(|c| Events(c).after(session, after))
    }

    // -- executions ---------------------------------------------------------------------------

    pub fn insert_execution(&self, new: &NewExecution) -> Result<ExecutionView, StoreError> {
        self.db.run(|c| {
            let executions = Executions(c);
            executions.insert(new)?;
            found(executions.get(&new.id)?, &new.id)
        })
    }

    pub fn finish_execution(
        &self,
        id: &ExecutionId,
        result: &ExecutionResult,
    ) -> Result<ExecutionView, StoreError> {
        self.db.run(|c| {
            let executions = Executions(c);
            if !executions.finish(id, result)? {
                return Err(StoreError::NotFound(id.to_string()));
            }
            found(executions.get(id)?, id)
        })
    }

    /// Rule P2: run a paused execution again under its id. Its usage, transcript, and Activity
    /// stay, so the resumed run adds to them.
    pub fn reopen_execution(&self, id: &ExecutionId) -> Result<ExecutionView, StoreError> {
        self.db.run(|c| {
            let executions = Executions(c);
            if !executions.reopen(id)? {
                return Err(StoreError::NotFound(id.to_string()));
            }
            found(executions.get(id)?, id)
        })
    }

    /// Live usage while an execution runs.
    pub fn update_execution_usage(
        &self,
        id: &ExecutionId,
        usage: &Usage,
    ) -> Result<(), StoreError> {
        self.db.run(|c| Executions(c).set_usage(id, usage))
    }

    pub fn set_execution_status(
        &self,
        id: &ExecutionId,
        status: ExecutionStatus,
    ) -> Result<(), StoreError> {
        self.db.run(|c| Executions(c).set_status(id, status))
    }

    /// The one-line summary the view shows: the last tool call or status message.
    pub fn set_execution_summary(&self, id: &ExecutionId, summary: &str) -> Result<(), StoreError> {
        self.db.run(|c| Executions(c).set_summary(id, summary))
    }

    pub fn set_native_session_id(&self, id: &ExecutionId, native: &str) -> Result<(), StoreError> {
        self.db
            .run(|c| Executions(c).set_native_session_id(id, native))
    }

    pub fn get_execution(&self, id: &ExecutionId) -> Result<Option<ExecutionView>, StoreError> {
        self.db.run(|c| Executions(c).get(id))
    }

    /// The spawn params, submit payload, and final text of an execution.
    pub fn execution_output(
        &self,
        id: &ExecutionId,
    ) -> Result<Option<(serde_json::Value, ExecutionOutput)>, StoreError> {
        self.db.run(|c| Executions(c).output(id))
    }

    /// Oldest first.
    pub fn list_executions(&self, session: &SessionId) -> Result<Vec<ExecutionView>, StoreError> {
        self.db.run(|c| Executions(c).of_session(session))
    }

    /// Side-panel executions (no session), newest first.
    pub fn list_sessionless_executions(
        &self,
        limit: usize,
    ) -> Result<Vec<ExecutionView>, StoreError> {
        self.db.run(|c| Executions(c).sessionless(limit))
    }

    pub fn running_executions(&self) -> Result<Vec<ExecutionView>, StoreError> {
        self.db.run(|c| Executions(c).running())
    }

    /// After a restart: every `running` execution becomes `interrupted`. Returns them.
    pub fn mark_running_interrupted(&self) -> Result<Vec<ExecutionView>, StoreError> {
        self.db.transaction(|c| Executions(c).interrupt_running())
    }

    // -- messages -----------------------------------------------------------------------------

    pub fn append_message(
        &self,
        execution: &ExecutionId,
        role: &str,
        content: &serde_json::Value,
    ) -> Result<i64, StoreError> {
        self.db
            .transaction(|c| Messages(c).append(execution, role, content))
    }

    pub fn messages(&self, execution: &ExecutionId) -> Result<Vec<StoredMessage>, StoreError> {
        self.db.run(|c| Messages(c).list(execution))
    }

    // -- activity -----------------------------------------------------------------------------

    pub fn append_activity(
        &self,
        execution: &ExecutionId,
        delta: &ExecutionDelta,
    ) -> Result<ActivityItem, StoreError> {
        self.db
            .transaction(|c| Activity(c).append(execution, delta))
    }

    pub fn activity_after(
        &self,
        execution: &ExecutionId,
        after: i64,
    ) -> Result<Vec<ActivityItem>, StoreError> {
        self.db.run(|c| Activity(c).after(execution, after))
    }

    // -- gates --------------------------------------------------------------------------------

    /// Insert a gate, or refresh its title, explanation, and payload if it exists.
    pub fn upsert_gate(
        &self,
        session: &SessionId,
        id: &GateId,
        title: &str,
        explanation: &str,
        payload: &GatePayload,
    ) -> Result<GateView, StoreError> {
        self.db.run(|c| {
            let gates = Gates(c);
            gates.upsert(session, id, title, explanation, payload)?;
            found(gates.get(id)?, id)
        })
    }

    pub fn answer_gate(
        &self,
        id: &GateId,
        source: AnswerSource,
        answer: &GateAnswer,
        reason: Option<&str>,
    ) -> Result<GateView, StoreError> {
        self.db.run(|c| {
            let gates = Gates(c);
            if !gates.answer(id, source, answer, reason)? {
                return Err(StoreError::NotFound(id.to_string()));
            }
            found(gates.get(id)?, id)
        })
    }

    pub fn get_gate(&self, id: &GateId) -> Result<Option<GateView>, StoreError> {
        self.db.run(|c| Gates(c).get(id))
    }

    /// Every gate of a session, oldest first.
    pub fn gates(&self, session: &SessionId) -> Result<Vec<GateView>, StoreError> {
        self.db.run(|c| Gates(c).of_session(session))
    }

    /// Unanswered gates of one session, or of the whole workspace with `None`.
    pub fn open_gates(&self, session: Option<&SessionId>) -> Result<Vec<GateView>, StoreError> {
        self.db.run(|c| Gates(c).open(session))
    }

    // -- decisions ----------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn insert_decision(
        &self,
        session: &SessionId,
        id: &DecisionId,
        judge: JudgeKind,
        subject: Option<&str>,
        input_summary: &str,
        output: &serde_json::Value,
        reason: &str,
    ) -> Result<DecisionView, StoreError> {
        self.db.run(|c| {
            let decisions = Decisions(c);
            decisions.insert(session, id, judge, subject, input_summary, output, reason)?;
            found(decisions.get(id)?, id)
        })
    }

    /// Record a user override: the output is replaced and the row flagged.
    pub fn mark_overridden(
        &self,
        id: &DecisionId,
        output: &serde_json::Value,
        reason: &str,
    ) -> Result<DecisionView, StoreError> {
        self.db.run(|c| {
            let decisions = Decisions(c);
            if !decisions.override_with(id, output, reason)? {
                return Err(StoreError::NotFound(id.to_string()));
            }
            found(decisions.get(id)?, id)
        })
    }

    pub fn set_can_override(&self, id: &DecisionId, can: bool) -> Result<(), StoreError> {
        self.db.run(|c| Decisions(c).set_can_override(id, can))
    }

    pub fn get_decision(&self, id: &DecisionId) -> Result<Option<DecisionView>, StoreError> {
        self.db.run(|c| Decisions(c).get(id))
    }

    /// The session a decision belongs to.
    pub fn decision_session(&self, id: &DecisionId) -> Result<Option<SessionId>, StoreError> {
        self.db.run(|c| Decisions(c).session_of(id))
    }

    pub fn decisions(&self, session: &SessionId) -> Result<Vec<DecisionView>, StoreError> {
        self.db.run(|c| Decisions(c).of_session(session))
    }

    // -- tool calls ---------------------------------------------------------------------------

    pub fn record_tool_call(&self, rec: &ToolCallRecord) -> Result<(), StoreError> {
        self.db.run(|c| ToolCalls(c).record(rec))
    }

    pub fn tool_calls(&self, execution: &ExecutionId) -> Result<Vec<ToolCallRecord>, StoreError> {
        self.db.run(|c| ToolCalls(c).of_execution(execution))
    }

    // -- search -------------------------------------------------------------------------------

    /// Index an artifact's label and headings for search. `mtime` is the file's modification time
    /// in milliseconds, so a caller can skip files that did not change.
    pub fn index_artifact(
        &self,
        session: &SessionId,
        path: &str,
        label: &str,
        body: &str,
        mtime: i64,
    ) -> Result<(), StoreError> {
        self.db
            .run(|c| Search(c).index_artifact(session, path, label, body, mtime))
    }

    /// Indexed artifacts of a session: path to label and mtime.
    pub fn indexed_artifacts(
        &self,
        session: &SessionId,
    ) -> Result<HashMap<String, (String, i64)>, StoreError> {
        self.db.run(|c| Search(c).artifacts_of(session))
    }

    /// Sessions and artifacts whose title, request, label, or headings hold every word of `text`
    /// as a word prefix, best first.
    pub fn search_text(&self, text: &str, limit: usize) -> Result<Vec<TextHit>, StoreError> {
        let query = fts_prefix_query(text);
        if query.is_empty() {
            return Ok(vec![]);
        }
        self.db.run(|c| Search(c).matching(&query, limit))
    }

    // -- cost ---------------------------------------------------------------------------------

    /// Dollars executions spent that started at or after `since`.
    pub fn spend_since(&self, since: DateTime<Utc>) -> Result<f64, StoreError> {
        self.db.run(|c| Costs(c).since(since))
    }

    /// Spend grouped four ways, over executions that started at or after `since` (all when `None`).
    pub fn cost_report(&self, since: Option<DateTime<Utc>>) -> Result<CostReport, StoreError> {
        self.db.run(|c| Costs(c).report(since))
    }
}
