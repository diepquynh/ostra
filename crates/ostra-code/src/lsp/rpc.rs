//! JSON-RPC over the Language Server Protocol's `Content-Length` framing. One `Client` owns one
//! server connection: a writer task, a reader task that routes responses to their callers and
//! answers the few requests a server sends its client, and the server's progress state.

use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// A server message larger than this ends the connection.
const MAX_MESSAGE: usize = 256 * 1024 * 1024;
const STDERR_LINES: usize = 20;

/// LSP `ContentModified` and `ServerCancelled`: the server dropped the request while it loads.
const CONTENT_MODIFIED: i64 = -32801;
const SERVER_CANCELLED: i64 = -32802;

/// The two ends of a server connection, and the process when there is one.
pub struct Transport {
    pub read: Box<dyn AsyncRead + Send + Unpin>,
    pub write: Box<dyn AsyncWrite + Send + Unpin>,
    pub stderr: Option<Box<dyn AsyncRead + Send + Unpin>>,
    pub child: Option<tokio::process::Child>,
    /// Kills what the server leaves running in its sandbox once the connection is gone.
    pub sandbox: Option<ostra_sandbox::Members>,
}

type Reply = Result<Value, String>;

#[derive(Default)]
struct Shared {
    pending: Mutex<HashMap<i64, oneshot::Sender<Reply>>>,
    progress: Mutex<HashSet<String>>,
    /// rust-analyzer's `experimental/serverStatus`: false while it loads the workspace.
    quiescent: Mutex<Option<bool>>,
    dead: Mutex<Option<String>>,
    stderr: Mutex<VecDeque<String>>,
    folders: Mutex<Value>,
}

impl Shared {
    fn die(&self, why: String) {
        let mut dead = self.dead.lock();
        if dead.is_some() {
            return;
        }
        let tail = self.stderr.lock().back().cloned();
        let why = match tail {
            Some(t) => format!("{why} Its last output: {t}"),
            None => why,
        };
        for (_, tx) in self.pending.lock().drain() {
            let _ = tx.send(Err(why.clone()));
        }
        *dead = Some(why);
    }
}

pub struct Client {
    name: String,
    next: AtomicI64,
    out: mpsc::UnboundedSender<Vec<u8>>,
    shared: Arc<Shared>,
    child: Mutex<Option<tokio::process::Child>>,
    sandbox: Mutex<Option<ostra_sandbox::Members>>,
    closed: AtomicBool,
}

/// Cancels a request whose caller stopped waiting, by timeout or because its future was dropped.
struct CancelOnDrop<'a> {
    client: &'a Client,
    id: i64,
}

impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        if self.client.shared.pending.lock().remove(&self.id).is_some() {
            self.client
                .notify("$/cancelRequest", json!({"id": self.id}));
        }
    }
}

impl Client {
    /// Start the reader and writer tasks over `t`. `name` labels errors and logs.
    pub fn start(name: &str, t: Transport, folders: Value) -> Arc<Client> {
        let shared = Arc::new(Shared::default());
        *shared.folders.lock() = folders;
        let (out, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let mut write = t.write;
        let w_shared = shared.clone();
        tokio::spawn(async move {
            while let Some(body) = rx.recv().await {
                let head = format!("Content-Length: {}\r\n\r\n", body.len());
                let ok = write.write_all(head.as_bytes()).await.is_ok()
                    && write.write_all(&body).await.is_ok()
                    && write.flush().await.is_ok();
                if !ok {
                    w_shared.die("The language server closed its input.".into());
                    break;
                }
            }
        });
        let r_shared = shared.clone();
        let r_out = out.clone();
        let r_name = name.to_string();
        let mut read = BufReader::new(t.read);
        tokio::spawn(async move {
            let why = loop {
                match read_message(&mut read).await {
                    Ok(Some(msg)) => dispatch(&r_name, &r_shared, &r_out, msg),
                    Ok(None) => break "The language server exited.".to_string(),
                    Err(e) => break format!("The language server sent a broken message: {e}."),
                }
            };
            r_shared.die(why);
        });
        if let Some(err) = t.stderr {
            let e_shared = shared.clone();
            let e_name = name.to_string();
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(server = %e_name, "{line}");
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let mut tail = e_shared.stderr.lock();
                    if tail.len() == STDERR_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(line.chars().take(300).collect());
                }
            });
        }
        Arc::new(Client {
            name: name.to_string(),
            next: AtomicI64::new(1),
            out,
            shared,
            child: Mutex::new(t.child),
            sandbox: Mutex::new(t.sandbox),
            closed: AtomicBool::new(false),
        })
    }

    /// Why the connection ended, once it has.
    pub fn dead(&self) -> Option<String> {
        if let Some(c) = self.child.lock().as_mut()
            && let Ok(Some(status)) = c.try_wait()
        {
            self.shared
                .die(format!("The language server exited with {status}."));
        }
        self.shared.dead.lock().clone()
    }

    /// The server reports work in progress, such as indexing.
    pub fn busy(&self) -> bool {
        *self.shared.quiescent.lock() == Some(false) || !self.shared.progress.lock().is_empty()
    }

    fn send(&self, msg: &Value) {
        let _ = self.out.send(serde_json::to_vec(msg).expect("json"));
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Reply {
        if let Some(why) = self.dead() {
            return Err(why);
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().insert(id, tx);
        // `die` drains pending under the `dead` lock, so a death before the insert shows here.
        if let Some(why) = self.shared.dead.lock().clone() {
            self.shared.pending.lock().remove(&id);
            return Err(why);
        }
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let _cancel = CancelOnDrop { client: self, id };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err("The language server connection closed.".into()),
            Err(_) => Err(format!(
                "`{}` did not answer `{method}` within {} seconds.",
                self.name,
                timeout.as_secs()
            )),
        }
    }

    /// `shutdown`, then `exit`, then kill the process if it is still running.
    pub async fn close(&self) {
        if self.closed.swap(true, Ordering::Relaxed) {
            return;
        }
        if self.dead().is_none() {
            let _ = self
                .request("shutdown", Value::Null, Duration::from_secs(3))
                .await;
            self.notify("exit", Value::Null);
        }
        let child = self.child.lock().take();
        if let Some(mut c) = child
            && tokio::time::timeout(Duration::from_secs(2), c.wait())
                .await
                .is_err()
        {
            let _ = c.kill().await;
        }
        drop(self.sandbox.lock().take());
        self.shared.die("The language server was stopped.".into());
    }
}

async fn read_message<R: AsyncRead + Unpin>(r: &mut BufReader<R>) -> Result<Option<Value>, String> {
    let mut len: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).await.map_err(|e| e.to_string())? == 0 {
            return Ok(None);
        }
        let l = line.trim_end();
        if l.is_empty() {
            if len.is_some() {
                break;
            }
            continue;
        }
        if let Some((k, v)) = l.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            len = Some(v.trim().parse().map_err(|_| format!("bad length `{v}`"))?);
        }
    }
    let len = len.expect("checked");
    if len > MAX_MESSAGE {
        return Err(format!("a message of {len} bytes"));
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| e.to_string())
}

fn dispatch(name: &str, sh: &Shared, out: &mpsc::UnboundedSender<Vec<u8>>, msg: Value) {
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id").cloned();
    match (method, id) {
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            let Some(tx) = sh.pending.lock().remove(&id) else {
                return;
            };
            let reply = match msg.get("error") {
                Some(e) => Err(rpc_error(name, e)),
                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(reply);
        }
        (Some(m), Some(id)) => {
            let result = match m {
                "workspace/configuration" => {
                    let n = msg["params"]["items"].as_array().map_or(0, Vec::len);
                    Ok(Value::Array(vec![Value::Null; n]))
                }
                "workspace/workspaceFolders" => Ok(sh.folders.lock().clone()),
                "window/workDoneProgress/create"
                | "client/registerCapability"
                | "client/unregisterCapability"
                | "window/showMessageRequest"
                | "workspace/semanticTokens/refresh"
                | "workspace/codeLens/refresh"
                | "workspace/inlayHint/refresh"
                | "workspace/diagnostic/refresh" => Ok(Value::Null),
                "workspace/applyEdit" => Ok(json!({"applied": false})),
                _ => Err(json!({"code": -32601, "message": format!("Ostra does not handle {m}.")})),
            };
            let reply = match result {
                Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
                Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
            };
            let _ = out.send(serde_json::to_vec(&reply).expect("json"));
        }
        (Some("$/progress"), None) => {
            let p = &msg["params"];
            let token = match &p["token"] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            match p["value"]["kind"].as_str() {
                Some("begin") => {
                    sh.progress.lock().insert(token);
                }
                Some("end") => {
                    sh.progress.lock().remove(&token);
                }
                _ => {}
            }
        }
        (Some("experimental/serverStatus"), None) => {
            *sh.quiescent.lock() = msg["params"]["quiescent"].as_bool();
        }
        _ => {}
    }
}

fn rpc_error(name: &str, e: &Value) -> String {
    match e["code"].as_i64() {
        Some(CONTENT_MODIFIED | SERVER_CANCELLED) => {
            format!("`{name}` is still loading the project and dropped the request.")
        }
        _ => format!(
            "`{name}` answered with an error: {}",
            e["message"].as_str().unwrap_or("no message")
        ),
    }
}
