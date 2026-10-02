use crate::StoreError;
use crate::sqlite::{RowExt, next_seq, query_all};
use crate::util::{now, parse_time};
use ostra_core::event::{SessionEvent, StoredEvent};
use ostra_core::ids::SessionId;
use rusqlite::{Connection, params};

/// The session event log, the source of truth every other table is derived from.
pub(crate) struct Events<'c>(pub &'c Connection);

impl Events<'_> {
    /// Assigns the next sequence number, so run it in a transaction.
    pub fn append(
        &self,
        session: &SessionId,
        event: &SessionEvent,
    ) -> Result<StoredEvent, StoreError> {
        let seq = next_seq(self.0, "events", "session_id", session.as_str())?;
        let payload = serde_json::to_value(event)?;
        let kind = payload
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let at = now();
        self.0.execute(
            "INSERT INTO events (session_id, seq, type, payload, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session.as_str(), seq, kind, payload.to_string(), at],
        )?;
        Ok(StoredEvent {
            seq,
            at: parse_time(&at)?,
            event: event.clone(),
        })
    }

    pub fn after(&self, session: &SessionId, after: i64) -> Result<Vec<StoredEvent>, StoreError> {
        query_all(
            self.0,
            "SELECT seq, at, payload FROM events WHERE session_id = ?1 AND seq > ?2 ORDER BY seq",
            params![session.as_str(), after],
            |r| {
                Ok(StoredEvent {
                    seq: r.get("seq")?,
                    at: r.time("at")?,
                    event: r.json("payload")?,
                })
            },
        )
    }
}
