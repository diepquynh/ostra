use crate::StoreError;
use crate::sqlite::query_all;
use ostra_core::ids::SessionId;
use rusqlite::{Connection, named_params, params};
use std::collections::HashMap;

/// One text-search match: a session (`ref` is its id) or an artifact (`ref` is its path).
#[derive(Debug, Clone, PartialEq)]
pub struct TextHit {
    pub kind: String,
    pub reference: String,
    pub session: SessionId,
    pub label: String,
    pub body: String,
    /// FTS5 bm25 rank; lower is better.
    pub rank: f64,
}

/// A prefix query over every word of `text`, all words required: `"cance"* "ord"*`.
pub fn fts_prefix_query(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `search_docs` full-text index. Triggers keep its session rows in step with `sessions`;
/// artifacts are indexed here.
pub(crate) struct Search<'c>(pub &'c Connection);

impl Search<'_> {
    pub fn index_artifact(
        &self,
        session: &SessionId,
        path: &str,
        label: &str,
        body: &str,
        mtime: i64,
    ) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO search_docs (kind, ref, session_id, label, body, mtime)
             VALUES ('artifact', :path, :session, :label, :body, :mtime)
             ON CONFLICT(kind, ref) DO UPDATE SET session_id = excluded.session_id, label = excluded.label,
             body = excluded.body, mtime = excluded.mtime",
            named_params! {
                ":path": path,
                ":session": session.as_str(),
                ":label": label,
                ":body": body,
                ":mtime": mtime,
            },
        )?;
        Ok(())
    }

    /// Path to label and mtime.
    pub fn artifacts_of(
        &self,
        session: &SessionId,
    ) -> Result<HashMap<String, (String, i64)>, StoreError> {
        let rows = query_all(
            self.0,
            "SELECT ref, label, mtime FROM search_docs WHERE kind = 'artifact' AND session_id = ?1",
            [session.as_str()],
            |r| Ok((r.get("ref")?, (r.get("label")?, r.get("mtime")?))),
        )?;
        Ok(rows.into_iter().collect())
    }

    /// Best first. Labels weigh four times the body.
    pub fn matching(&self, fts_query: &str, limit: usize) -> Result<Vec<TextHit>, StoreError> {
        query_all(
            self.0,
            "SELECT d.kind, d.ref, d.session_id, d.label, d.body, bm25(search_fts, 4.0, 1.0) AS score
             FROM search_fts JOIN search_docs d ON d.id = search_fts.rowid
             WHERE search_fts MATCH ?1 ORDER BY score LIMIT ?2",
            params![fts_query, limit as i64],
            |r| {
                Ok(TextHit {
                    kind: r.get("kind")?,
                    reference: r.get("ref")?,
                    session: SessionId(r.get("session_id")?),
                    label: r.get("label")?,
                    body: r.get("body")?,
                    rank: r.get("score")?,
                })
            },
        )
    }
}
