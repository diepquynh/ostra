//! Codex: hooks see `Bash {command}` for exec_command, `apply_patch {command: <patch>}`, and
//! `mcp__<server>__<tool>`. Its PreToolUse schema rejects `permissionDecision: "allow"` without an
//! `updatedInput`, and rejects `ask`, so an allowed call answers `{}`.

use super::*;
use crate::protocol::HookEvent;
use ostra_core::HarnessKind;
use ostra_core::policy::ToolCall;
use serde_json::{Value, json};

pub struct Codex;

/// The canonical tool for a Codex shell call. On Unix it is `Bash`. On Windows Codex runs its
/// commands in PowerShell unless the argv starts a POSIX shell, and Ostra's parser reads only
/// bash, so anything else becomes the opaque `PowerShell` tool, which is never auto-allowed and
/// has its raw text scanned for Ostra's own files and credentials.
fn windows_shell_tool(input: &Value) -> &'static str {
    if !cfg!(windows) {
        return "Bash";
    }
    let first = match input.get("command").or_else(|| input.get("cmd")) {
        Some(Value::Array(parts)) => parts.first().and_then(Value::as_str).map(str::to_string),
        _ => None,
    };
    let posix = first.is_some_and(|w| {
        let name = w
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&w)
            .to_ascii_lowercase();
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        matches!(name, "bash" | "sh")
    });
    if posix { "Bash" } else { "PowerShell" }
}

impl Adapter for Codex {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Codex
    }

    fn meta(&self, payload: &Value) -> HookMeta {
        meta_from(payload, &["session_id", "sessionId"])
    }

    fn parse_pre(&self, payload: &Value) -> PreParse {
        let name = get_str(payload, &["tool_name", "toolName"]).unwrap_or("");
        let input = get(payload, &["tool_input", "toolInput"])
            .cloned()
            .unwrap_or(json!({}));
        let cwd = get_str(payload, &["cwd"]);
        if is_spawn_tool(name) {
            return PreParse::Refuse(LEAF_REFUSAL.into());
        }
        if is_ask_tool(name) {
            return PreParse::Refuse(ASK_REFUSAL.into());
        }
        let call = if let Some(tool) = ostra_mcp_tool(name) {
            ToolCall::new(tool, input)
        } else {
            match name {
                "Bash" | "exec_command" | "shell" | "local_shell" => {
                    let command = match input.get("command").or_else(|| input.get("cmd")) {
                        Some(Value::String(s)) => s.clone(),
                        // Quoted, so the policy sees the words Codex runs, not a re-split line.
                        Some(Value::Array(parts)) => parts
                            .iter()
                            .filter_map(Value::as_str)
                            .map(crate::launch::posix_quote)
                            .collect::<Vec<_>>()
                            .join(" "),
                        _ => String::new(),
                    };
                    ToolCall::new(
                        windows_shell_tool(&input),
                        with_cwd(json!({"command": command}), cwd),
                    )
                }
                "apply_patch" => {
                    let patch = get_str(&input, &["command", "patch", "input"])
                        .unwrap_or("")
                        .to_string();
                    ToolCall::new("ApplyPatch", with_cwd(json!({"patch": patch}), cwd))
                }
                other_name => other(other_name, &input),
            }
        };
        PreParse::Call {
            call,
            native: name.to_string(),
        }
    }

    fn parse_post(&self, _event: HookEvent, payload: &Value) -> Option<PostParse> {
        let PreParse::Call { call, .. } = self.parse_pre(payload) else {
            return None;
        };
        let response = get(payload, &["tool_response", "toolResponse"])
            .cloned()
            .unwrap_or(Value::Null);
        // Codex hands hooks the raw command output without its exit status, so an absent code
        // stays unknown and the policy judges the output text instead.
        let outcome = outcome_from(&response, None, false);
        Some(PostParse { call, outcome })
    }

    fn pre_response(&self, decision: &Decision) -> Value {
        match decision {
            Decision::Allow { note } => match note.as_ref().filter(|n| !n.is_empty()) {
                Some(n) => {
                    json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": n}})
                }
                None => json!({}),
            },
            Decision::Deny { reason } => json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                }
            }),
        }
    }

    fn post_response(&self, notes: &[String]) -> Value {
        post_context(notes)
    }

    fn stop_response(&self, block: Option<&str>) -> Value {
        stop_block(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn payload(tool: &str, input: Value) -> Value {
        json!({
            "session_id": "019a0000-0000-7000-8000-000000000001",
            "turn_id": "t1",
            "transcript_path": null,
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "model": "gpt-5.6-luna",
            "permission_mode": "bypassPermissions",
            "tool_name": tool,
            "tool_input": input,
            "tool_use_id": "call_1",
        })
    }

    #[test]
    fn parses() {
        let PreParse::Call { call, .. } =
            Codex.parse_pre(&payload("Bash", json!({"command": "ls -la"})))
        else {
            panic!()
        };
        // On Windows a command string runs in PowerShell, which the policy treats as opaque.
        let shell = if cfg!(windows) { "PowerShell" } else { "Bash" };
        assert_eq!(
            call,
            ToolCall::new(shell, json!({"command": "ls -la", "cwd": "/repo"}))
        );

        let PreParse::Call { call, .. } = Codex.parse_pre(&payload(
            "exec_command",
            json!({"cmd": ["bash", "-lc", "echo a; rm -rf ~/x"]}),
        )) else {
            panic!()
        };
        assert_eq!(
            call.str_field("command"),
            Some("bash -lc 'echo a; rm -rf ~/x'")
        );

        let patch = "*** Begin Patch\n*** Add File: src/a.rs\n+x\n*** End Patch\n";
        let PreParse::Call { call, .. } =
            Codex.parse_pre(&payload("apply_patch", json!({"command": patch})))
        else {
            panic!()
        };
        assert_eq!(call.tool, "ApplyPatch");
        assert_eq!(call.str_field("patch"), Some(patch));

        let PreParse::Call { call, .. } =
            Codex.parse_pre(&payload("mcp__ostra__memory_recall", json!({"query": "x"})))
        else {
            panic!()
        };
        assert_eq!(call.tool, "MemoryRecall");

        let PreParse::Call { call, .. } = Codex.parse_pre(&payload(
            "mcp__ostra__code_impact",
            json!({"symbol": "releaseStock"}),
        )) else {
            panic!()
        };
        assert_eq!(
            (call.tool.as_str(), call.str_field("symbol")),
            ("CodeImpact", Some("releaseStock"))
        );

        assert!(matches!(
            Codex.parse_pre(&payload("spawn_agent", json!({}))),
            PreParse::Refuse(_)
        ));
        assert!(matches!(
            Codex.parse_pre(&payload("collaborationspawn_agent", json!({}))),
            PreParse::Refuse(_)
        ));
        assert!(matches!(
            Codex.parse_pre(&payload("request_user_input", json!({}))),
            PreParse::Refuse(_)
        ));
        assert_eq!(
            Codex.meta(&payload("Bash", json!({}))).transcript_path,
            None
        );
    }

    #[test]
    fn post_reads_exit_code_from_text() {
        let mut p = payload("Bash", json!({"command": "cargo test"}));
        p["hook_event_name"] = json!("PostToolUse");
        p["tool_response"] = json!(
            "Chunk ID: 1\nWall time: 0.5 seconds\nProcess exited with code 101\nOutput:\nerror"
        );
        let post = Codex.parse_post(HookEvent::PostToolUse, &p).unwrap();
        assert_eq!(post.outcome.exit_code, Some(101));
        assert!(post.outcome.is_error);
    }

    #[test]
    fn unknown_exit_status_stays_unknown() {
        let mut p = payload("Bash", json!({"command": "echo hi"}));
        p["tool_response"] = json!("");
        let post = Codex.parse_post(HookEvent::PostToolUse, &p).unwrap();
        assert_eq!(post.outcome.exit_code, None);
        assert!(!post.outcome.is_error);
    }

    #[test]
    fn responses_fit_the_strict_schema() {
        assert_eq!(
            Codex.pre_response(&Decision::Allow { note: None }),
            json!({})
        );
        assert_eq!(
            Codex.pre_response(&Decision::Allow {
                note: Some("n".into())
            }),
            json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": "n"}})
        );
        let deny = Codex.pre_response(&Decision::Deny { reason: "r".into() });
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(
            deny.get("reason").is_none(),
            "top-level reason without decision is not in the wire schema"
        );
    }
}
