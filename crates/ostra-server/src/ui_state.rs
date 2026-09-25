//! The console layout per workspace (`/api/workspaces/:ws/ui`), stored as JSON in the workspace db.

use crate::workspace::WorkspaceRt;
use ostra_core::api::{UI_STATE_MAX_BYTES, WorkspaceUiState};
use serde_json::{Map, Value};

const META_KEY: &str = "ui_state";

#[derive(Debug, PartialEq)]
pub enum UiStateError {
    TooLarge,
    Invalid(String),
    Store(String),
}

impl UiStateError {
    pub fn message(&self) -> String {
        match self {
            UiStateError::TooLarge => format!(
                "Keep the UI state under {} KiB of JSON.",
                UI_STATE_MAX_BYTES / 1024
            ),
            UiStateError::Invalid(m) | UiStateError::Store(m) => m.clone(),
        }
    }
}

/// Merge the top-level fields of a PATCH body onto the current state.
pub fn merge(current: &WorkspaceUiState, patch: &[u8]) -> Result<WorkspaceUiState, UiStateError> {
    if patch.len() > UI_STATE_MAX_BYTES {
        return Err(UiStateError::TooLarge);
    }
    let patch: Value = serde_json::from_slice(patch)
        .map_err(|e| UiStateError::Invalid(format!("Send a JSON object: {e}")))?;
    let Value::Object(patch) = patch else {
        return Err(UiStateError::Invalid("Send a JSON object.".into()));
    };
    let mut merged = match serde_json::to_value(current) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };
    merged.extend(patch);
    let mut state: WorkspaceUiState = serde_json::from_value(Value::Object(merged))
        .map_err(|e| UiStateError::Invalid(format!("The UI state has a wrong field: {e}")))?;
    for tab in state.tabs.iter_mut().filter(|t| t.pinned) {
        tab.preview = false;
    }
    state.tabs.sort_by_key(|t| !t.pinned);
    if serde_json::to_vec(&state).map_or(0, |v| v.len()) > UI_STATE_MAX_BYTES {
        return Err(UiStateError::TooLarge);
    }
    Ok(state)
}

impl WorkspaceRt {
    /// The stored layout, or the default when none is stored or it no longer parses.
    pub fn ui_state(&self) -> WorkspaceUiState {
        self.db
            .meta_get(META_KEY)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn patch_ui_state(&self, patch: &[u8]) -> Result<WorkspaceUiState, UiStateError> {
        let state = merge(&self.ui_state(), patch)?;
        let text = serde_json::to_string(&state).map_err(|e| UiStateError::Store(e.to_string()))?;
        self.db
            .meta_set(META_KEY, &text)
            .map_err(|e| UiStateError::Store(e.to_string()))?;
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostra_core::api::UiTab;

    #[test]
    fn patch_merges_top_level_fields() {
        let first = merge(
            &WorkspaceUiState::default(),
            br#"{"tabs": [{"id": "ws:overview", "pinned": true}], "active": "ws:overview"}"#,
        )
        .unwrap();
        assert_eq!(
            first.tabs,
            vec![UiTab {
                id: "ws:overview".into(),
                pinned: true,
                preview: false
            }]
        );
        let second = merge(
            &first,
            br#"{"theme": "dark", "left_tab": "files", "unknown": 1}"#,
        )
        .unwrap();
        assert_eq!(second.active.as_deref(), Some("ws:overview"));
        assert_eq!(second.tabs.len(), 1);
        assert_eq!(second.theme.as_deref(), Some("dark"));
        let cleared = merge(&second, br#"{"active": null}"#).unwrap();
        assert_eq!(cleared.active, None);
    }

    #[test]
    fn pinned_tabs_come_first_and_are_never_the_preview() {
        let s = merge(
            &WorkspaceUiState::default(),
            br#"{"tabs": [{"id": "a"}, {"id": "b", "pinned": true, "preview": true}, {"id": "c", "preview": true}, {"id": "d", "pinned": true}]}"#,
        )
        .unwrap();
        let tabs: Vec<_> = s
            .tabs
            .iter()
            .map(|t| (t.id.as_str(), t.pinned, t.preview))
            .collect();
        assert_eq!(
            tabs,
            vec![
                ("b", true, false),
                ("d", true, false),
                ("a", false, false),
                ("c", false, true)
            ]
        );
    }

    #[test]
    fn patch_must_be_a_typed_object() {
        let s = WorkspaceUiState::default();
        assert!(matches!(merge(&s, b"[1]"), Err(UiStateError::Invalid(_))));
        assert!(matches!(
            merge(&s, b"not json"),
            Err(UiStateError::Invalid(_))
        ));
        assert!(matches!(
            merge(&s, br#"{"tabs": "x"}"#),
            Err(UiStateError::Invalid(_))
        ));
    }

    #[test]
    fn state_size_is_capped() {
        let s = WorkspaceUiState::default();
        let big = format!(r#"{{"active": "{}"}}"#, "a".repeat(UI_STATE_MAX_BYTES));
        assert_eq!(merge(&s, big.as_bytes()), Err(UiStateError::TooLarge));
        let tab = format!(r#"{{"id": "{}"}}"#, "t".repeat(1000));
        let tabs = format!(r#"{{"tabs": [{}]}}"#, vec![tab; 60].join(","));
        let near = merge(&s, tabs.as_bytes()).unwrap();
        assert!(merge(&near, tabs.as_bytes()).is_ok());
        let more = format!(r#"{{"active": "{}"}}"#, "a".repeat(10_000));
        assert_eq!(merge(&near, more.as_bytes()), Err(UiStateError::TooLarge));
    }
}
