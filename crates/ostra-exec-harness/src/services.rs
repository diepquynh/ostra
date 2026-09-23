use ostra_core::ExecutionId;
use ostra_core::policy::{PermissionAnswer, PolicyDecision, RuleRef, ToolCall, ToolOutcome};

/// What the harness bridge needs from the rest of Ostra. The server implements it with the
/// policy engine, the tools crate, and the engine's permission gates.
#[async_trait::async_trait]
pub trait BridgeServices: Send + Sync {
    /// Extra authorization on top of the per-execution token the bridge already checks.
    fn authorize(&self, execution: &ExecutionId, token: &str) -> bool;

    /// Layer 1 guards then layer 2 permissions for one canonical tool call.
    fn policy_check(&self, execution: &ExecutionId, call: &ToolCall) -> PolicyDecision;

    /// Resolve an `Ask` decision: a browser card, or YOLO. Waits for the answer.
    async fn resolve_ask(
        &self,
        execution: &ExecutionId,
        call: &ToolCall,
        reason: &str,
        rule: &RuleRef,
    ) -> PermissionAnswer;

    /// Post-tool observation (build streak, lesson gate). Returns notes for the model.
    fn policy_observe(
        &self,
        execution: &ExecutionId,
        call: &ToolCall,
        outcome: &ToolOutcome,
    ) -> Vec<String>;

    /// Run an Ostra MCP tool other than `submit_*` (`report`, `memory`, `memory_recall`).
    /// `Ok` is the text result, `Err` a tool error shown to the model.
    async fn mcp_call(
        &self,
        execution: &ExecutionId,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<String, String>;

    /// `(name, description, input schema)` of the non-submit Ostra tools this execution may use.
    fn mcp_tools(&self, execution: &ExecutionId) -> Vec<(String, String, serde_json::Value)>;

    /// The harness reported a stop (turn end). Informational: the bridge decides blocking.
    fn stop_event(&self, execution: &ExecutionId, payload: serde_json::Value);
}
