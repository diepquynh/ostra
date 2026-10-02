use crate::StoreError;
use crate::sqlite::{RowExt, query_all};
use crate::util::{now, ts};
use chrono::{DateTime, Utc};
use ostra_core::ids::ExecutionId;
use ostra_core::policy::PolicyDecision;
use rusqlite::{Connection, named_params};

/// Tool call output kept in the table is truncated to this many bytes.
pub const TOOL_OUTPUT_LIMIT: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallRecord {
    pub execution: ExecutionId,
    pub call_id: String,
    pub tool: String,
    pub input: serde_json::Value,
    pub decision: Option<PolicyDecision>,
    pub rule: Option<String>,
    pub duration_ms: Option<u64>,
    pub output: Option<String>,
    pub at: Option<DateTime<Utc>>,
}

pub(crate) struct ToolCalls<'c>(pub &'c Connection);

impl ToolCalls<'_> {
    pub fn record(&self, rec: &ToolCallRecord) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO tool_calls (execution_id, call_id, tool, input, decision, rule, duration_ms, output, at)
             VALUES (:execution, :call_id, :tool, :input, :decision, :rule, :duration_ms, :output, :at)",
            named_params! {
                ":execution": rec.execution.as_str(),
                ":call_id": rec.call_id,
                ":tool": rec.tool,
                ":input": rec.input.to_string(),
                ":decision": rec.decision.as_ref().map(serde_json::to_string).transpose()?,
                ":rule": rec.rule,
                ":duration_ms": rec.duration_ms.map(|d| d as i64),
                ":output": rec.output.as_deref().map(|o| truncate_utf8(o, TOOL_OUTPUT_LIMIT)),
                ":at": rec.at.map(ts).unwrap_or_else(now),
            },
        )?;
        Ok(())
    }

    /// In the order they were recorded.
    pub fn of_execution(&self, execution: &ExecutionId) -> Result<Vec<ToolCallRecord>, StoreError> {
        query_all(
            self.0,
            "SELECT execution_id, call_id, tool, input, decision, rule, duration_ms, output, at
             FROM tool_calls WHERE execution_id = ?1 ORDER BY id",
            [execution.as_str()],
            |r| {
                Ok(ToolCallRecord {
                    execution: ExecutionId(r.get("execution_id")?),
                    call_id: r.get("call_id")?,
                    tool: r.get("tool")?,
                    input: r.json("input")?,
                    decision: r.json_opt("decision")?,
                    rule: r.get("rule")?,
                    duration_ms: r.get::<_, Option<i64>>("duration_ms")?.map(|d| d as u64),
                    output: r.get("output")?,
                    at: Some(r.time("at")?),
                })
            },
        )
    }
}

fn truncate_utf8(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    let mut end = limit;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated: {} bytes]", &s[..end], s.len())
}
