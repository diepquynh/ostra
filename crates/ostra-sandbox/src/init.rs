//! `ostra sandbox-init`: the first program inside a bubblewrap sandbox. The sandbox has its own
//! network namespace, so the helper listens on its loopback and forwards each connection to a
//! Unix socket that Ostra bound in: the egress proxy, and fixed ports such as the hook bridge.
//! It loads the seccomp filter, then starts the command and exits with its status. Std threads only, no async runtime.
//!
//! Arguments: `[--proxy <socket>] [--forward <port>=<socket>]... -- <program> [args...]`.

use crate::sys::Os;
use std::ffi::OsString;
use std::path::PathBuf;

/// Exit status for a helper that cannot start the command, as `env` and `timeout` use.
pub(crate) const FAILED: i32 = 125;

/// The helper's arguments. Only Linux reads them, because only bubblewrap starts the helper.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) struct Args {
    pub(crate) proxy: Option<PathBuf>,
    pub(crate) forwards: Vec<(u16, PathBuf)>,
    pub(crate) program: OsString,
    pub(crate) args: Vec<OsString>,
}

fn parse(raw: Vec<OsString>) -> Result<Args, String> {
    let mut it = raw.into_iter();
    let mut proxy = None;
    let mut forwards = vec![];
    loop {
        let a = it.next().ok_or("missing `--` before the program")?;
        match a.to_str() {
            Some("--proxy") => {
                proxy = Some(PathBuf::from(it.next().ok_or("--proxy needs a socket")?))
            }
            Some("--forward") => {
                let f = it.next().ok_or("--forward needs <port>=<socket>")?;
                let f = f.to_str().ok_or("--forward is not UTF-8")?;
                let (port, sock) = f.split_once('=').ok_or("--forward needs <port>=<socket>")?;
                let port: u16 = port.parse().map_err(|_| format!("bad port `{port}`"))?;
                forwards.push((port, PathBuf::from(sock)));
            }
            Some("--") => break,
            _ => return Err(format!("unknown argument `{}`", a.to_string_lossy())),
        }
    }
    let program = it.next().ok_or("missing the program")?;
    Ok(Args {
        proxy,
        forwards,
        program,
        args: it.collect(),
    })
}

/// Runs the helper and returns the exit status for the process.
pub fn main(raw: Vec<OsString>) -> i32 {
    match parse(raw).and_then(crate::sys::Platform::run_helper) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("ostra sandbox-init: {e}");
            FAILED
        }
    }
}
