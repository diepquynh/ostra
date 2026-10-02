use crate::StoreError;
use crate::sqlite::{RowExt, next_seq, query_all};
use crate::util::{now, parse_time};
use ostra_core::api::ActivityItem;
use ostra_core::exec::ExecutionDelta;
use ostra_core::ids::ExecutionId;
use rusqlite::{Connection, params};

/// The deltas an execution streamed, as the Activity view replays them.
pub(crate) struct Activity<'c>(pub &'c Connection);

impl Activity<'_> {
    /// Assigns the next sequence number, so run it in a transaction.
    pub fn append(
        &self,
        execution: &ExecutionId,
        delta: &ExecutionDelta,
    ) -> Result<ActivityItem, StoreError> {
        let seq = next_seq(self.0, "activity", "execution_id", execution.as_str())?;
        let at = now();
        self.0.execute(
            "INSERT INTO activity (execution_id, seq, delta, at) VALUES (?1, ?2, ?3, ?4)",
            params![execution.as_str(), seq, serde_json::to_string(delta)?, at],
        )?;
        Ok(ActivityItem {
            seq,
            at: parse_time(&at)?,
            delta: delta.clone(),
        })
    }

    pub fn after(
        &self,
        execution: &ExecutionId,
        after: i64,
    ) -> Result<Vec<ActivityItem>, StoreError> {
        query_all(
            self.0,
            "SELECT seq, at, delta FROM activity WHERE execution_id = ?1 AND seq > ?2 ORDER BY seq",
            params![execution.as_str(), after],
            |r| {
                Ok(ActivityItem {
                    seq: r.get("seq")?,
                    at: r.time("at")?,
                    delta: r.json("delta")?,
                })
            },
        )
    }
}
