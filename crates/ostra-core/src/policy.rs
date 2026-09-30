//! Tool calls and policy decisions, in canonical (Claude Code) tool names and input shapes.
//! Harness adapters translate their payloads into these before the policy engine sees them.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A tool call in canonical form. `tool` is a native tool name (`Read`, `Write`, `Edit`, `Bash`,
/// `Grep`, `Glob`, `Skill`, `WebSearch`, `WebFetch`, `Report`, `Memory`, `MemoryRecall`, `DocsSearch`,
/// `ProjectList`, `ProjectCreate`, `submit_<agent>`), or `ApplyPatch` for Codex patches, or
/// `Other:<name>` for a harness tool with no canonical equivalent.
///
/// Canonical input shapes:
/// - `Read {file_path, offset?, limit?}`
/// - `Write {file_path, content}`
/// - `Edit {file_path, old_string, new_string, replace_all?}`
/// - `Bash {command, timeout?, description?}`
/// - `Grep {pattern, path?, glob?, type?, output_mode?, "-i"?, "-n"?, "-A"?, "-B"?, "-C"?, head_limit?, multiline?}`
/// - `Glob {pattern, path?}`
/// - `Skill {name}` or `Skill {path}`
/// - `WebSearch {query}`, `WebFetch {url, prompt?}`
/// - `Report {content, reason?}`
/// - `Memory {area, lesson, source}`, `MemoryRecall {query, area?, limit?}`
/// - `DocsSearch {query, project?, limit?}`
/// - `ProjectList {}`, `ProjectCreate {key, stack, purpose, requirements, folder?, git_init?}`
/// - `ApplyPatch {patch}` (Codex `*** Begin Patch` format)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ToolCall {
    pub tool: String,
    #[ts(type = "unknown")]
    pub input: serde_json::Value,
}

impl ToolCall {
    pub fn new(tool: impl Into<String>, input: serde_json::Value) -> Self {
        ToolCall {
            tool: tool.into(),
            input,
        }
    }

    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.input.get(key).and_then(|v| v.as_str())
    }
}

/// The outcome of a finished tool call, as the policy's post-tool observer sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ToolOutcome {
    /// Text returned to the model. Harnesses that do not report results leave it empty.
    pub output: String,
    pub is_error: bool,
    /// Shell exit code, when the tool was `Bash`.
    pub exit_code: Option<i32>,
    /// False when the harness never reports tool results (Antigravity PostToolUse).
    pub result_known: bool,
}

/// Which layer and rule produced a decision. Shown in the Activity view with every denial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RuleRef {
    /// `guard` or `permission`.
    pub layer: String,
    /// Guard id (`write-scope`, `no-tests-from-implementer`, `state-ownership`,
    /// `artifact-ownership`, `report-path`, `lesson-gate`, `build-streak`, `self-protection`) or
    /// the permission rule text (`Bash(git push *)`) or mode (`mode:plan`).
    pub rule: String,
}

impl RuleRef {
    pub fn guard(rule: &str) -> Self {
        RuleRef {
            layer: "guard".into(),
            rule: rule.into(),
        }
    }
    pub fn permission(rule: &str) -> Self {
        RuleRef {
            layer: "permission".into(),
            rule: rule.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "decision", rename_all = "lowercase")]
#[ts(export)]
pub enum PolicyDecision {
    Allow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<RuleRef>,
    },
    /// Needs a user answer (or YOLO). `reason` says what is being asked.
    Ask { reason: String, rule: RuleRef },
    /// Refused. `reason` leads with the correction, because Grok clips reasons to 256 characters.
    Deny { reason: String, rule: RuleRef },
}

impl PolicyDecision {
    pub fn allow() -> Self {
        PolicyDecision::Allow { rule: None }
    }
    pub fn deny(rule: RuleRef, reason: impl Into<String>) -> Self {
        PolicyDecision::Deny {
            reason: reason.into(),
            rule,
        }
    }
    pub fn ask(rule: RuleRef, reason: impl Into<String>) -> Self {
        PolicyDecision::Ask {
            reason: reason.into(),
            rule,
        }
    }
    pub fn is_allow(&self) -> bool {
        matches!(self, PolicyDecision::Allow { .. })
    }
    pub fn is_deny(&self) -> bool {
        matches!(self, PolicyDecision::Deny { .. })
    }
}

/// A permission-ask answer from the browser card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum PermissionAnswer {
    AllowOnce,
    /// Allow and add the suggested rule to the workspace allow list.
    AlwaysInWorkspace,
    Deny,
}

impl PermissionAnswer {
    pub fn allows(self) -> bool {
        !matches!(self, PermissionAnswer::Deny)
    }
}
