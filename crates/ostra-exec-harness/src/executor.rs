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
    CancellationToken, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec,
    ExecutionStatus, Executor,
};
use ostra_core::paths::{harness_execution_dir, terminal_transcript};
use ostra_core::{ExecutorKind, HarnessKind};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a running harness's repositories are checked for a planted `commondir`.
const GIT_CHECK_EVERY: Duration = Duration::from_secs(2);

/// Undoes a planted `commondir` in the execution's repositories and reports each as a denial by
/// the `git-metadata` guard, which counts as a containment signal.
fn report_git_repairs(repos: &[PathBuf], host: &Arc<dyn ExecutionHost>) {
    for path in ostra_core::sandbox::repair_git_dirs(repos) {
        host.emit(ostra_core::exec::ExecutionDelta::Policy {
            call_id: String::new(),
            decision: ostra_core::policy::PolicyDecision::deny(
                ostra_core::policy::RuleRef::guard(ostra_core::containment::GIT_METADATA),
                ostra_core::sandbox::git_repair_note(&path),
            ),
        });
    }
}

/// How long a session may sit with no hook event and no terminal output before it is nudged.
pub const DEFAULT_IDLE_NUDGE: Duration = Duration::from_secs(240);
const MAX_IDLE_NUDGES: u32 = 2;
/// After the submit, how long to wait for the turn to end before closing the harness.
const AFTER_SUBMIT_GRACE: Duration = Duration::from_secs(20);
/// Antigravity has no Stop hook: a submit followed by this much terminal quiet ends the turn.
const AGY_QUIET_AFTER_SUBMIT: Duration = Duration::from_secs(4);

/// Screen text that means the harness needs its user to log in: an error, or a sign-in screen
/// that would otherwise wait out the whole budget.
pub const AUTH_MARKERS: &[&str] = &[
    "please log in",
    "please login",
    "not logged in",
    "run /login",
    "invalid api key",
    "login required",
    "authentication required",
    "oauth token has expired",
    // Grok Build's browser sign-in, shown when its saved token has expired.
    "finish signing in",
    "make sure your browser shows this code",
    // Claude Code's and Codex's first-run sign-in choices.
    "select login method",
    "sign in with chatgpt",
    // Gemini-family CLIs waiting on a browser sign-in.
    "waiting for auth",
    "login with google",
];

/// The screen shows a sign-in prompt or a login error.
fn needs_login(screen_lower: &str) -> bool {
    AUTH_MARKERS.iter().any(|m| screen_lower.contains(m))
}

/// Screen text that means the harness could not start the session at all.
pub const FATAL_MARKERS: &[&str] = &[
    "failed to construct executor",
    "unknown component:",
    "unknown model",
    "model not found",
];

/// Folder-trust prompts that must be answered before the session starts. See [`trust_answer`]
/// for which ones Ostra accepts.
pub const TRUST_MARKERS: &[&str] = &[
    "do you trust the files in this folder",
    "do you trust the contents of this directory",
    "trust this folder",
    "trust this project",
    "i trust this folder",
    "open restricted",
];

/// A new-model offer at startup (Codex 0.157 "Meet GPT-6 Luna"). Ostra keeps the routed model,
/// because accepting the offer switches the run and the user's default model.
pub const MODEL_OFFER_MARKERS: &[&str] = &["use existing model"];

/// Which key keeps the current model on a new-model offer: Enter when "Use existing model" is
/// highlighted, else Down to move to it.
fn keep_model_key(screen: &str) -> Option<TrustKey> {
    let selected = screen.lines().map(str::trim_start).find(|t| {
        t.starts_with('❯') || t.starts_with('›') || t.starts_with('▸') || t.starts_with("> ")
    });
    let l = selected?.to_lowercase();
    if l.contains("use existing") {
        Some(TrustKey::Confirm)
    } else if l.contains("try new") {
        Some(TrustKey::Down)
    } else {
        None
    }
}

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
    /// End the run with this reason instead of answering.
    Refuse(String),
}

/// Which key answers "yes" to a trust prompt: Enter when the highlighted option trusts the
/// folder, else Down to move to it. Prompt defaults differ between versions ("No, exit" is first
/// on Claude Code 2.1.280).
/// How to answer a harness's folder-trust prompt.
/// - Claude Code: yes. It loads only user settings (`--setting-sources user`) and only Ostra's MCP
///   config (`--strict-mcp-config`), so trust adds nothing from the repository.
/// - Codex: "Open restricted" only, which the restricted profile offers. Its "Trust and continue"
///   would save trust and load the repository's `.codex/` config and hooks, so it is refused.
/// - Grok Build: never. Trust makes it run the folder's own hooks and MCP servers, so the user
///   decides it in Grok.
fn trust_answer(harness: HarnessKind, repo: &std::path::Path, screen: &str) -> Option<TrustKey> {
    match harness {
        HarnessKind::Codex if screen.to_lowercase().contains("open restricted") => {
            restricted_key(screen)
        }
        HarnessKind::Codex => Some(TrustKey::Refuse(
            "remove `-p`/`--profile` from `[harness.codex].args` so Ostra can open the folder restricted, because Codex asked to trust it, and a trusted folder's own Codex config and hooks run outside Ostra's guards".into(),
        )),
        HarnessKind::Grok => Some(TrustKey::Refuse(format!(
            "trust `{}` in Grok yourself (run `grok` in that folder and answer yes), then run this again, because Ostra does not trust folders for you: Grok runs a trusted folder's own hooks and MCP servers outside Ostra's guards",
            repo.display()
        ))),
        HarnessKind::Claude | HarnessKind::Agy => trust_key(screen),
    }
}

/// Enter on "Open restricted", else Down toward it.
fn restricted_key(screen: &str) -> Option<TrustKey> {
    let selected = screen.lines().map(str::trim_start).find(|t| {
        t.starts_with('❯') || t.starts_with('›') || t.starts_with('▸') || t.starts_with("> ")
    })?;
    Some(if selected.to_lowercase().contains("open restricted") {
        TrustKey::Confirm
    } else {
        TrustKey::Down
    })
}

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
    /// The server's Unix socket that answers only the harness callbacks. A sandbox with its own
    /// network reaches the bridge through it instead of the whole server.
    pub bridge_socket: Option<PathBuf>,
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
            bridge_socket: None,
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
        let sandbox = cfg.global.sandbox.for_workspace(&spec.ctx.sandbox());
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
            bridge_socket: cfg.bridge_socket.clone(),
            token: live.token().to_string(),
            config_dir,
            resume_session: spec
                .resume
                .as_ref()
                .and_then(|r| r.native_session_id.clone()),
            home: cfg.home.clone(),
            sandboxed: matches!(
                ostra_core::sandbox::decide(&sandbox),
                Ok(ostra_core::sandbox::Decision::Sandboxed(_))
            ),
            credential_env: {
                let ws: ostra_core::config::WorkspaceSettings = ostra_core::config::load_toml(
                    &ostra_core::paths::workspace_toml(&spec.ctx.workspace_root),
                )
                .unwrap_or_default();
                ostra_core::config::credential_env_names(&cfg.global, &ws.mcp_servers)
                    .into_iter()
                    .collect()
            },
        };
        let plan = launch::plan(&input);
        launch::materialize(&plan)
            .map_err(|e| launch_error(format!("writing the harness config: {e}")))?;
        let crate::sandbox::Wrapped {
            plan,
            members,
            warning: unsandboxed,
        } = crate::sandbox::wrap(
            plan,
            &input,
            &sandbox,
            ostra_core::exec::egress_reporter(host.clone()),
            ostra_core::exec::decoy_reporter(host.clone()),
        )
        .map_err(launch_error)?;
        if let Some(w) = unsandboxed {
            if ostra_core::sandbox::first_warning() {
                tracing::warn!("agent commands run without a sandbox: {w}");
            }
            host.emit(ExecutionDelta::Status {
                message: format!("{} runs without a sandbox. {w}", harness.display_name()),
            });
        }
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
        pty.hold(members);
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
        live: &Arc<LiveExecution>,
        pty: &Arc<PtySession>,
        host: &Arc<dyn ExecutionHost>,
        repos: &[PathBuf],
        cancel: &CancellationToken,
    ) -> End {
        let harness = live.harness;
        let idle_nudge = self.config().idle_nudge;
        let started = Instant::now();
        let budget = spec.timeout_secs.max(30);
        let deadline = started + Duration::from_secs(budget);
        let (mut trust_steps, mut nudges) = (0u32, 0u32);
        let mut submit_seen_at: Option<Instant> = None;
        let mut reported_session: Option<String> = None;
        let inspect = spec.resume.as_ref().is_some_and(|r| r.inspect);
        let mut git_checked = Instant::now();
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return End::Cancelled,
                _ = live.changed(Duration::from_millis(500)) => {}
                info = pty.wait_exit() => {
                    return if live.has_submit() { End::Submitted } else { End::Exited(info.code, started.elapsed()) };
                }
            }
            // The CLI's commands never pass a hook Ostra could check, so its repos are checked on a
            // clock, keeping short the time a planted `commondir` could reach the user's own git.
            if git_checked.elapsed() >= GIT_CHECK_EVERY {
                git_checked = Instant::now();
                report_git_repairs(repos, host);
            }
            let s = live.snapshot();
            if s.session_id.is_some() && s.session_id != reported_session {
                reported_session = s.session_id.clone();
                host.emit(ExecutionDelta::NativeSessionId {
                    id: reported_session.clone().unwrap_or_default(),
                });
            }
            if !inspect && s.submit.is_some() {
                let at = *submit_seen_at.get_or_insert_with(Instant::now);
                let agy_quiet =
                    harness == HarnessKind::Agy && pty.idle_for() >= AGY_QUIET_AFTER_SUBMIT;
                if s.stopped_after_submit || agy_quiet || at.elapsed() >= AFTER_SUBMIT_GRACE {
                    return End::Submitted;
                }
                continue;
            }
            if !inspect && s.gave_up {
                return End::GaveUp;
            }
            if Instant::now() >= deadline {
                return End::Timeout(budget);
            }
            if s.tool_calls == 0 && started.elapsed() < Duration::from_secs(120) {
                let text = pty.screen_text().to_lowercase();
                if trust_steps < 6 && TRUST_MARKERS.iter().any(|m| text.contains(m)) {
                    trust_steps += 1;
                    match trust_answer(harness, &spec.ctx.repo_root, &pty.screen_text()) {
                        None => {}
                        Some(TrustKey::Refuse(why)) => return End::Fatal(why),
                        Some(TrustKey::Confirm) => {
                            let _ = pty.write(b"\r");
                            host.emit(ExecutionDelta::Status {
                                message: if harness == HarnessKind::Codex {
                                    "Opened the project restricted in Codex, so the repository's own Codex config and hooks stay off.".into()
                                } else {
                                    "Accepted the harness's folder-trust prompt for this imported project.".into()
                                },
                            });
                        }
                        Some(TrustKey::Down) => {
                            let _ = pty.write(b"\x1b[B");
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                if MODEL_OFFER_MARKERS.iter().any(|m| text.contains(m)) {
                    match keep_model_key(&pty.screen_text()) {
                        None | Some(TrustKey::Refuse(_)) => {}
                        Some(TrustKey::Confirm) => {
                            let _ = pty.write(b"\r");
                            host.emit(ExecutionDelta::Status {
                                message: "Declined the harness's new-model offer, so the run keeps its routed model.".into(),
                            });
                        }
                        Some(TrustKey::Down) => {
                            let _ = pty.write(b"\x1b[B");
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                if needs_login(&text) {
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
            // The user drives an inspection; it ends when they leave the CLI or stop it.
            if !inspect && s.last_activity.elapsed().min(pty.idle_for()) >= idle_nudge {
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
        if spec.resume.as_ref().is_some_and(|r| r.inspect) {
            live.set_inspect();
        }
        let repos =
            ostra_core::sandbox::git_repos(&[&spec.ctx.repo_root, &spec.ctx.workspace_root]);
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
                    .supervise(&spec, &live, &pty, &host, &repos, &cancel)
                    .await;
                report_git_repairs(&repos, &host);
                // Stopped before the final result, so a late live reading cannot overwrite it.
                stop_following.cancel();
                let _ = follower.await;
                if matches!(end, End::Cancelled) {
                    // Esc ends the CLI's turn so its session is saved whole and can be resumed.
                    let _ = pty.write(b"\x1b");
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                let screen = pty.screen_text();
                pty.terminate(Duration::from_secs(3)).await;
                self.ptys.remove(&spec.id);
                crate::sandbox::cleanup(&harness_execution_dir(
                    &spec.ctx.session_root,
                    spec.id.as_str(),
                ));
                if harness == HarnessKind::Codex {
                    let _ = std::fs::remove_file(crate::launch::codex_profile_path(
                        &home,
                        spec.id.as_str(),
                    ));
                }
                let inspect = spec.resume.as_ref().is_some_and(|r| r.inspect);
                if inspect && matches!(end, End::Exited(..) | End::Timeout(_)) {
                    let mut r = ExecutionResult::with_status(ExecutionStatus::Ok);
                    r.native_session_id = live.snapshot().session_id;
                    r.final_text = "The read-only session ended.".into();
                    r
                } else {
                    outcome::result(
                        end,
                        harness,
                        &spec.agent.submit_tool_name(),
                        &live.snapshot(),
                        &screen,
                        &home,
                        spec.ctx.sandbox_mode,
                    )
                }
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
    fn codex_opens_restricted_and_never_saves_trust() {
        let repo = std::path::Path::new("/r");
        let restricted = "  Folder access  /r\n  Config, hooks, and exec policies from untrusted folders stay disabled.\n› 1. Open restricted\n  2. Quit\n";
        assert_eq!(
            trust_answer(HarnessKind::Codex, repo, restricted),
            Some(TrustKey::Confirm)
        );
        let on_quit = "  Folder access\n  1. Open restricted\n› 2. Quit\n";
        assert_eq!(
            trust_answer(HarnessKind::Codex, repo, on_quit),
            Some(TrustKey::Down)
        );
        let trust = "  Trust this folder? Codex can read, edit, and run files here.\n› 1. Trust and continue\n  2. Quit\n";
        assert!(matches!(
            trust_answer(HarnessKind::Codex, repo, trust),
            Some(TrustKey::Refuse(why)) if why.contains("--profile")
        ));
    }

    #[test]
    fn grok_trust_is_left_to_the_user() {
        let prompt = "Do you trust the contents of this directory?\n  /r\n❯ Yes, proceed   y\n  No, quit   n\n";
        assert!(matches!(
            trust_answer(HarnessKind::Grok, std::path::Path::new("/r"), prompt),
            Some(TrustKey::Refuse(why)) if why.contains("trust `/r` in Grok")
        ));
        let claude = "   No, exit\n ❯ Yes, I trust this folder\n";
        assert_eq!(
            trust_answer(HarnessKind::Claude, std::path::Path::new("/r"), claude),
            Some(TrustKey::Confirm)
        );
    }

    #[test]
    fn a_new_model_offer_keeps_the_routed_model() {
        let offer =
            "  Meet GPT-6 Luna\n\n› 1. Try new model\n  2. Use existing model\n  enter/esc confirm";
        assert!(matches!(keep_model_key(offer), Some(TrustKey::Down)));
        let moved = "  1. Try new model\n› 2. Use existing model\n";
        assert!(matches!(keep_model_key(moved), Some(TrustKey::Confirm)));
        assert!(keep_model_key("› Ask Codex to do anything").is_none());
    }

    #[test]
    fn session_id_from_screen() {
        assert_eq!(
            screen_session_id("Resume with -c (or command below):\nagy --conversation=2c1a2063-f047-4000-b905-83620a93da8e\n").as_deref(),
            Some("2c1a2063-f047-4000-b905-83620a93da8e")
        );
        assert_eq!(screen_session_id("nothing"), None);
    }

    #[test]
    fn sign_in_screens_are_recognized() {
        // Grok Build 1.0.41 with an expired token, as its terminal showed it.
        let grok = "Connecting...\nApprove in your browser to finish signing in.\nGA7K-2QXM\nMake sure your browser shows this code.If it doesn't open, click here to copy.\nq  quit";
        assert!(needs_login(&grok.to_lowercase()));
        assert!(needs_login(
            "select login method:\n❯ 1. claude account with subscription"
        ));
        assert!(!needs_login("  › ask codex to do anything"));
        assert!(!needs_login("reading src/auth/login.ts"));
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
