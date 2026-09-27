//! Runs a harness CLI under the execution's sandbox profile. The CLI's own Bash tool, and
//! everything it starts, inherit the sandbox. Under bubblewrap the home folder is overlaid so the
//! CLI can write its state dirs but cannot leave files that the user's later shells or CLI
//! sessions would run. Seatbelt has no overlay, so there the rest of the home stays read-only and
//! a CLI's writes at the top of it (Claude Code's `~/.claude.json`) fail, which the CLIs survive.

use crate::launch::{LaunchInput, LaunchPlan, grok_home_real};
use crate::protocol::ENV_URL;
use ostra_core::HarnessKind;
use ostra_core::config::SandboxConfig;
use ostra_core::egress::HostRule;
use ostra_core::sandbox::{self, Backend, Decision, Members, Profile};
use std::ffi::OsStr;
use std::net::SocketAddr;
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
/// without one. The egress proxy reports its decisions to `on`, and the decoys their opens to
/// `on_decoy`. `Err` when the config requires one this machine cannot provide.
pub fn wrap(
    plan: LaunchPlan,
    inp: &LaunchInput<'_>,
    cfg: &SandboxConfig,
    on: ostra_core::egress::OnDecision,
    on_decoy: ostra_core::decoy::OnOpen,
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
    let bridge = bridge_addr(&inp.server_url)?;
    // The CLI needs its PTY as controlling terminal, its model API under every network choice,
    // and Ostra's hook bridge on the same loopback port inside its network namespace.
    let profile = Profile::for_execution(&inp.spec.ctx, cfg, &inp.home)
        .allow_hosts(model_hosts(inp.harness), base_url_hosts(&plan, inp))
        .start_egress(&backend, on)
        .map_err(|e| format!("starting the sandbox's egress proxy: {e}"))?
        .watch_decoys(&backend, &inp.home, &inp.spec.ctx.sandbox_decoys, on_decoy)
        .map_err(|e| format!("planting the sandbox's decoy files: {e}"))?;
    // The server's bridge socket answers only `/internal/*`, so the sandbox cannot reach `/api`.
    let profile = match &inp.bridge_socket {
        Some(sock) => profile.forward_socket(&backend, bridge.port(), sock),
        None => profile.forward(&backend, bridge),
    }
    .map_err(|e| format!("forwarding the hook bridge into the sandbox: {e}"))?;
    // A forwarded bridge is reached on loopback: its own port inside a network namespace, where
    // only loopback exists, and a fresh one under Seatbelt, whose policy names that port only.
    let inside_url = profile
        .forwarded_port(bridge.port())
        .map(|p| format!("http://127.0.0.1:{p}"));
    let mut plan = plan;
    if let Some(url) = inside_url.as_ref().filter(|u| **u != inp.server_url) {
        // Grok's config names the bridge for the MCP server it starts, so the file says it too.
        let mut changed = false;
        for (_, text) in plan.files.iter_mut() {
            if let Some(t) = replace_url(text, &inp.server_url, url) {
                *text = t;
                changed = true;
            }
        }
        if changed {
            crate::launch::materialize(&plan)
                .map_err(|e| format!("writing the harness config: {e}"))?;
        }
    }
    let mut profile = profile
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
    if let Some(url) = &inside_url {
        for (k, v) in env.iter_mut() {
            if k == ENV_URL {
                *v = url.clone();
            }
        }
    }
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

/// `text` with each `from` URL replaced by `to`, where the port does not go on with another
/// digit. `None` when it holds none.
fn replace_url(text: &str, from: &str, to: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut found = false;
    while let Some(i) = rest.find(from) {
        let after = &rest[i + from.len()..];
        out.push_str(&rest[..i]);
        if after.starts_with(|c: char| c.is_ascii_digit()) {
            out.push_str(from);
        } else {
            out.push_str(to);
            found = true;
        }
        rest = after;
    }
    out.push_str(rest);
    found.then_some(out)
}

/// The hook bridge's address from Ostra's callback URL, `http://<ip>:<port>`.
fn bridge_addr(server_url: &str) -> Result<SocketAddr, String> {
    server_url
        .strip_prefix("http://")
        .and_then(|a| a.trim_end_matches('/').parse().ok())
        .ok_or_else(|| format!("The hook bridge URL `{server_url}` is not http://<ip>:<port>."))
}

/// The CLI's model API and sign-in hosts, which must resolve to public addresses.
fn model_hosts(harness: HarnessKind) -> Vec<HostRule> {
    ostra_core::egress::model_hosts(harness)
        .iter()
        .filter_map(|h| HostRule::parse(h))
        .collect()
}

/// The hosts of the model endpoints the user chose for the CLI: `*_BASE_URL` variables from its
/// launch plan, Ostra's own environment, or Claude Code's settings, and Codex's model providers.
/// They often point at a gateway on this machine or the LAN, so they may resolve anywhere.
fn base_url_hosts(plan: &LaunchPlan, inp: &LaunchInput<'_>) -> Vec<HostRule> {
    let removed = |k: &str| plan.env_remove.iter().any(|r| r == k);
    let inherited = std::env::vars().filter(|(k, _)| !removed(k));
    let mut urls: Vec<String> = plan
        .env
        .iter()
        .cloned()
        .chain(inherited)
        .filter(|(k, _)| k.ends_with("_BASE_URL"))
        .map(|(_, v)| v)
        .collect();
    let (_, settings_dir, _) = harness_dirs(inp);
    match inp.harness {
        HarnessKind::Claude => {
            let settings: serde_json::Value =
                std::fs::read_to_string(settings_dir.join("settings.json"))
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .unwrap_or_default();
            if let Some(env) = settings.get("env").and_then(|e| e.as_object()) {
                urls.extend(
                    env.iter()
                        .filter(|(k, _)| k.ends_with("_BASE_URL"))
                        .filter_map(|(_, v)| v.as_str().map(str::to_string)),
                );
            }
        }
        HarnessKind::Codex => {
            let config: toml::Table = std::fs::read_to_string(settings_dir.join("config.toml"))
                .ok()
                .and_then(|t| t.parse().ok())
                .unwrap_or_default();
            if let Some(providers) = config.get("model_providers").and_then(|p| p.as_table()) {
                urls.extend(
                    providers
                        .values()
                        .filter_map(|p| p.get("base_url")?.as_str().map(str::to_string)),
                );
            }
        }
        HarnessKind::Grok | HarnessKind::Agy => {}
    }
    urls.iter().filter_map(|u| url_host(u)).collect()
}

/// The host and port of an `http` or `https` URL as a rule.
fn url_host(url: &str) -> Option<HostRule> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit('@').next()?;
    let has_port = auth.rsplit_once(':').is_some_and(|(_, p)| !p.contains(']'));
    // A URL without a port uses its scheme's, which a forward into the sandbox needs spelled out.
    if !has_port && url.starts_with("http://") {
        return HostRule::parse(&format!("{auth}:80"));
    }
    HostRule::parse(auth)
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
    fn the_bridge_url_is_replaced_only_where_the_port_ends() {
        let from = "http://127.0.0.1:4100";
        assert_eq!(
            replace_url(
                "a = \"http://127.0.0.1:4100\", b = \"http://127.0.0.1:41000\", c = http://127.0.0.1:4100/x",
                from,
                "http://127.0.0.1:5"
            )
            .as_deref(),
            Some("a = \"http://127.0.0.1:5\", b = \"http://127.0.0.1:41000\", c = http://127.0.0.1:5/x")
        );
        assert_eq!(replace_url("http://127.0.0.1:41000", from, "x"), None);
    }

    #[test]
    fn base_urls_and_the_bridge_address_parse() {
        let rule = |s: &str| HostRule::parse(s).unwrap();
        assert_eq!(
            url_host("http://127.0.0.1:8317/v1"),
            Some(rule("127.0.0.1:8317"))
        );
        assert_eq!(url_host("http://gw.lan/v1"), Some(rule("gw.lan:80")));
        assert_eq!(
            url_host("https://u:p@gw.example.com/x"),
            Some(rule("gw.example.com"))
        );
        assert_eq!(url_host("https://[::1]/"), Some(rule("[::1]")));
        assert_eq!(url_host("ftp://x"), None);
        assert_eq!(
            bridge_addr("http://127.0.0.1:4100").unwrap(),
            "127.0.0.1:4100".parse().unwrap()
        );
        assert!(bridge_addr("http://localhost:4100").is_err());
    }

    /// The hook bridge (at `OSTRA_URL`) and a loopback model gateway stay reachable from the
    /// CLI's sandbox, and nothing else on the host's loopback is.
    #[test]
    fn the_bridge_and_listed_loopback_hosts_reach_the_host() {
        if sandbox::backend().is_none() {
            eprintln!("no sandbox here; skipping");
            return;
        }
        let serve = |body: &'static str| {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            std::thread::spawn(move || {
                use std::io::{Read, Write};
                for mut s in l.incoming().flatten() {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf);
                    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
                }
            });
            port
        };
        let (bridge, gateway, other) = (serve("bridge"), serve("gateway"), serve("other"));
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
        std::fs::create_dir_all(&base).unwrap();
        let tmp = tempfile::tempdir_in(&base).unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for d in ["repo", "ws/.ostra/sessions/s_1/backend", "cfg", "home"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let s = spec(HarnessKind::Claude, "haiku", &root);
        let mut inp = input(&s, HarnessKind::Claude, &root);
        inp.server_url = format!("http://127.0.0.1:{bridge}");
        let script = format!(
            "echo \"url:$(curl -s --max-time 3 $OSTRA_URL/)\"; echo \"file:$(curl -s --max-time 3 $(cat {conf})/)\"; for p in {gateway} {other} {bridge}; do echo \"got:$(curl -s --max-time 3 http://127.0.0.1:$p/)\"; done",
            conf = root.join("cfg/bridge-url").display()
        );
        let run = |inp: &LaunchInput<'_>| {
            let plan = LaunchPlan {
                program: "sh".into(),
                args: vec!["-c".into(), script.clone()],
                env: vec![
                    (
                        "ANTHROPIC_BASE_URL".into(),
                        format!("http://127.0.0.1:{gateway}"),
                    ),
                    (ENV_URL.into(), inp.server_url.clone()),
                ],
                cwd: root.join("repo"),
                // As Grok's config names the bridge for its MCP server.
                files: vec![(root.join("cfg/bridge-url"), inp.server_url.clone())],
                links: vec![],
                session_id: None,
                env_remove: vec![],
                tty_param: None,
            };
            crate::launch::materialize(&plan).unwrap();
            // Listed ports only, so the other listener must stay unreachable on macOS too.
            let cfg = SandboxConfig {
                loopback: ostra_core::config::LoopbackAccess::Listed,
                ..Default::default()
            };
            let wrapped = wrap(
                plan,
                inp,
                &cfg,
                std::sync::Arc::new(|_| {}),
                std::sync::Arc::new(|_| {}),
            )
            .unwrap();
            let plan = wrapped.plan;
            let mut cmd = std::process::Command::new(&plan.program);
            if plan.tty_param.is_some() {
                cmd.args(["-D", "TTY=/dev/null"]);
            }
            let out = cmd
                .args(&plan.args)
                .envs(plan.env.iter().map(|(k, v)| (k, v)))
                .env("HOME", root.join("home"))
                .output()
                .unwrap();
            drop(wrapped.members);
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let text = run(&inp);
        assert!(text.contains("url:bridge"), "{text}");
        assert!(text.contains("file:bridge"), "{text}");
        assert!(text.contains("got:gateway"), "{text}");
        assert!(!text.contains("got:other"), "{text}");

        // With the server's bridge socket, the bridge port leads there and not to the listener.
        let sock = root.join("bridge.sock");
        let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for mut s in l.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsocket",
                );
            }
        });
        inp.bridge_socket = Some(sock.clone());
        let text = run(&inp);
        assert!(text.contains("url:socket"), "{text}");
        assert!(text.contains("file:socket"), "{text}");
        assert!(!text.contains("bridge"), "{text}");
        assert!(
            sock.exists(),
            "the sandbox leaves the server's socket in place"
        );
    }

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
        let wrapped = wrap(
            plan,
            &inp,
            &SandboxConfig::default(),
            std::sync::Arc::new(|_| {}),
            std::sync::Arc::new(|_| {}),
        )
        .unwrap();
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
