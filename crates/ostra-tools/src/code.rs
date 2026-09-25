//! The code navigation tools. The index and the dependency graph live in the server; a
//! [`CodeNav`] handed in through [`crate::ToolEnvConfig`] answers each call as text.

use crate::{ToolEnv, ToolOutput};
use serde_json::Value;
use std::path::Path;

/// Answers code navigation calls for the project that holds `repo_root`. `Err` is a tool error.
#[async_trait::async_trait]
pub trait CodeNav: Send + Sync {
    async fn call(&self, repo_root: &Path, tool: &str, input: &Value) -> Result<String, String>;
}

pub async fn run(env: &ToolEnv, tool: &str, input: &Value) -> ToolOutput {
    let Some(nav) = env.config().code.clone() else {
        return ToolOutput::err(
            "Code navigation is not available in this run. Use Grep, Glob, and Read instead.",
        );
    };
    match nav.call(&env.config().repo_root, tool, input).await {
        Ok(text) => ToolOutput::ok(text),
        Err(e) => ToolOutput::err(e),
    }
}
