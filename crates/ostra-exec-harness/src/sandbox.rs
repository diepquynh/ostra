//! Runs a harness CLI under the execution's bubblewrap profile. The CLI's own Bash tool, and
//! everything it starts, inherit the sandbox. The home folder is overlaid so the CLI can write its
//! state dirs but cannot leave files that the user's later shells or CLI sessions would run.

use crate::launch::{LaunchInput, LaunchPlan, grok_home_real};
use ostra_core::HarnessKind;
use ostra_core::config::SandboxConfig;
use ostra_core::sandbox::{self, Decision, Profile};
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

/// `plan` wrapped in `bwrap`, or unchanged with a warning when the config allows running without
/// a sandbox. `Err` when the config requires one this machine cannot provide.
pub fn wrap(
    plan: LaunchPlan,
    inp: &LaunchInput<'_>,
    cfg: &SandboxConfig,
) -> Result<(LaunchPlan, Option<String>), String> {
    let bwrap = match sandbox::decide(cfg)? {
        Decision::Sandboxed(b) => b,
        Decision::Unsandboxed(warning) => return Ok((plan, warning)),
    };
    // The CLI needs the network for its model API and for Ostra's hook bridge on loopback, and
    // its PTY as controlling terminal.
    let mut profile = Profile::for_execution(&inp.spec.ctx, cfg, &inp.home)
        .network(true)
        .new_session(false)
        .home_overlay(&inp.home)
        .writable(&inp.config_dir)
        .scratch(&inp.config_dir.join(SCRATCH))
        .map_err(|e| format!("creating the sandbox's /tmp: {e}"))?;
    let (rw, settings_dir, ro) = harness_dirs(inp);
    for d in &rw {
        profile = profile.writable(d);
    }
    for rel in ro {
        profile = profile.protect(&settings_dir, rel);
    }
    if inp.harness == HarnessKind::Claude {
        let real = inp.home.join(".claude.json");
        let copy = inp.config_dir.join(CLAUDE_JSON_COPY);
        if real.is_file() {
            copy_private(&real, &copy)
                .map_err(|e| format!("copying {} for the sandbox: {e}", real.display()))?;
            profile = profile.link(&real, &copy);
        }
    }
    let mut args: Vec<String> = profile
        .args(&plan.cwd)
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    args.push("--".into());
    args.push(plan.program.clone());
    args.extend(plan.args.iter().cloned());
    Ok((
        LaunchPlan {
            program: bwrap.to_string_lossy().into_owned(),
            args,
            ..plan
        },
        None,
    ))
}

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
        if sandbox::bwrap().is_none() {
            eprintln!("bwrap unavailable; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
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
            echo s > /tmp/scratch-file && echo tmp-writable
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
        };
        let marker = std::env::temp_dir().join("ostra-host-marker");
        std::fs::write(&marker, "").unwrap();
        let (plan, warning) = wrap(plan, &inp, &SandboxConfig::default()).unwrap();
        assert!(warning.is_none());
        assert!(plan.program.ends_with("bwrap"));
        let out = std::process::Command::new(&plan.program)
            .args(&plan.args)
            .env("HOME", &home)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        for want in [
            "read-config",
            "saved-config",
            "made-alias",
            "rc-refused",
            "settings-refused",
            "wrote-projects",
            "key-hidden",
            "tmp-private",
            "tmp-writable",
            "local-settings-refused",
        ] {
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
        assert_eq!(
            std::fs::read_to_string(home.join(".claude/settings.local.json")).unwrap(),
            "{}"
        );
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
