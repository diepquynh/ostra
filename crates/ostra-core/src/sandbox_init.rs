//! `ostra sandbox-init`: the first program inside a bubblewrap sandbox. The sandbox has its own
//! network namespace, so the helper listens on its loopback and forwards each connection to a
//! Unix socket that Ostra bound in: the egress proxy, and fixed ports such as the hook bridge.
//! It loads the seccomp filter, then starts the command and exits with its status. Std threads only, no async runtime.
//!
//! Arguments: `[--proxy <socket>] [--forward <port>=<socket>]... -- <program> [args...]`.

use std::ffi::OsString;
use std::path::PathBuf;

/// Exit status for a helper that cannot start the command, as `env` and `timeout` use.
const FAILED: i32 = 125;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Args {
    proxy: Option<PathBuf>,
    forwards: Vec<(u16, PathBuf)>,
    program: OsString,
    args: Vec<OsString>,
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
    match parse(raw).and_then(run) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("ostra sandbox-init: {e}");
            FAILED
        }
    }
}

#[cfg(target_os = "linux")]
fn run(args: Args) -> Result<i32, String> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener};
    use std::os::unix::process::ExitStatusExt;
    let bind = |ip: IpAddr, port: u16| {
        TcpListener::bind((ip, port)).map_err(|e| format!("cannot listen on {ip}:{port}: {e}"))
    };
    let mut listeners = vec![];
    // Fixed ports first, so the proxy's free port cannot take one of them.
    for (port, sock) in args.forwards {
        listeners.push((bind(Ipv4Addr::LOCALHOST.into(), port)?, sock.clone()));
        // `localhost` may resolve to ::1 first; the namespace may have no IPv6 loopback.
        if let Ok(l) = bind(Ipv6Addr::LOCALHOST.into(), port) {
            listeners.push((l, sock));
        }
    }
    let mut cmd = std::process::Command::new(&args.program);
    cmd.args(&args.args);
    if let Some(sock) = args.proxy {
        let l = bind(Ipv4Addr::LOCALHOST.into(), 0)?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        cmd.envs(crate::sandbox::proxy_env(&format!(
            "http://127.0.0.1:{port}"
        )));
        listeners.push((l, sock));
    }
    // While the helper still has one thread, so the filter covers every thread it starts.
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    crate::seccomp::install()?;
    for (l, sock) in listeners {
        std::thread::spawn(move || forward(l, sock));
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start `{}`: {e}", args.program.to_string_lossy()))?;
    // The command gets these from its process group; the helper outlives it to report its
    // status, and the pid namespace ends everything if the helper dies anyway.
    for sig in [
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGTERM,
        libc::SIGHUP,
        libc::SIGTSTP,
        libc::SIGTTIN,
        libc::SIGTTOU,
    ] {
        // SAFETY: setting a signal to SIG_IGN has no handler to race with.
        unsafe { libc::signal(sig, libc::SIG_IGN) };
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if let Some(sig) = status.signal() {
        // SAFETY: restores the default action and re-raises, so the caller sees the same signal.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
        return Ok(128 + sig);
    }
    Ok(status.code().unwrap_or(FAILED))
}

#[cfg(not(target_os = "linux"))]
fn run(_args: Args) -> Result<i32, String> {
    Err("the sandbox helper runs on Linux only".into())
}

/// Accepts on the sandbox's loopback and splices each connection to `sock`.
#[cfg(target_os = "linux")]
fn forward(l: std::net::TcpListener, sock: PathBuf) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    const MAX_OPEN: usize = 256;
    let open = Arc::new(AtomicUsize::new(0));
    for tcp in l.incoming() {
        let Ok(tcp) = tcp else {
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        };
        if open.load(Ordering::Relaxed) >= MAX_OPEN {
            continue;
        }
        let Ok(unix) = std::os::unix::net::UnixStream::connect(&sock) else {
            continue;
        };
        open.fetch_add(1, Ordering::Relaxed);
        let open = open.clone();
        std::thread::spawn(move || {
            let (Ok(mut tcp_r), Ok(mut unix_w)) = (tcp.try_clone(), unix.try_clone()) else {
                open.fetch_sub(1, Ordering::Relaxed);
                return;
            };
            let up = std::thread::spawn(move || {
                let _ = std::io::copy(&mut tcp_r, &mut unix_w);
                let _ = unix_w.shutdown(std::net::Shutdown::Write);
            });
            let (mut unix_r, mut tcp_w) = (unix, tcp);
            let _ = std::io::copy(&mut unix_r, &mut tcp_w);
            let _ = tcp_w.shutdown(std::net::Shutdown::Write);
            let _ = up.join();
            open.fetch_sub(1, Ordering::Relaxed);
        });
    }
}
