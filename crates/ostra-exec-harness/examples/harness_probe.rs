//! Live probe for one harness: `harness_probe run <harness> <model> <scratch-dir>`.
//! Serves `/internal/policy` and `/internal/mcp` with an allow-all policy that logs every payload,
//! runs one QuickAnswer execution in the harness, and prints the result. The same binary serves
//! as the `hook` and `mcp-stdio` subcommands the harness calls back.
//!
//! `harness_probe pause <harness> <model> <scratch-dir>` checks pause and continue the way the engine
//! does them: it cancels the run during a slow step, then resumes the harness session with the note
//! "Continue the workflow." and checks the task finishes.
//!
//! `harness_probe wake <harness> <model> <scratch-dir>` checks the subagent wait (Rule H2): the agent asks twice with
//! `subagent_ask`, the process stays alive while it waits, the answer is typed into its terminal after a delay, and
//! the agent must submit both answers.
//!
//! With `PROBE_CODE` set, the repo gets a small Rust project, the bridge serves the real code
//! navigation tools over an index of it, and the agent is asked to call `code_implementations`.

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

/// The log, the code index for `PROBE_CODE`, and whether the subagent tools are served (`wake`).
struct Services(Arc<Log>, Option<Code>, bool);

/// The code navigation tools over the probe repo, served the way the server's bridge serves them.
struct Code {
    root: PathBuf,
    list: Arc<ostra_core::api::FileIndex>,
    indexes: ostra_code::Indexes<()>,
}

const CODE_FILES: &[(&str, &str)] = &[
    ("Cargo.toml", "[package]\nname = \"shapes\"\n"),
    (
        "src/lib.rs",
        "pub mod shape;\npub mod circle;\npub mod square;\n",
    ),
    (
        "src/shape.rs",
        "pub trait Shape {\n    fn area(&self) -> f64;\n}\n",
    ),
    (
        "src/circle.rs",
        "use crate::shape::Shape;\npub struct Circle { pub r: f64 }\nimpl Shape for Circle {\n    fn area(&self) -> f64 { 3.14 * self.r * self.r }\n}\n",
    ),
    (
        "src/square.rs",
        "use crate::shape::Shape;\npub struct Square { pub side: f64 }\nimpl Shape for Square {\n    fn area(&self) -> f64 { self.side * self.side }\n}\n",
    ),
];

#[async_trait::async_trait]
impl BridgeServices for Services {
    fn authorize(&self, _: &ExecutionId, _: &str) -> bool {
        true
    }
    fn policy_check(&self, _: &ExecutionId, call: &ToolCall) -> PolicyDecision {
        self.0.line(json!({"check": call}));
        if std::env::var("PROBE_INSPECT").is_ok() {
            return PolicyDecision::deny(
                RuleRef::guard("read-only-session"),
                "Answer from what you already did, without tools: this is a read-only look back at a run that has ended, so Ostra refuses every tool call.",
            );
        }
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
    async fn mcp_call(
        &self,
        _: &ExecutionId,
        tool: &str,
        args: Value,
    ) -> Result<ostra_exec_harness::McpOut, String> {
        self.0.line(json!({"mcp_call": tool, "args": args}));
        // Rule H2: an ask makes the run wait with its process alive, as the engine's answer does.
        if self.2 && tool == "subagent_ask" {
            return Ok(ostra_exec_harness::McpOut {
                text: ostra_core::coord::waiting_text("the explore helper", true),
                end: ostra_core::coord::RunEnd::Wait,
            });
        }
        let Some(code) = &self.1 else {
            return Ok(ostra_exec_harness::McpOut::text("ok"));
        };
        let native = ostra_core::agent::CODE_TOOLS
            .iter()
            .find(|(op, _)| format!("code_{op}") == tool)
            .map(|(_, n)| *n)
            .ok_or_else(|| format!("unknown tool {tool}"))?;
        let out = code.indexes.with(&(), &code.root, &code.list, |ix| {
            ostra_code::tools::run(ix, native, &args)
        });
        self.0.line(json!({"mcp_result": out}));
        out.map(ostra_exec_harness::McpOut::text)
    }
    fn mcp_tools(&self, _: &ExecutionId) -> Vec<(String, String, Value)> {
        if self.2 {
            return ostra_tools::definitions(&[Capability::Coordinate])
                .into_iter()
                .filter_map(|d| {
                    ostra_core::coord::COORD_TOOLS
                        .iter()
                        .find(|(_, n)| *n == d.name)
                        .map(|(name, _)| (name.to_string(), d.description, d.input_schema))
                })
                .collect();
        }
        if self.1.is_none() {
            return vec![];
        }
        ostra_tools::definitions(&[Capability::Code])
            .into_iter()
            .filter_map(|d| {
                ostra_core::agent::CODE_TOOLS
                    .iter()
                    .find(|(_, n)| *n == d.name)
                    .map(|(op, _)| (format!("code_{op}"), d.description, d.input_schema))
            })
            .collect()
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

struct Host(Arc<Log>, Arc<Mutex<Option<String>>>);

#[async_trait::async_trait]
impl ExecutionHost for Host {
    fn emit(&self, delta: ExecutionDelta) {
        eprintln!("delta: {delta:?}");
        if let ExecutionDelta::NativeSessionId { id } = &delta {
            *self.1.lock().unwrap() = Some(id.clone());
        }
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
        Some("pause") => return pause_probe(&args).await,
        Some("inspect") => return inspect_probe(&args).await,
        Some("wake") => return wake_probe(&args).await,
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
    let code = std::env::var("PROBE_CODE").is_ok().then(|| {
        for (p, text) in CODE_FILES {
            let path = repo.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Code {
            root: ostra_core::paths::canonical(&repo).unwrap(),
            list: Arc::new(ostra_core::api::FileIndex {
                paths: CODE_FILES.iter().map(|(p, _)| p.to_string()).collect(),
                truncated: false,
            }),
            indexes: ostra_code::Indexes::default(),
        }
    });
    let code_probe = code.is_some();
    let harden = std::env::var("PROBE_HARDEN").is_ok();
    if harden {
        harden_setup(&repo, log.clone());
    }
    let live = LiveRegistry::new();
    let bridge = HarnessBridge::new(Arc::new(Services(log.clone(), code, false)), live.clone());
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
    let socket = harden.then(|| dir.join("bridge.sock"));
    #[cfg(unix)]
    let unix_socket = socket.clone();
    #[cfg(not(unix))]
    let unix_socket: Option<PathBuf> = None;
    if let Some(sock) = &unix_socket {
        // The bridge answers only on the socket, as the server's does; the TCP port logs any hit.
        let _ = std::fs::remove_file(sock);
        #[cfg(unix)]
        {
            let unix = tokio::net::UnixListener::bind(sock).unwrap();
            tokio::spawn(async move { axum::serve(unix, router).await.unwrap() });
        }
        let trap_log = log.clone();
        let trap = Router::new().fallback(move |uri: axum::http::Uri| {
            let log = trap_log.clone();
            async move {
                log.line(json!({"tcp_bridge_hit": uri.to_string()}));
                axum::http::StatusCode::IM_A_TEAPOT
            }
        });
        tokio::spawn(async move { axum::serve(listener, trap).await.unwrap() });
    } else {
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    }

    let mut cfg = HarnessExecutorConfig::new(
        GlobalConfig::default(),
        std::env::current_exe().unwrap(),
        url,
    );
    cfg.idle_nudge = std::time::Duration::from_secs(90);
    cfg.bridge_socket = socket;
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
        first_message: if harden {
            format!(
                "Do exactly these steps and nothing more:\n1. Run the shell command `cd {} && sh checks.sh > checks.txt 2>&1` and wait for it to finish (about 15 seconds).\n2. Call the `{}` tool with answer \"probe ok\" and sources [\"checks.txt\"].\nThen end your turn.",
                repo.display(),
                agent.submit_tool_name()
            )
        } else if code_probe {
            format!(
                "Do exactly these steps and nothing more:\n1. Call the `code_implementations` tool with symbol \"Shape\".\n2. Call the `{}` tool with answer set to the names of the types the tool listed under \"implemented or extended by\", comma-separated, and sources [\"src/shape.rs\"].\nThen end your turn.",
                agent.submit_tool_name()
            )
        } else {
            format!(
                "Do exactly these steps and nothing more:\n{}1. Run the shell command `echo ostra-probe > probe.txt`.\n2. Call the `{}` tool with answer \"probe ok\" and sources [\"probe.txt\"].\nThen end your turn.",
                if std::env::var("PROBE_DENY").is_ok() {
                    "0. Run the shell command `echo forbidden`. Ostra may refuse it; if so, continue.\n"
                } else {
                    ""
                },
                agent.submit_tool_name()
            )
        },
        capabilities: if code_probe {
            vec![Capability::Read, Capability::Code]
        } else {
            vec![Capability::Read, Capability::Shell]
        },
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
            sandbox_mode: None,
            sandbox_network: None,
            sandbox_allowed_hosts: vec![],
            sandbox_decoys: vec![],
            // `PROBE_LOOPBACK=listed` closes the host's loopback on macOS, as a workspace can.
            sandbox_loopback: if std::env::var("PROBE_LOOPBACK").as_deref() == Ok("listed") {
                ostra_core::config::LoopbackAccess::Listed
            } else {
                Default::default()
            },
            sandbox_blocked_ports: vec![],
            creates_project: false,
            answer_only: false,
            owes_reply: false,
        },
        resume: resume_sid.map(|sid| ResumeInfo {
            from: ExecutionId::new(),
            native_session_id: Some(sid),
            note: None,
            inspect: false,
        }),
        harness_session_id: None,
    };
    let transcript = ostra_core::paths::terminal_transcript(&spec.ctx.session_root, id.as_str());
    let host = Arc::new(Host(log.clone(), Arc::new(Mutex::new(None))));
    let result = exec.run(spec, host, CancellationToken::new()).await;
    let _ = std::fs::copy(
        transcript,
        dir.join(format!("{harness}{suffix}-terminal.bin")),
    );
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    if harden {
        let checks = dir.join("repo/checks.txt");
        println!(
            "---- checks.txt ----\n{}",
            std::fs::read_to_string(checks).unwrap_or_else(|e| format!("missing: {e}"))
        );
        println!(
            "commondir after the run: {}",
            dir.join("repo/.git/commondir").exists()
        );
    }
}

/// Host services the sandbox must not reach, the check script, and a git repo to plant files in.
fn harden_setup(repo: &std::path::Path, log: Arc<Log>) {
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(status.success());
    let _ = std::fs::remove_file(repo.join(".git/commondir"));
    let _ = std::fs::remove_file(repo.join("checks.txt"));

    let tcp = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = tcp.local_addr().unwrap().port();
    let lan = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("8.8.8.8:80").and_then(|_| s.local_addr()))
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into());
    let l = log.clone();
    std::thread::spawn(move || {
        for mut s in tcp.incoming().flatten() {
            l.line(json!({"CANARY_TCP_REACHED": s.peer_addr().ok().map(|a| a.to_string())}));
            // An answer, so the script's curl reports a connection that got through.
            let _ = std::io::Write::write_all(
                &mut s,
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let process = process_checks(log);

    let script = format!(
        r#"echo "== process"
{process}
echo "== proxy env"
env | grep -iE '^(https?|all|no)_proxy=' | sort
echo "== host services"
curl -s -m 3 --noproxy '*' -o /dev/null http://127.0.0.1:{port}/ && echo "host loopback: REACHED" || echo "host loopback: unreachable"
curl -s -m 3 --noproxy '*' -o /dev/null http://{lan}:{port}/ && echo "host lan: REACHED" || echo "host lan: unreachable"
curl -s -m 3 --noproxy '*' -o /dev/null http://169.254.169.254/ && echo "metadata direct: REACHED" || echo "metadata direct: unreachable"
python3 -c 'import socket; print("dns example.com:", socket.gethostbyname("example.com"))' 2>/dev/null || echo "dns example.com: refused"
echo "== egress"
curl -s -m 10 --noproxy '*' -o /dev/null https://crates.io/ && echo "direct crates.io: REACHED" || echo "direct crates.io: blocked"
echo "allowed index.crates.io: $(curl -s -m 15 -o /dev/null -w '%{{http_code}}' https://index.crates.io/config.json)"
r=$(curl -s -m 15 -o /dev/null -w '%{{http_connect}}' https://example.com/); echo "unlisted example.com: connect=$r exit=$?"
r=$(curl -s -m 5 -o /dev/null --noproxy '' -x "$HTTPS_PROXY" -w '%{{http_code}}' http://127.0.0.1:{port}/); echo "proxy to host loopback: connect=$r exit=$?"
r=$(curl -s -m 5 -o /dev/null --noproxy '' -x "$HTTPS_PROXY" -w '%{{http_code}}' http://169.254.169.254/); echo "proxy to metadata: code=$r exit=$?"
r=$(curl -s -m 15 -o /dev/null -w '%{{http_code}}' --connect-to crates.io:443:crates.io:443 https://crates.io/); echo "sni control crates.io: code=$r exit=$?"
r=$(curl -s -m 15 -o /dev/null -k -w '%{{http_code}}' --connect-to example.com:443:crates.io:443 https://example.com/); echo "sni mismatch: code=$r exit=$?"
echo "== git metadata"
(echo x >> .git/config) 2>/dev/null && echo ".git/config: WRITABLE" || echo ".git/config: refused"
(echo ../.. > .git/commondir) 2>/dev/null && echo "commondir planted" || echo "commondir: refused"
sleep 5
test -e .git/commondir && echo "commondir: STILL THERE" || echo "commondir: removed"
echo "== decoys"
head -c 36 ~/.ssh/id_rsa 2>&1; echo
head -c 20 ~/.git-credentials 2>&1; echo
echo "== done"
"#
    );
    std::fs::write(repo.join("checks.sh"), script).unwrap();
}

/// Kernel checks for the script, with the host canary they need: an abstract socket on Linux,
/// a process whose startup environment holds a secret on macOS.
#[cfg(target_os = "linux")]
fn process_checks(log: Arc<Log>) -> String {
    use std::os::linux::net::SocketAddrExt;
    let name = format!("ostra-probe-{}", std::process::id());
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
    let abs = std::os::unix::net::UnixListener::bind_addr(&addr).unwrap();
    std::thread::spawn(move || {
        for _ in abs.incoming().flatten() {
            log.line(json!({"CANARY_ABSTRACT_REACHED": true}));
        }
    });
    format!(
        r#"grep -E '^(Seccomp|NoNewPrivs):' /proc/self/status
unshare -U true 2>&1 && echo "userns: ALLOWED" || echo "userns: refused"
python3 -c 'import fcntl,termios,os,glob
fd=None
for p in glob.glob("/proc/[0-9]*/fd/[012]"):
    try:
        f=os.open(p,os.O_RDWR|os.O_NOCTTY)
        if os.isatty(f): fd=f; break
        os.close(f)
    except OSError: pass
if fd is None: print("tiocsti: no terminal found")
else:
    try: fcntl.ioctl(fd, termios.TIOCSTI, b" "); print("tiocsti: ALLOWED")
    except OSError as e: print("tiocsti: refused", e)'
python3 -c 'import socket
s=socket.socket(socket.AF_UNIX)
try:
    s.connect("\0{name}"); print("abstract socket: REACHED")
except OSError as e: print("abstract socket: unreachable", e)'"#
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_checks(_log: Arc<Log>) -> String {
    "echo \"process checks: none on this OS\"".into()
}

#[cfg(target_os = "macos")]
fn process_checks(_log: Arc<Log>) -> String {
    let secret = format!("probe-secret-{}", rand_id());
    let canary = std::process::Command::new("/bin/sleep")
        .arg("600")
        .env("OSTRA_PROBE_SECRET", &secret)
        .spawn()
        .unwrap();
    let pid = canary.id();
    std::mem::forget(canary);
    format!(
        r#"python3 -c 'import ctypes
libc=ctypes.CDLL(None, use_errno=True)
mib=(ctypes.c_int*3)(1,49,{pid}); n=ctypes.c_size_t(0)
if libc.sysctl(mib,3,None,ctypes.byref(n),None,0): print("other process env: refused errno", ctypes.get_errno())
else:
    b=ctypes.create_string_buffer(n.value); libc.sysctl(mib,3,b,ctypes.byref(n),None,0)
    print("other process env: READ" if b"{secret}" in b.raw else "other process env: read without the secret")'
kill {pid} 2>/dev/null && echo "kill outside: ALLOWED" || echo "kill outside: refused"
launchctl submit -l dev.ostra.probe.{pid} -- /usr/bin/true 2>/dev/null && echo "launchd submit: ALLOWED" || echo "launchd submit: refused""#
    )
}

/// Pause and continue, as `Engine::pause_session` and `resume_session` drive an execution.
async fn pause_probe(args: &[String]) {
    let harness: HarnessKind = args[1].parse().unwrap();
    let model = args[2].clone();
    let dir = PathBuf::from(&args[3]);
    let repo = dir.join("repo");
    let session_root = dir.join("ws/.ostra/sessions/s_probe");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&session_root).unwrap();
    for f in ["one.txt", "two.txt", "three.txt"] {
        let _ = std::fs::remove_file(repo.join(f));
    }
    let log = Arc::new(Log(Mutex::new(
        std::fs::File::create(dir.join(format!("{harness}-pause-events.jsonl"))).unwrap(),
    )));
    let live = LiveRegistry::new();
    let bridge = HarnessBridge::new(Arc::new(Services(log.clone(), None, false)), live.clone());
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
    let exec = Arc::new(HarnessExecutor::new(cfg, live, PtyRegistry::new()));
    let agent = AgentName::QuickAnswer;
    let task = format!(
        "Do exactly these steps in order, one shell command per step, and nothing more:\n1. Run `echo one > one.txt`.\n2. Run `sleep 40 && echo two > two.txt`. It is slow; wait for it.\n3. Run `echo three > three.txt`.\n4. Call the `{}` tool with answer \"steps done\" and sources [\"three.txt\"].\nBefore each step, check whether its file already exists and skip the step if it does. Then end your turn.",
        agent.submit_tool_name()
    );
    // The runner chooses the session id up front for the harnesses that accept one.
    let upfront = matches!(harness, HarnessKind::Claude | HarnessKind::Grok)
        .then(|| format!("{:032x}", rand_id()))
        .map(|h| {
            format!(
                "{}-{}-4{}-a{}-{}",
                &h[0..8],
                &h[8..12],
                &h[13..16],
                &h[17..20],
                &h[20..32]
            )
        });
    let spec = |id: &ExecutionId, resume: Option<ResumeInfo>, sid: Option<String>| ExecutionSpec {
        id: id.clone(),
        agent,
        route: ResolvedRoute {
            executor: ExecutorKind::Harness(harness),
            model: model.clone(),
            tier: None,
        },
        effort: Effort::High,
        system_prompt: format!(
            "You are an Ostra probe agent. Follow the user's steps exactly and do nothing else.\n\nWhen the task is finished and you have called `{}`, reply with only `Done!` and nothing else, because Ostra reads your result from the submit call and any other text costs output tokens.",
            agent.submit_tool_name()
        ),
        first_message: task.clone(),
        capabilities: vec![Capability::Read, Capability::Shell],
        submit_schema: ostra_core::submit::submit_schema(agent),
        timeout_secs: 300,
        ctx: ExecContext {
            execution_id: id.clone(),
            session_id: None,
            agent,
            initializer_mode: None,
            executor: ExecutorKind::Harness(harness),
            workspace_root: dir.join("ws"),
            repo_root: repo.clone(),
            project_key: "probe".into(),
            session_dir: session_root.clone(),
            session_root: session_root.clone(),
            report_file: None,
            phase: None,
            yolo: false,
            permission_mode: PermissionMode::Default,
            permissions: PermissionRules::default(),
            protected_paths: vec![],
            memory_db: dir.join("memory.sqlite3"),
            sandbox_mode: None,
            sandbox_network: None,
            sandbox_allowed_hosts: vec![],
            sandbox_decoys: vec![],
            sandbox_loopback: Default::default(),
            sandbox_blocked_ports: vec![],
            creates_project: false,
            answer_only: false,
            owes_reply: false,
        },
        resume,
        harness_session_id: sid,
    };

    let first = ExecutionId::new();
    let seen = Arc::new(Mutex::new(None::<String>));
    let host = Arc::new(Host(log.clone(), seen.clone()));
    let cancel = CancellationToken::new();
    let run = {
        let (exec, cancel, s) = (
            exec.clone(),
            cancel.clone(),
            spec(&first, None, upfront.clone()),
        );
        tokio::spawn(async move { exec.run(s, host, cancel).await })
    };
    let started = std::time::Instant::now();
    while !repo.join("one.txt").exists() {
        if run.is_finished() || started.elapsed().as_secs() > 180 {
            println!("PAUSE-PROBE FAIL: step 1 never ran");
            std::process::exit(1);
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    // Inside the 40 second step.
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;
    eprintln!("probe: pausing");
    cancel.cancel();
    let paused = run.await.unwrap();
    let sid = paused
        .native_session_id
        .clone()
        .or_else(|| seen.lock().unwrap().clone())
        .or(upfront.clone());
    println!(
        "paused: status={:?} session={sid:?} two_exists={}",
        paused.status,
        repo.join("two.txt").exists()
    );
    let Some(sid) = sid else {
        println!("PAUSE-PROBE FAIL: no session id to resume");
        std::process::exit(1);
    };

    let second = ExecutionId::new();
    let resume = ResumeInfo {
        from: first,
        native_session_id: Some(sid.clone()),
        note: Some("Continue the workflow.".into()),
        inspect: false,
    };
    let upfront_again =
        matches!(harness, HarnessKind::Claude | HarnessKind::Grok).then(|| sid.clone());
    let host = Arc::new(Host(log.clone(), Arc::new(Mutex::new(None))));
    let t = std::time::Instant::now();
    let resumed = exec
        .run(
            spec(&second, Some(resume), upfront_again),
            host,
            CancellationToken::new(),
        )
        .await;
    let ok = resumed.status == ExecutionStatus::Ok
        && resumed.submit.is_some()
        && repo.join("three.txt").exists();
    println!(
        "resumed: status={:?} submit={} three_exists={} secs={}",
        resumed.status,
        resumed.submit.is_some(),
        repo.join("three.txt").exists(),
        t.elapsed().as_secs()
    );
    println!("{}", serde_json::to_string_pretty(&resumed).unwrap());
    println!("PAUSE-PROBE {}", if ok { "PASS" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}

fn rand_id() -> u128 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    t ^ ((std::process::id() as u128) << 64)
}

/// Reopen a session read-only, as `Engine::inspect_execution` does, ask a question that invites a
/// tool call, and report what the agent did. Run with `PROBE_INSPECT=1` so every call is denied.
async fn inspect_probe(args: &[String]) {
    let harness: HarnessKind = args[1].parse().unwrap();
    let model = args[2].clone();
    let dir = PathBuf::from(&args[3]);
    let sid = args[4].clone();
    let repo = dir.join("repo");
    let session_root = dir.join("ws/.ostra/sessions/s_probe");
    let log = Arc::new(Log(Mutex::new(
        std::fs::File::create(dir.join(format!("{harness}-inspect-events.jsonl"))).unwrap(),
    )));
    let live = LiveRegistry::new();
    let bridge = HarnessBridge::new(Arc::new(Services(log.clone(), None, false)), live.clone());
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
    let cfg = HarnessExecutorConfig::new(
        GlobalConfig::default(),
        std::env::current_exe().unwrap(),
        url,
    );
    let ptys = PtyRegistry::new();
    let exec = Arc::new(HarnessExecutor::new(cfg, live, ptys.clone()));
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
        system_prompt: "The user reopened this session to read what you did and ask about it. The work has ended. Answer from the conversation above and do not call any tool, because Ostra refuses every tool call in this session. Keep answers short.".into(),
        first_message: String::new(),
        capabilities: vec![],
        submit_schema: serde_json::Value::Null,
        timeout_secs: 600,
        ctx: ExecContext {
            execution_id: id.clone(),
            session_id: None,
            agent,
            initializer_mode: None,
            executor: ExecutorKind::Harness(harness),
            workspace_root: dir.join("ws"),
            repo_root: repo.clone(),
            project_key: "probe".into(),
            session_dir: session_root.clone(),
            session_root: session_root.clone(),
            report_file: None,
            phase: None,
            yolo: false,
            permission_mode: PermissionMode::Plan,
            permissions: PermissionRules::default(),
            protected_paths: vec![],
            memory_db: dir.join("memory.sqlite3"),
            sandbox_mode: None,
            sandbox_network: None,
            sandbox_allowed_hosts: vec![],
            sandbox_decoys: vec![],
            sandbox_loopback: Default::default(),
            sandbox_blocked_ports: vec![],
            creates_project: false,
            answer_only: false,
            owes_reply: false,
        },
        resume: Some(ResumeInfo {
            from: ExecutionId::new(),
            native_session_id: Some(sid),
            note: None,
            inspect: true,
        }),
        harness_session_id: None,
    };
    let host = Arc::new(Host(log.clone(), Arc::new(Mutex::new(None))));
    let cancel = CancellationToken::new();
    let run = {
        let (exec, cancel) = (exec.clone(), cancel.clone());
        tokio::spawn(async move { exec.run(spec, host, cancel).await })
    };
    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    let pty = ptys.get(&id).expect("a live terminal");
    let before = pty.screen_text();
    let _ = pty.type_line("Run `cat three.txt` and tell me what it contains. Also, which steps did you run earlier?").await;
    tokio::time::sleep(std::time::Duration::from_secs(75)).await;
    let after = pty.screen_text();
    cancel.cancel();
    let result = run.await.unwrap();
    let checks = std::fs::read_to_string(dir.join(format!("{harness}-inspect-events.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("{\"check\""))
        .count();
    println!(
        "---- screen before typing ----\n{before}\n---- screen after the question ----\n{after}"
    );
    println!("status={:?} denied_tool_calls={checks}", result.status);
}

/// Answers each wait of the wake probe after a delay, the way the engine hands over a helper's answer.
struct WakeHost {
    log: Arc<Log>,
    notes: Mutex<Vec<String>>,
    waits: Mutex<Vec<(std::time::Instant, f64)>>,
    started: std::time::Instant,
}

#[async_trait::async_trait]
impl ExecutionHost for WakeHost {
    fn emit(&self, delta: ExecutionDelta) {
        eprintln!("delta: {delta:?}");
        self.log.line(json!({"delta": delta}));
    }
    async fn ask_permission(&self, _: &ToolCall, _: &str, _: &RuleRef) -> PermissionAnswer {
        PermissionAnswer::AllowOnce
    }
    async fn wait_for_wake(&self) -> Option<Wake> {
        let at = self.started.elapsed().as_secs_f64();
        eprintln!("probe: the run waits ({at:.0}s in)");
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        self.waits.lock().unwrap().push((std::time::Instant::now(), at));
        let note = {
            let mut n = self.notes.lock().unwrap();
            (!n.is_empty()).then(|| n.remove(0))
        };
        self.log.line(json!({"wake": note}));
        note.map(|note| Wake {
            note,
            owes_reply: false,
        })
    }
}

async fn wake_probe(args: &[String]) {
    let harness: HarnessKind = args[1].parse().unwrap();
    let model = args[2].clone();
    let dir = PathBuf::from(&args[3]);
    let repo = dir.join("repo");
    let session_root = dir.join("ws/.ostra/sessions/s_probe");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&session_root).unwrap();
    let log = Arc::new(Log(Mutex::new(
        std::fs::File::create(dir.join(format!("{harness}-wake-events.jsonl"))).unwrap(),
    )));
    let live = LiveRegistry::new();
    let bridge = HarnessBridge::new(Arc::new(Services(log.clone(), None, true)), live.clone());
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
    let agent = AgentName::QuickAnswer;
    let ask = if harness == HarnessKind::Claude {
        "mcp__ostra__subagent_ask"
    } else {
        "subagent_ask"
    };
    let submit = if harness == HarnessKind::Claude {
        format!("mcp__ostra__{}", agent.submit_tool_name())
    } else {
        agent.submit_tool_name()
    };
    let id = ExecutionId::new();
    let spec = ExecutionSpec {
        id: id.clone(),
        agent,
        route: ResolvedRoute {
            executor: ExecutorKind::Harness(harness),
            model,
            tier: None,
        },
        effort: Effort::Low,
        system_prompt: format!(
            "You are an Ostra probe agent. Follow the user's steps exactly and do nothing else.\n\nThe `mcp__ostra__*` tools come from Ostra's `ostra` MCP server. If one is not in your tool list yet, load it with ToolSearch (`select:mcp__ostra__<tool>`) and then call it.\n\nWhen you call `{ask}`, Ostra answers later: end your turn as the tool result says, and the answer arrives as the next message. When the task is finished and you have called `{submit}`, reply with only `Done!`."
        ),
        first_message: format!(
            "Do exactly these steps in order and nothing more:\n1. Call `{ask}` with agent \"explore\" and message \"What is the release codename?\"\n2. When its answer arrives, call `{ask}` with agent \"explore\" and message \"What is the release date?\"\n3. When that answer arrives, call `{submit}` with answer set to the codename and the date, and sources [\"explore helper\"]."
        ),
        capabilities: vec![Capability::Read, Capability::Coordinate],
        submit_schema: ostra_core::submit::submit_schema(agent),
        timeout_secs: 300,
        ctx: ExecContext {
            execution_id: id.clone(),
            session_id: None,
            agent,
            initializer_mode: None,
            executor: ExecutorKind::Harness(harness),
            workspace_root: dir.join("ws"),
            repo_root: repo.clone(),
            project_key: "probe".into(),
            session_dir: session_root.clone(),
            session_root: session_root.clone(),
            report_file: None,
            phase: None,
            yolo: false,
            permission_mode: PermissionMode::Default,
            permissions: PermissionRules::default(),
            protected_paths: vec![],
            memory_db: dir.join("memory.sqlite3"),
            sandbox_mode: None,
            sandbox_network: None,
            sandbox_allowed_hosts: vec![],
            sandbox_decoys: vec![],
            sandbox_loopback: Default::default(),
            sandbox_blocked_ports: vec![],
            creates_project: false,
            answer_only: false,
            owes_reply: false,
        },
        resume: None,
        harness_session_id: None,
    };
    let answer = |q: &str, a: &str| {
        format!(
            "The answer from the explore helper to your question arrived. Continue your task from here.\n\nYour question: {q}\n\nAnswer:\n{a}"
        )
    };
    let host = Arc::new(WakeHost {
        log: log.clone(),
        notes: Mutex::new(vec![
            answer(
                "What is the release codename?",
                "The release codename is BLUE-HERON-7.",
            ),
            answer("What is the release date?", "The release date is 2026-10-14."),
        ]),
        waits: Mutex::new(vec![]),
        started: std::time::Instant::now(),
    });
    let transcript = ostra_core::paths::terminal_transcript(&spec.ctx.session_root, id.as_str());
    let result = exec.run(spec, host.clone(), CancellationToken::new()).await;
    let _ = std::fs::copy(transcript, dir.join(format!("{harness}-wake-terminal.bin")));
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    let text = result
        .submit
        .as_ref()
        .map(|s| s.to_string())
        .unwrap_or_default();
    let waits = host.waits.lock().unwrap().len();
    let ok = result.status == ExecutionStatus::Ok
        && text.contains("BLUE-HERON-7")
        && text.contains("2026-10-14")
        && waits == 2;
    println!(
        "WAKE-PROBE {}: status {:?}, waits {waits}, submit {text}",
        if ok { "PASS" } else { "FAIL" },
        result.status
    );
    if !ok {
        std::process::exit(1);
    }
}
