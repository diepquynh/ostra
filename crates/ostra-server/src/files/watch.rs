//! Which files an execution wrote. Native executions are seen in their engine notices (a write
//! tool call, then its successful result); harness executions report from the hook bridge's
//! post-tool observer. Both produce a [`Touch`].

use ostra_core::exec::{ExecutionDelta, ExecutionStatus};
use ostra_core::ids::{ExecutionId, WorkspaceId};
use ostra_core::policy::ToolCall;
use ostra_engine::EngineNotice;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;

/// Files one successful write tool call changed. `paths` may be relative to `base`, or to the
/// execution's project root when `base` is `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Touch {
    pub workspace: Option<WorkspaceId>,
    pub execution: ExecutionId,
    pub paths: Vec<PathBuf>,
    pub base: Option<PathBuf>,
}

/// Target paths of a canonical write tool call, as the model wrote them.
pub fn write_targets(call: &ToolCall) -> Vec<PathBuf> {
    match call.tool.as_str() {
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            ["file_path", "notebook_path", "path"].iter().find_map(|k| call.str_field(k)).map(PathBuf::from).into_iter().collect()
        }
        "ApplyPatch" => {
            let patch = call.str_field("patch").or_else(|| call.str_field("input")).unwrap_or_default();
            ostra_policy::apply_patch_paths(patch).into_iter().map(PathBuf::from).collect()
        }
        _ => vec![],
    }
}

/// The working folder a harness recorded on the call (Codex patches carry `cwd`).
pub fn call_cwd(call: &ToolCall) -> Option<PathBuf> {
    call.str_field("cwd").map(PathBuf::from)
}

/// Bounds the pending map when results never arrive (hook-bridge calls log no result).
const MAX_PENDING: usize = 4096;

/// Pairs native write tool calls with their results.
/// A write call waiting for its result: target paths and the call's working folder.
type PendingWrite = (Vec<PathBuf>, Option<PathBuf>);

#[derive(Default)]
pub struct ToolWatch {
    pending: Mutex<HashMap<(ExecutionId, String), PendingWrite>>,
}

impl ToolWatch {
    pub fn observe(&self, workspace: &WorkspaceId, notice: &EngineNotice) -> Option<Touch> {
        match notice {
            EngineNotice::Delta { execution, item } => match &item.delta {
                ExecutionDelta::ToolCall { call_id, call } => {
                    let targets = write_targets(call);
                    if !targets.is_empty() {
                        let mut pending = self.pending.lock();
                        if pending.len() >= MAX_PENDING {
                            pending.clear();
                        }
                        pending.insert((execution.clone(), call_id.clone()), (targets, call_cwd(call)));
                    }
                    None
                }
                ExecutionDelta::ToolResult { call_id, is_error, .. } => {
                    let (paths, base) = self.pending.lock().remove(&(execution.clone(), call_id.clone()))?;
                    (!is_error).then(|| Touch { workspace: Some(workspace.clone()), execution: execution.clone(), paths, base })
                }
                _ => None,
            },
            EngineNotice::ExecutionStatus { execution, status } if *status != ExecutionStatus::Running => {
                self.pending.lock().retain(|(e, _), _| e != execution);
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostra_core::api::ActivityItem;
    use serde_json::json;

    fn delta(execution: &ExecutionId, delta: ExecutionDelta) -> EngineNotice {
        EngineNotice::Delta { execution: execution.clone(), item: ActivityItem { seq: 1, at: chrono::Utc::now(), delta } }
    }

    #[test]
    fn a_successful_write_result_becomes_a_touch() {
        let w = ToolWatch::default();
        let ws = WorkspaceId::from("ws_1");
        let x = ExecutionId::from("x_1");
        let call = |id: &str, tool: &str, input: serde_json::Value| delta(&x, ExecutionDelta::ToolCall { call_id: id.into(), call: ToolCall::new(tool, input) });
        let result = |id: &str, is_error: bool| delta(&x, ExecutionDelta::ToolResult { call_id: id.into(), output: String::new(), is_error, duration_ms: 1 });

        assert_eq!(w.observe(&ws, &call("1", "Write", json!({"file_path": "src/a.rs", "content": ""}))), None);
        let t = w.observe(&ws, &result("1", false)).unwrap();
        assert_eq!(t.paths, vec![PathBuf::from("src/a.rs")]);
        assert_eq!(t.workspace, Some(ws.clone()));
        assert_eq!(w.observe(&ws, &result("1", false)), None, "a result pairs once");

        w.observe(&ws, &call("2", "Edit", json!({"file_path": "/p/b.rs"})));
        assert_eq!(w.observe(&ws, &result("2", true)), None, "a failed edit changed nothing");

        w.observe(&ws, &call("3", "Read", json!({"file_path": "/p/b.rs"})));
        assert_eq!(w.observe(&ws, &result("3", false)), None);

        w.observe(&ws, &call("4", "ApplyPatch", json!({"patch": "*** Begin Patch\n*** Add File: n.rs\n+x\n*** End Patch", "cwd": "/p"})));
        let t = w.observe(&ws, &result("4", false)).unwrap();
        assert_eq!((t.paths, t.base), (vec![PathBuf::from("n.rs")], Some(PathBuf::from("/p"))));

        w.observe(&ws, &call("5", "Write", json!({"file_path": "c.rs"})));
        w.observe(&ws, &EngineNotice::ExecutionStatus { execution: x.clone(), status: ExecutionStatus::Ok });
        assert_eq!(w.observe(&ws, &result("5", false)), None, "an ended execution drops its pending calls");
    }

    #[test]
    fn notebook_targets() {
        assert_eq!(write_targets(&ToolCall::new("NotebookEdit", json!({"notebook_path": "/r/n.ipynb"}))), vec![PathBuf::from("/r/n.ipynb")]);
        assert!(write_targets(&ToolCall::new("Bash", json!({"command": "touch x"}))).is_empty());
    }
}
