//! Profiles rendered and run under each backend, where this machine has one.

use crate::backend::*;
use crate::cache::*;
use crate::egress;
use crate::git::*;
use crate::members::*;
use crate::profile::*;
use crate::seatbelt::*;
use crate::sys::{Os, Platform};
use ostra_core::config::{PermissionMode, SandboxConfig, SandboxMode, SandboxNetwork};
use ostra_core::exec::ExecContext;
use ostra_core::paths;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn ctx(root: &Path) -> ExecContext {
    ExecContext {
        work_dirs: Vec::new(),
        execution_id: "x_1".into(),
        session_id: None,
        agent: ostra_core::AgentName::Implementer,
        initializer_mode: None,
        executor: ostra_core::ExecutorKind::Native,
        workspace_root: root.to_path_buf(),
        repo_root: root.join("repo"),
        project_key: "p".into(),
        session_dir: root.join(".ostra/sessions/s1"),
        session_root: root.join(".ostra/sessions/s1"),
        report_file: None,
        phase: None,
        yolo: false,
        permission_mode: PermissionMode::Default,
        permissions: Default::default(),
        protected_paths: vec![],
        memory_db: PathBuf::new(),
        sandbox_mode: None,
        enforce_tool_calls: false,
        sandbox_network: None,
        sandbox_allowed_hosts: vec![],
        sandbox_decoys: vec![],
        sandbox_readable: vec![],
        sandbox_loopback: Default::default(),
        sandbox_blocked_ports: vec![],
        creates_project: false,
        owes_reply: false,
        write_scope: None,
        contract: ostra_core::Contract::Stage,
        capabilities: vec![],
    }
}

fn strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// Runs `script` under `profile` and returns its output, or `None` without a sandbox.
fn run_in(profile: &Profile, chdir: &Path, script: &str) -> Option<String> {
    let Some(b) = backend() else {
        eprintln!("no sandbox here ({}); skipping", unavailable_message());
        return None;
    };
    let sc = profile
        .command(b, chdir, OsStr::new("sh"), ["-c", script])
        .unwrap();
    let out = sc.std_command().output().unwrap();
    Some(format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}

fn seatbelt() -> bool {
    backend() == Some(&Backend::Seatbelt)
}

/// A workspace with a git repo and two sessions, and a home folder outside it.
fn layout() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let top = ostra_core::paths::canonical(d.path()).unwrap();
    let root = top.join("w");
    std::fs::create_dir_all(&root).unwrap();
    let ok = std::process::Command::new("git")
        .args(["init", "-q", "repo"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(ok.success());
    std::fs::create_dir_all(root.join(".ostra/sessions/s1")).unwrap();
    std::fs::create_dir_all(root.join(".ostra/sessions/s2/.state/harness/x_2")).unwrap();
    std::fs::write(
        root.join(".ostra/sessions/s2/.state/harness/x_2/config.toml"),
        "TOKEN",
    )
    .unwrap();
    let home = top.join("home");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::create_dir_all(home.join(".cargo/registry")).unwrap();
    std::fs::write(home.join(".cargo/config.toml"), "[build]\n").unwrap();
    std::fs::write(home.join(".bashrc"), "").unwrap();
    (d, root, home)
}

#[test]
fn profile_args_bind_roots_hide_secrets_and_set_caches() {
    let (_d, root, home) = layout();
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
    let args = strings(p.args(&root.join("repo")));
    let has = |pair: &[&str]| args.windows(pair.len()).any(|w| w == pair);
    let s = |p: PathBuf| p.display().to_string();
    assert!(has(&[
        "--bind",
        &s(root.join("repo")),
        &s(root.join("repo"))
    ]));
    assert!(has(&[
        "--bind",
        &s(root.join("repo/.git")),
        &s(root.join("repo/.git"))
    ]));
    assert!(has(&["--tmpfs", &s(home.join(".ssh"))]));
    assert!(has(&["--tmpfs", &s(root.join(".ostra/sessions"))]));
    assert!(has(&["--tmpfs", "/tmp"]));
    assert!(!args.iter().any(|a| a == &s(home.join(".cargo"))));
    let cargo = args
        .windows(3)
        .find(|w| w[0] == "--setenv" && w[1] == "CARGO_HOME")
        .map(|w| PathBuf::from(&w[2]))
        .expect("CARGO_HOME is set");
    assert!(cargo.starts_with(home.join(".cache/ostra/sandbox")));
    assert!(cargo.join("config.toml").is_symlink());
    let berry = args
        .windows(3)
        .find(|w| w[0] == "--setenv" && w[1] == "YARN_GLOBAL_FOLDER")
        .map(|w| PathBuf::from(&w[2]))
        .expect("YARN_GLOBAL_FOLDER is set");
    assert!(berry.starts_with(home.join(".cache/ostra/sandbox")) && berry.is_dir());
    assert!(has(&["--setenv", "TMPDIR", "/tmp"]));
    assert!(has(&["--unshare-pid"]) && has(&["--new-session"]));
    assert!(has(&["--unshare-net"]));
    let host = SandboxConfig {
        network: SandboxNetwork::Host,
        ..Default::default()
    };
    let args = Profile::for_execution(&ctx(&root), &host, &home).args(&root);
    assert!(!args.iter().any(|a| a == "--unshare-net"));
    let off = SandboxConfig {
        network: SandboxNetwork::None,
        ..Default::default()
    };
    let args = Profile::for_execution(&ctx(&root), &off, &home)
        .new_session(false)
        .args(&root);
    assert!(args.iter().any(|a| a == "--unshare-net"));
    assert!(!args.iter().any(|a| a == "--new-session"));
}

#[test]
fn every_work_dir_is_writable_with_its_git_and_memory_protected() {
    // Rule WD1: a work dir outside the workspace root is writable, and its `.git` config and
    // memory database stay read-only.
    let (_d, root, home) = layout();
    let other = root.parent().unwrap().join("api");
    let ok = std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&other)
        .status()
        .unwrap();
    assert!(ok.success());
    let db = paths::project_memory_db(&other);
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    std::fs::write(&db, "").unwrap();
    let mut c = ctx(&root);
    c.work_dirs = vec![
        ostra_core::exec::WorkDir {
            project: "p".into(),
            path: root.join("repo"),
        },
        ostra_core::exec::WorkDir {
            project: "api".into(),
            path: other.clone(),
        },
    ];
    let p = Profile::for_execution(&c, &SandboxConfig::default(), &home);
    let writable = |dir: &Path| {
        p.mounts
            .iter()
            .any(|m| matches!(m, Mount::Writable { src, .. } if src == dir))
    };
    let read_only = |file: &Path| {
        p.mounts
            .iter()
            .any(|m| matches!(m, Mount::ReadOnly { path, .. } if path == file))
    };
    assert!(writable(&root) && writable(&root.join("repo")) && writable(&other));
    assert!(read_only(&other.join(".git/config")));
    assert!(read_only(&root.join("repo/.git/config")));
    assert!(read_only(&db));
    let alone = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
    assert!(
        !alone
            .mounts
            .iter()
            .any(|m| matches!(m, Mount::Writable { src, .. } if src == &other))
    );
}

#[test]
fn planted_commondir_files_are_undone() {
    let d = tempfile::tempdir().unwrap();
    let repo = d.path().join("repo");
    let git = repo.join(".git");
    std::fs::create_dir_all(git.join("worktrees/wt")).unwrap();
    std::fs::create_dir_all(git.join("worktrees/ok")).unwrap();
    std::fs::create_dir_all(git.join("modules/sub")).unwrap();
    std::fs::write(git.join("config"), "").unwrap();
    std::fs::write(git.join("modules/sub/config"), "").unwrap();
    std::fs::write(git.join("commondir"), "/tmp/evil\n").unwrap();
    std::fs::write(git.join("modules/sub/commondir"), "/tmp/evil\n").unwrap();
    std::fs::write(git.join("worktrees/wt/commondir"), "/tmp/evil\n").unwrap();
    std::fs::write(git.join("worktrees/ok/commondir"), "../..\n").unwrap();
    let mut changed = repair_git_dirs(&git_repos(&[&repo]));
    changed.sort();
    let real = ostra_core::paths::canonical(&git).unwrap();
    assert_eq!(
        changed,
        vec![
            real.join("commondir"),
            real.join("modules/sub/commondir"),
            real.join("worktrees/wt/commondir"),
        ]
    );
    assert!(!git.join("commondir").exists());
    assert_eq!(
        std::fs::read_to_string(git.join("worktrees/wt/commondir")).unwrap(),
        "../..\n"
    );
    assert!(
        repair_git_dirs(&git_repos(&[&repo])).is_empty(),
        "a repaired repo stays quiet"
    );
}

#[test]
fn repos_are_found_at_any_depth_but_not_in_build_output() {
    let d = tempfile::tempdir().unwrap();
    let root = ostra_core::paths::canonical(d.path()).unwrap();
    for dir in [
        ".git",
        "a/.git",
        "a/b/c/.git",
        "node_modules/x/.git",
        "target/y/.git",
        "a/.git/modules/m",
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("target/CACHEDIR.TAG"), "").unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/.git"), "gitdir: ../a/.git/modules/m\n").unwrap();
    std::os::unix::fs::symlink(root.join("a"), root.join("link")).unwrap();
    assert_eq!(
        git_repos(&[&root, &root.join("a")]),
        vec![
            root.clone(),
            root.join("a"),
            root.join("sub"),
            root.join("a/b/c")
        ]
    );
    let sub = GitDir::of(&root.join("sub")).unwrap();
    assert_eq!(sub.dir, root.join("a/.git/modules/m"));
    assert!(sub.file && !sub.linked);
}

#[test]
fn nested_repos_and_git_files_are_protected_and_pinned() {
    let (_d, root, home) = layout();
    let repo = root.join("repo");
    let deep = repo.join("vendor/lib/dep");
    std::fs::create_dir_all(&deep).unwrap();
    let git = |dir: &Path, args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(ok.status.success(), "{args:?}: {ok:?}");
    };
    git(&deep, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=a",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "i",
        ],
    );
    git(&repo, &["worktree", "add", "-q", "wt"]);
    let gitfile = std::fs::read_to_string(repo.join("wt/.git")).unwrap();
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
    let script = format!(
        r#"
        git -C {deep} config core.fsmonitor x 2>/dev/null || echo deep-config-read-only
        echo 'gitdir: /tmp' > wt/.git 2>/dev/null || echo git-file-read-only
        mv {deep} {deep}-moved 2>/dev/null || echo deep-repo-not-renamed
        mv vendor vendor-moved 2>/dev/null || echo ancestor-not-renamed
        mv wt wt-moved 2>/dev/null || echo worktree-not-renamed
        echo a > vendor/f && echo other-writes-stay
        "#,
        deep = deep.display(),
    );
    let Some(out) = run_in(&p, &repo, &script) else {
        return;
    };
    for want in [
        "deep-config-read-only",
        "git-file-read-only",
        "deep-repo-not-renamed",
        "ancestor-not-renamed",
        "worktree-not-renamed",
        "other-writes-stay",
    ] {
        assert!(out.contains(want), "missing {want}:\n{out}");
    }
    assert!(deep.join(".git/config").is_file());
    assert_eq!(
        std::fs::read_to_string(repo.join("wt/.git")).unwrap(),
        gitfile
    );
}

#[test]
fn names_on_disk_use_a_stable_hash() {
    assert_ne!(
        stable_hex("", Path::new("/w")),
        stable_hex("agents", Path::new("/w"))
    );
    // SHA-256 of one NUL byte: a change here moves every user's caches and markers.
    assert_eq!(stable_hex("", Path::new("")), "6e340b9cffb37a98");
}

#[test]
fn each_session_gets_its_own_caches_apart_from_programs() {
    let (_d, root, home) = layout();
    let cargo = |p: Profile| {
        strings(p.args(&root))
            .windows(3)
            .find(|w| w[0] == "--setenv" && w[1] == "CARGO_HOME")
            .map(|w| PathBuf::from(&w[2]))
            .unwrap()
    };
    let cfg = SandboxConfig::default();
    let in_session = |id: &str| ExecContext {
        session_id: Some(id.into()),
        ..ctx(&root)
    };
    let s1 = cargo(Profile::for_execution(&in_session("s_1"), &cfg, &home));
    let s2 = cargo(Profile::for_execution(&in_session("s_2"), &cfg, &home));
    assert_eq!(s1, session_cache(&home, &"s_1".into()).join("cargo"));
    assert_ne!(s1, s2);
    let outside = cargo(Profile::for_execution(&ctx(&root), &cfg, &home));
    let program = cargo(Profile::for_program(&[&root], &cfg, &home));
    assert_ne!(outside, program, "agents never write a program's cache");
    assert!(![&s1, &s2].contains(&&outside));
}

#[test]
fn opening_a_decoy_is_reported_and_listing_is_not() {
    let (_d, root, home) = layout();
    std::fs::write(
        home.join(".git-credentials"),
        "https://u:real@example.invalid\n",
    )
    .unwrap();
    std::fs::write(home.join(".ssh/id_rsa"), "real key\n").unwrap();
    let Some(b) = backend().filter(|b| matches!(b, Backend::Bubblewrap(_))) else {
        return;
    };
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    let s = seen.clone();
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
        .watch_decoys(b, &home, &[], Arc::new(move |p| s.lock().unwrap().push(p)))
        .unwrap();
    let script = format!(
        "ls -la {ssh} >/dev/null; stat {ssh}/id_ed25519 >/dev/null && echo stat-ok
        cat {ssh}/id_rsa | head -1; cat {ssh}/id_rsa {creds} | grep -c real",
        ssh = home.join(".ssh").display(),
        creds = home.join(".git-credentials").display(),
    );
    let out = run_in(&p, &root.join("repo"), &script).unwrap();
    assert!(out.contains("stat-ok"), "{out}");
    assert!(out.contains("-----BEGIN OPENSSH PRIVATE KEY-----"), "{out}");
    assert!(
        out.contains("\n0"),
        "the real credentials stay hidden: {out}"
    );
    let start = std::time::Instant::now();
    while seen.lock().unwrap().len() < 2 && start.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    let mut got = seen.lock().unwrap().clone();
    got.sort();
    assert_eq!(
        got,
        vec![home.join(".git-credentials"), home.join(".ssh/id_rsa")]
    );
}

#[test]
fn workspace_decoys_cover_files_and_fill_hidden_dirs_only() {
    let (_d, root, home) = layout();
    std::fs::create_dir_all(home.join(".aws")).unwrap();
    std::fs::create_dir_all(home.join(".config/app")).unwrap();
    std::fs::write(home.join(".config/app/token"), "real\n").unwrap();
    std::fs::create_dir_all(home.join(".visible")).unwrap();
    let Some(b) = backend().filter(|b| matches!(b, Backend::Bubblewrap(_))) else {
        return;
    };
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    let s = seen.clone();
    let extra = [
        "~/.aws/sso/cache/x.json".to_string(),
        "~/.config/app/token".to_string(),
        "~/.visible/missing".to_string(),
    ];
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
        .watch_decoys(
            b,
            &home,
            &extra,
            Arc::new(move |p| s.lock().unwrap().push(p)),
        )
        .unwrap();
    let script = format!(
        "cat {aws} {token} | grep -c real; cat {missing} 2>/dev/null || echo skipped",
        aws = home.join(".aws/sso/cache/x.json").display(),
        token = home.join(".config/app/token").display(),
        missing = home.join(".visible/missing").display(),
    );
    let out = run_in(&p, &root.join("repo"), &script).unwrap();
    assert!(out.starts_with("0\n"), "the real token is covered: {out}");
    assert!(out.contains("skipped"), "{out}");
    assert!(
        !home.join(".visible/missing").exists(),
        "the host is never written"
    );
    let start = std::time::Instant::now();
    while seen.lock().unwrap().len() < 2 && start.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    let mut got = seen.lock().unwrap().clone();
    got.sort();
    assert_eq!(
        got,
        vec![
            home.join(".aws/sso/cache/x.json"),
            home.join(".config/app/token")
        ]
    );
}

#[test]
fn readable_credentials_are_shown_read_only_and_get_no_decoy() {
    let (_d, root, home) = layout();
    std::fs::write(
        home.join(".netrc"),
        "machine repo.corp login u password real\n",
    )
    .unwrap();
    std::fs::create_dir_all(home.join(".docker")).unwrap();
    std::fs::write(home.join(".docker/config.json"), "real\n").unwrap();
    std::fs::write(home.join(".docker/other.json"), "real\n").unwrap();
    std::fs::write(home.join(".git-credentials"), "real\n").unwrap();
    let never = [home.join(".ostra-data")];
    std::fs::create_dir_all(&never[0]).unwrap();
    let entries: Vec<String> = [
        "~/.netrc",
        "~/.docker/config.json",
        "~/.git-credentials",
        "~/",
        "~/.ostra-data",
        "~/.missing",
    ]
    .iter()
    .map(|e| e.to_string())
    .collect();
    assert_eq!(
        crate::profile::readable_paths(&home, &entries, &never),
        vec![
            home.join(".docker/config.json"),
            home.join(".git-credentials"),
            home.join(".netrc")
        ]
    );
    let cfg = SandboxConfig {
        extra_readable: entries[..3].to_vec(),
        ..Default::default()
    };
    let mut p = Profile::for_execution(&ctx(&root), &cfg, &home);
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    if let Some(b) = backend().filter(|b| matches!(b, Backend::Bubblewrap(_))) {
        let s = seen.clone();
        p = p
            .watch_decoys(b, &home, &[], Arc::new(move |p| s.lock().unwrap().push(p)))
            .unwrap();
    }
    let script = format!(
        "cat {netrc} {cfg} {creds} | grep -c real; cat {other} 2>/dev/null | grep -c real; \
         echo x > {netrc} 2>/dev/null && echo wrote",
        netrc = home.join(".netrc").display(),
        cfg = home.join(".docker/config.json").display(),
        creds = home.join(".git-credentials").display(),
        other = home.join(".docker/other.json").display(),
    );
    let Some(out) = run_in(&p, &root.join("repo"), &script) else {
        return;
    };
    assert!(out.starts_with("3\n0\n"), "{out}");
    assert!(!out.contains("wrote"), "readable stays read-only: {out}");
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        seen.lock().unwrap().is_empty(),
        "no decoy on a readable file"
    );
}

#[test]
fn deeper_rules_win_and_git_stays_usable() {
    let (_d, root, home) = layout();
    let own = paths::harness_execution_dir(&root.join(".ostra/sessions/s1"), "x_1");
    let scratch = root.join("scratch");
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
        .scratch(&scratch)
        .unwrap()
        .writable(&{
            std::fs::create_dir_all(&own).unwrap();
            own.clone()
        });
    let script = format!(
        r#"
        touch {own}/ok && echo own-harness-writable
        touch {state}/x 2>/dev/null || echo state-read-only
        cat {other} 2>/dev/null || echo other-session-hidden
        echo a > f && git add f && git -c user.email=a@b -c user.name=a commit -qm t && echo committed
        git config core.fsmonitor x 2>/dev/null || echo git-config-read-only
        touch .git/hooks/pre-commit 2>/dev/null || echo hooks-read-only
        mv .git .git-moved 2>/dev/null || echo git-not-renamed
        echo x >> {rc} 2>/dev/null || echo rc-read-only
        touch {cargo_reg}/x 2>/dev/null || echo cargo-read-only
        echo t > "${{TMPDIR:-/tmp}}/t" && cat {scratch}/t && echo scratch-shared
        "#,
        own = own.display(),
        state = root.join(".ostra/sessions/s1/.state").display(),
        other = root
            .join(".ostra/sessions/s2/.state/harness/x_2/config.toml")
            .display(),
        rc = home.join(".bashrc").display(),
        cargo_reg = home.join(".cargo/registry").display(),
        scratch = scratch.display(),
    );
    let Some(out) = run_in(&p, &root.join("repo"), &script) else {
        return;
    };
    for want in [
        "own-harness-writable",
        "state-read-only",
        "other-session-hidden",
        "committed",
        "git-config-read-only",
        "hooks-read-only",
        "git-not-renamed",
        "rc-read-only",
        "cargo-read-only",
        "scratch-shared",
    ] {
        assert!(out.contains(want), "missing {want}:\n{out}");
    }
    assert!(!out.contains("TOKEN"));
    assert!(own.join("ok").exists());
    assert!(scratch.join("t").exists());
    assert!(root.join("repo/.git").is_dir());
    assert!(!root.join("repo/.git/hooks/pre-commit").exists());
    if !seatbelt() {
        assert_eq!(p.to_host(Path::new("/tmp/a/b")), scratch.join("a/b"));
    }
}

#[test]
fn missing_protected_paths_get_read_only_placeholders_under_writable_dirs() {
    let (_d, root, home) = layout();
    let claude = home.join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    let cfg = SandboxConfig {
        extra_writable: vec!["~/.config".into()],
        ..Default::default()
    };
    std::fs::create_dir_all(home.join(".config")).unwrap();
    let p = Profile::for_execution(&ctx(&root), &cfg, &home)
        .writable(&claude)
        .read_only(&claude.join("settings.json"))
        .read_only_dir(&claude.join("hooks"));
    let script = format!(
        r#"
        echo '{{"hooks":1}}' > {settings} 2>/dev/null || echo settings-refused
        touch {hooks}/h 2>/dev/null || echo hooks-refused
        mkdir -p {fish} && touch {fish}/config.fish 2>/dev/null || echo fish-refused
        touch {claude}/other && echo claude-writable
        "#,
        settings = claude.join("settings.json").display(),
        hooks = claude.join("hooks").display(),
        fish = home.join(".config/fish").display(),
        claude = claude.display(),
    );
    let Some(out) = run_in(&p, &root, &script) else {
        return;
    };
    for want in [
        "settings-refused",
        "hooks-refused",
        "fish-refused",
        "claude-writable",
    ] {
        assert!(out.contains(want), "missing {want}:\n{out}");
    }
    // Bubblewrap mounts a placeholder; Seatbelt's literal rule refuses the missing path.
    if seatbelt() {
        assert!(!claude.join("settings.json").exists());
    } else {
        assert_eq!(
            std::fs::read_to_string(claude.join("settings.json")).unwrap(),
            "{}"
        );
    }
    assert!(!claude.join("hooks/h").exists());
    assert!(!home.join(".config/fish/config.fish").exists());
    // A home that is read-only anyway gets no placeholder files.
    assert!(!home.join(".zshrc").exists());
}

#[test]
fn a_harness_keeps_only_its_own_sign_in_file() {
    let (_d, root, home) = layout();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(home.join(".claude/.credentials.json"), "{}").unwrap();
    std::fs::write(home.join(".codex/auth.json"), "{}").unwrap();
    let mut c = ctx(&root);
    c.executor = ostra_core::ExecutorKind::Harness(ostra_core::HarnessKind::Claude);
    let args = strings(Profile::for_execution(&c, &SandboxConfig::default(), &home).args(&root));
    let hidden = |f: &str| {
        let f = home.join(f).display().to_string();
        args.windows(3)
            .any(|w| w[0] == "--ro-bind" && w[1] == "/dev/null" && w[2] == f)
    };
    assert!(!hidden(".claude/.credentials.json"));
    assert!(hidden(".codex/auth.json"));
    let args =
        strings(Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home).args(&root));
    assert!(args.windows(3).any(|w| w[1] == "/dev/null"
        && w[2] == home.join(".claude/.credentials.json").display().to_string()));
}

#[cfg(target_os = "linux")]
#[test]
fn a_private_network_leads_out_only_through_its_own_proxy() {
    let Some(b @ Backend::Bubblewrap(_)) = backend() else {
        eprintln!("bubblewrap unavailable; skipping");
        return;
    };
    let (_d, root, home) = layout();
    let host = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = host.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for mut s in host.incoming().flatten() {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ =
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    let unlisted = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let unlisted_port = unlisted.local_addr().unwrap().port();
    let abs = format!("ostra-test-{}", uuid::Uuid::new_v4().simple());
    let abs_listener = {
        use std::os::linux::net::SocketAddrExt;
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(&abs).unwrap();
        std::os::unix::net::UnixListener::bind_addr(&addr).unwrap()
    };
    let cfg = SandboxConfig {
        allowed_hosts: vec![format!("127.0.0.1:{port}")],
        ..Default::default()
    };
    let seen: Arc<std::sync::Mutex<Vec<egress::Decision>>> = Arc::default();
    let s2 = seen.clone();
    let other = Profile::for_execution(&ctx(&root), &cfg, &home)
        .start_egress(b, Arc::new(|_| {}))
        .unwrap();
    let p = Profile::for_execution(&ctx(&root), &cfg, &home)
        .start_egress(b, Arc::new(move |d| s2.lock().unwrap().push(d)))
        .unwrap();
    let script = format!(
        r#"
        bash -c 'exec 3<>/dev/tcp/127.0.0.1/{unlisted_port}' 2>/dev/null && echo direct-loopback
        echo "listed-forwarded:$(curl -s --max-time 5 http://127.0.0.1:{port}/)"
        python3 -c 'import socket; s=socket.socket(socket.AF_UNIX); s.connect("\0{abs}")' 2>/dev/null && echo abstract-socket
        echo "via-proxy:$(curl -s --max-time 5 --noproxy '' -x "$HTTP_PROXY" http://127.0.0.1:{port}/)"
        echo "refused:$(curl -s -o /dev/null -w '%{{http_code}}' --max-time 5 --noproxy '' -x "$HTTP_PROXY" http://127.0.0.1:1/)"
        ls {dir}
        "#,
        dir = egress::socket_dir().display(),
    );
    let out = run_in(&p, &root, &script).unwrap();
    assert!(!out.contains("direct-loopback"), "{out}");
    assert!(!out.contains("abstract-socket"), "{out}");
    assert!(out.contains("via-proxy:ok"), "{out}");
    assert!(out.contains("listed-forwarded:ok"), "{out}");
    assert!(out.contains("refused:403"), "{out}");
    let own = p.egress.as_ref().unwrap().socket().file_name().unwrap();
    let theirs = other.egress.as_ref().unwrap().socket().file_name().unwrap();
    assert!(out.contains(own.to_str().unwrap()), "{out}");
    assert!(!out.contains(theirs.to_str().unwrap()), "{out}");
    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().any(|d| d.allowed && d.port == port), "{seen:?}");
    assert!(
        seen.iter().any(|d| !d.allowed && d.local && d.port == 1),
        "{seen:?}"
    );

    let open = SandboxConfig {
        network: SandboxNetwork::Host,
        ..Default::default()
    };
    let p = Profile::for_execution(&ctx(&root), &open, &home);
    let out = run_in(
        &p,
        &root,
        &format!(
            "bash -c 'exec 3<>/dev/tcp/127.0.0.1/{unlisted_port}' && echo direct-loopback\npython3 -c 'import socket; s=socket.socket(socket.AF_UNIX); s.connect(\"\\0{abs}\")' && echo abstract-socket"
        ),
    )
    .unwrap();
    assert!(
        out.contains("direct-loopback") && out.contains("abstract-socket"),
        "{out}"
    );
    drop((abs_listener, unlisted));
}

#[test]
fn a_sandboxed_command_cannot_make_new_user_namespaces() {
    let Some(Backend::Bubblewrap(bwrap)) = backend() else {
        eprintln!("bubblewrap unavailable; skipping");
        return;
    };
    let (_d, root, home) = layout();
    let script = format!(
        r#"
        unshare -U true 2>/dev/null && echo nested-userns
        {bwrap} --ro-bind / / true 2>/dev/null && echo nested-bwrap
        python3 -c 'import os; os.unshare(os.CLONE_NEWUSER)' 2>&1 | grep -q 'not permitted' && echo userns-eperm
        python3 -c 'import fcntl, termios; fcntl.ioctl(0, termios.TIOCSTI, b"x")' 2>&1 | grep -q 'not permitted' && echo tiocsti-eperm
        python3 -c 'import os, threading; t = threading.Thread(target=print); t.start(); t.join(); os._exit(0) if os.fork() == 0 else os.wait()' && echo fork-ok
        echo "pipe:$(echo a | tr a b)"
        "#,
        bwrap = bwrap.bin().display(),
    );
    for network in [SandboxNetwork::Allowlist, SandboxNetwork::Host] {
        let cfg = SandboxConfig {
            network,
            ..Default::default()
        };
        let p = Profile::for_execution(&ctx(&root), &cfg, &home);
        let out = run_in(&p, &root, &script).unwrap();
        assert!(!out.contains("nested-userns"), "{network:?}: {out}");
        assert!(!out.contains("nested-bwrap"), "{network:?}: {out}");
        for want in ["userns-eperm", "tiocsti-eperm", "fork-ok", "pipe:b"] {
            assert!(out.contains(want), "{network:?}: missing {want}:\n{out}");
        }
    }
}

#[test]
fn decide_follows_the_mode() {
    let off = SandboxConfig {
        mode: SandboxMode::Off,
        ..Default::default()
    };
    assert_eq!(decide(&off), Ok(Decision::Unsandboxed(None)));
    let required = SandboxConfig {
        mode: SandboxMode::Required,
        ..Default::default()
    };
    match backend() {
        Some(b) => assert_eq!(decide(&required), Ok(Decision::Sandboxed(b.clone()))),
        None => assert!(decide(&required).is_err()),
    }
}

fn policy(p: &Profile) -> String {
    p.seatbelt(&Members {
        own: "dev.ostra.sandbox.test".into(),
        group: "dev.ostra.sandbox.dtest".into(),
        listeners: vec![],
        decoys: None,
    })
    .unwrap()
}

#[test]
fn sbpl_strings_cannot_break_out() {
    assert_eq!(sbpl_str("/a b").unwrap(), r#""/a b""#);
    assert_eq!(
        sbpl_str(r#"/tmp/a") (allow file-write* (subpath "/"#).unwrap(),
        r#""/tmp/a\") (allow file-write* (subpath \"/""#
    );
    assert_eq!(sbpl_str(r"/a\b").unwrap(), r#""/a\\b""#);
    assert!(sbpl_str("/a\nb").is_err());
    assert!(sbpl_str("/a\u{7}b").is_err());
}

#[test]
fn seatbelt_policy_orders_rules_and_pins_protected_paths() {
    let (_d, root, home) = layout();
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
    let text = policy(&p);
    let s = |p: PathBuf| p.display().to_string();
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("missing {needle}:\n{text}"))
    };
    let repo = s(root.join("repo"));
    let git = s(root.join("repo/.git"));
    let writable_repo = at(&format!(
        "(allow file-read* file-write* (subpath \"{repo}\"))"
    ));
    let config = at(&format!(
        "(deny file-write* network-bind (literal \"{git}/config\"))"
    ));
    assert!(writable_repo < config);
    at(&format!(
        "(deny file-write* network-bind (subpath \"{git}/hooks\"))"
    ));
    let sessions = s(root.join(".ostra/sessions"));
    let hidden = at(&format!(
        "(deny file-read* file-write* (subpath \"{sessions}\"))"
    ));
    let own = at(&format!(
        "(allow file-read* file-write* (subpath \"{sessions}/s1\"))"
    ));
    assert!(hidden < own);
    at(&format!(
        "(deny file-read* file-write* (subpath \"{}\"))",
        s(home.join(".ssh"))
    ));
    // `stat` on the dirs above an allowed session, even inside the hidden sessions dir.
    let meta = at("(allow file-read-metadata");
    assert!(meta > own && text[meta..].contains(&format!("(literal \"{sessions}\")")));
    let pins = &text[at("(deny file-write-unlink")..];
    for pinned in [&git, &repo, &s(root.join(".ostra")), &sessions] {
        assert!(
            pins.contains(&format!("(literal \"{pinned}\")")),
            "{pinned} not pinned:\n{pins}"
        );
    }
    assert!(!pins.contains(&format!("(literal \"{}\")", s(home.clone()))));
    assert!(text.contains("dev.ostra.sandbox.test"));
    assert!(!text.contains("(param"));
    // Under `allowlist` the loopback is open by default, the rest goes through the proxy, and
    // names do not resolve.
    assert!(!text.contains("(remote ip)"), "{text}");
    assert!(!text.contains("mDNSResponder"), "{text}");
    assert!(text.contains("(allow network-bind (local ip \"localhost:*\"))"));
    assert!(text.contains("(allow network-outbound (remote ip \"localhost:*\"))"));
    let blocked = SandboxConfig {
        blocked_ports: vec![5432],
        ..Default::default()
    };
    let text = policy(&Profile::for_execution(&ctx(&root), &blocked, &home));
    let open = text
        .find("(allow network-outbound (remote ip \"localhost:*\"))")
        .unwrap();
    let deny = text
        .find("(deny network-outbound (remote ip \"localhost:5432\"))")
        .unwrap();
    assert!(open < deny, "{text}");
    let listed = SandboxConfig {
        allowed_hosts: vec![
            "127.0.0.1:8317".into(),
            "127.0.0.1:5432".into(),
            "pkg.example".into(),
        ],
        loopback: ostra_core::config::LoopbackAccess::Listed,
        blocked_ports: vec![5432],
        ..Default::default()
    };
    let text = policy(&Profile::for_execution(&ctx(&root), &listed, &home));
    assert!(text.contains("(allow network-outbound (remote ip \"localhost:8317\"))"));
    assert!(!text.contains("(allow network-outbound (remote ip \"localhost:5432\"))"));
    assert_eq!(
        text.matches("(allow network-outbound (remote ip").count(),
        1,
        "{text}"
    );
    let off = SandboxConfig {
        network: SandboxNetwork::None,
        ..Default::default()
    };
    let text = policy(&Profile::for_execution(&ctx(&root), &off, &home).tty(true));
    assert!(!text.contains("(remote ip"));
    assert!(text.contains("(literal (param \"TTY\"))"));
    let host = SandboxConfig {
        network: SandboxNetwork::Host,
        ..Default::default()
    };
    let text = policy(&Profile::for_execution(&ctx(&root), &host, &home));
    assert!(text.contains("(allow network-outbound (remote ip))"));
    assert!(text.contains("mDNSResponder"));
}

#[test]
fn seatbelt_command_sets_tmpdir_and_unsets_agent_sockets() {
    let (_d, root, home) = layout();
    let scratch = root.join("scratch");
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
        .scratch(&scratch)
        .unwrap();
    let sc = p
        .command(&Backend::Seatbelt, &root, OsStr::new("true"), ["a"])
        .unwrap();
    assert_eq!(sc.program, Path::new(SANDBOX_EXEC));
    assert_eq!(sc.args[0], "-p");
    assert_eq!(
        &sc.args[2..],
        &[OsString::from("true"), OsString::from("a")]
    );
    assert!(
        sc.env
            .contains(&("TMPDIR".into(), format!("{}/", scratch.display())))
    );
    assert!(sc.env.iter().any(|(k, _)| k == "CARGO_HOME"));
    assert!(sc.env_remove.iter().any(|k| k == "SSH_AUTH_SOCK"));
    assert!(sc.env_remove.iter().any(|k| k == "DOCKER_HOST"));
    assert!(sc.members.is_some());
    let bad = root.join("a\nb");
    std::fs::create_dir_all(&bad).unwrap();
    let p = p.writable(&bad);
    assert!(
        p.command(&Backend::Seatbelt, &root, OsStr::new("true"), ["a"])
            .is_err()
    );
}

/// Everything macOS offers for reaching outside a Seatbelt sandbox, refused.
#[test]
fn seatbelt_refuses_escapes_to_the_rest_of_the_user_session() {
    if !seatbelt() {
        eprintln!("seatbelt unavailable; skipping");
        return;
    }
    let (_d, root, home) = layout();
    let label = format!("dev.ostra.test.{}", uuid::Uuid::new_v4().simple());
    let escaped = root.join("escaped");
    let script = format!(
        r#"
        launchctl submit -l {label} -- /usr/bin/touch {escaped} 2>/dev/null && echo launchd-submitted
        open -a Calculator 2>/dev/null && echo opened-app
        osascript -e 'tell application "Finder" to get name of startup disk' 2>/dev/null && echo apple-event
        defaults write {label} k -string v 2>/dev/null && echo pref-written
        security list-keychains 2>/dev/null | grep -q keychain && echo keychain-reached
        pbpaste >/dev/null 2>&1 && echo clipboard-read
        echo x >> /dev/tty 2>/dev/null && echo tty-written
        echo done
        "#,
        escaped = escaped.display()
    );
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
    let out = run_in(&p, &root.join("repo"), &script).unwrap();
    let _ = std::process::Command::new("launchctl")
        .args(["remove", &label])
        .output();
    let _ = std::process::Command::new("defaults")
        .args(["delete", &label])
        .output();
    assert!(out.contains("done"), "{out}");
    for bad in [
        "launchd-submitted",
        "opened-app",
        "apple-event",
        "pref-written",
        "keychain-reached",
        "clipboard-read",
        "tty-written",
    ] {
        assert!(!out.contains(bad), "{bad}:\n{out}");
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!escaped.exists());
}

#[test]
fn seatbelt_keeps_protected_paths_under_renames_links_and_case() {
    if !seatbelt() {
        eprintln!("seatbelt unavailable; skipping");
        return;
    }
    let (_d, root, home) = layout();
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/id"), "SECRET").unwrap();
    let data_home = Path::new("/System/Volumes/Data").join(home.strip_prefix("/").unwrap());
    let script = format!(
        r#"
        mv .git .g 2>/dev/null && echo git-renamed
        mv ../repo ../r2 2>/dev/null && echo repo-renamed
        rm -rf ../.ostra/sessions 2>/dev/null
        test -d ../.ostra/sessions/s2 && echo other-session-visible
        echo x >> .git/CONFIG 2>/dev/null && echo case-config-written
        echo x > .git/hooks/PRE-COMMIT 2>/dev/null && echo case-hook-written
        mkdir -p ../evil && mv ../evil .git/info 2>/dev/null && echo moved-onto-protected
        ln .git/config gc 2>/dev/null && echo x >> gc 2>/dev/null && echo linked-config-written
        ln {key} k 2>/dev/null && cat k
        cat {firmlink} 2>/dev/null
        cat {upper} 2>/dev/null
        echo a > f && ln f f2 && echo own-link-ok
        "#,
        key = home.join(".ssh/id").display(),
        firmlink = data_home.join(".ssh/id").display(),
        upper = home.join(".SSH/id").display(),
    );
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
    let out = run_in(&p, &root.join("repo"), &script).unwrap();
    for bad in [
        "git-renamed",
        "repo-renamed",
        "case-config-written",
        "case-hook-written",
        "moved-onto-protected",
        "linked-config-written",
        "other-session-visible",
        "SECRET",
    ] {
        assert!(!out.contains(bad), "{bad}:\n{out}");
    }
    assert!(out.contains("own-link-ok"), "{out}");
    assert!(
        root.join(".ostra/sessions/s2/.state/harness/x_2/config.toml")
            .is_file()
    );
    assert!(root.join("repo/.git/config").is_file());
}

#[test]
fn seatbelt_hides_processes_sockets_and_shared_temp() {
    if !seatbelt() {
        eprintln!("seatbelt unavailable; skipping");
        return;
    }
    let (_d, root, home) = layout();
    let mut victim = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let tmp_marker =
        std::env::temp_dir().join(format!("ostra-host-{}", uuid::Uuid::new_v4().simple()));
    std::fs::write(&tmp_marker, "HOSTTMP").unwrap();
    let script = format!(
        r#"
        kill {pid} 2>/dev/null && echo killed-outside
        ps -p {pid} 2>/dev/null | grep -q sleep && echo saw-outside
        cat {marker} 2>/dev/null
        ls /private/tmp >/dev/null 2>&1 && echo listed-tmp
        curl -s --max-time 2 --unix-socket /var/run/docker.sock http://x/version >/dev/null 2>&1 && echo docker
        echo "${{SSH_AUTH_SOCK-unset}}"
        echo t > "$TMPDIR/t" && echo own-tmp
        "#,
        pid = victim.id(),
        marker = tmp_marker.display(),
    );
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
        .scratch(&root.join("scratch"))
        .unwrap();
    let out = run_in(&p, &root.join("repo"), &script).unwrap();
    let alive = victim.try_wait().unwrap().is_none();
    let _ = victim.kill();
    let _ = std::fs::remove_file(&tmp_marker);
    assert!(alive, "{out}");
    for bad in [
        "killed-outside",
        "saw-outside",
        "HOSTTMP",
        "listed-tmp",
        "docker",
    ] {
        assert!(!out.contains(bad), "{bad}:\n{out}");
    }
    assert!(out.contains("unset") && out.contains("own-tmp"), "{out}");
}

/// A tiny HTTP server on loopback that answers every request with `body`.
fn serve(body: &'static str) -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for mut s in l.incoming().flatten() {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    port
}

/// macOS shares one loopback, so each Seatbelt policy names the ports it may connect to: its
/// own proxy, its forwards, and listed loopback hosts. Nothing resolves names but the proxy.
#[test]
fn seatbelt_leads_out_only_through_its_own_proxy_and_ports() {
    if !seatbelt() {
        eprintln!("seatbelt unavailable; skipping");
        return;
    }
    let (_d, root, home) = layout();
    let listed = serve("listed");
    let unlisted = serve("unlisted");
    let bridge = serve("bridge");
    let cfg = SandboxConfig {
        allowed_hosts: vec![format!("127.0.0.1:{listed}")],
        loopback: ostra_core::config::LoopbackAccess::Listed,
        ..Default::default()
    };
    let seen: Arc<std::sync::Mutex<Vec<egress::Decision>>> = Arc::default();
    let s2 = seen.clone();
    let b = backend().unwrap();
    let other = Profile::for_execution(&ctx(&root), &cfg, &home)
        .start_egress(b, Arc::new(|_| {}))
        .unwrap();
    let other_port = other.egress.as_ref().unwrap().port().unwrap();
    let bridge_addr: std::net::SocketAddr = ([127, 0, 0, 1], bridge).into();
    let p = Profile::for_execution(&ctx(&root), &cfg, &home)
        .start_egress(b, Arc::new(move |d| s2.lock().unwrap().push(d)))
        .unwrap()
        .forward(b, bridge_addr)
        .unwrap();
    let inside = p.forwarded_port(bridge).unwrap();
    assert_ne!(inside, bridge);
    let script = format!(
        r#"
        get() {{ curl -s --max-time 5 "$@"; }}
        echo "listed:$(get http://127.0.0.1:{listed}/)"
        get http://127.0.0.1:{unlisted}/ && echo direct-unlisted
        get http://127.0.0.1:{bridge}/ && echo direct-bridge
        echo "forwarded:$(get http://127.0.0.1:{inside}/)"
        echo "via-proxy:$(get --noproxy '' -x "$HTTP_PROXY" http://127.0.0.1:{listed}/)"
        echo "refused:$(get -o /dev/null -w '%{{http_code}}' --noproxy '' -x "$HTTP_PROXY" http://127.0.0.1:1/)"
        get --noproxy '' -x http://127.0.0.1:{other_port} http://127.0.0.1:{listed}/ && echo other-proxy
        get --noproxy '*' http://1.1.1.1/ >/dev/null && echo direct-public
        python3 -c 'import socket; socket.gethostbyname("example.com"); print("resolved")' 2>/dev/null
        python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(); print("listen-ok")'
        echo "$HTTPS_PROXY"
        "#
    );
    let out = run_in(&p, &root, &script).unwrap();
    for bad in [
        "direct-unlisted",
        "direct-bridge",
        "other-proxy",
        "direct-public",
        "resolved",
    ] {
        assert!(!out.contains(bad), "{bad}:\n{out}");
    }
    for want in [
        "listed:listed",
        "forwarded:bridge",
        "via-proxy:listed",
        "refused:403",
        "listen-ok",
    ] {
        assert!(out.contains(want), "missing {want}:\n{out}");
    }
    let own = p.egress.as_ref().unwrap().proxy_url().unwrap().to_string();
    assert!(out.contains(&own), "{out}");
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.iter().any(|d| !d.allowed && d.local && d.port == 1),
        "{seen:?}"
    );

    // The default opens the loopback: the command reaches its own server and unlisted
    // ports, but not a blocked port, and another execution's proxy wants its credential.
    let blocked = serve("blocked");
    let open = SandboxConfig {
        blocked_ports: vec![blocked],
        ..Default::default()
    };
    let p = Profile::for_execution(&ctx(&root), &open, &home)
        .start_egress(b, Arc::new(|_| {}))
        .unwrap();
    let script = format!(
        r#"
        get() {{ curl -s --max-time 5 "$@"; }}
        echo "unlisted:$(get http://127.0.0.1:{unlisted}/)"
        get http://127.0.0.1:{blocked}/ && echo direct-blocked
        echo "other-proxy:$(get -o /dev/null -w '%{{http_code}}' --noproxy '' -x http://127.0.0.1:{other_port} http://127.0.0.1:{listed}/)"
        echo "own-proxy:$(get -o /dev/null -w '%{{http_code}}' https://index.crates.io/config.json)"
        python3 -c 'import socket, threading
s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen()
threading.Thread(target=lambda: s.accept()[0].sendall(b"mine"), daemon=True).start()
c = socket.create_connection(s.getsockname()); print("own-server:" + c.recv(4).decode())'
        "#
    );
    let out = run_in(&p, &root, &script).unwrap();
    assert!(!out.contains("direct-blocked"), "{out}");
    for want in [
        "unlisted:unlisted",
        "other-proxy:407",
        "own-proxy:200",
        "own-server:mine",
    ] {
        assert!(out.contains(want), "missing {want}:\n{out}");
    }

    let host = SandboxConfig {
        network: SandboxNetwork::Host,
        ..Default::default()
    };
    let p = Profile::for_execution(&ctx(&root), &host, &home);
    let out = run_in(
        &p,
        &root,
        &format!("curl -s --max-time 5 http://127.0.0.1:{unlisted}/"),
    )
    .unwrap();
    assert!(out.contains("unlisted"), "{out}");
}

/// Another process's arguments and startup environment are not readable, and the
/// sandbox's own are.
#[test]
fn seatbelt_hides_other_processes_startup_environment() {
    if !seatbelt() {
        eprintln!("seatbelt unavailable; skipping");
        return;
    }
    let (_d, root, home) = layout();
    let secret = format!("s{}", uuid::Uuid::new_v4().simple());
    let mut victim = std::process::Command::new("sleep")
        .arg("30")
        .env("OSTRA_TEST_SECRET", &secret)
        .spawn()
        .unwrap();
    let read = |pid: &str| {
        format!(
            r#"python3 -c 'import ctypes
libc = ctypes.CDLL(None, use_errno=True)
mib = (ctypes.c_int * 3)(1, 49, {pid}); n = ctypes.c_size_t(0)
if libc.sysctl(mib, 3, None, ctypes.byref(n), None, 0): print("refused")
else:
b = ctypes.create_string_buffer(n.value); libc.sysctl(mib, 3, b, ctypes.byref(n), None, 0)
print("read:" + ("secret" if b"{secret}" in b.raw else "other"))'"#
        )
    };
    let script = format!(
        "echo \"outside:$({})\"\nOSTRA_TEST_SECRET={secret} sleep 5 & C=$!\necho \"own:$({})\"; kill $C",
        read(&victim.id().to_string()),
        read("'\"$C\"'"),
    );
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
    let out = run_in(&p, &root.join("repo"), &script).unwrap();
    let _ = victim.kill();
    let _ = victim.wait();
    assert!(out.contains("outside:refused"), "{out}");
    assert!(out.contains("own:read:secret"), "{out}");
}

/// Rule P3 under Seatbelt: an existing credential file is refused and its read reported
/// through the system log; `stat` and a listing are not reports, and a missing file is none.
#[test]
fn seatbelt_reports_reads_of_existing_decoys() {
    if !seatbelt() {
        eprintln!("seatbelt unavailable; skipping");
        return;
    }
    if !crate::decoy::supported(&Backend::Seatbelt) {
        eprintln!("the system log is not readable by this user; skipping");
        return;
    }
    let (_d, root, home) = layout();
    std::fs::write(home.join(".ssh/id_rsa"), "REAL KEY").unwrap();
    std::fs::write(home.join(".vault-token"), "REAL TOKEN").unwrap();
    let seen: Arc<std::sync::Mutex<Vec<PathBuf>>> = Arc::default();
    let s2 = seen.clone();
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home)
        .watch_decoys(
            &Backend::Seatbelt,
            &home,
            &["~/.aws/credentials".into()],
            Arc::new(move |p| s2.lock().unwrap().push(p)),
        )
        .unwrap();
    let tags = p.decoys.as_ref().unwrap().tags().to_vec();
    assert_eq!(tags.len(), 2, "existing files only: {tags:?}");
    let script = format!(
        "ls {h}/.ssh; stat {h}/.vault-token >/dev/null; cat {h}/.aws/credentials; cat {h}/.vault-token; cat {h}/.ssh/id_rsa; echo done",
        h = home.display()
    );
    let out = run_in(&p, &root.join("repo"), &script).unwrap();
    assert!(!out.contains("REAL"), "{out}");
    assert!(out.contains("done"), "{out}");
    let start = std::time::Instant::now();
    while seen.lock().unwrap().len() < 2 && start.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut got = seen.lock().unwrap().clone();
    got.sort();
    assert_eq!(
        got,
        vec![home.join(".ssh/id_rsa"), home.join(".vault-token")],
        "{out}"
    );
}

/// System sandboxes that allow every mach name (Image Capture's `icdd`, for one) must not
/// read as Ostra's, or killing a sandbox's members would kill them too.
#[test]
fn an_unused_marker_matches_no_process() {
    let unused = format!("dev.ostra.sandbox.{}", uuid::Uuid::new_v4().simple());
    assert_eq!(Platform::marked(&unused), Vec::<u32>::new());
}

#[test]
fn seatbelt_members_die_with_the_invocation_even_after_setsid() {
    if !seatbelt() {
        eprintln!("seatbelt unavailable; skipping");
        return;
    }
    let (_d, root, home) = layout();
    let p = Profile::for_execution(&ctx(&root), &SandboxConfig::default(), &home);
    let pid_file = root.join("repo/pid");
    let script = format!(
        "perl -e 'use POSIX; if (fork()==0) {{ POSIX::setsid(); if (fork()==0) {{ open(F, \">{}\"); print F $$; close F; sleep 30; exit }} exit }}'; sleep 0.3",
        pid_file.display()
    );
    let sc = p
        .command(
            backend().unwrap(),
            &root.join("repo"),
            OsStr::new("sh"),
            ["-c", &script],
        )
        .unwrap();
    let status = sc.std_command().status().unwrap();
    assert!(status.success());
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: signal 0 only checks that the pid exists.
    let alive = || unsafe { libc::kill(pid, 0) } == 0;
    assert!(
        alive(),
        "the detached child should still run before the members are killed"
    );
    drop(sc);
    assert!(!alive(), "the detached child outlived its sandbox");
}
