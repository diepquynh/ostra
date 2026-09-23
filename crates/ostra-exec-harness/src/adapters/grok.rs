//! Grok Build: camelCase payloads with snake_case aliases, Claude-shaped decisions, reasons clipped
//! to 256 characters from the front, and a 128 KiB payload cap past which `toolInput` becomes a
//! truncated string. A truncated input cannot be judged, so it is refused.

use super::*;
use crate::protocol::HookEvent;
use ostra_core::HarnessKind;
use ostra_core::policy::ToolCall;
use serde_json::{Value, json};

pub struct Grok;

/// Grok's `MAX_REASON_CHARS`.
pub const GROK_REASON_MAX: usize = 256;
const MIN_HEAD: usize = 120;

pub const TRUNCATED_REFUSAL: &str = "Split this call into smaller ones and retry: its input passed the 128 KiB hook \
    payload cap, so Ostra could not check it. Write a large file in several edits.";

/// Refit a reason to 256 characters keeping the head and the final sentence, because Ostra's
/// reasons lead with the correction and end with the instruction.
pub fn fit_reason(reason: &str) -> String {
    let flat = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= GROK_REASON_MAX {
        return flat;
    }
    let sentences: Vec<&str> = flat.split_inclusive(['.', '!', '?']).collect();
    let tail = sentences.last().map(|s| s.trim()).unwrap_or("");
    let joiner = " … ";
    let tail_len = tail.chars().count();
    let head_budget = GROK_REASON_MAX.saturating_sub(tail_len + joiner.chars().count());
    if head_budget < MIN_HEAD {
        return flat.chars().take(GROK_REASON_MAX).collect();
    }
    let head: String = flat.chars().take(head_budget).collect();
    format!("{}{joiner}{tail}", head.trim_end())
}

impl Adapter for Grok {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Grok
    }

    fn meta(&self, payload: &Value) -> HookMeta {
        meta_from(payload, &["sessionId", "session_id"])
    }

    fn parse_pre(&self, payload: &Value) -> PreParse {
        if payload
            .get("toolInputTruncated")
            .or_else(|| payload.get("tool_input_truncated"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            return PreParse::Refuse(TRUNCATED_REFUSAL.into());
        }
        let name = get_str(payload, &["toolName", "tool_name"]).unwrap_or("");
        let input = get(payload, &["toolInput", "tool_input"])
            .cloned()
            .unwrap_or(json!({}));
        let cwd = get_str(payload, &["cwd"]);
        if is_spawn_tool(name) {
            return PreParse::Refuse(LEAF_REFUSAL.into());
        }
        if is_ask_tool(name) {
            return PreParse::Refuse(ASK_REFUSAL.into());
        }
        const PATH_KEYS: &[&str] = &["file_path", "filePath", "path", "target_file", "targetFile"];
        let call = if let Some(tool) = ostra_mcp_tool(name) {
            // Grok wraps MCP arguments as `{tool_name, tool_input}` in the hook payload.
            let args = match input.get("tool_input") {
                Some(inner) if input.get("tool_name").is_some() => inner.clone(),
                _ => input,
            };
            ToolCall::new(tool, args)
        } else {
            match name {
                "use_tool" => {
                    let inner = get_str(&input, &["tool_name", "toolName", "name"]).unwrap_or("");
                    let args = get(&input, &["arguments", "args", "input", "tool_input"])
                        .cloned()
                        .unwrap_or(json!({}));
                    let args = match args {
                        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
                        v => v,
                    };
                    match ostra_mcp_tool(inner) {
                        Some(tool) => ToolCall::new(tool, args),
                        None => other(&format!("use_tool:{inner}"), &args),
                    }
                }
                "read_file" | "hashline_read" | "Read" => {
                    ToolCall::new("Read", with_file_path(&input, PATH_KEYS))
                }
                "write" | "write_file" | "Write" => {
                    ToolCall::new("Write", with_file_path(&input, PATH_KEYS))
                }
                "search_replace" | "hashline_edit" | "edit_file" | "Edit" => {
                    ToolCall::new("Edit", with_file_path(&input, PATH_KEYS))
                }
                "run_terminal_command" | "Bash" | "monitor" => {
                    let command = get_str(&input, &["command", "cmd"])
                        .unwrap_or("")
                        .to_string();
                    ToolCall::new("Bash", with_cwd(json!({"command": command}), cwd))
                }
                "grep" | "Grep" => {
                    let mut m = object(&input);
                    if let Some(q) = get_str(&input, &["pattern", "query", "regex"]) {
                        m.insert("pattern".into(), json!(q));
                    }
                    ToolCall::new("Grep", Value::Object(m))
                }
                "list_dir" | "glob" | "Glob" => {
                    let mut m = object(&input);
                    m.entry("pattern").or_insert(json!("*"));
                    if let Some(p) = get_str(&input, &["path", "dir", "directory"]) {
                        m.insert("path".into(), json!(p));
                    }
                    ToolCall::new("Glob", Value::Object(m))
                }
                "web_fetch" => ToolCall::new("WebFetch", input),
                "web_search" => ToolCall::new("WebSearch", input),
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
        let response = get(
            payload,
            &["toolResponse", "tool_response", "toolResult", "tool_result"],
        )
        .cloned()
        .unwrap_or(Value::Null);
        let outcome = outcome_from(&response, get_str(payload, &["error"]), call.tool == "Bash");
        Some(PostParse { call, outcome })
    }

    fn pre_response(&self, decision: &Decision) -> Value {
        claude_shaped_pre(decision, fit_reason)
    }

    fn post_response(&self, notes: &[String]) -> Value {
        post_context(notes)
    }

    fn stop_response(&self, block: Option<&str>) -> Value {
        match block {
            Some(r) => json!({"decision": "block", "reason": fit_reason(r)}),
            None => json!({}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn payload(tool: &str, input: Value) -> Value {
        json!({
            "sessionId": "0199aaaa-bbbb-7ccc-8ddd-eeeeeeeeeeee",
            "session_id": "0199aaaa-bbbb-7ccc-8ddd-eeeeeeeeeeee",
            "hookEventName": "PreToolUse",
            "cwd": "/repo",
            "transcriptPath": "/home/u/.grok/sessions/x.jsonl",
            "toolName": tool,
            "toolInput": input,
        })
    }

    #[test]
    fn parses_grok_tools() {
        let PreParse::Call { call, .. } = Grok.parse_pre(&payload(
            "search_replace",
            json!({"file_path": "src/a.rs", "old_string": "a", "new_string": "b"}),
        )) else {
            panic!()
        };
        assert_eq!(
            (call.tool.as_str(), call.str_field("file_path")),
            ("Edit", Some("src/a.rs"))
        );

        let PreParse::Call { call, .. } =
            Grok.parse_pre(&payload("run_terminal_command", json!({"command": "make"})))
        else {
            panic!()
        };
        assert_eq!(call.input, json!({"command": "make", "cwd": "/repo"}));

        let PreParse::Call { call, .. } = Grok.parse_pre(&payload(
            "use_tool",
            json!({"tool_name": "ostra__submit_explore", "arguments": "{\"research_path\":\"/s/r.md\"}"}),
        )) else {
            panic!()
        };
        assert_eq!(call.tool, "submit_explore");
        assert_eq!(call.str_field("research_path"), Some("/s/r.md"));

        let PreParse::Call { call, .. } = Grok.parse_pre(&payload(
            "ostra__report",
            json!({"tool_name": "ostra__report", "tool_input": {"content": "# Report"}}),
        )) else {
            panic!()
        };
        assert_eq!(
            (call.tool.as_str(), call.str_field("content")),
            ("Report", Some("# Report"))
        );

        assert!(matches!(
            Grok.parse_pre(&payload("spawn_subagent", json!({}))),
            PreParse::Refuse(_)
        ));
        assert!(matches!(
            Grok.parse_pre(&payload("ask_user_question", json!({}))),
            PreParse::Refuse(_)
        ));
    }

    #[test]
    fn truncated_input_is_refused() {
        let mut p = payload("write", json!("…enormous serialized input… [truncated]"));
        p["toolInputTruncated"] = json!(true);
        assert_eq!(
            Grok.parse_pre(&p),
            PreParse::Refuse(TRUNCATED_REFUSAL.into())
        );
    }

    #[test]
    fn reasons_fit_256() {
        let long = format!(
            "{} Call submit_implementer instead.",
            "The write target is outside the repo root. ".repeat(12)
        );
        let fit = fit_reason(&long);
        assert!(
            fit.chars().count() <= GROK_REASON_MAX,
            "{}",
            fit.chars().count()
        );
        assert!(fit.ends_with("Call submit_implementer instead."));
        assert_eq!(fit_reason("short"), "short");
        let deny = Grok.pre_response(&Decision::Deny { reason: long });
        assert!(
            deny["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 256
        );
    }
}
