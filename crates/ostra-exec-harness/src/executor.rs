//! [`HarnessExecutor`]: runs one leaf agent in an installed harness CLI under a PTY.
//!
//! Completion is the agent's `submit_<agent>` call through the MCP shim, confirmed by the
//! harness's Stop event where it has one (Antigravity has none, so its turn end is taken from a
//! quiet terminal after the submit). A Stop without a submit is turned back by the bridge; a
//! quiet session with no submit is nudged by typing into the terminal.

use crate::launch::{self, LaunchInput};
use crate::live::{LiveExecution, LiveRegistry, missing_submit_instruction};
use crate::outcome::{self, End, LAUNCH_PREFIX};
use crate::pty::{DEFAULT_COLS, DEFAULT_ROWS, PtyRegistry, PtySession};
use crate::term_log::TermLog;
use ostra_core::config::GlobalConfig;
use ostra_core::exec::{
    CancellationToken, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec, Executor,
};
use ostra_core::paths::{harness_execution_dir, terminal_transcript};
use ostra_core::{ExecutorKind, HarnessKind};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a session may sit with no hook event and no terminal output before it is nudged.
pub const DEFAULT_IDLE_NUDGE: Duration = Duration::from_secs(240);
const MAX_IDLE_NUDGES: u32 = 2;
/// After the submit, how long to wait for the turn to end before closing the harness.
const AFTER_SUBMIT_GRACE: Duration = Duration::from_secs(20);
/// Antigravity has no Stop hook: a submit followed by this much terminal quiet ends the turn.
const AGY_QUIET_AFTER_SUBMIT: Duration = Duration::from_secs(4);

/// Screen text that means the harness needs its user to log in.
pub const AUTH_MARKERS: &[&str] = &[
    "please log in",
    "please login",
    "not logged in",
    "run /login",
    "invalid api key",
    "login required",
    "authentication required",
    "oauth token has expired",
];

/// Screen text that means the harness could not start the session at all.
pub const FATAL_MARKERS: &[&str] = &[
    "failed to construct executor",
    "unknown component:",
    "unknown model",
    "model not found",
];

/// Folder-trust prompts that must be answered before the session starts. The user imported the
/// project into Ostra, and Ostra's guards govern every call, so the default "yes" is accepted.
pub const TRUST_MARKERS: &[&str] = &[
    "do you trust the files in this folder",
    "do you trust the contents of this directory",
    "trust this folder",
    "trust this project",
    "i trust this folder",
];

/// A session id the harness printed, such as Antigravity's `agy --conversation=<id>` resume line.
fn screen_session_id(screen: &str) -> Option<String> {
    let at = screen.find("--conversation=")? + "--conversation=".len();
    let id: String = screen[at..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() || *c == '-')
        .collect();
    (id.len() >= 32).then_some(id)
}

#[derive(Debug, PartialEq)]
enum TrustKey {
    Confirm,
    Down,
}

/// Which key answers "yes" to a trust prompt: Enter when the highlighted option trusts the
/// folder, else Down to move to it. Prompt defaults differ between versions ("No, exit" is first
/// on Claude Code 2.1.280).
fn trust_key(screen: &str) -> Option<TrustKey> {
    let option_like = |l: &str| {
        let l = l.to_lowercase();
        l.contains("yes")
            || l.contains("no,")
            || l.contains("quit")
            || l.contains("exit")
            || l.contains("trust")
    };
    let selected = screen.lines().map(str::trim_start).find(|t| {
        (t.starts_with('❯') || t.starts_with('›') || t.starts_with('▸') || t.starts_with("> "))
            && option_like(t)
    })?;
    let l = selected.to_lowercase();
    let positive =
        l.contains("yes") && !l.contains("no,") && !l.contains("quit") && !l.contains("exit");
    Some(if positive {
        TrustKey::Confirm
    } else {
        TrustKey::Down
    })
}

#[derive(Debug, Clone)]
pub struct HarnessExecutorConfig {
    pub global: GlobalConfig,
    /// The `ostra` binary serving `hook` and `mcp-stdio`.
    pub ostra_binary: PathBuf,
    /// Base URL of the running server, for example `http://127.0.0.1:4100`.
    pub server_url: String,
    pub home: PathBuf,
    pub idle_nudge: Duration,
    pub cols: u16,
    pub rows: u16,
}

impl HarnessExecutorConfig {
    pub fn new(global: GlobalConfig, ostra_binary: PathBuf, server_url: String) -> Self {
        HarnessExecutorConfig {
            global,
            ostra_binary,
            server_url,
            home: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
            idle_nudge: DEFAULT_IDLE_NUDGE,
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
        }
    }
}

pub struct HarnessExecutor {
    config: RwLock<HarnessExecutorConfig>,
    live: Arc<LiveRegistry>,
    ptys: Arc<PtyRegistry>,
}

fn launch_error(message: String) -> String {
    format!("{LAUNCH_PREFIX} {message}")
}

impl HarnessExecutor {
    pub fn new(
        config: HarnessExecutorConfig,
        live: Arc<LiveRegistry>,
        ptys: Arc<PtyRegistry>,
    ) -> Self {
        HarnessExecutor {
            config: RwLock::new(config),
            live,
            ptys,
        }
    }

    pub fn live(&self) -> Arc<LiveRegistry> {
        self.live.clone()
    }

    pub fn ptys(&self) -> Arc<PtyRegistry> {
        self.ptys.clone()
    }

    /// Settings are re-read on every execution; the server calls this when they change.
    pub fn set_global(&self, global: GlobalConfig) {
        self.config.write().global = global;
    }

    pub fn config(&self) -> HarnessExecutorConfig {
        self.config.read().clone()
    }

    async fn launch(
        &self,
        spec: &ExecutionSpec,
        harness: HarnessKind,
        live: &Arc<LiveExecution>,
        host: &Arc<dyn ExecutionHost>,
    ) -> Result<Arc<PtySession>, String> {
        let cfg = self.config();
        let command = cfg.global.harness_command(harness);
        if which::which(&command).is_err() {
            return Err(launch_error(format!(
                "`{command}` ({}) is not installed",
                harness.display_name()
            )));
        }
        if harness == HarnessKind::Agy {
            crate::setup::ensure_agy_integration(&cfg.global, &cfg.ostra_binary, &cfg.home)
                .await
                .map_err(|e| {
                    launch_error(format!(
                        "installing the Antigravity integration failed: {e}"
                    ))
                })?;
        }
        let config_dir = harness_execution_dir(&spec.ctx.session_root, spec.id.as_str());
        std::fs::create_dir_all(&config_dir)
            .map_err(|e| launch_error(format!("creating {}: {e}", config_dir.display())))?;
        let input = LaunchInput {
            spec,
            harness,
            command,
            extra_args: cfg
                .global
                .harness
                .get(harness.as_str())
                .map(|h| h.args.clone())
                .unwrap_or_default(),
            ostra_binary: cfg.ostra_binary.clone(),
            server_url: cfg.server_url.clone(),
            token: live.token().to_string(),
            config_dir,
            resume_session: spec
                .resume
                .as_ref()
                .and_then(|r| r.native_session_id.clone()),
            home: cfg.home.clone(),
        };
        let plan = launch::plan(&input);
        launch::materialize(&plan)
            .map_err(|e| launch_error(format!("writing the harness config: {e}")))?;
        if let Some(sid) = &plan.session_id {
            live.note_session(Some(sid.clone()), None);
        }
        let log = TermLog::create(terminal_transcript(
            &spec.ctx.session_root,
            spec.id.as_str(),
        ))
        .ok();
        let pty = PtySession::spawn(
            &plan,
            cfg.cols,
            cfg.rows,
            Box::new(move |b| {
                if let Some(log) = &log {
                    log.append(b);
                }
            }),
        )
        .map_err(|e| launch_error(format!("starting `{}`: {e}", plan.program)))?;
        host.emit(ExecutionDelta::Status {
            message: format!(
                "{} is running in the terminal on `{}`. Every tool call it makes goes through Ostra's guards.",
                harness.display_name(),
                spec.route.model
            ),
        });
        Ok(pty)
    }

    async fn supervise(
        &self,
        spec: &ExecutionSpec,
        harness: HarnessKind,
        live: &Arc<LiveExecution>,
        pty: &Arc<PtySession>,
        host: &Arc<dyn ExecutionHost>,
        cancel: &CancellationToken,
    ) -> End {
        let idle_nudge = self.config().idle_nudge;
        let started = Instant::now();
        let budget = spec.timeout_secs.max(30);
        let deadline = started + Duration::from_secs(budget);
        let (mut trust_steps, mut nudges) = (0u32, 0u32);
        let mut submit_seen_at: Option<Instant> = None;
        let mut reported_session: Option<String> = None;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return End::Cancelled,
                _ = live.changed(Duration::from_millis(500)) => {}
                info = pty.wait_exit() => {
                    return if live.has_submit() { End::Submitted } else { End::Exited(info.code, started.elapsed()) };
                }
            }
            let s = live.snapshot();
            if s.session_id.is_some() && s.session_id != reported_session {
                reported_session = s.session_id.clone();
                host.emit(ExecutionDelta::NativeSessionId {
                    id: reported_session.clone().unwrap_or_default(),
                });
            }
            if s.submit.is_some() {
                let at = *submit_seen_at.get_or_insert_with(Instant::now);
                let agy_quiet =
                    harness == HarnessKind::Agy && pty.idle_for() >= AGY_QUIET_AFTER_SUBMIT;
                if s.stopped_after_submit || agy_quiet || at.elapsed() >= AFTER_SUBMIT_GRACE {
                    return End::Submitted;
                }
                continue;
            }
            if s.gave_up {
                return End::GaveUp;
            }
            if Instant::now() >= deadline {
                return End::Timeout(budget);
            }
            if s.tool_calls == 0 && started.elapsed() < Duration::from_secs(120) {
                let text = pty.screen_text().to_lowercase();
                if trust_steps < 6 && TRUST_MARKERS.iter().any(|m| text.contains(m)) {
                    trust_steps += 1;
                    match trust_key(&pty.screen_text()) {
                        None => {}
                        Some(TrustKey::Confirm) => {
                            let _ = pty.write(b"\r");
                            host.emit(ExecutionDelta::Status {
                                message: "Accepted the harness's folder-trust prompt for this imported project.".into(),
                            });
                        }
                        Some(TrustKey::Down) => {
                            let _ = pty.write(b"\x1b[B");
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                if AUTH_MARKERS.iter().any(|m| text.contains(m)) {
                    return End::Auth;
                }
                if let Some(m) = FATAL_MARKERS.iter().find(|m| text.contains(**m)) {
                    return End::Fatal(format!("the harness reported \"{m}\""));
                }
                if s.session_id.is_none()
                    && let Some(id) = screen_session_id(&text)
                {
                    live.note_session(Some(id), None);
                }
            }
            if s.last_activity.elapsed().min(pty.idle_for()) >= idle_nudge {
                if nudges >= MAX_IDLE_NUDGES {
                    return End::Idle;
                }
                nudges += 1;
                live.touch();
                let _ = pty.type_line(&missing_submit_instruction(spec.agent)).await;
                host.emit(ExecutionDelta::Status {
                    message: "The session went quiet; Ostra reminded it to submit.".into(),
                });
            }
        }
    }
}

#[async_trait::async_trait]
impl Executor for HarnessExecutor {
    async fn run(
        &self,
        spec: ExecutionSpec,
        host: Arc<dyn ExecutionHost>,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        let ExecutorKind::Harness(harness) = spec.route.executor else {
            return ExecutionResult::error("the harness executor was given a native route");
        };
        let live = self.live.register(spec.id.clone(), spec.agent, harness);
        let result = match self.launch(&spec, harness, &live, &host).await {
            Ok(pty) => {
                self.ptys.insert(spec.id.clone(), pty.clone());
                let home = self.config().home;
                let stop_following = cancel.child_token();
                let follower = tokio::spawn(crate::usage_watch::follow(
                    harness,
                    home.clone(),
                    live.clone(),
                    host.clone(),
                    stop_following.clone(),
                ));
                let end = self
                    .supervise(&spec, harness, &live, &pty, &host, &cancel)
                    .await;
                // Stopped before the final result, so a late live reading cannot overwrite it.
                stop_following.cancel();
                let _ = follower.await;
                let screen = pty.screen_text();
                pty.terminate(Duration::from_secs(3)).await;
                self.ptys.remove(&spec.id);
                outcome::result(
                    end,
                    harness,
                    &spec.agent.submit_tool_name(),
                    &live.snapshot(),
                    &screen,
                    &home,
                )
            }
            Err(message) => ExecutionResult::error(message),
        };
        self.live.remove(&spec.id);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_from_screen() {
        assert_eq!(
            screen_session_id("Resume with -c (or command below):\nagy --conversation=2c1a2063-f047-4000-b905-83620a93da8e\n").as_deref(),
            Some("2c1a2063-f047-4000-b905-83620a93da8e")
        );
        assert_eq!(screen_session_id("nothing"), None);
    }

    #[test]
    fn trust_prompt_moves_to_yes() {
        let no_first = " Quick safety check: Is this a project you created or one you trust?\n ❯ No, exit\n   Yes, I trust this folder\n";
        assert_eq!(trust_key(no_first), Some(TrustKey::Down));
        let yes = "   No, exit\n ❯ Yes, I trust this folder\n";
        assert_eq!(trust_key(yes), Some(TrustKey::Confirm));
        assert_eq!(
            trust_key("❯ 1. Yes, proceed\n  2. No, exit"),
            Some(TrustKey::Confirm)
        );
        let codex = "  › Ask Codex to do anything\n Do you trust the contents of this directory?\n› 1. Yes, continue\n  2. No, quit";
        assert_eq!(trust_key(codex), Some(TrustKey::Confirm));
        assert_eq!(trust_key("  › Ask Codex to do anything"), None);
    }
}
