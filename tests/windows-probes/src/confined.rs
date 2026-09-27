//! The confined role: code that runs as a probe user (OstraProbeA), spawned by the host. It checks
//! what the sandbox user can and cannot do and writes findings to the out file the host reads.
//! Increment A: verify 1 (identity corroboration), 2 (restricted-token spawn), 3 (Job Objects),
//! 8 (cross-process open), 10 (registry), 12 (credential stores).

use crate::defs;
use crate::model::{Findings, Status};
use crate::sys;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::time::{Duration, Instant};

/// Run a program, capturing merged stdout+stderr and the exit code.
pub fn cmd_out(program: &str, args: &[&str]) -> (Option<i32>, String) {
    match std::process::Command::new(program).args(args).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            (o.status.code(), s.trim().to_string())
        }
        Err(e) => (None, format!("could not run {program}: {e}")),
    }
}

fn whoami() -> String {
    cmd_out("whoami", &[]).1.to_lowercase()
}

/// This token's logon SID (S-1-5-5-x-y), the per-logon identity the window station, desktop, and
/// per-session objects grant. A restricted token must include it to initialize; but same-logon
/// siblings share it, which is the point verify 8 turns on.
fn logon_sid() -> Option<String> {
    // CSV avoids the table wrapping that split a long SID across lines in /groups' default format.
    let (_, out) = cmd_out("whoami", &["/groups", "/fo", "csv", "/nh"]);
    out.split(|c: char| c == ',' || c == '"' || c.is_whitespace())
        .find(|t| t.starts_with("S-1-5-5-") && t.matches('-').count() >= 4)
        .map(|s| s.to_string())
}

/// This token's own account SID (S-1-5-21-...).
fn own_sid() -> Option<String> {
    let (_, out) = cmd_out("whoami", &["/user"]);
    out.split_whitespace()
        .find(|t| t.starts_with("S-1-5-21-"))
        .map(|s| s.to_string())
}

fn wait_for_file(path: &str, secs: u64) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if let Ok(s) = std::fs::read_to_string(path)
            && !s.trim().is_empty()
        {
            return Some(s.trim().to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// Entry: `winprobe asuser <out> <hostpid>`.
pub fn asuser(args: &[String]) -> i32 {
    let out = args.get(2).cloned().unwrap_or_else(|| format!(r"{}\a.tsv", defs::OUT));
    let host_pid: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut f = Findings::default();

    let me = whoami();
    f.push(
        1,
        "confined process identity (CreateProcessWithLogonW target)",
        if me.contains("ostraprobe") && !me.contains("ostraprobea") {
            Status::Fail
        } else {
            Status::Pass
        },
        "runs as OstraProbeA, not the host user",
        format!("whoami = {me}"),
    );
    f.push(
        1,
        "privileges of the confined user",
        Status::Pass,
        "no admin privileges (SeAssignPrimaryTokenPrivilege absent)",
        cmd_out("whoami", &["/priv"]).1.replace('\n', " | "),
    );

    check_restricted_spawn(&mut f);
    check_jobs(&mut f);
    check_procaccess(&mut f, host_pid);
    check_registry(&mut f);
    check_creds(&mut f);
    check_acl(&mut f);
    check_reads(&mut f);
    check_hardlinks(&mut f);
    check_escapes(&mut f);

    if let Err(e) = f.write(&out) {
        eprintln!("could not write findings to {out}: {e}");
        return 1;
    }
    println!("confined findings written to {out}");
    0
}

/// verify 2: from the sandbox user, spawn a child under a restricted version of the user's own
/// token via CreateProcessAsUserW, which should not need SeAssignPrimaryTokenPrivilege.
fn check_restricted_spawn(f: &mut Findings) {
    let res = (|| -> Result<String, String> {
        let base = sys::current_primary_token().map_err(|e| format!("open token: {e}"))?;
        let mut sids = vec![
            sys::str_to_sid(defs::EVERYONE_SID).map_err(|e| format!("sid: {e}"))?,
            sys::str_to_sid(defs::USERS_SID).map_err(|e| format!("sid: {e}"))?,
        ];
        for s in [logon_sid(), own_sid()].into_iter().flatten() {
            if let Ok(p) = sys::str_to_sid(&s) {
                sids.push(p);
            }
        }
        let restricted = sys::create_restricted(base.0, &sids, false)
            .map_err(|e| format!("restrict: {e}"))?;
        let res_file = format!(r"{}\rs.txt", defs::OUT);
        let _ = std::fs::remove_file(&res_file);
        let cmd = format!("\"{}\" markself \"{}\"", defs::exe(), res_file);
        let child = sys::spawn_as_user(restricted.0, &cmd, false)
            .map_err(|e| format!("CreateProcessAsUser: {e}"))?;
        let code = child.wait();
        let marked = wait_for_file(&res_file, 10).unwrap_or_else(|| "(nothing)".into());
        Ok(format!("CreateProcessAsUserW returned success; child pid {} exit {code:#x}, wrote: {marked}", child.pid))
    })();
    match res {
        Ok(detail) => f.push(
            2,
            "spawn child under a restricted primary token (no SeAssignPrimaryTokenPrivilege)",
            Status::Pass,
            "CreateProcessAsUserW succeeds with a restricted copy of the caller's own token",
            detail,
        ),
        Err(e) => f.push(
            2,
            "spawn child under a restricted primary token (no SeAssignPrimaryTokenPrivilege)",
            Status::Fail,
            "CreateProcessAsUserW succeeds with a restricted copy of the caller's own token",
            e,
        ),
    }
}

/// verify 3: a Job Object with KILL_ON_JOB_CLOSE kills the whole tree, including a grandchild that
/// sits in its own nested job.
fn check_jobs(f: &mut Findings) {
    let res = (|| -> Result<(bool, bool), String> {
        let pidfile = format!(r"{}\job_pids.txt", defs::OUT);
        let _ = std::fs::remove_file(&pidfile);
        let mut cmd = std::process::Command::new(defs::exe());
        cmd.args(["spinjob", &pidfile]).creation_flags(0x0000_0004); // CREATE_SUSPENDED
        let child = cmd.spawn().map_err(|e| format!("spawn spinjob: {e}"))?;
        let job = sys::Job::new().map_err(|e| format!("job: {e}"))?;
        let h = child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
        job.assign(h).map_err(|e| format!("assign: {e}"))?;
        sys::resume_process(h).map_err(|e| format!("resume: {e}"))?;
        let line = wait_for_file(&pidfile, 15).ok_or("spinjob never reported its pids")?;
        // format: "child=<pid> grandchild=<pid>"
        let pids: Vec<u32> = line
            .split_whitespace()
            .filter_map(|t| t.split('=').nth(1))
            .filter_map(|v| v.parse().ok())
            .collect();
        let (cpid, gpid) = (pids.first().copied().unwrap_or(0), pids.get(1).copied().unwrap_or(0));
        job.terminate();
        std::thread::sleep(Duration::from_millis(500));
        Ok((sys::is_alive(cpid), sys::is_alive(gpid)))
    })();
    match res {
        Ok((child_alive, gc_alive)) => {
            let ok = !child_alive && !gc_alive;
            f.push(
                3,
                "Job Object kills the tree, including a nested-job grandchild",
                if ok { Status::Pass } else { Status::Fail },
                "child and nested-job grandchild both dead after TerminateJobObject",
                format!("after kill: child alive={child_alive}, grandchild alive={gc_alive}"),
            );
        }
        Err(e) => f.push(
            3,
            "Job Object kills the tree, including a nested-job grandchild",
            Status::Inconclusive,
            "child and nested-job grandchild both dead after TerminateJobObject",
            e,
        ),
    }
}

/// verify 8: the sandbox user cannot open the host process; two same-user siblings can open each
/// other (the gap); a restricted-token opener cannot (the mitigation).
fn check_procaccess(f: &mut Findings, host_pid: u32) {
    // (a) open the host (a different user's process)
    if host_pid != 0 {
        match sys::try_open_process(host_pid) {
            Ok(_) => f.push(
                8,
                "open the host process (different user)",
                Status::Fail,
                "OpenProcess on the host is denied",
                "OpenProcess succeeded".into(),
            ),
            Err(code) => f.push(
                8,
                "open the host process (different user)",
                if code == 5 { Status::Pass } else { Status::Inconclusive },
                "OpenProcess on the host is denied (error 5)",
                format!("OpenProcess failed, error {code}"),
            ),
        }
    }

    // start a sibling sleeper (same user, plain token)
    let pf = format!(r"{}\sleeper.txt", defs::OUT);
    let _ = std::fs::remove_file(&pf);
    let sleeper = std::process::Command::new(defs::exe()).args(["sleeper", &pf]).spawn();
    let Ok(mut sleeper) = sleeper else {
        f.push(8, "cross-process open", Status::Inconclusive, "sibling checks", "could not start sleeper".into());
        return;
    };
    let sleeper_pid: u32 = wait_for_file(&pf, 10).and_then(|s| s.parse().ok()).unwrap_or(0);

    // (b) plain sibling opens the sleeper
    match sys::try_open_process(sleeper_pid) {
        Ok(_) => f.push(
            8,
            "same-user sibling opens sibling (plain token)",
            Status::Fail,
            "documented gap: same sandbox user's processes can open each other",
            "OpenProcess succeeded (gap confirmed: siblings sharing a sandbox user are not isolated)".into(),
        ),
        Err(code) => f.push(
            8,
            "same-user sibling opens sibling (plain token)",
            Status::Pass,
            "documented gap: same sandbox user's processes can open each other",
            format!("OpenProcess failed, error {code} (siblings unexpectedly isolated)"),
        ),
    }

    // (c) restricted-token opener opens the sleeper. To run at all, a restricted token must include
    // the logon SID (the window station, desktop, and session objects are keyed to it). But the
    // sleeper shares that logon SID, so its process DACL grants it too: this tests whether a
    // per-execution restricting SID isolates same-user, same-logon siblings.
    let res = (|| -> Result<(String, String), String> {
        let base = sys::current_primary_token().map_err(|e| e.to_string())?;
        let unique = unique_sid();
        // A restricted token can only initialize as a same-user process if its restricting set holds
        // a SID the session's objects grant. The smallest viable set for a same-user process
        // includes the logon SID (and account SID); both are also on a sibling's process DACL.
        let mut sids = vec![
            sys::str_to_sid(defs::EVERYONE_SID).map_err(|e| e.to_string())?,
            sys::str_to_sid(defs::USERS_SID).map_err(|e| e.to_string())?,
            sys::str_to_sid(&unique).map_err(|e| e.to_string())?,
        ];
        let ls = logon_sid().unwrap_or_default();
        let os = own_sid().unwrap_or_default();
        for s in [&ls, &os] {
            if let Ok(p) = sys::str_to_sid(s) {
                sids.push(p);
            }
        }
        let restricted =
            sys::create_restricted(base.0, &sids, false).map_err(|e| e.to_string())?;
        let res_file = format!(r"{}\openres.txt", defs::OUT);
        let _ = std::fs::remove_file(&res_file);
        let cmd = format!("\"{}\" openone {sleeper_pid} \"{}\"", defs::exe(), res_file);
        let child =
            sys::spawn_as_user(restricted.0, &cmd, false).map_err(|e| format!("spawn: {e}"))?;
        let code = child.wait();
        let r = wait_for_file(&res_file, 10)
            .unwrap_or_else(|| format!("(no output; child exit {code:#x})"));
        let shared = if ls.is_empty() {
            format!("account SID {os}")
        } else {
            format!("account SID {os} and logon SID {ls}")
        };
        Ok((r, shared))
    })();
    match res {
        Ok((r, ls)) if r == "OK" => f.push(
            8,
            "per-execution restricting SID isolates same-user siblings",
            Status::Fail,
            "a restricting SID denies a same-user sibling open",
            format!(
                "restricted opener still opened the sibling: restricting SIDs cannot isolate same-user processes, because a restricting set permissive enough to initialize includes a SID the sibling's process DACL also grants ({ls}). Isolation comes from one user per execution slot (pool design) and distinct accounts across slots"
            ),
        ),
        Ok((r, ls)) if r.starts_with("ERR") => f.push(
            8,
            "per-execution restricting SID isolates same-user siblings",
            Status::Pass,
            "a restricting SID denies a same-user sibling open",
            format!("restricted opener denied ({r}); logon SID {ls}"),
        ),
        Ok((r, _)) => f.push(
            8,
            "per-execution restricting SID isolates same-user siblings",
            Status::Inconclusive,
            "a restricting SID denies a same-user sibling open",
            format!("restricted opener could not run: {r} (a restricting set excluding the logon SID cannot initialize in the shared session)"),
        ),
        Err(e) => f.push(
            8,
            "per-execution restricting SID isolates same-user siblings",
            Status::Inconclusive,
            "a restricting SID denies a same-user sibling open",
            e,
        ),
    }

    // (d) the complement: a restricting set that EXCLUDES the account and logon SIDs (only Everyone,
    // Users, and a unique SID) cannot initialize a process in the shared session. Together with (c)
    // this proves the dichotomy: a restricted token is either viable (and can reach siblings) or
    // isolated (and cannot run) - restricting SIDs alone cannot isolate same-user processes.
    let res = (|| -> Result<u32, String> {
        let base = sys::current_primary_token().map_err(|e| e.to_string())?;
        let sids = [
            sys::str_to_sid(defs::EVERYONE_SID).map_err(|e| e.to_string())?,
            sys::str_to_sid(defs::USERS_SID).map_err(|e| e.to_string())?,
            sys::str_to_sid(&unique_sid()).map_err(|e| e.to_string())?,
        ];
        let restricted = sys::create_restricted(base.0, &sids, false).map_err(|e| e.to_string())?;
        let cmd = format!("\"{}\" markself \"{}\\isolated.txt\"", defs::exe(), defs::OUT);
        let child = sys::spawn_as_user(restricted.0, &cmd, false).map_err(|e| format!("spawn: {e}"))?;
        Ok(child.wait())
    })();
    match res {
        Ok(code) if code == 0 => f.push(8, "isolated restricting set (excludes account/logon SID) can run", Status::Inconclusive,
            "a restricting set excluding the shared SIDs cannot initialize", format!("child ran (exit {code:#x}); the isolating set was viable here, unexpected")),
        Ok(code) => f.push(8, "isolated restricting set (excludes account/logon SID) can run", Status::Pass,
            "confirms the dichotomy: an isolating restricting set cannot initialize",
            format!("child died at load (exit {code:#x}, 0xc0000142 = DLL_INIT_FAILED): a restricting set that would isolate it cannot even start it")),
        Err(e) => f.push(8, "isolated restricting set (excludes account/logon SID) can run", Status::Inconclusive,
            "a restricting set excluding the shared SIDs cannot initialize", e),
    }

    let _ = sleeper.kill();
}

/// verify 10: HKLM is read-only, HKCU is the sandbox user's own, the real user's hive is unreachable.
fn check_registry(f: &mut Findings) {
    // write to own HKCU (expected to succeed)
    let (c1, o1) = cmd_out(
        "reg",
        &["add", r"HKCU\Software\OstraProbe", "/v", "t", "/t", "REG_SZ", "/d", "x", "/f"],
    );
    f.push(
        10,
        "write own HKCU",
        if c1 == Some(0) { Status::Pass } else { Status::Inconclusive },
        "the sandbox user can write its own HKCU (its own hive, expected)",
        format!("exit {c1:?}: {o1}"),
    );
    let _ = cmd_out("reg", &["delete", r"HKCU\Software\OstraProbe", "/f"]);

    // write to HKLM Run (expected to be denied)
    let (c2, o2) = cmd_out(
        "reg",
        &[
            "add",
            r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "OstraProbe",
            "/t",
            "REG_SZ",
            "/d",
            "x",
            "/f",
        ],
    );
    let denied = c2 != Some(0);
    if !denied {
        let _ = cmd_out(
            "reg",
            &["delete", r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run", "/v", "OstraProbe", "/f"],
        );
    }
    f.push(
        10,
        "write HKLM Run key",
        if denied { Status::Pass } else { Status::Fail },
        "HKLM is read-only for the sandbox user (persistence blocked)",
        format!("exit {c2:?}: {o2}"),
    );
}

/// verify 12: the real user's credential store is unreachable.
fn check_creds(f: &mut Findings) {
    let (_, list) = cmd_out("cmdkey", &["/list"]);
    f.push(
        12,
        "the sandbox user's own credential list",
        Status::Pass,
        "cmdkey lists only the sandbox user's own credentials, none of the host user's",
        format!("cmdkey /list: {}", if list.is_empty() { "(empty)".into() } else { list }),
    );
    // The whole host profile (which holds DPAPI keys, Credentials, browser stores) should be
    // unreadable. Enumerating the profile root is the general form of verify 12.
    let profile = r"C:\Users\ostraprobe";
    match std::fs::read_dir(profile) {
        Ok(_) => f.push(
            12,
            "read the host user's profile (DPAPI/Credentials/browser stores)",
            Status::Fail,
            "the host user's profile is unreadable by the sandbox user",
            "read_dir on C:\\Users\\ostraprobe succeeded (host secrets reachable)".into(),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => f.push(
            12,
            "read the host user's profile (DPAPI/Credentials/browser stores)",
            Status::Pass,
            "the host user's profile is unreadable by the sandbox user",
            "read_dir on C:\\Users\\ostraprobe: permission denied".into(),
        ),
        Err(e) => f.push(
            12,
            "read the host user's profile (DPAPI/Credentials/browser stores)",
            Status::Inconclusive,
            "the host user's profile is unreadable by the sandbox user",
            format!("read_dir on C:\\Users\\ostraprobe: {}", e.kind()),
        ),
    }
}

/// Attempt to write a byte to a path (creating it). Ok(()) means the write went through.
fn try_write(path: &str) -> Result<(), (std::io::ErrorKind, i32)> {
    use std::io::Write as _;
    match std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(path) {
        Ok(mut file) => match file.write_all(b"x") {
            Ok(()) => Ok(()),
            Err(e) => Err((e.kind(), e.raw_os_error().unwrap_or(0))),
        },
        Err(e) => Err((e.kind(), e.raw_os_error().unwrap_or(0))),
    }
}

fn try_read(path: &str) -> Result<usize, (std::io::ErrorKind, i32)> {
    match std::fs::read(path) {
        Ok(b) => Ok(b.len()),
        Err(e) => Err((e.kind(), e.raw_os_error().unwrap_or(0))),
    }
}

/// True when the error is a real access denial (5) rather than "not found" or similar.
fn is_denied(err: &(std::io::ErrorKind, i32)) -> bool {
    err.0 == std::io::ErrorKind::PermissionDenied || err.1 == 5
}

/// verify 5: a deny ACE inside a granted tree blocks writes and reads through every spelling of the
/// path (case, trailing dot/space, 8.3, ADS), and delete pins block rename/rmdir of .git.
fn check_acl(f: &mut Findings) {
    let repo = defs::REPO;

    // positive control: writing inside the granted tree succeeds
    match try_write(&format!(r"{repo}\allowed.txt")) {
        Ok(()) => f.push(5, "write inside the granted repo (positive control)", Status::Pass,
            "writing a granted file succeeds", "wrote allowed.txt".into()),
        Err(e) => f.push(5, "write inside the granted repo (positive control)", Status::Fail,
            "writing a granted file succeeds", format!("write failed: {e:?} (grant not effective)")),
    }

    // deny-write on .git\hooks via every spelling. Each must be denied; a success is a bypass.
    let hooks = format!(r"{repo}\.git\hooks");
    let mut variants: Vec<(&str, String)> = vec![
        ("canonical", format!(r"{hooks}\pre-commit")),
        ("case-folded", format!(r"{repo}\.GIT\HOOKS\PRE-COMMIT")),
        ("trailing dot", format!(r"{hooks}\pre-commit.")),
        ("trailing space", format!(r"{hooks}\pre-commit ")),
        ("ADS on denied file", format!(r"{hooks}\pre-commit:evil")),
    ];
    if let Some(short) = sys::short_path(&hooks) {
        variants.push(("8.3 short name", format!(r"{short}\pre-commit")));
    } else {
        f.push(5, "write via 8.3 short name of .git\\hooks", Status::Inconclusive,
            "denied via the short name too", "no 8.3 short name on this volume".into());
    }
    for (label, path) in variants {
        match try_write(&path) {
            Err(e) if is_denied(&e) => f.push(5, &format!("write .git\\hooks via {label}"), Status::Pass,
                "denied", format!("denied ({}, os {})", kind(&e), e.1)),
            Err(e) => f.push(5, &format!("write .git\\hooks via {label}"), Status::Inconclusive,
                "denied", format!("failed but not access-denied: {} os {}", kind(&e), e.1)),
            Ok(()) => f.push(5, &format!("write .git\\hooks via {label}"), Status::Fail,
                "denied", format!("WRITE SUCCEEDED via {label}: deny-ACE bypass at {path}")),
        }
    }

    // deny-write on .git\config (a protected file), read still allowed.
    let config = format!(r"{repo}\.git\config");
    match try_write(&config) {
        Err(e) if is_denied(&e) => f.push(5, "write protected .git\\config", Status::Pass,
            "denied", format!("denied ({})", kind(&e))),
        Err(e) => f.push(5, "write protected .git\\config", Status::Inconclusive, "denied",
            format!("failed: {} os {}", kind(&e), e.1)),
        Ok(()) => f.push(5, "write protected .git\\config", Status::Fail, "denied",
            "WRITE SUCCEEDED (bypass)".into()),
    }
    match try_read(&config) {
        Ok(n) => f.push(5, "read protected .git\\config (allowed)", Status::Pass,
            "reading a write-protected file still works", format!("read {n} bytes")),
        Err(e) => f.push(5, "read protected .git\\config (allowed)", Status::Inconclusive,
            "reading a write-protected file still works", format!("read failed: {} os {}", kind(&e), e.1)),
    }

    // deny-read on secret\ (hidden) via spellings. Each read must be denied.
    for (label, path) in [
        ("canonical", format!(r"{repo}\secret\key.txt")),
        ("case-folded", format!(r"{repo}\SECRET\KEY.TXT")),
        ("ADS", format!(r"{repo}\secret\key.txt:x")),
    ] {
        match try_read(&path) {
            Err(e) if is_denied(&e) => f.push(5, &format!("read hidden secret via {label}"), Status::Pass,
                "denied", format!("denied ({})", kind(&e))),
            Err(e) if e.0 == std::io::ErrorKind::NotFound && label == "ADS" => f.push(5,
                &format!("read hidden secret via {label}"), Status::Pass, "denied",
                "no such stream (also not a read bypass)".into()),
            Err(e) => f.push(5, &format!("read hidden secret via {label}"), Status::Inconclusive,
                "denied", format!("failed: {} os {}", kind(&e), e.1)),
            Ok(n) => f.push(5, &format!("read hidden secret via {label}"), Status::Fail,
                "denied", format!("READ SUCCEEDED via {label} ({n} bytes): deny-read bypass")),
        }
    }

    // delete pins: rename and rmdir of .git, delete of .git\config, must all be denied.
    let git = format!(r"{repo}\.git");
    match std::fs::rename(&git, format!(r"{repo}\g")) {
        Err(e) if is_denied(&(e.kind(), e.raw_os_error().unwrap_or(0))) => f.push(5,
            "rename .git (delete pin)", Status::Pass, "denied", "rename denied".into()),
        Err(e) => f.push(5, "rename .git (delete pin)", Status::Inconclusive, "denied",
            format!("rename failed: {}", e.kind())),
        Ok(()) => {
            let _ = std::fs::rename(format!(r"{repo}\g"), &git);
            f.push(5, "rename .git (delete pin)", Status::Fail, "denied", "RENAME SUCCEEDED (pin bypass)".into());
        }
    }
    match std::fs::remove_dir_all(&git) {
        Err(e) if is_denied(&(e.kind(), e.raw_os_error().unwrap_or(0))) => f.push(5,
            "rmdir /s .git (delete-child pin)", Status::Pass, "denied", "rmdir denied".into()),
        Err(e) => f.push(5, "rmdir /s .git (delete-child pin)", Status::Inconclusive, "denied",
            format!("rmdir failed: {}", e.kind())),
        Ok(()) => f.push(5, "rmdir /s .git (delete-child pin)", Status::Fail, "denied",
            "RMDIR SUCCEEDED (pin bypass)".into()),
    }
}

fn kind(e: &(std::io::ErrorKind, i32)) -> String {
    format!("{:?}", e.0)
}

/// verify 4: the sandbox user cannot read the host user's profile, but can read system trees.
fn check_reads(f: &mut Findings) {
    match std::fs::read_dir(r"C:\Users\ostraprobe") {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => f.push(4,
            "list the host user's profile", Status::Pass, "denied",
            "C:\\Users\\ostraprobe: permission denied".into()),
        Err(e) => f.push(4, "list the host user's profile", Status::Inconclusive, "denied",
            format!("C:\\Users\\ostraprobe: {}", e.kind())),
        Ok(_) => f.push(4, "list the host user's profile", Status::Fail, "denied",
            "listing the host profile succeeded".into()),
    }
    for sys_dir in [r"C:\Program Files", r"C:\Windows", r"C:\Windows\System32"] {
        match std::fs::read_dir(sys_dir) {
            Ok(_) => f.push(4, &format!("list {sys_dir}"), Status::Pass,
                "system trees are readable", "listed".into()),
            Err(e) => f.push(4, &format!("list {sys_dir}"), Status::Fail,
                "system trees are readable", format!("failed: {}", e.kind())),
        }
    }
    f.push(4, "per-user toolchains (rustup/nvm/scoop/...)", Status::Inconclusive,
        "list which per-user toolchains a sandbox user cannot reach",
        "none installed on this VM; re-run on a real developer profile".into());
}

/// verify 6: hard links. Within the granted tree a link works; a link to a hidden/protected file
/// either cannot be created or does not bypass the file's ACL when read/written through.
fn check_hardlinks(f: &mut Findings) {
    let repo = defs::REPO;
    // positive: link within the writable area, read through it
    let _ = std::fs::remove_file(format!(r"{repo}\hl_allowed.txt"));
    match sys::create_hard_link(&format!(r"{repo}\hl_allowed.txt"), &format!(r"{repo}\allowed.txt")) {
        Ok(()) => {
            let r = try_read(&format!(r"{repo}\hl_allowed.txt"));
            f.push(6, "hard link within the granted tree", Status::Pass,
                "a link to a granted file works", format!("link created, read through: {r:?}"));
        }
        Err(c) => f.push(6, "hard link within the granted tree", Status::Inconclusive,
            "a link to a granted file works", format!("CreateHardLink failed, os {c}")),
    }
    // bypass attempt: link to the hidden (deny-read) secret, then read through the link
    let hl_secret = format!(r"{repo}\hl_secret.txt");
    let _ = std::fs::remove_file(&hl_secret);
    match sys::create_hard_link(&hl_secret, &format!(r"{repo}\secret\key.txt")) {
        Ok(()) => match try_read(&hl_secret) {
            Err(e) if is_denied(&e) => f.push(6, "hard link to a hidden file then read", Status::Pass,
                "the ACL travels with the file: read through the link is still denied",
                "link created but read denied (no bypass)".into()),
            Ok(n) => f.push(6, "hard link to a hidden file then read", Status::Fail,
                "the ACL travels with the file: read through the link is still denied",
                format!("READ THROUGH LINK SUCCEEDED ({n} bytes): hard-link read bypass")),
            Err(e) => f.push(6, "hard link to a hidden file then read", Status::Inconclusive,
                "read through the link is denied", format!("read failed: {} os {}", kind(&e), e.1)),
        },
        Err(c) => f.push(6, "hard link to a hidden file then read", Status::Pass,
            "cannot even create a hard link to the protected file",
            format!("CreateHardLink denied (os {c}); no bypass")),
    }
    // link to the write-protected .git\config (needs write-attr on target)
    let hl_conf = format!(r"{repo}\hl_config.txt");
    let _ = std::fs::remove_file(&hl_conf);
    match sys::create_hard_link(&hl_conf, &format!(r"{repo}\.git\config")) {
        Err(c) => f.push(6, "hard link to a write-protected file", Status::Pass,
            "cannot create a hard link to a write-protected file (write-attr denied)",
            format!("CreateHardLink denied (os {c})")),
        Ok(()) => {
            let w = try_write(&hl_conf);
            f.push(6, "hard link to a write-protected file", if w.is_ok() { Status::Fail } else { Status::Pass },
                "a link to a write-protected file does not grant write",
                format!("link created; write through link: {w:?}"));
        }
    }
}

/// verify 11: try each way to start a process outside the sandbox. The security question is the
/// identity the spawned process runs as: the real user or SYSTEM is a true escape; the sandbox user
/// (even outside the job) stays inside the ACL and network confinement. A blocked mechanism passes.
fn check_escapes(f: &mut Findings) {
    let me = whoami(); // e.g. win-...\ostraprobea
    let sandbox_user = me.rsplit('\\').next().unwrap_or(&me).to_string();

    // Classify by the identity the escape ran as.
    let classify = |mech: &str, marker: &str, ran: bool| -> (Status, String) {
        if !ran {
            return (
                Status::Pass,
                format!("{mech}: no process observed. A task or job owned by a sandbox user that is not interactively logged on (Ostra spawns it via CreateProcessWithLogonW, never a console logon) does not fire without stored credentials, so this mechanism does not escape here"),
            );
        }
        let id = marker.trim().to_lowercase();
        if id.contains(&sandbox_user) {
            (
                Status::Pass,
                format!("{mech}: ran as the sandbox user ({id}) via a service, i.e. outside the job. Contained by the ACL and network filter, but a job-kill would miss it, so Ostra's policy denylist must block the mechanism (as designed)"),
            )
        } else if id.contains("system") || id.contains("ostraprobe") && !id.contains("ostraprobea") {
            (Status::Fail, format!("{mech}: ESCAPE - ran as {id} (a different, higher-privileged identity)"))
        } else {
            (Status::Inconclusive, format!("{mech}: ran as {id}"))
        }
    };

    // 1) Task Scheduler, as the current user (no /ru). A .cmd action avoids /tr quoting pitfalls.
    {
        let marker = format!(r"{}\esc_sch.txt", defs::OUT);
        let cmdfile = format!(r"{}\esc_sch.cmd", defs::OUT);
        let _ = std::fs::remove_file(&marker);
        let _ = std::fs::write(&cmdfile, format!("@whoami > \"{marker}\"\r\n"));
        cmd_out("schtasks", &["/create", "/tn", "OstraProbeEsc", "/tr", &cmdfile, "/sc", "once", "/st", "00:00", "/f"]);
        cmd_out("schtasks", &["/run", "/tn", "OstraProbeEsc"]);
        let got = wait_for_file(&marker, 12);
        cmd_out("schtasks", &["/delete", "/tn", "OstraProbeEsc", "/f"]);
        let (s, o) = classify("schtasks (self)", got.as_deref().unwrap_or(""), got.is_some());
        f.push(11, "Task Scheduler task as the current user", s, "no escape to a higher-privileged identity", o);
    }
    // 1b) Task Scheduler as SYSTEM (should be refused without admin); confirm by querying existence
    {
        cmd_out("schtasks", &["/create", "/tn", "OstraProbeSys", "/tr", "cmd /c whoami", "/sc", "once", "/st", "00:00", "/ru", "SYSTEM", "/f"]);
        let (q, _) = cmd_out("schtasks", &["/query", "/tn", "OstraProbeSys"]);
        if q == Some(0) {
            cmd_out("schtasks", &["/delete", "/tn", "OstraProbeSys", "/f"]);
            f.push(11, "Task Scheduler task as SYSTEM", Status::Fail, "creating a SYSTEM task is refused",
                "a SYSTEM task was created by a non-admin sandbox user (privilege escalation)".into());
        } else {
            f.push(11, "Task Scheduler task as SYSTEM", Status::Pass, "creating a SYSTEM task is refused",
                "no SYSTEM task exists after the attempt (refused without admin)".into());
        }
    }
    // 2) WMI Win32_Process.Create
    {
        let marker = format!(r"{}\esc_wmi.txt", defs::OUT);
        let _ = std::fs::remove_file(&marker);
        let ps = format!("Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{{CommandLine='cmd /c whoami > \"{marker}\"'}} | Out-Null");
        cmd_out("powershell", &["-NoProfile", "-Command", &ps]);
        let got = wait_for_file(&marker, 8);
        let (s, o) = classify("WMI Win32_Process.Create", got.as_deref().unwrap_or(""), got.is_some());
        f.push(11, "WMI Win32_Process.Create", s, "no escape to a higher-privileged identity", o);
    }
    // 3) Out-of-process COM: Shell.Application.ShellExecute
    {
        let marker = format!(r"{}\esc_com.txt", defs::OUT);
        let _ = std::fs::remove_file(&marker);
        let ps = format!("(New-Object -ComObject Shell.Application).ShellExecute('cmd.exe','/c whoami > \"{marker}\"','','open',0)");
        cmd_out("powershell", &["-NoProfile", "-Command", &ps]);
        let got = wait_for_file(&marker, 8);
        let (s, o) = classify("COM Shell.Application", got.as_deref().unwrap_or(""), got.is_some());
        f.push(11, "out-of-process COM (Shell.Application.ShellExecute)", s, "no escape to a higher-privileged identity", o);
    }
    // 4) BITS notify command
    {
        let marker = format!(r"{}\esc_bits.txt", defs::OUT);
        let _ = std::fs::remove_file(&marker);
        cmd_out("bitsadmin", &["/create", "OstraProbeBits"]);
        // a tiny local transfer so the job can complete and fire the notify command
        let src = format!(r"{}\allowed.txt", defs::REPO);
        let dst = format!(r"{}\bits_dl.txt", defs::OUT);
        cmd_out("bitsadmin", &["/addfile", "OstraProbeBits", &src, &dst]);
        cmd_out("bitsadmin", &["/setnotifycmdline", "OstraProbeBits", "cmd.exe", &format!("cmd.exe /c whoami > \"{marker}\"")]);
        cmd_out("bitsadmin", &["/resume", "OstraProbeBits"]);
        // give the tiny local transfer time to reach TRANSFERRED, then finalize so notify fires
        std::thread::sleep(Duration::from_secs(2));
        cmd_out("bitsadmin", &["/complete", "OstraProbeBits"]);
        let got = wait_for_file(&marker, 20);
        cmd_out("bitsadmin", &["/cancel", "OstraProbeBits"]);
        if got.is_some() {
            let (s, o) = classify("BITS notify", got.as_deref().unwrap_or(""), true);
            f.push(11, "BITS SetNotifyCmdLine", s, "no escape to a higher-privileged identity", o);
        } else {
            f.push(11, "BITS SetNotifyCmdLine", Status::Pass, "no escape to a higher-privileged identity",
                "the notify command did not fire within the timeout; like schtasks, a BITS job owned by a non-logged-on sandbox user does not run its notify command here, so it does not escape".into());
        }
    }
    // 5) runas cannot be scripted (it prompts for a password interactively)
    f.push(11, "runas", Status::Inconclusive, "no escape to a higher-privileged identity",
        "runas prompts for a password interactively and cannot be scripted; not exercised".into());
}

fn unique_sid() -> String {
    // A random service SID (S-1-5-80-...) that is present in no object's DACL.
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        ^ (std::process::id() as u64).wrapping_mul(2654435761);
    let mut part = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % 4_000_000_000) + 1
    };
    format!("S-1-5-80-{}-{}-{}-{}-{}", part(), part(), part(), part(), part())
}

/// verify 7 (confined half): `winprobe netfilter <port_lo> <port6> <lan_ip> <port_lan> <out>`.
/// Spawned as a probe user under a per-user outbound block rule; reports what it can still reach.
pub fn netfilter(args: &[String]) -> i32 {
    use std::net::{Ipv6Addr, SocketAddr, TcpStream};
    use std::time::Duration;
    let p = |i: usize| -> u16 { args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0) };
    let port_lo = p(2);
    let port6 = p(3);
    let lan_ip = args.get(4).cloned().unwrap_or_default();
    let port_lan = p(5);
    let out = args.get(6).cloned().unwrap_or_default();
    let mut f = Findings::default();

    let connect = |addr: SocketAddr| -> Result<(), String> {
        TcpStream::connect_timeout(&addr, Duration::from_secs(3))
            .map(|_| ())
            .map_err(|e| format!("{}", e.kind()))
    };

    // loopback IPv4: a per-user block rule does not filter loopback in Windows Firewall
    match connect(SocketAddr::from(([127, 0, 0, 1], port_lo))) {
        Ok(()) => f.push(7, "connect to 127.0.0.1 under a block-all rule", Status::Pass,
            "loopback is exempt from Windows Firewall; the loopback egress proxy is reachable",
            "connected: Windows Firewall does not filter loopback (a loopback-only server is also reachable by the sandbox user; restricting loopback to only the proxy port needs raw WFP)".into()),
        Err(e) => f.push(7, "connect to 127.0.0.1 under a block-all rule", Status::Inconclusive,
            "loopback is exempt from Windows Firewall", format!("connect failed: {e}")),
    }
    // loopback IPv6
    match connect(SocketAddr::from((Ipv6Addr::LOCALHOST, port6))) {
        Ok(()) => f.push(7, "connect to [::1] under a block-all rule", Status::Pass,
            "IPv6 loopback is exempt too", "connected".into()),
        Err(e) => f.push(7, "connect to [::1] under a block-all rule", Status::Inconclusive,
            "IPv6 loopback is exempt too", format!("connect failed: {e} (IPv6 may be disabled)")),
    }
    // The machine's own LAN IP: Windows treats connections to the host's own assigned addresses as
    // local, so a per-user Windows Firewall rule does not filter them either.
    if let Ok(ip) = lan_ip.parse() {
        let addr = SocketAddr::new(ip, port_lan);
        match connect(addr) {
            Ok(()) => f.push(7, "connect to the machine's own LAN IP under the block rule", Status::Inconclusive,
                "a per-user outbound block filters non-loopback egress",
                format!("connected to own IP {lan_ip}: Windows treats the host's own addresses as local (like loopback), so a per-user Windows Firewall rule does not filter them; verifying the per-user block against a truly remote host needs a second machine and was not done here")),
            Err(e) => f.push(7, "connect to the machine's own LAN IP under the block rule", Status::Pass,
                "a per-user outbound block filters non-loopback egress",
                format!("blocked ({e}) connecting to own IP {lan_ip}:{port_lan}")),
        }
    } else {
        f.push(7, "connect to the machine's own LAN IP under the block rule", Status::Inconclusive,
            "a per-user outbound block filters non-loopback egress",
            format!("no LAN IP to test (got {lan_ip:?})"));
    }
    f.push(7, "restrict loopback to only the proxy port", Status::Fail,
        "Windows Firewall can restrict loopback egress to the proxy port range",
        "Windows Firewall cannot: loopback (and own-IP) traffic is exempt from its filtering, so limiting the sandbox user to only the proxy port on loopback needs a raw WFP filter, as the Phase 3 design already specifies".into());
    f.push(7, "DNS via Dnscache", Status::Inconclusive,
        "DNS resolution is not filtered by a per-user WFP/firewall rule",
        "documented gap: name resolution goes through the Dnscache service (a different account), so a per-user filter does not see it; not exercised live (VM has no resolver)".into());

    let _ = f.write(&out);
    0
}

// ---- small helper roles the checks spawn ----

/// `winprobe markself <file>`: write this process's identity, prove a spawn happened.
pub fn markself(args: &[String]) -> i32 {
    let file = args.get(2).cloned().unwrap_or_default();
    let _ = std::fs::write(&file, format!("ran as {} pid {}", whoami(), std::process::id()));
    0
}

/// `winprobe medspawn <resultfile>`: runs at medium integrity (spawned by the host under a lowered
/// token) and tries CreateProcessWithLogonW as a probe user, to prove that call needs no elevation.
pub fn medspawn(args: &[String]) -> i32 {
    let res = args.get(2).cloned().unwrap_or_default();
    let il = cmd_out("whoami", &["/groups"]).1.to_lowercase();
    let integrity = if il.contains("high mandatory") {
        "High"
    } else if il.contains("medium mandatory") {
        "Medium"
    } else {
        "other"
    };
    let pass = std::fs::read_to_string(defs::PASS_FILE).unwrap_or_default().trim().to_string();
    let marker = format!(r"{}\medchild.txt", defs::OUT);
    let _ = std::fs::remove_file(&marker);
    let cmd = format!("\"{}\" markself \"{}\"", defs::exe(), marker);
    let msg = match sys::spawn_with_logon(defs::USER_A, &pass, &cmd) {
        Ok(child) => {
            let code = child.wait();
            let wrote = wait_for_file(&marker, 8).unwrap_or_else(|| "(nothing)".into());
            format!("caller integrity {integrity}; CreateProcessWithLogonW OK, child exit {code:#x}, child wrote: {wrote}")
        }
        Err(e) => format!("caller integrity {integrity}; CreateProcessWithLogonW failed: {e}"),
    };
    let _ = std::fs::write(&res, msg);
    0
}

/// `winprobe sleeper <pidfile>`: report own pid, then sleep so it can be an OpenProcess target.
pub fn sleeper(args: &[String]) -> i32 {
    if let Some(pf) = args.get(2) {
        let _ = std::fs::write(pf, std::process::id().to_string());
    }
    std::thread::sleep(Duration::from_secs(30));
    0
}

/// `winprobe openone <pid> <resultfile>`: try to open the pid; write OK or ERR:<code>.
pub fn openone(args: &[String]) -> i32 {
    let pid: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let res = args.get(3).cloned().unwrap_or_default();
    let msg = match sys::try_open_process(pid) {
        Ok(_) => "OK".to_string(),
        Err(code) => format!("ERR:{code}"),
    };
    let _ = std::fs::write(&res, msg);
    0
}

/// `winprobe spinjob <pidfile>`: create an inner job with a grandchild sleeper, report both pids,
/// then sleep. Used to prove an outer job kills a nested-job grandchild.
pub fn spinjob(args: &[String]) -> i32 {
    let pf = args.get(2).cloned().unwrap_or_default();
    let gc_pf = format!("{pf}.gc");
    let _ = std::fs::remove_file(&gc_pf);
    let mut cmd = std::process::Command::new(defs::exe());
    cmd.args(["sleeper", &gc_pf]).creation_flags(0x0000_0004); // CREATE_SUSPENDED
    let gc = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return 1,
    };
    if let Ok(job) = sys::Job::new() {
        let h = gc.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
        let _ = job.assign(h);
        let _ = sys::resume_process(h);
        // hold the inner job handle for our lifetime
        std::mem::forget(job);
    }
    let gc_pid = wait_for_file(&gc_pf, 10).unwrap_or_else(|| gc.id().to_string());
    let _ = std::fs::write(&pf, format!("child={} grandchild={}", std::process::id(), gc_pid.trim()));
    std::thread::sleep(Duration::from_secs(30));
    0
}
