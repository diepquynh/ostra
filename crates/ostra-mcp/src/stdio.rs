//! The stdio transport: a child process speaking newline-delimited JSON-RPC.

use crate::{McpError, Shared, Transport, is_response, on_server_message};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio as PStdio;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

/// Bytes of the server's stderr kept to explain a crash.
const STDERR_TAIL: usize = 4096;

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>;

pub(crate) struct Stdio {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    pending: Pending,
    shared: Arc<Shared>,
    stderr: Arc<Mutex<String>>,
    stderr_eof: tokio::sync::watch::Receiver<bool>,
    _child: Mutex<Child>,
}

impl Stdio {
    pub fn spawn(
        program: &str,
        args: &[String],
        env: &[(String, String)],
        env_remove: &[String],
        cwd: &Path,
        shared: Arc<Shared>,
    ) -> Result<Stdio, McpError> {
        let mut cmd = Command::new(program);
        for k in env_remove {
            cmd.env_remove(k);
        }
        cmd.args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd)
            .stdin(PStdio::piped())
            .stdout(PStdio::piped())
            .stderr(PStdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Transport(format!("could not start `{program}`: {e}")))?;
        let stdin = Arc::new(tokio::sync::Mutex::new(child.stdin.take().expect("piped")));
        let stdout = child.stdout.take().expect("piped");
        let mut stderr_pipe = child.stderr.take().expect("piped");
        let pending: Pending = Arc::default();
        let stderr = Arc::new(Mutex::new(String::new()));

        let tail = stderr.clone();
        let (stderr_done, stderr_eof) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok(n) = stderr_pipe.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut t = tail.lock();
                t.push_str(&String::from_utf8_lossy(&buf[..n]));
                if t.len() > STDERR_TAIL {
                    let mut cut = t.len() - STDERR_TAIL;
                    while !t.is_char_boundary(cut) {
                        cut += 1;
                    }
                    t.drain(..cut);
                }
            }
            let _ = stderr_done.send(true);
        });

        let (p, s, w) = (pending.clone(), shared.clone(), stdin.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                    // Servers that log to stdout break the protocol; skip the line.
                    tracing::debug!(
                        "mcp stdio: not JSON: {}",
                        line.chars().take(200).collect::<String>()
                    );
                    continue;
                };
                let messages = match message {
                    Value::Array(items) => items,
                    one => vec![one],
                };
                for m in messages {
                    if is_response(&m) {
                        if let Some(id) = m.get("id").and_then(Value::as_i64)
                            && let Some(tx) = p.lock().remove(&id)
                        {
                            let _ = tx.send(m);
                        }
                    } else if let Some(reply) = on_server_message(&s, &m) {
                        let mut w = w.lock().await;
                        let _ = w.write_all(format!("{reply}\n").as_bytes()).await;
                        let _ = w.flush().await;
                    }
                }
            }
            s.alive.store(false, Ordering::SeqCst);
            p.lock().clear();
        });

        Ok(Stdio {
            stdin,
            pending,
            shared,
            stderr,
            stderr_eof,
            _child: Mutex::new(child),
        })
    }

    /// Why the server is gone. A crashing server's last stderr lines explain the exit, so they are
    /// read to the end first, for a short while.
    async fn closed(&self) -> McpError {
        let mut eof = self.stderr_eof.clone();
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            eof.wait_for(|done| *done),
        )
        .await;
        let tail = self.stderr.lock().trim().to_string();
        let last: Vec<&str> = tail.lines().rev().take(5).collect();
        let last: Vec<&str> = last.into_iter().rev().collect();
        McpError::Closed(if last.is_empty() {
            "the server exited".into()
        } else {
            format!("the server exited: {}", last.join(" / "))
        })
    }

    async fn write(&self, message: &Value) -> Result<(), McpError> {
        if !self.shared.alive.load(Ordering::SeqCst) {
            return Err(self.closed().await);
        }
        let mut w = self.stdin.lock().await;
        let line = format!("{message}\n");
        if w.write_all(line.as_bytes()).await.is_err() || w.flush().await.is_err() {
            drop(w);
            return Err(self.closed().await);
        }
        Ok(())
    }
}

impl Drop for Stdio {
    fn drop(&mut self) {
        // Servers started through `npx` or `uvx` run in grandchildren; end the whole group.
        #[cfg(unix)]
        if let Some(pid) = self._child.lock().id() {
            // SAFETY: signals the process group this transport created.
            unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
        }
    }
}

#[async_trait::async_trait]
impl Transport for Stdio {
    async fn request(&self, id: i64, message: Value) -> Result<Value, McpError> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        if let Err(e) = self.write(&message).await {
            self.pending.lock().remove(&id);
            return Err(e);
        }
        match rx.await {
            Ok(v) => Ok(v),
            Err(_) => Err(self.closed().await),
        }
    }

    async fn notify(&self, message: Value) -> Result<(), McpError> {
        self.write(&message).await
    }

    fn abandon(&self, id: i64) {
        self.pending.lock().remove(&id);
    }
}
