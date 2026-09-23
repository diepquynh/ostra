//! `<workspace>/.ostra/workspace.db`: the session event log (source of truth) and the tables the
//! engine materializes beside it for the UI.

use crate::StoreError;
use crate::util::{enum_from, enum_str, now, parse_time, parse_time_opt};
use chrono::{DateTime, Utc};
use ostra_core::agent::AgentName;
use ostra_core::api::{
    ActivityItem, CostReport, CostRow, DecisionView, ExecutionView, GateView, InitStatus, SessionStatus,
    SessionSummary, execution_group,
};
use ostra_core::event::{AnswerSource, ExecPurpose, GateAnswer, GatePayload, JudgeKind, SessionEvent, SessionKind, StoredEvent};
use ostra_core::exec::{ExecutionDelta, ExecutionResult, ExecutionStatus, Usage};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
use ostra_core::pipeline::{Category, Lane, StageKind};
use ostra_core::policy::PolicyDecision;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE projects (
  key TEXT PRIMARY KEY,
  path TEXT NOT NULL,
  init_status TEXT NOT NULL,
  stack TEXT
);
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  request TEXT NOT NULL,
  category TEXT,
  status TEXT NOT NULL,
  lane TEXT NOT NULL,
  stage_label TEXT NOT NULL,
  projects TEXT NOT NULL,
  yolo INTEGER NOT NULL,
  cost_usd REAL NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE events (
  session_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  type TEXT NOT NULL,
  payload TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (session_id, seq)
);
CREATE TABLE executions (
  id TEXT PRIMARY KEY,
  session_id TEXT,
  agent TEXT NOT NULL,
  purpose TEXT,
  stage TEXT,
  project TEXT NOT NULL,
  executor TEXT NOT NULL,
  model TEXT NOT NULL,
  params TEXT NOT NULL,
  spawn_block TEXT NOT NULL,
  report_path TEXT,
  status TEXT NOT NULL,
  native_session_id TEXT,
  usage TEXT NOT NULL,
  cost_usd REAL NOT NULL DEFAULT 0,
  error TEXT,
  submit TEXT,
  final_text TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT
);
CREATE INDEX executions_session ON executions(session_id);
CREATE INDEX executions_status ON executions(status);
CREATE TABLE messages (
  execution_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  PRIMARY KEY (execution_id, seq)
);
CREATE TABLE activity (
  execution_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  delta TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY (execution_id, seq)
);
CREATE TABLE gates (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  title TEXT NOT NULL,
  explanation TEXT NOT NULL,
  payload TEXT NOT NULL,
  answer TEXT,
  source TEXT,
  reason TEXT,
  opened_at TEXT NOT NULL,
  answered_at TEXT
);
CREATE INDEX gates_session ON gates(session_id);
CREATE TABLE decisions (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  judge TEXT NOT NULL,
  subject TEXT,
  input_summary TEXT NOT NULL,
  output TEXT NOT NULL,
  reason TEXT NOT NULL,
  overridden INTEGER NOT NULL DEFAULT 0,
  can_override INTEGER NOT NULL DEFAULT 1,
  at TEXT NOT NULL
);
CREATE INDEX decisions_session ON decisions(session_id);
CREATE TABLE tool_calls (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  execution_id TEXT NOT NULL,
  call_id TEXT NOT NULL,
  tool TEXT NOT NULL,
  input TEXT NOT NULL,
  decision TEXT,
  rule TEXT,
  duration_ms INTEGER,
  output TEXT,
  at TEXT NOT NULL
);
CREATE INDEX tool_calls_execution ON tool_calls(execution_id);
"#, r#"
ALTER TABLE sessions ADD COLUMN title TEXT;
ALTER TABLE executions ADD COLUMN summary TEXT;
"#, r#"
CREATE TABLE search_docs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,
  ref TEXT NOT NULL,
  session_id TEXT NOT NULL,
  label TEXT NOT NULL,
  body TEXT NOT NULL,
  mtime INTEGER NOT NULL DEFAULT 0,
  UNIQUE(kind, ref)
);
CREATE VIRTUAL TABLE search_fts USING fts5(
  label, body, content='search_docs', content_rowid='id', tokenize='unicode61 remove_diacritics 2', prefix='2 3'
);
CREATE TRIGGER search_docs_ai AFTER INSERT ON search_docs BEGIN
  INSERT INTO search_fts(rowid, label, body) VALUES (new.id, new.label, new.body);
END;
CREATE TRIGGER search_docs_ad AFTER DELETE ON search_docs BEGIN
  INSERT INTO search_fts(search_fts, rowid, label, body) VALUES ('delete', old.id, old.label, old.body);
END;
CREATE TRIGGER search_docs_au AFTER UPDATE ON search_docs BEGIN
  INSERT INTO search_fts(search_fts, rowid, label, body) VALUES ('delete', old.id, old.label, old.body);
  INSERT INTO search_fts(rowid, label, body) VALUES (new.id, new.label, new.body);
END;
CREATE TRIGGER sessions_search_ai AFTER INSERT ON sessions BEGIN
  INSERT INTO search_docs (kind, ref, session_id, label, body) VALUES ('session', new.id, new.id, COALESCE(new.title, ''), new.request);
END;
CREATE TRIGGER sessions_search_au AFTER UPDATE OF title, request ON sessions
  WHEN old.title IS NOT new.title OR old.request IS NOT new.request BEGIN
  UPDATE search_docs SET label = COALESCE(new.title, ''), body = new.request WHERE kind = 'session' AND ref = new.id;
END;
INSERT INTO search_docs (kind, ref, session_id, label, body)
  SELECT 'session', id, id, COALESCE(title, ''), request FROM sessions;
"#];

/// One text-search match: a session (`ref` is its id) or an artifact (`ref` is its path).
#[derive(Debug, Clone, PartialEq)]
pub struct TextHit {
    pub kind: String,
    pub reference: String,
    pub session: SessionId,
    pub label: String,
    pub body: String,
    /// FTS5 bm25 rank; lower is better.
    pub rank: f64,
}

/// A prefix query over every word of `text`, all words required: `"cance"* "ord"*`.
pub fn fts_prefix_query(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Tool call output kept in the table is truncated to this many bytes.
pub const TOOL_OUTPUT_LIMIT: usize = 8 * 1024;

pub(crate) fn open_connection(path: &Path, migrations: &[&str]) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn, migrations)?;
    Ok(conn)
}

fn migrate(conn: &Connection, migrations: &[&str]) -> Result<(), StoreError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in migrations.iter().enumerate().skip(current.max(0) as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct WorkspaceDb {
    conn: Arc<Mutex<Connection>>,
}

/// A new session row.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub id: SessionId,
    pub kind: SessionKind,
    pub request: String,
    pub category: Option<Category>,
    pub projects: Vec<String>,
    pub yolo: bool,
}

/// Fields the engine updates on a session. `None` leaves a field unchanged.
#[derive(Debug, Clone, Default)]
pub struct SessionUpdate {
    pub request: Option<String>,
    pub category: Option<Option<Category>>,
    pub status: Option<SessionStatus>,
    pub lane: Option<Lane>,
    pub stage_label: Option<String>,
    pub projects: Option<Vec<String>>,
    pub yolo: Option<bool>,
    pub cost_usd: Option<f64>,
    pub title: Option<Option<String>>,
}

#[derive(Debug, Clone)]
pub struct NewExecution {
    pub id: ExecutionId,
    pub session: Option<SessionId>,
    pub agent: AgentName,
    pub purpose: Option<ExecPurpose>,
    pub stage: Option<StageKind>,
    pub project: String,
    pub executor: ExecutorKind,
    pub model: String,
    pub params: serde_json::Value,
    pub spawn_block: String,
    pub report_path: Option<PathBuf>,
    pub native_session_id: Option<String>,
}

/// The stored result fields of a finished execution that the view does not carry.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionOutput {
    pub submit: Option<serde_json::Value>,
    pub final_text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredMessage {
    pub seq: i64,
    pub role: String,
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRow {
    pub key: String,
    pub path: PathBuf,
    pub init_status: InitStatus,
    pub stack: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallRecord {
    pub execution: ExecutionId,
    pub call_id: String,
    pub tool: String,
    pub input: serde_json::Value,
    pub decision: Option<PolicyDecision>,
    pub rule: Option<String>,
    pub duration_ms: Option<u64>,
    pub output: Option<String>,
    pub at: Option<DateTime<Utc>>,
}

fn truncate_utf8(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let mut end = limit;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated: {} bytes]", &s[..end], s.len())
}

const SESSION_COLS: &str =
    "id, kind, request, category, status, lane, stage_label, projects, yolo, cost_usd, created_at, updated_at, title";

const EXECUTION_COLS: &str = "id, session_id, agent, purpose, stage, project, executor, model, status, started_at, \
     ended_at, usage, report_path, native_session_id, spawn_block, error, summary";

const GATE_COLS: &str = "id, session_id, title, explanation, payload, answer, source, reason, opened_at, answered_at";

const DECISION_COLS: &str = "id, judge, subject, input_summary, output, reason, overridden, can_override, at";

impl WorkspaceDb {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = open_connection(path, MIGRATIONS)?;
        Ok(WorkspaceDb { conn: Arc::new(Mutex::new(conn)) })
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn, MIGRATIONS)?;
        Ok(WorkspaceDb { conn: Arc::new(Mutex::new(conn)) })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // -- meta ---------------------------------------------------------------------------------

    pub fn set_workspace_id(&self, id: &WorkspaceId) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO meta (key, value) VALUES ('workspace_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![id.as_str()],
        )?;
        Ok(())
    }

    pub fn workspace_id(&self) -> Result<WorkspaceId, StoreError> {
        let conn = self.lock();
        workspace_id_of(&conn)
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self.lock().query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // -- projects -----------------------------------------------------------------------------

    pub fn upsert_project(&self, row: &ProjectRow) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO projects (key, path, init_status, stack) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET path = excluded.path, init_status = excluded.init_status,
             stack = excluded.stack",
            params![row.key, row.path.to_string_lossy(), enum_str(&row.init_status)?, row.stack],
        )?;
        Ok(())
    }

    pub fn set_project_init_status(&self, key: &str, status: InitStatus) -> Result<bool, StoreError> {
        let n = self
            .lock()
            .execute("UPDATE projects SET init_status = ?1 WHERE key = ?2", params![enum_str(&status)?, key])?;
        Ok(n > 0)
    }

    pub fn delete_project(&self, key: &str) -> Result<bool, StoreError> {
        Ok(self.lock().execute("DELETE FROM projects WHERE key = ?1", params![key])? > 0)
    }

    pub fn get_project(&self, key: &str) -> Result<Option<ProjectRow>, StoreError> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT key, path, init_status, stack FROM projects WHERE key = ?1",
                params![key],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get(3)?)),
            )
            .optional()?;
        row.map(|(key, path, status, stack)| {
            Ok(ProjectRow { key, path: PathBuf::from(path), init_status: enum_from(&status)?, stack })
        })
        .transpose()
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectRow>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare("SELECT key, path, init_status, stack FROM projects ORDER BY key")?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get(3)?)))?
            .collect::<Result<Vec<(String, String, String, Option<String>)>, _>>()?;
        rows.into_iter()
            .map(|(key, path, status, stack)| {
                Ok(ProjectRow { key, path: PathBuf::from(path), init_status: enum_from(&status)?, stack })
            })
            .collect()
    }

    // -- sessions -----------------------------------------------------------------------------

    pub fn create_session(&self, new: &NewSession) -> Result<SessionSummary, StoreError> {
        let at = now();
        {
            let conn = self.lock();
            conn.execute(
                &format!("INSERT INTO sessions ({SESSION_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?10, NULL)"),
                params![
                    new.id.as_str(),
                    serde_json::to_string(&new.kind)?,
                    new.request,
                    new.category.map(|c| enum_str(&c)).transpose()?,
                    enum_str(&SessionStatus::Running)?,
                    enum_str(&Lane::Research)?,
                    "Intake",
                    serde_json::to_string(&new.projects)?,
                    new.yolo,
                    at,
                ],
            )?;
        }
        self.get_session(&new.id)?.ok_or_else(|| StoreError::NotFound(new.id.to_string()))
    }

    pub fn update_session(&self, id: &SessionId, update: &SessionUpdate) -> Result<SessionSummary, StoreError> {
        {
            let conn = self.lock();
            let tx = conn.unchecked_transaction()?;
            let exists: Option<i64> =
                tx.query_row("SELECT 1 FROM sessions WHERE id = ?1", params![id.as_str()], |r| r.get(0)).optional()?;
            if exists.is_none() {
                return Err(StoreError::NotFound(id.to_string()));
            }
            let set = |col: &str, value: &dyn rusqlite::ToSql| -> Result<(), StoreError> {
                tx.execute(&format!("UPDATE sessions SET {col} = ?1 WHERE id = ?2"), params![value, id.as_str()])?;
                Ok(())
            };
            if let Some(v) = &update.request {
                set("request", v)?;
            }
            if let Some(v) = &update.category {
                set("category", &v.map(|c| enum_str(&c)).transpose()?)?;
            }
            if let Some(v) = &update.status {
                set("status", &enum_str(v)?)?;
            }
            if let Some(v) = &update.lane {
                set("lane", &enum_str(v)?)?;
            }
            if let Some(v) = &update.stage_label {
                set("stage_label", v)?;
            }
            if let Some(v) = &update.projects {
                set("projects", &serde_json::to_string(v)?)?;
            }
            if let Some(v) = &update.yolo {
                set("yolo", v)?;
            }
            if let Some(v) = &update.cost_usd {
                set("cost_usd", v)?;
            }
            if let Some(v) = &update.title {
                set("title", v)?;
            }
            set("updated_at", &now())?;
            tx.commit()?;
        }
        self.get_session(id)?.ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub fn get_session(&self, id: &SessionId) -> Result<Option<SessionSummary>, StoreError> {
        let conn = self.lock();
        let ws = workspace_id_of(&conn)?;
        let raw = conn
            .query_row(&format!("SELECT {SESSION_COLS} FROM sessions WHERE id = ?1"), params![id.as_str()], raw_session)
            .optional()?;
        raw.map(|r| session_from_raw(&conn, &ws, r)).transpose()
    }

    /// Newest first.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>, StoreError> {
        let conn = self.lock();
        let ws = workspace_id_of(&conn)?;
        let mut st = conn.prepare(&format!("SELECT {SESSION_COLS} FROM sessions ORDER BY created_at DESC, id DESC"))?;
        let raws = st.query_map([], raw_session)?.collect::<Result<Vec<_>, _>>()?;
        raws.into_iter().map(|r| session_from_raw(&conn, &ws, r)).collect()
    }

    // -- events -------------------------------------------------------------------------------

    /// Append one event, assigning the next sequence number for the session.
    pub fn append_event(&self, session: &SessionId, event: &SessionEvent) -> Result<StoredEvent, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
            params![session.as_str()],
            |r| r.get(0),
        )?;
        let payload = serde_json::to_value(event)?;
        let kind = payload.get("type").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let at = Utc::now();
        let at_text = at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        tx.execute(
            "INSERT INTO events (session_id, seq, type, payload, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session.as_str(), seq, kind, payload.to_string(), at_text],
        )?;
        tx.execute("UPDATE sessions SET updated_at = ?1 WHERE id = ?2", params![at_text, session.as_str()])?;
        tx.commit()?;
        Ok(StoredEvent { seq, at: parse_time(&at_text)?, event: event.clone() })
    }

    pub fn events(&self, session: &SessionId) -> Result<Vec<StoredEvent>, StoreError> {
        self.events_after(session, 0)
    }

    pub fn events_after(&self, session: &SessionId, after: i64) -> Result<Vec<StoredEvent>, StoreError> {
        let conn = self.lock();
        let mut st =
            conn.prepare("SELECT seq, at, payload FROM events WHERE session_id = ?1 AND seq > ?2 ORDER BY seq")?;
        let rows = st
            .query_map(params![session.as_str(), after], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(seq, at, payload)| Ok(StoredEvent { seq, at: parse_time(&at)?, event: serde_json::from_str(&payload)? }))
            .collect()
    }

    // -- executions ---------------------------------------------------------------------------

    pub fn insert_execution(&self, new: &NewExecution) -> Result<ExecutionView, StoreError> {
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO executions (id, session_id, agent, purpose, stage, project, executor, model, params,
                 spawn_block, report_path, status, native_session_id, usage, cost_usd, started_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0, ?15)",
                params![
                    new.id.as_str(),
                    new.session.as_ref().map(|s| s.as_str().to_string()),
                    new.agent.as_str(),
                    new.purpose.as_ref().map(serde_json::to_string).transpose()?,
                    new.stage.map(|s| enum_str(&s)).transpose()?,
                    new.project,
                    new.executor.to_string(),
                    new.model,
                    new.params.to_string(),
                    new.spawn_block,
                    new.report_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                    enum_str(&ExecutionStatus::Running)?,
                    new.native_session_id,
                    serde_json::to_string(&Usage::default())?,
                    now(),
                ],
            )?;
        }
        self.get_execution(&new.id)?.ok_or_else(|| StoreError::NotFound(new.id.to_string()))
    }

    pub fn finish_execution(&self, id: &ExecutionId, result: &ExecutionResult) -> Result<ExecutionView, StoreError> {
        let n = self.lock().execute(
            "UPDATE executions SET status = ?1, usage = ?2, cost_usd = ?3, error = ?4, submit = ?5, final_text = ?6,
             native_session_id = COALESCE(?7, native_session_id), ended_at = ?8 WHERE id = ?9",
            params![
                enum_str(&result.status)?,
                serde_json::to_string(&result.usage)?,
                result.usage.cost_usd,
                result.error,
                result.submit.as_ref().map(|v| v.to_string()),
                result.final_text,
                result.native_session_id,
                now(),
                id.as_str(),
            ],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.get_execution(id)?.ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    /// Live usage while an execution runs.
    pub fn update_execution_usage(&self, id: &ExecutionId, usage: &Usage) -> Result<(), StoreError> {
        self.lock().execute(
            "UPDATE executions SET usage = ?1, cost_usd = ?2 WHERE id = ?3",
            params![serde_json::to_string(usage)?, usage.cost_usd, id.as_str()],
        )?;
        Ok(())
    }

    pub fn set_execution_status(&self, id: &ExecutionId, status: ExecutionStatus) -> Result<(), StoreError> {
        let ended = if status.is_terminal() { Some(now()) } else { None };
        self.lock().execute(
            "UPDATE executions SET status = ?1, ended_at = COALESCE(?2, ended_at) WHERE id = ?3",
            params![enum_str(&status)?, ended, id.as_str()],
        )?;
        Ok(())
    }

    /// The one-line summary the view shows: the last tool call or status message.
    pub fn set_execution_summary(&self, id: &ExecutionId, summary: &str) -> Result<(), StoreError> {
        self.lock().execute("UPDATE executions SET summary = ?1 WHERE id = ?2", params![summary, id.as_str()])?;
        Ok(())
    }

    pub fn set_native_session_id(&self, id: &ExecutionId, native: &str) -> Result<(), StoreError> {
        self.lock()
            .execute("UPDATE executions SET native_session_id = ?1 WHERE id = ?2", params![native, id.as_str()])?;
        Ok(())
    }

    pub fn get_execution(&self, id: &ExecutionId) -> Result<Option<ExecutionView>, StoreError> {
        let conn = self.lock();
        let raw = conn
            .query_row(&format!("SELECT {EXECUTION_COLS} FROM executions WHERE id = ?1"), params![id.as_str()], raw_execution)
            .optional()?;
        raw.map(execution_from_raw).transpose()
    }

    /// The spawn params, submit payload, and final text of an execution.
    pub fn execution_output(&self, id: &ExecutionId) -> Result<Option<(serde_json::Value, ExecutionOutput)>, StoreError> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT params, submit, final_text FROM executions WHERE id = ?1",
                params![id.as_str()],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)),
            )
            .optional()?;
        row.map(|(p, s, f)| {
            Ok((
                serde_json::from_str(&p)?,
                ExecutionOutput {
                    submit: s.map(|s| serde_json::from_str(&s)).transpose()?,
                    final_text: f.unwrap_or_default(),
                },
            ))
        })
        .transpose()
    }

    /// Oldest first.
    pub fn list_executions(&self, session: &SessionId) -> Result<Vec<ExecutionView>, StoreError> {
        self.query_executions(
            &format!("SELECT {EXECUTION_COLS} FROM executions WHERE session_id = ?1 ORDER BY started_at, id"),
            params![session.as_str()],
        )
    }

    /// Side-panel executions (no session), newest first.
    pub fn list_sessionless_executions(&self, limit: usize) -> Result<Vec<ExecutionView>, StoreError> {
        self.query_executions(
            &format!("SELECT {EXECUTION_COLS} FROM executions WHERE session_id IS NULL ORDER BY started_at DESC, id DESC LIMIT ?1"),
            params![limit as i64],
        )
    }

    pub fn running_executions(&self) -> Result<Vec<ExecutionView>, StoreError> {
        self.query_executions(
            &format!("SELECT {EXECUTION_COLS} FROM executions WHERE status = ?1 ORDER BY started_at, id"),
            params![enum_str(&ExecutionStatus::Running)?],
        )
    }

    /// After a restart: every `running` execution becomes `interrupted`. Returns them.
    pub fn mark_running_interrupted(&self) -> Result<Vec<ExecutionView>, StoreError> {
        let running = self.running_executions()?;
        for e in &running {
            self.set_execution_status(&e.id, ExecutionStatus::Interrupted)?;
        }
        running.into_iter().map(|e| Ok(ExecutionView { status: ExecutionStatus::Interrupted, ..e })).collect()
    }

    fn query_executions(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<ExecutionView>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare(sql)?;
        let raws = st.query_map(p, raw_execution)?.collect::<Result<Vec<_>, _>>()?;
        raws.into_iter().map(execution_from_raw).collect()
    }

    // -- messages -----------------------------------------------------------------------------

    pub fn append_message(&self, execution: &ExecutionId, role: &str, content: &serde_json::Value) -> Result<i64, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE execution_id = ?1",
            params![execution.as_str()],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO messages (execution_id, seq, role, content) VALUES (?1, ?2, ?3, ?4)",
            params![execution.as_str(), seq, role, content.to_string()],
        )?;
        tx.commit()?;
        Ok(seq)
    }

    pub fn messages(&self, execution: &ExecutionId) -> Result<Vec<StoredMessage>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare("SELECT seq, role, content FROM messages WHERE execution_id = ?1 ORDER BY seq")?;
        let rows = st
            .query_map(params![execution.as_str()], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(seq, role, content)| Ok(StoredMessage { seq, role, content: serde_json::from_str(&content)? }))
            .collect()
    }

    // -- activity -----------------------------------------------------------------------------

    pub fn append_activity(&self, execution: &ExecutionId, delta: &ExecutionDelta) -> Result<ActivityItem, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM activity WHERE execution_id = ?1",
            params![execution.as_str()],
            |r| r.get(0),
        )?;
        let at = now();
        tx.execute(
            "INSERT INTO activity (execution_id, seq, delta, at) VALUES (?1, ?2, ?3, ?4)",
            params![execution.as_str(), seq, serde_json::to_string(delta)?, at],
        )?;
        tx.commit()?;
        Ok(ActivityItem { seq, at: parse_time(&at)?, delta: delta.clone() })
    }

    pub fn activity_after(&self, execution: &ExecutionId, after: i64) -> Result<Vec<ActivityItem>, StoreError> {
        let conn = self.lock();
        let mut st =
            conn.prepare("SELECT seq, at, delta FROM activity WHERE execution_id = ?1 AND seq > ?2 ORDER BY seq")?;
        let rows = st
            .query_map(params![execution.as_str(), after], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(seq, at, delta)| Ok(ActivityItem { seq, at: parse_time(&at)?, delta: serde_json::from_str(&delta)? }))
            .collect()
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
        self.lock().execute(
            "INSERT INTO gates (id, session_id, title, explanation, payload, opened_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, explanation = excluded.explanation,
             payload = excluded.payload",
            params![id.as_str(), session.as_str(), title, explanation, serde_json::to_string(payload)?, now()],
        )?;
        self.get_gate(id)?.ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub fn answer_gate(
        &self,
        id: &GateId,
        source: AnswerSource,
        answer: &GateAnswer,
        reason: Option<&str>,
    ) -> Result<GateView, StoreError> {
        let n = self.lock().execute(
            "UPDATE gates SET answer = ?1, source = ?2, reason = ?3, answered_at = ?4 WHERE id = ?5",
            params![serde_json::to_string(answer)?, enum_str(&source)?, reason, now(), id.as_str()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.get_gate(id)?.ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub fn get_gate(&self, id: &GateId) -> Result<Option<GateView>, StoreError> {
        let conn = self.lock();
        let raw = conn
            .query_row(&format!("SELECT {GATE_COLS} FROM gates WHERE id = ?1"), params![id.as_str()], raw_gate)
            .optional()?;
        raw.map(gate_from_raw).transpose()
    }

    /// Every gate of a session, oldest first.
    pub fn gates(&self, session: &SessionId) -> Result<Vec<GateView>, StoreError> {
        self.query_gates(
            &format!("SELECT {GATE_COLS} FROM gates WHERE session_id = ?1 ORDER BY opened_at, id"),
            params![session.as_str()],
        )
    }

    /// Unanswered gates of one session, or of the whole workspace with `None`.
    pub fn open_gates(&self, session: Option<&SessionId>) -> Result<Vec<GateView>, StoreError> {
        match session {
            Some(s) => self.query_gates(
                &format!("SELECT {GATE_COLS} FROM gates WHERE session_id = ?1 AND answer IS NULL ORDER BY opened_at, id"),
                params![s.as_str()],
            ),
            None => self.query_gates(
                &format!("SELECT {GATE_COLS} FROM gates WHERE answer IS NULL ORDER BY opened_at, id"),
                [],
            ),
        }
    }

    fn query_gates(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<GateView>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare(sql)?;
        let raws = st.query_map(p, raw_gate)?.collect::<Result<Vec<_>, _>>()?;
        raws.into_iter().map(gate_from_raw).collect()
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
        self.lock().execute(
            "INSERT INTO decisions (id, session_id, judge, subject, input_summary, output, reason, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id.as_str(), session.as_str(), enum_str(&judge)?, subject, input_summary, output.to_string(), reason, now()],
        )?;
        self.get_decision(id)?.ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    /// Record a user override: the output is replaced and the row flagged.
    pub fn mark_overridden(&self, id: &DecisionId, output: &serde_json::Value, reason: &str) -> Result<DecisionView, StoreError> {
        let n = self.lock().execute(
            "UPDATE decisions SET overridden = 1, output = ?1, reason = ?2 WHERE id = ?3",
            params![output.to_string(), reason, id.as_str()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.get_decision(id)?.ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub fn set_can_override(&self, id: &DecisionId, can: bool) -> Result<(), StoreError> {
        self.lock().execute("UPDATE decisions SET can_override = ?1 WHERE id = ?2", params![can, id.as_str()])?;
        Ok(())
    }

    pub fn get_decision(&self, id: &DecisionId) -> Result<Option<DecisionView>, StoreError> {
        let conn = self.lock();
        let raw = conn
            .query_row(&format!("SELECT {DECISION_COLS} FROM decisions WHERE id = ?1"), params![id.as_str()], raw_decision)
            .optional()?;
        raw.map(decision_from_raw).transpose()
    }

    /// The session a decision belongs to.
    pub fn decision_session(&self, id: &DecisionId) -> Result<Option<SessionId>, StoreError> {
        Ok(self
            .lock()
            .query_row("SELECT session_id FROM decisions WHERE id = ?1", params![id.as_str()], |r| r.get::<_, String>(0))
            .optional()?
            .map(SessionId))
    }

    pub fn decisions(&self, session: &SessionId) -> Result<Vec<DecisionView>, StoreError> {
        let conn = self.lock();
        let mut st =
            conn.prepare(&format!("SELECT {DECISION_COLS} FROM decisions WHERE session_id = ?1 ORDER BY at, id"))?;
        let raws = st.query_map(params![session.as_str()], raw_decision)?.collect::<Result<Vec<_>, _>>()?;
        raws.into_iter().map(decision_from_raw).collect()
    }

    // -- tool calls ---------------------------------------------------------------------------

    pub fn record_tool_call(&self, rec: &ToolCallRecord) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO tool_calls (execution_id, call_id, tool, input, decision, rule, duration_ms, output, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                rec.execution.as_str(),
                rec.call_id,
                rec.tool,
                rec.input.to_string(),
                rec.decision.as_ref().map(serde_json::to_string).transpose()?,
                rec.rule,
                rec.duration_ms.map(|d| d as i64),
                rec.output.as_deref().map(|o| truncate_utf8(o, TOOL_OUTPUT_LIMIT)),
                rec.at.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)).unwrap_or_else(now),
            ],
        )?;
        Ok(())
    }

    pub fn tool_calls(&self, execution: &ExecutionId) -> Result<Vec<ToolCallRecord>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare(
            "SELECT execution_id, call_id, tool, input, decision, rule, duration_ms, output, at
             FROM tool_calls WHERE execution_id = ?1 ORDER BY id",
        )?;
        #[allow(clippy::type_complexity)]
        let rows: Vec<(String, String, String, String, Option<String>, Option<String>, Option<i64>, Option<String>, String)> = st
            .query_map(params![execution.as_str()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
            })?
            .collect::<Result<_, _>>()?;
        rows.into_iter()
            .map(|(e, call_id, tool, input, decision, rule, duration, output, at)| {
                Ok(ToolCallRecord {
                    execution: ExecutionId(e),
                    call_id,
                    tool,
                    input: serde_json::from_str(&input)?,
                    decision: decision.map(|d| serde_json::from_str(&d)).transpose()?,
                    rule,
                    duration_ms: duration.map(|d| d as u64),
                    output,
                    at: Some(parse_time(&at)?),
                })
            })
            .collect()
    }

    // -- search -------------------------------------------------------------------------------

    /// Index an artifact's label and headings for search. `mtime` is the file's modification time
    /// in milliseconds, so a caller can skip files that did not change.
    pub fn index_artifact(&self, session: &SessionId, path: &str, label: &str, body: &str, mtime: i64) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO search_docs (kind, ref, session_id, label, body, mtime) VALUES ('artifact', ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(kind, ref) DO UPDATE SET session_id = excluded.session_id, label = excluded.label,
             body = excluded.body, mtime = excluded.mtime",
            params![path, session.as_str(), label, body, mtime],
        )?;
        Ok(())
    }

    /// Indexed artifacts of a session: path to label and mtime.
    pub fn indexed_artifacts(&self, session: &SessionId) -> Result<HashMap<String, (String, i64)>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare("SELECT ref, label, mtime FROM search_docs WHERE kind = 'artifact' AND session_id = ?1")?;
        let rows = st
            .query_map(params![session.as_str()], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, r.get::<_, i64>(2)?))))?
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(rows)
    }

    /// Sessions and artifacts whose title, request, label, or headings hold every word of `text`
    /// as a word prefix, best first.
    pub fn search_text(&self, text: &str, limit: usize) -> Result<Vec<TextHit>, StoreError> {
        let query = fts_prefix_query(text);
        if query.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.lock();
        let mut st = conn.prepare(
            "SELECT d.kind, d.ref, d.session_id, d.label, d.body, bm25(search_fts, 4.0, 1.0) AS r
             FROM search_fts JOIN search_docs d ON d.id = search_fts.rowid
             WHERE search_fts MATCH ?1 ORDER BY r LIMIT ?2",
        )?;
        let rows = st
            .query_map(params![query, limit as i64], |r| {
                Ok(TextHit {
                    kind: r.get(0)?,
                    reference: r.get(1)?,
                    session: SessionId(r.get(2)?),
                    label: r.get(3)?,
                    body: r.get(4)?,
                    rank: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Dollars executions spent that started at or after `since`.
    pub fn spend_since(&self, since: DateTime<Utc>) -> Result<f64, StoreError> {
        let at = since.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        Ok(self.lock().query_row(
            "SELECT COALESCE(SUM(cost_usd), 0) FROM executions WHERE started_at >= ?1",
            params![at],
            |r| r.get(0),
        )?)
    }

    // -- cost ---------------------------------------------------------------------------------

    /// Spend grouped four ways, over executions that started at or after `since` (all when `None`).
    pub fn cost_report(&self, since: Option<DateTime<Utc>>) -> Result<CostReport, StoreError> {
        let conn = self.lock();
        let at = since.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)).unwrap_or_default();
        let mut st = conn.prepare("SELECT session_id, stage, agent, executor, usage FROM executions WHERE started_at >= ?1")?;
        let rows = st
            .query_map(params![at], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut by_session: BTreeMap<String, (u32, Usage)> = BTreeMap::new();
        let mut by_stage: BTreeMap<String, (u32, Usage)> = BTreeMap::new();
        let mut by_agent: BTreeMap<String, (u32, Usage)> = BTreeMap::new();
        let mut by_executor: BTreeMap<String, (u32, Usage)> = BTreeMap::new();
        let mut total = (0u32, Usage::default());
        for (session, stage, agent, executor, usage) in rows {
            let usage: Usage = serde_json::from_str(&usage)?;
            let keys = [
                (&mut by_session, session.unwrap_or_else(|| "side-panel".into())),
                (&mut by_stage, stage.unwrap_or_else(|| "none".into())),
                (&mut by_agent, agent),
                (&mut by_executor, executor),
            ];
            for (map, key) in keys {
                let entry = map.entry(key).or_default();
                entry.0 += 1;
                entry.1.add(&usage);
            }
            total.0 += 1;
            total.1.add(&usage);
        }
        let rows_of = |m: BTreeMap<String, (u32, Usage)>| -> Vec<CostRow> {
            let mut v: Vec<CostRow> = m.into_iter().map(|(k, (n, u))| cost_row(k, n, u)).collect();
            v.sort_by(|a, b| b.usage.cost_usd.total_cmp(&a.usage.cost_usd).then(a.key.cmp(&b.key)));
            v
        };
        Ok(CostReport {
            since,
            by_session: rows_of(by_session),
            by_stage: rows_of(by_stage),
            by_agent: rows_of(by_agent),
            by_executor: rows_of(by_executor),
            total: cost_row("total".into(), total.0, total.1),
        })
    }
}

fn cost_row(key: String, executions: u32, usage: Usage) -> CostRow {
    let per_call = if usage.tool_calls == 0 { 0.0 } else { usage.cache_read_tokens as f64 / usage.tool_calls as f64 };
    CostRow { key, executions, usage, cache_reads_per_tool_call: per_call }
}

fn workspace_id_of(conn: &Connection) -> Result<WorkspaceId, StoreError> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM meta WHERE key = 'workspace_id'", [], |r| r.get(0)).optional()?;
    Ok(WorkspaceId(v.unwrap_or_default()))
}

type RawSession =
    (String, String, String, Option<String>, String, String, String, String, bool, f64, String, String, Option<String>);

fn raw_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawSession> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
    ))
}

fn session_from_raw(conn: &Connection, ws: &WorkspaceId, r: RawSession) -> Result<SessionSummary, StoreError> {
    let (id, kind, request, category, status, lane, stage_label, projects, yolo, cost_usd, created, updated, title) = r;
    let open_gates: i64 = conn.query_row(
        "SELECT count(*) FROM gates WHERE session_id = ?1 AND answer IS NULL",
        params![id],
        |r| r.get(0),
    )?;
    Ok(SessionSummary {
        id: SessionId(id),
        workspace: ws.clone(),
        kind: serde_json::from_str(&kind)?,
        request,
        category: category.map(|c| enum_from(&c)).transpose()?,
        status: enum_from(&status)?,
        lane: enum_from(&lane)?,
        stage_label,
        yolo,
        open_gates: open_gates as u32,
        projects: serde_json::from_str(&projects)?,
        cost_usd,
        created_at: parse_time(&created)?,
        updated_at: parse_time(&updated)?,
        title,
    })
}

type RawExecution = (
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

fn raw_execution(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawExecution> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
        r.get(14)?,
        r.get(15)?,
        r.get(16)?,
    ))
}

fn execution_from_raw(r: RawExecution) -> Result<ExecutionView, StoreError> {
    let (id, session, agent, purpose, stage, project, executor, model, status, started, ended, usage, report, native, spawn, error, summary) =
        r;
    let executor: ExecutorKind = executor.parse().map_err(StoreError::Invalid)?;
    let can_resume = matches!(executor, ExecutorKind::Harness(_)) && native.is_some();
    let agent: AgentName = agent.parse().map_err(StoreError::Invalid)?;
    let purpose: Option<ExecPurpose> = purpose.map(|p| serde_json::from_str(&p)).transpose()?;
    // The engine numbers repeated runs from the fold; this is the label of a first run.
    let run_label = purpose.as_ref().map(ExecPurpose::run_label).unwrap_or_else(|| agent.as_str().to_string());
    Ok(ExecutionView {
        id: ExecutionId(id),
        session: session.map(SessionId),
        agent,
        purpose,
        stage: stage.map(|s| enum_from(&s)).transpose()?,
        group: execution_group(agent, &project),
        run_label,
        stream: executor.stream(),
        summary,
        has_transcript: false,
        project,
        executor,
        model,
        status: enum_from(&status)?,
        started_at: parse_time(&started)?,
        ended_at: parse_time_opt(ended.as_deref())?,
        usage: serde_json::from_str(&usage)?,
        report_path: report.map(PathBuf::from),
        native_session_id: native,
        spawn_block: spawn,
        error,
        can_resume,
        has_terminal: false,
        pending_gate: None,
        repo_root: None,
    })
}

type RawGate =
    (String, String, String, String, String, Option<String>, Option<String>, Option<String>, String, Option<String>);

fn raw_gate(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawGate> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?))
}

fn gate_from_raw(r: RawGate) -> Result<GateView, StoreError> {
    let (id, session, title, explanation, payload, answer, source, reason, opened, answered) = r;
    Ok(GateView {
        id: GateId(id),
        session: SessionId(session),
        title,
        explanation,
        payload: serde_json::from_str(&payload)?,
        answer: answer.map(|a| serde_json::from_str(&a)).transpose()?,
        source: source.map(|s| enum_from(&s)).transpose()?,
        reason,
        opened_at: parse_time(&opened)?,
        answered_at: parse_time_opt(answered.as_deref())?,
    })
}

type RawDecision = (String, String, Option<String>, String, String, String, bool, bool, String);

fn raw_decision(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawDecision> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
}

fn decision_from_raw(r: RawDecision) -> Result<DecisionView, StoreError> {
    let (id, judge, subject, input_summary, output, reason, overridden, can_override, at) = r;
    Ok(DecisionView {
        id: DecisionId(id),
        judge: enum_from(&judge)?,
        subject,
        input_summary,
        output: serde_json::from_str(&output)?,
        reason,
        overridden,
        can_override,
        at: parse_time(&at)?,
    })
}
