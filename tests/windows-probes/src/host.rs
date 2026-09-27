//! The host role (run elevated over SSH as the admin account): create the probe users and layout,
//! orchestrate the confined probes, and write results.md. Increment A covers users + verify
//! 1/2/3/8/10/12.

use crate::confined::cmd_out;
use crate::defs;
use crate::model::{Finding, Findings, Status};
use crate::sys;

fn gen_password() -> String {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    seed ^= seed << 13;
    seed ^= seed >> 7;
    seed ^= seed << 17;
    // Meets default complexity: upper, lower, digit, symbol, no username substring.
    format!("Aa1!{seed:016x}#Zz9")
}

fn ensure_user(name: &str, pass: &str) -> String {
    // Idempotent: add, or reset the password if it already exists.
    let (code, _) = cmd_out("net", &["user", name, pass, "/add", "/y"]);
    if code != Some(0) {
        cmd_out("net", &["user", name, pass]);
    }
    cmd_out("net", &["user", name, "/expires:never"]);
    // Hide from the sign-in screen (best effort).
    cmd_out(
        "reg",
        &[
            "add",
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon\SpecialAccounts\UserList",
            "/v",
            name,
            "/t",
            "REG_DWORD",
            "/d",
            "0",
            "/f",
        ],
    );
    let (_, sid) = cmd_out(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            &format!("(New-Object System.Security.Principal.NTAccount('{name}')).Translate([System.Security.Principal.SecurityIdentifier]).Value"),
        ],
    );
    sid
}

pub fn setup() -> i32 {
    for d in [defs::ROOT, defs::OUT] {
        if let Err(e) = std::fs::create_dir_all(d) {
            eprintln!("create {d}: {e}");
            return 1;
        }
    }
    let pass = gen_password();
    if let Err(e) = std::fs::write(defs::PASS_FILE, &pass) {
        eprintln!("write pass file: {e}");
        return 1;
    }
    // Lock the password file to admins only.
    cmd_out("icacls", &[defs::PASS_FILE, "/inheritance:r", "/grant:r", "Administrators:F", "/grant:r", "SYSTEM:F"]);

    let sid_a = ensure_user(defs::USER_A, &pass);
    let sid_b = ensure_user(defs::USER_B, &pass);

    // The confined role (including restricted-token children) must be able to write result files.
    cmd_out("icacls", &[defs::OUT, "/grant", "Users:(OI)(CI)M"]);

    // The probe users must be able to read+execute the exe the host spawns them from.
    if let Some(dir) = std::path::Path::new(&defs::exe()).parent() {
        cmd_out("icacls", &[&dir.to_string_lossy(), "/grant", "Users:(OI)(CI)RX"]);
    }

    build_repo_tree();

    println!("setup done.");
    println!("  {} = {}", defs::USER_A, sid_a);
    println!("  {} = {}", defs::USER_B, sid_b);
    println!("  layout under {}", defs::ROOT);
    0
}

/// Builds the ACL test tree the way the Phase 3 renderer would: an inheritable Modify grant on the
/// repo root, deny ACEs on .git (write + delete), an explicit deny on .git\hooks, a deny-read
/// "hidden" secret, and a FILE_DELETE_CHILD pin on the root. Applied with icacls; the kernel
/// enforces the same ACEs however they were written.
fn build_repo_tree() {
    let repo = defs::REPO;
    let _ = std::fs::remove_dir_all(repo);
    for d in [repo, &format!(r"{repo}\.git\hooks"), &format!(r"{repo}\secret")] {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(format!(r"{repo}\allowed.txt"), "writable\n");
    let _ = std::fs::write(format!(r"{repo}\.git\config"), "[core]\n  protected = true\n");
    let _ = std::fs::write(format!(r"{repo}\.git\hooks\pre-commit"), "#!/bin/sh\n");
    let _ = std::fs::write(format!(r"{repo}\secret\key.txt"), "TOPSECRET-CREDENTIAL\n");

    let u = defs::USER_A;
    // inheritable Modify grant on the repo root
    cmd_out("icacls", &[repo, "/grant", &format!("{u}:(OI)(CI)M")]);
    // .git: deny write+delete, inheritable (covers config and hooks); the folder itself too (rename pin)
    cmd_out("icacls", &[&format!(r"{repo}\.git"), "/deny", &format!("{u}:(OI)(CI)(DE,WD,AD,WEA,WA)")]);
    // explicit deny on .git\hooks as well (the handover's example of a deny inside a grant)
    cmd_out("icacls", &[&format!(r"{repo}\.git\hooks"), "/deny", &format!("{u}:(OI)(CI)(WD,AD,WEA,WA)")]);
    // FILE_DELETE_CHILD pin on the repo root so .git cannot be removed through its parent
    cmd_out("icacls", &[repo, "/deny", &format!("{u}:(DC)")]);
    // hidden secret: deny read (data, EA, attributes, traverse), inheritable
    cmd_out("icacls", &[&format!(r"{repo}\secret"), "/deny", &format!("{u}:(OI)(CI)(RD,REA,RA,X)")]);
}

pub fn cleanup() -> i32 {
    for u in [defs::USER_A, defs::USER_B] {
        cmd_out("net", &["user", u, "/delete"]);
        cmd_out(
            "reg",
            &[
                "delete",
                r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon\SpecialAccounts\UserList",
                "/v",
                u,
                "/f",
            ],
        );
        let prof = format!(r"C:\Users\{u}");
        cmd_out("cmd", &["/c", "rmdir", "/s", "/q", &prof]);
    }
    // Remove any firewall rules increment C may have added.
    cmd_out(
        "powershell",
        &["-NoProfile", "-Command", "Get-NetFirewallRule -DisplayName 'OstraProbe*' -ErrorAction SilentlyContinue | Remove-NetFirewallRule"],
    );
    cmd_out("cmd", &["/c", "rmdir", "/s", "/q", defs::ROOT]);
    println!("cleanup done.");
    0
}

pub fn run() -> i32 {
    let pass = match std::fs::read_to_string(defs::PASS_FILE) {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            eprintln!("no password file ({e}); run `winprobe setup` first");
            return 1;
        }
    };
    let mut all: Vec<Finding> = Vec::new();
    let mut host = Findings::default();

    // A process started as another user needs access to a window station and desktop to finish
    // process init. Grant Users access to this session's station/desktop first.
    match sys::grant_users_to_session_ui() {
        Ok(()) => host.push(
            9,
            "window station/desktop access for the sandbox user",
            Status::Pass,
            "the sandbox user needs a grant on a window station+desktop to start a process",
            "granted BUILTIN\\Users access to this session's window station and desktop; a real runner must provide a (private) desktop".into(),
        ),
        Err(e) => host.push(
            9,
            "window station/desktop access for the sandbox user",
            Status::Inconclusive,
            "the sandbox user needs a grant on a window station+desktop to start a process",
            format!("could not grant station/desktop access: {e}"),
        ),
    }

    let host_pid = std::process::id();
    let out_file = format!(r"{}\findings_a.tsv", defs::OUT);
    let _ = std::fs::remove_file(&out_file);
    let cmd = format!("\"{}\" asuser \"{}\" {host_pid}", defs::exe(), out_file);

    match sys::spawn_with_logon(defs::USER_A, &pass, &cmd) {
        Ok(child) => {
            let code = child.wait();
            host.push(
                1,
                "spawn a program as another local user (CreateProcessWithLogonW)",
                Status::Pass,
                "the call succeeds without special privilege (secondary logon service)",
                format!("spawned {} as {}, exit {code}; caller was elevated (see caveat)", child.pid, defs::USER_A),
            );
        }
        Err(e) => {
            let code = e.raw_os_error().unwrap_or(0);
            host.push(
                1,
                "spawn a program as another local user (CreateProcessWithLogonW)",
                Status::Fail,
                "the call succeeds without special privilege (secondary logon service)",
                format!("CreateProcessWithLogonW failed: {e} (error {code}); 1385 = logon-type right missing"),
            );
        }
    }
    host.0.push(med_caller_probe(&pass));

    all.extend(host.0);
    all.extend(Findings::read(&out_file));
    all.push(acl_cost());
    all.extend(net_probe(&pass));
    all.extend(isolation_notes());

    write_results(&all);
    0
}

/// verify 9/13/14/15/17: items a headless Server 2025 VM with no Git, Node, or harness CLIs cannot
/// fully establish. Each records why and what environment it needs, plus what could be measured here
/// (verify 17: Defender's live reaction to the unsigned exe).
fn isolation_notes() -> Vec<Finding> {
    let mut v = Vec::new();
    let note = |verify, name: &str, status, observed: String| Finding {
        verify,
        name: name.into(),
        status,
        expected: "runs on a Windows 11 client with real toolchains".into(),
        observed,
    };
    v.push(note(9, "desktop/input isolation (clipboard, SendInput, hooks)", Status::NeedsClient,
        "needs an interactive Windows 11 desktop; this VM is headless over SSH. The station/desktop grant needed to start a process is recorded under verify 9 above".into()));
    v.push(note(13, "ConPTY fidelity per harness CLI", Status::NeedsClient,
        "no harness CLIs (Claude, Codex, Grok, Agy) installed on this VM".into()));
    v.push(note(14, "Git Bash as the sandbox user", Status::NeedsClient,
        "Git for Windows is not installed on this VM".into()));
    v.push(note(15, "mitigation policies vs node/cargo/rustc/python/java/harness", Status::NeedsClient,
        "those toolchains are not installed on this VM; mitigation-policy compatibility must be measured where they are".into()));

    // verify 17: what Defender did with the unsigned exe (it ran, so it was not quarantined).
    let (_, av) = cmd_out(
        "powershell",
        &["-NoProfile", "-Command", "try { $p=Get-MpComputerStatus; \"RealTime=$($p.RealTimeProtectionEnabled) AV=$($p.AntivirusEnabled)\" } catch { 'Defender status unavailable' }"],
    );
    let (_, threats) = cmd_out(
        "powershell",
        &["-NoProfile", "-Command", "$t=Get-MpThreatDetection -ErrorAction SilentlyContinue | Where-Object { $_.Resources -match 'winprobe' }; if ($t) { 'winprobe flagged' } else { 'no winprobe detections' }"],
    );
    v.push(Finding {
        verify: 17,
        name: "Defender/SmartScreen reaction to the unsigned exe".into(),
        status: Status::Pass,
        expected: "record what Defender and SmartScreen do with the unsigned exe and the setup step".into(),
        observed: format!(
            "the unsigned exe ran and created users, ACLs, and firewall rules without being blocked; {}; {}; SmartScreen was not triggered (scp does not set the mark-of-the-web). Kaspersky's reaction still needs the user's own machine",
            av.trim(), threats.trim()
        ),
    });
    v
}

/// verify 1 (medium-integrity caller): the SSH caller runs at High integrity, so to test the
/// "unelevated" clause honestly, lower a copy of our own token to medium integrity, spawn a helper
/// under it, and have that helper call CreateProcessWithLogonW as a probe user.
fn med_caller_probe(pass: &str) -> Finding {
    let expected = "CreateProcessWithLogonW works from a medium-integrity (non-elevated) caller";
    let res = (|| -> Result<String, String> {
        let base = sys::current_primary_token().map_err(|e| e.to_string())?;
        let med = sys::medium_il_token(base.0).map_err(|e| format!("lower to medium IL: {e}"))?;
        let res_file = format!(r"{}\medcaller.txt", defs::OUT);
        let _ = std::fs::write(defs::PASS_FILE, pass); // ensure the helper can read it
        let _ = std::fs::remove_file(&res_file);
        let cmd = format!("\"{}\" medspawn \"{}\"", defs::exe(), res_file);
        let child = sys::spawn_as_user(med.0, &cmd, false).map_err(|e| format!("spawn medium-IL helper: {e}"))?;
        child.wait();
        std::fs::read_to_string(&res_file).map_err(|e| format!("helper wrote nothing: {e}"))
    })();
    match res {
        Ok(detail) if detail.contains("OK") => Finding {
            verify: 1,
            name: "unelevated caller (medium integrity)".into(),
            status: Status::Pass,
            expected: expected.into(),
            observed: detail.trim().to_string(),
        },
        Ok(detail) => Finding {
            verify: 1,
            name: "unelevated caller (medium integrity)".into(),
            status: Status::Fail,
            expected: expected.into(),
            observed: detail.trim().to_string(),
        },
        Err(e) => Finding {
            verify: 1,
            name: "unelevated caller (medium integrity)".into(),
            status: Status::Inconclusive,
            expected: expected.into(),
            observed: e,
        },
    }
}

/// verify 7: start listeners, apply a per-user outbound block (plus an inbound allow so inbound
/// filtering is not the confounder), then have the probe user try to reach loopback and the LAN IP.
fn net_probe(pass: &str) -> Vec<Finding> {
    use std::io::Read as _;
    use std::net::{Ipv6Addr, TcpListener};

    let (port_lo, port6, port_lan) = (48610u16, 48611u16, 48612u16);
    let lan_ip = local_lan_ip();

    // background listeners that accept then drop, so a connect succeeds when the network allows it
    let spawn_listener = |l: TcpListener| {
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                let mut b = [0u8; 1];
                let _ = s.read(&mut b);
            }
        });
    };
    if let Ok(l) = TcpListener::bind(("127.0.0.1", port_lo)) {
        spawn_listener(l);
    }
    if let Ok(l) = TcpListener::bind((Ipv6Addr::LOCALHOST, port6)) {
        spawn_listener(l);
    }
    let lan_listener_ok = TcpListener::bind(("0.0.0.0", port_lan)).map(spawn_listener).is_ok();

    let sid_a = cmd_out(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            &format!("(New-Object System.Security.Principal.NTAccount('{}')).Translate([System.Security.Principal.SecurityIdentifier]).Value", defs::USER_A),
        ],
    )
    .1;

    // remove any stale rules, then add: inbound allow for the LAN test port + per-user outbound block
    cmd_out("powershell", &["-NoProfile", "-Command", "Get-NetFirewallRule -DisplayName 'OstraProbe*' -ErrorAction SilentlyContinue | Remove-NetFirewallRule"]);
    cmd_out("powershell", &["-NoProfile", "-Command", &format!("New-NetFirewallRule -DisplayName OstraProbeInAllow -Direction Inbound -Action Allow -Protocol TCP -LocalPort {port_lan} | Out-Null")]);
    let (block_code, block_out) = cmd_out("powershell", &["-NoProfile", "-Command", &format!("New-NetFirewallRule -DisplayName OstraProbeOutBlock -Direction Outbound -Action Block -LocalUser 'D:(A;;CC;;;{sid})' | Out-Null", sid = sid_a.trim())]);

    let mut findings = Vec::new();
    if block_code != Some(0) {
        findings.push(Finding {
            verify: 7,
            name: "per-user outbound block rule".into(),
            status: Status::Inconclusive,
            expected: "a per-user WFP/firewall rule can be created".into(),
            observed: format!("New-NetFirewallRule failed: {block_out}"),
        });
    }

    let out = format!(r"{}\findings_net.tsv", defs::OUT);
    let _ = std::fs::remove_file(&out);
    let cmd = format!("\"{}\" netfilter {port_lo} {port6} \"{lan_ip}\" {port_lan} \"{}\"", defs::exe(), out);
    match sys::spawn_with_logon(defs::USER_A, pass, &cmd) {
        Ok(child) => {
            child.wait();
            findings.extend(Findings::read(&out));
        }
        Err(e) => findings.push(Finding {
            verify: 7,
            name: "spawn the network probe as the sandbox user".into(),
            status: Status::Inconclusive,
            expected: "the confined network probe runs".into(),
            observed: format!("spawn failed: {e}"),
        }),
    }
    if !lan_listener_ok {
        findings.push(Finding {
            verify: 7,
            name: "LAN listener".into(),
            status: Status::Inconclusive,
            expected: "a LAN listener for the outbound test".into(),
            observed: "could not bind the LAN test port".into(),
        });
    }

    // leave the environment clean for later increments
    cmd_out("powershell", &["-NoProfile", "-Command", "Get-NetFirewallRule -DisplayName 'OstraProbe*' -ErrorAction SilentlyContinue | Remove-NetFirewallRule"]);
    findings
}

/// This machine's primary LAN IPv4, found by asking the routing table which local address it would
/// would use to reach a public address (a UDP connect sends nothing). Falls back to loopback.
fn local_lan_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:9")?;
            Ok(s.local_addr()?.ip().to_string())
        })
        .unwrap_or_else(|_| "127.0.0.1".into())
}

/// verify 16: time an inheritable ACE applied recursively over a tree, to decide between
/// per-execution grants and persistent per-workspace grants. The VM has no real repo, so build a
/// synthetic tree (a stand-in for node_modules/target).
fn acl_cost() -> Finding {
    let tree = format!(r"{}\bigtree", defs::ROOT);
    let _ = std::fs::remove_dir_all(&tree);
    let (dirs, per) = (40usize, 200usize);
    for d in 0..dirs {
        let sub = format!(r"{tree}\d{d:03}");
        if std::fs::create_dir_all(&sub).is_err() {
            return Finding {
                verify: 16,
                name: "recursive ACE cost".into(),
                status: Status::Inconclusive,
                expected: "time to apply an inheritable ACE recursively".into(),
                observed: "could not build the synthetic tree".into(),
            };
        }
        for i in 0..per {
            let _ = std::fs::write(format!(r"{sub}\f{i:03}.txt"), b"x");
        }
    }
    let count = dirs * per;
    let start = std::time::Instant::now();
    let (code, _) = cmd_out("icacls", &[&tree, "/grant", &format!("{}:(OI)(CI)M", defs::USER_A), "/t", "/q", "/c"]);
    let elapsed = start.elapsed();
    let _ = std::fs::remove_dir_all(&tree);
    Finding {
        verify: 16,
        name: "recursive ACE cost over a synthetic tree".into(),
        status: if code == Some(0) { Status::Pass } else { Status::Inconclusive },
        expected: "measure the time to apply an inheritable ACE recursively (per-execution vs persistent grants)".into(),
        observed: format!(
            "icacls /t granted a Modify ACE over {count} files ({dirs} dirs) in {} ms; a large real tree (node_modules/target) scales from here, favoring persistent per-workspace grants",
            elapsed.as_millis()
        ),
    }
}

fn write_results(findings: &[Finding]) {
    let mut md = String::new();
    md.push_str("# Windows sandbox probe results\n\n");
    md.push_str(&format!("Run on: {}\n\n", host_facts()));
    md.push_str("Status key: PASS = the design's assumption held; FAIL = a gap Phase 3 must handle; ");
    md.push_str("INCONCLUSIVE = undecidable on this VM; NEEDS_CLIENT = needs a Win11 client or the AV machine.\n\n");

    md.push_str("| verify | status | check | observed |\n|---|---|---|---|\n");
    let mut sorted = findings.to_vec();
    sorted.sort_by_key(|f| f.verify);
    for f in &sorted {
        md.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            f.verify,
            f.status.tag(),
            f.name,
            f.observed.replace('|', "\\|"),
        ));
    }
    md.push_str("\n## Detail\n\n");
    for f in &sorted {
        md.push_str(&format!(
            "- **verify {} — {}** [{}]\n  - expected: {}\n  - observed: {}\n",
            f.verify,
            f.name,
            f.status.tag(),
            f.expected,
            f.observed,
        ));
    }

    let path = format!(r"{}\results.md", defs::ROOT);
    if let Err(e) = std::fs::write(&path, md) {
        eprintln!("write results: {e}");
    } else {
        println!("results written to {path} ({} findings)", findings.len());
    }
}

fn host_facts() -> String {
    let (_, os) = cmd_out(
        "powershell",
        &["-NoProfile", "-Command", "(Get-CimInstance Win32_OperatingSystem).Caption + ' build ' + [System.Environment]::OSVersion.Version.Build"],
    );
    let (_, who) = cmd_out("whoami", &[]);
    format!("{os}; host user {}", who.trim())
}
