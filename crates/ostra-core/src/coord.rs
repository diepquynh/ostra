//! Messaging between subagents (HANDOVER 10.8): any agent sends any other subagent of its session
//! a message, and may pause itself until a message arrives. Messages are always queued: Ostra hands
//! them over at the receiver's next turn boundary, never in the middle of a model request, so the
//! receiver's prompt cache holds. A subagent ID is the id of its conversation's first execution.

use crate::agent::AgentName;
use crate::ids::ExecutionId;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const LIST_AGENTS: &str = "ListAgents";
pub const SEND_MESSAGE: &str = "SendMessage";
pub const WAIT_FOR_MESSAGE: &str = "WaitForMessage";

/// The messaging tools: the name on Ostra's MCP server and the native tool name.
pub const COORD_TOOLS: [(&str, &str); 3] = [
    ("list_agents", LIST_AGENTS),
    ("send_message", SEND_MESSAGE),
    ("wait_for_message", WAIT_FOR_MESSAGE),
];

pub fn is_coord_tool(native: &str) -> bool {
    COORD_TOOLS.iter().any(|(_, n)| *n == native)
}

/// Rule SM5: helpers one run may start.
pub const MAX_HELPERS_PER_RUN: usize = 3;
/// Rule SM5: messages the agents of one session may send.
pub const MAX_SESSION_MESSAGES: usize = 48;
/// Rule H6: runs one conversation may have before a pair loop starts fresh, and before a message
/// continues it (Rule SM4).
pub const MAX_CONVERSATION_RUNS: usize = 6;
pub const MAX_MESSAGE_CHARS: usize = 8000;

/// Who a message goes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum MessageTarget {
    /// A new helper of this agent, working in `project`.
    Agent {
        agent: AgentName,
        project: String,
        /// Rule SM7: the helper's contract. A `research` helper runs as a research task whose
        /// document joins the session's research. Absent in logs from before contracts, whose
        /// helpers were all research helpers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        contract: Option<crate::contract::Contract>,
    },
    /// An existing subagent, by subagent ID.
    Subagent { id: ExecutionId },
}

/// Logs written before messaging name the target of a question this way.
pub type AskTarget = MessageTarget;

/// What a message is, which decides what happens when its receiver has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MessageKind {
    /// Sent by an agent with `SendMessage`. It continues a receiver that ended (Rule SM4).
    #[default]
    Sent,
    /// A helper's result, which Ostra sends to the run that started it. Dropped when that
    /// subagent ended, because nothing waits for it.
    Result,
}

/// Logs written before messaging say what a delivered message was to its run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DeliveryKind {
    Question,
    Answer,
}

/// `SendMessage {message, to? | agent?, project?, wait?}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SendInput {
    pub message: String,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub wait: bool,
}

impl SendInput {
    pub fn parse(input: &serde_json::Value) -> Result<Self, String> {
        let mut a: SendInput = serde_json::from_value(input.clone()).map_err(|e| {
            format!("Call {SEND_MESSAGE} with `message` and one of `to` or `agent`: {e}.")
        })?;
        a.message = a.message.trim().to_string();
        let clean = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        a.to = clean(a.to);
        a.agent = clean(a.agent);
        a.project = clean(a.project);
        check_message(&a.message)?;
        match (&a.to, &a.agent) {
            (Some(_), Some(_)) | (None, None) => Err(format!(
                "Give exactly one of `to` (a subagent ID from {LIST_AGENTS}) or `agent` (a new helper)."
            )),
            _ => Ok(a),
        }
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

/// What a messaging call does to the run that made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    /// The run goes on.
    Continue,
    /// Rule SM3: the run pauses until a message arrives.
    Wait,
    /// The run ends.
    Finish,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordReply {
    pub text: String,
    pub end: RunEnd,
}

/// A messaging tool's name as the executor shows it.
pub fn tool_on(native: &str, executor: crate::executor::ExecutorKind) -> String {
    use crate::executor::{ExecutorKind, HarnessKind};
    let mcp = COORD_TOOLS
        .iter()
        .find(|(_, n)| *n == native)
        .map(|(m, _)| *m)
        .unwrap_or(native);
    match executor {
        ExecutorKind::Native => native.to_string(),
        ExecutorKind::Harness(HarnessKind::Claude) => format!("mcp__ostra__{mcp}"),
        ExecutorKind::Harness(_) => mcp.to_string(),
    }
}

/// The send tool's name as the executor shows it.
pub fn send_tool(executor: crate::executor::ExecutorKind) -> String {
    tool_on(SEND_MESSAGE, executor)
}

/// Rule SM6: what a run that owes a reply is told when it tries to end without one.
pub fn reply_instruction(tool: &str, to: &[String]) -> String {
    format!(
        "Call `{tool}` with `to` set to {} now, with your complete reply: that subagent sent you a message and waits for your answer, and it reads only that call, so a reply given in text is lost. This run submits nothing until you reply.",
        to.iter()
            .map(|t| format!("`{t}`"))
            .collect::<Vec<_>>()
            .join(" and ")
    )
}

/// The payload a run that pauses records in place of a submit.
pub fn end_payload(tool: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "coordination": tool, "message": message })
}

/// The tool result of a run that pauses, and on a harness the instruction to end its turn.
pub fn waiting_text(harness: bool) -> String {
    if harness {
        "End your turn now and reply with only `Waiting`: Ostra types the next message for you into this session when it arrives, and you continue from there.".into()
    } else {
        "This run waits now: Ostra wakes you with the next message for you, and you continue from there.".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{ExecutorKind, HarnessKind};
    use serde_json::json;

    #[test]
    fn send_needs_one_target_and_a_message() {
        assert!(SendInput::parse(&json!({"message": "hi"})).is_err());
        assert!(
            SendInput::parse(&json!({"message": "hi", "agent": "explore", "to": "x_1"})).is_err()
        );
        assert!(SendInput::parse(&json!({"message": "  ", "agent": "explore"})).is_err());
        let a = SendInput::parse(&json!({"message": " find X ", "agent": "explore"})).unwrap();
        assert_eq!((a.message.as_str(), a.wait), ("find X", false));
        let a = SendInput::parse(&json!({"message": "m", "to": "x_1", "wait": true})).unwrap();
        assert!(a.wait);
        assert!(
            SendInput::parse(&json!({"message": "x".repeat(MAX_MESSAGE_CHARS + 1), "to": "a"}))
                .is_err()
        );
        assert!(is_coord_tool(SEND_MESSAGE) && !is_coord_tool("Read"));
    }

    #[test]
    fn tool_names_follow_the_executor() {
        assert_eq!(send_tool(ExecutorKind::Native), "SendMessage");
        assert_eq!(
            send_tool(ExecutorKind::Harness(HarnessKind::Claude)),
            "mcp__ostra__send_message"
        );
        assert_eq!(
            tool_on(WAIT_FOR_MESSAGE, ExecutorKind::Harness(HarnessKind::Codex)),
            "wait_for_message"
        );
    }

    #[test]
    fn old_targets_still_parse() {
        let t: AskTarget =
            serde_json::from_value(json!({"kind": "subagent", "id": "x_1"})).unwrap();
        assert!(matches!(t, MessageTarget::Subagent { .. }));
    }
}
