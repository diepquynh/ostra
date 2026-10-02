use crate::StoreError;
use crate::sqlite::{RowExt, query_all, query_one};
use crate::util::enum_str;
use ostra_core::api::InitStatus;
use rusqlite::{Connection, Row, params};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRow {
    pub key: String,
    pub path: PathBuf,
    pub init_status: InitStatus,
    pub stack: Option<String>,
}

const COLS: &str = "key, path, init_status, stack";

pub(crate) struct Projects<'c>(pub &'c Connection);

impl Projects<'_> {
    pub fn upsert(&self, row: &ProjectRow) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO projects (key, path, init_status, stack) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET path = excluded.path, init_status = excluded.init_status,
             stack = excluded.stack",
            params![
                row.key,
                row.path.to_string_lossy(),
                enum_str(&row.init_status)?,
                row.stack
            ],
        )?;
        Ok(())
    }

    pub fn set_init_status(&self, key: &str, status: InitStatus) -> Result<bool, StoreError> {
        let n = self.0.execute(
            "UPDATE projects SET init_status = ?1 WHERE key = ?2",
            params![enum_str(&status)?, key],
        )?;
        Ok(n > 0)
    }

    pub fn delete(&self, key: &str) -> Result<bool, StoreError> {
        Ok(self
            .0
            .execute("DELETE FROM projects WHERE key = ?1", [key])?
            > 0)
    }

    pub fn get(&self, key: &str) -> Result<Option<ProjectRow>, StoreError> {
        query_one(
            self.0,
            &format!("SELECT {COLS} FROM projects WHERE key = ?1"),
            [key],
            project,
        )
    }

    pub fn list(&self) -> Result<Vec<ProjectRow>, StoreError> {
        query_all(
            self.0,
            &format!("SELECT {COLS} FROM projects ORDER BY key"),
            [],
            project,
        )
    }
}

fn project(r: &Row<'_>) -> Result<ProjectRow, StoreError> {
    Ok(ProjectRow {
        key: r.get("key")?,
        path: PathBuf::from(r.get::<_, String>("path")?),
        init_status: r.variant("init_status")?,
        stack: r.get("stack")?,
    })
}
