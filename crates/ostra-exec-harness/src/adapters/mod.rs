//! Per-harness hook adapters. Each turns its harness's hook payload into a canonical
//! [`ToolCall`] (Claude Code tool names and input shapes) and formats Ostra's decision into the
//! exact response shape that harness accepts.

pub mod agy;
pub mod claude;
pub mod codex;
pub mod grok;

use crate::protocol::{HookEvent, MCP_SERVER_NAME};
use ostra_core::HarnessKind;
use ostra_core::policy::{ToolCall, ToolOutcome};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

/// Largest tool output forwarded to the policy observer.
pub const MAX_OUTCOME_CHARS: usize = 64 * 1024;

pub const LEAF_REFUSAL: &str = "Do this work yourself with your own tools and spawn no subagent. Every Ostra agent \
    is a leaf, so a delegated task would run outside Ostra's guards and its result would never reach the engine.";

pub const ASK_REFUSAL: &str = "Put the question in your submit call instead of asking here. Ostra asks the user \
    from the pipeline's gate, because nobody watches this terminal for questions.";

/// Payload fields every harness may carry, read the same way for every event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HookMeta {
    pub session_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
    pub cwd: Option<String>,
    pub last_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreParse {
    /// A tool call for the policy engine. `native` is the harness's own tool name.
    Call { call: ToolCall, native: String },
    /// Refuse without consulting the policy engine (spawns, user questions, unreadable input).
    Refuse(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PostParse {
    pub call: ToolCall,
    pub outcome: ToolOutcome,
}

/// Ostra's final answer to a PreToolUse event. Asks are resolved before this point.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow { note: Option<String> },
    Deny { reason: String },
}

pub trait Adapter: Sync {
    fn kind(&self) -> HarnessKind;
    fn meta(&self, payload: &Value) -> HookMeta;
    fn parse_pre(&self, payload: &Value) -> PreParse;
    fn parse_post(&self, event: HookEvent, payload: &Value) -> Option<PostParse>;
    fn pre_response(&self, decision: &Decision) -> Value;
    fn post_response(&self, notes: &[String]) -> Value;
    fn stop_response(&self, block: Option<&str>) -> Value;
}

pub fn for_harness(kind: HarnessKind) -> &'static dyn Adapter {
    match kind {
        HarnessKind::Claude => &claude::Claude,
        HarnessKind::Codex => &codex::Codex,
        HarnessKind::Grok => &grok::Grok,
        HarnessKind::Agy => &agy::Agy,
    }
}

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

pub(crate) fn get<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| v.get(*k).filter(|x| !x.is_null()))
}

pub(crate) fn get_str<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
}

pub(crate) fn meta_from(payload: &Value, session_keys: &[&str]) -> HookMeta {
    HookMeta {
        session_id: get_str(payload, session_keys).map(str::to_string),
        transcript_path: get_str(payload, &["transcript_path", "transcriptPath"])
            .map(PathBuf::from),
        cwd: get_str(payload, &["cwd", "Cwd"]).map(str::to_string),
        last_message: get_str(payload, &["last_assistant_message", "lastAssistantMessage"])
            .map(str::to_string),
    }
}

/// Canonical name of an Ostra MCP tool as each harness spells it, or `None` for any other tool.
pub fn ostra_mcp_tool(name: &str) -> Option<String> {
    let s = MCP_SERVER_NAME;
    let prefixes = [
        format!("mcp__{s}__"),
        format!("mcp_{s}_"),
        format!("{s}__"),
        format!("{s}/"),
        format!("{s}."),
        format!("{s}:"),
    ];
    let bare = prefixes
        .iter()
        .find_map(|p| name.strip_prefix(p.as_str()))?;
    Some(canonical_ostra_tool(bare))
}

/// `report` to `Report`, `document` to `Document`, `memory` to `Memory`, `memory_recall` to
/// `MemoryRecall`, `code_callers` to `CodeCallers`; `submit_*` keeps its name.
pub fn canonical_ostra_tool(bare: &str) -> String {
    match bare {
        "report" => "Report".into(),
        "document" => "Document".into(),
        "memory" => "Memory".into(),
        "memory_recall" => "MemoryRecall".into(),
        other => bare
            .strip_prefix("code_")
            .and_then(|op| ostra_core::agent::CODE_TOOLS.iter().find(|(o, _)| *o == op))
            .map_or_else(|| other.to_string(), |(_, native)| native.to_string()),
    }
}

pub(crate) fn is_spawn_tool(name: &str) -> bool {
    matches!(
        name,
        "Task"
            | "Agent"
            | "task"
            | "spawn_agent"
            | "spawn_subagent"
            | "invoke_subagent"
            | "define_subagent"
    ) || name.ends_with("spawn_agent")
}

pub(crate) fn is_ask_tool(name: &str) -> bool {
    matches!(
        name,
        "AskUserQuestion" | "ask_user_question" | "request_user_input" | "ask_question"
    )
}

/// A copy of `input` as an object, for adding canonical fields beside the original ones.
pub(crate) fn object(input: &Value) -> Map<String, Value> {
    input.as_object().cloned().unwrap_or_default()
}

/// Canonical file-tool input: `file_path` from whichever key the harness uses, original fields kept.
pub(crate) fn with_file_path(input: &Value, keys: &[&str]) -> Value {
    let mut m = object(input);
    if let Some(p) = get_str(input, keys) {
        m.insert("file_path".into(), json!(p));
    }
    Value::Object(m)
}

pub(crate) fn with_cwd(mut input: Value, cwd: Option<&str>) -> Value {
    if let (Some(cwd), Some(m)) = (cwd, input.as_object_mut())
        && !m.contains_key("cwd")
    {
        m.insert("cwd".into(), json!(cwd));
    }
    input
}

pub(crate) fn other(name: &str, input: &Value) -> ToolCall {
    ToolCall::new(format!("Other:{name}"), input.clone())
}

/// Each harness reports the exit status in its own words.
pub fn exit_code_from(text: &str) -> Option<i32> {
    static PATTERNS: std::sync::OnceLock<Vec<regex::Regex>> = std::sync::OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r"(?m)^\s*Exit code:?\s*(-?\d+)",
            r"(?i)\bexited with code\s+(-?\d+)",
            r"(?i)\bexit status\s+(-?\d+)",
            r"(?i)\bexit code:?\s+(-?\d+)",
        ]
        .iter()
        .map(|p| regex::Regex::new(p).expect("valid pattern"))
        .collect()
    });
    patterns
        .iter()
        .find_map(|re| re.captures(text).and_then(|c| c[1].parse().ok()))
}

pub(crate) fn response_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Object(_) => {
            let mut pieces = vec![];
            let mut known = false;
            for key in ["result", "stdout", "stderr", "output", "content", "error"] {
                known |= v.get(key).is_some();
                match v.get(key) {
                    Some(Value::String(s)) if !s.is_empty() => pieces.push(s.clone()),
                    Some(Value::Array(items)) => {
                        for item in items {
                            if let Some(t) = item.get("text").and_then(Value::as_str) {
                                pieces.push(t.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            if pieces.is_empty() && !known {
                v.to_string()
            } else {
                pieces.join("\n")
            }
        }
        other => other.to_string(),
    }
}

pub(crate) fn clip(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n[output truncated]");
    }
    s
}

/// Outcome from a response value, reading an exit code where the harness reports one.
pub(crate) fn outcome_from(
    response: &Value,
    explicit_error: Option<&str>,
    bash: bool,
) -> ToolOutcome {
    let mut text = response_text(response);
    if let Some(err) = explicit_error.filter(|e| !e.trim().is_empty()) {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(err);
    }
    let interrupted = response.get("interrupted").and_then(Value::as_bool) == Some(true);
    let mut exit_code = exit_code_from(&text);
    let flagged_error = response
        .get("isError")
        .or_else(|| response.get("is_error"))
        .and_then(Value::as_bool)
        == Some(true)
        || explicit_error.is_some_and(|e| !e.trim().is_empty());
    if exit_code.is_none() && bash && !interrupted {
        exit_code = Some(if flagged_error { 1 } else { 0 });
    }
    let is_error = flagged_error || exit_code.is_some_and(|c| c != 0);
    ToolOutcome {
        output: clip(text, MAX_OUTCOME_CHARS),
        is_error,
        exit_code: if interrupted { None } else { exit_code },
        result_known: true,
    }
}

/// Claude-shaped PreToolUse output, used by Claude Code and Grok Build.
pub(crate) fn claude_shaped_pre(decision: &Decision, reason_fit: impl Fn(&str) -> String) -> Value {
    match decision {
        Decision::Allow { note } => {
            let mut hso = json!({"hookEventName": "PreToolUse", "permissionDecision": "allow"});
            if let Some(n) = note.as_ref().filter(|n| !n.is_empty()) {
                hso["additionalContext"] = json!(n);
            }
            json!({ "hookSpecificOutput": hso })
        }
        Decision::Deny { reason } => {
            let r = reason_fit(reason);
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": r,
                }
            })
        }
    }
}

pub(crate) fn post_context(notes: &[String]) -> Value {
    let notes: Vec<&str> = notes
        .iter()
        .map(String::as_str)
        .filter(|n| !n.trim().is_empty())
        .collect();
    if notes.is_empty() {
        return json!({});
    }
    json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": notes.join("\n\n")}})
}

pub(crate) fn stop_block(block: Option<&str>) -> Value {
    match block {
        Some(reason) => json!({"decision": "block", "reason": reason}),
        None => json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_names() {
        assert_eq!(
            ostra_mcp_tool("mcp__ostra__report").as_deref(),
            Some("Report")
        );
        assert_eq!(
            ostra_mcp_tool("ostra__memory_recall").as_deref(),
            Some("MemoryRecall")
        );
        assert_eq!(
            ostra_mcp_tool("mcp__ostra__code_callers").as_deref(),
            Some("CodeCallers")
        );
        assert_eq!(
            ostra_mcp_tool("mcp__ostra__code_implementations").as_deref(),
            Some("CodeImplementations")
        );
        assert_eq!(
            ostra_mcp_tool("ostra__code_unknown").as_deref(),
            Some("code_unknown")
        );
        assert_eq!(
            ostra_mcp_tool("mcp_ostra_submit_plan").as_deref(),
            Some("submit_plan")
        );
        assert_eq!(ostra_mcp_tool("mcp__other__report"), None);
        assert_eq!(ostra_mcp_tool("Read"), None);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code_from("Exit code 2\nboom"), Some(2));
        assert_eq!(exit_code_from("The command exited with code 1."), Some(1));
        assert_eq!(exit_code_from("exit status 127"), Some(127));
        assert_eq!(exit_code_from("Process exited with code 0"), Some(0));
        assert_eq!(exit_code_from("all fine"), None);
    }
}
