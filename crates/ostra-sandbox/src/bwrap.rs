//! The bubblewrap backend: a profile rendered as `bwrap` mounts on Linux. The sandbox gets its
//! own mount, pid, IPC, and network namespaces; the helper (`ostra sandbox-init`) starts first
//! inside it, binds the loopback ports that lead to Ostra's sockets, and loads the seccomp
//! filter. The rendering is plain code, so it is tested on every OS.

use crate::backend::Enforcer;
use crate::decoy::{Decoys, Kind, OnOpen};
use crate::egress;
use crate::members::Members;
use crate::profile::{Mount, Profile, SandboxedCommand, UNSET_VARS, real_or_missing};
use ostra_core::config::SandboxNetwork;
use ostra_core::paths;
use std::ffi::{OsStr, OsString};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The `bwrap` binary the probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bubblewrap {
    bin: PathBuf,
}

impl Bubblewrap {
    pub fn new(bin: PathBuf) -> Self {
        Bubblewrap { bin }
    }

    pub fn bin(&self) -> &Path {
        &self.bin
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

impl Enforcer for Bubblewrap {
    fn name(&self) -> &'static str {
        "bubblewrap"
    }

    fn has_mount_namespace(&self) -> bool {
        true
    }

    fn known_gaps(&self, _network: SandboxNetwork) -> Option<String> {
        (!cfg!(any(target_arch = "x86_64", target_arch = "aarch64"))).then(|| {
            "Run Ostra on x86_64 or aarch64 where agent commands must not create user namespaces, because the seccomp filter that refuses them is built for those two only.".into()
        })
    }

    fn decoys_supported(&self) -> bool {
        true
    }

    /// A socket bound into each sandbox, which the helper serves on the sandbox's loopback.
    fn proxy(
        &self,
        policy: egress::Policy,
        on: egress::OnDecision,
    ) -> std::io::Result<egress::Listener> {
        egress::proxy(policy, &egress::socket_dir(), on)
    }

    /// `127.0.0.1:<target port>` in the sandbox's network namespace, through a socket of its own
    /// and the helper.
    fn forward(&self, target: SocketAddr) -> std::io::Result<egress::Listener> {
        egress::splice(&egress::socket_dir(), target)
    }

    /// The socket itself, bound in and served on its port by the helper.
    fn forward_socket(&self, socket: &Path) -> std::io::Result<egress::Listener> {
        Ok(egress::Listener::served_elsewhere(socket.to_path_buf()))
    }

    /// A decoy goes where a hidden dir's tmpfs can hold a new file, or over an existing file,
    /// so the host is never written.
    fn decoys(
        &self,
        profile: &Profile,
        home: &Path,
        wanted: Vec<(String, Kind)>,
        on: OnOpen,
    ) -> std::io::Result<Option<Decoys>> {
        let mounts = profile.ordered();
        let in_tmpfs = |path: &Path| {
            mounts
                .iter()
                .filter(|m| m.dest() != path && path.starts_with(m.dest()))
                .max_by_key(|m| m.dest().components().count())
                .is_some_and(|m| matches!(m, Mount::Hidden(_) | Mount::HomeTmpfs(_)))
        };
        let mut at: Vec<(PathBuf, Kind)> = vec![];
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
        if at.is_empty() {
            return Ok(None);
        }
        Decoys::plant(&crate::decoy::dir(), &at, on).map(Some)
    }

    fn wrap(
        &self,
        p: &Profile,
        chdir: &Path,
        program: &OsStr,
        args: Vec<OsString>,
    ) -> Result<SandboxedCommand, String> {
        let helper = helper().ok_or("The sandbox helper (the `ostra` binary) is missing.")?;
        let egress = match &p.egress {
            Some(l) => Some(l.clone()),
            None if p.policy.needs_proxy() => Some(Arc::new(
                self.proxy(p.policy.clone(), Arc::new(egress::log_decision))
                    .map_err(|e| format!("Cannot start the sandbox's egress proxy: {e}"))?,
            )),
            None => None,
        };
        let mut forwards = p.forwards.clone();
        if p.private_network() {
            for port in p.policy.loopback_ports() {
                if forwards.iter().any(|(f, _)| *f == port) {
                    continue;
                }
                let target = SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
                let l = self.forward(target).map_err(|e| {
                    format!("Cannot forward 127.0.0.1:{port} into the sandbox: {e}")
                })?;
                forwards.push((port, Arc::new(l)));
            }
        }
        let mut a = p.args_with(chdir, egress.as_deref(), &forwards);
        a.push("--".into());
        a.push(helper.into_os_string());
        a.push("sandbox-init".into());
        let mut listeners: Vec<Arc<egress::Listener>> = vec![];
        if p.private_network() {
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
        a.extend(args);
        Ok(SandboxedCommand {
            program: self.bin.clone(),
            args: a,
            env: vec![],
            env_remove: vec![],
            cwd: chdir.to_path_buf(),
            members: Some(Members::new(listeners, p.decoys.clone())),
        })
    }
}

impl Profile {
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
