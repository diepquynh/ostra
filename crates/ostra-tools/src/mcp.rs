//! Tools of the workspace's external MCP servers. The connections live in the server; an
//! [`McpTools`] handed in through [`crate::ToolEnvConfig`] lists and runs one execution's tools.

use crate::{ToolDefinition, ToolEnv, ToolOutput};
use ostra_core::exec::ExecutionSpec;
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// One execution's MCP tools, fixed when the execution starts.
#[async_trait::async_trait]
pub trait McpTools: Send + Sync {
    /// Definitions under canonical names (`mcp__<server>__<tool>`).
    fn definitions(&self) -> Vec<ToolDefinition>;

    /// Canonical names of the tools their server marks read-only (`readOnlyHint`).
    fn read_only(&self) -> Vec<String>;

    /// Run a tool by canonical name. `Err` is a tool error.
    async fn call(
        &self,
        tool: &str,
        input: &Value,
        cancel: CancellationToken,
    ) -> Result<String, String>;
}

/// What opening an execution's MCP tools produced.
pub struct McpOpened {
    pub tools: Option<Arc<dyn McpTools>>,
    /// One line per server that could not be reached, for the Activity view.
    pub notes: Vec<String>,
}

/// Connects an execution to its workspace's MCP servers.
#[async_trait::async_trait]
pub trait McpConnector: Send + Sync {
    async fn open(&self, spec: &ExecutionSpec) -> McpOpened;
}

pub async fn run(
    env: &ToolEnv,
    tool: &str,
    input: &Value,
    cancel: CancellationToken,
) -> ToolOutput {
    let Some(mcp) = env.config().mcp.clone() else {
        return ToolOutput::err(format!("Unknown tool `{tool}`."));
    };
    match mcp.call(tool, input, cancel).await {
        Ok(text) => ToolOutput::ok(text),
        Err(e) => ToolOutput::err(e),
    }
}
