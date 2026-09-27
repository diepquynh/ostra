//! What Linux and macOS share: Unix sockets, signals, and file modes.

use std::path::Path;

pub(super) fn user_id() -> Option<u32> {
    // SAFETY: getuid cannot fail.
    Some(unsafe { libc::getuid() })
}

pub(super) fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

pub(super) fn kill(pid: u32) -> bool {
    // SAFETY: kill(2) on a pid the caller just found running.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) == 0 }
}

pub(super) fn socket_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

pub(super) fn bind_local(path: &Path) -> std::io::Result<tokio::net::UnixListener> {
    let l = std::os::unix::net::UnixListener::bind(path)?;
    l.set_nonblocking(true)?;
    tokio::net::UnixListener::from_std(l)
}

pub(super) async fn accept_local(
    l: &tokio::net::UnixListener,
) -> std::io::Result<tokio::net::UnixStream> {
    l.accept().await.map(|(c, _)| c)
}

pub(super) async fn connect_local(path: &Path) -> std::io::Result<tokio::net::UnixStream> {
    tokio::net::UnixStream::connect(path).await
}
