use crate::StoreError;
use crate::sqlite::{RowExt, query_all, query_one};
use crate::util::now;
use chrono::{DateTime, Utc};
use ostra_core::ids::WorkspaceId;
use rusqlite::{Connection, Row, params};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceRecord {
    pub id: WorkspaceId,
    pub name: String,
    pub root: PathBuf,
    pub created_at: DateTime<Utc>,
}

const COLS: &str = "id, name, root, created_at";

pub(crate) struct Workspaces<'c>(pub &'c Connection);

impl Workspaces<'_> {
    pub fn insert(&self, id: &WorkspaceId, name: &str, root: &Path) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO workspaces (id, name, root, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id.as_str(), name, root.to_string_lossy(), now()],
        )?;
        Ok(())
    }

    pub fn rename(&self, id: &WorkspaceId, name: &str) -> Result<bool, StoreError> {
        Ok(self.0.execute(
            "UPDATE workspaces SET name = ?1 WHERE id = ?2",
            params![name, id.as_str()],
        )? > 0)
    }

    pub fn delete(&self, id: &WorkspaceId) -> Result<bool, StoreError> {
        Ok(self
            .0
            .execute("DELETE FROM workspaces WHERE id = ?1", [id.as_str()])?
            > 0)
    }

    /// Oldest first.
    pub fn list(&self) -> Result<Vec<WorkspaceRecord>, StoreError> {
        query_all(
            self.0,
            &format!("SELECT {COLS} FROM workspaces ORDER BY created_at, id"),
            [],
            workspace,
        )
    }

    pub fn get(&self, id: &WorkspaceId) -> Result<Option<WorkspaceRecord>, StoreError> {
        query_one(
            self.0,
            &format!("SELECT {COLS} FROM workspaces WHERE id = ?1"),
            [id.as_str()],
            workspace,
        )
    }

    pub fn by_root(&self, root: &Path) -> Result<Option<WorkspaceRecord>, StoreError> {
        query_one(
            self.0,
            &format!("SELECT {COLS} FROM workspaces WHERE root = ?1"),
            [root.to_string_lossy()],
            workspace,
        )
    }
}

fn workspace(r: &Row<'_>) -> Result<WorkspaceRecord, StoreError> {
    Ok(WorkspaceRecord {
        id: WorkspaceId(r.get("id")?),
        name: r.get("name")?,
        root: PathBuf::from(r.get::<_, String>("root")?),
        created_at: r.time("created_at")?,
    })
}
