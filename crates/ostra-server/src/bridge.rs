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
use std::sync::{Arc, LazyLock, OnceLock};

/// What one live harness execution needs server-side.
struct Running {
    policy: ExecutionPolicy,
    env: ToolEnv,
    host: Arc<dyn ExecutionHost>,
    memory_db: PathBuf,
    repo: PathBuf,
    /// A read-only reopening of an ended session: every tool call is refused.
    inspect: bool,
}

/// Why an inspection refuses a tool call, correction first because Grok clips reasons.
const INSPECT_DENIAL: &str = "Answer from what you already did, without tools: this is a read-only look back at a run that has ended, so Ostra refuses every tool call.";

#[derive(Default)]
struct Registry {
    running: Mutex<HashMap<ExecutionId, Arc<Running>>>,
    calls: AtomicU64,
    /// Where successful harness writes are reported for live file updates.
    write_sink: OnceLock<tokio::sync::mpsc::UnboundedSender<Touch>>,
    code: OnceLock<Arc<dyn ostra_tools::CodeNav>>,
    mcp: OnceLock<Arc<dyn ostra_tools::McpConnector>>,
    manage: OnceLock<Arc<dyn ostra_tools::ManageConnector>>,
    coord: OnceLock<Arc<dyn ostra_tools::CoordConnector>>,
}

pub struct HarnessRuntime {
    exe: PathBuf,
    callback: String,
    /// The Unix socket serving only [`internal_routes`], set once the server binds it.
    bridge_socket: OnceLock<PathBuf>,
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
        read_only_mcp_tools: vec![],
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
            bridge_socket: OnceLock::new(),
            live,
            ptys: PtyRegistry::new(),
            executor: OnceLock::new(),
            registry,
            bridge,
        }
    }

    /// Set before the first harness execution starts.
    pub fn set_bridge_socket(&self, path: PathBuf) {
        let _ = self.bridge_socket.set(path);
    }

    pub fn bridge_socket(&self) -> Option<&std::path::Path> {
        self.bridge_socket.get().map(PathBuf::as_path)
    }

    pub fn set_write_sink(&self, sink: tokio::sync::mpsc::UnboundedSender<Touch>) {
        let _ = self.registry.write_sink.set(sink);
    }

    pub fn set_code_nav(&self, nav: Arc<dyn ostra_tools::CodeNav>) {
        let _ = self.registry.code.set(nav);
    }

    pub fn set_mcp(&self, mcp: Arc<dyn ostra_tools::McpConnector>) {
        let _ = self.registry.mcp.set(mcp);
    }

    pub fn set_manage(&self, manage: Arc<dyn ostra_tools::ManageConnector>) {
        let _ = self.registry.manage.set(manage);
    }

    pub fn set_coord(&self, coord: Arc<dyn ostra_tools::CoordConnector>) {
        let _ = self.registry.coord.set(coord);
    }

    fn inner(&self, global: &GlobalConfig) -> Arc<HarnessExecutor> {
        let exec = self
            .executor
            .get_or_init(|| {
                let mut cfg = HarnessExecutorConfig::new(
                    global.clone(),
                    self.exe.clone(),
                    format!("http://{}", self.callback),
                );
                cfg.bridge_socket = self.bridge_socket.get().cloned();
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
        // Opened before the harness starts, because it lists Ostra's MCP tools once at startup.
        let mcp = match self.registry.mcp.get() {
            Some(c) => {
                let opened = c.open(&spec).await;
                for message in opened.notes {
                    host.emit(ExecutionDelta::Status { message });
                }
                opened.tools
            }
            None => None,
        };
        let mut inputs = policy_inputs(&ctx.repo_root);
        inputs.read_only_mcp_tools = mcp.iter().flat_map(|m| m.read_only()).collect();
        let running = Arc::new(Running {
            // Rule G2: the harness runs sandboxed exactly when this decides so.
            policy: ExecutionPolicy::new(ctx.clone(), inputs)
                .sandboxed(ostra_sandbox::runs_sandboxed(&ctx.sandbox())),
            env: ToolEnv::new(ToolEnvConfig {
                agent: spec.agent,
                doc_kinds: ostra_core::doc::DocKind::granted(&spec.capabilities),
                repo_root: ctx.repo_root.clone(),
                work_dirs: ctx.work_dirs.clone(),
                workspace_root: ctx.workspace_root.clone(),
                session_dir: ctx.session_dir.clone(),
                report_file: ctx.report_file.clone(),
                memory_db: ctx.memory_db.clone(),
                memory_source: format!("{} {}", spec.agent, spec.id),
                skill_resolver: crate::app::skill_resolver(),
                code: self.registry.code.get().cloned(),
                mcp,
                manage: self.registry.manage.get().and_then(|m| m.open(&spec)),
                coord: self.registry.coord.get().and_then(|c| c.open(&spec)),
            })
            .with_private_hosts(ostra_tools::webfetch_hosts(&ctx.permissions.allow)),
            host: host.clone(),
            memory_db: ctx.memory_db.clone(),
            repo: ctx.repo_root.clone(),
            inspect: spec.resume.as_ref().is_some_and(|r| r.inspect),
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
        let decision = if r.inspect {
            PolicyDecision::deny(RuleRef::guard("read-only-session"), INSPECT_DENIAL)
        } else {
            r.policy.check(call)
        };
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

/// Ostra's MCP tools other than `submit_*`: the MCP name, the native tool, its capability.
static MCP_TOOLS: LazyLock<Vec<(String, &'static str, Capability)>> = LazyLock::new(|| {
    let mut v: Vec<(String, &'static str, Capability)> = vec![
        ("report".into(), "Report", Capability::Report),
        ("document".into(), "Document", Capability::DocumentResearch),
        ("memory".into(), "Memory", Capability::Memory),
        (
            "memory_recall".into(),
            "MemoryRecall",
            Capability::MemoryRecall,
        ),
        ("docs_search".into(), "DocsSearch", Capability::DocsSearch),
    ];
    v.extend(
        ostra_core::agent::CODE_TOOLS
            .iter()
            .map(|(op, native)| (format!("code_{op}"), *native, Capability::Code)),
    );
    v.extend(
        ostra_core::manage::PROJECT_TOOLS
            .iter()
            .map(|(name, native)| (name.to_string(), *native, Capability::ManageProjects)),
    );
    v.extend(
        ostra_core::coord::COORD_TOOLS
            .iter()
            .map(|(name, native)| (name.to_string(), *native, Capability::Coordinate)),
    );
    v
});

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
        // Rule M2: a workspace MCP tool reaches the harness only through Ostra's MCP server,
        // which checks and logs the call when it runs it, so the hook passes it unchecked.
        if ostra_core::mcp::is_gateway_tool(&call.tool) && !r.inspect {
            return PolicyDecision::Allow { rule: None };
        }
        // Rule O1: the same holds for a management tool, so its ask reaches the user once.
        if ostra_core::manage::is_manage_tool(&call.tool) && !r.inspect {
            return PolicyDecision::Allow { rule: None };
        }
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
    ) -> Result<ostra_exec_harness::McpOut, String> {
        let r = self
            .get(execution)
            .ok_or("This execution is not running in Ostra.")?;
        let native = match MCP_TOOLS.iter().find(|(name, _, _)| *name == tool) {
            Some((_, n, _)) => n.to_string(),
            None if ostra_core::mcp::is_gateway_bare(tool) => ostra_core::mcp::canonical(tool),
            None => return Err(format!("unknown tool {tool}")),
        };
        let call = r.env.canonical_call(&ToolCall::new(native, args));
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
        if out.is_error {
            Err(text)
        } else {
            Ok(ostra_exec_harness::McpOut { text, end: out.end })
        }
    }

    fn mcp_tools(&self, execution: &ExecutionId) -> Vec<(String, String, Value)> {
        let Some(r) = self.get(execution).filter(|r| !r.inspect) else {
            return vec![];
        };
        // Every run gets Ostra's own tools, except management tools, which only an execution
        // whose agent holds the capability is given a handle for.
        let caps: Vec<Capability> = MCP_TOOLS
            .iter()
            .map(|(_, _, c)| *c)
            .filter(|c| *c != Capability::ManageProjects || r.env.config().manage.is_some())
            .filter(|c| *c != Capability::Coordinate || r.env.config().coord.is_some())
            .collect();
        ostra_tools::definitions(&caps)
            .into_iter()
            .chain(ostra_tools::document_tool_definition(
                &r.env.config().doc_kinds,
            ))
            .filter_map(|d| {
                MCP_TOOLS
                    .iter()
                    .find(|(_, n, _)| *n == d.name)
                    .map(|(name, _, _)| (name.to_string(), d.description, d.input_schema))
            })
            .chain(
                r.env
                    .config()
                    .mcp
                    .iter()
                    .flat_map(|m| m.definitions())
                    .filter_map(|d| {
                        let bare = ostra_core::mcp::bare(&d.name)?.to_string();
                        Some((bare, d.description, d.input_schema))
                    }),
            )
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

#[cfg(unix)]
/// Serves [`internal_routes`] on a Unix socket in the egress dir, which sandboxes with their own
/// network reach the hook bridge through. Nothing else of the server answers there.
pub fn serve_bridge_socket(app: &Arc<App>) -> std::io::Result<PathBuf> {
    let path = ostra_sandbox::egress::socket_path(&ostra_sandbox::egress::socket_dir(), "bridge-")?;
    let listener = tokio::net::UnixListener::bind(&path)?;
    let router = internal_routes().with_state(app.clone());
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!("the hook bridge socket stopped: {e}");
        }
    });
    Ok(path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use ostra_core::AgentName;
    use ostra_core::config::{PermissionMode, PermissionRules};
    use ostra_core::exec::ExecContext;
    use ostra_core::executor::ExecutorKind;
    use ostra_core::policy::RuleRef;
    use ostra_tools::{McpTools, ToolDefinition};
    use serde_json::json;

    #[derive(Default)]
    struct Host(Mutex<Vec<ExecutionDelta>>);

    #[async_trait::async_trait]
    impl ExecutionHost for Host {
        fn emit(&self, delta: ExecutionDelta) {
            self.0.lock().push(delta);
        }
        async fn ask_permission(&self, _: &ToolCall, _: &str, _: &RuleRef) -> PermissionAnswer {
            PermissionAnswer::Deny
        }
    }

    struct Echo;

    #[async_trait::async_trait]
    impl McpTools for Echo {
        fn definitions(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: "mcp__fake__echo".into(),
                description: "Echo".into(),
                input_schema: json!({"type": "object"}),
            }]
        }
        fn read_only(&self) -> Vec<String> {
            vec![]
        }
        async fn call(
            &self,
            _: &str,
            input: &Value,
            _: CancellationToken,
        ) -> Result<String, String> {
            Ok(format!("echo: {}", input["text"].as_str().unwrap_or("")))
        }
    }

    fn running(
        dir: &std::path::Path,
        deny: &[&str],
        inspect: bool,
        host: Arc<Host>,
    ) -> Arc<Running> {
        let ctx = ExecContext {
            work_dirs: Vec::new(),
            execution_id: "x_1".into(),
            session_id: None,
            agent: AgentName::Implementer,
            initializer_mode: None,
            executor: ExecutorKind::Harness(HarnessKind::Codex),
            workspace_root: dir.join("ws"),
            repo_root: dir.join("repo"),
            project_key: "app".into(),
            session_dir: dir.join("ws/s"),
            session_root: dir.join("ws/s"),
            report_file: None,
            phase: None,
            yolo: false,
            permission_mode: PermissionMode::Default,
            permissions: PermissionRules {
                deny: deny.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
            protected_paths: vec![],
            memory_db: dir.join("memory.sqlite3"),
            sandbox_mode: None,
            enforce_tool_calls: false,
            sandbox_network: None,
            sandbox_allowed_hosts: vec![],
            sandbox_decoys: vec![],
            sandbox_readable: vec![],
            sandbox_loopback: Default::default(),
            sandbox_blocked_ports: vec![],
            creates_project: false,
            owes_reply: false,
            write_scope: None,
            contract: ostra_core::Contract::Implementation,
            capabilities: vec![Capability::ManageProjects],
        };
        Arc::new(Running {
            policy: ExecutionPolicy::new(ctx.clone(), Default::default()),
            env: ToolEnv::new(ToolEnvConfig {
                agent: ctx.agent,
                doc_kinds: vec![],
                repo_root: ctx.repo_root.clone(),
                work_dirs: ctx.work_dirs.clone(),
                workspace_root: ctx.workspace_root.clone(),
                session_dir: ctx.session_dir.clone(),
                report_file: None,
                memory_db: ctx.memory_db.clone(),
                memory_source: "test".into(),
                skill_resolver: Arc::new(|_: &str| None),
                code: None,
                mcp: Some(Arc::new(Echo)),
                manage: None,
                coord: None,
            }),
            host,
            memory_db: ctx.memory_db.clone(),
            repo: ctx.repo_root.clone(),
            inspect,
        })
    }

    // Rule M2: the hook passes a workspace MCP tool, and the shim checks and logs it once.
    #[tokio::test]
    async fn workspace_mcp_tools_through_the_shim() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = ServerBridge {
            registry: Arc::new(Registry::default()),
        };
        let id = ExecutionId::from("x_1");
        let host = Arc::new(Host::default());
        bridge
            .registry
            .running
            .lock()
            .insert(id.clone(), running(dir.path(), &[], false, host.clone()));

        let listed: Vec<String> = bridge.mcp_tools(&id).into_iter().map(|t| t.0).collect();
        assert!(listed.contains(&"fake__echo".to_string()), "{listed:?}");
        let hook = bridge.policy_check(&id, &ToolCall::new("mcp__fake__echo", json!({})));
        assert_eq!(hook, PolicyDecision::Allow { rule: None });
        assert!(host.0.lock().is_empty(), "the hook logs nothing");

        let out = bridge
            .mcp_call(&id, "fake__echo", json!({"text": "hi"}))
            .await;
        assert_eq!(out.unwrap().text, "echo: hi");
        let logged = host.0.lock().clone();
        let calls: Vec<&str> = logged
            .iter()
            .filter_map(|d| match d {
                ExecutionDelta::ToolCall { call, .. } => Some(call.tool.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(calls, ["mcp__fake__echo"]);

        let denied_host = Arc::new(Host::default());
        bridge.registry.running.lock().insert(
            id.clone(),
            running(dir.path(), &["mcp__fake"], false, denied_host),
        );
        let err = bridge
            .mcp_call(&id, "fake__echo", json!({}))
            .await
            .unwrap_err();
        assert!(err.starts_with("Denied by permission"), "{err}");

        bridge.registry.running.lock().insert(
            id.clone(),
            running(dir.path(), &[], true, Arc::new(Host::default())),
        );
        assert!(bridge.mcp_tools(&id).is_empty());
    }

    #[derive(Default)]
    struct AskCounter(Mutex<u32>);

    #[async_trait::async_trait]
    impl ExecutionHost for AskCounter {
        fn emit(&self, _: ExecutionDelta) {}
        async fn ask_permission(&self, _: &ToolCall, _: &str, _: &RuleRef) -> PermissionAnswer {
            *self.0.lock() += 1;
            PermissionAnswer::AllowOnce
        }
    }

    struct FakeManage;

    #[async_trait::async_trait]
    impl ostra_tools::Manage for FakeManage {
        async fn call(&self, tool: &str, input: &Value) -> Result<String, String> {
            Ok(format!("{tool} {}", input["key"].as_str().unwrap_or("")))
        }
    }

    // Rules O1 and O2 on a harness: only an agent with a handle sees the project tools, the hook
    // passes them, and the MCP handler asks the user once before it runs the call.
    #[tokio::test]
    async fn project_tools_through_the_shim_ask_once() {
        let dir = tempfile::tempdir().unwrap();
        let bridge = ServerBridge {
            registry: Arc::new(Registry::default()),
        };
        let id = ExecutionId::from("x_1");
        let host = Arc::new(AskCounter::default());
        let base = running(dir.path(), &[], false, Arc::new(Host::default()));
        let listed: Vec<String> = {
            bridge.registry.running.lock().insert(id.clone(), base);
            bridge.mcp_tools(&id).into_iter().map(|t| t.0).collect()
        };
        assert!(
            !listed.iter().any(|t| t.starts_with("project_")),
            "{listed:?}"
        );

        let mut ctx = running(dir.path(), &[], false, Arc::new(Host::default()))
            .policy
            .ctx()
            .clone();
        ctx.agent = AgentName::Implementer;
        ctx.session_id = Some("s1".into());
        ctx.project_key = "mcp".into();
        ctx.creates_project = true;
        let r = Arc::new(Running {
            policy: ExecutionPolicy::new(ctx.clone(), Default::default()),
            env: ToolEnv::new(ToolEnvConfig {
                agent: ctx.agent,
                doc_kinds: vec![],
                repo_root: ctx.repo_root.clone(),
                work_dirs: ctx.work_dirs.clone(),
                workspace_root: ctx.workspace_root.clone(),
                session_dir: ctx.session_dir.clone(),
                report_file: None,
                memory_db: ctx.memory_db.clone(),
                memory_source: "test".into(),
                skill_resolver: Arc::new(|_: &str| None),
                code: None,
                mcp: None,
                manage: Some(Arc::new(FakeManage)),
                coord: None,
            }),
            host: host.clone(),
            memory_db: ctx.memory_db.clone(),
            repo: ctx.repo_root.clone(),
            inspect: false,
        });
        bridge.registry.running.lock().insert(id.clone(), r);
        let listed: Vec<String> = bridge.mcp_tools(&id).into_iter().map(|t| t.0).collect();
        assert!(listed.contains(&"project_create".to_string()), "{listed:?}");
        assert!(listed.contains(&"project_list".to_string()), "{listed:?}");

        let args = json!({"key": "mcp", "stack": "rust", "purpose": "p", "requirements": ["x"]});
        let hook = bridge.policy_check(&id, &ToolCall::new("ProjectCreate", args.clone()));
        assert_eq!(hook, PolicyDecision::Allow { rule: None });
        assert_eq!(*host.0.lock(), 0, "the hook does not ask");
        let out = bridge.mcp_call(&id, "project_create", args).await.unwrap();
        assert_eq!(out.text, "ProjectCreate mcp");
        assert_eq!(*host.0.lock(), 1, "the shim asks once");
    }
}
