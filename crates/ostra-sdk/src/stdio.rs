//! Rule PL1: the stdio transport between Ostra and an out-of-process plugin: newline-delimited
//! JSON-RPC 2.0 in both directions.
//!
//! Ostra calls the plugin:
//! - `initialize {protocol}` returns the [`PluginManifest`].
//! - `stage/decide {stage, view}` returns a [`StageDecision`].
//! - `agent/run {agent, task}` returns an [`AgentOutcome`]. While it runs, the plugin calls Ostra
//!   back with `host/tool {execution, name, input}` (a [`ToolReply`]) and
//!   `host/complete {execution, request}` (`{text}`), and notifies `host/status {execution, text}`.
//! - `agent/cancel {execution}` tells the plugin a run was stopped.
//! - `result/handle {contract, result}` returns the outcome (a `CustomSubmit`) of one result of
//!   the plugin's own contract (Rule PL5).
//! - `transform/run {name, inputs, args}` returns the output of one of the plugin's transform
//!   functions (Rule PL7).
//!
//! Rule PL8: `stage/decide`, `result/handle`, and `agent/run` also carry `call`, a token for the
//! call, and `checkpoints`, what the plugin saved in the session. While the call runs, the plugin
//! saves with `host/checkpoint {call, key, value}`; a `null` value removes the key.
//!
//! A plugin program calls [`serve`] from its `main`; Ostra starts it with [`StdioPlugin::start`].

use crate::{
    AgentCalls, AgentOutcome, AgentTask, Checkpoints, CompleteRequest, CustomSubmit, Plugin,
    PluginManifest, ResultView, StageDecision, StageView, ToolReply,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// The protocol version Ostra speaks.
pub const PROTOCOL: u32 = 1;
/// The longest line either side handles; a longer one is read, then skipped.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// What one side answers to the other's requests.
#[async_trait::async_trait]
pub trait Handler: Send + Sync {
    async fn request(&self, method: &str, params: Value) -> Result<Value, String>;
    fn notify(&self, _method: &str, _params: Value) {}
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>;

/// One end of a JSON-RPC connection: it sends requests and answers the other end's.
pub struct Peer {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next: AtomicU64,
    closed: AtomicBool,
}

impl Peer {
    /// Read requests and answers from `reader`, write to `writer`, and answer requests with
    /// `handler`, each on its own task so a long request never blocks the others.
    pub fn spawn<R, W>(reader: R, writer: W, handler: Arc<dyn Handler>) -> Arc<Peer>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let peer = Arc::new(Peer {
            out: tx,
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        });
        let mut writer = writer;
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if writer.write_all(line.as_bytes()).await.is_err()
                    || writer.write_all(b"\n").await.is_err()
                    || writer.flush().await.is_err()
                {
                    break;
                }
            }
        });
        let weak = Arc::downgrade(&peer);
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Some(peer) = weak.upgrade() else { break };
                if line.len() > MAX_LINE || line.trim().is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                peer.dispatch(msg, handler.clone());
            }
            if let Some(peer) = weak.upgrade() {
                peer.close("the other side closed the connection");
            }
        });
        peer
    }

    fn close(&self, why: &str) {
        self.closed.store(true, Ordering::SeqCst);
        for (_, tx) in self.pending.lock().drain() {
            let _ = tx.send(Err(why.to_string()));
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn dispatch(self: &Arc<Self>, msg: Value, handler: Arc<dyn Handler>) {
        let method = msg.get("method").and_then(Value::as_str).map(String::from);
        let id = msg.get("id").cloned().filter(|v| !v.is_null());
        match (method, id) {
            (Some(method), Some(id)) => {
                let out = self.out.clone();
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                tokio::spawn(async move {
                    let reply = match handler.request(&method, params).await {
                        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                        Err(message) => {
                            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": message}})
                        }
                    };
                    let _ = out.send(reply.to_string());
                });
            }
            (Some(method), None) => {
                handler.notify(&method, msg.get("params").cloned().unwrap_or(Value::Null))
            }
            (None, Some(id)) => {
                let Some(n) = id.as_u64() else { return };
                let Some(tx) = self.pending.lock().remove(&n) else {
                    return;
                };
                let r = match msg.get("error") {
                    Some(e) => Err(e
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the request failed")
                        .to_string()),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(r);
            }
            (None, None) => {}
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        if self.is_closed() {
            return Err("the connection is closed".into());
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self.out.send(line.to_string()).is_err() {
            self.pending.lock().remove(&id);
            return Err("the connection is closed".into());
        }
        rx.await
            .unwrap_or_else(|_| Err("the connection is closed".into()))
    }

    pub fn notify(&self, method: &str, params: Value) {
        let line = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let _ = self.out.send(line.to_string());
    }
}

fn parse<T: serde::de::DeserializeOwned>(v: Value, what: &str) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| format!("{what} is malformed: {e}"))
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------
// The plugin's side
// ---------------------------------------------------------------------------------------------

/// Serve `plugin` on stdin and stdout until Ostra closes the connection. Call it from the plugin
/// program's `main` inside a Tokio runtime. Write logs to stderr: stdout carries the protocol.
pub async fn serve(plugin: Arc<dyn Plugin>) {
    let handler = Arc::new(PluginSide {
        plugin,
        peer: OnceLock::new(),
    });
    let peer = Peer::spawn(tokio::io::stdin(), tokio::io::stdout(), handler.clone());
    let _ = handler.peer.set(Arc::downgrade(&peer));
    while !peer.is_closed() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

struct PluginSide {
    plugin: Arc<dyn Plugin>,
    peer: OnceLock<Weak<Peer>>,
}

#[async_trait::async_trait]
impl Handler for PluginSide {
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "initialize" => to_value(&self.plugin.manifest()),
            "stage/decide" => {
                let stage = params["stage"].as_str().unwrap_or_default().to_string();
                let view: StageView = parse(params["view"].clone(), "the stage view")?;
                let store = self.checkpoints(&params)?;
                to_value(&self.plugin.decide_stage(&stage, view, store).await?)
            }
            "transform/run" => {
                let name = params["name"].as_str().unwrap_or_default().to_string();
                let map = |v: &Value| v.as_object().cloned().unwrap_or_default();
                self.plugin
                    .transform(&name, map(&params["inputs"]), map(&params["args"]))
                    .await
            }
            "result/handle" => {
                let contract = params["contract"].as_str().unwrap_or_default().to_string();
                let result: ResultView = parse(params["result"].clone(), "the result")?;
                let store = self.checkpoints(&params)?;
                to_value(&self.plugin.handle_result(&contract, result, store).await?)
            }
            "agent/run" => {
                let agent = params["agent"].as_str().unwrap_or_default().to_string();
                let task: AgentTask = parse(params["task"].clone(), "the agent task")?;
                let store = self.checkpoints(&params)?;
                let calls = Arc::new(RemoteCalls {
                    peer: self.peer()?,
                    execution: task.execution.to_string(),
                });
                to_value(&self.plugin.run_agent(&agent, task, calls, store).await?)
            }
            other => Err(format!("unknown method `{other}`")),
        }
    }
}

impl PluginSide {
    fn peer(&self) -> Result<Arc<Peer>, String> {
        self.peer
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| "the connection is closed".to_string())
    }

    /// Rule PL8: the checkpoints a call brought, saved back through `host/checkpoint`.
    fn checkpoints(&self, params: &Value) -> Result<Arc<dyn Checkpoints>, String> {
        let kept: BTreeMap<String, Value> = match params.get("checkpoints") {
            Some(v) if !v.is_null() => parse(v.clone(), "the checkpoints")?,
            _ => BTreeMap::new(),
        };
        Ok(Arc::new(RemoteCheckpoints {
            peer: self.peer()?,
            call: params["call"].as_str().unwrap_or_default().to_string(),
            kept: Mutex::new(kept),
        }))
    }
}

/// Rule PL8: a call's checkpoints on the plugin's side: a copy it reads, and saves sent to Ostra.
struct RemoteCheckpoints {
    peer: Arc<Peer>,
    call: String,
    kept: Mutex<BTreeMap<String, Value>>,
}

#[async_trait::async_trait]
impl Checkpoints for RemoteCheckpoints {
    fn all(&self) -> BTreeMap<String, Value> {
        self.kept.lock().clone()
    }

    async fn write(&self, key: &str, value: Option<Value>) -> Result<(), String> {
        self.peer
            .request(
                "host/checkpoint",
                json!({"call": self.call, "key": key, "value": value}),
            )
            .await?;
        let mut kept = self.kept.lock();
        match value {
            Some(v) => kept.insert(key.to_string(), v),
            None => kept.remove(key),
        };
        Ok(())
    }
}

/// A programmatic agent's calls, sent back to Ostra.
struct RemoteCalls {
    peer: Arc<Peer>,
    execution: String,
}

#[async_trait::async_trait]
impl AgentCalls for RemoteCalls {
    async fn tool(&self, name: &str, input: Value) -> ToolReply {
        match self
            .peer
            .request(
                "host/tool",
                json!({"execution": self.execution, "name": name, "input": input}),
            )
            .await
            .and_then(|v| parse::<ToolReply>(v, "the tool reply"))
        {
            Ok(r) => r,
            Err(e) => ToolReply {
                output: e,
                is_error: true,
            },
        }
    }

    async fn complete(&self, request: CompleteRequest) -> Result<String, String> {
        let v = self
            .peer
            .request(
                "host/complete",
                json!({"execution": self.execution, "request": request}),
            )
            .await?;
        Ok(v["text"].as_str().unwrap_or_default().to_string())
    }

    fn status(&self, text: &str) {
        self.peer.notify(
            "host/status",
            json!({"execution": self.execution, "text": text}),
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Ostra's side
// ---------------------------------------------------------------------------------------------

/// An out-of-process plugin, as Ostra sees it: a [`Plugin`] whose calls go over stdio.
type Stores = Arc<Mutex<HashMap<String, Arc<dyn Checkpoints>>>>;

pub struct StdioPlugin {
    manifest: PluginManifest,
    peer: Arc<Peer>,
    runs: Arc<Mutex<HashMap<String, Arc<dyn AgentCalls>>>>,
    /// Rule PL8: the checkpoints of each call in progress, by call token.
    stores: Stores,
    next_call: AtomicU64,
    decide_timeout: Duration,
    stderr: Arc<Mutex<String>>,
    _child: Mutex<tokio::process::Child>,
}

/// Bytes of a plugin's stderr kept to explain a failure.
const STDERR_TAIL: usize = 4096;

impl StdioPlugin {
    /// Start `command` in `cwd` and read its manifest. The process stops when this is dropped.
    pub async fn start(
        command: &[String],
        env: &[(String, String)],
        cwd: &Path,
        decide_timeout: Duration,
    ) -> Result<StdioPlugin, String> {
        let (program, args) = command.split_first().ok_or("the plugin has no command")?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start `{program}`: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr_pipe = child.stderr.take().ok_or("no stderr")?;
        let stderr = Arc::new(Mutex::new(String::new()));
        let tail = stderr.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr_pipe).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                tracing::debug!("plugin stderr: {l}");
                let mut t = tail.lock();
                t.push_str(&l);
                t.push('\n');
                if t.len() > STDERR_TAIL {
                    let mut cut = t.len() - STDERR_TAIL;
                    while !t.is_char_boundary(cut) {
                        cut += 1;
                    }
                    t.drain(..cut);
                }
            }
        });
        let runs: Arc<Mutex<HashMap<String, Arc<dyn AgentCalls>>>> = Arc::default();
        let stores: Stores = Arc::default();
        let peer = Peer::spawn(
            stdout,
            stdin,
            Arc::new(HostSide {
                runs: runs.clone(),
                stores: stores.clone(),
            }),
        );
        let manifest = tokio::time::timeout(
            decide_timeout,
            peer.request("initialize", json!({ "protocol": PROTOCOL })),
        )
        .await
        .map_err(|_| "the plugin did not answer `initialize` in time".to_string())
        .and_then(|r| r)
        .and_then(|v| parse::<PluginManifest>(v, "the plugin's manifest"))
        .map_err(|e| {
            let err = stderr.lock().trim().to_string();
            if err.is_empty() {
                e
            } else {
                format!("{e}. Its stderr: {err}")
            }
        })?;
        Ok(StdioPlugin {
            manifest,
            peer,
            runs,
            stores,
            next_call: AtomicU64::new(1),
            decide_timeout,
            stderr,
            _child: Mutex::new(child),
        })
    }

    pub fn stderr_tail(&self) -> String {
        self.stderr.lock().clone()
    }

    /// Rule PL8: register a call's checkpoints under a fresh token, until the guard drops.
    fn open(&self, checkpoints: Arc<dyn Checkpoints>) -> CallGuard {
        let call = format!("c{}", self.next_call.fetch_add(1, Ordering::SeqCst));
        let kept = checkpoints.all();
        self.stores.lock().insert(call.clone(), checkpoints);
        CallGuard {
            stores: self.stores.clone(),
            call,
            kept,
        }
    }
}

/// Removes a call's checkpoints when the call ends.
struct CallGuard {
    stores: Stores,
    call: String,
    kept: BTreeMap<String, Value>,
}

impl CallGuard {
    fn params(&self, mut params: Value) -> Value {
        params["call"] = json!(self.call);
        params["checkpoints"] = json!(self.kept);
        params
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        self.stores.lock().remove(&self.call);
    }
}

struct HostSide {
    runs: Arc<Mutex<HashMap<String, Arc<dyn AgentCalls>>>>,
    stores: Stores,
}

impl HostSide {
    fn run(&self, params: &Value) -> Result<Arc<dyn AgentCalls>, String> {
        let id = params["execution"].as_str().unwrap_or_default();
        self.runs
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| format!("no run `{id}` of this plugin is in progress"))
    }
}

#[async_trait::async_trait]
impl Handler for HostSide {
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "host/tool" => {
                let run = self.run(&params)?;
                let name = params["name"].as_str().unwrap_or_default();
                to_value(&run.tool(name, params["input"].clone()).await)
            }
            "host/complete" => {
                let run = self.run(&params)?;
                let req: CompleteRequest = parse(params["request"].clone(), "the request")?;
                Ok(json!({ "text": run.complete(req).await? }))
            }
            "host/checkpoint" => {
                let call = params["call"].as_str().unwrap_or_default();
                let store = self
                    .stores
                    .lock()
                    .get(call)
                    .cloned()
                    .ok_or_else(|| format!("no call `{call}` of this plugin is in progress"))?;
                let key = params["key"].as_str().ok_or("the checkpoint has no key")?;
                let value = Some(params["value"].clone()).filter(|v| !v.is_null());
                store.write(key, value).await?;
                Ok(json!({}))
            }
            other => Err(format!("unknown method `{other}`")),
        }
    }

    fn notify(&self, method: &str, params: Value) {
        if method == "host/status"
            && let Ok(run) = self.run(&params)
        {
            run.status(params["text"].as_str().unwrap_or_default());
        }
    }
}

/// Removes a run's calls when it ends, and tells the plugin when it was stopped.
struct RunGuard {
    runs: Arc<Mutex<HashMap<String, Arc<dyn AgentCalls>>>>,
    peer: Arc<Peer>,
    execution: String,
    done: bool,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.runs.lock().remove(&self.execution);
        if !self.done {
            self.peer
                .notify("agent/cancel", json!({ "execution": self.execution }));
        }
    }
}

#[async_trait::async_trait]
impl Plugin for StdioPlugin {
    fn manifest(&self) -> PluginManifest {
        self.manifest.clone()
    }

    fn is_alive(&self) -> bool {
        !self.peer.is_closed()
    }

    async fn decide_stage(
        &self,
        stage: &str,
        view: StageView,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<StageDecision, String> {
        let call = self.open(checkpoints);
        let v = tokio::time::timeout(
            self.decide_timeout,
            self.peer.request(
                "stage/decide",
                call.params(json!({ "stage": stage, "view": view })),
            ),
        )
        .await
        .map_err(|_| format!("the plugin did not decide stage `{stage}` in time"))??;
        parse(v, "the stage decision")
    }

    async fn transform(
        &self,
        name: &str,
        inputs: serde_json::Map<String, Value>,
        args: serde_json::Map<String, Value>,
    ) -> Result<Value, String> {
        tokio::time::timeout(
            self.decide_timeout,
            self.peer.request(
                "transform/run",
                json!({ "name": name, "inputs": inputs, "args": args }),
            ),
        )
        .await
        .map_err(|_| format!("the plugin did not run transform `{name}` in time"))?
    }

    async fn handle_result(
        &self,
        contract: &str,
        result: ResultView,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<CustomSubmit, String> {
        let call = self.open(checkpoints);
        let v = tokio::time::timeout(
            self.decide_timeout,
            self.peer.request(
                "result/handle",
                call.params(json!({ "contract": contract, "result": result })),
            ),
        )
        .await
        .map_err(|_| format!("the plugin did not handle a `{contract}` result in time"))??;
        parse(v, "the handled result")
    }

    async fn run_agent(
        &self,
        agent: &str,
        task: AgentTask,
        calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<AgentOutcome, String> {
        let call = self.open(checkpoints);
        let execution = task.execution.to_string();
        self.runs.lock().insert(execution.clone(), calls);
        let mut guard = RunGuard {
            runs: self.runs.clone(),
            peer: self.peer.clone(),
            execution,
            done: false,
        };
        let v = self
            .peer
            .request(
                "agent/run",
                call.params(json!({ "agent": agent, "task": task })),
            )
            .await;
        guard.done = true;
        parse(v?, "the agent's outcome")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryCheckpoints, PluginStage, pass};

    struct Echo;

    #[async_trait::async_trait]
    impl Plugin for Echo {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                workflows: vec![],
                transforms: vec![],
                name: "echo".into(),
                stages: vec![PluginStage {
                    name: "gate".into(),
                    description: String::new(),
                }],
                ..Default::default()
            }
        }

        async fn decide_stage(
            &self,
            _: &str,
            view: StageView,
            _: Arc<dyn Checkpoints>,
        ) -> Result<StageDecision, String> {
            Ok(match view.runs.len() {
                0 => StageDecision::Run {
                    agent: "auditor".into(),
                    instructions: Some(view.request),
                },
                _ => StageDecision::Pass {
                    summary: "done".into(),
                },
            })
        }

        async fn run_agent(
            &self,
            _: &str,
            task: AgentTask,
            calls: Arc<dyn AgentCalls>,
            checkpoints: Arc<dyn Checkpoints>,
        ) -> Result<AgentOutcome, String> {
            // Rule PL8: the run reads what was saved before it and saves its own step.
            let step = checkpoints
                .get("step")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            checkpoints.save("step", json!(step + 1)).await?;
            assert_eq!(checkpoints.get("step"), Some(json!(step + 1)));
            calls.status("reading");
            let r = calls.tool("Read", json!({"file_path": "a"})).await;
            let text = calls
                .complete(CompleteRequest {
                    system: "s".into(),
                    messages: vec![],
                    max_tokens: None,
                })
                .await?;
            Ok(pass(format!("{}|{}|{}|{step}", task.agent, r.output, text)))
        }
    }

    struct Fake {
        log: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl AgentCalls for Fake {
        async fn tool(&self, name: &str, _: Value) -> ToolReply {
            ToolReply {
                output: format!("{name} ok"),
                is_error: false,
            }
        }
        async fn complete(&self, _: CompleteRequest) -> Result<String, String> {
            Ok("model says hi".into())
        }
        fn status(&self, text: &str) {
            self.log.lock().push(text.into());
        }
    }

    /// Both ends over in-memory pipes, the way `serve` and `StdioPlugin` talk over stdio.
    #[tokio::test]
    async fn requests_and_callbacks_cross_both_ways() {
        let (host_io, plugin_io) = tokio::io::duplex(64 * 1024);
        let (pr, pw) = tokio::io::split(plugin_io);
        let side = Arc::new(PluginSide {
            plugin: Arc::new(Echo),
            peer: OnceLock::new(),
        });
        let plugin_peer = Peer::spawn(pr, pw, side.clone());
        let _ = side.peer.set(Arc::downgrade(&plugin_peer));

        let runs: Arc<Mutex<HashMap<String, Arc<dyn AgentCalls>>>> = Arc::default();
        let stores: Stores = Arc::default();
        let (hr, hw) = tokio::io::split(host_io);
        let host = Peer::spawn(
            hr,
            hw,
            Arc::new(HostSide {
                runs: runs.clone(),
                stores: stores.clone(),
            }),
        );
        let m: PluginManifest =
            parse(host.request("initialize", json!({})).await.unwrap(), "m").unwrap();
        assert_eq!(m.name, "echo");

        let fake = Arc::new(Fake {
            log: Mutex::new(vec![]),
        });
        runs.lock().insert("x_1".into(), fake.clone());
        let task = AgentTask {
            execution: "x_1".into(),
            session: None,
            agent: "auditor".into(),
            first_message: String::new(),
            repo_root: "/r".into(),
            session_dir: "/s".into(),
            workspace_root: "/w".into(),
            model: "mock:m".into(),
            tools: vec!["Read".into()],
            submit_schema: Value::Null,
            resumed: false,
        };
        let store = Arc::new(MemoryCheckpoints::new(BTreeMap::from([(
            "step".to_string(),
            json!(3),
        )])));
        stores.lock().insert("c1".into(), store.clone());
        let out: AgentOutcome = parse(
            host.request(
                "agent/run",
                json!({"agent": "auditor", "task": task, "call": "c1", "checkpoints": {"step": 3}}),
            )
            .await
            .unwrap(),
            "o",
        )
        .unwrap();
        assert_eq!(out.submit["summary"], "auditor|Read ok|model says hi|3");
        assert_eq!(
            store.get("step"),
            Some(json!(4)),
            "the save crossed back to Ostra's store"
        );
        assert_eq!(*fake.log.lock(), vec!["reading".to_string()]);
        let err = host.request("nope", json!({})).await.unwrap_err();
        assert!(err.contains("unknown method"), "{err}");
    }
}
