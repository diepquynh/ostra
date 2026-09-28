//! Management tools. The workspace and the engine live in the server; a [`Manage`] handed in
//! through [`crate::ToolEnvConfig`] runs one execution's management calls.

use crate::{ToolEnv, ToolOutput};
use ostra_core::exec::ExecutionSpec;
use serde_json::Value;
use std::sync::Arc;

/// One execution's management calls, bound to its workspace, session, and execution.
#[async_trait::async_trait]
pub trait Manage: Send + Sync {
    /// Run a management tool by native name. `Err` is a tool error shown to the model.
    async fn call(&self, tool: &str, input: &Value) -> Result<String, String>;
}

/// Opens an execution's management handle: `None` when its agent has no management capability
/// or it runs outside a session.
pub trait ManageConnector: Send + Sync {
    fn open(&self, spec: &ExecutionSpec) -> Option<Arc<dyn Manage>>;
}

pub async fn run(env: &ToolEnv, tool: &str, input: &Value) -> ToolOutput {
    let Some(manage) = env.config().manage.clone() else {
        return ToolOutput::err(format!(
            "{tool} is not available to this run, because its agent does not manage projects."
        ));
    };
    match manage.call(tool, input).await {
        Ok(text) => ToolOutput::ok(text),
        Err(e) => ToolOutput::err(e),
    }
}
