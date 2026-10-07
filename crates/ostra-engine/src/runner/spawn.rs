//! Starting an execution: route, sandbox, spawn block, and the run to its result, and the shell helpers.

use super::*;
use crate::plan::SpawnRequest;
use crate::services::{BuiltSpawn, SpawnEnv};
use crate::state::purpose_key;
use ostra_core::config::{PermissionRules, ProjectProfile, RouteQuery, load_toml, resolve_route};
use ostra_core::event::{ExecPurpose, SessionEvent};
use ostra_core::exec::ExecutionHost;
use ostra_core::exec::{
    CancellationToken, ExecContext, ExecutionDelta, ExecutionSpec, ExecutionStatus, ResumeInfo,
    Usage,
};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, SessionId};
use ostra_core::paths;
use ostra_store::NewExecution;
use std::path::Path;
use std::sync::{Arc, Mutex};

impl Inner {
    pub(crate) async fn perform_spawn(
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
        // Rule WD1: the run works in each project the planner named, the main one first. A named
        // project that is not in the session yet has no folder and is left out.
        let work_dirs: Vec<ostra_core::exec::WorkDir> = req
            .projects()
            .into_iter()
            .enumerate()
            .filter_map(|(i, key)| {
                let path = if i == 0 {
                    repo_root.clone()
                } else {
                    st.project_path(&key)?
                };
                Some(ostra_core::exec::WorkDir { project: key, path })
            })
            .collect();
        let profile: Option<ProjectProfile> = load_toml(&paths::project_profile(&repo_root)).ok();
        let inventory = std::fs::read_to_string(paths::project_inventory(&repo_root)).ok();
        let project_docs = ostra_agents::brief::project_docs(&repo_root);
        let other_dirs: Vec<ostra_agents::brief::WorkDirBrief> = work_dirs
            .iter()
            .skip(1)
            .map(|w| ostra_agents::brief::WorkDirBrief::read(&w.project, &w.path))
            .collect();
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
                    work_dirs: &other_dirs,
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
            work_dirs,
            execution_id: id.clone(),
            session_id: Some(session.clone()),
            agent: req.agent,
            initializer_mode: req.purpose.initializer_mode(),
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
                    projects: req.recorded_projects(),
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
}

pub(crate) fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub async fn run_shell(
    cwd: &Path,
    program: &str,
    args: &[String],
    timeout_secs: u64,
) -> (Option<i32>, String) {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args);
    run_command(cmd, cwd, program, timeout_secs).await
}

pub async fn run_host(
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

pub(crate) async fn run_command(
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

pub(crate) fn tail(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut start = s.len() - n;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", &s[start..])
}
