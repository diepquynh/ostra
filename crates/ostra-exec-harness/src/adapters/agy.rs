//! Antigravity: payloads nest the call in `toolCall.{name,args}` with PascalCase arguments. Hook
//! output is proto-validated and an unknown field discards the whole response, so PreToolUse
//! answers only `decision` and `reason`, and PostToolUse answers `{}`. PostToolUse carries no
//! tool result: only `stepIdx` and, on failure, `error`.

use super::*;
use crate::protocol::HookEvent;
use ostra_core::HarnessKind;
use ostra_core::policy::{ToolCall, ToolOutcome};
use serde_json::{Value, json};

pub struct Agy;

fn call_parts(payload: &Value) -> (String, Value) {
    let tc = payload.get("toolCall").or_else(|| payload.get("tool_call"));
    let name = tc
        .and_then(|t| get_str(t, &["name", "toolName"]))
        .or_else(|| get_str(payload, &["toolName", "tool_name"]))
        .unwrap_or("")
        .to_string();
    let args = tc
        .and_then(|t| get(t, &["args", "input", "arguments"]))
        .or_else(|| get(payload, &["toolInput", "tool_input"]))
        .cloned()
        .unwrap_or(json!({}));
    let args = match args {
        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        v => v,
    };
    (name, args)
}

impl Adapter for Agy {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Agy
    }

    fn meta(&self, payload: &Value) -> HookMeta {
        meta_from(
            payload,
            &[
                "conversationId",
                "conversation_id",
                "sessionId",
                "session_id",
            ],
        )
    }

    fn parse_pre(&self, payload: &Value) -> PreParse {
        let (name, args) = call_parts(payload);
        if is_spawn_tool(&name) {
            return PreParse::Refuse(LEAF_REFUSAL.into());
        }
        if is_ask_tool(&name) {
            return PreParse::Refuse(ASK_REFUSAL.into());
        }
        let cwd = get_str(&args, &["Cwd", "cwd"]).or_else(|| get_str(payload, &["cwd"]));
        let call = if let Some(tool) = ostra_mcp_tool(&name) {
            ToolCall::new(tool, args)
        } else if name == "call_mcp_tool" {
            let server = get_str(&args, &["ServerName", "serverName"]).unwrap_or("");
            let tool = get_str(&args, &["ToolName", "toolName"]).unwrap_or("");
            let inner = get(&args, &["Arguments", "arguments"])
                .cloned()
                .unwrap_or(json!({}));
            if server == crate::protocol::MCP_SERVER_NAME {
                ToolCall::new(canonical_ostra_tool(tool), inner)
            } else {
                other(&format!("mcp__{server}__{tool}"), &inner)
            }
        } else {
            match name.as_str() {
                "view_file" | "view_file_outline" | "view_code_item" => ToolCall::new(
                    "Read",
                    with_file_path(&args, &["AbsolutePath", "File", "TargetFile", "path"]),
                ),
                "write_to_file" => {
                    let mut v = with_file_path(&args, &["TargetFile", "AbsolutePath", "path"]);
                    if let Some(content) = get_str(&args, &["CodeContent", "Content"]) {
                        v["content"] = json!(content);
                    }
                    ToolCall::new("Write", v)
                }
                "replace_file_content" | "multi_replace_file_content" => ToolCall::new(
                    "Edit",
                    with_file_path(&args, &["TargetFile", "AbsolutePath", "path"]),
                ),
                "run_command" | "send_command_input" => {
                    let command = get_str(&args, &["CommandLine", "Command", "Input"])
                        .unwrap_or("")
                        .to_string();
                    ToolCall::new("Bash", with_cwd(json!({"command": command}), cwd))
                }
                "grep_search" => {
                    let mut m = object(&args);
                    if let Some(q) = get_str(&args, &["Query", "Pattern"]) {
                        m.insert("pattern".into(), json!(q));
                    }
                    if let Some(p) = get_str(&args, &["SearchPath", "SearchDirectory"]) {
                        m.insert("path".into(), json!(p));
                    }
                    ToolCall::new("Grep", Value::Object(m))
                }
                "find_by_name" | "list_dir" => {
                    let mut m = object(&args);
                    m.insert(
                        "pattern".into(),
                        json!(get_str(&args, &["Pattern"]).unwrap_or("*")),
                    );
                    if let Some(p) = get_str(&args, &["SearchDirectory", "DirectoryPath"]) {
                        m.insert("path".into(), json!(p));
                    }
                    ToolCall::new("Glob", Value::Object(m))
                }
                "read_url_content" => {
                    let mut m = object(&args);
                    if let Some(u) = get_str(&args, &["Url", "url"]) {
                        m.insert("url".into(), json!(u));
                    }
                    ToolCall::new("WebFetch", Value::Object(m))
                }
                "search_web" => {
                    let mut m = object(&args);
                    if let Some(q) = get_str(&args, &["query", "Query"]) {
                        m.insert("query".into(), json!(q));
                    }
                    ToolCall::new("WebSearch", Value::Object(m))
                }
                other_name => other(other_name, &args),
            }
        };
        PreParse::Call { call, native: name }
    }

    fn parse_post(&self, _event: HookEvent, payload: &Value) -> Option<PostParse> {
        let PreParse::Call { call, .. } = self.parse_pre(payload) else {
            return None;
        };
        let error = get_str(payload, &["error"]);
        let bash = call.tool == "Bash";
        let outcome = match error {
            Some(err) => {
                let code = exit_code_from(err).or(if bash { Some(1) } else { None });
                ToolOutcome {
                    output: err.to_string(),
                    is_error: true,
                    exit_code: code,
                    result_known: false,
                }
            }
            None => ToolOutcome {
                output: String::new(),
                is_error: false,
                exit_code: if bash { Some(0) } else { None },
                result_known: false,
            },
        };
        Some(PostParse { call, outcome })
    }

    fn pre_response(&self, decision: &Decision) -> Value {
        match decision {
            Decision::Allow { .. } => json!({"decision": "allow"}),
            Decision::Deny { reason } => json!({"decision": "deny", "reason": reason}),
        }
    }

    fn post_response(&self, _notes: &[String]) -> Value {
        json!({})
    }

    fn stop_response(&self, _block: Option<&str>) -> Value {
        json!({})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn payload(name: &str, args: Value) -> Value {
        json!({"cwd": "/repo", "conversationId": "676523eb", "stepIdx": 4, "toolCall": {"name": name, "args": args}})
    }

    #[test]
    fn parses_nested_calls() {
        let PreParse::Call { call, .. } = Agy.parse_pre(&payload(
            "run_command",
            json!({"CommandLine": "./mvnw -q compile", "Cwd": "/repo/sub"}),
        )) else {
            panic!()
        };
        assert_eq!(
            call.input,
            json!({"command": "./mvnw -q compile", "cwd": "/repo/sub"})
        );

        let PreParse::Call { call, .. } = Agy.parse_pre(&payload(
            "write_to_file",
            json!({"TargetFile": "/repo/a.md", "CodeContent": "x"}),
        )) else {
            panic!()
        };
        assert_eq!(
            (
                call.tool.as_str(),
                call.str_field("file_path"),
                call.str_field("content")
            ),
            ("Write", Some("/repo/a.md"), Some("x"))
        );

        assert!(matches!(
            Agy.parse_pre(&payload(
                "invoke_subagent",
                json!({"Subagents": [{"TypeName": "x"}]})
            )),
            PreParse::Refuse(_)
        ));
        assert_eq!(
            Agy.meta(&payload("x", json!({}))).session_id.as_deref(),
            Some("676523eb")
        );

        let PreParse::Call { call, .. } = Agy.parse_pre(&payload(
            "call_mcp_tool",
            json!({"ServerName": "ostra", "ToolName": "submit_quick_answer", "Arguments": {"answer": "a"}, "toolAction": "x"}),
        )) else {
            panic!()
        };
        assert_eq!(
            call,
            ToolCall::new("submit_quick_answer", json!({"answer": "a"}))
        );
        let PreParse::Call { call, .. } = Agy.parse_pre(&payload(
            "call_mcp_tool",
            json!({"ServerName": "github", "ToolName": "x", "Arguments": {}}),
        )) else {
            panic!()
        };
        assert_eq!(call.tool, "Other:mcp__github__x");
    }

    #[test]
    fn post_reads_error_field() {
        let mut p = payload("run_command", json!({"CommandLine": "make"}));
        p["error"] = json!("exit status 2");
        let post = Agy.parse_post(HookEvent::PostToolUse, &p).unwrap();
        assert_eq!(post.outcome.exit_code, Some(2));
        assert!(!post.outcome.result_known);
        let ok = Agy
            .parse_post(
                HookEvent::PostToolUse,
                &payload("run_command", json!({"CommandLine": "make"})),
            )
            .unwrap();
        assert_eq!(ok.outcome.exit_code, Some(0));
    }

    #[test]
    fn responses_have_no_unknown_fields() {
        assert_eq!(
            Agy.pre_response(&Decision::Allow {
                note: Some("dropped".into())
            }),
            json!({"decision": "allow"})
        );
        assert_eq!(
            Agy.pre_response(&Decision::Deny { reason: "r".into() }),
            json!({"decision": "deny", "reason": "r"})
        );
        assert_eq!(Agy.post_response(&["x".into()]), json!({}));
    }
}
