//! Sandbox profiles: one execution's boundary, described once for every backend. The host is
//! read-only, only the execution's own roots are writable, and Ostra's data, the user's
//! credentials, and the session bus or keychain are not reachable at all. A [`Backend`] renders
//! the profile (`bwrap.rs`, `seatbelt.rs`), and whatever differs between backends is asked of
//! it here rather than matched on.

use crate::backend::Backend;
use crate::cache::{CACHE_ENV, CARGO_SETTINGS, keyed_cache, session_cache};
use crate::git::{GIT_PROTECTED, GitDir, git_dirs, git_repos, read_dirs};
use crate::members::Members;
use crate::sys::{Os, Platform};
use crate::{decoy, egress};
use ostra_core::config::{SandboxConfig, SandboxNetwork};
use ostra_core::exec::ExecContext;
use ostra_core::paths;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// Variables that point a command at the desktop session, an agent, or a daemon socket.
pub(crate) const UNSET_VARS: &[&str] = &[
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

fn expand(home: &Path, p: &str) -> PathBuf {
    match p.trim().strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p.trim()),
    }
}

/// Existing paths only, symlinks resolved, because bubblewrap mounts at real paths and Seatbelt
/// matches them.
pub(crate) fn real(p: &Path) -> Option<PathBuf> {
    paths::canonical(p).ok()
}

/// The real path of `p`'s longest existing ancestor with the missing rest appended, for paths
/// that may not exist yet.
pub(crate) fn real_or_missing(p: &Path) -> Option<PathBuf> {
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

use paths::{
    create_private_dir as make_private_dir, create_private_dir_all as make_private_dir_all,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mount {
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
    pub(crate) fn dest(&self) -> &Path {
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
    pub(crate) mounts: Vec<Mount>,
    /// A home folder replaced by a tmpfs with its entries bound back read-only, so new files a
    /// harness writes at its top level vanish with the execution.
    pub(crate) home_overlay: Option<PathBuf>,
    /// The host dir mounted at `/tmp`. `None` gives `/tmp` an empty tmpfs.
    pub(crate) scratch: Option<PathBuf>,
    pub(crate) env: Vec<(String, String)>,
    /// What the network namespace lets out, through the egress proxy.
    pub(crate) policy: egress::Policy,
    /// The proxy shared by every command of this profile, once started.
    pub(crate) egress: Option<Arc<egress::Listener>>,
    /// Loopback ports inside the sandbox forwarded to host sockets, such as the hook bridge.
    pub(crate) forwards: Vec<(u16, Arc<egress::Listener>)>,
    /// Fake credential files bound into hidden paths and watched, once planted.
    pub(crate) decoys: Option<Arc<decoy::Decoys>>,
    pub(crate) new_session: bool,
    /// The per-workspace cache dir, whose `tmp` is `TMPDIR` under Seatbelt when no scratch dir
    /// is set.
    pub(crate) cache_root: Option<PathBuf>,
    /// Seatbelt lets the program use the PTY named by the `TTY` parameter as its terminal.
    pub(crate) tty: bool,
    /// Single files made writable, rendered after every other rule.
    pub(crate) writable_files: Vec<PathBuf>,
    /// Credential paths the user lets agents read, which no decoy replaces.
    pub(crate) readable: Vec<PathBuf>,
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
        // Rule WD1: every work dir the orchestrator named is writable, beside the workspace root.
        let work = ctx.work_roots();
        for root in [&ctx.workspace_root, &ctx.session_root, &ctx.session_dir]
            .into_iter()
            .map(PathBuf::as_path)
            .chain(work.iter().copied())
        {
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
            ostra_core::ExecutorKind::Harness(h) => Some(h),
            ostra_core::ExecutorKind::Native => None,
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
            // Rules W1 and B5: agents read workspace artifacts and books and never write them.
            p = p
                .workspace_state(&ctx.workspace_root)
                .read_only_dir(&ostra_core::artifacts::dir(&ctx.workspace_root))
                .read_only_dir(&ostra_core::book::dir(&ctx.workspace_root));
        }
        let memory_dbs = std::iter::once(ctx.memory_db.clone())
            .chain(work.iter().map(|r| paths::project_memory_db(r)));
        for f in memory_dbs.flat_map(|db| db_files(&db)) {
            p = p.read_only(&f);
        }
        for prot in &ctx.protected_paths {
            p = p.read_only(prot);
        }
        let mut git_roots = work;
        git_roots.push(&ctx.workspace_root);
        p.protect_git(&git_roots)
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
        own: Option<ostra_core::HarnessKind>,
        cache: PathBuf,
    ) -> Profile {
        for w in &cfg.extra_writable {
            self = self.writable(&expand(home, w));
        }
        self.add_caches(cache, home);
        // Never opened by `extra_readable`: Ostra's own files, the session bus, container sockets.
        let mut never: Vec<PathBuf> = paths::ostra_private_paths();
        never.extend(
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .or_else(|| {
                    Platform::user_id().map(|uid| PathBuf::from(format!("/run/user/{uid}")))
                }),
        );
        never.extend(std::env::var_os("XAUTHORITY").map(PathBuf::from));
        never.extend(DOCKER_SOCKETS.iter().map(|s| expand(home, s)));
        let never: Vec<PathBuf> = never.iter().filter_map(|n| real(n)).collect();
        self.readable = readable_paths(home, &cfg.extra_readable, &never);
        let mut credentials: Vec<PathBuf> = paths::HOME_CREDENTIALS
            .iter()
            .filter(|c| own.is_none() || c.harness != own)
            .map(|c| home.join(c.path))
            .collect();
        credentials.extend(cfg.extra_hidden.iter().map(|h| expand(home, h)));
        for h in never {
            self = self.hidden(&h);
        }
        for h in credentials {
            if let Some(h) = real(&h)
                && !self.readable.iter().any(|r| h.starts_with(r))
            {
                self.mounts.push(Mount::Hidden(h));
            }
        }
        for path in self.readable.clone() {
            self.mounts.push(Mount::ReadOnly {
                dir: path.is_dir(),
                path,
                revealed: true,
            });
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
            if from.is_file() && std::fs::symlink_metadata(&to).is_err() {
                let _ = Platform::symlink_file(&from, &to);
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
            self.egress = Some(Arc::new(backend.enforcer().proxy(self.policy.clone(), on)?));
        }
        Ok(self)
    }

    /// Rule P3: plants the built-in decoys and the workspace's own (`extra`, `~/` paths), apart
    /// from those under a readable path, and reports each one's first open to `on`. Under bubblewrap a decoy goes where a hidden dir's
    /// tmpfs can hold a new file, or over an existing file, so the host is never written. Under
    /// Seatbelt only an existing file can be one, refused and reported through the system log.
    pub fn watch_decoys(
        mut self,
        backend: &Backend,
        home: &Path,
        extra: &[String],
        on: decoy::OnOpen,
    ) -> std::io::Result<Self> {
        let ssh_reachable = self.policy.reaches_port(22);
        let wanted = decoy::wanted(extra)
            .into_iter()
            .filter(|(_, kind)| !(*kind == decoy::Kind::SshKey && ssh_reachable))
            // A decoy over a file the user lets agents read would hide it and report its reads.
            .filter(|(rel, _)| {
                real_or_missing(&home.join(rel))
                    .is_none_or(|d| !self.readable.iter().any(|r| d.starts_with(r)))
            })
            .collect();
        if let Some(d) = backend.enforcer().decoys(&self, home, wanted, on)? {
            self.decoys = Some(Arc::new(d));
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
            let l = backend.enforcer().forward(target)?;
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
            let l = backend.enforcer().forward_socket(socket)?;
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

    pub(crate) fn private_network(&self) -> bool {
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
    /// once the PTY is open, so the launcher passes it as `-D TTY=<path>` ([`crate::TTY_PARAM`]).
    pub fn tty(mut self, on: bool) -> Self {
        self.tty = on;
        self
    }

    /// Every rule ordered for both backends: shallower paths first, so the deepest rule for a
    /// path wins, and at one path only the most restrictive rule.
    pub(crate) fn ordered(&self) -> Vec<Mount> {
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
        let args = args.into_iter().map(Into::into).collect();
        backend.enforcer().wrap(self, chdir, program, args)
    }
}

/// The real paths of `entries` (absolute or `~/...`), dropping any that would reach one of
/// `never`, the home folder, or a dir above it, because each entry opens only what it names.
pub(crate) fn readable_paths(home: &Path, entries: &[String], never: &[PathBuf]) -> Vec<PathBuf> {
    let home = real(home).unwrap_or_else(|| home.to_path_buf());
    let mut out: Vec<PathBuf> = entries
        .iter()
        .filter_map(|e| real(&expand(&home, e)))
        .filter(|r| {
            !home.starts_with(r) && !never.iter().any(|n| n.starts_with(r) || r.starts_with(n))
        })
        .collect();
    out.sort();
    out.dedup();
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
