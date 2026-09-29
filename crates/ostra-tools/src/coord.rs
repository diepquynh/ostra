//! Subagent coordination tools (HANDOVER 10.8). The engine lives in the server; a [`Coordinate`]
//! handed in through [`crate::ToolEnvConfig`] runs one execution's coordination calls.

use crate::{ToolEnv, ToolOutput};
use ostra_core::coord::CoordReply;
use ostra_core::exec::ExecutionSpec;
use serde_json::Value;
use std::sync::Arc;

/// One execution's coordination calls, bound to its session and execution.
#[async_trait::async_trait]
pub trait Coordinate: Send + Sync {
    /// Run a coordination tool by native name. `Err` is a tool error shown to the model.
    async fn call(&self, tool: &str, input: &Value) -> Result<CoordReply, String>;
}

/// Opens an execution's coordination handle: `None` when its agent lacks the capability or it
/// runs outside a session.
pub trait CoordConnector: Send + Sync {
    fn open(&self, spec: &ExecutionSpec) -> Option<Arc<dyn Coordinate>>;
}

pub async fn run(env: &ToolEnv, tool: &str, input: &Value) -> ToolOutput {
    let Some(coord) = env.config().coord.clone() else {
        return ToolOutput::err(format!(
            "{tool} is not available to this run, because its agent does not coordinate with other subagents."
        ));
    };
    match coord.call(tool, input).await {
        Ok(reply) => ToolOutput {
            text: reply.text,
            end: reply.end,
            ..Default::default()
        },
        Err(e) => ToolOutput::err(e),
    }
}
