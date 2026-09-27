//! Per-harness launch plans: the command line, the environment, and the config files each CLI
//! needs so that every hook event reaches `ostra hook` and Ostra's tools are served by
//! `ostra mcp-stdio`. Each flag here was checked against the installed CLI (see `docs` in the
//! crate README section of the final report): claude 2.1.280, codex 0.153.4, grok 1.0.30,
//! agy 1.2.6.

use crate::protocol::*;
use ostra_core::agent::Capability;
use ostra_core::exec::ExecutionSpec;
use ostra_core::{Effort, HarnessKind};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Hook timeout in seconds. A permission ask waits for the user, so it must not time out first.
pub const HOOK_TIMEOUT_SECS: u64 = 86_400;

/// Largest first message passed on the command line. Longer ones go through a file.
pub const MAX_ARG_PROMPT: usize = 96 * 1024;

#[derive(Debug, Clone)]
pub struct LaunchInput<'a> {
    pub spec: &'a ExecutionSpec,
    pub harness: HarnessKind,
    /// CLI binary, from `[harness.<name>].command`.
    pub command: String,
    pub extra_args: Vec<String>,
    /// Absolute path of the `ostra` binary that serves `hook` and `mcp-stdio`.
    pub ostra_binary: PathBuf,
    pub server_url: String,
    /// See [`crate::executor::HarnessExecutorConfig::bridge_socket`].
    pub bridge_socket: Option<PathBuf>,
    pub token: String,
    /// Per-execution config dir, created by the caller.
    pub config_dir: PathBuf,
    /// Harness session to resume instead of starting a new one.
    pub resume_session: Option<String>,
    /// Home dirs, overridable for tests.
    pub home: PathBuf,
    /// Credential variables of Ostra's environment (see
    /// [`ostra_core::config::credential_env_names`]). The harness inherits none of them except
    /// those its own CLI reads to sign in.
    pub credential_env: Vec<String>,
    /// Ostra runs the CLI in its sandbox, so the CLI must not start one of its own: macOS cannot
    /// nest Seatbelt sandboxes.
    pub sandboxed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchPlan {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// Written before launch, parent directories created.
    pub files: Vec<(PathBuf, String)>,
    /// Symlinks created before launch: `(link, target)`.
    pub links: Vec<(PathBuf, PathBuf)>,
    /// The session id Ostra chose up front (Claude Code, Grok Build).
    pub session_id: Option<String>,
    /// Variables of Ostra's environment the harness must not inherit.
    pub env_remove: Vec<String>,
    /// A `sandbox-exec -D` parameter the PTY launcher sets to the PTY's device path, which is
    /// only known once the PTY is open. Set by the Seatbelt wrap.
    pub tty_param: Option<String>,
}

/// POSIX single-quote a word for a hook command line, which harnesses run through a shell.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn hook_command(inp: &LaunchInput<'_>, event: HookEvent, with_execution: bool) -> String {
    let mut words = vec![
        shell_quote(&inp.ostra_binary.to_string_lossy()),
        "hook".into(),
    ];
    if with_execution {
        words.push("--execution".into());
        words.push(shell_quote(inp.spec.id.as_str()));
    }
    words.extend([
        "--harness".into(),
        inp.harness.as_str().into(),
        "--event".into(),
        event.as_str().into(),
    ]);
    words.join(" ")
}

fn base_env(inp: &LaunchInput<'_>) -> Vec<(String, String)> {
    vec![
        (ENV_URL.into(), inp.server_url.clone()),
        (ENV_EXECUTION.into(), inp.spec.id.to_string()),
        (ENV_TOKEN.into(), inp.token.clone()),
        (ENV_HARNESS.into(), inp.harness.as_str().into()),
    ]
}

fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// The first message, or a pointer to a file holding it when it is too long for argv.
fn first_prompt(inp: &LaunchInput<'_>, files: &mut Vec<(PathBuf, String)>) -> String {
    let msg = &inp.spec.first_message;
    if msg.len() <= MAX_ARG_PROMPT {
        return msg.clone();
    }
    let path = inp.config_dir.join("first-message.md");
    files.push((path.clone(), msg.clone()));
    format!(
        "Your full task is in `{}`. Read that whole file before your first other tool call, then do what it says.",
        path.display()
    )
}

/// Sent when a harness session is resumed: the resume note, or the interruption notice.
pub fn resume_prompt(spec: &ExecutionSpec) -> String {
    if let Some(note) = spec.resume.as_ref().and_then(|r| r.note.clone()) {
        return note;
    }
    let agent = spec.agent;
    format!(
        "Ostra resumed this session after an interruption. Continue the task from where it stopped: check \
         what is already done before redoing it, finish the rest, then call `{}` with your result.",
        agent.submit_tool_name()
    )
}

/// Claude Code tools of a read-only session. Every call is refused by the bridge.
const INSPECT_TOOLS: &str = "Read,Bash,Grep,Glob";

/// A read-only reopening of an ended session: the user types, and the CLI gets no prompt.
fn inspecting(spec: &ExecutionSpec) -> bool {
    spec.resume.as_ref().is_some_and(|r| r.inspect)
}

/// The prompt argument: the first message, the resume note, or none when inspecting.
fn prompt_arg(inp: &LaunchInput<'_>, files: &mut Vec<(PathBuf, String)>) -> Option<String> {
    match inp.resume_session {
        None => Some(first_prompt(inp, files)),
        Some(_) if inspecting(inp.spec) => None,
        Some(_) => Some(resume_prompt(inp.spec)),
    }
}

fn effort_word(e: Effort, max_supported: Effort) -> &'static str {
    let e = e.min(max_supported);
    e.as_str()
}

fn claude_tools(caps: &[Capability]) -> String {
    let mut tools: Vec<&str> = vec![];
    for c in caps {
        let t = match c {
            Capability::Read => "Read",
            Capability::Write => "Write",
            Capability::Edit => "Edit",
            Capability::Shell => "Bash",
            Capability::SearchText => "Grep",
            Capability::Glob => "Glob",
            Capability::WebSearch => "WebSearch",
            Capability::WebFetch => "WebFetch",
            _ => continue,
        };
        if !tools.contains(&t) {
            tools.push(t);
        }
    }
    if caps.contains(&Capability::Skill) && !tools.contains(&"Read") {
        tools.push("Read");
    }
    // Interactive Claude Code connects MCP servers after startup and exposes their tools only as
    // deferred tools, so without ToolSearch the `mcp__ostra__*` tools never reach the model.
    tools.push("ToolSearch");
    tools.join(",")
}

pub fn plan(inp: &LaunchInput<'_>) -> LaunchPlan {
    let mut p = match inp.harness {
        HarnessKind::Claude => plan_claude(inp),
        HarnessKind::Codex => plan_codex(inp),
        HarnessKind::Grok => plan_grok(inp),
        HarnessKind::Agy => plan_agy(inp),
    };
    let keep = sign_in_env(inp.harness);
    p.env_remove = inp
        .credential_env
        .iter()
        .filter(|k| !keep.contains(&k.as_str()))
        .cloned()
        .collect();
    p.env.extend(ostra_core::git::agent_env());
    p
}

/// Provider variables a harness CLI reads to sign in, so it keeps them. The agent's own shell
/// commands inside that CLI can still read them.
fn sign_in_env(harness: HarnessKind) -> &'static [&'static str] {
    match harness {
        HarnessKind::Claude => &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
        ],
        HarnessKind::Codex => &["OPENAI_API_KEY", "OPENAI_BASE_URL"],
        // Antigravity keeps its sign-in in the OS keychain, which the macOS sandbox denies.
        HarnessKind::Agy => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        HarnessKind::Grok => &[],
    }
}

// ---------------------------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------------------------

fn plan_claude(inp: &LaunchInput<'_>) -> LaunchPlan {
    let spec = inp.spec;
    let dir = &inp.config_dir;
    let mut files = vec![];
    let hook = |event: HookEvent| json!([{"hooks": [{"type": "command", "command": hook_command(inp, event, true), "timeout": HOOK_TIMEOUT_SECS}]}]);
    let tool_hook = |event: HookEvent| json!([{"matcher": "*", "hooks": [{"type": "command", "command": hook_command(inp, event, true), "timeout": HOOK_TIMEOUT_SECS}]}]);
    // A user settings file cannot switch off Ostra's hooks, because flag settings win over it.
    let mut settings = json!({
        "disableAllHooks": false,
        "hooks": {
            "PreToolUse": tool_hook(HookEvent::PreToolUse),
            "PostToolUse": tool_hook(HookEvent::PostToolUse),
            "PostToolUseFailure": tool_hook(HookEvent::PostToolUseFailure),
            "Stop": hook(HookEvent::Stop),
            "SessionStart": hook(HookEvent::SessionStart),
        }
    });
    if inp.sandboxed {
        settings["sandbox"] = json!({"enabled": false});
    }
    let settings_path = dir.join("claude-settings.json");
    files.push((
        settings_path.clone(),
        serde_json::to_string_pretty(&settings).unwrap_or_default(),
    ));
    let mcp = json!({"mcpServers": {MCP_SERVER_NAME: {
        "type": "stdio",
        "command": inp.ostra_binary.to_string_lossy(),
        "args": ["mcp-stdio", "--execution", spec.id.as_str()],
    }}});
    let mcp_path = dir.join("claude-mcp.json");
    files.push((
        mcp_path.clone(),
        serde_json::to_string_pretty(&mcp).unwrap_or_default(),
    ));
    let prompt_path = dir.join("system-prompt.md");
    files.push((prompt_path.clone(), spec.system_prompt.clone()));

    let mut args: Vec<String> = inp.extra_args.clone();
    let session_id = match &inp.resume_session {
        Some(sid) => {
            args.extend(["--resume".into(), sid.clone()]);
            None
        }
        None => {
            let sid = spec
                .harness_session_id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            args.extend(["--session-id".into(), sid.clone()]);
            Some(sid)
        }
    };
    // An inspection gets no ToolSearch and no MCP server. It keeps plain tools for Ostra to refuse,
    // because with none a model asked to run something replies with nothing.
    let tools = if inspecting(spec) {
        INSPECT_TOOLS.into()
    } else {
        claude_tools(&spec.capabilities)
    };
    // Project and local settings are the repository's own files, so their hooks, env, and
    // permissions would run outside Ostra's guards. Only the user's settings and Ostra's load.
    args.extend([
        "--setting-sources".into(),
        "user".into(),
        "--settings".into(),
        settings_path.to_string_lossy().into(),
        "--strict-mcp-config".into(),
    ]);
    if !inspecting(spec) {
        args.extend(["--mcp-config".into(), mcp_path.to_string_lossy().into()]);
    }
    args.extend([
        "--append-system-prompt-file".into(),
        prompt_path.to_string_lossy().into(),
        "--add-dir".into(),
        spec.ctx.session_dir.to_string_lossy().into(),
        "--disallowed-tools".into(),
        "Agent,Task,AskUserQuestion,EnterPlanMode,ExitPlanMode".into(),
        "--tools".into(),
        tools,
        "--permission-mode".into(),
        "default".into(),
        "--model".into(),
        spec.route.model.clone(),
        "--effort".into(),
        effort_word(spec.effort, Effort::Max).into(),
    ]);
    args.extend(artifacts_dir_args(spec));
    args.extend(prompt_arg(inp, &mut files));
    LaunchPlan {
        program: inp.command.clone(),
        args,
        env: base_env(inp),
        cwd: spec.ctx.repo_root.clone(),
        files,
        links: vec![],
        session_id,
        env_remove: vec![],
        tty_param: None,
    }
}

// ---------------------------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------------------------

fn plan_codex(inp: &LaunchInput<'_>) -> LaunchPlan {
    let spec = inp.spec;
    let mut files = vec![];
    let group = |event: HookEvent, matcher: bool| {
        let handler = format!(
            "{{type={}, command={}, timeout={HOOK_TIMEOUT_SECS}}}",
            toml_str("command"),
            toml_str(&hook_command(inp, event, true))
        );
        if matcher {
            format!("[{{matcher={}, hooks=[{handler}]}}]", toml_str(".*"))
        } else {
            format!("[{{hooks=[{handler}]}}]")
        }
    };
    let mut args: Vec<String> = vec![];
    if let Some(sid) = &inp.resume_session {
        args.extend(["resume".into(), sid.clone()]);
    }
    args.extend(inp.extra_args.iter().cloned());
    let mut config = |k: &str, v: String| {
        args.push("-c".into());
        args.push(format!("{k}={v}"));
    };
    config("hooks.PreToolUse", group(HookEvent::PreToolUse, true));
    config("hooks.PostToolUse", group(HookEvent::PostToolUse, true));
    config("hooks.Stop", group(HookEvent::Stop, false));
    config("hooks.SessionStart", group(HookEvent::SessionStart, false));
    config(
        "mcp_servers.ostra.command",
        toml_str(&inp.ostra_binary.to_string_lossy()),
    );
    config(
        "mcp_servers.ostra.args",
        format!(
            "[{}, {}, {}]",
            toml_str("mcp-stdio"),
            toml_str("--execution"),
            toml_str(spec.id.as_str())
        ),
    );
    config(
        "mcp_servers.ostra.env_vars",
        format!(
            "[{}]",
            [ENV_URL, ENV_EXECUTION, ENV_TOKEN, ENV_HARNESS]
                .map(toml_str)
                .join(", ")
        ),
    );
    config(
        "mcp_servers.ostra.tool_timeout_sec",
        HOOK_TIMEOUT_SECS.to_string(),
    );
    // Codex updates itself on startup and then exits, which ends the run before it starts.
    config("check_for_update_on_startup", "false".into());
    config("developer_instructions", toml_str(&spec.system_prompt));
    config(
        "model_reasoning_effort",
        toml_str(effort_word(spec.effort, Effort::Xhigh)),
    );
    // Codex reads folder trust only from config files, not from `-c`. A profile that marks the
    // repository untrusted opens it restricted: its `.codex/` config, hooks, and exec policies
    // stay off, and the user's saved trust is untouched.
    let has_profile = inp
        .extra_args
        .iter()
        .any(|a| a == "-p" || a == "--profile" || a.starts_with("--profile="));
    if !has_profile {
        let (name, path, body) = codex_restricted_profile(inp);
        files.push((path, body));
        args.extend(["-p".into(), name]);
    }
    args.extend([
        "--disable".into(),
        "multi_agent".into(),
        "--dangerously-bypass-approvals-and-sandbox".into(),
        // Ostra's hooks change with every execution, so they can never hold persisted hook trust.
        // Project hooks stay off anyway, because the folder opens restricted.
        "--dangerously-bypass-hook-trust".into(),
        "-C".into(),
        spec.ctx.repo_root.to_string_lossy().into(),
        "-m".into(),
        spec.route.model.clone(),
    ]);
    args.extend(prompt_arg(inp, &mut files));
    LaunchPlan {
        program: inp.command.clone(),
        args,
        env: base_env(inp),
        cwd: spec.ctx.repo_root.clone(),
        files,
        links: vec![],
        session_id: None,
        env_remove: vec![],
        tty_param: None,
    }
}

pub fn codex_home_real(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
}

/// The repository and each parent up to its git root, since Codex applies project config from
/// every folder between the two.
fn codex_project_dirs(repo_root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![];
    for dir in repo_root.ancestors().take(32) {
        dirs.push(dir.to_path_buf());
        if dir.join(".git").exists() {
            return dirs;
        }
    }
    vec![repo_root.to_path_buf()]
}

/// The Codex profile name of one execution. Each execution has its own, so removing it at the
/// end never affects another run.
fn codex_profile_name(execution: &str) -> String {
    let id: String = execution
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("ostra-restricted-{id}")
}

/// The Codex profile file [`plan`] writes for `execution`, removed when the execution ends.
pub fn codex_profile_path(home: &Path, execution: &str) -> PathBuf {
    codex_home_real(home).join(format!("{}.config.toml", codex_profile_name(execution)))
}

/// `(profile name, file, contents)` of the Codex profile that marks this repository untrusted.
fn codex_restricted_profile(inp: &LaunchInput<'_>) -> (String, PathBuf, String) {
    let repo = &inp.spec.ctx.repo_root;
    let name = codex_profile_name(inp.spec.id.as_str());
    let path = codex_profile_path(&inp.home, inp.spec.id.as_str());
    let mut body =
        String::from("# Written by Ostra: Codex runs for Ostra open these folders restricted.\n");
    for dir in codex_project_dirs(repo) {
        body.push_str(&format!(
            "\n[projects.{}]\ntrust_level = \"untrusted\"\n",
            toml_str(&dir.to_string_lossy())
        ));
    }
    (name, path, body)
}

// ---------------------------------------------------------------------------------------------
// Grok Build
// ---------------------------------------------------------------------------------------------

/// Entries of the real `GROK_HOME` that the per-execution home must not share: the config Ostra
/// rewrites, the trust list it extends, and the leader socket, so the session runs its own leader
/// and reads this execution's hooks.
const GROK_PRIVATE: &[&str] = &["config.toml", "trusted_folders.toml", "leader.sock"];

pub fn grok_home_real(home: &Path) -> PathBuf {
    std::env::var_os("GROK_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".grok"))
}

fn plan_grok(inp: &LaunchInput<'_>) -> LaunchPlan {
    let spec = inp.spec;
    let real = grok_home_real(&inp.home);
    let farm = inp.config_dir.join("grok-home");
    let mut files = vec![];
    let mut links = vec![];
    if let Ok(entries) = std::fs::read_dir(&real) {
        for e in entries.flatten() {
            let name = e.file_name();
            let name_s = name.to_string_lossy();
            if GROK_PRIVATE.contains(&name_s.as_ref())
                || name_s.starts_with("leader-")
                || name_s.ends_with(".lock")
            {
                continue;
            }
            links.push((farm.join(&name), e.path()));
        }
    }
    let user_config = std::fs::read_to_string(real.join("config.toml")).unwrap_or_default();
    let hook_block = |event: &str, e: HookEvent, matcher: bool| {
        let mut s = format!("\n[[hooks.{event}]]\n");
        if matcher {
            s.push_str("matcher = \"*\"\n");
        }
        s.push_str(&format!(
            "  [[hooks.{event}.hooks]]\n  type = \"command\"\n  command = {}\n  timeout = {HOOK_TIMEOUT_SECS}\n",
            toml_str(&hook_command(inp, e, true))
        ));
        s
    };
    let mut config = String::new();
    config.push_str(&strip_ostra_sections(&user_config));
    config.push_str("\n# Added by Ostra for one execution.\n");
    config.push_str(&hook_block("PreToolUse", HookEvent::PreToolUse, true));
    config.push_str(&hook_block("PostToolUse", HookEvent::PostToolUse, true));
    config.push_str(&hook_block("Stop", HookEvent::Stop, false));
    config.push_str(&hook_block("SessionStart", HookEvent::SessionStart, false));
    config.push_str(&format!(
        "\n[mcp_servers.{MCP_SERVER_NAME}]\ncommand = {}\nargs = [\"mcp-stdio\", \"--execution\", {}]\ntool_timeout_sec = {HOOK_TIMEOUT_SECS}\n",
        toml_str(&inp.ostra_binary.to_string_lossy()),
        toml_str(spec.id.as_str()),
    ));
    let env_table: Vec<String> = base_env(inp)
        .iter()
        .map(|(k, v)| format!("{k} = {}", toml_str(v)))
        .collect();
    config.push_str(&format!("env = {{ {} }}\n", env_table.join(", ")));
    files.push((farm.join("config.toml"), config));

    // The repository is trusted only if the user trusted it in Grok, because Grok runs a trusted
    // folder's own hooks and MCP servers. The session dir is Ostra's own.
    let user_trust = std::fs::read_to_string(real.join("trusted_folders.toml")).unwrap_or_default();
    let mut trust = user_trust.clone();
    let key = toml_str(&spec.ctx.session_dir.to_string_lossy());
    if !user_trust.contains(&key) {
        trust.push_str(&format!(
            "\n[folders.{key}]\ntrusted = true\ndecided_at = 0\n"
        ));
    }
    files.push((farm.join("trusted_folders.toml"), trust));

    let mut args: Vec<String> = inp.extra_args.clone();
    let session_id = match &inp.resume_session {
        Some(sid) => {
            args.extend(["--resume".into(), sid.clone()]);
            None
        }
        None => {
            let sid = spec
                .harness_session_id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            args.extend(["--session-id".into(), sid.clone()]);
            Some(sid)
        }
    };
    args.extend([
        "--leader-socket".into(),
        farm.join("leader-ostra.sock").to_string_lossy().into(),
        "--cwd".into(),
        spec.ctx.repo_root.to_string_lossy().into(),
        "--no-subagents".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
        "--rules".into(),
        spec.system_prompt.clone(),
        "-m".into(),
        spec.route.model.clone(),
        "--reasoning-effort".into(),
        effort_word(spec.effort, Effort::High).into(),
    ]);
    args.extend(prompt_arg(inp, &mut files));
    let mut env = base_env(inp);
    env.push(("GROK_HOME".into(), farm.to_string_lossy().into()));
    env.push(("GROK_SUBAGENTS".into(), "0".into()));
    LaunchPlan {
        program: inp.command.clone(),
        args,
        env,
        cwd: spec.ctx.repo_root.clone(),
        files,
        links,
        session_id,
        env_remove: vec![],
        tty_param: None,
    }
}

/// Drop any `[mcp_servers.ostra]` table a user config already carries, so the one Ostra adds is
/// the only definition.
fn strip_ostra_sections(config: &str) -> String {
    let header = format!("[mcp_servers.{MCP_SERVER_NAME}]");
    let mut out = String::new();
    let mut skipping = false;
    for line in config.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            skipping = t == header;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Antigravity
// ---------------------------------------------------------------------------------------------

/// Antigravity reads hooks and MCP servers only from global config, so Ostra installs one
/// integration that does nothing unless `$OSTRA_EXECUTION` is set. See [`crate::setup`].
fn plan_agy(inp: &LaunchInput<'_>) -> LaunchPlan {
    let spec = inp.spec;
    let mut files = vec![];
    let agent_name = crate::setup::agy_agent_name(spec.agent);
    files.push((
        crate::setup::agy_agent_file(&inp.home, spec.agent),
        crate::setup::agy_agent_markdown(spec),
    ));
    let mut args: Vec<String> = inp.extra_args.clone();
    if let Some(sid) = &inp.resume_session {
        args.extend(["--conversation".into(), sid.clone()]);
    }
    args.extend([
        "--agent".into(),
        agent_name,
        "--add-dir".into(),
        spec.ctx.session_dir.to_string_lossy().into(),
        "--dangerously-skip-permissions".into(),
        "--model".into(),
        spec.route.model.clone(),
        "--effort".into(),
        effort_word(spec.effort, Effort::High).into(),
    ]);
    args.extend(artifacts_dir_args(spec));
    if let Some(prompt) = prompt_arg(inp, &mut files) {
        args.extend(["-i".into(), prompt]);
    }
    LaunchPlan {
        program: inp.command.clone(),
        args,
        env: base_env(inp),
        cwd: spec.ctx.repo_root.clone(),
        files,
        links: vec![],
        session_id: None,
        env_remove: vec![],
        tty_param: None,
    }
}

/// The hook command the global Antigravity integration registers, which reads the execution from
/// the environment.
pub fn agy_hook_command(ostra_binary: &Path, event: HookEvent) -> String {
    format!(
        "{} hook --harness agy --event {}",
        shell_quote(&ostra_binary.to_string_lossy()),
        event.as_str()
    )
}

/// Write `plan.files` (owner-only, because Grok's config holds the bridge token) and create
/// `plan.links`.
pub fn materialize(plan: &LaunchPlan) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    for (path, content) in &plan.files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(content.as_bytes())?;
    }
    for (link, target) in &plan.links {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::symlink_metadata(link).is_ok() {
            continue;
        }
        std::os::unix::fs::symlink(target, link)?;
    }
    Ok(())
}

/// Rule W1: the harness may read the workspace artifacts folder, outside its working directory.
fn artifacts_dir_args(spec: &ExecutionSpec) -> Vec<String> {
    let dir = ostra_core::artifacts::dir(&spec.ctx.workspace_root);
    if spec.ctx.workspace_root.as_os_str().is_empty() || !dir.is_dir() {
        return vec![];
    }
    vec!["--add-dir".into(), dir.to_string_lossy().into()]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ostra_core::agent::AgentName;
    use ostra_core::config::{PermissionMode, PermissionRules, ResolvedRoute};
    use ostra_core::exec::ExecContext;
    use ostra_core::{ExecutionId, ExecutorKind};

    pub(crate) fn spec(harness: HarnessKind, model: &str, root: &Path) -> ExecutionSpec {
        let session_dir = root.join("ws/.ostra/sessions/s_1/backend");
        ExecutionSpec {
            id: ExecutionId::from("x_test1"),
            agent: AgentName::Implementer,
            route: ResolvedRoute {
                executor: ExecutorKind::Harness(harness),
                model: model.into(),
                tier: None,
            },
            effort: Effort::High,
            system_prompt: "You are the implementer. Say \"hi\".".into(),
            first_message: "Repo root: /repo\nTask: do it".into(),
            capabilities: vec![
                Capability::Read,
                Capability::Edit,
                Capability::Shell,
                Capability::Report,
            ],
            submit_schema: json!({}),
            timeout_secs: 600,
            ctx: ExecContext {
                execution_id: ExecutionId::from("x_test1"),
                session_id: None,
                agent: AgentName::Implementer,
                initializer_mode: None,
                executor: ExecutorKind::Harness(harness),
                workspace_root: root.join("ws"),
                repo_root: root.join("repo"),
                project_key: "backend".into(),
                session_dir,
                session_root: root.join("ws/.ostra/sessions/s_1"),
                report_file: None,
                phase: None,
                yolo: false,
                permission_mode: PermissionMode::Default,
                permissions: PermissionRules::default(),
                protected_paths: vec![],
                memory_db: root.join("repo/.ostra/memory/knowledge.sqlite3"),
                sandbox_mode: None,
                sandbox_network: None,
                sandbox_allowed_hosts: vec![],
                sandbox_decoys: vec![],
                sandbox_loopback: Default::default(),
                sandbox_blocked_ports: vec![],
            },
            resume: None,
            harness_session_id: None,
        }
    }

    pub(crate) fn input<'a>(
        spec: &'a ExecutionSpec,
        harness: HarnessKind,
        root: &Path,
    ) -> LaunchInput<'a> {
        LaunchInput {
            spec,
            harness,
            command: harness.as_str().into(),
            extra_args: vec![],
            ostra_binary: PathBuf::from("/opt/ostra/bin/ostra"),
            server_url: "http://127.0.0.1:4100".into(),
            bridge_socket: None,
            token: "tok".into(),
            config_dir: root.join("cfg"),
            resume_session: None,
            home: root.join("home"),
            credential_env: vec![
                "ANTHROPIC_AUTH_TOKEN".into(),
                "OPENAI_API_KEY".into(),
                "MY_SERVICE_KEY".into(),
                "OSTRA_TOKEN".into(),
            ],
            sandboxed: false,
        }
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/usr/bin/ostra"), "/usr/bin/ostra");
        assert_eq!(shell_quote("/a b/o'k"), "'/a b/o'\\''k'");
    }

    #[test]
    fn claude_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let s = spec(HarnessKind::Claude, "haiku", tmp.path());
        let p = plan(&input(&s, HarnessKind::Claude, tmp.path()));
        assert_eq!(p.args.last().unwrap(), "Repo root: /repo\nTask: do it");
        let i = p.args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(p.args[i + 1], "Read,Edit,Bash,ToolSearch");
        assert!(p.session_id.is_some());
        let settings: serde_json::Value = serde_json::from_str(&p.files[0].1).unwrap();
        let cmd = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(
            cmd,
            "/opt/ostra/bin/ostra hook --execution x_test1 --harness claude --event pre_tool_use"
        );
        assert!(p.env.iter().any(|(k, v)| k == ENV_TOKEN && v == "tok"));
        // The prompt must follow a non-variadic flag so commander does not swallow it.
        assert_eq!(p.args[p.args.len() - 3], "--effort");
    }

    #[test]
    fn resumed_session_starts_with_the_note() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = spec(HarnessKind::Claude, "haiku", tmp.path());
        s.resume = Some(ostra_core::exec::ResumeInfo {
            from: ExecutionId::from("x_old"),
            native_session_id: Some("sid-1".into()),
            note: Some("Continue the workflow.".into()),
            inspect: false,
        });
        let mut inp = input(&s, HarnessKind::Claude, tmp.path());
        inp.resume_session = Some("sid-1".into());
        let p = plan(&inp);
        assert_eq!(p.args.last().unwrap(), "Continue the workflow.");
        let i = p.args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(p.args[i + 1], "sid-1");
    }

    #[test]
    fn inspection_reopens_the_session_with_no_prompt_and_no_tools() {
        let tmp = tempfile::tempdir().unwrap();
        for h in [
            HarnessKind::Claude,
            HarnessKind::Codex,
            HarnessKind::Grok,
            HarnessKind::Agy,
        ] {
            let mut s = spec(h, "m", tmp.path());
            s.resume = Some(ostra_core::exec::ResumeInfo {
                from: ExecutionId::from("x_old"),
                native_session_id: Some("sid-1".into()),
                note: None,
                inspect: true,
            });
            let mut inp = input(&s, h, tmp.path());
            inp.resume_session = Some("sid-1".into());
            let p = plan(&inp);
            assert!(p.args.iter().any(|a| a == "sid-1"), "{h:?} does not resume");
            assert!(
                !p.args
                    .iter()
                    .any(|a| a.contains("Repo root") || a.contains("resumed")),
                "{h:?} sends a prompt: {:?}",
                p.args
            );
            assert!(!p.args.contains(&"-i".to_string()), "{h:?}");
            if h == HarnessKind::Claude {
                let i = p.args.iter().position(|a| a == "--tools").unwrap();
                assert!(!p.args[i + 1].contains("ToolSearch"));
                assert!(!p.args.contains(&"--mcp-config".to_string()));
            }
        }
    }

    #[test]
    fn codex_plan_is_valid_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let s = spec(HarnessKind::Codex, "gpt-5.6-luna", tmp.path());
        let p = plan(&input(&s, HarnessKind::Codex, tmp.path()));
        let mut doc = String::new();
        for w in p.args.windows(2) {
            if w[0] == "-c" {
                let (k, v) = w[1].split_once('=').unwrap();
                doc.push_str(&format!("{k} = {v}\n"));
            }
        }
        let parsed: toml::Table = toml::from_str(&doc).expect("every -c value parses as TOML");
        assert_eq!(
            parsed["developer_instructions"].as_str(),
            Some("You are the implementer. Say \"hi\".")
        );
        assert!(
            p.args
                .contains(&"--dangerously-bypass-hook-trust".to_string())
        );
    }

    #[test]
    fn grok_plan_farm() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("home/.grok");
        std::fs::create_dir_all(real.join("sessions")).unwrap();
        std::fs::write(real.join("auth.json"), "{}").unwrap();
        std::fs::write(real.join("leader.sock"), "").unwrap();
        std::fs::write(
            real.join("config.toml"),
            "[ui]\nyolo = false\n\n[mcp_servers.ostra]\ncommand = \"old\"\n",
        )
        .unwrap();
        let s = spec(HarnessKind::Grok, "grok-4.5", tmp.path());
        let _guard = EnvGuard::unset("GROK_HOME");
        let p = plan(&input(&s, HarnessKind::Grok, tmp.path()));
        let linked: Vec<String> = p
            .links
            .iter()
            .map(|(l, _)| l.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(linked.contains(&"auth.json".into()) && linked.contains(&"sessions".into()));
        assert!(!linked.contains(&"leader.sock".into()));
        let config = &p
            .files
            .iter()
            .find(|(f, _)| f.ends_with("config.toml"))
            .unwrap()
            .1;
        let parsed: toml::Table = toml::from_str(config).expect("valid TOML");
        assert_eq!(
            parsed["mcp_servers"]["ostra"]["args"][2].as_str(),
            Some("x_test1")
        );
        assert_eq!(
            parsed["hooks"]["PreToolUse"][0]["matcher"].as_str(),
            Some("*")
        );
        assert!(parsed["ui"]["yolo"].as_bool() == Some(false));
        materialize(&p).unwrap();
        assert!(tmp.path().join("cfg/grok-home/auth.json").exists());
    }

    #[test]
    fn claude_loads_only_user_settings_and_keeps_its_sign_in_env() {
        let tmp = tempfile::tempdir().unwrap();
        let s = spec(HarnessKind::Claude, "haiku", tmp.path());
        let p = plan(&input(&s, HarnessKind::Claude, tmp.path()));
        let i = p
            .args
            .iter()
            .position(|a| a == "--setting-sources")
            .unwrap();
        assert_eq!(p.args[i + 1], "user");
        let settings: serde_json::Value = serde_json::from_str(&p.files[0].1).unwrap();
        assert_eq!(settings["disableAllHooks"], false);
        assert!(!p.env_remove.contains(&"ANTHROPIC_AUTH_TOKEN".to_string()));
        for k in ["OPENAI_API_KEY", "MY_SERVICE_KEY", "OSTRA_TOKEN"] {
            assert!(p.env_remove.contains(&k.to_string()), "{k}");
        }
        assert!(p.env.iter().any(|(k, _)| k == "GIT_CONFIG_COUNT"));
        assert!(p.env.iter().any(|(k, v)| k == ENV_TOKEN && v == "tok"));
    }

    #[test]
    fn codex_opens_the_repository_restricted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("repo")).unwrap();
        let s = spec(HarnessKind::Codex, "gpt-5.6-luna", tmp.path());
        let p = plan(&input(&s, HarnessKind::Codex, tmp.path()));
        let i = p.args.iter().position(|a| a == "-p").unwrap();
        let name = &p.args[i + 1];
        assert!(name.starts_with("ostra-restricted-"), "{name}");
        let (path, body) = p
            .files
            .iter()
            .find(|(f, _)| f.ends_with(format!("{name}.config.toml")))
            .unwrap();
        assert_eq!(
            path.parent().unwrap(),
            codex_home_real(&tmp.path().join("home"))
        );
        let parsed: toml::Table = toml::from_str(body).unwrap();
        let repo = tmp.path().join("repo");
        assert_eq!(
            parsed["projects"][&*repo.to_string_lossy()]["trust_level"].as_str(),
            Some("untrusted")
        );
        assert!(!p.args.iter().any(|a| a.contains("trust_level")));
        assert!(!p.env_remove.contains(&"OPENAI_API_KEY".to_string()));
        assert!(p.env_remove.contains(&"ANTHROPIC_AUTH_TOKEN".to_string()));

        let mut inp = input(&s, HarnessKind::Codex, tmp.path());
        inp.extra_args = vec!["--profile".into(), "mine".into()];
        let p = plan(&inp);
        assert!(!p.args.contains(&"-p".to_string()));
    }

    #[test]
    fn codex_marks_every_folder_up_to_the_git_root() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("mono/services/api");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(tmp.path().join("mono/.git")).unwrap();
        let dirs = codex_project_dirs(&sub);
        assert_eq!(dirs.first().unwrap(), &sub);
        assert_eq!(dirs.last().unwrap(), &tmp.path().join("mono"));
        assert_eq!(dirs.len(), 3);
    }

    #[test]
    fn grok_does_not_trust_the_repository_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("home/.grok")).unwrap();
        let s = spec(HarnessKind::Grok, "grok-4.5", tmp.path());
        let _guard = EnvGuard::unset("GROK_HOME");
        let p = plan(&input(&s, HarnessKind::Grok, tmp.path()));
        let trust = &p
            .files
            .iter()
            .find(|(f, _)| f.ends_with("trusted_folders.toml"))
            .unwrap()
            .1;
        assert!(
            !trust.contains(&*s.ctx.repo_root.to_string_lossy()),
            "{trust}"
        );
        assert!(trust.contains(&*s.ctx.session_dir.to_string_lossy()));
        materialize(&p).unwrap();
        let config = tmp.path().join("cfg/grok-home/config.toml");
        let mode = std::fs::metadata(config).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    pub(crate) struct EnvGuard(&'static str, Option<std::ffi::OsString>);
    impl EnvGuard {
        pub(crate) fn unset(k: &'static str) -> Self {
            let old = std::env::var_os(k);
            // SAFETY: tests that touch this variable do not run concurrently with readers of it.
            unsafe { std::env::remove_var(k) };
            EnvGuard(k, old)
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(v) = &self.1 {
                // SAFETY: see `unset`.
                unsafe { std::env::set_var(self.0, v) };
            }
        }
    }
}
