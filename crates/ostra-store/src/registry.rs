//! `~/.local/share/ostra/registry.db`: the machine's workspace list, push subscriptions, and
//! secrets such as the VAPID private key.

use crate::StoreError;
use crate::util::{now, parse_time};
use crate::workspace::open_connection;
use chrono::{DateTime, Utc};
use ostra_core::api::{PushKeys, PushSubscription};
use ostra_core::ids::WorkspaceId;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE workspaces (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  root TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL
);
CREATE TABLE push_subscriptions (
  endpoint TEXT PRIMARY KEY,
  p256dh TEXT NOT NULL,
  auth TEXT NOT NULL,
  workspace_id TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE kv (key TEXT PRIMARY KEY, value BLOB NOT NULL);
"#];

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceRecord {
    pub id: WorkspaceId,
    pub name: String,
    pub root: PathBuf,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredPushSubscription {
    pub subscription: PushSubscription,
    pub workspace: Option<WorkspaceId>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RegistryDb {
    conn: Arc<Mutex<Connection>>,
}

fn workspace_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, String, String)> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
}

fn to_record((id, name, root, created): (String, String, String, String)) -> Result<WorkspaceRecord, StoreError> {
    Ok(WorkspaceRecord { id: WorkspaceId(id), name, root: PathBuf::from(root), created_at: parse_time(&created)? })
}

impl RegistryDb {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Ok(RegistryDb { conn: Arc::new(Mutex::new(open_connection(path, MIGRATIONS)?)) })
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(MIGRATIONS[0])?;
        Ok(RegistryDb { conn: Arc::new(Mutex::new(conn)) })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // -- workspaces ---------------------------------------------------------------------------

    /// Register a workspace. Fails with `Invalid` when the root is already registered.
    pub fn add_workspace(&self, id: &WorkspaceId, name: &str, root: &Path) -> Result<WorkspaceRecord, StoreError> {
        let root_text = root.to_string_lossy().to_string();
        {
            let conn = self.lock();
            let taken: Option<String> = conn
                .query_row("SELECT id FROM workspaces WHERE root = ?1", params![root_text], |r| r.get(0))
                .optional()?;
            if let Some(other) = taken {
                return Err(StoreError::Invalid(format!("{root_text} is already registered as workspace {other}")));
            }
            conn.execute(
                "INSERT INTO workspaces (id, name, root, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![id.as_str(), name, root_text, now()],
            )?;
        }
        self.get_workspace(id)?.ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub fn rename_workspace(&self, id: &WorkspaceId, name: &str) -> Result<bool, StoreError> {
        Ok(self.lock().execute("UPDATE workspaces SET name = ?1 WHERE id = ?2", params![name, id.as_str()])? > 0)
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare("SELECT id, name, root, created_at FROM workspaces ORDER BY created_at, id")?;
        let rows = st.query_map([], workspace_row)?.collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(to_record).collect()
    }

    pub fn get_workspace(&self, id: &WorkspaceId) -> Result<Option<WorkspaceRecord>, StoreError> {
        let row = self
            .lock()
            .query_row("SELECT id, name, root, created_at FROM workspaces WHERE id = ?1", params![id.as_str()], workspace_row)
            .optional()?;
        row.map(to_record).transpose()
    }

    pub fn workspace_by_root(&self, root: &Path) -> Result<Option<WorkspaceRecord>, StoreError> {
        let row = self
            .lock()
            .query_row(
                "SELECT id, name, root, created_at FROM workspaces WHERE root = ?1",
                params![root.to_string_lossy()],
                workspace_row,
            )
            .optional()?;
        row.map(to_record).transpose()
    }

    /// Unregister a workspace. Deletes nothing on disk.
    pub fn remove_workspace(&self, id: &WorkspaceId) -> Result<bool, StoreError> {
        Ok(self.lock().execute("DELETE FROM workspaces WHERE id = ?1", params![id.as_str()])? > 0)
    }

    // -- push subscriptions -------------------------------------------------------------------

    /// Add or refresh a subscription, keyed by endpoint.
    pub fn add_push_subscription(&self, sub: &PushSubscription, workspace: Option<&WorkspaceId>) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO push_subscriptions (endpoint, p256dh, auth, workspace_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(endpoint) DO UPDATE SET p256dh = excluded.p256dh, auth = excluded.auth,
             workspace_id = excluded.workspace_id",
            params![sub.endpoint, sub.keys.p256dh, sub.keys.auth, workspace.map(|w| w.as_str().to_string()), now()],
        )?;
        Ok(())
    }

    pub fn list_push_subscriptions(&self) -> Result<Vec<StoredPushSubscription>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare(
            "SELECT endpoint, p256dh, auth, workspace_id, created_at FROM push_subscriptions ORDER BY created_at, endpoint",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(endpoint, p256dh, auth, ws, created)| {
                Ok(StoredPushSubscription {
                    subscription: PushSubscription { endpoint, keys: PushKeys { p256dh, auth } },
                    workspace: ws.map(WorkspaceId),
                    created_at: parse_time(&created)?,
                })
            })
            .collect()
    }

    pub fn remove_push_subscription(&self, endpoint: &str) -> Result<bool, StoreError> {
        Ok(self.lock().execute("DELETE FROM push_subscriptions WHERE endpoint = ?1", params![endpoint])? > 0)
    }

    // -- kv -----------------------------------------------------------------------------------

    pub fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.lock().query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| r.get(0)).optional()?)
    }

    pub fn kv_set(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn kv_delete(&self, key: &str) -> Result<bool, StoreError> {
        Ok(self.lock().execute("DELETE FROM kv WHERE key = ?1", params![key])? > 0)
    }

    /// Every entry whose key starts with `prefix`, in key order.
    pub fn kv_scan(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT key, value FROM kv WHERE substr(key, 1, length(?1)) = ?1 ORDER BY key")?;
        let rows = stmt.query_map(params![prefix], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // -- onboarding ---------------------------------------------------------------------------

    pub fn onboarded_at(&self) -> Result<Option<DateTime<Utc>>, StoreError> {
        match self.kv_get(ONBOARDED_AT)? {
            Some(bytes) => Ok(Some(parse_time(&String::from_utf8_lossy(&bytes))?)),
            None => Ok(None),
        }
    }

    /// Record that the first-run setup is done. A second call keeps the first time.
    pub fn mark_onboarded(&self) -> Result<DateTime<Utc>, StoreError> {
        self.lock().execute("INSERT OR IGNORE INTO kv (key, value) VALUES (?1, ?2)", params![ONBOARDED_AT, now().into_bytes()])?;
        self.onboarded_at()?.ok_or_else(|| StoreError::NotFound(ONBOARDED_AT.into()))
    }
}

const ONBOARDED_AT: &str = "onboarded_at";
