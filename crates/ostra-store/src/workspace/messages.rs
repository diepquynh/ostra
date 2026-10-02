use crate::StoreError;
use crate::sqlite::{RowExt, next_seq, query_all};
use ostra_core::ids::ExecutionId;
use rusqlite::{Connection, params};

#[derive(Debug, Clone, PartialEq)]
pub struct StoredMessage {
    pub seq: i64,
    pub role: String,
    pub content: serde_json::Value,
}

/// An execution's model transcript.
pub(crate) struct Messages<'c>(pub &'c Connection);

impl Messages<'_> {
    /// Assigns the next sequence number, so run it in a transaction.
    pub fn append(
        &self,
        execution: &ExecutionId,
        role: &str,
        content: &serde_json::Value,
    ) -> Result<i64, StoreError> {
        let seq = next_seq(self.0, "messages", "execution_id", execution.as_str())?;
        self.0.execute(
            "INSERT INTO messages (execution_id, seq, role, content) VALUES (?1, ?2, ?3, ?4)",
            params![execution.as_str(), seq, role, content.to_string()],
        )?;
        Ok(seq)
    }

    pub fn list(&self, execution: &ExecutionId) -> Result<Vec<StoredMessage>, StoreError> {
        query_all(
            self.0,
            "SELECT seq, role, content FROM messages WHERE execution_id = ?1 ORDER BY seq",
            [execution.as_str()],
            |r| {
                Ok(StoredMessage {
                    seq: r.get("seq")?,
                    role: r.get("role")?,
                    content: r.json("content")?,
                })
            },
        )
    }
}
