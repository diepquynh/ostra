//! Sandbox profiles for agent commands. The policy layer judges each tool call by the paths it
//! names, which a shell can hide; the sandbox is what the kernel enforces underneath it: the host
//! is read-only, only the execution's own roots are writable, and Ostra's data, the user's
//! credentials, and the session bus or keychain are not reachable at all.
//!
//! One [`Profile`] renders for two backends: bubblewrap mounts on Linux, and a Seatbelt (SBPL)
//! policy for `/usr/bin/sandbox-exec` on macOS. Seatbelt has no mount namespace, so on macOS a
//! hidden path returns EPERM instead of looking empty, `/tmp` is denied instead of private
//! (`TMPDIR` points at the scratch dir), and a harness gets no disposable home.

use crate::config::{SandboxConfig, SandboxMode};
use crate::exec::ExecContext;
use crate::paths;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Files that run code in the user's later shells, logins, or builds. They stay read-only even
/// where their folder is writable, so an agent cannot leave a program behind for the user. A
/// trailing `/` marks a dir.
const PERSISTENCE_IN_HOME: &[&str] = &[
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".bash_logout",
    ".profile",
    ".zshrc",
    ".zshenv",
    ".zprofile",
    ".zlogin",
    ".config/fish/",
    ".gitconfig",
    ".config/git/",
    ".config/autostart/",
    ".config/systemd/",
    ".config/environment.d/",
    ".pam_environment",
    ".xprofile",
    ".xsessionrc",
    ".npmrc",
    ".local/bin/",
    ".cargo/bin/",
    ".cargo/config",
    ".cargo/config.toml",
    "Library/LaunchAgents/",
    "Library/Application Support/com.apple.backgroundtaskmanagementagent/",
    "Library/Preferences/",
    "Library/Scripts/",
    "Library/Services/",
    "Library/Application Support/iTerm2/Scripts/",
];

/// Paths inside a `.git` dir that choose programs git runs. A trailing `/` marks a dir.
const GIT_PROTECTED: &[&str] = &["config", "hooks/", "info/"];

/// Variables that point a command at the desktop session, an agent, or a daemon socket.
const UNSET_VARS: &[&str] = &[
    "SSH_AUTH_SOCK",
    "DBUS_SESSION_BUS_ADDRESS",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "DOCKER_HOST",
    "GPG_AGENT_INFO",
];

/// Container engine sockets. Whoever reaches one can start a container that mounts the host, so
/// they are hidden even though the host is read-only (a read-only file does not stop `connect`).
const DOCKER_SOCKETS: &[&str] = &[
    "/run/docker.sock",
    "/var/run/docker.sock",
    "/run/podman/podman.sock",
    "~/.orbstack/run/docker.sock",
    "~/.colima/default/docker.sock",
    "~/.rd/docker.sock",
    "~/.local/share/containers/podman/machine",
];

/// The sandbox's own tool caches, one set per workspace under `~/.cache/ostra/sandbox/` (or
/// `$OSTRA_SANDBOX_CACHE`). The
/// user's `~/.cargo`, `~/.npm`, and `~/.cache` stay read-only, because builds on the host run
/// what those caches hold (extracted crate sources, npm and pnpm stores, Go build output). macOS
/// tools ignore `XDG_CACHE_HOME` and write under `~/Library/Caches`, which stays read-only for the
/// same reason, so they get their own variables.
const CACHE_ENV: &[(&str, &str)] = &[
    ("CARGO_HOME", "cargo"),
    ("npm_config_cache", "npm"),
    ("npm_config_store_dir", "pnpm-store"),
    ("XDG_CACHE_HOME", "xdg"),
    ("GOMODCACHE", "go-mod"),
    ("GOCACHE", "go-build"),
    ("PIP_CACHE_DIR", "pip"),
    ("YARN_CACHE_FOLDER", "yarn"),
    ("UV_CACHE_DIR", "uv"),
    ("CLANG_MODULE_CACHE_PATH", "clang-modules"),
];

/// The user's cargo settings, linked into the sandbox's `CARGO_HOME` so builds keep the same
/// linker and registry settings. Credentials are not linked.
const CARGO_SETTINGS: &[&str] = &["config.toml", "config"];

/// Seatbelt's front end. Called by absolute path and checked to be root-owned, because the
/// Homebrew prefix on `PATH` is writable by the user.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// What enforces the profile on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// The `bwrap` binary.
    Bubblewrap(PathBuf),
    /// `/usr/bin/sandbox-exec`.
    Seatbelt,
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Bubblewrap(_) => "bubblewrap",
            Backend::Seatbelt => "seatbelt",
        }
    }
}

/// The sandbox backend when one works on this machine. Probed once per process.
pub fn backend() -> Option<&'static Backend> {
    probed().as_ref().ok()
}

/// Why [`backend`] is `None`.
pub fn unavailable_reason() -> Option<&'static str> {
    probed().as_ref().err().map(String::as_str)
}

fn probed() -> &'static Result<Backend, String> {
    static PROBE: OnceLock<Result<Backend, String>> = OnceLock::new();
    PROBE.get_or_init(probe)
}

fn probe() -> Result<Backend, String> {
    if cfg!(target_os = "linux") {
        probe_bwrap().map(Backend::Bubblewrap)
    } else if cfg!(target_os = "macos") {
        probe_seatbelt().map(|()| Backend::Seatbelt)
    } else {
        Err("The sandbox needs Linux or macOS.".into())
    }
}

fn probe_bwrap() -> Result<PathBuf, String> {
    let bin = std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join("bwrap"))
                .find(|p| p.is_file())
        })
        .ok_or("The `bwrap` command is not installed.")?;
    let out = std::process::Command::new(&bin)
        .args([
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--unshare-pid",
            "true",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("`{}` does not start: {e}.", bin.display()))?;
    if out.status.success() {
        return Ok(bin);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let err = err.trim();
    Err(format!(
        "`{}` is installed but cannot create a sandbox here ({}). In a container, the default seccomp or AppArmor profile blocks user namespaces: run the container with a profile that allows them. On the host, allow unprivileged user namespaces (for example `kernel.apparmor_restrict_unprivileged_userns=0`).",
        bin.display(),
        if err.is_empty() {
            "no error output"
        } else {
            err
        }
    ))
}

fn probe_seatbelt() -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let meta =
        std::fs::metadata(SANDBOX_EXEC).map_err(|_| format!("`{SANDBOX_EXEC}` is missing."))?;
    if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
        return Err(format!(
            "`{SANDBOX_EXEC}` is not owned by root or is writable by others, so Ostra does not trust it."
        ));
    }
    let out = std::process::Command::new(SANDBOX_EXEC)
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("`{SANDBOX_EXEC}` does not start: {e}."))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let err = err.trim();
    if err.contains("sandbox_apply: Operation not permitted") {
        return Err(format!(
            "Ostra runs inside another sandbox, and macOS cannot start a sandbox inside one ({err}). Start Ostra outside it, for example not from a sandboxed terminal or agent."
        ));
    }
    Err(format!(
        "`{SANDBOX_EXEC}` cannot create a sandbox here ({}).",
        if err.is_empty() {
            "no error output"
        } else {
            err
        }
    ))
}

/// What an execution does about the sandbox under this config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Sandboxed(Backend),
    /// Unsandboxed, with the reason to warn about when the config asked for a sandbox.
    Unsandboxed(Option<String>),
}

#[cfg(target_os = "macos")]
pub const UNAVAILABLE: &str = "Run Ostra outside any other sandbox, because macOS cannot nest them, and agent commands otherwise run with the full rights of your user. Choose sandbox mode off in the workspace settings, or set `[sandbox] mode = \"off\"` in config.toml, to run without it on purpose.";
#[cfg(not(target_os = "macos"))]
pub const UNAVAILABLE: &str = "Install bubblewrap (the `bwrap` command) on the Linux machine that runs Ostra, or allow unprivileged user namespaces, because agent commands otherwise run with the full rights of your user. Choose sandbox mode off in the workspace settings, or set `[sandbox] mode = \"off\"` in config.toml, to run without it on purpose.";

/// What the sandbox on this machine cannot enforce, for the setup check.
pub fn known_gaps(backend: &Backend) -> Option<&'static str> {
    match backend {
        Backend::Bubblewrap(_) => None,
        Backend::Seatbelt => Some(
            "Keep tokens out of the environment of long-running programs, such as exports in your shell profile, and keep them in the keychain or a file the sandbox hides, because on macOS agent commands can read the arguments and startup environment of every program running as your user, and Seatbelt cannot block that.",
        ),
    }
}

/// [`UNAVAILABLE`] with the probe's reason in front.
pub fn unavailable_message() -> String {
    match unavailable_reason() {
        Some(r) => format!("{r} {UNAVAILABLE}"),
        None => UNAVAILABLE.to_string(),
    }
}

/// `Err` when the config requires a sandbox this machine cannot provide.
pub fn decide(cfg: &SandboxConfig) -> Result<Decision, String> {
    match (cfg.mode, backend()) {
        (SandboxMode::Off, _) => Ok(Decision::Unsandboxed(None)),
        (_, Some(b)) => Ok(Decision::Sandboxed(b.clone())),
        (SandboxMode::Auto, None) => Ok(Decision::Unsandboxed(Some(unavailable_message()))),
        (SandboxMode::Required, None) => Err(format!(
            "The sandbox is required by the workspace settings or by `[sandbox] mode` in config.toml, but it is not available. {}",
            unavailable_message()
        )),
    }
}

/// How Ostra starts a program for a project or workspace: under the sandbox when the config and
/// this machine give one, without the credential variables Ostra holds, and with git settings
/// that keep a planted `.git/config` from starting programs.
#[derive(Debug)]
pub struct HostCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env_remove: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Kills what the program leaves running once dropped. Hold it as long as the program runs.
    pub members: Option<Members>,
}

/// [`HostCommand`] for `program args` started in `cwd`, writing only under `roots`. The global
/// config is read fresh, and `mode` is the workspace's own sandbox mode, when it sets one. `Err`
/// when the mode is `required` and no sandbox is available.
pub fn host_command(
    program: &str,
    args: &[String],
    cwd: &Path,
    roots: &[&Path],
    mode: Option<SandboxMode>,
) -> Result<HostCommand, String> {
    let mut global: crate::config::GlobalConfig =
        crate::config::load_toml(&paths::global_config_path()).unwrap_or_default();
    global.sandbox = global.sandbox.for_workspace(mode);
    let mut env_remove: Vec<String> = crate::config::credential_env_names(&global, &[])
        .into_iter()
        .collect();
    env_remove.extend(
        std::env::vars_os()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .filter(|k| k.starts_with("OSTRA_")),
    );
    let mut env = crate::git::agent_env();
    let backend = match decide(&global.sandbox)? {
        Decision::Unsandboxed(_) => {
            return Ok(HostCommand {
                program: program.to_string(),
                args: args.to_vec(),
                env_remove,
                env,
                members: None,
            });
        }
        Decision::Sandboxed(b) => b,
    };
    let found = find_program(program, cwd).ok_or_else(|| {
        format!("Cannot start `{program}`: no such program in the folder or on PATH.")
    })?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/".into());
    let sc = Profile::for_program(roots, &global.sandbox, &home).command(
        &backend,
        cwd,
        found.as_os_str(),
        args.iter().map(OsString::from),
    )?;
    let text = |s: &OsStr| s.to_string_lossy().into_owned();
    env_remove.extend(sc.env_remove);
    env.extend(sc.env);
    Ok(HostCommand {
        program: text(sc.program.as_os_str()),
        args: sc.args.iter().map(|a| text(a)).collect(),
        env_remove,
        env,
        members: sc.members,
    })
}

/// `program` as the host would start it from `cwd`: a path with a `/`, else the first match on
/// `PATH`. Checked before the sandbox starts, so a missing program reads as missing.
fn find_program(program: &str, cwd: &Path) -> Option<PathBuf> {
    if program.contains('/') {
        let p = cwd.join(program);
        return p.is_file().then_some(p);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// A program and its arguments wrapped in the sandbox, with the environment and working dir the
/// caller must set on its `Command`.
#[derive(Debug)]
pub struct SandboxedCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub cwd: PathBuf,
    /// Kills what the program leaves running once dropped, on backends where a child can outlive
    /// the sandboxed program. Hold it as long as the program runs.
    pub members: Option<Members>,
}

impl SandboxedCommand {
    /// A `std` command with the program, arguments, environment, and working dir set.
    pub fn std_command(&self) -> std::process::Command {
        let mut c = std::process::Command::new(&self.program);
        c.args(&self.args).current_dir(&self.cwd);
        for k in &self.env_remove {
            c.env_remove(k);
        }
        c.envs(self.env.iter().map(|(k, v)| (k, v)));
        c
    }
}

/// The processes of one Seatbelt invocation. Seatbelt has no pid namespace and nothing like
/// `--die-with-parent`, so a child that calls `setsid` or double-forks outlives its shell. Each
/// invocation's policy allows looking up two mach service names that no daemon registers: its
/// own, and one shared by every sandbox of this data dir. The kernel answers for any process
/// whether its policy allows a name (`sandbox_check`), and a policy is inherited and cannot be
/// dropped, so the name finds every descendant however it detached. Some system sandboxes allow
/// every name, so a member must also be refused [`CANARY`], which no Ostra policy allows.
#[derive(Debug)]
pub struct Members {
    own: String,
    group: String,
}

impl Members {
    fn new() -> Members {
        Members {
            own: format!("dev.ostra.sandbox.{}", uuid::Uuid::new_v4().simple()),
            group: group_marker(),
        }
    }

    /// Kills every process still running in this invocation's sandbox.
    pub fn kill(&self) {
        kill_marked(&self.own);
    }
}

impl Drop for Members {
    fn drop(&mut self) {
        self.kill();
    }
}

/// A mach name every Ostra policy refuses, through `(deny default)`.
#[cfg(target_os = "macos")]
const CANARY: &str = "dev.ostra.sandbox.canary";

/// The marker every sandbox of this data dir carries, so a restarted server finds the processes
/// an earlier one left behind.
fn group_marker() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    paths::data_dir().hash(&mut h);
    format!("dev.ostra.sandbox.d{:016x}", h.finish())
}

/// Kills processes that sandboxes of this data dir left running. Call once at server start,
/// before any execution starts.
pub fn kill_leftovers() -> usize {
    kill_marked(&group_marker())
}

/// SIGKILLs every process of this user in a sandbox marked `marker`, until none is left. Returns
/// how many it killed.
fn kill_marked(marker: &str) -> usize {
    let mut total = 0;
    for _ in 0..20 {
        let found = marked(marker);
        if found.is_empty() {
            break;
        }
        for pid in &found {
            // SAFETY: kill(2) on a pid the kernel just reported as running in the marked sandbox.
            if unsafe { libc::kill(*pid, libc::SIGKILL) } == 0 {
                total += 1;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    total
}

#[cfg(target_os = "macos")]
fn marked(marker: &str) -> Vec<libc::pid_t> {
    use std::ffi::{CString, c_char, c_int};
    unsafe extern "C" {
        fn sandbox_check(pid: libc::pid_t, operation: *const c_char, kind: c_int, ...) -> c_int;
    }
    const FILTER_NONE: c_int = 0;
    const FILTER_GLOBAL_NAME: c_int = 2;
    const NO_REPORT: c_int = 0x4000_0000;
    let (Ok(name), Ok(canary)) = (CString::new(marker), CString::new(CANARY)) else {
        return vec![];
    };
    let op = c"mach-lookup";
    let mut pids: Vec<libc::pid_t> = vec![0; 8192];
    // SAFETY: the buffer holds `pids.len()` pids and its size in bytes is passed.
    let n = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<libc::pid_t>()) as c_int,
        )
    };
    pids.truncate(n.max(0) as usize);
    let me = std::process::id() as libc::pid_t;
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let owned = |pid: libc::pid_t| {
        let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as c_int;
        // SAFETY: `info` is sized for PROC_PIDT_SHORTBSDINFO, and its size is passed.
        unsafe {
            let mut info: libc::proc_bsdshortinfo = std::mem::zeroed();
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDT_SHORTBSDINFO,
                0,
                (&raw mut info).cast(),
                size,
            ) == size
                && info.pbsi_uid == uid
        }
    };
    // SAFETY: sandbox_check reads the pid's policy; the strings outlive the calls.
    let boxed = |pid: libc::pid_t| unsafe {
        sandbox_check(pid, std::ptr::null(), FILTER_NONE | NO_REPORT) == 1
    };
    // SAFETY: as above.
    let allows = |pid: libc::pid_t, n: &CString| unsafe {
        sandbox_check(pid, op.as_ptr(), FILTER_GLOBAL_NAME | NO_REPORT, n.as_ptr()) == 0
    };
    pids.into_iter()
        .filter(|&pid| pid > 1 && pid != me && owned(pid) && boxed(pid))
        .filter(|&pid| allows(pid, &name) && !allows(pid, &canary))
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn marked(_marker: &str) -> Vec<libc::pid_t> {
    vec![]
}

/// Moves this process's environment off the stack and zeroes the original strings. On macOS any
/// program of the same user, a sandboxed one included, reads another process's startup
/// environment with `sysctl(KERN_PROCARGS2)`, which reads those stack strings. Call first thing
/// in `main`, before any thread starts.
pub fn scrub_startup_env() {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: single-threaded at this point, as documented. The pointers come from
        // `environ` before it changes; each string is NUL-terminated and was written by the
        // kernel at exec, so zeroing its bytes in place is sound once nothing points at it.
        unsafe {
            let envp = *libc::_NSGetEnviron();
            let mut originals: Vec<(*mut libc::c_char, usize)> = vec![];
            let mut i = 0;
            while !envp.is_null() && !(*envp.add(i)).is_null() {
                let p = *envp.add(i);
                originals.push((p, libc::strlen(p)));
                i += 1;
            }
            let vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
            for (k, _) in &vars {
                std::env::remove_var(k);
            }
            for (k, v) in &vars {
                std::env::set_var(k, v);
            }
            for (p, len) in originals {
                std::ptr::write_bytes(p, 0, len);
            }
        }
    }
}

/// True the first time it is called, so the missing-sandbox warning reaches the log once.
pub fn first_warning() -> bool {
    static WARNED: AtomicBool = AtomicBool::new(false);
    !WARNED.swap(true, Ordering::Relaxed)
}

fn expand(home: &Path, p: &str) -> PathBuf {
    match p.trim().strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p.trim()),
    }
}

/// Existing paths only, symlinks resolved, because bubblewrap mounts at real paths and Seatbelt
/// matches them.
fn real(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
}

/// The real path of `p`'s longest existing ancestor with the missing rest appended, for paths
/// that may not exist yet.
fn real_or_missing(p: &Path) -> Option<PathBuf> {
    let mut rest = vec![];
    let mut cur = p;
    loop {
        if let Some(r) = real(cur) {
            return Some(rest.iter().rev().fold(r, |acc: PathBuf, n| acc.join(n)));
        }
        rest.push(cur.file_name()?.to_os_string());
        cur = cur.parent()?;
    }
}

/// `(path, is_dir)` for a list entry where a trailing `/` marks a dir.
fn entry(base: &Path, rel: &str) -> (PathBuf, bool) {
    match rel.strip_suffix('/') {
        Some(dir) => (base.join(dir), true),
        None => (base.join(rel), false),
    }
}

/// A fresh owner-only scratch dir under the OS temp dir, for a native execution's `/tmp`.
pub fn new_scratch() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("ostra-scratch-{}", uuid::Uuid::new_v4().simple()));
    make_private_dir(&dir)?;
    Ok(dir)
}

fn make_private_dir_all(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(dir)
}

fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mount {
    /// The overlaid home folder, as a tmpfs.
    HomeTmpfs(PathBuf),
    /// A top-level entry of the overlaid home, bound back read-only or recreated as a symlink.
    HomeEntry(PathBuf, Option<PathBuf>),
    Writable {
        src: PathBuf,
        dest: PathBuf,
    },
    /// `revealed` binds it even inside a hidden dir. `dir` says what a placeholder for a missing
    /// path is.
    ReadOnly {
        path: PathBuf,
        dir: bool,
        revealed: bool,
    },
    Hidden(PathBuf),
    Link {
        link: PathBuf,
        target: PathBuf,
    },
}

impl Mount {
    fn dest(&self) -> &Path {
        match self {
            Mount::HomeTmpfs(p) | Mount::HomeEntry(p, _) | Mount::Hidden(p) => p,
            Mount::Writable { dest, .. } => dest,
            Mount::ReadOnly { path, .. } => path,
            Mount::Link { link, .. } => link,
        }
    }

    /// Which mount stays when two share a path: the more restrictive one.
    fn rank(&self) -> u8 {
        match self {
            Mount::HomeTmpfs(_) => 0,
            Mount::HomeEntry(..) => 1,
            Mount::Writable { .. } => 2,
            Mount::ReadOnly { .. } => 3,
            Mount::Hidden(_) => 4,
            Mount::Link { .. } => 5,
        }
    }
}

/// The mounts for one execution's agent commands.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    mounts: Vec<Mount>,
    /// A home folder replaced by a tmpfs with its entries bound back read-only, so new files a
    /// harness writes at its top level vanish with the execution.
    home_overlay: Option<PathBuf>,
    /// The host dir mounted at `/tmp`. `None` gives `/tmp` an empty tmpfs.
    scratch: Option<PathBuf>,
    env: Vec<(String, String)>,
    network: bool,
    new_session: bool,
    /// The per-workspace cache dir, whose `tmp` is `TMPDIR` under Seatbelt when no scratch dir
    /// is set.
    cache_root: Option<PathBuf>,
    /// Seatbelt lets the program use the PTY named by the `TTY` parameter as its terminal.
    tty: bool,
    /// Single files made writable, rendered after every other rule.
    writable_files: Vec<PathBuf>,
}

impl Profile {
    /// Writable: the workspace and repo roots, the execution's own session dirs, each repo's
    /// `.git` (bound onto itself, so it cannot be renamed away), and the sandbox's tool caches.
    /// Read-only inside those: Ostra's state files, what in each `.git` chooses programs, and the
    /// home files that run code later. Hidden: Ostra's data and config dirs, other sessions, the
    /// session bus dir, and credential stores other than the running harness's own. `/tmp` is
    /// private.
    pub fn for_execution(ctx: &ExecContext, cfg: &SandboxConfig, home: &Path) -> Profile {
        let mut p = Profile {
            network: cfg.network,
            ..Profile::base()
        };
        for root in [
            &ctx.workspace_root,
            &ctx.repo_root,
            &ctx.session_root,
            &ctx.session_dir,
        ] {
            if !root.as_os_str().is_empty() {
                p = p.writable(root);
            }
        }
        let cache_key = if ctx.workspace_root.as_os_str().is_empty() {
            &ctx.repo_root
        } else {
            &ctx.workspace_root
        };
        let own = match ctx.executor {
            crate::ExecutorKind::Harness(h) => Some(h),
            crate::ExecutorKind::Native => None,
        };
        p = p.common(cfg, home, own, cache_key);
        if !ctx.session_root.as_os_str().is_empty() {
            // Other executions' harness dirs in this session hold their bridge tokens.
            let harness = paths::session_state_dir(&ctx.session_root).join("harness");
            let _ = std::fs::create_dir_all(&harness);
            p = p
                .hidden(&harness)
                .read_only_dir(&paths::session_state_dir(&ctx.session_root));
        }
        if !ctx.workspace_root.as_os_str().is_empty() {
            // Rule W1: agents read workspace artifacts and never write them.
            p = p
                .workspace_state(&ctx.workspace_root)
                .read_only_dir(&crate::artifacts::dir(&ctx.workspace_root));
        }
        for f in db_files(&ctx.memory_db) {
            p = p.read_only(&f);
        }
        for prot in &ctx.protected_paths {
            p = p.read_only(prot);
        }
        p.protect_git(&[&ctx.repo_root, &ctx.workspace_root])
    }

    /// For a program Ostra starts for a project or workspace (a format command, a language
    /// server, a code provider, a stdio MCP server): the same rules as an agent command, with
    /// `roots` writable. The network stays on, because these programs fetch dependencies and
    /// call their own services.
    pub fn for_program(roots: &[&Path], cfg: &SandboxConfig, home: &Path) -> Profile {
        let mut p = Profile {
            network: true,
            ..Profile::base()
        };
        for r in roots {
            p = p.writable(r);
        }
        let key = roots.first().copied().unwrap_or(Path::new("/"));
        p = p.common(cfg, home, None, key);
        for r in roots {
            p = p.workspace_state(r);
        }
        p.protect_git(roots)
    }

    fn base() -> Profile {
        Profile {
            new_session: true,
            env: vec![("TMPDIR".into(), "/tmp".into())],
            ..Default::default()
        }
    }

    /// What every profile hides or keeps read-only, whoever runs in it. `own` is the harness
    /// whose sign-in file stays visible.
    fn common(
        mut self,
        cfg: &SandboxConfig,
        home: &Path,
        own: Option<crate::HarnessKind>,
        cache_key: &Path,
    ) -> Profile {
        for w in &cfg.extra_writable {
            self = self.writable(&expand(home, w));
        }
        self.add_caches(cache_key, home);
        let mut hidden: Vec<PathBuf> = vec![paths::data_dir()];
        // Ostra's own config dir, or only the file when `OSTRA_CONFIG` places it in a shared dir.
        hidden.push(match std::env::var_os("OSTRA_CONFIG") {
            Some(_) => paths::global_config_path(),
            None => paths::global_config_path()
                .parent()
                .map_or_else(paths::global_config_path, Path::to_path_buf),
        });
        hidden.extend(
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .or_else(|| user_id().map(|uid| PathBuf::from(format!("/run/user/{uid}")))),
        );
        hidden.extend(std::env::var_os("OSTRA_MASTER_KEY_FILE").map(PathBuf::from));
        hidden.extend(std::env::var_os("XAUTHORITY").map(PathBuf::from));
        hidden.extend(
            paths::HOME_CREDENTIALS
                .iter()
                .filter(|c| own.is_none() || c.harness != own)
                .map(|c| home.join(c.path)),
        );
        hidden.extend(cfg.extra_hidden.iter().map(|h| expand(home, h)));
        hidden.extend(DOCKER_SOCKETS.iter().map(|s| expand(home, s)));
        for h in hidden {
            self = self.hidden(&h);
        }
        // Agents read skills and references from the assets dir inside the hidden data dir.
        if let Some(path) = real(&paths::data_dir().join("assets")) {
            self.mounts.push(Mount::ReadOnly {
                path,
                dir: true,
                revealed: true,
            });
        }
        for rel in PERSISTENCE_IN_HOME {
            self = self.protect(home, rel);
        }
        self
    }

    /// A workspace's settings and database read-only, and its sessions hidden. A session the
    /// profile made writable is bound again below the hidden dir.
    fn workspace_state(mut self, ws: &Path) -> Profile {
        let sessions = paths::sessions_root(ws);
        if sessions.is_dir() {
            self = self.hidden(&sessions);
        }
        if paths::workspace_toml(ws).is_file() {
            self = self.read_only(&paths::workspace_toml(ws));
            for f in db_files(&paths::workspace_db(ws)) {
                self = self.read_only(&f);
            }
        }
        self
    }

    /// Each repo's `.git` bound onto itself, so it cannot be renamed away, with what chooses
    /// programs inside it read-only.
    fn protect_git(mut self, roots: &[&Path]) -> Profile {
        for repo in git_repos(roots) {
            let git = repo.join(".git");
            self = self.writable(&git);
            for dir in git_dirs(&git) {
                for rel in GIT_PROTECTED {
                    self = self.protect(&dir, rel);
                }
            }
        }
        self
    }

    /// Per-workspace tool caches the sandbox owns, set through their variables.
    fn add_caches(&mut self, key_path: &Path, home: &Path) {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        key_path.hash(&mut h);
        let root = std::env::var_os("OSTRA_SANDBOX_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache/ostra/sandbox"))
            .join(format!("{:016x}", h.finish()));
        let _ = make_private_dir_all(&root.join("tmp"));
        self.cache_root = real(&root);
        for (var, sub) in CACHE_ENV {
            let dir = root.join(sub);
            if std::fs::create_dir_all(&dir).is_ok() {
                self.env
                    .push((var.to_string(), dir.to_string_lossy().into_owned()));
            }
        }
        let user_cargo = home.join(".cargo");
        for f in CARGO_SETTINGS {
            let (from, to) = (user_cargo.join(f), root.join("cargo").join(f));
            if from.is_file() && std::fs::symlink_metadata(&to).is_err() {
                let _ = std::os::unix::fs::symlink(&from, &to);
            }
        }
        *self = std::mem::take(self).writable(&root);
    }

    /// Adds a writable path, for a harness CLI's own state dirs.
    pub fn writable(mut self, path: &Path) -> Self {
        if let Some(p) = real(path) {
            self.mounts.push(Mount::Writable {
                src: p.clone(),
                dest: p,
            });
        }
        self
    }

    /// Makes one existing file writable, such as the Bash tool's pwd file in the host temp dir.
    pub fn writable_file(mut self, path: &Path) -> Self {
        if let Some(p) = real(path) {
            self.writable_files.push(p);
        }
        self
    }

    /// Keeps a file read-only, for a harness CLI's settings inside its writable dir. A missing
    /// file under a writable dir gets an empty read-only placeholder, so it cannot be created.
    pub fn read_only(self, path: &Path) -> Self {
        self.read_only_as(path, false)
    }

    /// [`Profile::read_only`] for a dir: a missing one gets an empty read-only dir.
    pub fn read_only_dir(self, path: &Path) -> Self {
        self.read_only_as(path, true)
    }

    fn read_only_as(mut self, path: &Path, dir: bool) -> Self {
        if let Some(path) = real_or_missing(path) {
            self.mounts.push(Mount::ReadOnly {
                path,
                dir,
                revealed: false,
            });
        }
        self
    }

    /// Read-only `base/rel`, where a trailing `/` on `rel` marks a dir.
    pub fn protect(self, base: &Path, rel: &str) -> Self {
        let (path, dir) = entry(base, rel);
        self.read_only_as(&path, dir)
    }

    fn hidden(mut self, path: &Path) -> Self {
        if let Some(p) = real(path) {
            self.mounts.push(Mount::Hidden(p));
        }
        self
    }

    /// Mounts `dir` at `/tmp` (and at its own path, so both names reach the same files). Under
    /// Seatbelt it is writable at its own path only, and `TMPDIR` names it.
    pub fn scratch(mut self, dir: &Path) -> std::io::Result<Self> {
        make_private_dir(dir)?;
        self.scratch = real(dir);
        Ok(self)
    }

    /// The host dir behind the sandbox's `/tmp`.
    pub fn scratch_dir(&self) -> Option<&Path> {
        self.scratch.as_deref()
    }

    /// A path as bubblewrap names it, translated to the host path of the same file. Paths under
    /// `/tmp` that another mount binds at their real location stay as they are. Seatbelt remaps
    /// nothing, so its callers skip this.
    pub fn to_host(&self, inside: &Path) -> PathBuf {
        let (Some(s), Ok(rest)) = (&self.scratch, inside.strip_prefix("/tmp")) else {
            return inside.to_path_buf();
        };
        let covered = inside.starts_with(s)
            || self.mounts.iter().any(|m| {
                let d = m.dest();
                d.starts_with("/tmp") && d != Path::new("/tmp") && inside.starts_with(d)
            });
        if covered {
            inside.to_path_buf()
        } else {
            s.join(rest)
        }
    }

    /// Replaces `home` with a tmpfs holding read-only binds of its entries. Bubblewrap only.
    pub fn home_overlay(mut self, home: &Path) -> Self {
        self.home_overlay = real(home);
        self
    }

    /// A symlink at `link` (inside the overlaid home) pointing at `target`. Bubblewrap only.
    pub fn link(mut self, link: &Path, target: &Path) -> Self {
        let link = match (link.parent().and_then(real), link.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => link.to_path_buf(),
        };
        self.mounts.push(Mount::Link {
            link,
            target: target.to_path_buf(),
        });
        self
    }

    pub fn network(mut self, on: bool) -> Self {
        self.network = on;
        self
    }

    /// `setsid` for the command, which blocks `TIOCSTI` input injection into the terminal Ostra
    /// was started from. Off for a PTY program, which needs its PTY as controlling terminal.
    /// Bubblewrap only: Seatbelt denies every terminal device instead, apart from the PTY that
    /// [`Profile::tty`] allows.
    pub fn new_session(mut self, on: bool) -> Self {
        self.new_session = on;
        self
    }

    /// Lets a PTY program use its terminal. Under Seatbelt the PTY's device path is only known
    /// once the PTY is open, so the launcher passes it as `-D TTY=<path>` ([`TTY_PARAM`]).
    pub fn tty(mut self, on: bool) -> Self {
        self.tty = on;
        self
    }

    /// Every rule ordered for both backends: shallower paths first, so the deepest rule for a
    /// path wins, and at one path only the most restrictive rule.
    fn ordered(&self) -> Vec<Mount> {
        let mut all = self.mounts.clone();
        if let Some(home) = &self.home_overlay {
            all.push(Mount::HomeTmpfs(home.clone()));
            if let Ok(rd) = std::fs::read_dir(home) {
                for e in rd.flatten() {
                    let path = e.path();
                    all.push(Mount::HomeEntry(
                        path.clone(),
                        std::fs::read_link(&path).ok(),
                    ));
                }
            }
        }
        all.sort_by(|a, b| {
            (a.dest().components().count(), a.dest(), a.rank()).cmp(&(
                b.dest().components().count(),
                b.dest(),
                b.rank(),
            ))
        });
        let mut out: Vec<Mount> = vec![];
        for m in all {
            if out.last().is_some_and(|l| l.dest() == m.dest()) {
                out.pop();
            }
            out.push(m);
        }
        out
    }

    /// [`Profile::ordered`] as bubblewrap mounts, with placeholders created for missing
    /// protected paths.
    fn plan(&self) -> Vec<Mount> {
        let mut out = self.ordered();
        // A read-only bind whose nearest mounted ancestor is hidden would expose it again, so it
        // is dropped. A missing path can only be created where that ancestor is a writable host
        // dir; there it gets a placeholder, and elsewhere nothing needs mounting.
        let snapshot = out.clone();
        out.retain(|m| {
            let Mount::ReadOnly {
                path,
                dir,
                revealed,
            } = m
            else {
                return true;
            };
            let parent = snapshot
                .iter()
                .filter(|a| a.dest() != path && path.starts_with(a.dest()))
                .max_by_key(|a| a.dest().components().count());
            if matches!(parent, Some(Mount::Hidden(_))) && !revealed {
                return false;
            }
            if std::fs::symlink_metadata(path).is_ok() {
                return true;
            }
            matches!(parent, Some(Mount::Writable { src, dest }) if src == dest)
                && placeholder(path, *dir).is_ok()
        });
        out
    }

    /// Everything bubblewrap needs before the program: mounts, namespaces, and the directory
    /// the program starts in. Creates placeholders for missing protected paths.
    pub fn args(&self, chdir: &Path) -> Vec<OsString> {
        let mut a: Vec<OsString> = vec![];
        let mut push = |xs: &[&std::ffi::OsStr]| a.extend(xs.iter().map(|x| x.to_os_string()));
        push(&["--ro-bind".as_ref(), "/".as_ref(), "/".as_ref()]);
        push(&["--dev".as_ref(), "/dev".as_ref()]);
        push(&["--proc".as_ref(), "/proc".as_ref()]);
        let mut mounts = self.plan();
        match &self.scratch {
            Some(s) => {
                mounts.push(Mount::Writable {
                    src: s.clone(),
                    dest: s.clone(),
                });
                push(&["--bind".as_ref(), s.as_os_str(), "/tmp".as_ref()]);
            }
            None => push(&["--tmpfs".as_ref(), "/tmp".as_ref()]),
        }
        // Mounts under /tmp come after it, in the same depth order.
        mounts.sort_by_key(|m| m.dest().components().count());
        for m in &mounts {
            match m {
                Mount::HomeTmpfs(p) => push(&["--tmpfs".as_ref(), p.as_os_str()]),
                Mount::HomeEntry(p, Some(target)) => {
                    push(&["--symlink".as_ref(), target.as_os_str(), p.as_os_str()])
                }
                Mount::HomeEntry(p, None) | Mount::ReadOnly { path: p, .. } => {
                    push(&["--ro-bind".as_ref(), p.as_os_str(), p.as_os_str()])
                }
                Mount::Writable { src, dest } => {
                    push(&["--bind".as_ref(), src.as_os_str(), dest.as_os_str()])
                }
                Mount::Hidden(p) if p.is_dir() => push(&["--tmpfs".as_ref(), p.as_os_str()]),
                Mount::Hidden(p) => {
                    push(&["--ro-bind".as_ref(), "/dev/null".as_ref(), p.as_os_str()])
                }
                Mount::Link { link, target } => {
                    push(&["--symlink".as_ref(), target.as_os_str(), link.as_os_str()])
                }
            }
        }
        for v in UNSET_VARS {
            push(&["--unsetenv".as_ref(), v.as_ref()]);
        }
        for (k, v) in &self.env {
            push(&["--setenv".as_ref(), k.as_ref(), v.as_ref()]);
        }
        push(&["--unshare-pid".as_ref(), "--unshare-ipc".as_ref()]);
        if !self.network {
            push(&["--unshare-net".as_ref()]);
        }
        push(&["--die-with-parent".as_ref()]);
        if self.new_session {
            push(&["--new-session".as_ref()]);
        }
        push(&["--chdir".as_ref(), chdir.as_os_str()]);
        for f in &self.writable_files {
            push(&["--bind".as_ref(), f.as_os_str(), f.as_os_str()]);
        }
        a
    }

    /// `program args` wrapped for `backend`, started in `chdir`. `Err` when a path cannot be
    /// written into a Seatbelt policy.
    pub fn command<I, A>(
        &self,
        backend: &Backend,
        chdir: &Path,
        program: &OsStr,
        args: I,
    ) -> Result<SandboxedCommand, String>
    where
        I: IntoIterator<Item = A>,
        A: Into<OsString>,
    {
        match backend {
            Backend::Bubblewrap(bin) => {
                let mut a = self.args(chdir);
                a.push("--".into());
                a.push(program.to_os_string());
                a.extend(args.into_iter().map(Into::into));
                Ok(SandboxedCommand {
                    program: bin.clone(),
                    args: a,
                    env: vec![],
                    env_remove: vec![],
                    cwd: chdir.to_path_buf(),
                    members: None,
                })
            }
            Backend::Seatbelt => {
                let members = Members::new();
                let mut a: Vec<OsString> = vec!["-p".into(), self.seatbelt(&members)?.into()];
                a.push(program.to_os_string());
                a.extend(args.into_iter().map(Into::into));
                let mut env: Vec<(String, String)> = self
                    .env
                    .iter()
                    .filter(|(k, _)| k != "TMPDIR")
                    .cloned()
                    .collect();
                if let Some(t) = self.seatbelt_tmp() {
                    env.push(("TMPDIR".into(), format!("{}/", t.display())));
                    // zsh writes heredocs under `$TMPPREFIX`, which defaults to `/tmp/zsh`.
                    env.push(("TMPPREFIX".into(), format!("{}/zsh", t.display())));
                }
                Ok(SandboxedCommand {
                    program: SANDBOX_EXEC.into(),
                    args: a,
                    env,
                    env_remove: UNSET_VARS.iter().map(|v| v.to_string()).collect(),
                    cwd: chdir.to_path_buf(),
                    members: Some(members),
                })
            }
        }
    }

    /// `TMPDIR` under Seatbelt, which cannot remap `/tmp`: the scratch dir, else the workspace
    /// cache's `tmp`.
    fn seatbelt_tmp(&self) -> Option<PathBuf> {
        self.scratch
            .clone()
            .or_else(|| self.cache_root.as_ref().map(|c| c.join("tmp")))
    }

    /// The profile as a Seatbelt policy. Seatbelt applies the last matching rule, so the rules
    /// follow [`Profile::ordered`] and a deeper path overrides a shallower one, as a deeper mount
    /// does under bubblewrap.
    pub fn seatbelt(&self, members: &Members) -> Result<String, String> {
        let mut out = String::from(SEATBELT_BASE);
        out.push_str(&format!(
            "(allow mach-lookup (global-name {}) (global-name {}))\n",
            sbpl_str(&members.own)?,
            sbpl_str(&members.group)?
        ));
        let mut tmp_dirs = vec![
            PathBuf::from("/private/tmp"),
            PathBuf::from("/private/var/tmp"),
        ];
        tmp_dirs.extend(user_temp_dir());
        out.push_str("(deny file-read* file-write*");
        for d in &tmp_dirs {
            out.push_str(&format!(" (subpath {})", sbpl_str(d)?));
        }
        out.push_str(")\n");
        let mut rules = self.ordered();
        if let Some(s) = self.seatbelt_tmp() {
            rules.push(Mount::Writable {
                src: s.clone(),
                dest: s,
            });
        }
        let writable: Vec<PathBuf> = rules
            .iter()
            .filter_map(|m| match m {
                Mount::Writable { src, dest } if src == dest => Some(dest.clone()),
                _ => None,
            })
            .chain(self.writable_files.iter().cloned())
            .collect();
        let mut reached: Vec<PathBuf> = vec![];
        let mut pinned: Vec<PathBuf> = vec![];
        for m in &rules {
            match m {
                Mount::Writable { src, dest } if src == dest => {
                    let p = sbpl_str(dest)?;
                    out.push_str(&format!(
                        "(allow file-read* file-write* (subpath {p}))\n(allow network-bind network-outbound (subpath {p}))\n"
                    ));
                    reached.push(dest.clone());
                }
                Mount::ReadOnly {
                    path,
                    dir,
                    revealed,
                } => {
                    let f = path_filter(path, *dir)?;
                    if *revealed {
                        out.push_str(&format!("(allow file-read* {f})\n"));
                        reached.push(path.clone());
                    }
                    out.push_str(&format!("(deny file-write* network-bind {f})\n"));
                    pinned.push(path.clone());
                }
                Mount::Hidden(p) => {
                    let p = sbpl_str(p)?;
                    out.push_str(&format!(
                        "(deny file-read* file-write* (subpath {p}))\n(deny network-bind network-outbound (subpath {p}))\n"
                    ));
                    pinned.push(m.dest().to_path_buf());
                }
                // Remapping, a disposable home, and symlinks need a mount namespace.
                Mount::Writable { .. }
                | Mount::HomeTmpfs(_)
                | Mount::HomeEntry(..)
                | Mount::Link { .. } => {}
            }
        }
        for f in &self.writable_files {
            out.push_str(&format!(
                "(allow file-read* file-write* (literal {}))\n",
                sbpl_str(f)?
            ));
            reached.push(f.clone());
        }
        // Reaching an allowed path needs `stat` on each dir above it, even inside a denied one.
        let mut ancestors: Vec<&Path> =
            reached.iter().flat_map(|p| p.ancestors().skip(1)).collect();
        ancestors.sort();
        ancestors.dedup();
        if !ancestors.is_empty() {
            out.push_str("(allow file-read-metadata");
            for a in ancestors {
                out.push_str(&format!(" (literal {})", sbpl_str(a)?));
            }
            out.push_str(")\n");
        }
        if self.tty {
            out.push_str(&format!(
                "(allow file-read* file-write* file-ioctl (literal \"/dev/tty\") (literal (param {})))\n",
                sbpl_str(TTY_PARAM)?
            ));
        }
        if self.network {
            out.push_str(SEATBELT_NETWORK);
        }
        // A mount point cannot be renamed; here every writable dir above a protected path is
        // pinned instead, so `mv .git .g` cannot carry `.git/config` out from under its rule.
        let mut pins: Vec<&Path> = pinned
            .iter()
            .flat_map(|p| p.ancestors().skip(1))
            .filter(|a| writable.iter().any(|w| a.starts_with(w)))
            .collect();
        pins.sort();
        pins.dedup();
        if !pins.is_empty() {
            out.push_str("(deny file-write-unlink");
            for p in pins {
                out.push_str(&format!(" (literal {})", sbpl_str(p)?));
            }
            out.push_str(")\n");
        }
        Ok(out)
    }
}

/// The `sandbox-exec -D` parameter holding a PTY program's terminal device.
pub const TTY_PARAM: &str = "TTY";

/// What every Seatbelt policy allows before the profile's own rules, measured with git, cargo,
/// npm, go, pip, and clang through xcrun. The mach services are the ones those tools need; the
/// keychain, pasteboard, LaunchServices, Apple Events, the window server, and TCC stay denied,
/// because each reaches something outside the sandbox. `(deny default)` also denies
/// `lsopen`, `appleevent-send`, and `user-preference-write`. Terminals are denied, so nothing
/// reads keystrokes from, writes to, or injects input into another terminal of the user.
const SEATBELT_BASE: &str = r##"(version 1)
(deny default)
(allow process-exec process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow sysctl-read)
(allow file-read*)
(deny file-read* file-write* file-ioctl (regex #"^/dev/tty"))
(allow file-write-data (literal "/dev/stdout") (literal "/dev/stderr") (regex #"^/dev/fd/[0-9]+$"))
(allow file-write* file-ioctl (literal "/dev/null") (literal "/dev/zero") (literal "/dev/dtracehelper"))
(allow ipc-posix-sem ipc-posix-shm*)
(allow user-preference-read)
(allow mach-lookup (global-name "com.apple.system.opendirectoryd.libinfo" "com.apple.system.opendirectoryd.membership" "com.apple.system.notification_center" "com.apple.system.logger" "com.apple.logd" "com.apple.diagnosticd" "com.apple.trustd.agent" "com.apple.SystemConfiguration.configd" "com.apple.cfprefsd.agent" "com.apple.bsd.dirhelper"))
"##;

/// Outbound IP, a listener on loopback for dev servers and tests, and the DNS resolver's socket.
/// Other Unix sockets (the SSH agent, Docker, password managers, IDEs) stay denied.
const SEATBELT_NETWORK: &str = r##"(allow network-outbound (remote ip))
(allow network-inbound (local ip "localhost:*"))
(allow network-bind (local ip "localhost:*"))
(allow network-outbound (literal "/private/var/run/mDNSResponder"))
"##;

/// `path` as an SBPL string. `Err` for a path that is not UTF-8 or holds a control character,
/// because a path that ends its string early would rewrite the policy.
fn sbpl_str(text: impl AsRef<OsStr>) -> Result<String, String> {
    let text = text.as_ref();
    let s = text.to_str().ok_or_else(|| {
        format!(
            "The sandbox cannot name `{}`: it is not UTF-8.",
            text.display()
        )
    })?;
    if s.chars().any(char::is_control) {
        return Err(format!(
            "The sandbox cannot name `{}`: it holds a control character.",
            s.escape_debug()
        ));
    }
    Ok(format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// `(subpath P)` for a dir, `(literal P)` for a file. A literal rule also covers a missing path,
/// in any letter case, so no placeholder is needed.
fn path_filter(path: &Path, dir: bool) -> Result<String, String> {
    let kind = if dir || path.is_dir() {
        "subpath"
    } else {
        "literal"
    };
    Ok(format!("({kind} {})", sbpl_str(path)?))
}

/// The per-user temp dir under `/private/var/folders`, which holds other programs' sockets and
/// files.
fn user_temp_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0u8; 1024];
        // SAFETY: confstr writes at most `buf.len()` bytes including the terminating NUL.
        let n = unsafe {
            libc::confstr(
                libc::_CS_DARWIN_USER_TEMP_DIR,
                buf.as_mut_ptr().cast(),
                buf.len(),
            )
        };
        if n == 0 || n > buf.len() {
            return None;
        }
        buf.truncate(n - 1);
        use std::os::unix::ffi::OsStringExt;
        real(Path::new(&OsString::from_vec(buf)))
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// An empty dir, or a file holding the neutral value for its format, created only where nothing
/// exists yet.
fn placeholder(path: &Path, dir: bool) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if dir {
        return std::fs::create_dir_all(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body: &[u8] = if path.extension().is_some_and(|e| e == "json") {
        b"{}"
    } else {
        b""
    };
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(body)
}

/// A `.git` dir and each submodule git dir under its `modules/`.
fn git_dirs(git: &Path) -> Vec<PathBuf> {
    let mut out = vec![git.to_path_buf()];
    let mut i = 0;
    while i < out.len() && out.len() < 256 {
        if let Ok(rd) = std::fs::read_dir(out[i].join("modules")) {
            for e in rd.flatten() {
                let p = e.path();
                if p.join("config").is_file() {
                    out.push(p);
                } else if p.is_dir() {
                    // Nested module names keep their slashes as dirs.
                    if let Ok(inner) = std::fs::read_dir(&p) {
                        out.extend(
                            inner
                                .flatten()
                                .map(|e| e.path())
                                .filter(|q| q.join("config").is_file()),
                        );
                    }
                }
            }
        }
        i += 1;
    }
    out
}

fn db_files(db: &Path) -> Vec<PathBuf> {
    if db.as_os_str().is_empty() {
        return vec![];
    }
    ["", "-wal", "-shm", "-journal"]
        .iter()
        .map(|s| {
            let mut p = db.as_os_str().to_os_string();
            p.push(s);
            PathBuf::from(p)
        })
        .collect()
}

/// The roots that are git work trees, and their direct children that are, so a workspace's
/// projects are covered too.
fn git_repos(roots: &[&Path]) -> Vec<PathBuf> {
    let mut out = vec![];
    for root in roots {
        if root.as_os_str().is_empty() {
            continue;
        }
        let Some(root) = real(root) else { continue };
        if root.join(".git").is_dir() && !out.contains(&root) {
            out.push(root.clone());
        }
        if let Ok(entries) = std::fs::read_dir(&root) {
            for e in entries.flatten() {
                let p = e.path();
                if p.join(".git").is_dir() && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn user_id() -> Option<u32> {
    // SAFETY: getuid cannot fail.
    Some(unsafe { libc::getuid() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PermissionMode;

    fn ctx(root: &Path) -> ExecContext {
        ExecContext {
            execution_id: "x_1".into(),
            session_id: None,
            agent: crate::AgentName::Implementer,
            initializer_mode: None,
            executor: crate::ExecutorKind::Native,
            workspace_root: root.to_path_buf(),
            repo_root: root.join("repo"),
            project_key: "p".into(),
            session_dir: root.join(".ostra/sessions/s1"),
            session_root: root.join(".ostra/sessions/s1"),
            report_file: None,
            phase: None,
            yolo: false,
            permission_mode: PermissionMode::Default,
            permissions: Default::default(),
            protected_paths: vec![],
            memory_db: PathBuf::new(),
            sandbox_mode: None,
        }
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// Runs `script` under `profile` and returns its output, or `None` without a sandbox.
    fn run_in(profile: &Profile, chdir: &Path, script: &str) -> Option<String> {
        let Some(b) = backend() else {
            eprintln!("no sandbox here ({}); skipping", unavailable_message());
            return None;
        };
        let sc = profile
            .command(b, chdir, OsStr::new("sh"), ["-c", script])
            .unwrap();
        let out = sc.std_command().output().unwrap();
        Some(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }

    fn seatbelt() -> bool {
        backend() == Some(&Backend::Seatbelt)
    }

    /// A workspace with a git repo and two sessions, and a home folder outside it.
    fn layout() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let top = std::fs::canonicalize(d.path()).unwrap();
        let root = top.join("w");
        std::fs::create_dir_all(&root).unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "-q", "repo"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(ok.success());
        std::fs::create_dir_all(root.join(".ostra/sessions/s1")).unwrap();
        std::fs::create_dir_all(root.join(".ostra/sessions/s2/.state/harness/x_2")).unwrap();
        std::fs::write(
            root.join(".ostra/sessions/s2/.state/harness/x_2/config.toml"),
            "TOKEN",
        )
        .unwrap();
        let home = top.join("home");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join(".cargo/registry")).unwrap();
        std::fs::write(home.join(".cargo/config.toml"), "[build]\n").unwrap();
        std::fs::write(home.join(".bashrc"), "").unwrap();
        (d, root, home)
    }

    #[test]
    fn profile_args_bind_roots_hide_secrets_and_set_caches() {
        let (_d, root, home) = layout();
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
        let args = strings(p.args(&root.join("repo")));
        let has = |pair: &[&str]| args.windows(pair.len()).any(|w| w == pair);
        let s = |p: PathBuf| p.display().to_string();
        assert!(has(&[
            "--bind",
            &s(root.join("repo")),
            &s(root.join("repo"))
        ]));
        assert!(has(&[
            "--bind",
            &s(root.join("repo/.git")),
            &s(root.join("repo/.git"))
        ]));
        assert!(has(&["--tmpfs", &s(home.join(".ssh"))]));
        assert!(has(&["--tmpfs", &s(root.join(".ostra/sessions"))]));
        assert!(has(&["--tmpfs", "/tmp"]));
        assert!(!args.iter().any(|a| a == &s(home.join(".cargo"))));
        let cargo = args
            .windows(3)
            .find(|w| w[0] == "--setenv" && w[1] == "CARGO_HOME")
            .map(|w| PathBuf::from(&w[2]))
            .expect("CARGO_HOME is set");
        assert!(cargo.starts_with(home.join(".cache/ostra/sandbox")));
        assert!(cargo.join("config.toml").is_symlink());
        assert!(has(&["--setenv", "TMPDIR", "/tmp"]));
        assert!(has(&["--unshare-pid"]) && has(&["--new-session"]));
        assert!(!has(&["--unshare-net"]));
        let off = SandboxConfig {
            network: false,
            ..Default::default()
        };
        let args = Profile::for_execution(&ctx(&root), &off, &home)
            .new_session(false)
            .args(&root);
        assert!(args.iter().any(|a| a == "--unshare-net"));
        assert!(!args.iter().any(|a| a == "--new-session"));
    }

    #[test]
    fn deeper_rules_win_and_git_stays_usable() {
        let (_d, root, home) = layout();
        let own = paths::harness_execution_dir(&root.join(".ostra/sessions/s1"), "x_1");
        let scratch = root.join("scratch");
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
            .scratch(&scratch)
            .unwrap()
            .writable(&{
                std::fs::create_dir_all(&own).unwrap();
                own.clone()
            });
        let script = format!(
            r#"
            touch {own}/ok && echo own-harness-writable
            touch {state}/x 2>/dev/null || echo state-read-only
            cat {other} 2>/dev/null || echo other-session-hidden
            echo a > f && git add f && git -c user.email=a@b -c user.name=a commit -qm t && echo committed
            git config core.fsmonitor x 2>/dev/null || echo git-config-read-only
            touch .git/hooks/pre-commit 2>/dev/null || echo hooks-read-only
            mv .git .git-moved 2>/dev/null || echo git-not-renamed
            echo x >> {rc} 2>/dev/null || echo rc-read-only
            touch {cargo_reg}/x 2>/dev/null || echo cargo-read-only
            echo t > "${{TMPDIR:-/tmp}}/t" && cat {scratch}/t && echo scratch-shared
            "#,
            own = own.display(),
            state = root.join(".ostra/sessions/s1/.state").display(),
            other = root
                .join(".ostra/sessions/s2/.state/harness/x_2/config.toml")
                .display(),
            rc = home.join(".bashrc").display(),
            cargo_reg = home.join(".cargo/registry").display(),
            scratch = scratch.display(),
        );
        let Some(out) = run_in(&p, &root.join("repo"), &script) else {
            return;
        };
        for want in [
            "own-harness-writable",
            "state-read-only",
            "other-session-hidden",
            "committed",
            "git-config-read-only",
            "hooks-read-only",
            "git-not-renamed",
            "rc-read-only",
            "cargo-read-only",
            "scratch-shared",
        ] {
            assert!(out.contains(want), "missing {want}:\n{out}");
        }
        assert!(!out.contains("TOKEN"));
        assert!(own.join("ok").exists());
        assert!(scratch.join("t").exists());
        assert!(root.join("repo/.git").is_dir());
        assert!(!root.join("repo/.git/hooks/pre-commit").exists());
        if !seatbelt() {
            assert_eq!(p.to_host(Path::new("/tmp/a/b")), scratch.join("a/b"));
        }
    }

    #[test]
    fn missing_protected_paths_get_read_only_placeholders_under_writable_dirs() {
        let (_d, root, home) = layout();
        let claude = home.join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let cfg = SandboxConfig {
            extra_writable: vec!["~/.config".into()],
            ..Default::default()
        };
        std::fs::create_dir_all(home.join(".config")).unwrap();
        let p = Profile::for_execution(&ctx(&root), &cfg, &home)
            .writable(&claude)
            .read_only(&claude.join("settings.json"))
            .read_only_dir(&claude.join("hooks"));
        let script = format!(
            r#"
            echo '{{"hooks":1}}' > {settings} 2>/dev/null || echo settings-refused
            touch {hooks}/h 2>/dev/null || echo hooks-refused
            mkdir -p {fish} && touch {fish}/config.fish 2>/dev/null || echo fish-refused
            touch {claude}/other && echo claude-writable
            "#,
            settings = claude.join("settings.json").display(),
            hooks = claude.join("hooks").display(),
            fish = home.join(".config/fish").display(),
            claude = claude.display(),
        );
        let Some(out) = run_in(&p, &root, &script) else {
            return;
        };
        for want in [
            "settings-refused",
            "hooks-refused",
            "fish-refused",
            "claude-writable",
        ] {
            assert!(out.contains(want), "missing {want}:\n{out}");
        }
        // Bubblewrap mounts a placeholder; Seatbelt's literal rule refuses the missing path.
        if seatbelt() {
            assert!(!claude.join("settings.json").exists());
        } else {
            assert_eq!(
                std::fs::read_to_string(claude.join("settings.json")).unwrap(),
                "{}"
            );
        }
        assert!(!claude.join("hooks/h").exists());
        assert!(!home.join(".config/fish/config.fish").exists());
        // A home that is read-only anyway gets no placeholder files.
        assert!(!home.join(".zshrc").exists());
    }

    #[test]
    fn a_harness_keeps_only_its_own_sign_in_file() {
        let (_d, root, home) = layout();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".claude/.credentials.json"), "{}").unwrap();
        std::fs::write(home.join(".codex/auth.json"), "{}").unwrap();
        let mut c = ctx(&root);
        c.executor = crate::ExecutorKind::Harness(crate::HarnessKind::Claude);
        let args =
            strings(Profile::for_execution(&c, &SandboxConfig::default(), &home).args(&root));
        let hidden = |f: &str| {
            let f = home.join(f).display().to_string();
            args.windows(3)
                .any(|w| w[0] == "--ro-bind" && w[1] == "/dev/null" && w[2] == f)
        };
        assert!(!hidden(".claude/.credentials.json"));
        assert!(hidden(".codex/auth.json"));
        let args = strings(
            Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home).args(&root),
        );
        assert!(args.windows(3).any(|w| w[1] == "/dev/null"
            && w[2] == home.join(".claude/.credentials.json").display().to_string()));
    }

    #[test]
    fn decide_follows_the_mode() {
        let off = SandboxConfig {
            mode: SandboxMode::Off,
            ..Default::default()
        };
        assert_eq!(decide(&off), Ok(Decision::Unsandboxed(None)));
        let required = SandboxConfig {
            mode: SandboxMode::Required,
            ..Default::default()
        };
        match backend() {
            Some(b) => assert_eq!(decide(&required), Ok(Decision::Sandboxed(b.clone()))),
            None => assert!(decide(&required).is_err()),
        }
    }

    fn policy(p: &Profile) -> String {
        p.seatbelt(&Members {
            own: "dev.ostra.sandbox.test".into(),
            group: "dev.ostra.sandbox.dtest".into(),
        })
        .unwrap()
    }

    #[test]
    fn sbpl_strings_cannot_break_out() {
        assert_eq!(sbpl_str("/a b").unwrap(), r#""/a b""#);
        assert_eq!(
            sbpl_str(r#"/tmp/a") (allow file-write* (subpath "/"#).unwrap(),
            r#""/tmp/a\") (allow file-write* (subpath \"/""#
        );
        assert_eq!(sbpl_str(r"/a\b").unwrap(), r#""/a\\b""#);
        assert!(sbpl_str("/a\nb").is_err());
        assert!(sbpl_str("/a\u{7}b").is_err());
    }

    #[test]
    fn seatbelt_policy_orders_rules_and_pins_protected_paths() {
        let (_d, root, home) = layout();
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
        let text = policy(&p);
        let s = |p: PathBuf| p.display().to_string();
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("missing {needle}:\n{text}"))
        };
        let repo = s(root.join("repo"));
        let git = s(root.join("repo/.git"));
        let writable_repo = at(&format!(
            "(allow file-read* file-write* (subpath \"{repo}\"))"
        ));
        let writable_git = at(&format!(
            "(allow file-read* file-write* (subpath \"{git}\"))"
        ));
        let config = at(&format!(
            "(deny file-write* network-bind (literal \"{git}/config\"))"
        ));
        assert!(writable_repo < writable_git && writable_git < config);
        at(&format!(
            "(deny file-write* network-bind (subpath \"{git}/hooks\"))"
        ));
        let sessions = s(root.join(".ostra/sessions"));
        let hidden = at(&format!(
            "(deny file-read* file-write* (subpath \"{sessions}\"))"
        ));
        let own = at(&format!(
            "(allow file-read* file-write* (subpath \"{sessions}/s1\"))"
        ));
        assert!(hidden < own);
        at(&format!(
            "(deny file-read* file-write* (subpath \"{}\"))",
            s(home.join(".ssh"))
        ));
        // `stat` on the dirs above an allowed session, even inside the hidden sessions dir.
        let meta = at("(allow file-read-metadata");
        assert!(meta > own && text[meta..].contains(&format!("(literal \"{sessions}\")")));
        let pins = &text[at("(deny file-write-unlink")..];
        for pinned in [&git, &repo, &s(root.join(".ostra")), &sessions] {
            assert!(
                pins.contains(&format!("(literal \"{pinned}\")")),
                "{pinned} not pinned:\n{pins}"
            );
        }
        assert!(!pins.contains(&format!("(literal \"{}\")", s(home.clone()))));
        assert!(text.contains("(allow network-outbound (remote ip))"));
        assert!(text.contains("dev.ostra.sandbox.test"));
        assert!(!text.contains("(param"));
        let off = SandboxConfig {
            network: false,
            ..Default::default()
        };
        let text = policy(&Profile::for_execution(&ctx(&root), &off, &home).tty(true));
        assert!(!text.contains("(remote ip)"));
        assert!(text.contains("(literal (param \"TTY\"))"));
    }

    #[test]
    fn seatbelt_command_sets_tmpdir_and_unsets_agent_sockets() {
        let (_d, root, home) = layout();
        let scratch = root.join("scratch");
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
            .scratch(&scratch)
            .unwrap();
        let sc = p
            .command(&Backend::Seatbelt, &root, OsStr::new("true"), ["a"])
            .unwrap();
        assert_eq!(sc.program, Path::new(SANDBOX_EXEC));
        assert_eq!(sc.args[0], "-p");
        assert_eq!(
            &sc.args[2..],
            &[OsString::from("true"), OsString::from("a")]
        );
        assert!(
            sc.env
                .contains(&("TMPDIR".into(), format!("{}/", scratch.display())))
        );
        assert!(sc.env.iter().any(|(k, _)| k == "CARGO_HOME"));
        assert!(sc.env_remove.iter().any(|k| k == "SSH_AUTH_SOCK"));
        assert!(sc.env_remove.iter().any(|k| k == "DOCKER_HOST"));
        assert!(sc.members.is_some());
        let bad = root.join("a\nb");
        std::fs::create_dir_all(&bad).unwrap();
        let p = p.writable(&bad);
        assert!(
            p.command(&Backend::Seatbelt, &root, OsStr::new("true"), ["a"])
                .is_err()
        );
    }

    /// Everything macOS offers for reaching outside a Seatbelt sandbox, refused.
    #[test]
    fn seatbelt_refuses_escapes_to_the_rest_of_the_user_session() {
        if !seatbelt() {
            eprintln!("seatbelt unavailable; skipping");
            return;
        }
        let (_d, root, home) = layout();
        let label = format!("dev.ostra.test.{}", uuid::Uuid::new_v4().simple());
        let escaped = root.join("escaped");
        let script = format!(
            r#"
            launchctl submit -l {label} -- /usr/bin/touch {escaped} 2>/dev/null && echo launchd-submitted
            open -a Calculator 2>/dev/null && echo opened-app
            osascript -e 'tell application "Finder" to get name of startup disk' 2>/dev/null && echo apple-event
            defaults write {label} k -string v 2>/dev/null && echo pref-written
            security list-keychains 2>/dev/null | grep -q keychain && echo keychain-reached
            pbpaste >/dev/null 2>&1 && echo clipboard-read
            echo x >> /dev/tty 2>/dev/null && echo tty-written
            echo done
            "#,
            escaped = escaped.display()
        );
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
        let out = run_in(&p, &root.join("repo"), &script).unwrap();
        let _ = std::process::Command::new("launchctl")
            .args(["remove", &label])
            .output();
        let _ = std::process::Command::new("defaults")
            .args(["delete", &label])
            .output();
        assert!(out.contains("done"), "{out}");
        for bad in [
            "launchd-submitted",
            "opened-app",
            "apple-event",
            "pref-written",
            "keychain-reached",
            "clipboard-read",
            "tty-written",
        ] {
            assert!(!out.contains(bad), "{bad}:\n{out}");
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!escaped.exists());
    }

    #[test]
    fn seatbelt_keeps_protected_paths_under_renames_links_and_case() {
        if !seatbelt() {
            eprintln!("seatbelt unavailable; skipping");
            return;
        }
        let (_d, root, home) = layout();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/id"), "SECRET").unwrap();
        let data_home = Path::new("/System/Volumes/Data").join(home.strip_prefix("/").unwrap());
        let script = format!(
            r#"
            mv .git .g 2>/dev/null && echo git-renamed
            mv ../repo ../r2 2>/dev/null && echo repo-renamed
            rm -rf ../.ostra/sessions 2>/dev/null
            test -d ../.ostra/sessions/s2 && echo other-session-visible
            echo x >> .git/CONFIG 2>/dev/null && echo case-config-written
            echo x > .git/hooks/PRE-COMMIT 2>/dev/null && echo case-hook-written
            mkdir -p ../evil && mv ../evil .git/info 2>/dev/null && echo moved-onto-protected
            ln .git/config gc 2>/dev/null && echo x >> gc 2>/dev/null && echo linked-config-written
            ln {key} k 2>/dev/null && cat k
            cat {firmlink} 2>/dev/null
            cat {upper} 2>/dev/null
            echo a > f && ln f f2 && echo own-link-ok
            "#,
            key = home.join(".ssh/id").display(),
            firmlink = data_home.join(".ssh/id").display(),
            upper = home.join(".SSH/id").display(),
        );
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
        let out = run_in(&p, &root.join("repo"), &script).unwrap();
        for bad in [
            "git-renamed",
            "repo-renamed",
            "case-config-written",
            "case-hook-written",
            "moved-onto-protected",
            "linked-config-written",
            "other-session-visible",
            "SECRET",
        ] {
            assert!(!out.contains(bad), "{bad}:\n{out}");
        }
        assert!(out.contains("own-link-ok"), "{out}");
        assert!(
            root.join(".ostra/sessions/s2/.state/harness/x_2/config.toml")
                .is_file()
        );
        assert!(root.join("repo/.git/config").is_file());
    }

    #[test]
    fn seatbelt_hides_processes_sockets_and_shared_temp() {
        if !seatbelt() {
            eprintln!("seatbelt unavailable; skipping");
            return;
        }
        let (_d, root, home) = layout();
        let mut victim = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let tmp_marker =
            std::env::temp_dir().join(format!("ostra-host-{}", uuid::Uuid::new_v4().simple()));
        std::fs::write(&tmp_marker, "HOSTTMP").unwrap();
        let script = format!(
            r#"
            kill {pid} 2>/dev/null && echo killed-outside
            ps -p {pid} 2>/dev/null | grep -q sleep && echo saw-outside
            cat {marker} 2>/dev/null
            ls /private/tmp >/dev/null 2>&1 && echo listed-tmp
            curl -s --max-time 2 --unix-socket /var/run/docker.sock http://x/version >/dev/null 2>&1 && echo docker
            echo "${{SSH_AUTH_SOCK-unset}}"
            echo t > "$TMPDIR/t" && echo own-tmp
            "#,
            pid = victim.id(),
            marker = tmp_marker.display(),
        );
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
            .scratch(&root.join("scratch"))
            .unwrap();
        let out = run_in(&p, &root.join("repo"), &script).unwrap();
        let alive = victim.try_wait().unwrap().is_none();
        let _ = victim.kill();
        let _ = std::fs::remove_file(&tmp_marker);
        assert!(alive, "{out}");
        for bad in [
            "killed-outside",
            "saw-outside",
            "HOSTTMP",
            "listed-tmp",
            "docker",
        ] {
            assert!(!out.contains(bad), "{bad}:\n{out}");
        }
        assert!(out.contains("unset") && out.contains("own-tmp"), "{out}");
    }

    /// System sandboxes that allow every mach name (Image Capture's `icdd`, for one) must not
    /// read as Ostra's, or killing a sandbox's members would kill them too.
    #[test]
    fn an_unused_marker_matches_no_process() {
        let unused = format!("dev.ostra.sandbox.{}", uuid::Uuid::new_v4().simple());
        assert_eq!(marked(&unused), Vec::<libc::pid_t>::new());
    }

    #[test]
    fn seatbelt_members_die_with_the_invocation_even_after_setsid() {
        if !seatbelt() {
            eprintln!("seatbelt unavailable; skipping");
            return;
        }
        let (_d, root, home) = layout();
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
        let pid_file = root.join("repo/pid");
        let script = format!(
            "perl -e 'use POSIX; if (fork()==0) {{ POSIX::setsid(); if (fork()==0) {{ open(F, \">{}\"); print F $$; close F; sleep 30; exit }} exit }}'; sleep 0.3",
            pid_file.display()
        );
        let sc = p
            .command(
                backend().unwrap(),
                &root.join("repo"),
                OsStr::new("sh"),
                ["-c", &script],
            )
            .unwrap();
        let status = sc.std_command().status().unwrap();
        assert!(status.success());
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SAFETY: signal 0 only checks that the pid exists.
        let alive = || unsafe { libc::kill(pid, 0) } == 0;
        assert!(
            alive(),
            "the detached child should still run before the members are killed"
        );
        drop(sc);
        assert!(!alive(), "the detached child outlived its sandbox");
    }
}
