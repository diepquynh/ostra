//! Windows and every OS without a sandbox yet: the probe says why, and everything a sandbox
//! would need refuses.

use super::Os;
use crate::backend::Backend;
use crate::decoy::OnOpen;
use std::path::{Path, PathBuf};

pub(crate) struct Other;

/// No decoy is ever watched here.
#[derive(Debug)]
pub(crate) struct NoWatch;

/// Stands in for a Unix socket, which these OSes do not give the egress proxy.
pub(crate) enum NoSocket {}

fn unsupported<T>() -> std::io::Result<T> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix sockets exist only on Linux and macOS.",
    ))
}

impl Os for Other {
    const UNAVAILABLE: &'static str = if cfg!(windows) {
        "Run Ostra inside WSL 2 or in Docker to sandbox agent commands, because Ostra has no Windows sandbox yet and agent commands otherwise run with the full rights of your user. Choose sandbox mode off in the workspace settings, or set `[sandbox] mode = \"off\"` in config.toml, to run without it on purpose."
    } else {
        "Run Ostra on Linux or macOS to sandbox agent commands, because agent commands otherwise run with the full rights of your user. Choose sandbox mode off in the workspace settings, or set `[sandbox] mode = \"off\"` in config.toml, to run without it on purpose."
    };

    type DecoyWatch = NoWatch;
    type LocalListener = NoSocket;
    type LocalStream = tokio::io::DuplexStream;

    fn probe() -> Result<Backend, String> {
        Err(if cfg!(windows) {
            "The Windows sandbox is not built yet."
        } else {
            "The sandbox needs Linux or macOS."
        }
        .into())
    }

    fn run_helper(_args: crate::init::Args) -> Result<i32, String> {
        Err("the sandbox helper runs on Linux only".into())
    }

    fn scrub_startup_env() {}

    fn user_id() -> Option<u32> {
        None
    }

    fn user_temp_dir() -> Option<PathBuf> {
        None
    }

    fn symlink_file(_target: &Path, _link: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    fn marked(_marker: &str) -> Vec<u32> {
        vec![]
    }

    fn kill(_pid: u32) -> bool {
        false
    }

    fn socket_dir(dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)
    }

    fn bind_local(_path: &Path) -> std::io::Result<Self::LocalListener> {
        unsupported()
    }

    async fn accept_local(l: &Self::LocalListener) -> std::io::Result<Self::LocalStream> {
        match *l {}
    }

    async fn connect_local(_path: &Path) -> std::io::Result<Self::LocalStream> {
        unsupported()
    }

    fn watch_opens(
        _binds: &[(PathBuf, PathBuf)],
        _on: OnOpen,
    ) -> std::io::Result<Self::DecoyWatch> {
        Err(std::io::ErrorKind::Unsupported.into())
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
