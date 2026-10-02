use crate::StoreError;
use ostra_core::ids::WorkspaceId;
use rusqlite::{Connection, OptionalExtension, params};

const WORKSPACE_ID: &str = "workspace_id";

pub(crate) struct Meta<'c>(pub &'c Connection);

impl Meta<'_> {
    pub fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .0
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Empty until the workspace is created.
    pub fn workspace_id(&self) -> Result<WorkspaceId, StoreError> {
        Ok(WorkspaceId(self.get(WORKSPACE_ID)?.unwrap_or_default()))
    }

    pub fn set_workspace_id(&self, id: &WorkspaceId) -> Result<(), StoreError> {
        self.set(WORKSPACE_ID, id.as_str())
    }
}
