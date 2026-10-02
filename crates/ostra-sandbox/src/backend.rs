//! Which backend enforces a profile, and the [`Enforcer`] contract every backend implements.
//! Code outside a backend's own module never matches on the backend to pick a behavior: it asks
//! [`Backend::enforcer`], so a backend's quirks stay in its file.

use crate::decoy::{Decoys, Kind, OnOpen};
use crate::egress;
use crate::profile::{Profile, SandboxedCommand};
use crate::sys::{self, Os};
use ostra_core::config::{SandboxConfig, SandboxMode, SandboxNetwork};
use std::ffi::{OsStr, OsString};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// What enforces the profile on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// Linux mount, pid, and network namespaces through the `bwrap` binary.
    Bubblewrap(crate::Bubblewrap),
    /// A Seatbelt (SBPL) policy through `/usr/bin/sandbox-exec` on macOS.
    Seatbelt,
}

impl Backend {
    pub(crate) fn enforcer(&self) -> &dyn Enforcer {
        match self {
            Backend::Bubblewrap(b) => b,
            Backend::Seatbelt => &crate::seatbelt::Seatbelt,
        }
    }

    pub fn name(&self) -> &'static str {
        self.enforcer().name()
    }

    /// Whether the sandbox has a mount namespace of its own: paths can be remapped (the scratch
    /// dir at `/tmp`), the home folder overlaid, and symlinks placed. Seatbelt only allows or
    /// denies paths where they are.
    pub fn has_mount_namespace(&self) -> bool {
        self.enforcer().has_mount_namespace()
    }

    /// The `sandbox-exec -D` parameter a PTY program's launcher sets to the PTY's device path,
    /// when the backend needs one to let the program use its terminal.
    pub fn tty_param(&self) -> Option<&'static str> {
        self.enforcer().tty_param()
    }
}

/// How one backend turns a [`Profile`] into a running sandbox.
pub(crate) trait Enforcer: Send + Sync {
    fn name(&self) -> &'static str;

    fn has_mount_namespace(&self) -> bool;

    fn tty_param(&self) -> Option<&'static str> {
        None
    }

    /// What this backend cannot enforce on this machine under `network`, for the setup check.
    fn known_gaps(&self, network: SandboxNetwork) -> Option<String>;

    /// Whether decoy opens are reported on this machine.
    fn decoys_supported(&self) -> bool;

    /// Starts an egress proxy where this backend's sandbox reaches it.
    fn proxy(
        &self,
        policy: egress::Policy,
        on: egress::OnDecision,
    ) -> std::io::Result<egress::Listener>;

    /// A listener the sandbox reaches in place of `target` on the host's loopback.
    fn forward(&self, target: SocketAddr) -> std::io::Result<egress::Listener>;

    /// A listener the sandbox reaches in place of the Unix socket `socket`, which Ostra serves.
    fn forward_socket(&self, socket: &Path) -> std::io::Result<egress::Listener>;

    /// Places the `wanted` decoys (paths under `home`) where this backend can watch them. `None`
    /// when none of them fits.
    fn decoys(
        &self,
        profile: &Profile,
        home: &Path,
        wanted: Vec<(String, Kind)>,
        on: OnOpen,
    ) -> std::io::Result<Option<Decoys>>;

    /// `program args` wrapped in `profile`, started in `chdir`.
    fn wrap(
        &self,
        profile: &Profile,
        chdir: &Path,
        program: &OsStr,
        args: Vec<OsString>,
    ) -> Result<SandboxedCommand, String>;
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
    PROBE.get_or_init(sys::Platform::probe)
}

/// What an execution does about the sandbox under this config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Sandboxed(Backend),
    /// Unsandboxed, with the reason to warn about when the config asked for a sandbox.
    Unsandboxed(Option<String>),
}

/// What the sandbox on this machine cannot enforce under `network`, for the setup check.
pub fn known_gaps(backend: &Backend, network: SandboxNetwork) -> Option<String> {
    backend.enforcer().known_gaps(network)
}

/// What to do about a missing sandbox on this OS, with the probe's reason in front.
pub fn unavailable_message() -> String {
    let fix = <sys::Platform as Os>::UNAVAILABLE;
    match unavailable_reason() {
        Some(r) => format!("{r} {fix}"),
        None => fix.to_string(),
    }
}

/// Whether an execution with this workspace sandbox runs sandboxed under the global config read
/// fresh now.
pub fn runs_sandboxed(ws: &ostra_core::config::WorkspaceSandbox) -> bool {
    let global: ostra_core::config::GlobalConfig =
        ostra_core::config::load_toml(&ostra_core::paths::global_config_path()).unwrap_or_default();
    matches!(
        decide(&global.sandbox.for_workspace(ws)),
        Ok(Decision::Sandboxed(_))
    )
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

/// True the first time it is called, so the missing-sandbox warning reaches the log once.
pub fn first_warning() -> bool {
    static WARNED: AtomicBool = AtomicBool::new(false);
    !WARNED.swap(true, Ordering::Relaxed)
}
