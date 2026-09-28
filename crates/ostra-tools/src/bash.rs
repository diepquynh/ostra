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
/// How long to wait for a killed command to be reaped.
pub(crate) const KILL_WAIT: Duration = Duration::from_secs(5);

// The command runs through `eval` in the same shell so a `cd` persists into the EXIT trap, which
// records the final directory for the next call.
#[cfg(not(windows))]
const SCRIPT: &str = r#"exec 2>&1
__ostra_cmd=$OSTRA_CMD
__ostra_pwd=$OSTRA_PWD_FILE
cd -- "$OSTRA_CWD" || exit 1
unset OSTRA_CMD OSTRA_PWD_FILE OSTRA_CWD
trap 'pwd -P > "$__ostra_pwd" 2>/dev/null' EXIT
eval "$__ostra_cmd"
"#;

// Git Bash's `pwd -P` prints an MSYS path (`/c/Users/me`); `pwd -W` prints the Windows path the
// next call and the other tools resolve against, after `cd -P` resolves links.
#[cfg(windows)]
const SCRIPT: &str = r#"exec 2>&1
__ostra_cmd=$OSTRA_CMD
__ostra_pwd=$OSTRA_PWD_FILE
cd -- "$OSTRA_CWD" || exit 1
unset OSTRA_CMD OSTRA_PWD_FILE OSTRA_CWD
trap '{ cd -P . && pwd -W; } > "$__ostra_pwd" 2>/dev/null' EXIT
eval "$__ostra_cmd"
"#;

/// The credential variables `config.toml` names, read fresh, plus the defaults and the bridge
/// variables. The executor adds the workspace's MCP secrets.
pub(crate) fn configured_secret_vars() -> Vec<String> {
    let global: ostra_core::config::GlobalConfig =
        ostra_core::config::load_toml(&ostra_core::paths::global_config_path()).unwrap_or_default();
    ostra_core::config::credential_env_names(&global, &[])
        .into_iter()
        .collect()
}

/// Inherited variables a child must not see: the configured secrets and every `OSTRA_*`.
pub(crate) fn scrubbed_vars(
    inherited: impl Iterator<Item = std::ffi::OsString>,
    secrets: &[String],
) -> Vec<std::ffi::OsString> {
    let mut out: Vec<std::ffi::OsString> = secrets.iter().map(Into::into).collect();
    out.extend(inherited.filter(|k| k.to_string_lossy().starts_with("OSTRA_")));
    out
}

/// Output capture bounded in memory: everything up to `2 * CAPTURE_HALF`, then head and tail.
pub(crate) struct Capture {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    total: usize,
}

impl Capture {
    pub(crate) fn new() -> Self {
        Capture {
            head: Vec::new(),
            tail: std::collections::VecDeque::new(),
            total: 0,
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) {
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

    pub(crate) fn text(&self) -> String {
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
pub(crate) struct Utf8Stream {
    pub(crate) pending: Vec<u8>,
}

impl Utf8Stream {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> String {
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

pub(crate) fn spawn_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    mut r: R,
    tx: mpsc::UnboundedSender<Vec<u8>>,
) {
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
    let timeout_ms = u64_arg(input, "timeout")
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS);
    let mut cwd = env.cwd();
    if !cwd.is_dir() {
        cwd = env.config().repo_root.clone();
        env.set_cwd(cwd.clone());
    }
    let pwd_file =
        std::env::temp_dir().join(format!("ostra-pwd-{}", uuid::Uuid::new_v4().simple()));

    // Kills what the command leaves running when the call ends, as bubblewrap's pid namespace
    // does.
    let mut _members = None;
    let mut cmd = match &env.sandbox {
        Some((backend, profile)) => {
            // The EXIT trap writes here from inside the sandbox, so the file must exist and be
            // writable there even when `/tmp` is private or denied.
            if let Err(e) = std::fs::write(&pwd_file, "") {
                return ToolOutput::err(format!("Cannot prepare the sandbox: {e}"));
            }
            let sc = match profile.clone().writable_file(&pwd_file).command(
                backend,
                &cwd,
                std::ffi::OsStr::new("bash"),
                std::iter::empty::<std::ffi::OsString>(),
            ) {
                Ok(sc) => sc,
                Err(e) => {
                    let _ = std::fs::remove_file(&pwd_file);
                    return ToolOutput::err(format!("Cannot prepare the sandbox: {e}"));
                }
            };
            let c = tokio::process::Command::from(sc.std_command());
            _members = sc.members;
            c
        }
        None => match ostra_core::shells::bash() {
            Ok(bash) => tokio::process::Command::new(bash),
            Err(e) => return ToolOutput::err(e),
        },
    };
    for name in scrubbed_vars(std::env::vars_os().map(|(k, _)| k), &env.scrub_env) {
        cmd.env_remove(name);
    }
    cmd.env_remove("GIT_EXTERNAL_DIFF")
        .envs(ostra_core::git::agent_env());
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
        .kill_on_drop(true);
    ostra_core::proctree::prepare_tokio(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ToolOutput::err(format!("Cannot start bash: {e}")),
    };
    let tree = match ostra_core::proctree::Tree::of_tokio(&child) {
        Ok(t) => t,
        Err(e) => {
            let _ = child.start_kill();
            return ToolOutput::err(format!("Cannot start bash: {e}"));
        }
    };
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
        tree.kill();
        // Bounded: on Windows a process killed partway through its own creation never exits.
        let _ = tokio::time::timeout(KILL_WAIT, child.wait()).await;
    }
    loop {
        match tokio::time::timeout(DRAIN_GRACE, rx.recv()).await {
            Ok(Some(chunk)) => on_chunk(&chunk, &mut capture),
            Ok(None) => break,
            Err(_) => {
                // Background processes still hold the pipe; stop them so nothing outlives the call.
                tree.kill();
                break;
            }
        }
    }

    tree.end();
    drop(_members);
    if let Ok(dir) = std::fs::read_to_string(&pwd_file) {
        let dir = env.sandbox_to_host(&std::path::PathBuf::from(dir.trim_end_matches(['\n', '\r'])));
        if dir.is_dir() {
            env.set_cwd(dir);
        }
    }
    let _ = std::fs::remove_file(&pwd_file);

    let text = capture.text();
    let body = if text.trim().is_empty() {
        "(no output)".to_string()
    } else {
        text.trim_end().to_string()
    };
    match end {
        End::Exited(status) => {
            let code = status.code();
            match code {
                Some(0) => ToolOutput {
                    exit_code: Some(0),
                    ..ToolOutput::ok(body)
                },
                Some(c) => ToolOutput {
                    exit_code: Some(c),
                    ..ToolOutput::err(format!("{body}\n\nExit code: {c}"))
                },
                None => {
                    ToolOutput::err(format!("{body}\n\nThe command was terminated by a signal."))
                }
            }
        }
        End::TimedOut => ToolOutput::err(format!(
            "The command timed out after {timeout_ms} ms and was stopped. Output so far:\n{body}"
        )),
        End::Cancelled => {
            ToolOutput::err(format!("The command was cancelled. Output so far:\n{body}"))
        }
        End::WaitFailed(e) => {
            ToolOutput::err(format!("Waiting for the command failed: {e}\n{body}"))
        }
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
        assert!(out.text.replace('\\', "/").contains("/sub/missing"), "{}", out.text);
    }

    #[tokio::test]
    async fn quoting_survives() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let out = run_tool(
            &env,
            "Bash",
            json!({"command": "printf '%s|' \"a b\" 'c\"d' $'e\\nf'"}),
        )
        .await;
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
        let call = ToolCall::new(
            "Bash",
            json!({"command": "for i in $(seq 1 20000); do echo line$i; done"}),
        );
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

    #[test]
    fn scrubs_configured_secrets_and_ostra_vars() {
        let inherited =
            ["PATH", "OSTRA_TOKEN", "OSTRA_DATA_DIR", "HOME"].map(std::ffi::OsString::from);
        let out = scrubbed_vars(inherited.into_iter(), &["MY_KEY".into()]);
        let out: Vec<String> = out
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(out, ["MY_KEY", "OSTRA_TOKEN", "OSTRA_DATA_DIR"]);
        assert!(configured_secret_vars().contains(&"ANTHROPIC_API_KEY".to_string()));
    }

    #[tokio::test]
    async fn child_does_not_see_scrubbed_vars() {
        let d = tempfile::tempdir().unwrap();
        let mut env = env_in(d.path());
        // A variable set in this process stands in for a provider key. HOME is unsuitable on
        // Windows, where Git Bash re-derives it, so use a name the shell will not recreate.
        // SAFETY: no other test reads this variable.
        unsafe { std::env::set_var("OSTRA_TEST_PROVIDER_KEY", "leaked") };
        env.scrub_env.push("OSTRA_TEST_PROVIDER_KEY".into());
        let out = run_tool(
            &env,
            "Bash",
            json!({"command": "echo \"[${OSTRA_TEST_PROVIDER_KEY-unset}]\""}),
        )
        .await;
        assert_eq!(out.text, "[unset]");
    }

    #[tokio::test]
    async fn repo_git_config_cannot_run_fsmonitor() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let marker = d.path().join("fsmonitor-ran");
        let cmd = format!(
            "git init -q && git config core.fsmonitor 'touch {} #' && echo a > f && git status --short",
            marker.display()
        );
        let out = run_tool(&env, "Bash", json!({"command": cmd})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(!marker.exists(), "fsmonitor hook ran");
    }

    /// A tool env whose Bash runs sandboxed with `home` as the home folder, or `None` where no
    /// sandbox works.
    fn sandboxed(dir: &std::path::Path, home: &std::path::Path) -> Option<ToolEnv> {
        let Some(backend) = ostra_sandbox::backend() else {
            eprintln!(
                "no sandbox here ({}); skipping",
                ostra_sandbox::unavailable_message()
            );
            return None;
        };
        let env = env_in(dir);
        let root = ostra_core::paths::canonical(dir).unwrap();
        let ctx = ostra_core::exec::ExecContext {
            execution_id: "x_sb".into(),
            session_id: None,
            agent: ostra_core::AgentName::Implementer,
            initializer_mode: None,
            executor: ostra_core::ExecutorKind::Native,
            workspace_root: env.config().repo_root.clone(),
            repo_root: env.config().repo_root.clone(),
            project_key: "p".into(),
            session_dir: env.config().session_dir.clone(),
            session_root: env.config().session_dir.clone(),
            report_file: None,
            phase: None,
            yolo: false,
            permission_mode: ostra_core::config::PermissionMode::Default,
            permissions: Default::default(),
            protected_paths: vec![],
            memory_db: root.join("session/memory/knowledge.sqlite3"),
            sandbox_mode: None,
            sandbox_network: None,
            sandbox_allowed_hosts: vec![],
            sandbox_decoys: vec![],
            sandbox_loopback: Default::default(),
            sandbox_blocked_ports: vec![],
            creates_project: false,
        };
        let profile = ostra_sandbox::Profile::for_execution(
            &ctx,
            &ostra_core::config::SandboxConfig::default(),
            home,
        )
        .scratch(&root.join("scratch"))
        .unwrap();
        Some(env.with_sandbox(backend.clone(), profile))
    }

    #[tokio::test]
    async fn sandbox_confines_writes_and_hides_secrets() {
        let d = tempfile::tempdir().unwrap();
        let home = ostra_core::paths::canonical(d.path()).unwrap().join("home");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/id_ed25519"), "SECRET").unwrap();
        std::fs::write(home.join(".bashrc"), "").unwrap();
        let Some(env) = sandboxed(d.path(), &home) else {
            return;
        };
        let out = run_tool(
            &env,
            "Bash",
            json!({"command": "echo ok > made.txt && cat made.txt"}),
        )
        .await;
        assert_eq!(out.text, "ok", "{}", out.text);
        assert!(env.config().repo_root.join("made.txt").exists());

        let key = home.join(".ssh/id_ed25519");
        let out = run_tool(
            &env,
            "Bash",
            json!({"command": format!("cat {}", key.display())}),
        )
        .await;
        assert!(out.is_error && !out.text.contains("SECRET"), "{}", out.text);

        let rc = home.join(".bashrc");
        let out = run_tool(
            &env,
            "Bash",
            json!({"command": format!("echo evil >> {}", rc.display())}),
        )
        .await;
        assert!(out.is_error, "{}", out.text);
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), "");
        if let Some(real_home) = ostra_core::paths::home() {
            let probe = std::path::Path::new(&real_home).join(format!(
                ".ostra-sandbox-probe-{}",
                uuid::Uuid::new_v4().simple()
            ));
            let out = run_tool(
                &env,
                "Bash",
                json!({"command": format!("touch {}", probe.display())}),
            )
            .await;
            let made = probe.exists();
            let _ = std::fs::remove_file(&probe);
            assert!(out.is_error && !made, "{}", out.text);
        }

        if cfg!(target_os = "linux") {
            let out = run_tool(
                &env,
                "Bash",
                json!({"command": "ls /proc | grep -cE '^[0-9]+$'"}),
            )
            .await;
            let n: u32 = out.text.trim().parse().unwrap_or(99);
            assert!(n < 10, "sandbox sees {n} processes");
        }

        let data = ostra_core::paths::data_dir();
        if data.is_dir() {
            let out = run_tool(
                &env,
                "Bash",
                json!({"command": format!("ls -A {}", data.display())}),
            )
            .await;
            assert!(!out.text.contains("registry.db"), "{}", out.text);
        }
        if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
            let bus = std::path::Path::new(&rt).join("bus");
            if bus.exists() {
                let out = run_tool(&env, "Bash", json!({"command": format!("test -e {} && echo visible || echo hidden", bus.display())})).await;
                assert_eq!(out.text, "hidden");
            }
        }
        let out = run_tool(&env, "Bash", json!({"command": "echo \"${SSH_AUTH_SOCK-unset}\" \"${DBUS_SESSION_BUS_ADDRESS-unset}\""})).await;
        assert_eq!(out.text, "unset unset");
    }

    #[tokio::test]
    async fn sandbox_keeps_git_config_read_only_and_cwd_persistent() {
        let d = tempfile::tempdir().unwrap();
        let home = ostra_core::paths::canonical(d.path()).unwrap().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let repo = env_in(d.path()).config().repo_root.clone();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(ok.success());
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        let Some(env) = sandboxed(d.path(), &home) else {
            return;
        };
        let out = run_tool(
            &env,
            "Bash",
            json!({"command": "git config core.fsmonitor 'touch x #'"}),
        )
        .await;
        assert!(out.is_error, "{}", out.text);
        assert!(
            !std::fs::read_to_string(repo.join(".git/config"))
                .unwrap()
                .contains("fsmonitor")
        );
        let out = run_tool(
            &env,
            "Bash",
            json!({"command": "echo a > f && git add f && git status --short"}),
        )
        .await;
        assert!(out.text.contains("A  f"), "{}", out.text);

        run_tool(&env, "Bash", json!({"command": "cd sub"})).await;
        assert!(env.cwd().ends_with("sub"));
        let out = run_tool(&env, "Bash", json!({"command": "pwd"})).await;
        assert!(out.text.trim().ends_with("/sub"), "{}", out.text);
    }

    #[tokio::test]
    async fn sandbox_tmp_is_private_and_persists_across_calls() {
        let d = tempfile::tempdir().unwrap();
        let home = ostra_core::paths::canonical(d.path()).unwrap().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let marker =
            std::env::temp_dir().join(format!("ostra-host-{}", uuid::Uuid::new_v4().simple()));
        std::fs::write(&marker, "").unwrap();
        let Some(env) = sandboxed(d.path(), &home) else {
            let _ = std::fs::remove_file(&marker);
            return;
        };
        let out = run_tool(&env, "Bash", json!({"command": format!("test -e {} && echo shared || echo private", marker.display())})).await;
        let _ = std::fs::remove_file(&marker);
        assert_eq!(out.text, "private");
        // `/tmp` under bubblewrap; under Seatbelt, which denies `/tmp`, the scratch dir.
        let out = run_tool(
            &env,
            "Bash",
            json!({"command": "d=\"${TMPDIR%/}/w\" && mkdir -p \"$d\" && echo kept > \"$d/f\" && cd \"$d\" && pwd"}),
        )
        .await;
        let dir = out.text.trim().to_string();
        let out = run_tool(&env, "Bash", json!({"command": "cat f && pwd"})).await;
        assert!(out.text.starts_with("kept"), "{}", out.text);
        let out = run_tool(&env, "Read", json!({"file_path": format!("{dir}/f")})).await;
        assert!(out.text.contains("kept"), "{}", out.text);
    }

    #[tokio::test]
    async fn sandbox_timeout_stops_background_children() {
        let d = tempfile::tempdir().unwrap();
        let home = ostra_core::paths::canonical(d.path()).unwrap().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let Some(env) = sandboxed(d.path(), &home) else {
            return;
        };
        let marker = env.config().repo_root.join("survived");
        let cmd = format!("(sleep 2; touch {}) & sleep 30", marker.display());
        let started = std::time::Instant::now();
        let out = run_tool(&env, "Bash", json!({"command": cmd, "timeout": 300})).await;
        assert!(
            out.is_error && out.text.contains("timed out"),
            "{}",
            out.text
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(!marker.exists(), "background child outlived the timeout");
    }
}
