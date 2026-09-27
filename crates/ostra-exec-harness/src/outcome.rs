//! Turning how a harness session ended into an [`ExecutionResult`].

use crate::live::LiveState;
use crate::transcript;
use ostra_core::HarnessKind;
use ostra_core::config::SandboxMode;
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use std::path::Path;
use std::time::Duration;

pub const AUTH_PREFIX: &str = "harness-auth:";
pub const LAUNCH_PREFIX: &str = "harness-launch:";

/// A launch that dies faster than this is a launch failure, not a run failure.
pub const LAUNCH_WINDOW: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq)]
pub enum End {
    Submitted,
    GaveUp,
    Exited(Option<u32>, Duration),
    Timeout(u64),
    Cancelled,
    Idle,
    Auth,
    Fatal(String),
}

/// Map the submit payload's `status` to the execution status.
pub fn status_of(submit: &serde_json::Value) -> ExecutionStatus {
    match submit.get("status").and_then(|v| v.as_str()) {
        Some("stuck") => ExecutionStatus::Stuck,
        Some("handoff") => ExecutionStatus::Handoff,
        _ => ExecutionStatus::Ok,
    }
}

/// Harness executions run under Seatbelt with the current config and the workspace's mode, which
/// keeps the keychain closed.
fn seatbelt(mode: Option<SandboxMode>) -> bool {
    let global: ostra_core::config::GlobalConfig =
        ostra_core::config::load_toml(&ostra_core::paths::global_config_path()).unwrap_or_default();
    ostra_core::sandbox::decide(&global.sandbox.for_workspace(
        &ostra_core::config::WorkspaceSandbox {
            mode,
            ..Default::default()
        },
    )) == Ok(ostra_core::sandbox::Decision::Sandboxed(
        ostra_core::sandbox::Backend::Seatbelt,
    ))
}

fn tail(screen: &str, lines: usize) -> String {
    let kept: Vec<&str> = screen
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();
    kept[kept.len().saturating_sub(lines)..].join("\n")
}

pub fn result(
    end: End,
    harness: HarnessKind,
    agent_submit: &str,
    state: &LiveState,
    screen: &str,
    home: &Path,
    sandbox_mode: Option<SandboxMode>,
) -> ExecutionResult {
    let facts = transcript::facts(
        harness,
        state.transcript_path.as_deref(),
        state.session_id.as_deref(),
        home,
    );
    let mut usage = facts.usage;
    usage.tool_calls = usage.tool_calls.max(state.tool_calls);
    let final_text = state
        .last_message
        .clone()
        .or(facts.last_message)
        .unwrap_or_default();
    let name = harness.display_name();
    let (status, error) = match end {
        End::Submitted => {
            let submit = state.submit.clone().unwrap_or_default();
            return ExecutionResult {
                status: status_of(&submit),
                submit: Some(submit),
                final_text,
                usage,
                native_session_id: state.session_id.clone(),
                error: None,
            };
        }
        End::Cancelled => (ExecutionStatus::Cancelled, None),
        End::Auth if harness == HarnessKind::Agy && seatbelt(sandbox_mode) => (
            ExecutionStatus::Error,
            Some(format!(
                "{AUTH_PREFIX} {name} cannot read its sign-in: set `GEMINI_API_KEY` or `GOOGLE_API_KEY` in Ostra's environment, then retry, because on macOS it keeps its login in the keychain, which the sandbox closes to agents."
            )),
        ),
        End::Auth => (
            ExecutionStatus::Error,
            Some(format!(
                "{AUTH_PREFIX} {name} is not logged in. Log in from its terminal, then retry."
            )),
        ),
        End::Exited(code, after) if after < LAUNCH_WINDOW && state.tool_calls == 0 => {
            let code = code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "a signal".into());
            let lower = screen.to_lowercase();
            let prefix = if crate::executor::AUTH_MARKERS
                .iter()
                .any(|m| lower.contains(m))
            {
                AUTH_PREFIX
            } else {
                LAUNCH_PREFIX
            };
            (
                ExecutionStatus::Error,
                Some(format!(
                    "{prefix} {name} exited with {code} right after starting:\n{}",
                    tail(screen, 12)
                )),
            )
        }
        End::Exited(code, _) => (
            ExecutionStatus::Error,
            Some(format!(
                "{name} exited (code {}) before the agent called `{agent_submit}`.",
                code.map(|c| c.to_string()).unwrap_or_else(|| "none".into())
            )),
        ),
        End::GaveUp => (
            ExecutionStatus::Error,
            Some(format!(
                "The agent ended its turn three times without calling `{agent_submit}`."
            )),
        ),
        End::Idle => (
            ExecutionStatus::Error,
            Some(format!(
                "The session went quiet without calling `{agent_submit}`, and two reminders did not help."
            )),
        ),
        End::Fatal(why) => (
            ExecutionStatus::Error,
            Some(format!(
                "{LAUNCH_PREFIX} {name} could not start the session: {why}.\n{}",
                tail(screen, 12)
            )),
        ),
        End::Timeout(secs) => (
            ExecutionStatus::Error,
            Some(format!(
                "The execution passed its {secs} s budget and was stopped."
            )),
        ),
    };
    ExecutionResult {
        status,
        submit: None,
        final_text,
        usage,
        native_session_id: state.session_id.clone(),
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn statuses() {
        assert_eq!(
            status_of(&json!({"status": "stuck"})),
            ExecutionStatus::Stuck
        );
        assert_eq!(
            status_of(&json!({"status": "handoff"})),
            ExecutionStatus::Handoff
        );
        assert_eq!(status_of(&json!({"verdict": "PASS"})), ExecutionStatus::Ok);
    }

    #[test]
    fn early_exit_is_a_launch_failure() {
        let live = crate::live::LiveRegistry::new();
        let l = live.register(
            ostra_core::ExecutionId::new(),
            ostra_core::AgentName::Plan,
            HarnessKind::Codex,
        );
        let tmp = tempfile::tempdir().unwrap();
        let r = result(
            End::Exited(Some(1), Duration::from_secs(2)),
            HarnessKind::Codex,
            "submit_plan",
            &l.snapshot(),
            "error: unknown model\n",
            tmp.path(),
            None,
        );
        assert!(r.error.unwrap().starts_with(LAUNCH_PREFIX));
        let r = result(
            End::Exited(Some(1), Duration::from_secs(2)),
            HarnessKind::Codex,
            "submit_plan",
            &l.snapshot(),
            "Not logged in. Run codex login",
            tmp.path(),
            None,
        );
        assert!(r.error.unwrap().starts_with(AUTH_PREFIX));
    }
}
