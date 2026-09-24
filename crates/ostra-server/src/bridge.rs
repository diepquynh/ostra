//! Harness executors wired into the server: one policy per execution, the hook bridge and MCP
//! shim endpoints (`/internal/policy`, `/internal/mcp`, per-execution bearer tokens), and PTY
//! input, resize, and snapshots for the Terminal tab.

use crate::app::App;
use crate::files::watch::{Touch, call_cwd, write_targets};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use ostra_core::agent::Capability;
use ostra_core::config::{GlobalConfig, ProjectProfile, load_toml};
use ostra_core::exec::{
    CancellationToken, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec, Executor,
};
use ostra_core::executor::HarnessKind;
use ostra_core::ids::ExecutionId;
use ostra_core::paths;
use ostra_core::policy::{PermissionAnswer, PolicyDecision, RuleRef, ToolCall, ToolOutcome};
use ostra_exec_harness::{
    BridgeServices, HarnessBridge, HarnessExecutor, HarnessExecutorConfig, LiveRegistry,
    McpRequest, PolicyRequest, PtyRegistry,
};
use ostra_policy::{ExecutionPolicy, Observation, PolicyInputs};
use ostra_tools::{ToolEnv, ToolEnvConfig};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// What one live harness execution needs server-side.
struct Running {
    policy: ExecutionPolicy,
    env: ToolEnv,
    host: Arc<dyn ExecutionHost>,
    memory_db: PathBuf,
    repo: PathBuf,
}

#[derive(Default)]
struct Registry {
    running: Mutex<HashMap<ExecutionId, Arc<Running>>>,
    calls: AtomicU64,
    /// Where successful harness writes are reported for live file updates.
    write_sink: OnceLock<tokio::sync::mpsc::UnboundedSender<Touch>>,
}

pub struct HarnessRuntime {
    exe: PathBuf,
    callback: String,
    live: Arc<LiveRegistry>,
    ptys: Arc<PtyRegistry>,
    executor: OnceLock<Arc<HarnessExecutor>>,
    registry: Arc<Registry>,
    bridge: HarnessBridge,
}

fn policy_inputs(repo: &std::path::Path) -> PolicyInputs {
    let profile: ProjectProfile = load_toml(&paths::project_profile(repo)).unwrap_or_default();
    let c = &profile.commands;
    let some = |v: &[&Option<String>]| {
        v.iter()
            .filter_map(|x| x.as_ref().filter(|s| !s.trim().is_empty()).cloned())
            .collect()
    };
    PolicyInputs {
        build_commands: some(&[&c.build, &c.test, &c.test_one, &c.lint, &c.typecheck]),
        test_commands: some(&[&c.test, &c.test_one]),
    }
}

impl HarnessRuntime {
    pub fn new(exe: PathBuf, callback: String) -> Self {
        let live = LiveRegistry::new();
        let registry = Arc::new(Registry::default());
        let bridge = HarnessBridge::new(
            Arc::new(ServerBridge {
                registry: registry.clone(),
            }),
            live.clone(),
        );
        HarnessRuntime {
            exe,
            callback,
            live,
            ptys: PtyRegistry::new(),
            executor: OnceLock::new(),
            registry,
            bridge,
        }
    }

    pub fn set_write_sink(&self, sink: tokio::sync::mpsc::UnboundedSender<Touch>) {
        let _ = self.registry.write_sink.set(sink);
    }

    fn inner(&self, global: &GlobalConfig) -> Arc<HarnessExecutor> {
        let exec = self
            .executor
            .get_or_init(|| {
                let cfg = HarnessExecutorConfig::new(
                    global.clone(),
                    self.exe.clone(),
                    format!("http://{}", self.callback),
                );
                Arc::new(HarnessExecutor::new(
                    cfg,
                    self.live.clone(),
                    self.ptys.clone(),
                ))
            })
            .clone();
        exec.set_global(global.clone());
        exec
    }

    pub fn executor(
        &self,
        _harness: HarnessKind,
        global: &GlobalConfig,
    ) -> Option<Arc<dyn Executor>> {
        Some(Arc::new(Wrapped {
            inner: self.inner(global),
            registry: self.registry.clone(),
        }))
    }

    pub fn has_terminal(&self, id: &ExecutionId) -> bool {
        self.ptys.contains(id)
    }

    /// Live PTYs, for streaming, input, and resizes from the Terminal tab.
    pub fn ptys(&self) -> Arc<PtyRegistry> {
        self.ptys.clone()
    }
}

/// Registers the execution's policy, tools, and host around the harness run.
struct Wrapped {
    inner: Arc<HarnessExecutor>,
    registry: Arc<Registry>,
}

#[async_trait::async_trait]
impl Executor for Wrapped {
    async fn run(
        &self,
        spec: ExecutionSpec,
        host: Arc<dyn ExecutionHost>,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        let ctx = spec.ctx.clone();
        let running = Arc::new(Running {
            policy: ExecutionPolicy::new(ctx.clone(), policy_inputs(&ctx.repo_root)),
            env: ToolEnv::new(ToolEnvConfig {
                agent: spec.agent,
                repo_root: ctx.repo_root.clone(),
                session_dir: ctx.session_dir.clone(),
                report_file: ctx.report_file.clone(),
                memory_db: ctx.memory_db.clone(),
                memory_source: format!("{} {}", spec.agent, spec.id),
                skill_resolver: crate::app::skill_resolver(),
            }),
            host: host.clone(),
            memory_db: ctx.memory_db.clone(),
            repo: ctx.repo_root.clone(),
        });
        self.registry
            .running
            .lock()
            .insert(spec.id.clone(), running);
        let id = spec.id.clone();
        let result = self.inner.run(spec, host, cancel).await;
        self.registry.running.lock().remove(&id);
        result
    }
}

struct ServerBridge {
    registry: Arc<Registry>,
}

impl ServerBridge {
    fn get(&self, id: &ExecutionId) -> Option<Arc<Running>> {
        self.registry.running.lock().get(id).cloned()
    }

    fn call_id(&self) -> String {
        format!("h{}", self.registry.calls.fetch_add(1, Ordering::SeqCst))
    }

    /// Check a call and, when `log` is set, record the call and its decision under `id`.
    fn check(&self, r: &Running, call: &ToolCall, id: &str, log: bool) -> PolicyDecision {
        r.policy.set_yolo(r.host.yolo());
        let decision = r.policy.check(call);
        if log {
            r.host.emit(ExecutionDelta::ToolCall {
                call_id: id.into(),
                call: call.clone(),
            });
            r.host.emit(ExecutionDelta::Policy {
                call_id: id.into(),
                decision: decision.clone(),
            });
        }
        decision
    }
}

/// Tools the MCP handler logs itself, so the hook path does not log them twice.
fn served_by_mcp(tool: &str) -> bool {
    MCP_TOOLS.iter().any(|(_, native, _)| *native == tool)
}

const MCP_TOOLS: [(&str, &str, Capability); 4] = [
    ("report", "Report", Capability::Report),
    ("document", "Document", Capability::Document),
    ("memory", "Memory", Capability::Memory),
    ("memory_recall", "MemoryRecall", Capability::MemoryRecall),
];

#[async_trait::async_trait]
impl BridgeServices for ServerBridge {
    fn authorize(&self, execution: &ExecutionId, _token: &str) -> bool {
        self.get(execution).is_some()
    }

    fn policy_check(&self, execution: &ExecutionId, call: &ToolCall) -> PolicyDecision {
        let Some(r) = self.get(execution) else {
            return PolicyDecision::deny(
                RuleRef::guard("self-protection"),
                "This execution is not running in Ostra.",
            );
        };
        let id = self.call_id();
        self.check(&r, call, &id, !served_by_mcp(&call.tool))
    }

    async fn resolve_ask(
        &self,
        execution: &ExecutionId,
        call: &ToolCall,
        reason: &str,
        rule: &RuleRef,
    ) -> PermissionAnswer {
        let Some(r) = self.get(execution) else {
            return PermissionAnswer::Deny;
        };
        let answer = r.host.ask_permission(call, reason, rule).await;
        if answer == PermissionAnswer::AlwaysInWorkspace
            && let Some(rule) = r.policy.allow_rule_suggestion(call)
        {
            r.policy.add_session_allow(rule);
        }
        answer
    }

    fn policy_observe(
        &self,
        execution: &ExecutionId,
        call: &ToolCall,
        outcome: &ToolOutcome,
    ) -> Vec<String> {
        let Some(r) = self.get(execution) else {
            return vec![];
        };
        if !outcome.is_error
            && let Some(sink) = self.registry.write_sink.get()
        {
            let paths = write_targets(call);
            if !paths.is_empty() {
                let base = call_cwd(call).unwrap_or_else(|| r.repo.clone());
                let _ = sink.send(Touch {
                    workspace: None,
                    execution: execution.clone(),
                    paths,
                    base: Some(base),
                });
            }
        }
        let mut notes = vec![];
        for o in r.policy.observe(call, outcome) {
            match o {
                Observation::AppendNote(n) => notes.push(n),
                Observation::RecallLessons { query } => {
                    let lessons = ostra_store::MemoryStore::open(&r.memory_db)
                        .and_then(|m| m.recall(None, Some(&query), 5))
                        .unwrap_or_default();
                    if !lessons.is_empty() {
                        let mut s = String::from("Lessons recorded for failures like this one:\n");
                        for (i, l) in lessons.iter().enumerate() {
                            s.push_str(&format!("{}. [{}] {}\n", i + 1, l.area, l.lesson));
                        }
                        notes.push(s);
                    }
                }
            }
        }
        notes
    }

    async fn mcp_call(
        &self,
        execution: &ExecutionId,
        tool: &str,
        args: Value,
    ) -> Result<String, String> {
        let r = self
            .get(execution)
            .ok_or("This execution is not running in Ostra.")?;
        let native = MCP_TOOLS
            .iter()
            .find(|(name, _, _)| *name == tool)
            .map(|(_, n, _)| *n)
            .ok_or_else(|| format!("unknown tool {tool}"))?;
        let call = ToolCall::new(native, args);
        let id = self.call_id();
        match self.check(&r, &call, &id, true) {
            PolicyDecision::Deny { reason, rule } => {
                return Err(format!(
                    "Denied by {} `{}`: {reason}",
                    rule.layer, rule.rule
                ));
            }
            PolicyDecision::Ask { reason, rule } => {
                if !self
                    .resolve_ask(execution, &call, &reason, &rule)
                    .await
                    .allows()
                {
                    return Err(format!("The user denied this call ({reason})."));
                }
            }
            PolicyDecision::Allow { .. } => {}
        }
        let out = ostra_tools::execute(&r.env, &id, &call, None, CancellationToken::new()).await;
        let outcome = ToolOutcome {
            output: out.text.clone(),
            is_error: out.is_error,
            exit_code: out.exit_code,
            result_known: true,
        };
        let notes = self.policy_observe(execution, &call, &outcome);
        r.host.emit(ExecutionDelta::ToolResult {
            call_id: id,
            output: out.text.clone(),
            is_error: out.is_error,
            duration_ms: out.duration_ms,
        });
        let mut text = out.text;
        for n in notes {
            text.push_str("\n\n");
            text.push_str(&n);
        }
        if out.is_error { Err(text) } else { Ok(text) }
    }

    fn mcp_tools(&self, execution: &ExecutionId) -> Vec<(String, String, Value)> {
        let Some(r) = self.get(execution) else {
            return vec![];
        };
        let caps: Vec<Capability> = MCP_TOOLS.iter().map(|(_, _, c)| *c).collect();
        ostra_tools::definitions(&caps)
            .into_iter()
            .chain(ostra_tools::document_tool_definition(r.env.config().agent))
            .filter_map(|d| {
                MCP_TOOLS
                    .iter()
                    .find(|(_, n, _)| *n == d.name)
                    .map(|(name, _, _)| (name.to_string(), d.description, d.input_schema))
            })
            .collect()
    }

    fn stop_event(&self, execution: &ExecutionId, _payload: Value) {
        if let Some(r) = self.get(execution) {
            r.host.emit(ExecutionDelta::Status {
                message: "The harness ended its turn.".into(),
            });
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(|s| s.trim().to_string())
}

async fn policy_route(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<PolicyRequest>,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    Json(app.shared.harness.bridge.handle_policy(&token, req).await).into_response()
}

async fn mcp_route(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<McpRequest>,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    Json(app.shared.harness.bridge.handle_mcp(&token, req).await).into_response()
}

pub fn internal_routes() -> axum::Router<Arc<App>> {
    axum::Router::new()
        .route(
            ostra_exec_harness::protocol::POLICY_PATH,
            post(policy_route),
        )
        .route(ostra_exec_harness::protocol::MCP_PATH, post(mcp_route))
}

pub async fn hook_cli(args: Vec<String>) -> i32 {
    ostra_exec_harness::run_hook_cli(&args).await
}

pub async fn mcp_cli(args: Vec<String>) -> i32 {
    ostra_exec_harness::run_mcp_stdio(&args).await
}
