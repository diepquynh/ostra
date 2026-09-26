//! Setup terminals: a harness's official installer or its own interactive login, in a PTY on this
//! machine that the browser streams and types into like a harness execution.

use crate::app::{App, Pushed};
use ostra_core::api::{HarnessSetupAction, HarnessSetupTerminal, ServerMsg};
use ostra_core::executor::HarnessKind;
use ostra_core::ids::ExecutionId;
use ostra_exec_harness::PtySession;
use ostra_exec_harness::launch::{LaunchPlan, shell_quote};
use std::path::PathBuf;
use std::sync::Arc;

/// The PTY registry key. Fixed per harness and action, so a second click reattaches.
pub fn terminal_id(harness: HarnessKind, action: HarnessSetupAction) -> ExecutionId {
    let action = match action {
        HarnessSetupAction::Install => "install",
        HarnessSetupAction::Login => "login",
    };
    ExecutionId::from(format!("setup_{}_{action}", harness.as_str()))
}

/// A shell prefix that stops with a readable message when `tool` is missing, instead of the
/// shell's bare "not found" halfway through a pipeline.
fn require(tool: &str) -> String {
    format!(
        "command -v {tool} >/dev/null 2>&1 || {{ echo 'Install {tool} on the machine that runs Ostra, then try again, because the installer downloads with it.'; exit 127; }}; "
    )
}

/// The program, arguments, and display line a setup terminal runs.
pub fn command(
    global: &ostra_core::config::GlobalConfig,
    harness: HarnessKind,
    action: HarnessSetupAction,
) -> (String, Vec<String>, String) {
    match action {
        HarnessSetupAction::Install => {
            let script = ostra_exec_harness::install_script(harness);
            let checked = format!("{}{script}", require("curl"));
            ("sh".into(), vec!["-c".into(), checked], script.into())
        }
        HarnessSetupAction::Login => {
            let (program, args) = ostra_exec_harness::login_command(global, harness);
            let line = std::iter::once(&program)
                .chain(&args)
                .map(|w| shell_quote(w))
                .collect::<Vec<_>>()
                .join(" ");
            (program, args, line)
        }
    }
}

/// Start the terminal, or return the one still running for this harness and action. When it
/// exits, the harnesses are checked again and the result goes to `home`.
pub fn start(
    app: &Arc<App>,
    harness: HarnessKind,
    action: HarnessSetupAction,
) -> Result<HarnessSetupTerminal, String> {
    let global = app.shared.global();
    let (program, args, line) = command(&global, harness, action);
    let id = terminal_id(harness, action);
    let out = HarnessSetupTerminal {
        terminal: id.to_string(),
        command: line,
    };
    let ptys = app.shared.harness.ptys();
    if ptys.get(&id).is_some_and(|p| p.exit_info().is_none()) {
        return Ok(out);
    }
    if action == HarnessSetupAction::Login && which::which(&program).is_err() {
        return Err(format!(
            "Install {} first, because `{program}` is not on this machine's PATH.",
            harness.display_name()
        ));
    }
    let plan = LaunchPlan {
        program,
        args,
        env: vec![],
        cwd: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
        files: vec![],
        links: vec![],
        session_id: None,
        env_remove: vec![],
    };
    let pty = PtySession::spawn(&plan, 100, 30, Box::new(|_| {}))
        .map_err(|e| format!("Could not start `{}`: {e}", out.command))?;
    ptys.insert(id, pty.clone());
    let app = app.clone();
    tokio::spawn(async move {
        pty.wait_exit().await;
        let fresh = crate::env::EnvStatus::detect(&app.shared.global()).await;
        let statuses = fresh.harnesses.clone();
        *app.shared.env.write() = fresh;
        let _ = app.push.send(Pushed {
            channels: vec!["home".into()],
            msg: ServerMsg::HarnessStatus { statuses },
        });
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostra_core::config::GlobalConfig;

    #[test]
    fn commands_per_action() {
        let g = GlobalConfig::default();
        let (p, a, line) = command(&g, HarnessKind::Codex, HarnessSetupAction::Install);
        assert_eq!((p.as_str(), a[0].as_str()), ("sh", "-c"));
        assert!(a[1].starts_with("command -v curl ") && a[1].ends_with(&line));
        assert_eq!(line, "curl -fsSL https://chatgpt.com/codex/install.sh | sh");
        let (p, a, line) = command(&g, HarnessKind::Claude, HarnessSetupAction::Login);
        assert_eq!(
            (p.as_str(), a),
            ("claude", vec!["auth".into(), "login".into()])
        );
        assert_eq!(line, "claude auth login");
        assert_eq!(
            terminal_id(HarnessKind::Agy, HarnessSetupAction::Login).as_str(),
            "setup_agy_login"
        );
    }
}
