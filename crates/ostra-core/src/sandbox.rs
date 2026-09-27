//! Sandbox profiles for agent commands. The policy layer judges each tool call by the paths it
//! names, which a shell can hide; the sandbox is what the kernel enforces underneath it: the host
//! is read-only, only the execution's own roots are writable, and Ostra's data, the user's
//! credentials, and the session bus or keychain are not reachable at all.
//!
//! One [`Profile`] renders for two backends: bubblewrap mounts on Linux, and a Seatbelt (SBPL)
//! policy for `/usr/bin/sandbox-exec` on macOS. Seatbelt has no mount namespace, so on macOS a
//! hidden path returns EPERM instead of looking empty, `/tmp` is denied instead of private
//! (`TMPDIR` points at the scratch dir), and a harness gets no disposable home.

use crate::config::{SandboxConfig, SandboxMode, SandboxNetwork};
use crate::egress;
use crate::exec::ExecContext;
use crate::paths;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

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
    "AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Startup/",
    "Documents/PowerShell/",
    "Documents/WindowsPowerShell/",
];

/// Paths inside a `.git` dir that choose programs git runs. A trailing `/` marks a dir.
/// `config.worktree` is read only with `extensions.worktreeConfig`, which `config` sets, so the
/// empty placeholder a missing one gets is harmless. `commondir` cannot have one: git refuses to
/// open a repository whose `commondir` is empty; [`repair_git_dirs`] covers it instead.
const GIT_PROTECTED: &[&str] = &["config", "config.worktree", "hooks/", "info/"];

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

/// The sandbox's own tool caches under `~/.cache/ostra/sandbox/` (or `$OSTRA_SANDBOX_CACHE`): one
/// set per session for agents and one per project for project programs. The user's `~/.cargo`, `~/.npm`, and `~/.cache` stay read-only, because builds on the host run
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

fn cache_base(home: &Path) -> PathBuf {
    std::env::var_os("OSTRA_SANDBOX_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache/ostra/sandbox"))
}

/// The tool caches of every agent execution in one session.
pub fn session_cache(home: &Path, session: &crate::ids::SessionId) -> PathBuf {
    cache_base(home).join(format!("session-{session}"))
}

/// Caches keyed by a path. `salt` keeps two users of the same path apart.
fn keyed_cache(home: &Path, salt: &str, key: &Path) -> PathBuf {
    cache_base(home).join(stable_hex(salt, key))
}

/// 16 hex digits of SHA-256 over `salt` and `path`. Names on disk use it instead of
/// `DefaultHasher`, whose algorithm may change between Rust releases and would move them.
fn stable_hex(salt: &str, path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update([0]);
    h.update(path.as_os_str().as_encoded_bytes());
    h.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Removes a session's tool caches on a thread of its own, because they can hold gigabytes.
pub fn remove_session_cache(session: &crate::ids::SessionId) {
    let Some(home) = paths::home() else {
        return;
    };
    let dir = session_cache(Path::new(&home), session);
    if dir.is_dir() {
        let _ = std::thread::Builder::new()
            .name("ostra-cache-remove".into())
            .spawn(move || {
                if let Err(e) = std::fs::remove_dir_all(&dir) {
                    tracing::warn!("could not remove the session cache {}: {e}", dir.display());
                }
            });
    }
}

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

static HELPER: OnceLock<PathBuf> = OnceLock::new();

/// Names the `ostra` binary that starts first inside every bubblewrap sandbox
/// (`ostra sandbox-init`). Call before the first probe.
pub fn set_helper(path: PathBuf) {
    let _ = HELPER.set(path);
}

/// The helper binary: the one [`set_helper`] named, else an `ostra` next to this program or one
/// dir up, which finds `target/debug/ostra` from a test binary in `target/debug/deps`.
pub fn helper() -> Option<PathBuf> {
    if let Some(p) = HELPER.get() {
        return Some(p.clone());
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("ostra"), dir.parent()?.join("ostra")]
        .into_iter()
        .find(|p| p.is_file())
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
    #[cfg(target_os = "linux")]
    return probe_bwrap().map(Backend::Bubblewrap);
    #[cfg(target_os = "macos")]
    return probe_seatbelt().map(|()| Backend::Seatbelt);
    #[cfg(windows)]
    return Err("The Windows sandbox is not built yet.".into());
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    Err("The sandbox needs Linux or macOS.".into())
}

#[cfg(target_os = "linux")]
fn probe_bwrap() -> Result<PathBuf, String> {
    let bin = std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join("bwrap"))
                .find(|p| p.is_file())
        })
        .ok_or("The `bwrap` command is not installed.")?;
    let helper = helper().ok_or(
        "The `ostra` binary that starts first inside each sandbox was not found next to this program.",
    )?;
    // The helper binds the new network namespace's loopback, so this also checks it comes up.
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
            "--unshare-net",
            "--",
        ])
        .arg(&helper)
        .args(["sandbox-init", "--proxy", "/dev/null", "--", "true"])
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

#[cfg(target_os = "macos")]
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
#[cfg(windows)]
pub const UNAVAILABLE: &str = "Run Ostra inside WSL 2 or in Docker to sandbox agent commands, because Ostra has no Windows sandbox yet and agent commands otherwise run with the full rights of your user. Choose sandbox mode off in the workspace settings, or set `[sandbox] mode = \"off\"` in config.toml, to run without it on purpose.";
#[cfg(not(any(target_os = "macos", windows)))]
pub const UNAVAILABLE: &str = "Install bubblewrap (the `bwrap` command) on the Linux machine that runs Ostra, or allow unprivileged user namespaces, because agent commands otherwise run with the full rights of your user. Choose sandbox mode off in the workspace settings, or set `[sandbox] mode = \"off\"` in config.toml, to run without it on purpose.";

/// What the sandbox on this machine cannot enforce under `network`, for the setup check.
pub fn known_gaps(backend: &Backend, _network: SandboxNetwork) -> Option<String> {
    match backend {
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        Backend::Bubblewrap(_) => None,
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        Backend::Bubblewrap(_) => Some("Run Ostra on x86_64 or aarch64 where agent commands must not create user namespaces, because the seccomp filter that refuses them is built for those two only.".into()),
        Backend::Seatbelt => None,
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
/// config is read fresh, and `ws` holds the workspace's own sandbox settings. `Err` when the mode
/// is `required` and no sandbox is available.
pub fn host_command(
    program: &str,
    args: &[String],
    cwd: &Path,
    roots: &[&Path],
    ws: &crate::config::WorkspaceSandbox,
) -> Result<HostCommand, String> {
    let mut global: crate::config::GlobalConfig =
        crate::config::load_toml(&paths::global_config_path()).unwrap_or_default();
    global.sandbox = global.sandbox.for_workspace(ws);
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
    let home = paths::home()
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
    /// The egress proxy and forwarded sockets, held so they live as long as the invocation.
    /// Under Seatbelt their ports are the loopback ports its policy allows.
    listeners: Vec<Arc<egress::Listener>>,
    /// Held so the decoys stay watched while a process of the invocation runs.
    #[allow(dead_code)]
    decoys: Option<Arc<crate::decoy::Decoys>>,
}

impl Members {
    fn new(
        listeners: Vec<Arc<egress::Listener>>,
        decoys: Option<Arc<crate::decoy::Decoys>>,
    ) -> Members {
        Members {
            own: format!("dev.ostra.sandbox.{}", uuid::Uuid::new_v4().simple()),
            group: group_marker(),
            listeners,
            decoys,
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
    format!("dev.ostra.sandbox.d{}", stable_hex("", &paths::data_dir()))
}

/// Kills processes that sandboxes of this data dir left running. Call once at server start,
/// before any execution starts.
pub fn kill_leftovers() -> usize {
    kill_marked(&group_marker())
}

/// Removes the egress sockets a server of this data dir left behind when it stopped without
/// dropping them. Call once at server start, while no other server of the data dir runs.
pub fn remove_stale_sockets() -> usize {
    let Ok(dir) = std::fs::read_dir(egress_dir()) else {
        return 0;
    };
    dir.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "sock"))
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// SIGKILLs every process of this user in a sandbox marked `marker`, until none is left. Returns
/// how many it killed.
fn kill_marked(marker: &str) -> usize {
    #[allow(unused_mut)]
    let mut total = 0;
    for _ in 0..20 {
        let found = marked(marker);
        if found.is_empty() {
            break;
        }
        #[cfg(unix)]
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
fn marked(_marker: &str) -> Vec<i32> {
    vec![]
}

/// Moves this process's environment off the stack and zeroes the original strings. On macOS any
/// unsandboxed program of the same user, agent commands under mode `off` included, reads another
/// process's startup environment with `sysctl(KERN_PROCARGS2)`, which reads those stack strings;
/// Ostra's Seatbelt policies refuse it. Call first thing in `main`, before any thread starts.
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
    crate::paths::canonical(p).ok()
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

use paths::{create_private_dir as make_private_dir, create_private_dir_all as make_private_dir_all};

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
    /// What the network namespace lets out, through the egress proxy.
    policy: egress::Policy,
    /// The proxy shared by every command of this profile, once started.
    egress: Option<Arc<egress::Listener>>,
    /// Loopback ports inside the sandbox forwarded to host sockets, such as the hook bridge.
    forwards: Vec<(u16, Arc<egress::Listener>)>,
    /// Fake credential files bound into hidden paths and watched, once planted.
    decoys: Option<Arc<crate::decoy::Decoys>>,
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
            policy: egress::Policy::new(cfg),
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
        // One session's downloads must not feed a later session's builds, so each session gets
        // its own caches. A run outside a session keys on its workspace, apart from programs.
        let cache = match &ctx.session_id {
            Some(id) => session_cache(home, id),
            None => {
                let key = if ctx.workspace_root.as_os_str().is_empty() {
                    &ctx.repo_root
                } else {
                    &ctx.workspace_root
                };
                keyed_cache(home, "agents", key)
            }
        };
        let own = match ctx.executor {
            crate::ExecutorKind::Harness(h) => Some(h),
            crate::ExecutorKind::Native => None,
        };
        p = p.common(cfg, home, own, cache);
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
    /// `roots` writable and the same network choice.
    pub fn for_program(roots: &[&Path], cfg: &SandboxConfig, home: &Path) -> Profile {
        let mut p = Profile {
            policy: egress::Policy::new(cfg),
            ..Profile::base()
        };
        for r in roots {
            p = p.writable(r);
        }
        let key = roots.first().copied().unwrap_or(Path::new("/"));
        p = p.common(cfg, home, None, keyed_cache(home, "", key));
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
        cache: PathBuf,
    ) -> Profile {
        for w in &cfg.extra_writable {
            self = self.writable(&expand(home, w));
        }
        self.add_caches(cache, home);
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

    /// What chooses programs in each repo's git dir read-only, at any depth under `roots`, and a
    /// `.git` file read-only so it cannot name a planted git dir. The dirs above them are pinned
    /// by [`Profile::plan`], so none of them can be renamed away.
    fn protect_git(mut self, roots: &[&Path]) -> Profile {
        for repo in git_repos(roots) {
            let Some(git) = GitDir::of(&repo) else {
                continue;
            };
            if git.file {
                self = self.read_only(&repo.join(".git"));
            }
            if git.linked {
                // A linked worktree reads its main repo's config, which that repo's entry covers.
                self = self.protect(&git.dir, "config.worktree");
                continue;
            }
            for dir in git_dirs(&git.dir) {
                for rel in GIT_PROTECTED {
                    self = self.protect(&dir, rel);
                }
                for w in read_dirs(&dir.join("worktrees")) {
                    self = self.protect(&w, "config.worktree");
                }
            }
        }
        self
    }

    /// The tool caches the sandbox owns under `root`, set through their variables.
    fn add_caches(&mut self, root: PathBuf, home: &Path) {
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
            #[cfg(unix)]
            if from.is_file() && std::fs::symlink_metadata(&to).is_err() {
                let _ = std::os::unix::fs::symlink(&from, &to);
            }
            #[cfg(not(unix))]
            let _ = (from, to);
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

    /// Hosts a harness CLI reaches under every network choice: see [`egress::Policy::allow`].
    pub fn allow_hosts(
        mut self,
        builtin: Vec<egress::HostRule>,
        trusted: Vec<egress::HostRule>,
    ) -> Self {
        self.policy.allow(builtin, trusted);
        self
    }

    /// Starts this profile's egress proxy, shared by every command it wraps, when the policy
    /// lets anything out of a private network: a socket bound into each bubblewrap sandbox, or a
    /// port of `127.0.0.1` that each Seatbelt policy names. A command wrapped without one starts
    /// its own, which reports decisions to the log only.
    pub fn start_egress(
        mut self,
        backend: &Backend,
        on: egress::OnDecision,
    ) -> std::io::Result<Self> {
        if self.policy.needs_proxy() {
            self.egress = Some(Arc::new(new_proxy(backend, self.policy.clone(), on)?));
        }
        Ok(self)
    }

    /// Rule P3: plants the built-in decoys and the workspace's own (`extra`, `~/` paths), and
    /// reports each one's first open to `on`. Under bubblewrap a decoy goes where a hidden dir's
    /// tmpfs can hold a new file, or over an existing file, so the host is never written. Under
    /// Seatbelt only an existing file can be one, refused and reported through the system log.
    pub fn watch_decoys(
        mut self,
        backend: &Backend,
        home: &Path,
        extra: &[String],
        on: crate::decoy::OnOpen,
    ) -> std::io::Result<Self> {
        let ssh_reachable = self.policy.reaches_port(22);
        let wanted = crate::decoy::wanted(extra)
            .into_iter()
            .filter(|(_, kind)| !(*kind == crate::decoy::Kind::SshKey && ssh_reachable));
        if *backend == Backend::Seatbelt {
            // Seatbelt reports no refusal for a missing file, so only existing ones qualify.
            let mut at: Vec<PathBuf> = vec![];
            for (rel, _) in wanted {
                if let Some(p) = real(&home.join(&rel)).filter(|p| p.is_file())
                    && !at.contains(&p)
                {
                    at.push(p);
                }
            }
            if !at.is_empty() && crate::decoy::supported(backend) {
                // The system log is outside Ostra's control, so the execution runs on without decoys.
                match crate::decoy::Decoys::refuse(&at, on) {
                    Ok(d) => self.decoys = Some(Arc::new(d)),
                    Err(e) => tracing::warn!("decoy files are not watched: {e}"),
                }
            }
            return Ok(self);
        }
        let mounts = self.ordered();
        let in_tmpfs = |path: &Path| {
            mounts
                .iter()
                .filter(|m| m.dest() != path && path.starts_with(m.dest()))
                .max_by_key(|m| m.dest().components().count())
                .is_some_and(|m| matches!(m, Mount::Hidden(_) | Mount::HomeTmpfs(_)))
        };
        let mut at: Vec<(PathBuf, crate::decoy::Kind)> = vec![];
        for (rel, kind) in wanted {
            let Some(dest) = real_or_missing(&home.join(&rel)) else {
                continue;
            };
            let fits = match std::fs::metadata(&dest) {
                Ok(m) => m.is_file(),
                Err(_) => in_tmpfs(&dest),
            };
            if fits && !at.iter().any(|(d, _)| *d == dest) {
                at.push((dest, kind));
            }
        }
        if !at.is_empty() {
            self.decoys = Some(Arc::new(crate::decoy::Decoys::plant(
                &crate::decoy::dir(),
                &at,
                on,
            )?));
        }
        Ok(self)
    }

    /// Makes a loopback port inside the sandbox reach `target` on the host: under bubblewrap
    /// `127.0.0.1:<target port>` in its network namespace, through a socket of its own and the
    /// helper; under Seatbelt a fresh port of `127.0.0.1` that its policy names
    /// ([`Profile::forwarded_port`]). Under `host` the target is reachable as it is, so nothing
    /// is forwarded.
    pub fn forward(
        mut self,
        backend: &Backend,
        target: std::net::SocketAddr,
    ) -> std::io::Result<Self> {
        if self.private_network() {
            let l = match backend {
                Backend::Bubblewrap(_) => egress::splice(&egress_dir(), target)?,
                Backend::Seatbelt => egress::loopback_splice(egress::Target::Tcp(target))?,
            };
            self.forwards.push((target.port(), Arc::new(l)));
        }
        Ok(self)
    }

    /// Like [`Profile::forward`], to a Unix socket that Ostra serves itself, such as the hook
    /// bridge socket, which answers only the harness callbacks. `port` is the loopback port the
    /// sandbox knows it by under bubblewrap.
    pub fn forward_socket(
        mut self,
        backend: &Backend,
        port: u16,
        socket: &Path,
    ) -> std::io::Result<Self> {
        if self.private_network() {
            let l = match backend {
                Backend::Bubblewrap(_) => egress::Listener::served_elsewhere(socket.to_path_buf()),
                Backend::Seatbelt => {
                    egress::loopback_splice(egress::Target::Socket(socket.to_path_buf()))?
                }
            };
            self.forwards.push((port, Arc::new(l)));
        }
        Ok(self)
    }

    /// The loopback port inside the sandbox that reaches the port [`Profile::forward`] or
    /// [`Profile::forward_socket`] was given: the same one under bubblewrap, a fresh one under
    /// Seatbelt, whose sandboxes share the host's loopback. `None` when it is not forwarded.
    pub fn forwarded_port(&self, port: u16) -> Option<u16> {
        self.forwards
            .iter()
            .find(|(p, _)| *p == port)
            .map(|(p, l)| l.port().unwrap_or(*p))
    }

    fn private_network(&self) -> bool {
        self.policy.network() != SandboxNetwork::Host
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
        // A mount point cannot be renamed, but the dirs above it can: `mv repo r && git init repo`
        // would leave a protected `.git` behind under another name. Each dir between a writable
        // bind and a mount inside it is bound onto itself, as Seatbelt pins them with a rule.
        let by_dest: std::collections::HashMap<&Path, &Mount> =
            out.iter().map(|m| (m.dest(), m)).collect();
        let mut pins: Vec<PathBuf> = vec![];
        for m in &out {
            if matches!(
                m,
                Mount::HomeTmpfs(_) | Mount::HomeEntry(..) | Mount::Link { .. }
            ) {
                continue;
            }
            let mut below = vec![];
            for a in m.dest().ancestors().skip(1) {
                if let Some(enclosing) = by_dest.get(a) {
                    if matches!(enclosing, Mount::Writable { src, dest } if src == dest) {
                        pins.append(&mut below);
                    }
                    break;
                }
                if pins.iter().any(|p| p == a) {
                    pins.append(&mut below);
                    break;
                }
                below.push(a.to_path_buf());
            }
        }
        pins.sort();
        pins.dedup();
        out.extend(
            pins.into_iter()
                .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
                .map(|p| Mount::Writable {
                    src: p.clone(),
                    dest: p,
                }),
        );
        out
    }

    /// Everything bubblewrap needs before the program: mounts, namespaces, and the directory
    /// the program starts in. Creates placeholders for missing protected paths.
    pub fn args(&self, chdir: &Path) -> Vec<OsString> {
        self.args_with(chdir, self.egress.as_deref(), &self.forwards)
    }

    fn args_with(
        &self,
        chdir: &Path,
        egress: Option<&egress::Listener>,
        forwards: &[(u16, Arc<egress::Listener>)],
    ) -> Vec<OsString> {
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
        for (src, dest) in self.decoys.iter().flat_map(|d| d.binds()) {
            push(&["--ro-bind".as_ref(), src.as_os_str(), dest.as_os_str()]);
        }
        for v in UNSET_VARS {
            push(&["--unsetenv".as_ref(), v.as_ref()]);
        }
        for (k, v) in &self.env {
            push(&["--setenv".as_ref(), k.as_ref(), v.as_ref()]);
        }
        push(&["--unshare-pid".as_ref(), "--unshare-ipc".as_ref()]);
        if self.private_network() {
            // A namespace of its own: loopback, abstract Unix sockets, the LAN, and host services
            // are out of reach, and only the sockets bound below lead out.
            push(&["--unshare-net".as_ref()]);
            let sockets = egress.into_iter().chain(forwards.iter().map(|(_, l)| &**l));
            for l in sockets {
                push(&[
                    "--bind".as_ref(),
                    l.socket().as_os_str(),
                    l.socket().as_os_str(),
                ]);
            }
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
                let helper =
                    helper().ok_or("The sandbox helper (the `ostra` binary) is missing.")?;
                let egress = match &self.egress {
                    Some(l) => Some(l.clone()),
                    None if self.policy.needs_proxy() => Some(Arc::new(
                        egress::proxy(self.policy.clone(), &egress_dir(), Arc::new(log_decision))
                            .map_err(|e| format!("Cannot start the sandbox's egress proxy: {e}"))?,
                    )),
                    None => None,
                };
                let mut forwards = self.forwards.clone();
                if self.private_network() {
                    for port in self.policy.loopback_ports() {
                        if forwards.iter().any(|(p, _)| *p == port) {
                            continue;
                        }
                        let target =
                            std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
                        let l = egress::splice(&egress_dir(), target).map_err(|e| {
                            format!("Cannot forward 127.0.0.1:{port} into the sandbox: {e}")
                        })?;
                        forwards.push((port, Arc::new(l)));
                    }
                }
                let mut a = self.args_with(chdir, egress.as_deref(), &forwards);
                a.push("--".into());
                a.push(helper.into_os_string());
                a.push("sandbox-init".into());
                let mut listeners: Vec<Arc<egress::Listener>> = vec![];
                if self.private_network() {
                    if let Some(l) = &egress {
                        a.push("--proxy".into());
                        a.push(l.socket().into());
                    }
                    for (port, l) in &forwards {
                        let mut f = OsString::from(format!("{port}="));
                        f.push(l.socket());
                        a.push("--forward".into());
                        a.push(f);
                        listeners.push(l.clone());
                    }
                }
                listeners.extend(egress);
                a.push("--".into());
                a.push(program.to_os_string());
                a.extend(args.into_iter().map(Into::into));
                Ok(SandboxedCommand {
                    program: bin.clone(),
                    args: a,
                    env: vec![],
                    env_remove: vec![],
                    cwd: chdir.to_path_buf(),
                    members: Some(Members::new(listeners, self.decoys.clone())),
                })
            }
            Backend::Seatbelt => {
                let egress = match &self.egress {
                    Some(l) => Some(l.clone()),
                    None if self.policy.needs_proxy() => Some(Arc::new(
                        new_proxy(backend, self.policy.clone(), Arc::new(log_decision))
                            .map_err(|e| format!("Cannot start the sandbox's egress proxy: {e}"))?,
                    )),
                    None => None,
                };
                let proxy_url = egress
                    .as_ref()
                    .and_then(|l| l.proxy_url().map(str::to_string));
                let mut listeners: Vec<Arc<egress::Listener>> =
                    self.forwards.iter().map(|(_, l)| l.clone()).collect();
                listeners.extend(egress);
                let members = Members::new(listeners, self.decoys.clone());
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
                if let Some(url) = &proxy_url {
                    env.extend(proxy_env(url));
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
        if self.private_network() {
            // The host's loopback is every sandbox's. Each may listen on it; it connects to every
            // port when the workspace opens the loopback, else to listed ports, and never to a
            // blocked port or Ostra's server. Its own proxy and forwards come last, so they win.
            out.push_str(SEATBELT_LOOPBACK_LISTEN);
            let allow = |out: &mut String, p: &str| {
                out.push_str(&format!(
                    "(allow network-outbound (remote ip \"localhost:{p}\"))\n"
                ))
            };
            if self.policy.shared_loopback() {
                allow(&mut out, "*");
            }
            for p in self.policy.loopback_ports() {
                allow(&mut out, &p.to_string());
            }
            let mut blocked: Vec<u16> = self.policy.blocked_ports().to_vec();
            blocked.extend(SERVER_PORT.get());
            blocked.sort_unstable();
            blocked.dedup();
            for p in blocked {
                out.push_str(&format!(
                    "(deny network-outbound (remote ip \"localhost:{p}\"))\n"
                ));
            }
            let mut own: Vec<u16> = members.listeners.iter().filter_map(|l| l.port()).collect();
            own.sort_unstable();
            for p in own {
                allow(&mut out, &p.to_string());
            }
        } else {
            out.push_str(SEATBELT_HOST_NETWORK);
        }
        // Rule P3: a decoy's read is refused with a message the system log reports; `stat` stays
        // silent, because only `file-read-data` carries it.
        for (tag, path) in self.decoys.iter().flat_map(|d| d.tags()) {
            out.push_str(&format!(
                "(deny file-read-data (literal {}) (with message {}))\n",
                sbpl_str(path)?,
                sbpl_str(tag)?
            ));
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
/// npm, go, pip, and clang through xcrun, and the four harness CLIs. The mach services are the
/// ones those tools need; the keychain, pasteboard, LaunchServices, Apple Events, the window
/// server, and TCC stay denied, because each reaches something outside the sandbox.
/// `(deny default)` also denies `lsopen`, `appleevent-send`, and `user-preference-write`.
/// Terminals are denied, so nothing reads keystrokes from, writes to, or injects input into
/// another terminal of the user.
///
/// Another process's arguments and startup environment (`KERN_PROCARGS2`) are readable when
/// either `sysctl-read` covers `kern.procargs2` or `process-info-pidinfo` allows it, and
/// `(deny default)` does not cover the second, so both are closed: the sysctls are a measured
/// list, and `process-info*` is denied before the same-sandbox allow (measured on macOS 26).
const SEATBELT_BASE: &str = r##"(version 1)
(deny default)
(allow process-exec process-fork)
(allow signal (target same-sandbox))
(deny process-info*)
(allow process-info* (target same-sandbox))
(allow sysctl-read (sysctl-name-prefix "hw.") (sysctl-name-prefix "machdep.cpu.") (sysctl-name "kern.argmax" "kern.bootargs" "kern.hostname" "kern.iossupportversion" "kern.maxfilesperproc" "kern.ngroups" "kern.osproductversion" "kern.osrelease" "kern.ostype" "kern.osvariant_status" "kern.osversion" "kern.version" "kern.willshutdown" "net.routetable.0.0.3.0" "security.mac.lockdown_mode_state"))
(allow file-read*)
(deny file-read* file-write* file-ioctl (regex #"^/dev/tty"))
(allow file-write-data (literal "/dev/stdout") (literal "/dev/stderr") (regex #"^/dev/fd/[0-9]+$"))
(allow file-write* file-ioctl (literal "/dev/null") (literal "/dev/zero") (literal "/dev/dtracehelper"))
(allow ipc-posix-sem ipc-posix-shm*)
(allow user-preference-read)
(allow mach-lookup (global-name "com.apple.system.opendirectoryd.libinfo" "com.apple.system.opendirectoryd.membership" "com.apple.system.notification_center" "com.apple.system.logger" "com.apple.logd" "com.apple.diagnosticd" "com.apple.trustd.agent" "com.apple.SystemConfiguration.configd" "com.apple.cfprefsd.agent" "com.apple.bsd.dirhelper"))
"##;

/// Under `host`: outbound IP, a listener on loopback for dev servers and tests, and the DNS
/// resolver's socket. Other Unix sockets (the SSH agent, Docker, password managers, IDEs) stay
/// denied.
const SEATBELT_HOST_NETWORK: &str = r##"(allow network-outbound (remote ip))
(allow network-inbound (local ip "localhost:*"))
(allow network-bind (local ip "localhost:*"))
(allow network-outbound (literal "/private/var/run/mDNSResponder"))
"##;

/// Under every other choice: a listener on loopback, and no DNS resolver, because the proxy
/// resolves names and a lookup would carry data out in the name itself. `localhost` still
/// resolves, from `/etc/hosts`.
const SEATBELT_LOOPBACK_LISTEN: &str = r##"(allow network-inbound (local ip "localhost:*"))
(allow network-bind (local ip "localhost:*"))
"##;

/// Starts one execution's egress proxy where `backend`'s sandbox reaches it.
fn new_proxy(
    backend: &Backend,
    policy: egress::Policy,
    on: egress::OnDecision,
) -> std::io::Result<egress::Listener> {
    match backend {
        Backend::Bubblewrap(_) => egress::proxy(policy, &egress_dir(), on),
        Backend::Seatbelt => egress::loopback_proxy(policy, on),
    }
}

static SERVER_PORT: OnceLock<u16> = OnceLock::new();

/// Names Ostra's own TCP port, which no Seatbelt sandbox connects to even where its workspace
/// opens the loopback, because the port serves the console's API.
pub fn set_server_port(port: u16) {
    let _ = SERVER_PORT.set(port);
}

/// What points a command's HTTP clients at the egress proxy at `url`. Loopback goes direct.
pub fn proxy_env(url: &str) -> Vec<(String, String)> {
    let url = url.to_string();
    let mut out = vec![];
    for k in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
        out.push((k.to_string(), url.clone()));
        out.push((k.to_ascii_lowercase(), url.clone()));
    }
    for k in ["NO_PROXY", "no_proxy"] {
        out.push((k.to_string(), "localhost,127.0.0.1,::1".to_string()));
    }
    out.push(("NODE_USE_ENV_PROXY".into(), "1".into()));
    out
}

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
    paths::private_file_options()
        .write(true)
        .create_new(true)
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

/// Undoes what an agent can write inside a repository's `.git` that makes git load another
/// repository's config, and so run its `core.fsmonitor`, `credential.helper`, or hooks: a
/// `commondir` in a repository's own git dir (which git never writes there) is removed, and a
/// linked worktree's `commondir` that no longer names `../..` is restored. Takes the repos
/// [`git_repos`] found, the ones [`Profile::protect_git`] covers. Returns the paths it changed.
pub fn repair_git_dirs(repos: &[PathBuf]) -> Vec<PathBuf> {
    let mut changed = vec![];
    for repo in repos {
        let Some(git) = GitDir::of(repo) else {
            continue;
        };
        if git.linked {
            continue;
        }
        for dir in git_dirs(&git.dir) {
            let own = dir.join("commondir");
            if let Ok(meta) = std::fs::symlink_metadata(&own) {
                let removed = if meta.is_dir() {
                    std::fs::remove_dir_all(&own)
                } else {
                    std::fs::remove_file(&own)
                };
                if removed.is_ok() {
                    changed.push(own);
                }
            }
            for w in read_dirs(&dir.join("worktrees")) {
                let common = w.join("commondir");
                let Ok(meta) = std::fs::symlink_metadata(&common) else {
                    continue;
                };
                let intact = meta.is_file()
                    && std::fs::read_to_string(&common).is_ok_and(|t| t.trim() == "../..");
                if intact {
                    continue;
                }
                if meta.is_dir() {
                    let _ = std::fs::remove_dir_all(&common);
                } else {
                    let _ = std::fs::remove_file(&common);
                }
                if std::fs::write(&common, "../..\n").is_ok() {
                    changed.push(common);
                }
            }
        }
    }
    changed
}

/// The correction an agent gets for each path [`repair_git_dirs`] changed.
pub fn git_repair_note(path: &Path) -> String {
    format!(
        "Leave `{}` to git: Ostra undid it, because a `commondir` makes git load another repository's config and run the programs it names.",
        path.display()
    )
}

/// Dirs [`git_repos`] reads before it stops, which bounds the walk in a large tree.
const GIT_WALK_DIRS: usize = 20_000;
/// Repos [`git_repos`] returns at most, which bounds the mounts they add.
const GIT_WALK_REPOS: usize = 128;

/// Every git work tree under `roots` at any depth, shallowest first: a dir holding a `.git` dir
/// or a `.git` file. Symlinks, `node_modules`, and dirs tagged as caches (`CACHEDIR.TAG`, as
/// Cargo tags `target/`) are not entered, and the walk stops after [`GIT_WALK_DIRS`] dirs or
/// [`GIT_WALK_REPOS`] repos.
pub fn git_repos(roots: &[&Path]) -> Vec<PathBuf> {
    let mut queue: std::collections::VecDeque<PathBuf> = roots
        .iter()
        .filter(|r| !r.as_os_str().is_empty())
        .filter_map(|r| real(r))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    while let Some(dir) = queue.pop_front() {
        if !seen.insert(dir.clone()) {
            continue;
        }
        if seen.len() > GIT_WALK_DIRS || out.len() >= GIT_WALK_REPOS {
            tracing::warn!(
                "stopped looking for git repositories at {}: repositories below it keep a writable .git/config",
                dir.display()
            );
            break;
        }
        if dir.join("CACHEDIR.TAG").is_file() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut subdirs = vec![];
        for e in rd.flatten() {
            let name = e.file_name();
            if name == ".git" {
                let git = e.path();
                if (git.is_dir() || git.is_file()) && !out.contains(&dir) {
                    out.push(dir.clone());
                }
            } else if name != "node_modules" && e.file_type().is_ok_and(|t| t.is_dir()) {
                subdirs.push(e.path());
            }
        }
        subdirs.sort();
        queue.extend(subdirs);
    }
    out
}

/// A work tree's git dir: its `.git` dir, or the dir its `.git` file names.
struct GitDir {
    dir: PathBuf,
    /// Named by a `.git` file.
    file: bool,
    /// A linked worktree's dir, `<common dir>/worktrees/<name>`, whose config is the main repo's.
    linked: bool,
}

impl GitDir {
    fn of(repo: &Path) -> Option<GitDir> {
        let git = repo.join(".git");
        if git.is_dir() {
            return Some(GitDir {
                dir: git,
                file: false,
                linked: false,
            });
        }
        let mut head = String::new();
        std::io::Read::read_to_string(
            &mut std::io::Read::take(std::fs::File::open(&git).ok()?, 4096),
            &mut head,
        )
        .ok()?;
        let named = head.lines().next()?.strip_prefix("gitdir:")?.trim();
        let dir = real(&repo.join(named))?;
        let linked = dir
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "worktrees");
        dir.is_dir().then_some(GitDir {
            dir,
            file: true,
            linked,
        })
    }
}

/// The dirs directly inside `dir`, none when it is missing.
fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// Where each execution's egress socket lives: inside the data dir, which every sandbox hides.
pub fn egress_dir() -> PathBuf {
    paths::data_dir().join("egress")
}

fn log_decision(d: egress::Decision) {
    match &d.reason {
        None => tracing::info!(host = %d.host, port = d.port, "sandbox egress allowed"),
        Some(r) => {
            tracing::warn!(host = %d.host, port = d.port, local = d.local, "sandbox egress refused: {r}")
        }
    }
}

fn user_id() -> Option<u32> {
    // SAFETY: getuid cannot fail.
    #[cfg(unix)]
    return Some(unsafe { libc::getuid() });
    #[cfg(not(unix))]
    None
}

#[cfg(all(test, unix))]
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
            sandbox_network: None,
            sandbox_allowed_hosts: vec![],
            sandbox_decoys: vec![],
            sandbox_loopback: Default::default(),
            sandbox_blocked_ports: vec![],
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
        let top = crate::paths::canonical(d.path()).unwrap();
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
        assert!(has(&["--unshare-net"]));
        let host = SandboxConfig {
            network: SandboxNetwork::Host,
            ..Default::default()
        };
        let args = Profile::for_execution(&ctx(&root), &host, &home).args(&root);
        assert!(!args.iter().any(|a| a == "--unshare-net"));
        let off = SandboxConfig {
            network: SandboxNetwork::None,
            ..Default::default()
        };
        let args = Profile::for_execution(&ctx(&root), &off, &home)
            .new_session(false)
            .args(&root);
        assert!(args.iter().any(|a| a == "--unshare-net"));
        assert!(!args.iter().any(|a| a == "--new-session"));
    }

    #[test]
    fn planted_commondir_files_are_undone() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        let git = repo.join(".git");
        std::fs::create_dir_all(git.join("worktrees/wt")).unwrap();
        std::fs::create_dir_all(git.join("worktrees/ok")).unwrap();
        std::fs::create_dir_all(git.join("modules/sub")).unwrap();
        std::fs::write(git.join("config"), "").unwrap();
        std::fs::write(git.join("modules/sub/config"), "").unwrap();
        std::fs::write(git.join("commondir"), "/tmp/evil\n").unwrap();
        std::fs::write(git.join("modules/sub/commondir"), "/tmp/evil\n").unwrap();
        std::fs::write(git.join("worktrees/wt/commondir"), "/tmp/evil\n").unwrap();
        std::fs::write(git.join("worktrees/ok/commondir"), "../..\n").unwrap();
        let mut changed = repair_git_dirs(&git_repos(&[&repo]));
        changed.sort();
        let real = crate::paths::canonical(&git).unwrap();
        assert_eq!(
            changed,
            vec![
                real.join("commondir"),
                real.join("modules/sub/commondir"),
                real.join("worktrees/wt/commondir"),
            ]
        );
        assert!(!git.join("commondir").exists());
        assert_eq!(
            std::fs::read_to_string(git.join("worktrees/wt/commondir")).unwrap(),
            "../..\n"
        );
        assert!(
            repair_git_dirs(&git_repos(&[&repo])).is_empty(),
            "a repaired repo stays quiet"
        );
    }

    #[test]
    fn repos_are_found_at_any_depth_but_not_in_build_output() {
        let d = tempfile::tempdir().unwrap();
        let root = crate::paths::canonical(d.path()).unwrap();
        for dir in [
            ".git",
            "a/.git",
            "a/b/c/.git",
            "node_modules/x/.git",
            "target/y/.git",
            "a/.git/modules/m",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("target/CACHEDIR.TAG"), "").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/.git"), "gitdir: ../a/.git/modules/m\n").unwrap();
        std::os::unix::fs::symlink(root.join("a"), root.join("link")).unwrap();
        assert_eq!(
            git_repos(&[&root, &root.join("a")]),
            vec![
                root.clone(),
                root.join("a"),
                root.join("sub"),
                root.join("a/b/c")
            ]
        );
        let sub = GitDir::of(&root.join("sub")).unwrap();
        assert_eq!(sub.dir, root.join("a/.git/modules/m"));
        assert!(sub.file && !sub.linked);
    }

    #[test]
    fn nested_repos_and_git_files_are_protected_and_pinned() {
        let (_d, root, home) = layout();
        let repo = root.join("repo");
        let deep = repo.join("vendor/lib/dep");
        std::fs::create_dir_all(&deep).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(ok.status.success(), "{args:?}: {ok:?}");
        };
        git(&deep, &["init", "-q"]);
        git(
            &repo,
            &[
                "-c",
                "user.email=a@b",
                "-c",
                "user.name=a",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "i",
            ],
        );
        git(&repo, &["worktree", "add", "-q", "wt"]);
        let gitfile = std::fs::read_to_string(repo.join("wt/.git")).unwrap();
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
        let script = format!(
            r#"
            git -C {deep} config core.fsmonitor x 2>/dev/null || echo deep-config-read-only
            echo 'gitdir: /tmp' > wt/.git 2>/dev/null || echo git-file-read-only
            mv {deep} {deep}-moved 2>/dev/null || echo deep-repo-not-renamed
            mv vendor vendor-moved 2>/dev/null || echo ancestor-not-renamed
            mv wt wt-moved 2>/dev/null || echo worktree-not-renamed
            echo a > vendor/f && echo other-writes-stay
            "#,
            deep = deep.display(),
        );
        let Some(out) = run_in(&p, &repo, &script) else {
            return;
        };
        for want in [
            "deep-config-read-only",
            "git-file-read-only",
            "deep-repo-not-renamed",
            "ancestor-not-renamed",
            "worktree-not-renamed",
            "other-writes-stay",
        ] {
            assert!(out.contains(want), "missing {want}:\n{out}");
        }
        assert!(deep.join(".git/config").is_file());
        assert_eq!(
            std::fs::read_to_string(repo.join("wt/.git")).unwrap(),
            gitfile
        );
    }

    #[test]
    fn names_on_disk_use_a_stable_hash() {
        assert_ne!(
            stable_hex("", Path::new("/w")),
            stable_hex("agents", Path::new("/w"))
        );
        // SHA-256 of one NUL byte: a change here moves every user's caches and markers.
        assert_eq!(stable_hex("", Path::new("")), "6e340b9cffb37a98");
    }

    #[test]
    fn each_session_gets_its_own_caches_apart_from_programs() {
        let (_d, root, home) = layout();
        let cargo = |p: Profile| {
            strings(p.args(&root))
                .windows(3)
                .find(|w| w[0] == "--setenv" && w[1] == "CARGO_HOME")
                .map(|w| PathBuf::from(&w[2]))
                .unwrap()
        };
        let cfg = SandboxConfig::default();
        let in_session = |id: &str| ExecContext {
            session_id: Some(id.into()),
            ..ctx(&root)
        };
        let s1 = cargo(Profile::for_execution(&in_session("s_1"), &cfg, &home));
        let s2 = cargo(Profile::for_execution(&in_session("s_2"), &cfg, &home));
        assert_eq!(s1, session_cache(&home, &"s_1".into()).join("cargo"));
        assert_ne!(s1, s2);
        let outside = cargo(Profile::for_execution(&ctx(&root), &cfg, &home));
        let program = cargo(Profile::for_program(&[&root], &cfg, &home));
        assert_ne!(outside, program, "agents never write a program's cache");
        assert!(![&s1, &s2].contains(&&outside));
    }

    #[test]
    fn opening_a_decoy_is_reported_and_listing_is_not() {
        let (_d, root, home) = layout();
        std::fs::write(
            home.join(".git-credentials"),
            "https://u:real@example.invalid\n",
        )
        .unwrap();
        std::fs::write(home.join(".ssh/id_rsa"), "real key\n").unwrap();
        let Some(b) = backend().filter(|b| matches!(b, Backend::Bubblewrap(_))) else {
            return;
        };
        let seen = Arc::new(std::sync::Mutex::new(vec![]));
        let s = seen.clone();
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
            .watch_decoys(b, &home, &[], Arc::new(move |p| s.lock().unwrap().push(p)))
            .unwrap();
        let script = format!(
            "ls -la {ssh} >/dev/null; stat {ssh}/id_ed25519 >/dev/null && echo stat-ok
            cat {ssh}/id_rsa | head -1; cat {ssh}/id_rsa {creds} | grep -c real",
            ssh = home.join(".ssh").display(),
            creds = home.join(".git-credentials").display(),
        );
        let out = run_in(&p, &root.join("repo"), &script).unwrap();
        assert!(out.contains("stat-ok"), "{out}");
        assert!(out.contains("-----BEGIN OPENSSH PRIVATE KEY-----"), "{out}");
        assert!(
            out.contains("\n0"),
            "the real credentials stay hidden: {out}"
        );
        let start = std::time::Instant::now();
        while seen.lock().unwrap().len() < 2 && start.elapsed() < std::time::Duration::from_secs(5)
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut got = seen.lock().unwrap().clone();
        got.sort();
        assert_eq!(
            got,
            vec![home.join(".git-credentials"), home.join(".ssh/id_rsa")]
        );
    }

    #[test]
    fn workspace_decoys_cover_files_and_fill_hidden_dirs_only() {
        let (_d, root, home) = layout();
        std::fs::create_dir_all(home.join(".aws")).unwrap();
        std::fs::create_dir_all(home.join(".config/app")).unwrap();
        std::fs::write(home.join(".config/app/token"), "real\n").unwrap();
        std::fs::create_dir_all(home.join(".visible")).unwrap();
        let Some(b) = backend().filter(|b| matches!(b, Backend::Bubblewrap(_))) else {
            return;
        };
        let seen = Arc::new(std::sync::Mutex::new(vec![]));
        let s = seen.clone();
        let extra = [
            "~/.aws/sso/cache/x.json".to_string(),
            "~/.config/app/token".to_string(),
            "~/.visible/missing".to_string(),
        ];
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
            .watch_decoys(
                b,
                &home,
                &extra,
                Arc::new(move |p| s.lock().unwrap().push(p)),
            )
            .unwrap();
        let script = format!(
            "cat {aws} {token} | grep -c real; cat {missing} 2>/dev/null || echo skipped",
            aws = home.join(".aws/sso/cache/x.json").display(),
            token = home.join(".config/app/token").display(),
            missing = home.join(".visible/missing").display(),
        );
        let out = run_in(&p, &root.join("repo"), &script).unwrap();
        assert!(out.starts_with("0\n"), "the real token is covered: {out}");
        assert!(out.contains("skipped"), "{out}");
        assert!(
            !home.join(".visible/missing").exists(),
            "the host is never written"
        );
        let start = std::time::Instant::now();
        while seen.lock().unwrap().len() < 2 && start.elapsed() < std::time::Duration::from_secs(5)
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut got = seen.lock().unwrap().clone();
        got.sort();
        assert_eq!(
            got,
            vec![
                home.join(".aws/sso/cache/x.json"),
                home.join(".config/app/token")
            ]
        );
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

    #[cfg(target_os = "linux")]
    #[test]
    fn a_private_network_leads_out_only_through_its_own_proxy() {
        let Some(b @ Backend::Bubblewrap(_)) = backend() else {
            eprintln!("bubblewrap unavailable; skipping");
            return;
        };
        let (_d, root, home) = layout();
        let host = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = host.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for mut s in host.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
            }
        });
        let unlisted = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let unlisted_port = unlisted.local_addr().unwrap().port();
        let abs = format!("ostra-test-{}", uuid::Uuid::new_v4().simple());
        let abs_listener = {
            use std::os::linux::net::SocketAddrExt;
            let addr = std::os::unix::net::SocketAddr::from_abstract_name(&abs).unwrap();
            std::os::unix::net::UnixListener::bind_addr(&addr).unwrap()
        };
        let cfg = SandboxConfig {
            allowed_hosts: vec![format!("127.0.0.1:{port}")],
            ..Default::default()
        };
        let seen: Arc<std::sync::Mutex<Vec<egress::Decision>>> = Arc::default();
        let s2 = seen.clone();
        let other = Profile::for_execution(&ctx(&root), &cfg, &home)
            .start_egress(b, Arc::new(|_| {}))
            .unwrap();
        let p = Profile::for_execution(&ctx(&root), &cfg, &home)
            .start_egress(b, Arc::new(move |d| s2.lock().unwrap().push(d)))
            .unwrap();
        let script = format!(
            r#"
            bash -c 'exec 3<>/dev/tcp/127.0.0.1/{unlisted_port}' 2>/dev/null && echo direct-loopback
            echo "listed-forwarded:$(curl -s --max-time 5 http://127.0.0.1:{port}/)"
            python3 -c 'import socket; s=socket.socket(socket.AF_UNIX); s.connect("\0{abs}")' 2>/dev/null && echo abstract-socket
            echo "via-proxy:$(curl -s --max-time 5 --noproxy '' -x "$HTTP_PROXY" http://127.0.0.1:{port}/)"
            echo "refused:$(curl -s -o /dev/null -w '%{{http_code}}' --max-time 5 --noproxy '' -x "$HTTP_PROXY" http://127.0.0.1:1/)"
            ls {dir}
            "#,
            dir = egress_dir().display(),
        );
        let out = run_in(&p, &root, &script).unwrap();
        assert!(!out.contains("direct-loopback"), "{out}");
        assert!(!out.contains("abstract-socket"), "{out}");
        assert!(out.contains("via-proxy:ok"), "{out}");
        assert!(out.contains("listed-forwarded:ok"), "{out}");
        assert!(out.contains("refused:403"), "{out}");
        let own = p.egress.as_ref().unwrap().socket().file_name().unwrap();
        let theirs = other.egress.as_ref().unwrap().socket().file_name().unwrap();
        assert!(out.contains(own.to_str().unwrap()), "{out}");
        assert!(!out.contains(theirs.to_str().unwrap()), "{out}");
        let seen = seen.lock().unwrap().clone();
        assert!(seen.iter().any(|d| d.allowed && d.port == port), "{seen:?}");
        assert!(
            seen.iter().any(|d| !d.allowed && d.local && d.port == 1),
            "{seen:?}"
        );

        let open = SandboxConfig {
            network: SandboxNetwork::Host,
            ..Default::default()
        };
        let p = Profile::for_execution(&ctx(&root), &open, &home);
        let out = run_in(
            &p,
            &root,
            &format!(
                "bash -c 'exec 3<>/dev/tcp/127.0.0.1/{unlisted_port}' && echo direct-loopback\npython3 -c 'import socket; s=socket.socket(socket.AF_UNIX); s.connect(\"\\0{abs}\")' && echo abstract-socket"
            ),
        )
        .unwrap();
        assert!(
            out.contains("direct-loopback") && out.contains("abstract-socket"),
            "{out}"
        );
        drop((abs_listener, unlisted));
    }

    #[test]
    fn a_sandboxed_command_cannot_make_new_user_namespaces() {
        let Some(Backend::Bubblewrap(bwrap)) = backend() else {
            eprintln!("bubblewrap unavailable; skipping");
            return;
        };
        let (_d, root, home) = layout();
        let script = format!(
            r#"
            unshare -U true 2>/dev/null && echo nested-userns
            {bwrap} --ro-bind / / true 2>/dev/null && echo nested-bwrap
            python3 -c 'import os; os.unshare(os.CLONE_NEWUSER)' 2>&1 | grep -q 'not permitted' && echo userns-eperm
            python3 -c 'import fcntl, termios; fcntl.ioctl(0, termios.TIOCSTI, b"x")' 2>&1 | grep -q 'not permitted' && echo tiocsti-eperm
            python3 -c 'import os, threading; t = threading.Thread(target=print); t.start(); t.join(); os._exit(0) if os.fork() == 0 else os.wait()' && echo fork-ok
            echo "pipe:$(echo a | tr a b)"
            "#,
            bwrap = bwrap.display(),
        );
        for network in [SandboxNetwork::Allowlist, SandboxNetwork::Host] {
            let cfg = SandboxConfig {
                network,
                ..Default::default()
            };
            let p = Profile::for_execution(&ctx(&root), &cfg, &home);
            let out = run_in(&p, &root, &script).unwrap();
            assert!(!out.contains("nested-userns"), "{network:?}: {out}");
            assert!(!out.contains("nested-bwrap"), "{network:?}: {out}");
            for want in ["userns-eperm", "tiocsti-eperm", "fork-ok", "pipe:b"] {
                assert!(out.contains(want), "{network:?}: missing {want}:\n{out}");
            }
        }
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
            listeners: vec![],
            decoys: None,
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
        let config = at(&format!(
            "(deny file-write* network-bind (literal \"{git}/config\"))"
        ));
        assert!(writable_repo < config);
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
        assert!(text.contains("dev.ostra.sandbox.test"));
        assert!(!text.contains("(param"));
        // Under `allowlist` the loopback is open by default, the rest goes through the proxy, and
        // names do not resolve.
        assert!(!text.contains("(remote ip)"), "{text}");
        assert!(!text.contains("mDNSResponder"), "{text}");
        assert!(text.contains("(allow network-bind (local ip \"localhost:*\"))"));
        assert!(text.contains("(allow network-outbound (remote ip \"localhost:*\"))"));
        let blocked = SandboxConfig {
            blocked_ports: vec![5432],
            ..Default::default()
        };
        let text = policy(&Profile::for_execution(&ctx(&root), &blocked, &home));
        let open = text
            .find("(allow network-outbound (remote ip \"localhost:*\"))")
            .unwrap();
        let deny = text
            .find("(deny network-outbound (remote ip \"localhost:5432\"))")
            .unwrap();
        assert!(open < deny, "{text}");
        let listed = SandboxConfig {
            allowed_hosts: vec![
                "127.0.0.1:8317".into(),
                "127.0.0.1:5432".into(),
                "pkg.example".into(),
            ],
            loopback: crate::config::LoopbackAccess::Listed,
            blocked_ports: vec![5432],
            ..Default::default()
        };
        let text = policy(&Profile::for_execution(&ctx(&root), &listed, &home));
        assert!(text.contains("(allow network-outbound (remote ip \"localhost:8317\"))"));
        assert!(!text.contains("(allow network-outbound (remote ip \"localhost:5432\"))"));
        assert_eq!(
            text.matches("(allow network-outbound (remote ip").count(),
            1,
            "{text}"
        );
        let off = SandboxConfig {
            network: SandboxNetwork::None,
            ..Default::default()
        };
        let text = policy(&Profile::for_execution(&ctx(&root), &off, &home).tty(true));
        assert!(!text.contains("(remote ip"));
        assert!(text.contains("(literal (param \"TTY\"))"));
        let host = SandboxConfig {
            network: SandboxNetwork::Host,
            ..Default::default()
        };
        let text = policy(&Profile::for_execution(&ctx(&root), &host, &home));
        assert!(text.contains("(allow network-outbound (remote ip))"));
        assert!(text.contains("mDNSResponder"));
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

    /// A tiny HTTP server on loopback that answers every request with `body`.
    fn serve(body: &'static str) -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for mut s in l.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            }
        });
        port
    }

    /// macOS shares one loopback, so each Seatbelt policy names the ports it may connect to: its
    /// own proxy, its forwards, and listed loopback hosts. Nothing resolves names but the proxy.
    #[test]
    fn seatbelt_leads_out_only_through_its_own_proxy_and_ports() {
        if !seatbelt() {
            eprintln!("seatbelt unavailable; skipping");
            return;
        }
        let (_d, root, home) = layout();
        let listed = serve("listed");
        let unlisted = serve("unlisted");
        let bridge = serve("bridge");
        let cfg = SandboxConfig {
            allowed_hosts: vec![format!("127.0.0.1:{listed}")],
            loopback: crate::config::LoopbackAccess::Listed,
            ..Default::default()
        };
        let seen: Arc<std::sync::Mutex<Vec<egress::Decision>>> = Arc::default();
        let s2 = seen.clone();
        let b = backend().unwrap();
        let other = Profile::for_execution(&ctx(&root), &cfg, &home)
            .start_egress(b, Arc::new(|_| {}))
            .unwrap();
        let other_port = other.egress.as_ref().unwrap().port().unwrap();
        let bridge_addr: std::net::SocketAddr = ([127, 0, 0, 1], bridge).into();
        let p = Profile::for_execution(&ctx(&root), &cfg, &home)
            .start_egress(b, Arc::new(move |d| s2.lock().unwrap().push(d)))
            .unwrap()
            .forward(b, bridge_addr)
            .unwrap();
        let inside = p.forwarded_port(bridge).unwrap();
        assert_ne!(inside, bridge);
        let script = format!(
            r#"
            get() {{ curl -s --max-time 5 "$@"; }}
            echo "listed:$(get http://127.0.0.1:{listed}/)"
            get http://127.0.0.1:{unlisted}/ && echo direct-unlisted
            get http://127.0.0.1:{bridge}/ && echo direct-bridge
            echo "forwarded:$(get http://127.0.0.1:{inside}/)"
            echo "via-proxy:$(get --noproxy '' -x "$HTTP_PROXY" http://127.0.0.1:{listed}/)"
            echo "refused:$(get -o /dev/null -w '%{{http_code}}' --noproxy '' -x "$HTTP_PROXY" http://127.0.0.1:1/)"
            get --noproxy '' -x http://127.0.0.1:{other_port} http://127.0.0.1:{listed}/ && echo other-proxy
            get --noproxy '*' http://1.1.1.1/ >/dev/null && echo direct-public
            python3 -c 'import socket; socket.gethostbyname("example.com"); print("resolved")' 2>/dev/null
            python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(); print("listen-ok")'
            echo "$HTTPS_PROXY"
            "#
        );
        let out = run_in(&p, &root, &script).unwrap();
        for bad in [
            "direct-unlisted",
            "direct-bridge",
            "other-proxy",
            "direct-public",
            "resolved",
        ] {
            assert!(!out.contains(bad), "{bad}:\n{out}");
        }
        for want in [
            "listed:listed",
            "forwarded:bridge",
            "via-proxy:listed",
            "refused:403",
            "listen-ok",
        ] {
            assert!(out.contains(want), "missing {want}:\n{out}");
        }
        let own = p.egress.as_ref().unwrap().proxy_url().unwrap().to_string();
        assert!(out.contains(&own), "{out}");
        let seen = seen.lock().unwrap().clone();
        assert!(
            seen.iter().any(|d| !d.allowed && d.local && d.port == 1),
            "{seen:?}"
        );

        // The default opens the loopback: the command reaches its own server and unlisted
        // ports, but not a blocked port, and another execution's proxy wants its credential.
        let blocked = serve("blocked");
        let open = SandboxConfig {
            blocked_ports: vec![blocked],
            ..Default::default()
        };
        let p = Profile::for_execution(&ctx(&root), &open, &home)
            .start_egress(b, Arc::new(|_| {}))
            .unwrap();
        let script = format!(
            r#"
            get() {{ curl -s --max-time 5 "$@"; }}
            echo "unlisted:$(get http://127.0.0.1:{unlisted}/)"
            get http://127.0.0.1:{blocked}/ && echo direct-blocked
            echo "other-proxy:$(get -o /dev/null -w '%{{http_code}}' --noproxy '' -x http://127.0.0.1:{other_port} http://127.0.0.1:{listed}/)"
            echo "own-proxy:$(get -o /dev/null -w '%{{http_code}}' https://index.crates.io/config.json)"
            python3 -c 'import socket, threading
s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen()
threading.Thread(target=lambda: s.accept()[0].sendall(b"mine"), daemon=True).start()
c = socket.create_connection(s.getsockname()); print("own-server:" + c.recv(4).decode())'
            "#
        );
        let out = run_in(&p, &root, &script).unwrap();
        assert!(!out.contains("direct-blocked"), "{out}");
        for want in [
            "unlisted:unlisted",
            "other-proxy:407",
            "own-proxy:200",
            "own-server:mine",
        ] {
            assert!(out.contains(want), "missing {want}:\n{out}");
        }

        let host = SandboxConfig {
            network: SandboxNetwork::Host,
            ..Default::default()
        };
        let p = Profile::for_execution(&ctx(&root), &host, &home);
        let out = run_in(
            &p,
            &root,
            &format!("curl -s --max-time 5 http://127.0.0.1:{unlisted}/"),
        )
        .unwrap();
        assert!(out.contains("unlisted"), "{out}");
    }

    /// Another process's arguments and startup environment are not readable, and the
    /// sandbox's own are.
    #[test]
    fn seatbelt_hides_other_processes_startup_environment() {
        if !seatbelt() {
            eprintln!("seatbelt unavailable; skipping");
            return;
        }
        let (_d, root, home) = layout();
        let secret = format!("s{}", uuid::Uuid::new_v4().simple());
        let mut victim = std::process::Command::new("sleep")
            .arg("30")
            .env("OSTRA_TEST_SECRET", &secret)
            .spawn()
            .unwrap();
        let read = |pid: &str| {
            format!(
                r#"python3 -c 'import ctypes
libc = ctypes.CDLL(None, use_errno=True)
mib = (ctypes.c_int * 3)(1, 49, {pid}); n = ctypes.c_size_t(0)
if libc.sysctl(mib, 3, None, ctypes.byref(n), None, 0): print("refused")
else:
    b = ctypes.create_string_buffer(n.value); libc.sysctl(mib, 3, b, ctypes.byref(n), None, 0)
    print("read:" + ("secret" if b"{secret}" in b.raw else "other"))'"#
            )
        };
        let script = format!(
            "echo \"outside:$({})\"\nOSTRA_TEST_SECRET={secret} sleep 5 & C=$!\necho \"own:$({})\"; kill $C",
            read(&victim.id().to_string()),
            read("'\"$C\"'"),
        );
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
        let out = run_in(&p, &root.join("repo"), &script).unwrap();
        let _ = victim.kill();
        let _ = victim.wait();
        assert!(out.contains("outside:refused"), "{out}");
        assert!(out.contains("own:read:secret"), "{out}");
    }

    /// Rule P3 under Seatbelt: an existing credential file is refused and its read reported
    /// through the system log; `stat` and a listing are not reports, and a missing file is none.
    #[test]
    fn seatbelt_reports_reads_of_existing_decoys() {
        if !seatbelt() {
            eprintln!("seatbelt unavailable; skipping");
            return;
        }
        if !crate::decoy::supported(&Backend::Seatbelt) {
            eprintln!("the system log is not readable by this user; skipping");
            return;
        }
        let (_d, root, home) = layout();
        std::fs::write(home.join(".ssh/id_rsa"), "REAL KEY").unwrap();
        std::fs::write(home.join(".vault-token"), "REAL TOKEN").unwrap();
        let seen: Arc<std::sync::Mutex<Vec<PathBuf>>> = Arc::default();
        let s2 = seen.clone();
        let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
            .watch_decoys(
                &Backend::Seatbelt,
                &home,
                &["~/.aws/credentials".into()],
                Arc::new(move |p| s2.lock().unwrap().push(p)),
            )
            .unwrap();
        let tags = p.decoys.as_ref().unwrap().tags().to_vec();
        assert_eq!(tags.len(), 2, "existing files only: {tags:?}");
        let script = format!(
            "ls {h}/.ssh; stat {h}/.vault-token >/dev/null; cat {h}/.aws/credentials; cat {h}/.vault-token; cat {h}/.ssh/id_rsa; echo done",
            h = home.display()
        );
        let out = run_in(&p, &root.join("repo"), &script).unwrap();
        assert!(!out.contains("REAL"), "{out}");
        assert!(out.contains("done"), "{out}");
        let start = std::time::Instant::now();
        while seen.lock().unwrap().len() < 2 && start.elapsed() < std::time::Duration::from_secs(10)
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut got = seen.lock().unwrap().clone();
        got.sort();
        assert_eq!(
            got,
            vec![home.join(".ssh/id_rsa"), home.join(".vault-token")],
            "{out}"
        );
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
