//! `~/.local/share/ostra/registry.db`: the machine's workspace list, push subscriptions, and
//! secrets such as the VAPID private key.
//!
//! Each table has a repository in its own module that holds its SQL. `RegistryDb` owns the
//! connection and the encryption key, and seals credentials before they reach the `kv` table.

mod kv;
mod push;
mod workspaces;

pub use push::StoredPushSubscription;
pub use workspaces::WorkspaceRecord;

use crate::StoreError;
use crate::secrets::{OpenError, Sealer, is_sealed, is_secret_key};
use crate::sqlite::{Db, compact, migrate, open_file};
use crate::util::{now, parse_time};
use chrono::{DateTime, Utc};
use kv::Kv;
use ostra_core::api::PushSubscription;
use ostra_core::ids::WorkspaceId;
use push::PushSubscriptions;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;
use workspaces::Workspaces;

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

/// Set once a rewrite after sealing finished, so plaintext left by an interrupted one is removed
/// at the next start.
const SECRETS_VACUUMED: &str = "secrets_vacuumed";

const ONBOARDED_AT: &str = "onboarded_at";

#[derive(Debug, Clone)]
pub struct RegistryDb {
    db: Db,
    sealer: Option<Arc<Sealer>>,
}

fn open_logged(sealer: &Sealer, key: &str, stored: &[u8]) -> Option<Vec<u8>> {
    match sealer.open(key, stored) {
        Ok(v) => Some(v),
        Err(OpenError::Undecryptable) => {
            tracing::warn!(
                "the saved credential under {key} cannot be decrypted, because the encryption key changed or the value was altered; enter it again in settings"
            );
            None
        }
    }
}

impl RegistryDb {
    /// Opens without an encryption key: credentials cannot be read or saved until
    /// [`RegistryDb::with_sealer`] gives it one.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = open_file(path, MIGRATIONS)?;
        // Freed pages are zeroed, so a replaced plaintext credential does not linger in the file.
        conn.pragma_update(None, "secure_delete", "ON")?;
        Ok(RegistryDb {
            db: Db::new(conn),
            sealer: None,
        })
    }

    /// An in-memory registry with a key that lives as long as the process.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn, MIGRATIONS)?;
        Ok(RegistryDb {
            db: Db::new(conn),
            sealer: Some(Arc::new(Sealer::ephemeral())),
        })
    }

    pub fn with_sealer(mut self, sealer: Arc<Sealer>) -> Self {
        self.sealer = Some(sealer);
        self
    }

    fn sealer(&self) -> Result<&Sealer, StoreError> {
        self.sealer.as_deref().ok_or(StoreError::Locked)
    }

    // -- workspaces ---------------------------------------------------------------------------

    /// Register a workspace. Fails with `Invalid` when the root is already registered.
    pub fn add_workspace(
        &self,
        id: &WorkspaceId,
        name: &str,
        root: &Path,
    ) -> Result<WorkspaceRecord, StoreError> {
        self.db.transaction(|c| {
            let workspaces = Workspaces(c);
            if let Some(other) = workspaces.by_root(root)? {
                return Err(StoreError::Invalid(format!(
                    "{} is already registered as workspace {}",
                    root.to_string_lossy(),
                    other.id
                )));
            }
            workspaces.insert(id, name, root)?;
            workspaces
                .get(id)?
                .ok_or_else(|| StoreError::NotFound(id.to_string()))
        })
    }

    pub fn rename_workspace(&self, id: &WorkspaceId, name: &str) -> Result<bool, StoreError> {
        self.db.run(|c| Workspaces(c).rename(id, name))
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, StoreError> {
        self.db.run(|c| Workspaces(c).list())
    }

    pub fn get_workspace(&self, id: &WorkspaceId) -> Result<Option<WorkspaceRecord>, StoreError> {
        self.db.run(|c| Workspaces(c).get(id))
    }

    pub fn workspace_by_root(&self, root: &Path) -> Result<Option<WorkspaceRecord>, StoreError> {
        self.db.run(|c| Workspaces(c).by_root(root))
    }

    /// Unregister a workspace and drop the push subscriptions scoped to it. Deletes nothing on
    /// disk.
    pub fn remove_workspace(&self, id: &WorkspaceId) -> Result<bool, StoreError> {
        self.db.transaction(|c| {
            PushSubscriptions(c).delete_for_workspace(id)?;
            Workspaces(c).delete(id)
        })
    }

    // -- push subscriptions -------------------------------------------------------------------

    /// Add or refresh a subscription, keyed by endpoint.
    pub fn add_push_subscription(
        &self,
        sub: &PushSubscription,
        workspace: Option<&WorkspaceId>,
    ) -> Result<(), StoreError> {
        self.db.run(|c| PushSubscriptions(c).upsert(sub, workspace))
    }

    pub fn list_push_subscriptions(&self) -> Result<Vec<StoredPushSubscription>, StoreError> {
        self.db.run(|c| PushSubscriptions(c).list())
    }

    pub fn remove_push_subscription(&self, endpoint: &str) -> Result<bool, StoreError> {
        self.db.run(|c| PushSubscriptions(c).delete(endpoint))
    }

    // -- kv -----------------------------------------------------------------------------------

    pub fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.db.run(|c| Kv(c).get(key))
    }

    pub fn kv_set(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.db.run(|c| Kv(c).set(key, value))
    }

    /// Sets `key` to `new` only while it still holds `expected`, so two callers cannot both win.
    pub fn kv_replace(&self, key: &str, expected: &[u8], new: &[u8]) -> Result<bool, StoreError> {
        self.db.run(|c| Kv(c).replace(key, expected, new))
    }

    pub fn kv_delete(&self, key: &str) -> Result<bool, StoreError> {
        self.db.run(|c| Kv(c).delete(key))
    }

    /// Every entry whose key starts with `prefix`, in key order.
    pub fn kv_scan(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        self.db.run(|c| Kv(c).scan(prefix))
    }

    // -- secrets ------------------------------------------------------------------------------

    /// A credential, decrypted. A value sealed under another key reads as absent, with a log line
    /// asking the user to enter it again.
    pub fn secret_get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        debug_assert!(is_secret_key(key), "{key} is not listed in SECRET_KEYS");
        let sealer = self.sealer()?;
        Ok(self
            .kv_get(key)?
            .and_then(|stored| open_logged(sealer, key, &stored)))
    }

    pub fn secret_set(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        debug_assert!(is_secret_key(key), "{key} is not listed in SECRET_KEYS");
        let sealed = self.sealer()?.seal(key, value);
        self.kv_set(key, &sealed)
    }

    /// Every credential under `prefix`, decrypted, skipping values that do not open.
    pub fn secret_scan(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        let sealer = self.sealer()?;
        Ok(self
            .kv_scan(prefix)?
            .into_iter()
            .filter_map(|(k, v)| open_logged(sealer, &k, &v).map(|p| (k, p)))
            .collect())
    }

    /// Seals every credential still stored as plaintext, then rewrites the file and empties the
    /// write-ahead log so the plaintext is gone from disk. The rewrite also runs when an earlier
    /// run sealed rows but stopped before finishing it. Returns how many rows were sealed.
    pub fn seal_plaintext_secrets(&self) -> Result<usize, StoreError> {
        let sealer = self.sealer()?;
        let mut sealed = 0;
        for (key, value) in self.kv_scan("")? {
            if is_secret_key(&key) && !is_sealed(&value) {
                let new = sealer.seal(&key, &value);
                if self.kv_replace(&key, &value, &new)? {
                    sealed += 1;
                }
            }
        }
        if sealed > 0 || self.kv_get(SECRETS_VACUUMED)?.is_none() {
            self.db.run(compact)?;
            self.kv_set(SECRETS_VACUUMED, b"1")?;
        }
        Ok(sealed)
    }

    // -- onboarding ---------------------------------------------------------------------------

    pub fn onboarded_at(&self) -> Result<Option<DateTime<Utc>>, StoreError> {
        self.kv_get(ONBOARDED_AT)?
            .map(|bytes| parse_time(&String::from_utf8_lossy(&bytes)))
            .transpose()
    }

    /// Record that the first-run setup is done. A second call keeps the first time.
    pub fn mark_onboarded(&self) -> Result<DateTime<Utc>, StoreError> {
        self.db
            .run(|c| Kv(c).insert_if_absent(ONBOARDED_AT, now().as_bytes()))?;
        self.onboarded_at()?
            .ok_or_else(|| StoreError::NotFound(ONBOARDED_AT.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_replace_lets_only_one_caller_claim_a_value() {
        let r = RegistryDb::open_in_memory().unwrap();
        r.kv_set("auth:token:t", b"100").unwrap();
        assert!(r.kv_replace("auth:token:t", b"100", b"used:1").unwrap());
        assert!(!r.kv_replace("auth:token:t", b"100", b"used:2").unwrap());
        assert_eq!(r.kv_get("auth:token:t").unwrap().unwrap(), b"used:1");
        assert!(!r.kv_replace("missing", b"x", b"y").unwrap());
    }
}
