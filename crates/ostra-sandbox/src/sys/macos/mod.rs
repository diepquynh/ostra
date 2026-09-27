//! macOS: Seatbelt through `sandbox-exec`, `sandbox_check` to find a sandbox's processes, and
//! the system log for decoy refusals.

mod log;

use super::{Os, unix};
use crate::SANDBOX_EXEC;
use crate::backend::Backend;
use crate::decoy::OnOpen;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(crate) struct MacOs;

impl Os for MacOs {
    const UNAVAILABLE: &'static str = "Run Ostra outside any other sandbox, because macOS cannot nest them, and agent commands otherwise run with the full rights of your user. Choose sandbox mode off in the workspace settings, or set `[sandbox] mode = \"off\"` in config.toml, to run without it on purpose.";

    type DecoyWatch = log::Registration;
    type LocalListener = tokio::net::UnixListener;
    type LocalStream = tokio::net::UnixStream;

    fn probe() -> Result<Backend, String> {
        probe_seatbelt().map(|()| Backend::Seatbelt)
    }

    fn run_helper(_args: crate::init::Args) -> Result<i32, String> {
        Err("the sandbox helper runs on Linux only".into())
    }

    fn scrub_startup_env() {
        scrub_startup_env();
    }

    fn user_id() -> Option<u32> {
        unix::user_id()
    }

    fn user_temp_dir() -> Option<PathBuf> {
        user_temp_dir()
    }

    fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        unix::symlink_file(target, link)
    }

    fn marked(marker: &str) -> Vec<u32> {
        marked(marker).into_iter().map(|p| p as u32).collect()
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

    fn watch_opens(
        _binds: &[(PathBuf, PathBuf)],
        _on: OnOpen,
    ) -> std::io::Result<Self::DecoyWatch> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    fn watch_refusals(tags: &[(String, PathBuf)], on: OnOpen) -> std::io::Result<Self::DecoyWatch> {
        log::register(tags, on)
    }

    fn refusals_readable() -> bool {
        log::admin()
    }

    fn start_refusal_reports() {
        if log::admin() {
            log::reader();
        }
    }
}

/// A root-owned `sandbox-exec` that can apply a policy here.
fn probe_seatbelt() -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let meta =
        std::fs::metadata(SANDBOX_EXEC).map_err(|_| format!("`{SANDBOX_EXEC}` is missing."))?;
    if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
        return Err(format!(
            "`{SANDBOX_EXEC}` is not owned by root or is writable by others, so Ostra does not trust it."
        ));
    }
    let out = std::process::Command::new(SANDBOX_EXEC)
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("`{SANDBOX_EXEC}` does not start: {e}."))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let err = err.trim();
    if err.contains("sandbox_apply: Operation not permitted") {
        return Err(format!(
            "Ostra runs inside another sandbox, and macOS cannot start a sandbox inside one ({err}). Start Ostra outside it, for example not from a sandboxed terminal or agent."
        ));
    }
    Err(format!(
        "`{SANDBOX_EXEC}` cannot create a sandbox here ({}).",
        if err.is_empty() {
            "no error output"
        } else {
            err
        }
    ))
}

/// A mach name every Ostra policy refuses, through `(deny default)`.
const CANARY: &str = "dev.ostra.sandbox.canary";

/// Every process of this user whose Seatbelt policy allows the mach name `marker`. The kernel
/// answers for any process whether its policy allows a name (`sandbox_check`), and a policy is
/// inherited and cannot be dropped, so the name finds every descendant however it detached.
/// Some system sandboxes allow every name, so a member must also be refused [`CANARY`].
fn marked(marker: &str) -> Vec<libc::pid_t> {
    use std::ffi::{CString, c_char, c_int};
    unsafe extern "C" {
        fn sandbox_check(pid: libc::pid_t, operation: *const c_char, kind: c_int, ...) -> c_int;
    }
    const FILTER_NONE: c_int = 0;
    const FILTER_GLOBAL_NAME: c_int = 2;
    const NO_REPORT: c_int = 0x4000_0000;
    let (Ok(name), Ok(canary)) = (CString::new(marker), CString::new(CANARY)) else {
        return vec![];
    };
    let op = c"mach-lookup";
    let mut pids: Vec<libc::pid_t> = vec![0; 8192];
    // SAFETY: the buffer holds `pids.len()` pids and its size in bytes is passed.
    let n = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<libc::pid_t>()) as c_int,
        )
    };
    pids.truncate(n.max(0) as usize);
    let me = std::process::id() as libc::pid_t;
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let owned = |pid: libc::pid_t| {
        let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as c_int;
        // SAFETY: `info` is sized for PROC_PIDT_SHORTBSDINFO, and its size is passed.
        unsafe {
            let mut info: libc::proc_bsdshortinfo = std::mem::zeroed();
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDT_SHORTBSDINFO,
                0,
                (&raw mut info).cast(),
                size,
            ) == size
                && info.pbsi_uid == uid
        }
    };
    // SAFETY: sandbox_check reads the pid's policy; the strings outlive the calls.
    let boxed = |pid: libc::pid_t| unsafe {
        sandbox_check(pid, std::ptr::null(), FILTER_NONE | NO_REPORT) == 1
    };
    // SAFETY: as above.
    let allows = |pid: libc::pid_t, n: &CString| unsafe {
        sandbox_check(pid, op.as_ptr(), FILTER_GLOBAL_NAME | NO_REPORT, n.as_ptr()) == 0
    };
    pids.into_iter()
        .filter(|&pid| pid > 1 && pid != me && owned(pid) && boxed(pid))
        .filter(|&pid| allows(pid, &name) && !allows(pid, &canary))
        .collect()
}

/// Moves this process's environment off the stack and zeroes the original strings. On macOS any
/// unsandboxed program of the same user, agent commands under mode `off` included, reads another
/// process's startup environment with `sysctl(KERN_PROCARGS2)`, which reads those stack strings;
/// Ostra's Seatbelt policies refuse it. Call first thing in `main`, before any thread starts.
fn scrub_startup_env() {
    // SAFETY: single-threaded at this point, as documented. The pointers come from
    // `environ` before it changes; each string is NUL-terminated and was written by the
    // kernel at exec, so zeroing its bytes in place is sound once nothing points at it.
    unsafe {
        let envp = *libc::_NSGetEnviron();
        let mut originals: Vec<(*mut libc::c_char, usize)> = vec![];
        let mut i = 0;
        while !envp.is_null() && !(*envp.add(i)).is_null() {
            let p = *envp.add(i);
            originals.push((p, libc::strlen(p)));
            i += 1;
        }
        let vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        for (k, _) in &vars {
            std::env::remove_var(k);
        }
        for (k, v) in &vars {
            std::env::set_var(k, v);
        }
        for (p, len) in originals {
            std::ptr::write_bytes(p, 0, len);
        }
    }
}

/// The per-user temp dir under `/private/var/folders`, which holds other programs' sockets and
/// files.
fn user_temp_dir() -> Option<PathBuf> {
    let mut buf = vec![0u8; 1024];
    // SAFETY: confstr writes at most `buf.len()` bytes including the terminating NUL.
    let n = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    if n == 0 || n > buf.len() {
        return None;
    }
    buf.truncate(n - 1);
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(OsString::from_vec(buf)))
}
