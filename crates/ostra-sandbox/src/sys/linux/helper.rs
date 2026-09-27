//! The Linux side of `ostra sandbox-init` ([`crate::init`]): the loopback listeners, the seccomp
//! filter, and the command.

use crate::init::{Args, FAILED};
use std::path::PathBuf;

pub(super) fn run(args: Args) -> Result<i32, String> {
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
        cmd.envs(crate::egress::proxy_env(&format!(
            "http://127.0.0.1:{port}"
        )));
        listeners.push((l, sock));
    }
    // While the helper still has one thread, so the filter covers every thread it starts.
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    super::seccomp::install()?;
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

/// Accepts on the sandbox's loopback and splices each connection to `sock`.
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
