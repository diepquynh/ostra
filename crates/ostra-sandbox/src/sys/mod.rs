//! Everything the sandbox asks of the operating system. Each OS implements [`Os`] once, and this
//! module is the one place that picks the implementation, so the rest of the crate carries no
//! per-OS attributes. An OS without a sandbox implements it with refusals (`other.rs`).

use crate::backend::Backend;
use crate::decoy::OnOpen;
use std::future::Future;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(unix)]
mod unix;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::Linux as Platform;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::MacOs as Platform;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod other;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) use other::Other as Platform;

pub(crate) trait Os {
    /// What to do when no sandbox works here, and why it matters.
    const UNAVAILABLE: &'static str;

    /// Keeps decoys watched until dropped.
    type DecoyWatch: std::fmt::Debug + Send + Sync;
    /// A listening Unix socket on the egress runtime.
    type LocalListener: Send + Sync + 'static;
    /// A connection over a Unix socket.
    type LocalStream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// The backend that works on this machine, or why none does.
    fn probe() -> Result<Backend, String>;

    /// Runs `ostra sandbox-init` inside a bubblewrap sandbox and returns the command's status.
    fn run_helper(args: crate::init::Args) -> Result<i32, String>;

    /// See [`crate::scrub_startup_env`].
    fn scrub_startup_env();

    fn user_id() -> Option<u32>;

    /// The per-user temp dir that holds other programs' sockets, when the OS has one apart from
    /// `/tmp` (`/private/var/folders/...` on macOS).
    fn user_temp_dir() -> Option<PathBuf>;

    /// A symlink at `link` pointing at the file `target`.
    fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()>;

    /// This user's processes in a sandbox whose policy carries `marker`, on backends where a
    /// process can leave its sandbox's process tree.
    fn marked(marker: &str) -> Vec<u32>;

    /// SIGKILLs `pid`; true when the signal was sent.
    fn kill(pid: u32) -> bool;

    /// Creates `dir` owner-only and makes it so if it already exists, for Unix sockets.
    fn socket_dir(dir: &Path) -> std::io::Result<()>;

    /// Binds a Unix socket at `path`. Call inside the runtime that will accept on it.
    fn bind_local(path: &Path) -> std::io::Result<Self::LocalListener>;

    fn accept_local(
        l: &Self::LocalListener,
    ) -> impl Future<Output = std::io::Result<Self::LocalStream>> + Send;

    fn connect_local(
        path: &Path,
    ) -> impl Future<Output = std::io::Result<Self::LocalStream>> + Send;

    /// Reports the first open of each `(host file, path inside the sandbox)` with the inside
    /// path, for decoys bound into a mount namespace.
    fn watch_opens(binds: &[(PathBuf, PathBuf)], on: OnOpen) -> std::io::Result<Self::DecoyWatch>;

    /// Reports the first refusal to read each `(message, path)`, for decoys a policy refuses
    /// with that message.
    fn watch_refusals(tags: &[(String, PathBuf)], on: OnOpen) -> std::io::Result<Self::DecoyWatch>;

    /// Whether this user may read the refusals [`Os::watch_refusals`] needs.
    fn refusals_readable() -> bool;

    /// Starts reading refusals, so the first execution's decoys are watched from its first
    /// command. Does nothing where decoys do not use refusals.
    fn start_refusal_reports();
}
