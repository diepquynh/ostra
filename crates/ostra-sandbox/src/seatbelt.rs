//! The Seatbelt backend: a profile rendered as an SBPL policy for `/usr/bin/sandbox-exec` on
//! macOS. Seatbelt has no mount namespace, so a hidden path returns EPERM instead of looking
//! empty, `/tmp` is denied instead of private (`TMPDIR` points at the scratch dir), a harness
//! gets no disposable home, and every sandbox shares the host's loopback, where its policy names
//! the ports it may reach. The rendering is plain code, so it is tested on every OS.

use crate::backend::Enforcer;
use crate::decoy::{Decoys, Kind, OnOpen};
use crate::egress::{self, proxy_env};
use crate::members::Members;
use crate::profile::{Mount, Profile, SandboxedCommand, UNSET_VARS, real};
use crate::sys::{Os, Platform};
use ostra_core::config::SandboxNetwork;
use std::ffi::{OsStr, OsString};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Seatbelt's front end. Called by absolute path and checked to be root-owned, because the
/// Homebrew prefix on `PATH` is writable by the user.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Seatbelt;

impl Enforcer for Seatbelt {
    fn name(&self) -> &'static str {
        "seatbelt"
    }

    fn has_mount_namespace(&self) -> bool {
        false
    }

    fn tty_param(&self) -> Option<&'static str> {
        Some(TTY_PARAM)
    }

    fn known_gaps(&self, _network: SandboxNetwork) -> Option<String> {
        None
    }

    fn decoys_supported(&self) -> bool {
        crate::decoy::refusals_readable()
    }

    /// A port of `127.0.0.1` that each policy names, with a credential of its own, because
    /// every sandbox shares the host's loopback.
    fn proxy(
        &self,
        policy: egress::Policy,
        on: egress::OnDecision,
    ) -> std::io::Result<egress::Listener> {
        egress::loopback_proxy(policy, on)
    }

    fn forward(&self, target: SocketAddr) -> std::io::Result<egress::Listener> {
        egress::loopback_splice(egress::Target::Tcp(target))
    }

    fn forward_socket(&self, socket: &Path) -> std::io::Result<egress::Listener> {
        egress::loopback_splice(egress::Target::Socket(socket.to_path_buf()))
    }

    /// Existing files only, refused by the policy and reported through the system log, because
    /// Seatbelt reports no refusal for a missing file and cannot bind one in.
    fn decoys(
        &self,
        _profile: &Profile,
        home: &Path,
        wanted: Vec<(String, Kind)>,
        on: OnOpen,
    ) -> std::io::Result<Option<Decoys>> {
        let mut at: Vec<PathBuf> = vec![];
        for (rel, _) in wanted {
            if let Some(p) = real(&home.join(&rel)).filter(|p| p.is_file())
                && !at.contains(&p)
            {
                at.push(p);
            }
        }
        if at.is_empty() || !self.decoys_supported() {
            return Ok(None);
        }
        // The system log is outside Ostra's control, so the execution runs on without decoys.
        match Decoys::refuse(&at, on) {
            Ok(d) => Ok(Some(d)),
            Err(e) => {
                tracing::warn!("decoy files are not watched: {e}");
                Ok(None)
            }
        }
    }

    fn wrap(
        &self,
        p: &Profile,
        chdir: &Path,
        program: &OsStr,
        args: Vec<OsString>,
    ) -> Result<SandboxedCommand, String> {
        let egress = match &p.egress {
            Some(l) => Some(l.clone()),
            None if p.policy.needs_proxy() => Some(Arc::new(
                self.proxy(p.policy.clone(), Arc::new(egress::log_decision))
                    .map_err(|e| format!("Cannot start the sandbox's egress proxy: {e}"))?,
            )),
            None => None,
        };
        let proxy_url = egress
            .as_ref()
            .and_then(|l| l.proxy_url().map(str::to_string));
        let mut listeners: Vec<Arc<egress::Listener>> =
            p.forwards.iter().map(|(_, l)| l.clone()).collect();
        listeners.extend(egress);
        let members = Members::new(listeners, p.decoys.clone());
        let mut a: Vec<OsString> = vec!["-p".into(), p.seatbelt(&members)?.into()];
        a.push(program.to_os_string());
        a.extend(args);
        let mut env: Vec<(String, String)> = p
            .env
            .iter()
            .filter(|(k, _)| k != "TMPDIR")
            .cloned()
            .collect();
        if let Some(t) = p.seatbelt_tmp() {
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

impl Profile {
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
        tmp_dirs.extend(Platform::user_temp_dir().as_deref().and_then(real));
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

static SERVER_PORT: OnceLock<u16> = OnceLock::new();

/// Names Ostra's own TCP port, which no Seatbelt sandbox connects to even where its workspace
/// opens the loopback, because the port serves the console's API.
pub fn set_server_port(port: u16) {
    let _ = SERVER_PORT.set(port);
}

/// `path` as an SBPL string. `Err` for a path that is not UTF-8 or holds a control character,
/// because a path that ends its string early would rewrite the policy.
pub(crate) fn sbpl_str(text: impl AsRef<OsStr>) -> Result<String, String> {
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
