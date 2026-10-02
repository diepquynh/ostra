use crate::StoreError;
use crate::sqlite::{RowExt, query_all, query_one};
use crate::util::{enum_str, now};
use ostra_core::agent::AgentName;
use ostra_core::api::{ExecutionView, execution_group};
use ostra_core::event::ExecPurpose;
use ostra_core::exec::{ExecutionResult, ExecutionStatus, Usage};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, SessionId};
use ostra_core::pipeline::StageKind;
use rusqlite::{Connection, Row, named_params, params};
use std::path::PathBuf;

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

const COLS: &str = "id, session_id, agent, purpose, stage, project, executor, model, status, started_at, \
     ended_at, usage, report_path, native_session_id, spawn_block, error, summary";

pub(crate) struct Executions<'c>(pub &'c Connection);

impl Executions<'_> {
    pub fn insert(&self, new: &NewExecution) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO executions (id, session_id, agent, purpose, stage, project, executor, model, params,
             spawn_block, report_path, status, native_session_id, usage, cost_usd, started_at)
             VALUES (:id, :session, :agent, :purpose, :stage, :project, :executor, :model, :params,
             :spawn_block, :report_path, :status, :native_session_id, :usage, 0, :started_at)",
            named_params! {
                ":id": new.id.as_str(),
                ":session": new.session.as_ref().map(SessionId::as_str),
                ":agent": new.agent.as_str(),
                ":purpose": new.purpose.as_ref().map(serde_json::to_string).transpose()?,
                ":stage": new.stage.map(|s| enum_str(&s)).transpose()?,
                ":project": new.project,
                ":executor": new.executor.to_string(),
                ":model": new.model,
                ":params": new.params.to_string(),
                ":spawn_block": new.spawn_block,
                ":report_path": new.report_path.as_ref().map(|p| p.to_string_lossy()),
                ":status": enum_str(&ExecutionStatus::Running)?,
                ":native_session_id": new.native_session_id,
                ":usage": serde_json::to_string(&Usage::default())?,
                ":started_at": now(),
            },
        )?;
        Ok(())
    }

    /// Returns false when no execution has the id.
    pub fn finish(&self, id: &ExecutionId, result: &ExecutionResult) -> Result<bool, StoreError> {
        let n = self.0.execute(
            "UPDATE executions SET status = :status, usage = :usage, cost_usd = :cost, error = :error,
             submit = :submit, final_text = :final_text,
             native_session_id = COALESCE(:native_session_id, native_session_id), ended_at = :ended_at
             WHERE id = :id",
            named_params! {
                ":status": enum_str(&result.status)?,
                ":usage": serde_json::to_string(&result.usage)?,
                ":cost": result.usage.cost_usd,
                ":error": result.error,
                ":submit": result.submit.as_ref().map(|v| v.to_string()),
                ":final_text": result.final_text,
                ":native_session_id": result.native_session_id,
                ":ended_at": now(),
                ":id": id.as_str(),
            },
        )?;
        Ok(n > 0)
    }

    /// Returns false when no execution has the id.
    pub fn reopen(&self, id: &ExecutionId) -> Result<bool, StoreError> {
        let n = self.0.execute(
            "UPDATE executions SET status = ?1, error = NULL, ended_at = NULL WHERE id = ?2",
            params![enum_str(&ExecutionStatus::Running)?, id.as_str()],
        )?;
        Ok(n > 0)
    }

    pub fn set_usage(&self, id: &ExecutionId, usage: &Usage) -> Result<(), StoreError> {
        self.0.execute(
            "UPDATE executions SET usage = ?1, cost_usd = ?2 WHERE id = ?3",
            params![serde_json::to_string(usage)?, usage.cost_usd, id.as_str()],
        )?;
        Ok(())
    }

    /// A terminal status also stamps `ended_at`.
    pub fn set_status(&self, id: &ExecutionId, status: ExecutionStatus) -> Result<(), StoreError> {
        let ended = status.is_terminal().then(now);
        self.0.execute(
            "UPDATE executions SET status = ?1, ended_at = COALESCE(?2, ended_at) WHERE id = ?3",
            params![enum_str(&status)?, ended, id.as_str()],
        )?;
        Ok(())
    }

    pub fn set_summary(&self, id: &ExecutionId, summary: &str) -> Result<(), StoreError> {
        self.0.execute(
            "UPDATE executions SET summary = ?1 WHERE id = ?2",
            params![summary, id.as_str()],
        )?;
        Ok(())
    }

    pub fn set_native_session_id(&self, id: &ExecutionId, native: &str) -> Result<(), StoreError> {
        self.0.execute(
            "UPDATE executions SET native_session_id = ?1 WHERE id = ?2",
            params![native, id.as_str()],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &ExecutionId) -> Result<Option<ExecutionView>, StoreError> {
        query_one(
            self.0,
            &format!("SELECT {COLS} FROM executions WHERE id = ?1"),
            [id.as_str()],
            execution,
        )
    }

    /// The spawn params, submit payload, and final text.
    pub fn output(
        &self,
        id: &ExecutionId,
    ) -> Result<Option<(serde_json::Value, ExecutionOutput)>, StoreError> {
        query_one(
            self.0,
            "SELECT params, submit, final_text FROM executions WHERE id = ?1",
            [id.as_str()],
            |r| {
                Ok((
                    r.json("params")?,
                    ExecutionOutput {
                        submit: r.json_opt("submit")?,
                        final_text: r
                            .get::<_, Option<String>>("final_text")?
                            .unwrap_or_default(),
                    },
                ))
            },
        )
    }

    /// Oldest first.
    pub fn of_session(&self, session: &SessionId) -> Result<Vec<ExecutionView>, StoreError> {
        query_all(
            self.0,
            &format!("SELECT {COLS} FROM executions WHERE session_id = ?1 ORDER BY started_at, id"),
            [session.as_str()],
            execution,
        )
    }

    /// Side-panel executions (no session), newest first.
    pub fn sessionless(&self, limit: usize) -> Result<Vec<ExecutionView>, StoreError> {
        query_all(
            self.0,
            &format!(
                "SELECT {COLS} FROM executions WHERE session_id IS NULL
                 ORDER BY started_at DESC, id DESC LIMIT ?1"
            ),
            [limit as i64],
            execution,
        )
    }

    pub fn running(&self) -> Result<Vec<ExecutionView>, StoreError> {
        query_all(
            self.0,
            &format!("SELECT {COLS} FROM executions WHERE status = ?1 ORDER BY started_at, id"),
            [enum_str(&ExecutionStatus::Running)?],
            execution,
        )
    }

    /// Every `running` execution becomes `interrupted`. Returns them.
    pub fn interrupt_running(&self) -> Result<Vec<ExecutionView>, StoreError> {
        let running = self.running()?;
        for e in &running {
            self.set_status(&e.id, ExecutionStatus::Interrupted)?;
        }
        Ok(running
            .into_iter()
            .map(|e| ExecutionView {
                status: ExecutionStatus::Interrupted,
                ..e
            })
            .collect())
    }
}

fn execution(r: &Row<'_>) -> Result<ExecutionView, StoreError> {
    let executor: ExecutorKind = r
        .get::<_, String>("executor")?
        .parse()
        .map_err(StoreError::Invalid)?;
    let agent: AgentName = r
        .get::<_, String>("agent")?
        .parse()
        .map_err(StoreError::Invalid)?;
    let purpose: Option<ExecPurpose> = r.json_opt("purpose")?;
    let project: String = r.get("project")?;
    let spans_session = purpose.as_ref().is_some_and(ExecPurpose::spans_session);
    let native_session_id: Option<String> = r.get("native_session_id")?;
    // The engine numbers repeated runs from the fold; this is the label of a first run.
    let run_label = purpose
        .as_ref()
        .map(ExecPurpose::run_label)
        .unwrap_or_else(|| agent.as_str().to_string());
    Ok(ExecutionView {
        id: ExecutionId(r.get("id")?),
        session: r.get::<_, Option<String>>("session_id")?.map(SessionId),
        agent,
        purpose,
        stage: r.variant_opt("stage")?,
        group: execution_group(agent, (!spans_session).then_some(project.as_str())),
        spans_session,
        run_label,
        stream: executor.stream(),
        summary: r.get("summary")?,
        has_transcript: false,
        project,
        can_resume: matches!(executor, ExecutorKind::Harness(_)) && native_session_id.is_some(),
        executor,
        model: r.get("model")?,
        status: r.variant("status")?,
        started_at: r.time("started_at")?,
        ended_at: r.time_opt("ended_at")?,
        usage: r.json("usage")?,
        report_path: r
            .get::<_, Option<String>>("report_path")?
            .map(PathBuf::from),
        native_session_id,
        spawn_block: r.get("spawn_block")?,
        error: r.get("error")?,
        can_skip: false,
        can_steer: false,
        queued_steer: None,
        has_terminal: false,
        pending_gate: None,
        repo_root: None,
    })
}
