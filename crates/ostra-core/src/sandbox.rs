//! Bubblewrap profiles for agent commands. The policy layer judges each tool call by the paths it
//! names, which a shell can hide; the sandbox is what the kernel enforces underneath it: the host
//! is read-only, only the execution's own roots are writable, and Ostra's data, the user's
//! credentials, and the session bus (which reaches the OS keychain) are not visible at all.

use crate::config::{SandboxConfig, SandboxMode};
use crate::exec::ExecContext;
use crate::paths;
use std::ffi::OsString;
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
];

/// Paths inside a `.git` dir that choose programs git runs. A trailing `/` marks a dir.
const GIT_PROTECTED: &[&str] = &["config", "hooks/", "info/"];

/// Variables that point a command at the desktop session, the SSH agent, or the session bus.
const UNSET_VARS: &[&str] = &[
    "SSH_AUTH_SOCK",
    "DBUS_SESSION_BUS_ADDRESS",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
];

/// The sandbox's own tool caches, one set per workspace under `~/.cache/ostra/sandbox/` (or
/// `$OSTRA_SANDBOX_CACHE`). The
/// user's `~/.cargo`, `~/.npm`, and `~/.cache` stay read-only, because builds on the host run
/// what those caches hold (extracted crate sources, npm and pnpm stores, Go build output).
const CACHE_ENV: &[(&str, &str)] = &[
    ("CARGO_HOME", "cargo"),
    ("npm_config_cache", "npm"),
    ("npm_config_store_dir", "pnpm-store"),
    ("XDG_CACHE_HOME", "xdg"),
    ("GOMODCACHE", "go-mod"),
];

/// The user's cargo settings, linked into the sandbox's `CARGO_HOME` so builds keep the same
/// linker and registry settings. Credentials are not linked.
const CARGO_SETTINGS: &[&str] = &["config.toml", "config"];

/// The bubblewrap binary when it works on this machine. Probed once per process.
pub fn bwrap() -> Option<&'static Path> {
    probed().as_ref().ok().map(PathBuf::as_path)
}

/// Why [`bwrap`] is `None`.
pub fn unavailable_reason() -> Option<&'static str> {
    probed().as_ref().err().map(String::as_str)
}

fn probed() -> &'static Result<PathBuf, String> {
    static PROBE: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    PROBE.get_or_init(probe)
}

fn probe() -> Result<PathBuf, String> {
    if !cfg!(target_os = "linux") {
        return Err("The sandbox needs Linux.".into());
    }
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

/// What an execution does about the sandbox under this config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Sandboxed(PathBuf),
    /// Unsandboxed, with the reason to warn about when the config asked for a sandbox.
    Unsandboxed(Option<String>),
}

pub const UNAVAILABLE: &str = "Install bubblewrap (the `bwrap` command) on the Linux machine that runs Ostra, or allow unprivileged user namespaces, because agent commands otherwise run with the full rights of your user. Set `[sandbox] mode = \"off\"` in config.toml to run without it on purpose.";

/// [`UNAVAILABLE`] with the probe's reason in front.
pub fn unavailable_message() -> String {
    match unavailable_reason() {
        Some(r) => format!("{r} {UNAVAILABLE}"),
        None => UNAVAILABLE.to_string(),
    }
}

/// `Err` when the config requires a sandbox this machine cannot provide.
pub fn decide(cfg: &SandboxConfig) -> Result<Decision, String> {
    match (cfg.mode, bwrap()) {
        (SandboxMode::Off, _) => Ok(Decision::Unsandboxed(None)),
        (_, Some(bin)) => Ok(Decision::Sandboxed(bin.to_path_buf())),
        (SandboxMode::Auto, None) => Ok(Decision::Unsandboxed(Some(unavailable_message()))),
        (SandboxMode::Required, None) => Err(format!(
            "The sandbox is required by `[sandbox] mode = \"required\"` but is not available. {}",
            unavailable_message()
        )),
    }
}

/// How Ostra starts a program for a project or workspace: under bubblewrap when the config and
/// this machine give one, without the credential variables Ostra holds, and with git settings
/// that keep a planted `.git/config` from starting programs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env_remove: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// [`HostCommand`] for `program args` started in `cwd`, writing only under `roots`. The global
/// config is read fresh. `Err` when `[sandbox] mode = "required"` and no sandbox is available.
pub fn host_command(
    program: &str,
    args: &[String],
    cwd: &Path,
    roots: &[&Path],
) -> Result<HostCommand, String> {
    let global: crate::config::GlobalConfig =
        crate::config::load_toml(&paths::global_config_path()).unwrap_or_default();
    let mut env_remove: Vec<String> = crate::config::credential_env_names(&global, &[])
        .into_iter()
        .collect();
    env_remove.extend(
        std::env::vars_os()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .filter(|k| k.starts_with("OSTRA_")),
    );
    let env = crate::git::agent_env();
    let (program, args) = match decide(&global.sandbox)? {
        Decision::Unsandboxed(_) => (program.to_string(), args.to_vec()),
        Decision::Sandboxed(bin) => {
            let program = find_program(program, cwd).ok_or_else(|| {
                format!("Cannot start `{program}`: no such program in the folder or on PATH.")
            })?;
            let program = program.to_string_lossy();
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/".into());
            let mut a: Vec<String> = Profile::for_program(roots, &global.sandbox, &home)
                .args(cwd)
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            a.push("--".into());
            a.push(program.to_string());
            a.extend(args.iter().cloned());
            (bin.to_string_lossy().into_owned(), a)
        }
    };
    Ok(HostCommand {
        program,
        args,
        env_remove,
        env,
    })
}

/// `program` as the host would start it from `cwd`: a path with a `/`, else the first match on
/// `PATH`. Checked before bubblewrap starts, so a missing program reads as missing.
fn find_program(program: &str, cwd: &Path) -> Option<PathBuf> {
    if program.contains('/') {
        let p = cwd.join(program);
        return p.is_file().then_some(p);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
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

/// Existing paths only, symlinks resolved, because bubblewrap mounts at real paths.
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
            p = p.workspace_state(&ctx.workspace_root);
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

    /// Mounts `dir` at `/tmp` (and at its own path, so both names reach the same files).
    pub fn scratch(mut self, dir: &Path) -> std::io::Result<Self> {
        make_private_dir(dir)?;
        self.scratch = real(dir);
        Ok(self)
    }

    /// The host dir behind the sandbox's `/tmp`.
    pub fn scratch_dir(&self) -> Option<&Path> {
        self.scratch.as_deref()
    }

    /// A path as the sandbox names it, translated to the host path of the same file. Paths under
    /// `/tmp` that another mount binds at their real location stay as they are.
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

    /// Replaces `home` with a tmpfs holding read-only binds of its entries.
    pub fn home_overlay(mut self, home: &Path) -> Self {
        self.home_overlay = real(home);
        self
    }

    /// A symlink at `link` (inside the overlaid home) pointing at `target`.
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
    pub fn new_session(mut self, on: bool) -> Self {
        self.new_session = on;
        self
    }

    /// Every mount in the order bubblewrap applies them: shallower paths first, so the deepest
    /// rule for a path wins, and at one path only the most restrictive rule.
    fn plan(&self) -> Vec<Mount> {
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
        a
    }
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

#[cfg(unix)]
fn user_id() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

#[cfg(not(unix))]
fn user_id() -> Option<u32> {
    None
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
        }
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// Runs `script` under `profile` and returns its stdout, or `None` without bubblewrap.
    fn run_in(profile: &Profile, chdir: &Path, script: &str) -> Option<String> {
        let bin = bwrap()?;
        let out = std::process::Command::new(bin)
            .args(profile.args(chdir))
            .args(["--", "sh", "-c", script])
            .output()
            .unwrap();
        Some(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
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
            echo t > /tmp/t && cat {scratch}/t && echo scratch-shared
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
            eprintln!("bwrap unavailable; skipping");
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
        assert_eq!(p.to_host(Path::new("/tmp/a/b")), scratch.join("a/b"));
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
            eprintln!("bwrap unavailable; skipping");
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
        assert_eq!(
            std::fs::read_to_string(claude.join("settings.json")).unwrap(),
            "{}"
        );
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
        match bwrap() {
            Some(b) => assert_eq!(decide(&required), Ok(Decision::Sandboxed(b.to_path_buf()))),
            None => assert!(decide(&required).is_err()),
        }
    }
}
