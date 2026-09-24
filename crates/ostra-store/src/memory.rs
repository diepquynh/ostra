//! Durable project lessons in `<project>/.ostra/memory/knowledge.sqlite3`, using Ultracode's
//! `mcp/lib/memory.js` schema: dedupe on (area, lesson), area sub-scopes (`area::sub`), bm25
//! ranking, and targeted forget. Never capped and never auto-expired.

use crate::StoreError;
use ostra_core::api::Lesson;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_RECALL_LIMIT: usize = 8;

const SCHEMA: &str = "
  CREATE TABLE IF NOT EXISTS lessons (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    area TEXT NOT NULL,
    lesson TEXT NOT NULL,
    source TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(area, lesson)
  );
  CREATE VIRTUAL TABLE IF NOT EXISTS lessons_fts USING fts5(area, lesson, content='lessons', content_rowid='id');
  CREATE TRIGGER IF NOT EXISTS lessons_ai AFTER INSERT ON lessons BEGIN
    INSERT INTO lessons_fts(rowid, area, lesson) VALUES (new.id, new.area, new.lesson);
  END;
  CREATE TRIGGER IF NOT EXISTS lessons_ad AFTER DELETE ON lessons BEGIN
    INSERT INTO lessons_fts(lessons_fts, rowid, area, lesson) VALUES('delete', old.id, old.area, old.lesson);
  END;
  CREATE TRIGGER IF NOT EXISTS lessons_au AFTER UPDATE ON lessons BEGIN
    INSERT INTO lessons_fts(lessons_fts, rowid, area, lesson) VALUES('delete', old.id, old.area, old.lesson);
    INSERT INTO lessons_fts(rowid, area, lesson) VALUES (new.id, new.area, new.lesson);
  END;
";

/// One project's lesson store. Opens a fresh connection per call so parallel executions wait on
/// SQLite's file lock instead of sharing a handle.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    path: std::path::PathBuf,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn row_to_lesson(row: &rusqlite::Row<'_>) -> rusqlite::Result<Lesson> {
    Ok(Lesson {
        id: row.get(0)?,
        area: row.get(1)?,
        lesson: row.get(2)?,
        source: row.get(3)?,
        created_at: row.get(4)?,
    })
}

/// Any-token-matches FTS query, each token quoted, as `ftsQueryFromText` does.
pub fn fts_query_from_text(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

impl MemoryStore {
    /// Creates the directory and schema if needed.
    pub fn open(db_path: &Path) -> Result<Self, StoreError> {
        let store = MemoryStore {
            path: db_path.to_path_buf(),
        };
        store.conn()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn conn(&self) -> Result<Connection, StoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(Duration::from_millis(5000))?;
        conn.execute_batch(SCHEMA)?;
        Ok(conn)
    }

    /// Record a lesson. The newest occurrence of an (area, lesson) pair wins. Returns the total.
    pub fn record(&self, area: &str, lesson: &str, source: &str) -> Result<u64, StoreError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO lessons (area, lesson, source, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(area, lesson) DO UPDATE SET source = excluded.source, created_at = excluded.created_at",
            params![area, lesson, source, now()],
        )?;
        let total: i64 = conn.query_row("SELECT count(*) FROM lessons", [], |r| r.get(0))?;
        Ok(total as u64)
    }

    /// Buckets, most relevant first: lessons in `area` (and `area::*`) ranked by the query or by
    /// recency; then global text matches as fill; then, with neither, the most recent overall.
    pub fn recall(
        &self,
        area: Option<&str>,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Lesson>, StoreError> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let conn = self.conn()?;
        let fts = query.map(fts_query_from_text).filter(|q| !q.is_empty());
        let area = area.map(str::trim).filter(|a| !a.is_empty());
        let cols = "l.id, l.area, l.lesson, l.source, l.created_at";
        let mut buckets: Vec<Vec<Lesson>> = vec![];
        if let (Some(area), Some(fts)) = (area, fts.as_deref()) {
            let mut st = conn.prepare(&format!(
                "SELECT {cols} FROM lessons_fts JOIN lessons l ON l.id = lessons_fts.rowid
                 WHERE lessons_fts MATCH ?1 AND (l.area = ?2 OR l.area LIKE ?3) ORDER BY bm25(lessons_fts)"
            ))?;
            buckets.push(
                st.query_map(params![fts, area, format!("{area}::%")], row_to_lesson)?
                    .collect::<Result<_, _>>()?,
            );
        } else if let Some(area) = area {
            let mut st = conn.prepare(&format!(
                "SELECT {cols} FROM lessons l WHERE l.area = ?1 OR l.area LIKE ?2 ORDER BY l.created_at DESC"
            ))?;
            buckets.push(
                st.query_map(params![area, format!("{area}::%")], row_to_lesson)?
                    .collect::<Result<_, _>>()?,
            );
        }
        if let Some(fts) = fts.as_deref() {
            let mut st = conn.prepare(&format!(
                "SELECT {cols} FROM lessons_fts JOIN lessons l ON l.id = lessons_fts.rowid
                 WHERE lessons_fts MATCH ?1 ORDER BY bm25(lessons_fts)"
            ))?;
            buckets.push(
                st.query_map(params![fts], row_to_lesson)?
                    .collect::<Result<_, _>>()?,
            );
        } else if area.is_none() {
            let mut st = conn.prepare(&format!(
                "SELECT {cols} FROM lessons l ORDER BY l.created_at DESC"
            ))?;
            buckets.push(st.query_map([], row_to_lesson)?.collect::<Result<_, _>>()?);
        }
        let mut seen = std::collections::HashSet::new();
        let mut out = vec![];
        for bucket in buckets {
            for lesson in bucket {
                if seen.insert((lesson.area.clone(), lesson.lesson.clone())) {
                    out.push(lesson);
                    if out.len() >= limit {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Exact removal of one confirmed-stale lesson. A no-op when it is absent.
    pub fn forget(&self, area: &str, lesson: &str) -> Result<bool, StoreError> {
        if !self.path.exists() {
            return Ok(false);
        }
        let conn = self.conn()?;
        let n = conn.execute(
            "DELETE FROM lessons WHERE area = ?1 AND lesson = ?2",
            params![area, lesson],
        )?;
        Ok(n > 0)
    }

    /// For the memory browser: every lesson, or those matching `query`, newest first.
    pub fn list(
        &self,
        query: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Lesson>, StoreError> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let conn = self.conn()?;
        let fts = query.map(fts_query_from_text).filter(|q| !q.is_empty());
        let cols = "l.id, l.area, l.lesson, l.source, l.created_at";
        let rows = match fts {
            Some(fts) => {
                let mut st = conn.prepare(&format!(
                    "SELECT {cols} FROM lessons_fts JOIN lessons l ON l.id = lessons_fts.rowid
                     WHERE lessons_fts MATCH ?1 ORDER BY bm25(lessons_fts) LIMIT ?2 OFFSET ?3"
                ))?;
                st.query_map(params![fts, limit as i64, offset as i64], row_to_lesson)?
                    .collect::<Result<_, _>>()?
            }
            None => {
                let mut st = conn.prepare(&format!(
                    "SELECT {cols} FROM lessons l ORDER BY l.created_at DESC LIMIT ?1 OFFSET ?2"
                ))?;
                st.query_map(params![limit as i64, offset as i64], row_to_lesson)?
                    .collect::<Result<_, _>>()?
            }
        };
        Ok(rows)
    }

    /// A user edit from the memory browser.
    pub fn update(&self, id: i64, area: &str, lesson: &str) -> Result<Lesson, StoreError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE lessons SET area = ?1, lesson = ?2, source = 'user', created_at = ?3 WHERE id = ?4",
            params![area, lesson, now(), id],
        )?;
        conn.query_row(
            "SELECT id, area, lesson, source, created_at FROM lessons WHERE id = ?1",
            params![id],
            row_to_lesson,
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(format!("lesson {id}")))
    }

    pub fn delete(&self, id: i64) -> Result<bool, StoreError> {
        if !self.path.exists() {
            return Ok(false);
        }
        let conn = self.conn()?;
        Ok(conn.execute("DELETE FROM lessons WHERE id = ?1", params![id])? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, MemoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = MemoryStore::open(&dir.path().join("memory/knowledge.sqlite3")).unwrap();
        (dir, s)
    }

    #[test]
    fn dedupes_on_area_and_lesson() {
        let (_d, s) = store();
        assert_eq!(
            s.record("orders", "cancel needs ownership check", "explore")
                .unwrap(),
            1
        );
        assert_eq!(
            s.record("orders", "cancel needs ownership check", "implementer")
                .unwrap(),
            1
        );
        let got = s.recall(Some("orders"), None, 8).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source, "implementer");
    }

    #[test]
    fn area_scope_then_global_fill() {
        let (_d, s) = store();
        s.record("orders::Service", "refund validates ownership", "a")
            .unwrap();
        s.record("billing", "ownership lives in billing too", "a")
            .unwrap();
        s.record("web", "unrelated lesson", "a").unwrap();
        let got = s.recall(Some("orders"), Some("ownership"), 8).unwrap();
        assert_eq!(got[0].area, "orders::Service");
        assert_eq!(got.len(), 2);
        let all = s.recall(None, None, 8).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn forget_is_targeted() {
        let (_d, s) = store();
        s.record("a", "x", "s").unwrap();
        s.record("a", "y", "s").unwrap();
        assert!(s.forget("a", "x").unwrap());
        assert!(!s.forget("a", "x").unwrap());
        assert_eq!(s.recall(Some("a"), None, 8).unwrap().len(), 1);
        assert!(s.recall(None, Some("x"), 8).unwrap().is_empty());
    }

    #[test]
    fn edit_and_delete() {
        let (_d, s) = store();
        s.record("a", "x", "s").unwrap();
        let id = s.list(None, 10, 0).unwrap()[0].id;
        let l = s.update(id, "b", "z").unwrap();
        assert_eq!(
            (l.area.as_str(), l.lesson.as_str(), l.source.as_str()),
            ("b", "z", "user")
        );
        assert_eq!(s.list(Some("z"), 10, 0).unwrap().len(), 1);
        assert!(s.delete(id).unwrap());
        assert!(s.list(None, 10, 0).unwrap().is_empty());
    }

    #[test]
    fn missing_store_recalls_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let s = MemoryStore {
            path: dir.path().join("none.sqlite3"),
        };
        assert!(s.recall(None, Some("x"), 8).unwrap().is_empty());
        assert!(!s.forget("a", "b").unwrap());
    }
}
