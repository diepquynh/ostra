//! Subagent coordination (HANDOVER 10.8): agents ask each other questions and wake each other
//! with the answer, so a subagent continues from its own conversation instead of reading a
//! report cold. A subagent ID is the id of its conversation's first execution.

use crate::agent::AgentName;
use crate::ids::ExecutionId;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const SUBAGENT_LIST: &str = "SubagentList";
pub const SUBAGENT_ASK: &str = "SubagentAsk";
pub const SUBAGENT_REPLY: &str = "SubagentReply";

/// The coordination tools: the name on Ostra's MCP server and the native tool name.
pub const COORD_TOOLS: [(&str, &str); 3] = [
    ("subagent_list", SUBAGENT_LIST),
    ("subagent_ask", SUBAGENT_ASK),
    ("subagent_reply", SUBAGENT_REPLY),
];

pub fn is_coord_tool(native: &str) -> bool {
    COORD_TOOLS.iter().any(|(_, n)| *n == native)
}

/// Rule H4: helpers one run may start.
pub const MAX_HELPERS_PER_RUN: usize = 3;
/// Rule H4: asks one session may make.
pub const MAX_SESSION_ASKS: usize = 24;
/// Rule H6: runs one conversation may have before a pair loop starts fresh.
pub const MAX_CONVERSATION_RUNS: usize = 6;
pub const MAX_MESSAGE_CHARS: usize = 8000;
/// Agents `SubagentAsk` may start as helpers.
pub const HELPER_AGENTS: [AgentName; 1] = [AgentName::Explore];

/// Who a question goes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum AskTarget {
    /// A new helper of this agent, working in `project`.
    Agent { agent: AgentName, project: String },
    /// An existing subagent, by subagent ID.
    Subagent { id: ExecutionId },
}

/// What a delivered message is to the run that receives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DeliveryKind {
    Question,
    Answer,
}

/// `SubagentAsk {message, agent? | subagent_id?, project?}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AskInput {
    pub message: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub subagent_id: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
}

impl AskInput {
    pub fn parse(input: &serde_json::Value) -> Result<Self, String> {
        let mut a: AskInput = serde_json::from_value(input.clone()).map_err(|e| {
            format!("Call {SUBAGENT_ASK} with `message` and one of `agent` or `subagent_id`: {e}.")
        })?;
        a.message = a.message.trim().to_string();
        let clean = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        a.agent = clean(a.agent);
        a.subagent_id = clean(a.subagent_id);
        a.project = clean(a.project);
        check_message(&a.message)?;
        match (&a.agent, &a.subagent_id) {
            (Some(_), Some(_)) | (None, None) => Err(format!(
                "Give exactly one of `agent` (a new helper: {}) or `subagent_id` (an existing subagent from {SUBAGENT_LIST}).",
                helper_names()
            )),
            _ => Ok(a),
        }
    }

    /// The helper agent, when the ask starts one.
    pub fn helper(&self) -> Result<Option<AgentName>, String> {
        let Some(name) = &self.agent else {
            return Ok(None);
        };
        match name.parse::<AgentName>() {
            Ok(a) if HELPER_AGENTS.contains(&a) => Ok(Some(a)),
            _ => Err(format!(
                "Ask a helper of an agent Ostra can start for you: {}. `{name}` is not one; ask an existing subagent by `subagent_id` instead.",
                helper_names()
            )),
        }
    }
}

/// `SubagentReply {message}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReplyInput {
    pub message: String,
}

impl ReplyInput {
    pub fn parse(input: &serde_json::Value) -> Result<Self, String> {
        let mut r: ReplyInput = serde_json::from_value(input.clone())
            .map_err(|e| format!("Call {SUBAGENT_REPLY} with `message`: {e}."))?;
        r.message = r.message.trim().to_string();
        check_message(&r.message)?;
        Ok(r)
    }
}

fn check_message(m: &str) -> Result<(), String> {
    if m.is_empty() || m.chars().count() > MAX_MESSAGE_CHARS {
        return Err(format!(
            "Give `message` as plain text of at most {MAX_MESSAGE_CHARS} characters. Put longer material in a file and name its path."
        ));
    }
    Ok(())
}

fn helper_names() -> String {
    HELPER_AGENTS
        .iter()
        .map(|a| format!("`{a}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a coordination call does to the run that made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    /// The run goes on.
    Continue,
    /// Rule H2: the run waits for a message.
    Wait,
    /// A consult run answered and ends.
    Finish,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordReply {
    pub text: String,
    pub end: RunEnd,
}

/// The reply tool's name as the executor shows it.
pub fn reply_tool(executor: crate::executor::ExecutorKind) -> &'static str {
    use crate::executor::{ExecutorKind, HarnessKind};
    match executor {
        ExecutorKind::Native => SUBAGENT_REPLY,
        ExecutorKind::Harness(HarnessKind::Claude) => "mcp__ostra__subagent_reply",
        ExecutorKind::Harness(_) => "subagent_reply",
    }
}

/// Rule H3: what a run that owes an answer is told when it tries to end without one.
pub fn reply_instruction(tool: &str) -> String {
    format!(
        "Call `{tool}` now with your complete answer: another subagent asked you a question and waits for it, and it reads only that call, so an answer given in text is lost. This run submits nothing until you reply."
    )
}

/// The payload a run that ends through a coordination call records in place of a submit.
pub fn end_payload(tool: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "coordination": tool, "message": message })
}

/// The tool result a waiting run sees, and on a harness the instruction to end its turn.
pub fn waiting_text(who: &str, harness: bool) -> String {
    if harness {
        format!(
            "Sent. End your turn now and reply with only `Waiting`: Ostra types the answer from {who} into this session when it arrives, and you continue from there."
        )
    } else {
        format!(
            "Sent. This run waits now: Ostra wakes you with the answer from {who}, and you continue from there."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ask_needs_one_target_and_a_message() {
        assert!(AskInput::parse(&json!({"message": "hi"})).is_err());
        assert!(
            AskInput::parse(&json!({"message": "hi", "agent": "explore", "subagent_id": "x_1"}))
                .is_err()
        );
        assert!(AskInput::parse(&json!({"message": "  ", "agent": "explore"})).is_err());
        let a = AskInput::parse(&json!({"message": " find X ", "agent": "explore"})).unwrap();
        assert_eq!(a.message, "find X");
        assert_eq!(a.helper().unwrap(), Some(AgentName::Explore));
        let a = AskInput::parse(&json!({"message": "m", "agent": "implementer"})).unwrap();
        assert!(a.helper().is_err());
        let a = AskInput::parse(&json!({"message": "m", "subagent_id": "x_1"})).unwrap();
        assert_eq!(a.helper().unwrap(), None);
        assert!(ReplyInput::parse(&json!({"message": "x".repeat(MAX_MESSAGE_CHARS + 1)})).is_err());
        assert!(is_coord_tool(SUBAGENT_ASK) && !is_coord_tool("Read"));
    }
}
