use crate::text::truncate_middle;
use crate::{LiveOutput, ToolEnv, ToolOutput, required, u64_arg};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_OUTPUT_CHARS: usize = 30_000;
const CAPTURE_HALF: usize = 128 * 1024;
const DRAIN_GRACE: Duration = Duration::from_millis(300);

// The command runs through `eval` in the same shell so a `cd` persists into the EXIT trap, which
// records the final directory for the next call.
const SCRIPT: &str = r#"exec 2>&1
__ostra_cmd=$OSTRA_CMD
__ostra_pwd=$OSTRA_PWD_FILE
cd -- "$OSTRA_CWD" || exit 1
unset OSTRA_CMD OSTRA_PWD_FILE OSTRA_CWD
trap 'pwd -P > "$__ostra_pwd" 2>/dev/null' EXIT
eval "$__ostra_cmd"
"#;

/// Output capture bounded in memory: everything up to `2 * CAPTURE_HALF`, then head and tail.
struct Capture {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    total: usize,
}

impl Capture {
    fn new() -> Self {
        Capture { head: Vec::new(), tail: std::collections::VecDeque::new(), total: 0 }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        for &b in bytes {
            if self.head.len() < CAPTURE_HALF {
                self.head.push(b);
            } else {
                self.tail.push_back(b);
                if self.tail.len() > CAPTURE_HALF {
                    self.tail.pop_front();
                }
            }
        }
    }

    fn text(&self) -> String {
        let dropped = self.total - self.head.len() - self.tail.len();
        let head = String::from_utf8_lossy(&self.head);
        let tail: Vec<u8> = self.tail.iter().copied().collect();
        let tail = String::from_utf8_lossy(&tail);
        let joined = if dropped > 0 {
            format!("{head}\n... [{dropped} bytes dropped] ...\n{tail}")
        } else {
            format!("{head}{tail}")
        };
        truncate_middle(&joined, MAX_OUTPUT_CHARS)
    }
}

/// Splits streamed bytes into valid UTF-8 text, holding back an incomplete trailing sequence.
struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending) {
            Ok(s) => {
                let out = s.to_string();
                self.pending.clear();
                out
            }
            Err(e) if e.error_len().is_none() => {
                let valid = e.valid_up_to();
                let out = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
                self.pending.drain(..valid);
                out
            }
            Err(_) => {
                let out = String::from_utf8_lossy(&self.pending).into_owned();
                self.pending.clear();
                out
            }
        }
    }
}

fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // SAFETY: plain syscall; a negative pid targets the process group the child leads.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

fn spawn_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(mut r: R, tx: mpsc::UnboundedSender<Vec<u8>>) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

enum End {
    Exited(std::process::ExitStatus),
    TimedOut,
    Cancelled,
    WaitFailed(String),
}

pub async fn run(
    env: &ToolEnv,
    call_id: &str,
    input: &Value,
    live: Option<LiveOutput>,
    cancel: CancellationToken,
) -> ToolOutput {
    let command = match required(input, "command") {
        Ok(c) => c,
        Err(e) => return e,
    };
    if command.trim().is_empty() {
        return ToolOutput::err("The command is empty.");
    }
    let timeout_ms = u64_arg(input, "timeout").unwrap_or(DEFAULT_TIMEOUT_MS).clamp(1, MAX_TIMEOUT_MS);
    let mut cwd = env.cwd();
    if !cwd.is_dir() {
        cwd = env.config().repo_root.clone();
        env.set_cwd(cwd.clone());
    }
    let pwd_file = std::env::temp_dir().join(format!("ostra-pwd-{}", uuid::Uuid::new_v4().simple()));

    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c")
        .arg(SCRIPT)
        .env("OSTRA_CMD", command)
        .env("OSTRA_CWD", &cwd)
        .env("OSTRA_PWD_FILE", &pwd_file)
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ToolOutput::err(format!("Cannot start bash: {e}")),
    };
    let pid = child.id();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    if let Some(out) = child.stdout.take() {
        spawn_reader(out, tx.clone());
    }
    if let Some(err) = child.stderr.take() {
        spawn_reader(err, tx.clone());
    }
    drop(tx);

    let mut capture = Capture::new();
    let mut stream = Utf8Stream { pending: vec![] };
    let mut on_chunk = |chunk: &[u8], capture: &mut Capture| {
        capture.push(chunk);
        if let Some(live) = &live {
            let text = stream.push(chunk);
            if !text.is_empty() {
                live(call_id, &text);
            }
        }
    };

    let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(deadline);
    let end = loop {
        tokio::select! {
            Some(chunk) = rx.recv() => on_chunk(&chunk, &mut capture),
            status = child.wait() => break match status {
                Ok(s) => End::Exited(s),
                Err(e) => End::WaitFailed(e.to_string()),
            },
            _ = &mut deadline => break End::TimedOut,
            _ = cancel.cancelled() => break End::Cancelled,
        }
    };
    if !matches!(end, End::Exited(_)) {
        kill_group(pid);
        let _ = child.wait().await;
    }
    loop {
        match tokio::time::timeout(DRAIN_GRACE, rx.recv()).await {
            Ok(Some(chunk)) => on_chunk(&chunk, &mut capture),
            Ok(None) => break,
            Err(_) => {
                // Background processes still hold the pipe; stop them so nothing outlives the call.
                kill_group(pid);
                break;
            }
        }
    }

    if let Ok(dir) = std::fs::read_to_string(&pwd_file) {
        let dir = std::path::PathBuf::from(dir.trim_end_matches('\n'));
        if dir.is_dir() {
            env.set_cwd(dir);
        }
    }
    let _ = std::fs::remove_file(&pwd_file);

    let text = capture.text();
    let body = if text.trim().is_empty() { "(no output)".to_string() } else { text.trim_end().to_string() };
    match end {
        End::Exited(status) => {
            let code = status.code();
            match code {
                Some(0) => ToolOutput { exit_code: Some(0), ..ToolOutput::ok(body) },
                Some(c) => ToolOutput { exit_code: Some(c), ..ToolOutput::err(format!("{body}\n\nExit code: {c}")) },
                None => ToolOutput::err(format!("{body}\n\nThe command was terminated by a signal.")),
            }
        }
        End::TimedOut => ToolOutput::err(format!(
            "The command timed out after {timeout_ms} ms and was stopped. Output so far:\n{body}"
        )),
        End::Cancelled => ToolOutput::err(format!("The command was cancelled. Output so far:\n{body}")),
        End::WaitFailed(e) => ToolOutput::err(format!("Waiting for the command failed: {e}\n{body}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{env_in, run as run_tool};
    use ostra_core::policy::ToolCall;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn runs_and_reports_exit_code() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let out = run_tool(&env, "Bash", json!({"command": "echo hello; echo err >&2"})).await;
        assert!(!out.is_error);
        assert_eq!(out.exit_code, Some(0));
        assert!(out.text.contains("hello") && out.text.contains("err"));
        let out = run_tool(&env, "Bash", json!({"command": "exit 3"})).await;
        assert!(out.is_error);
        assert_eq!(out.exit_code, Some(3));
        assert!(out.text.contains("Exit code: 3"));
        let out = run_tool(&env, "Bash", json!({"command": "true"})).await;
        assert_eq!(out.text, "(no output)");
    }

    #[tokio::test]
    async fn cwd_persists_across_calls() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        std::fs::create_dir_all(env.config().repo_root.join("sub/dir")).unwrap();
        run_tool(&env, "Bash", json!({"command": "cd sub/dir"})).await;
        assert!(env.cwd().ends_with("sub/dir"));
        let out = run_tool(&env, "Bash", json!({"command": "pwd"})).await;
        assert!(out.text.trim().ends_with("sub/dir"));
        run_tool(&env, "Bash", json!({"command": "cd .. && exit 1"})).await;
        assert!(env.cwd().ends_with("sub"));
        let out = run_tool(&env, "Read", json!({"file_path": "missing"})).await;
        assert!(out.text.contains("/sub/missing"));
    }

    #[tokio::test]
    async fn quoting_survives() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let out = run_tool(&env, "Bash", json!({"command": "printf '%s|' \"a b\" 'c\"d' $'e\\nf'"})).await;
        assert_eq!(out.text, "a b|c\"d|e\nf|");
    }

    #[tokio::test]
    async fn timeout_kills_process_group() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let marker = d.path().join("survived");
        let cmd = format!("(sleep 2; touch {}) & sleep 30", marker.display());
        let started = std::time::Instant::now();
        let out = run_tool(&env, "Bash", json!({"command": cmd, "timeout": 300})).await;
        assert!(out.is_error && out.text.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(!marker.exists(), "background child outlived the timeout");
    }

    #[tokio::test]
    async fn background_process_does_not_hang_the_call() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let started = std::time::Instant::now();
        let out = run_tool(&env, "Bash", json!({"command": "sleep 30 & echo started"})).await;
        assert!(out.text.contains("started"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn cancel_stops_command() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            c2.cancel();
        });
        let call = ToolCall::new("Bash", json!({"command": "sleep 30"}));
        let out = crate::execute(&env, "c", &call, None, cancel).await;
        assert!(out.is_error && out.text.contains("cancelled"));
    }

    #[tokio::test]
    async fn streams_live_output_and_truncates() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let seen = Arc::new(Mutex::new(String::new()));
        let s2 = seen.clone();
        let live: LiveOutput = Arc::new(move |id: &str, chunk: &str| {
            assert_eq!(id, "c9");
            s2.lock().unwrap().push_str(chunk);
        });
        let call = ToolCall::new("Bash", json!({"command": "for i in $(seq 1 20000); do echo line$i; done"}));
        let out = crate::execute(&env, "c9", &call, Some(live), CancellationToken::new()).await;
        assert!(out.text.contains("truncated"));
        assert!(out.text.len() < 31_000);
        assert!(out.text.starts_with("line1\n"));
        assert!(out.text.ends_with("line20000"));
        assert!(seen.lock().unwrap().contains("line19999\n"));
    }

    #[test]
    fn utf8_stream_holds_partial_sequences() {
        let mut s = Utf8Stream { pending: vec![] };
        let bytes = "é".as_bytes();
        assert_eq!(s.push(&bytes[..1]), "");
        assert_eq!(s.push(&bytes[1..]), "é");
    }
}
