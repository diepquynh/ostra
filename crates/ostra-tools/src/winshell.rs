//! The Windows PowerShell and Cmd tools. They exist only on Windows, alongside the Bash tool.
//!
//! Ostra's command parser is tree-sitter-bash, so a PowerShell or cmd command is opaque to the
//! write-scope and secret-read guards that read a parsed command
//! ([`ostra_policy`]). The policy therefore treats these tools as it treats an unparsed shell
//! command: never auto-allowed, always subject to the mode's permission, and refused outright when
//! the raw text names Ostra's own state or a credential path. This module adds the rest of the
//! Bash tool's hardening that does not need a parser: the credential and `OSTRA_*` variables are
//! scrubbed from the child, and the whole process tree is killed when the call ends.

use crate::bash::{
    Capture, KILL_WAIT, Utf8Stream, configured_secret_vars, scrubbed_vars, spawn_reader,
};
use crate::{LiveOutput, ToolEnv, ToolOutput, required, u64_arg};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const DRAIN_GRACE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy)]
pub enum Shell {
    PowerShell,
    Cmd,
}

impl Shell {
    fn program(self) -> std::path::PathBuf {
        match self {
            Shell::PowerShell => ostra_core::shells::powershell(),
            Shell::Cmd => ostra_core::shells::cmd(),
        }
    }

    /// The arguments that run `command` and then exit, and how the working dir is set. PowerShell
    /// runs a `cd` first so a directory change persists into the recorded pwd.
    fn args(self, command: &str, cwd: &str, pwd_file: &str) -> Vec<String> {
        match self {
            Shell::PowerShell => {
                // The final location is written for the next call. `-NonInteractive` and a null
                // input keep console prompts from blocking.
                let script = format!(
                    "Set-Location -LiteralPath '{cwd}'; try {{ {command} }} finally {{ (Get-Location).Path | Out-File -Encoding utf8 -LiteralPath '{pwd_file}' }}",
                    cwd = ps_single_quote(cwd),
                    pwd_file = ps_single_quote(pwd_file),
                );
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    script,
                ]
            }
            // Passed as one raw argument: see [`Shell::raw_cmd_line`].
            Shell::Cmd => vec![],
        }
    }

    /// cmd's command line, which must reach it unescaped: std quotes an inner `"` as `\"`, which
    /// cmd does not understand. `/s` strips the outer quotes. The trailing `cd` records the final
    /// directory and leaves `ERRORLEVEL`, which `cmd /c` exits with, as the command set it. The
    /// command is not grouped in parentheses, because a `)` in it would end the group.
    fn raw_cmd_line(command: &str, cwd: &str, pwd_file: &str) -> String {
        format!("/d /s /c \"cd /d \"{cwd}\" && {command} & cd > \"{pwd_file}\"\"")
    }
}

/// A string inside a PowerShell single-quoted literal: each `'` is doubled.
fn ps_single_quote(s: &str) -> String {
    s.replace('\'', "''")
}

pub async fn run(
    env: &ToolEnv,
    call_id: &str,
    input: &Value,
    shell: Shell,
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
        .unwrap_or(crate::bash::DEFAULT_TIMEOUT_MS)
        .clamp(1, crate::bash::MAX_TIMEOUT_MS);
    let mut cwd = env.cwd();
    if !cwd.is_dir() {
        cwd = env.config().repo_root.clone();
        env.set_cwd(cwd.clone());
    }
    let pwd_file =
        std::env::temp_dir().join(format!("ostra-pwd-{}", uuid::Uuid::new_v4().simple()));

    let program = shell.program();
    let args = shell.args(command, &cwd.to_string_lossy(), &pwd_file.to_string_lossy());
    let mut cmd = tokio::process::Command::new(&program);
    let mut scrub = env.scrub_env.clone();
    scrub.extend(configured_secret_vars());
    for name in scrubbed_vars(std::env::vars_os().map(|(k, _)| k), &scrub) {
        cmd.env_remove(name);
    }
    match shell {
        Shell::PowerShell => {
            cmd.args(&args);
        }
        Shell::Cmd => {
            cmd.raw_arg(Shell::raw_cmd_line(
                command,
                &cwd.to_string_lossy(),
                &pwd_file.to_string_lossy(),
            ));
        }
    }
    cmd.envs(ostra_core::git::agent_env())
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    ostra_core::proctree::prepare_tokio(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&pwd_file);
            return ToolOutput::err(format!("Cannot start {}: {e}", program.display()));
        }
    };
    let tree = match ostra_core::proctree::Tree::of_tokio(&child) {
        Ok(t) => t,
        Err(e) => {
            let _ = child.start_kill();
            let _ = std::fs::remove_file(&pwd_file);
            return ToolOutput::err(format!("Cannot start {}: {e}", program.display()));
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
            status = child.wait() => break Some(status),
            _ = &mut deadline => break None,
            _ = cancel.cancelled() => break Some(Err(std::io::Error::other("cancelled"))),
        }
    };
    let timed_out = end.is_none();
    let cancelled = cancel.is_cancelled();
    if timed_out || cancelled {
        tree.kill();
        // Bounded: on Windows a process killed partway through its own creation never exits.
        let _ = tokio::time::timeout(KILL_WAIT, child.wait()).await;
    }
    loop {
        match tokio::time::timeout(DRAIN_GRACE, rx.recv()).await {
            Ok(Some(chunk)) => on_chunk(&chunk, &mut capture),
            Ok(None) => break,
            Err(_) => {
                tree.kill();
                break;
            }
        }
    }
    drop(tree);

    if let Ok(dir) = std::fs::read_to_string(&pwd_file) {
        let dir = std::path::PathBuf::from(dir.trim().trim_start_matches('\u{feff}'));
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
    if cancelled {
        return ToolOutput::err(format!("The command was cancelled. Output so far:\n{body}"));
    }
    match end {
        None => ToolOutput::err(format!(
            "The command timed out after {timeout_ms} ms and was stopped. Output so far:\n{body}"
        )),
        Some(Ok(status)) => match status.code() {
            Some(0) => ToolOutput {
                exit_code: Some(0),
                ..ToolOutput::ok(body)
            },
            Some(c) => ToolOutput {
                exit_code: Some(c),
                ..ToolOutput::err(format!("{body}\n\nExit code: {c}"))
            },
            None => ToolOutput::err(format!("{body}\n\nThe command was terminated.")),
        },
        Some(Err(e)) => ToolOutput::err(format!("Waiting for the command failed: {e}\n{body}")),
    }
}

#[cfg(test)]
mod tests {
    use crate::testutil::{env_in, run as run_tool};
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn powershell_runs_and_reports_exit_code() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let out = run_tool(&env, "PowerShell", json!({"command": "Write-Output hello"})).await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(out.text, "hello");
        let out = run_tool(&env, "PowerShell", json!({"command": "exit 3"})).await;
        assert_eq!(out.exit_code, Some(3), "{}", out.text);
    }

    #[tokio::test]
    async fn cmd_runs_and_reports_exit_code() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let out = run_tool(&env, "Cmd", json!({"command": "echo hello"})).await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(out.text, "hello");
        let out = run_tool(&env, "Cmd", json!({"command": "exit /b 4"})).await;
        assert_eq!(out.exit_code, Some(4), "{}", out.text);
    }

    #[tokio::test]
    async fn the_working_dir_persists_across_calls() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        std::fs::create_dir_all(env.config().repo_root.join("sub")).unwrap();
        run_tool(&env, "PowerShell", json!({"command": "Set-Location sub"})).await;
        assert!(env.cwd().ends_with("sub"), "{}", env.cwd().display());
        let out = run_tool(&env, "Cmd", json!({"command": "cd"})).await;
        assert!(out.text.trim().ends_with("sub"), "{}", out.text);
    }

    #[tokio::test]
    async fn children_do_not_see_scrubbed_vars() {
        let d = tempfile::tempdir().unwrap();
        let mut env = env_in(d.path());
        // SAFETY: no other test reads this variable.
        unsafe { std::env::set_var("OSTRA_TEST_WINSHELL_KEY", "leaked") };
        env.scrub_env.push("OSTRA_TEST_WINSHELL_KEY".into());
        let out = run_tool(
            &env,
            "PowerShell",
            json!({"command": "Write-Output \"[$env:OSTRA_TEST_WINSHELL_KEY]\""}),
        )
        .await;
        assert_eq!(out.text, "[]");
    }

    #[tokio::test]
    async fn a_timeout_stops_the_command_and_what_it_started() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let marker = env.config().repo_root.join("survived");
        // A plain CreateProcess child (no ShellExecute), and a timeout that leaves it time to
        // finish starting, because a process whose creator is killed partway through creating
        // it can neither start nor end.
        let cmd = format!(
            "$i = New-Object System.Diagnostics.ProcessStartInfo 'powershell.exe', '-NoProfile -Command \"Start-Sleep 8; Set-Content -LiteralPath ''{}'' x\"'; $i.UseShellExecute = $false; [void][System.Diagnostics.Process]::Start($i); Start-Sleep 30",
            marker.display()
        );
        let started = Instant::now();
        let out = run_tool(&env, "PowerShell", json!({"command": cmd, "timeout": 6000})).await;
        assert!(out.is_error && out.text.contains("timed out"), "{}", out.text);
        assert!(started.elapsed() < Duration::from_secs(15));
        tokio::time::sleep(Duration::from_millis(4000)).await;
        assert!(!marker.exists(), "a child outlived the timeout");
    }
}
