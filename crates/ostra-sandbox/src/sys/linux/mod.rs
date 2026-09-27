//! Linux: bubblewrap namespaces, the `sandbox-init` helper with its seccomp filter, and inotify
//! for decoys.

mod helper;
mod inotify;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod seccomp;

use super::{Os, unix};
use crate::backend::Backend;
use crate::decoy::OnOpen;
use std::path::{Path, PathBuf};

pub(crate) struct Linux;

impl Os for Linux {
    const UNAVAILABLE: &'static str = "Install bubblewrap (the `bwrap` command) on the Linux machine that runs Ostra, or allow unprivileged user namespaces, because agent commands otherwise run with the full rights of your user. Choose sandbox mode off in the workspace settings, or set `[sandbox] mode = \"off\"` in config.toml, to run without it on purpose.";

    type DecoyWatch = inotify::Watch;
    type LocalListener = tokio::net::UnixListener;
    type LocalStream = tokio::net::UnixStream;

    fn probe() -> Result<Backend, String> {
        probe_bwrap().map(|bin| Backend::Bubblewrap(crate::Bubblewrap::new(bin)))
    }

    fn run_helper(args: crate::init::Args) -> Result<i32, String> {
        helper::run(args)
    }

    /// Nothing to do: `/proc/<pid>/environ` is readable only by the same user, and every
    /// sandbox has a pid namespace of its own.
    fn scrub_startup_env() {}

    fn user_id() -> Option<u32> {
        unix::user_id()
    }

    fn user_temp_dir() -> Option<PathBuf> {
        None
    }

    fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        unix::symlink_file(target, link)
    }

    /// None: a pid namespace and `--die-with-parent` end every process of a sandbox with it.
    fn marked(_marker: &str) -> Vec<u32> {
        vec![]
    }

    fn kill(pid: u32) -> bool {
        unix::kill(pid)
    }

    fn socket_dir(dir: &Path) -> std::io::Result<()> {
        unix::socket_dir(dir)
    }

    fn bind_local(path: &Path) -> std::io::Result<Self::LocalListener> {
        unix::bind_local(path)
    }

    async fn accept_local(l: &Self::LocalListener) -> std::io::Result<Self::LocalStream> {
        unix::accept_local(l).await
    }

    async fn connect_local(path: &Path) -> std::io::Result<Self::LocalStream> {
        unix::connect_local(path).await
    }

    fn watch_opens(binds: &[(PathBuf, PathBuf)], on: OnOpen) -> std::io::Result<Self::DecoyWatch> {
        inotify::Watch::start(binds, on)
    }

    fn watch_refusals(
        _tags: &[(String, PathBuf)],
        _on: OnOpen,
    ) -> std::io::Result<Self::DecoyWatch> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    fn refusals_readable() -> bool {
        false
    }

    fn start_refusal_reports() {}
}

/// A `bwrap` on `PATH` that can create the namespaces a profile needs.
fn probe_bwrap() -> Result<PathBuf, String> {
    let bin = std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join("bwrap"))
                .find(|p| p.is_file())
        })
        .ok_or("The `bwrap` command is not installed.")?;
    let helper = crate::helper().ok_or(
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
