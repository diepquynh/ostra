//! The files the built-in stages write before a spawn, and the session facts every stage reads.

use crate::inputs::OstraInputs;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::stages::{book, closing, feedback};
use ostra_core::event::{ExecPurpose, FactTarget};
use ostra_engine::plan::SpawnRequest;
use ostra_engine::state::SessionState;
use serde_json::Value;

pub(crate) fn before_spawn(st: &SessionState, req: &SpawnRequest) -> Result<(), String> {
    let x = OstraInputs::of(&req.inputs);
    if x.context_files.contains(&st.session_context_path()) {
        feedback::hooks::write_session_context(st).map_err(|e| e.to_string())?;
    }
    // Rule D4a: rendered for each spawn, so every mark reflects the files as they are now.
    if x.wants_code_facts_file(&req.inputs) {
        let path = st.code_facts_path();
        std::fs::write(&path, ostra_core::doc::render_code_facts(&x.code_facts))
            .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    }
    Ok(())
}

pub(crate) fn spawn_files(st: &SessionState, req: &SpawnRequest) {
    closing::hooks::spawn_files(st, req);
    book::hooks::spawn_files(st, req);
}

pub(crate) fn session_facts(s: &SessionState) -> serde_json::Map<String, Value> {
    let os = s.ext.os();
    let mut m = serde_json::Map::new();
    m.insert("track".into(), serde_json::json!(os.track));
    m.insert(
        "stakes".into(),
        serde_json::json!(os.stakes.as_ref().map(|(_, s)| s)),
    );
    m
}

/// The spec and plan runs, and their fact-checks, serve the whole session.
pub(crate) fn session_wide(purpose: &ExecPurpose) -> bool {
    matches!(
        purpose,
        ExecPurpose::Spec { .. }
            | ExecPurpose::Plan { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Spec | FactTarget::Plan,
                ..
            }
    )
}
