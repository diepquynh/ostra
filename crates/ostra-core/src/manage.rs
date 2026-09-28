//! Management tools: calls an agent makes to Ostra itself rather than to the files it works on.
//! The first toolset manages projects: list the workspace's projects, and create a new one when
//! the request needs a codebase no project holds.

use crate::agent::AgentName;
use crate::ids::ExecutionId;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use ts_rs::TS;

pub const PROJECT_LIST: &str = "ProjectList";
pub const PROJECT_CREATE: &str = "ProjectCreate";

/// The project tools: the name on Ostra's MCP server and the native tool name.
pub const PROJECT_TOOLS: [(&str, &str); 2] = [
    ("project_list", PROJECT_LIST),
    ("project_create", PROJECT_CREATE),
];

pub fn is_manage_tool(native: &str) -> bool {
    PROJECT_TOOLS.iter().any(|(_, n)| *n == native)
}

/// Rule O1: management tools that change Ostra. Each one asks the user unless YOLO is on.
pub fn changes_ostra(native: &str) -> bool {
    native == PROJECT_CREATE
}

pub const MAX_PURPOSE_CHARS: usize = 800;
pub const MAX_REQUIREMENTS: usize = 20;
pub const MAX_REQUIREMENT_CHARS: usize = 400;

/// `ProjectCreate {key, stack, purpose, requirements, folder?, git_init?}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCreateInput {
    pub key: String,
    pub stack: String,
    pub purpose: String,
    pub requirements: Vec<String>,
    /// Relative to the workspace root. Absent means a folder named after the key.
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub git_init: Option<bool>,
}

impl ProjectCreateInput {
    pub fn parse(input: &serde_json::Value) -> Result<Self, String> {
        let mut req: ProjectCreateInput = serde_json::from_value(input.clone()).map_err(|e| {
            format!(
                "Call {PROJECT_CREATE} with `key`, `stack`, `purpose`, and `requirements`: {e}."
            )
        })?;
        req.key = req.key.trim().to_string();
        req.stack = req.stack.trim().to_string();
        req.purpose = req.purpose.trim().to_string();
        req.requirements = req
            .requirements
            .iter()
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .collect();
        req.folder = req
            .folder
            .map(|f| f.trim().trim_end_matches(['/', '\\']).to_string())
            .filter(|f| !f.is_empty());
        req.check()?;
        Ok(req)
    }

    /// Checks that need no workspace state, so a malformed call is refused before the user is
    /// asked. The key, stack, and folder rules are the Add project dialog's.
    fn check(&self) -> Result<(), String> {
        if !crate::slug::is_project_key(&self.key) {
            return Err(format!(
                "Use a project key of lowercase letters, digits, and dashes, starting with a letter or digit: `{}` is not one.",
                self.key
            ));
        }
        if !crate::slug::is_stack_name(&self.stack) {
            return Err(crate::slug::stack_issue(&self.stack));
        }
        if self.purpose.is_empty() || self.purpose.chars().count() > MAX_PURPOSE_CHARS {
            return Err(format!(
                "Give `purpose` in one to three sentences, at most {MAX_PURPOSE_CHARS} characters, because the user approves the project from it."
            ));
        }
        if self.requirements.is_empty() || self.requirements.len() > MAX_REQUIREMENTS {
            return Err(format!(
                "Give between 1 and {MAX_REQUIREMENTS} base requirements, because every later agent builds the project from them."
            ));
        }
        if self
            .requirements
            .iter()
            .any(|r| r.chars().count() > MAX_REQUIREMENT_CHARS)
        {
            return Err(format!(
                "Shorten each requirement to at most {MAX_REQUIREMENT_CHARS} characters: one fact per entry."
            ));
        }
        if let Some(f) = &self.folder {
            let relative = Path::new(f)
                .components()
                .all(|c| matches!(c, Component::Normal(_)));
            if !relative {
                return Err(format!(
                    "Give `folder` relative to the workspace root, with no `..`, such as `{}`: `{f}` is not.",
                    self.key
                ));
            }
        }
        Ok(())
    }

    /// The folder relative to the workspace root.
    pub fn folder(&self) -> &str {
        self.folder.as_deref().unwrap_or(&self.key)
    }

    pub fn git_init(&self) -> bool {
        self.git_init.unwrap_or(true)
    }

    /// What the permission card asks, short enough for a harness that clips ask reasons.
    pub fn ask_reason(&self) -> String {
        let text = format!(
            "Create project `{}` ({}) in `{}/` of the workspace: {}",
            self.key,
            self.stack,
            self.folder(),
            self.purpose
        );
        if text.chars().count() > 250 {
            let cut: String = text.chars().take(249).collect();
            format!("{cut}…")
        } else {
            text
        }
    }
}

/// A project an agent created during a session (Rule O3). The event carries every fact the fold
/// and later spawns need, because the fold cannot read workspace settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreatedProject {
    pub key: String,
    #[ts(type = "string")]
    pub path: PathBuf,
    pub stack: String,
    pub purpose: String,
    pub requirements: Vec<String>,
    pub execution: ExecutionId,
    pub agent: AgentName,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(v: serde_json::Value) -> Result<ProjectCreateInput, String> {
        ProjectCreateInput::parse(&v)
    }

    #[test]
    fn parse_trims_and_defaults() {
        let r = input(json!({
            "key": " notes-mcp ", "stack": "rust", "purpose": " An MCP server. ",
            "requirements": ["Rust 2024", "  ", "rmcp 3.5 over stdio"], "folder": "tools/notes-mcp/"
        }))
        .unwrap();
        assert_eq!(r.key, "notes-mcp");
        assert_eq!(r.requirements, ["Rust 2024", "rmcp 3.5 over stdio"]);
        assert_eq!(r.folder(), "tools/notes-mcp");
        assert!(r.git_init());
        let r = input(json!({"key": "a", "stack": "go", "purpose": "p", "requirements": ["x"], "git_init": false})).unwrap();
        assert_eq!((r.folder(), r.git_init()), ("a", false));
        assert!(
            r.ask_reason()
                .starts_with("Create project `a` (go) in `a/`")
        );
    }

    #[test]
    fn malformed_calls_name_the_fix() {
        let base = json!({"key": "a", "stack": "go", "purpose": "p", "requirements": ["x"]});
        let with = |k: &str, v: serde_json::Value| {
            let mut b = base.clone();
            b[k] = v;
            input(b).unwrap_err()
        };
        assert!(with("key", json!("Api")).starts_with("Use a project key"));
        assert!(with("stack", json!("")).contains("stack"));
        assert!(with("purpose", json!(" ")).starts_with("Give `purpose`"));
        assert!(with("requirements", json!([])).starts_with("Give between 1"));
        assert!(with("requirements", json!(["x".repeat(401)])).starts_with("Shorten"));
        for bad in ["../x", "/abs", "a/../b", "./a"] {
            assert!(
                with("folder", json!(bad)).starts_with("Give `folder`"),
                "{bad}"
            );
        }
        assert!(
            input(json!({"key": "a"}))
                .unwrap_err()
                .starts_with("Call ProjectCreate")
        );
        let long = with("purpose", json!("p".repeat(900)));
        assert!(long.starts_with("Give `purpose`"));
    }

    #[test]
    fn ask_reason_is_clipped() {
        let r = input(
            json!({"key": "a", "stack": "go", "purpose": "p".repeat(700), "requirements": ["x"]}),
        )
        .unwrap();
        assert_eq!(r.ask_reason().chars().count(), 250);
    }

    #[test]
    fn only_create_changes_ostra() {
        assert!(changes_ostra(PROJECT_CREATE));
        assert!(!changes_ostra(PROJECT_LIST));
        assert!(is_manage_tool(PROJECT_LIST) && !is_manage_tool("Read"));
    }
}
