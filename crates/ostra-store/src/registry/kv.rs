use crate::StoreError;
use crate::sqlite::query_all;
use rusqlite::{Connection, OptionalExtension, params};

/// Raw key-value rows. Credentials are sealed and opened by `RegistryDb`, never here.
pub(crate) struct Kv<'c>(pub &'c Connection);

impl Kv<'_> {
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .0
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Keeps an existing value.
    pub fn insert_if_absent(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT OR IGNORE INTO kv (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Sets `key` to `new` only while it still holds `expected`.
    pub fn replace(&self, key: &str, expected: &[u8], new: &[u8]) -> Result<bool, StoreError> {
        Ok(self.0.execute(
            "UPDATE kv SET value = ?3 WHERE key = ?1 AND value = ?2",
            params![key, expected, new],
        )? > 0)
    }

    pub fn delete(&self, key: &str) -> Result<bool, StoreError> {
        Ok(self.0.execute("DELETE FROM kv WHERE key = ?1", [key])? > 0)
    }

    /// Every entry whose key starts with `prefix`, in key order.
    pub fn scan(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        query_all(
            self.0,
            "SELECT key, value FROM kv WHERE substr(key, 1, length(?1)) = ?1 ORDER BY key",
            [prefix],
            |r| Ok((r.get("key")?, r.get("value")?)),
        )
    }
}
