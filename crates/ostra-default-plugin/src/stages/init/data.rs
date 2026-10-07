//! The init flow's state.

#[allow(unused_imports)]
use crate::data::*;
use ostra_core::ids::{ExecutionId, GateId};
use serde_json::Value;
use std::collections::BTreeMap;

/// State of the init flow (HANDOVER 8.4).
#[derive(Debug, Clone, Default)]
pub struct InitTrack {
    pub project: String,
    pub detect: Option<ExecutionId>,
    pub detect_result: Option<Value>,
    pub scouts: Vec<InitItem>,
    pub propose: Option<ExecutionId>,
    pub propose_result: Option<Value>,
    pub approval_gate: Option<GateId>,
    pub decisions: Option<Vec<(String, String)>>,
    pub generates: Vec<InitItem>,
    pub inventory: Option<ExecutionId>,
    pub inventory_result: Option<Value>,
    pub adopt: Option<ExecutionId>,
    pub adopt_result: Option<Value>,
    pub failed: Option<(ExecutionId, String)>,
    pub failed_gate: Option<GateId>,
    pub retries: BTreeMap<String, u32>,
    /// Rule O4: a created project's init ended. `note` says why it ended without initializing.
    pub finished: bool,
    pub note: Option<String>,
    /// Rule O5: advice rounds per step (`{mode}:{item}`), and every guidance given, oldest first.
    pub advice: BTreeMap<String, Vec<String>>,
    /// The advisor run looking at the current failure.
    pub advising: Option<ExecutionId>,
    /// The advisor asked for the user, or could not help, so the failure goes to the user's gate.
    pub escalated: bool,
}

impl InitTrack {
    /// The spawn's `User focus:` for a project an agent created, from its `ProjectCreate` call.
    pub fn created_focus(p: &ostra_core::manage::CreatedProject) -> String {
        let reqs: Vec<String> = p.requirements.iter().map(|r| format!("- {r}")).collect();
        format!(
            "A new project `{}`, created in this session by the {}. Its folder is empty, so initialize it from the stack and requirements here.\n\nStack: {}\nPurpose: {}\nBase requirements:\n{}",
            p.key,
            p.agent,
            p.stack,
            p.purpose,
            reqs.join("\n")
        )
    }
}

#[derive(Debug, Clone)]
pub struct InitItem {
    pub key: String,
    pub exec: Option<ExecutionId>,
    pub result: Option<Value>,
}
