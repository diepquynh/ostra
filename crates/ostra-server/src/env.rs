//! What is installed on this machine: harness CLIs, their versions and logins.

use ostra_core::api::HarnessStatus;
use ostra_core::config::GlobalConfig;
use ostra_core::executor::HarnessKind;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct EnvStatus {
    pub harnesses: Vec<HarnessStatus>,
    pub checked: Instant,
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// Whether a harness has stored credentials. `None` when Ostra cannot tell.
fn logged_in(h: HarnessKind) -> Option<bool> {
    let h_dir = home();
    let any = |paths: &[PathBuf]| paths.iter().any(|p| p.exists());
    match h {
        HarnessKind::Claude => {
            if std::env::var_os("ANTHROPIC_API_KEY").is_some() {
                return Some(true);
            }
            let creds = [h_dir.join(".claude/.credentials.json")];
            if any(&creds) || std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN").is_some() {
                return Some(true);
            }
            let cfg = std::fs::read_to_string(h_dir.join(".claude.json")).ok()?;
            // Credentials may live in the OS keychain, which Ostra does not read, so absence of
            // a marker means unknown rather than logged out.
            cfg.contains("\"oauthAccount\"").then_some(true)
        }
        HarnessKind::Codex => {
            if std::env::var_os("OPENAI_API_KEY").is_some() {
                return Some(true);
            }
            let codex_home = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| h_dir.join(".codex"));
            Some(codex_home.join("auth.json").exists())
        }
        HarnessKind::Grok | HarnessKind::Agy => None,
    }
}

async fn version(command: &str) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(8),
        tokio::process::Command::new(command).arg("--version").kill_on_drop(true).output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(String::from)
}

impl EnvStatus {
    pub async fn detect(global: &GlobalConfig) -> Self {
        let mut harnesses = vec![];
        for h in HarnessKind::ALL {
            let command = global.harness_command(h);
            let installed = which::which(&command).is_ok();
            let v = if installed { version(&command).await } else { None };
            harnesses.push(HarnessStatus {
                harness: h,
                command,
                installed,
                version: v,
                logged_in: if installed { logged_in(h) } else { Some(false) },
            });
        }
        EnvStatus { harnesses, checked: Instant::now() }
    }

    pub fn installed(&self) -> Vec<HarnessKind> {
        self.harnesses.iter().filter(|h| h.installed).map(|h| h.harness).collect()
    }

    pub fn stale(&self) -> bool {
        self.checked.elapsed() > Duration::from_secs(60)
    }
}
