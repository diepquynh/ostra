//! Wire shapes between the `ostra hook` / `ostra mcp-stdio` subprocesses and the server.

use ostra_core::{ExecutionId, HarnessKind};
use serde::{Deserialize, Serialize};

pub const ENV_URL: &str = "OSTRA_URL";
pub const ENV_EXECUTION: &str = "OSTRA_EXECUTION";
pub const ENV_TOKEN: &str = "OSTRA_TOKEN";
pub const ENV_HARNESS: &str = "OSTRA_HARNESS";

pub const POLICY_PATH: &str = "/internal/policy";
pub const MCP_PATH: &str = "/internal/mcp";

/// MCP server name every harness registers.
pub const MCP_SERVER_NAME: &str = "ostra";

/// Hook lifecycle events Ostra registers. Passed on the hook command line as `--event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    /// Claude Code reports failed tool calls as their own event.
    PostToolUseFailure,
    Stop,
    SessionStart,
    /// Antigravity's per-model-call events, used as activity and turn-end signals.
    PreInvocation,
    PostInvocation,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "pre_tool_use",
            HookEvent::PostToolUse => "post_tool_use",
            HookEvent::PostToolUseFailure => "post_tool_use_failure",
            HookEvent::Stop => "stop",
            HookEvent::SessionStart => "session_start",
            HookEvent::PreInvocation => "pre_invocation",
            HookEvent::PostInvocation => "post_invocation",
        }
    }

    pub fn parse(s: &str) -> Option<HookEvent> {
        Some(match s.trim() {
            "pre_tool_use" | "PreToolUse" => HookEvent::PreToolUse,
            "post_tool_use" | "PostToolUse" => HookEvent::PostToolUse,
            "post_tool_use_failure" | "PostToolUseFailure" => HookEvent::PostToolUseFailure,
            "stop" | "Stop" => HookEvent::Stop,
            "session_start" | "SessionStart" => HookEvent::SessionStart,
            "pre_invocation" | "PreInvocation" => HookEvent::PreInvocation,
            "post_invocation" | "PostInvocation" => HookEvent::PostInvocation,
            _ => return None,
        })
    }
}

/// Body of `POST /internal/policy`. The per-execution token rides in `Authorization: Bearer`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyRequest {
    pub execution: ExecutionId,
    pub harness: HarnessKind,
    pub event: HookEvent,
    pub payload: serde_json::Value,
}

/// What `ostra hook` prints and how it exits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyResponse {
    /// Printed verbatim to stdout. Empty prints nothing.
    pub stdout: String,
    pub exit_code: i32,
}

impl PolicyResponse {
    pub fn json(value: serde_json::Value) -> Self {
        let stdout = if value.as_object().is_some_and(|o| o.is_empty()) {
            "{}".into()
        } else {
            value.to_string()
        };
        PolicyResponse {
            stdout,
            exit_code: 0,
        }
    }

    pub fn empty() -> Self {
        PolicyResponse {
            stdout: String::new(),
            exit_code: 0,
        }
    }
}

/// Body of `POST /internal/mcp`: one JSON-RPC message from the harness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpRequest {
    pub execution: ExecutionId,
    pub message: serde_json::Value,
}

/// `message` is absent for notifications, which take no response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpResponse {
    pub message: Option<serde_json::Value>,
}
