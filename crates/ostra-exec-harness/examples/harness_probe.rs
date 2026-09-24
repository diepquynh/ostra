//! Live probe for one harness: `harness_probe run <harness> <model> <scratch-dir>`.
//! Serves `/internal/policy` and `/internal/mcp` with an allow-all policy that logs every payload,
//! runs one QuickAnswer execution in the harness, and prints the result. The same binary serves
//! as the `hook` and `mcp-stdio` subcommands the harness calls back.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use ostra_core::agent::{AgentName, Capability};
use ostra_core::config::{GlobalConfig, PermissionMode, PermissionRules, ResolvedRoute};
use ostra_core::exec::*;
use ostra_core::policy::*;
use ostra_core::{Effort, ExecutionId, ExecutorKind, HarnessKind};
use ostra_exec_harness::*;
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct Log(Mutex<std::fs::File>);
impl Log {
    fn line(&self, v: Value) {
        let mut f = self.0.lock().unwrap();
        let _ = writeln!(f, "{v}");
    }
}

struct Services(Arc<Log>);

#[async_trait::async_trait]
impl BridgeServices for Services {
    fn authorize(&self, _: &ExecutionId, _: &str) -> bool {
        true
    }
    fn policy_check(&self, _: &ExecutionId, call: &ToolCall) -> PolicyDecision {
        self.0.line(json!({"check": call}));
        let deny = std::env::var("PROBE_DENY").is_ok()
            && call.tool == "Bash"
            && call
                .str_field("command")
                .is_some_and(|c| c.contains("forbidden"));
        if deny {
            return PolicyDecision::deny(
                RuleRef::permission("Bash(echo forbidden*)"),
                "Run the next step instead: this command is refused.",
            );
        }
        PolicyDecision::allow()
    }
    async fn resolve_ask(
        &self,
        _: &ExecutionId,
        _: &ToolCall,
        _: &str,
        _: &RuleRef,
    ) -> PermissionAnswer {
        PermissionAnswer::AllowOnce
    }
    fn policy_observe(
        &self,
        _: &ExecutionId,
        call: &ToolCall,
        outcome: &ToolOutcome,
    ) -> Vec<String> {
        self.0.line(json!({"observe": call, "outcome": outcome}));
        vec![]
    }
    async fn mcp_call(&self, _: &ExecutionId, tool: &str, args: Value) -> Result<String, String> {
        self.0.line(json!({"mcp_call": tool, "args": args}));
        Ok("ok".into())
    }
    fn mcp_tools(&self, _: &ExecutionId) -> Vec<(String, String, Value)> {
        vec![]
    }
    fn stop_event(&self, _: &ExecutionId, payload: Value) {
        self.0.line(json!({"stop": payload}));
    }
}

#[derive(Clone)]
struct App {
    bridge: HarnessBridge,
    log: Arc<Log>,
}

fn token(h: &HeaderMap) -> String {
    h.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_string()
}

async fn policy(
    State(app): State<App>,
    headers: HeaderMap,
    Json(req): Json<PolicyRequest>,
) -> Json<PolicyResponse> {
    app.log
        .line(json!({"hook": req.event, "payload": req.payload}));
    let resp = app.bridge.handle_policy(&token(&headers), req).await;
    app.log.line(json!({"answer": resp.stdout}));
    Json(resp)
}

async fn mcp(
    State(app): State<App>,
    headers: HeaderMap,
    Json(req): Json<McpRequest>,
) -> Json<McpResponse> {
    app.log.line(json!({"mcp_in": req.message}));
    Json(app.bridge.handle_mcp(&token(&headers), req).await)
}

struct Host(Arc<Log>);

#[async_trait::async_trait]
impl ExecutionHost for Host {
    fn emit(&self, delta: ExecutionDelta) {
        eprintln!("delta: {delta:?}");
        self.0.line(json!({"delta": delta}));
    }
    async fn ask_permission(&self, _: &ToolCall, _: &str, _: &RuleRef) -> PermissionAnswer {
        PermissionAnswer::AllowOnce
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("hook") => std::process::exit(run_hook_cli(&args[1..]).await),
        Some("mcp-stdio") => std::process::exit(run_mcp_stdio(&args[1..]).await),
        Some("run") | Some("resume") => {}
        _ => {
            eprintln!("usage: harness_probe run <harness> <model> <scratch-dir> [timeout-secs]");
            std::process::exit(2);
        }
    }
    let harness: HarnessKind = args[1].parse().unwrap();
    let model = args[2].clone();
    let dir = PathBuf::from(&args[3]);
    let resume_sid = (args[0] == "resume").then(|| args[4].clone());
    let timeout: u64 = if resume_sid.is_some() {
        240
    } else {
        args.get(4).and_then(|s| s.parse().ok()).unwrap_or(240)
    };
    let repo = dir.join("repo");
    let session_root = dir.join("ws/.ostra/sessions/s_probe");
    std::fs::create_dir_all(&repo).unwrap();
    let suffix = if resume_sid.is_some() { "-resume" } else { "" };
    std::fs::create_dir_all(&session_root).unwrap();
    let log = Arc::new(Log(Mutex::new(
        std::fs::File::create(dir.join(format!("{harness}{suffix}-events.jsonl"))).unwrap(),
    )));
    let live = LiveRegistry::new();
    let bridge = HarnessBridge::new(Arc::new(Services(log.clone())), live.clone());
    let app = App {
        bridge,
        log: log.clone(),
    };
    let router = Router::new()
        .route("/internal/policy", post(policy))
        .route("/internal/mcp", post(mcp))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let mut cfg = HarnessExecutorConfig::new(
        GlobalConfig::default(),
        std::env::current_exe().unwrap(),
        url,
    );
    cfg.idle_nudge = std::time::Duration::from_secs(90);
    let exec = HarnessExecutor::new(cfg, live, PtyRegistry::new());
    let id = ExecutionId::new();
    let agent = AgentName::QuickAnswer;
    let spec = ExecutionSpec {
        id: id.clone(),
        agent,
        route: ResolvedRoute {
            executor: ExecutorKind::Harness(harness),
            model,
            tier: None,
        },
        effort: Effort::Low,
        system_prompt:
            "You are an Ostra probe agent. Follow the user's steps exactly and do nothing else."
                .into(),
        first_message: format!(
            "Do exactly these steps and nothing more:\n{}1. Run the shell command `echo ostra-probe > probe.txt`.\n2. Call the `{}` tool with answer \"probe ok\" and sources [\"probe.txt\"].\nThen end your turn.",
            if std::env::var("PROBE_DENY").is_ok() {
                "0. Run the shell command `echo forbidden`. Ostra may refuse it; if so, continue.\n"
            } else {
                ""
            },
            agent.submit_tool_name()
        ),
        capabilities: vec![Capability::Read, Capability::Shell],
        submit_schema: ostra_core::submit::submit_schema(agent),
        timeout_secs: timeout,
        ctx: ExecContext {
            execution_id: id.clone(),
            session_id: None,
            agent,
            initializer_mode: None,
            executor: ExecutorKind::Harness(harness),
            workspace_root: dir.join("ws"),
            repo_root: repo,
            project_key: "probe".into(),
            session_dir: session_root.clone(),
            session_root,
            report_file: None,
            phase: None,
            yolo: false,
            permission_mode: PermissionMode::Default,
            permissions: PermissionRules::default(),
            protected_paths: vec![],
            memory_db: dir.join("memory.sqlite3"),
        },
        resume: resume_sid.map(|sid| ResumeInfo {
            from: ExecutionId::new(),
            native_session_id: Some(sid),
        }),
        harness_session_id: None,
    };
    let transcript = ostra_core::paths::terminal_transcript(&spec.ctx.session_root, id.as_str());
    let host = Arc::new(Host(log.clone()));
    let result = exec.run(spec, host, CancellationToken::new()).await;
    let _ = std::fs::copy(transcript, dir.join(format!("{harness}{suffix}-terminal.bin")));
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}
