//! `scrub_startup_env` against `sysctl(KERN_PROCARGS2)`, which any program of the same user can
//! run on another process, a sandboxed one included. Runs in its own process because the scrub
//! rewrites the whole environment.

#[cfg(target_os = "macos")]
const CHILD: &str = "OSTRA_SCRUB_TEST_CHILD";
#[cfg(target_os = "macos")]
const SECRET: &str = "scrub-test-secret-3f9a";

#[cfg(target_os = "macos")]
fn procargs(pid: i32) -> Vec<u8> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut buf = vec![0u8; 1 << 18];
    let mut len = buf.len();
    // SAFETY: `buf` holds `len` bytes, and sysctl writes at most that many.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    assert_eq!(rc, 0, "sysctl(KERN_PROCARGS2) failed");
    buf.truncate(len);
    buf
}

#[cfg(target_os = "macos")]
#[test]
fn startup_env_is_not_readable_from_another_process() {
    use std::io::{BufRead, Write};
    if std::env::var_os(CHILD).is_some() {
        ostra_sandbox::scrub_startup_env();
        assert_eq!(std::env::var("OSTRA_SCRUB_SECRET").as_deref(), Ok(SECRET));
        println!("scrubbed");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(20));
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "startup_env_is_not_readable_from_another_process",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("OSTRA_SCRUB_SECRET", SECRET)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    while out.read_line(&mut line).unwrap() > 0 && !line.contains("scrubbed") {
        line.clear();
    }
    let args = procargs(pid);
    let _ = child.kill();
    let _ = child.wait();
    assert!(line.contains("scrubbed"), "the child did not start");
    let leaked = args.windows(SECRET.len()).any(|w| w == SECRET.as_bytes());
    assert!(!leaked, "the startup environment still holds the secret");
}
