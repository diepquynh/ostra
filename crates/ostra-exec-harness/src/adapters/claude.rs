//! Claude Code: snake_case payloads, canonical tool names already. Rewrites and decisions go only
//! in `hookSpecificOutput`, because a top-level `overwrite` fails its hook schema.

use super::*;
use crate::protocol::HookEvent;
use ostra_core::HarnessKind;
use ostra_core::policy::ToolCall;
use serde_json::{Value, json};

pub struct Claude;

pub(crate) fn canonical_claude_call(name: &str, input: &Value, cwd: Option<&str>) -> PreParse {
    if is_spawn_tool(name) {
        return PreParse::Refuse(LEAF_REFUSAL.into());
    }
    if is_ask_tool(name) {
        return PreParse::Refuse(ASK_REFUSAL.into());
    }
    let call = if let Some(tool) = ostra_mcp_tool(name) {
        ToolCall::new(tool, input.clone())
    } else {
        match name {
            "Read" | "Write" | "Edit" | "Grep" | "Glob" | "WebFetch" | "WebSearch" | "Skill" => {
                ToolCall::new(name, input.clone())
            }
            "MultiEdit" => ToolCall::new("Edit", input.clone()),
            "NotebookEdit" => ToolCall::new(
                "Edit",
                with_file_path(input, &["notebook_path", "file_path"]),
            ),
            "Bash" => ToolCall::new("Bash", with_cwd(input.clone(), cwd)),
            "LS" => {
                let path = get_str(input, &["path"]).unwrap_or(".");
                ToolCall::new("Glob", json!({"pattern": "*", "path": path}))
            }
            other_name => other(other_name, input),
        }
    };
    PreParse::Call {
        call,
        native: name.to_string(),
    }
}

impl Adapter for Claude {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Claude
    }

    fn meta(&self, payload: &Value) -> HookMeta {
        meta_from(payload, &["session_id", "sessionId"])
    }

    fn parse_pre(&self, payload: &Value) -> PreParse {
        let name = get_str(payload, &["tool_name", "toolName"]).unwrap_or("");
        let input = get(payload, &["tool_input", "toolInput"])
            .cloned()
            .unwrap_or(json!({}));
        canonical_claude_call(name, &input, get_str(payload, &["cwd"]))
    }

    fn parse_post(&self, event: HookEvent, payload: &Value) -> Option<PostParse> {
        let PreParse::Call { call, native } = self.parse_pre(payload) else {
            return None;
        };
        let response = get(payload, &["tool_response", "toolResponse"])
            .cloned()
            .unwrap_or(Value::Null);
        let error = match event {
            HookEvent::PostToolUseFailure => {
                Some(get_str(payload, &["error"]).unwrap_or("tool call failed"))
            }
            _ => None,
        };
        let mut outcome = outcome_from(&response, error, native == "Bash");
        if payload.get("is_interrupt").and_then(Value::as_bool) == Some(true) {
            outcome.exit_code = None;
        }
        Some(PostParse { call, outcome })
    }

    fn pre_response(&self, decision: &Decision) -> Value {
        claude_shaped_pre(decision, |r| r.to_string())
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

    fn pre(tool: &str, input: Value) -> Value {
        json!({
            "session_id": "11111111-2222-4333-8444-555555555555",
            "transcript_path": "/home/u/.claude/projects/p/1111.jsonl",
            "cwd": "/repo",
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "tool_input": input,
        })
    }

    #[test]
    fn parses_native_and_mcp_tools() {
        let p = pre("Write", json!({"file_path": "/repo/a.rs", "content": "x"}));
        let PreParse::Call { call, .. } = Claude.parse_pre(&p) else {
            panic!()
        };
        assert_eq!(call.tool, "Write");
        assert_eq!(call.str_field("file_path"), Some("/repo/a.rs"));

        let p = pre("Bash", json!({"command": "cargo build"}));
        let PreParse::Call { call, .. } = Claude.parse_pre(&p) else {
            panic!()
        };
        assert_eq!(call.str_field("cwd"), Some("/repo"));

        let p = pre("mcp__ostra__submit_implementer", json!({"status": "ok"}));
        let PreParse::Call { call, .. } = Claude.parse_pre(&p) else {
            panic!()
        };
        assert_eq!(call.tool, "submit_implementer");

        let p = pre("NotebookEdit", json!({"notebook_path": "/repo/n.ipynb"}));
        let PreParse::Call { call, .. } = Claude.parse_pre(&p) else {
            panic!()
        };
        assert_eq!(
            (call.tool.as_str(), call.str_field("file_path")),
            ("Edit", Some("/repo/n.ipynb"))
        );

        assert!(matches!(
            Claude.parse_pre(&pre("Agent", json!({}))),
            PreParse::Refuse(_)
        ));
        assert!(matches!(
            Claude.parse_pre(&pre("AskUserQuestion", json!({}))),
            PreParse::Refuse(_)
        ));

        let meta = Claude.meta(&p);
        assert_eq!(
            meta.session_id.as_deref(),
            Some("11111111-2222-4333-8444-555555555555")
        );
    }

    #[test]
    fn post_outcomes() {
        let mut p = pre("Bash", json!({"command": "cargo test"}));
        p["tool_response"] = json!({"stdout": "ok", "stderr": "", "interrupted": false});
        let post = Claude.parse_post(HookEvent::PostToolUse, &p).unwrap();
        assert_eq!(post.outcome.exit_code, Some(0));
        assert!(!post.outcome.is_error);

        let mut f = pre("Bash", json!({"command": "cargo test"}));
        f["hook_event_name"] = json!("PostToolUseFailure");
        f["error"] = json!("Exit code 101\nerror[E0425]: cannot find value");
        let post = Claude
            .parse_post(HookEvent::PostToolUseFailure, &f)
            .unwrap();
        assert_eq!(post.outcome.exit_code, Some(101));
        assert!(post.outcome.is_error);
    }

    #[test]
    fn responses() {
        assert_eq!(
            Claude.pre_response(&Decision::Deny {
                reason: "no".into()
            }),
            json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": "no"}})
        );
        assert_eq!(
            Claude.pre_response(&Decision::Allow { note: None }),
            json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "allow"}})
        );
        assert_eq!(Claude.post_response(&[]), json!({}));
        assert_eq!(
            Claude.post_response(&["a".into(), "b".into()]),
            json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": "a\n\nb"}})
        );
        assert_eq!(
            Claude.stop_response(Some("call submit")),
            json!({"decision": "block", "reason": "call submit"})
        );
        assert_eq!(Claude.stop_response(None), json!({}));
    }
}
