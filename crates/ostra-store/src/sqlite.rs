//! Plumbing every store shares: opening and migrating a database file, the locked connection the
//! repositories run on, and typed reads out of rows.

use crate::StoreError;
use crate::util::{enum_from, parse_time};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, Params, Row, TransactionBehavior};
use serde::de::DeserializeOwned;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

pub(crate) fn open_file(path: &Path, migrations: &[&str]) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        restrict(parent, 0o700)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn, migrations)?;
    // The event log holds tool output and the registry holds credentials, and SQLite creates its
    // side files with the process umask.
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        restrict(Path::new(&p), 0o600)?;
    }
    Ok(conn)
}

fn restrict(path: &Path, mode: u32) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            r => r?,
        }
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// Runs the migrations past `user_version`, each in its own transaction.
pub(crate) fn migrate(conn: &Connection, migrations: &[&str]) -> Result<(), StoreError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in migrations.iter().enumerate().skip(current.max(0) as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

/// Rewrites the file and empties the write-ahead log, so deleted rows are gone from disk.
pub(crate) fn compact(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("VACUUM;")?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    Ok(())
}

/// One connection shared by every clone of a store.
#[derive(Debug, Clone)]
pub(crate) struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn new(conn: Connection) -> Self {
        Db {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Each statement in `f` commits on its own.
    pub fn run<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        f(&self.lock())
    }

    /// `f` commits as one unit or not at all. The write lock is taken up front, so a concurrent
    /// writer waits instead of failing to upgrade a read.
    pub fn transaction<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

pub(crate) fn query_all<T>(
    conn: &Connection,
    sql: &str,
    params: impl Params,
    mut map: impl FnMut(&Row<'_>) -> Result<T, StoreError>,
) -> Result<Vec<T>, StoreError> {
    let mut st = conn.prepare(sql)?;
    let mut rows = st.query(params)?;
    let mut out = vec![];
    while let Some(row) = rows.next()? {
        out.push(map(row)?);
    }
    Ok(out)
}

pub(crate) fn query_one<T>(
    conn: &Connection,
    sql: &str,
    params: impl Params,
    map: impl FnOnce(&Row<'_>) -> Result<T, StoreError>,
) -> Result<Option<T>, StoreError> {
    let mut st = conn.prepare(sql)?;
    let mut rows = st.query(params)?;
    rows.next()?.map(map).transpose()
}

/// The next `seq` for a child row under `parent`. Call it inside a transaction, so two appends
/// cannot take the same number.
pub(crate) fn next_seq(
    conn: &Connection,
    table: &'static str,
    parent_col: &'static str,
    parent: &str,
) -> Result<i64, StoreError> {
    Ok(conn.query_row(
        &format!("SELECT COALESCE(MAX(seq), 0) + 1 FROM {table} WHERE {parent_col} = ?1"),
        [parent],
        |r| r.get(0),
    )?)
}

/// Reads of TEXT columns that hold JSON, serde enum names, or RFC 3339 times.
pub(crate) trait RowExt {
    fn json<T: DeserializeOwned>(&self, col: &str) -> Result<T, StoreError>;
    fn json_opt<T: DeserializeOwned>(&self, col: &str) -> Result<Option<T>, StoreError>;
    fn variant<T: DeserializeOwned>(&self, col: &str) -> Result<T, StoreError>;
    fn variant_opt<T: DeserializeOwned>(&self, col: &str) -> Result<Option<T>, StoreError>;
    fn time(&self, col: &str) -> Result<DateTime<Utc>, StoreError>;
    fn time_opt(&self, col: &str) -> Result<Option<DateTime<Utc>>, StoreError>;
}

impl RowExt for Row<'_> {
    fn json<T: DeserializeOwned>(&self, col: &str) -> Result<T, StoreError> {
        Ok(serde_json::from_str(&self.get::<_, String>(col)?)?)
    }

    fn json_opt<T: DeserializeOwned>(&self, col: &str) -> Result<Option<T>, StoreError> {
        Ok(self
            .get::<_, Option<String>>(col)?
            .map(|s| serde_json::from_str(&s))
            .transpose()?)
    }

    fn variant<T: DeserializeOwned>(&self, col: &str) -> Result<T, StoreError> {
        enum_from(&self.get::<_, String>(col)?)
    }

    fn variant_opt<T: DeserializeOwned>(&self, col: &str) -> Result<Option<T>, StoreError> {
        self.get::<_, Option<String>>(col)?
            .map(|s| enum_from(&s))
            .transpose()
    }

    fn time(&self, col: &str) -> Result<DateTime<Utc>, StoreError> {
        parse_time(&self.get::<_, String>(col)?)
    }

    fn time_opt(&self, col: &str) -> Result<Option<DateTime<Utc>>, StoreError> {
        self.get::<_, Option<String>>(col)?
            .map(|s| parse_time(&s))
            .transpose()
    }
}
