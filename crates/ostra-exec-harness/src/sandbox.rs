//! Runs a harness CLI under the execution's sandbox profile. The CLI's own Bash tool, and
//! everything it starts, inherit the sandbox. Under bubblewrap the home folder is overlaid so the
//! CLI can write its state dirs but cannot leave files that the user's later shells or CLI
//! sessions would run. Seatbelt has no overlay, so there the rest of the home stays read-only and
//! a CLI's writes at the top of it (Claude Code's `~/.claude.json`) fail, which the CLIs survive.

use crate::launch::{LaunchInput, LaunchPlan, grok_home_real};
use ostra_core::HarnessKind;
use ostra_core::config::SandboxConfig;
use ostra_core::sandbox::{self, Backend, Decision, Members, Profile};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The per-execution copy of `~/.claude.json`, which Claude Code rewrites through a temp file and
/// a rename that a read-only home would refuse.
pub const CLAUDE_JSON_COPY: &str = "claude.json";

/// The scratch dir mounted at the sandbox's `/tmp`, inside the execution's own dir.
const SCRATCH: &str = "tmp";

/// Grok's per-execution config, which holds the execution's bridge token.
const GROK_CONFIG: &str = "grok-home/config.toml";

/// Settings, instructions, and hooks inside a harness state dir. They stay read-only, because a
/// hook or instruction written there would run in the user's own later sessions of that CLI. A
/// trailing `/` marks a dir.
fn harness_dirs(inp: &LaunchInput<'_>) -> (Vec<PathBuf>, PathBuf, &'static [&'static str]) {
    let home = &inp.home;
    match inp.harness {
        HarnessKind::Claude => {
            let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"));
            (
                vec![dir.clone(), home.join(".local/state/claude")],
                dir,
                &[
                    "settings.json",
                    "settings.local.json",
                    "CLAUDE.md",
                    "commands/",
                    "agents/",
                    "skills/",
                    "plugins/",
                    "hooks/",
                    "output-styles/",
                ],
            )
        }
        HarnessKind::Codex => {
            let dir = std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"));
            (
                vec![dir.clone()],
                dir,
                &["config.toml", "AGENTS.md", "hooks.json", "rules/"],
            )
        }
        HarnessKind::Grok => {
            let dir = grok_home_real(home);
            (
                vec![dir.clone()],
                dir,
                &["config.toml", "trusted_folders.toml", "hooks/"],
            )
        }
        HarnessKind::Agy => {
            let dir = home.join(".gemini");
            (
                vec![dir.clone()],
                dir,
                &["settings.json", "GEMINI.md", "config/plugins/"],
            )
        }
    }
}

/// A launch plan after [`wrap`].
#[derive(Debug)]
pub struct Wrapped {
    pub plan: LaunchPlan,
    /// Kills what the CLI leaves running once dropped. Hold it until the CLI has exited.
    pub members: Option<Members>,
    /// Why the CLI runs unsandboxed, when the config asked for a sandbox.
    pub warning: Option<String>,
}

/// `plan` wrapped in the sandbox, or unchanged with a warning when the config allows running
/// without one. `Err` when the config requires one this machine cannot provide.
pub fn wrap(
    plan: LaunchPlan,
    inp: &LaunchInput<'_>,
    cfg: &SandboxConfig,
) -> Result<Wrapped, String> {
    let backend = match sandbox::decide(cfg)? {
        Decision::Sandboxed(b) => b,
        Decision::Unsandboxed(warning) => {
            return Ok(Wrapped {
                plan,
                members: None,
                warning,
            });
        }
    };
    let seatbelt = backend == Backend::Seatbelt;
    // The CLI needs the network for its model API and for Ostra's hook bridge on loopback, and
    // its PTY as controlling terminal.
    let mut profile = Profile::for_execution(&inp.spec.ctx, cfg, &inp.home)
        .network(true)
        .new_session(false)
        .tty(seatbelt)
        .writable(&inp.config_dir)
        .scratch(&inp.config_dir.join(SCRATCH))
        .map_err(|e| format!("creating the sandbox's /tmp: {e}"))?;
    if !seatbelt {
        profile = profile.home_overlay(&inp.home);
    }
    let (rw, settings_dir, ro) = harness_dirs(inp);
    for d in &rw {
        profile = profile.writable(d);
    }
    for rel in ro {
        profile = profile.protect(&settings_dir, rel);
    }
    if inp.harness == HarnessKind::Claude && !seatbelt {
        let real = inp.home.join(".claude.json");
        let copy = inp.config_dir.join(CLAUDE_JSON_COPY);
        if real.is_file() {
            copy_private(&real, &copy)
                .map_err(|e| format!("copying {} for the sandbox: {e}", real.display()))?;
            profile = profile.link(&real, &copy);
        }
    }
    let sc = profile.command(
        &backend,
        &plan.cwd,
        OsStr::new(&plan.program),
        plan.args.iter(),
    )?;
    let text = |s: &OsStr| s.to_string_lossy().into_owned();
    let mut env = plan.env.clone();
    env.extend(sc.env.iter().cloned());
    if seatbelt {
        env.extend(seatbelt_env(&sc.env));
    }
    let mut env_remove = plan.env_remove.clone();
    env_remove.extend(sc.env_remove.iter().cloned());
    Ok(Wrapped {
        plan: LaunchPlan {
            program: text(sc.program.as_os_str()),
            args: sc.args.iter().map(|a| text(a)).collect(),
            env,
            env_remove,
            tty_param: seatbelt.then(|| sandbox::TTY_PARAM.to_string()),
            ..plan
        },
        members: sc.members,
        warning: None,
    })
}

/// Variables the CLIs need under Seatbelt, which denies `/tmp` and the keychain: Claude Code
/// makes its own dir under `/tmp` unless told otherwise, and CLIs built on `rustls-native-certs`
/// (Codex) read the trusted roots from the keychain unless given a bundle.
fn seatbelt_env(sandbox_env: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = vec![];
    if let Some((_, tmp)) = sandbox_env.iter().find(|(k, _)| k == "TMPDIR") {
        out.push((
            "CLAUDE_CODE_TMPDIR".into(),
            tmp.trim_end_matches('/').to_string(),
        ));
    }
    if std::env::var_os("SSL_CERT_FILE").is_none() && Path::new(MACOS_CERTS).is_file() {
        out.push(("SSL_CERT_FILE".into(), MACOS_CERTS.into()));
    }
    out
}

/// The system's trusted roots as a PEM bundle, shipped with macOS.
const MACOS_CERTS: &str = "/etc/ssl/cert.pem";

fn copy_private(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let body = std::fs::read(from)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(to)?;
    f.write_all(&body)
}

/// Removes what [`wrap`] and the launch plan left in the execution's config dir that is private
/// or large: the copied Claude config, Grok's token-bearing config, and the scratch `/tmp`.
pub fn cleanup(config_dir: &Path) {
    let _ = std::fs::remove_file(config_dir.join(CLAUDE_JSON_COPY));
    let _ = std::fs::remove_file(config_dir.join(GROK_CONFIG));
    let _ = std::fs::remove_dir_all(config_dir.join(SCRATCH));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::tests::{input, spec};

    #[test]
    fn harness_home_is_overlaid_and_settings_stay_read_only() {
        let Some(backend) = sandbox::backend() else {
            eprintln!(
                "no sandbox here ({}); skipping",
                sandbox::unavailable_message()
            );
            return;
        };
        let seatbelt = *backend == Backend::Seatbelt;
        // Seatbelt denies the OS temp dir, so the fake home, which must stay readable, lives
        // under `target/`.
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
        std::fs::create_dir_all(&base).unwrap();
        let tmp = tempfile::tempdir_in(&base).unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for d in [
            "repo",
            "ws/.ostra/sessions/s_1/backend",
            "cfg",
            "home/.claude/projects",
            "home/.ssh",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let home = root.join("home");
        std::fs::write(home.join(".claude.json"), "{\"real\":true}").unwrap();
        std::fs::write(home.join(".bashrc"), "").unwrap();
        std::fs::write(home.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(home.join(".ssh/id"), "SECRET").unwrap();
        let s = spec(HarnessKind::Claude, "haiku", &root);
        let inp = input(&s, HarnessKind::Claude, &root);
        let script = r#"
            set -u
            grep -q real ~/.claude.json && echo read-config
            echo '{"new":1}' > ~/.claude.json.tmp.1 && mv ~/.claude.json.tmp.1 ~/.claude.json && echo saved-config
            touch ~/.bash_aliases && echo made-alias
            echo x >> ~/.bashrc 2>/dev/null || echo rc-refused
            echo x >> ~/.claude/settings.json 2>/dev/null || echo settings-refused
            touch ~/.claude/projects/t && echo wrote-projects
            cat ~/.ssh/id 2>/dev/null || echo key-hidden
            test -e /tmp/ostra-host-marker || echo tmp-private
            echo s > "${TMPDIR:-/tmp}/scratch-file" && echo tmp-writable
            echo "$CLAUDE_CODE_TMPDIR" | grep -q . && echo claude-tmp-set
            echo '{}' > ~/.claude/settings.local.json 2>/dev/null || echo local-settings-refused
        "#;
        let plan = LaunchPlan {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            env: vec![],
            cwd: root.join("repo"),
            files: vec![],
            links: vec![],
            session_id: None,
            env_remove: vec![],
            tty_param: None,
        };
        let marker = std::env::temp_dir().join("ostra-host-marker");
        std::fs::write(&marker, "").unwrap();
        let wrapped = wrap(plan, &inp, &SandboxConfig::default()).unwrap();
        assert!(wrapped.warning.is_none());
        let plan = wrapped.plan;
        let mut cmd = std::process::Command::new(&plan.program);
        if seatbelt {
            assert_eq!(plan.program, sandbox::SANDBOX_EXEC);
            assert_eq!(plan.tty_param.as_deref(), Some(sandbox::TTY_PARAM));
            // No PTY in this test, so the parameter names a terminal it does not use.
            cmd.args(["-D", "TTY=/dev/null"]);
        } else {
            assert!(plan.program.ends_with("bwrap"));
        }
        for k in &plan.env_remove {
            cmd.env_remove(k);
        }
        let out = cmd
            .args(&plan.args)
            .envs(plan.env.iter().map(|(k, v)| (k, v)))
            .env("HOME", &home)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        // Only bubblewrap overlays the home: under Seatbelt the CLI's top-level writes fail.
        let overlay: &[&str] = if seatbelt {
            &["claude-tmp-set"]
        } else {
            &["saved-config", "made-alias"]
        };
        for want in overlay.iter().copied().chain([
            "read-config",
            "rc-refused",
            "settings-refused",
            "wrote-projects",
            "key-hidden",
            "tmp-private",
            "tmp-writable",
            "local-settings-refused",
        ]) {
            assert!(
                text.contains(want),
                "missing {want}: {text}\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        assert!(!text.contains("SECRET"));
        assert_eq!(
            std::fs::read_to_string(home.join(".claude.json")).unwrap(),
            "{\"real\":true}"
        );
        assert!(!home.join(".bash_aliases").exists());
        assert_eq!(std::fs::read_to_string(home.join(".bashrc")).unwrap(), "");
        assert!(home.join(".claude/projects/t").exists());
        // Bubblewrap mounts a placeholder; Seatbelt's literal rule refuses the missing path.
        if seatbelt {
            assert!(!home.join(".claude/settings.local.json").exists());
        } else {
            assert_eq!(
                std::fs::read_to_string(home.join(".claude/settings.local.json")).unwrap(),
                "{}"
            );
        }
        assert!(inp.config_dir.join("tmp/scratch-file").exists());
        let _ = std::fs::remove_file(&marker);
        std::fs::create_dir_all(inp.config_dir.join("grok-home")).unwrap();
        std::fs::write(inp.config_dir.join(GROK_CONFIG), "OSTRA_TOKEN").unwrap();
        cleanup(&inp.config_dir);
        assert!(!inp.config_dir.join(CLAUDE_JSON_COPY).exists());
        assert!(!inp.config_dir.join(GROK_CONFIG).exists());
        assert!(!inp.config_dir.join("tmp").exists());
    }
}
