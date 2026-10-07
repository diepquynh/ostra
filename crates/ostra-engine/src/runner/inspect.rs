//! Reopening an ended harness run to read it, and the quick answers of the side panel.

use super::*;
use ostra_core::config::{PermissionRules, RouteQuery, resolve_route};
use ostra_core::event::ExecPurpose;
use ostra_core::exec::{
    CancellationToken, ExecContext, ExecutionSpec, ExecutionStatus, ResumeInfo, Usage,
};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, SessionId};
use ostra_core::paths;
use ostra_store::NewExecution;
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex};

impl Engine {
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
            work_dirs: Vec::new(),
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

impl Inner {
    pub(crate) fn prepare_quick(
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
            work_dirs: Vec::new(),
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
