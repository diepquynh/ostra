use crate::StoreError;
use crate::sqlite::{RowExt, query_all};
use crate::util::ts;
use chrono::{DateTime, Utc};
use ostra_core::api::{CostReport, CostRow};
use ostra_core::exec::Usage;
use rusqlite::Connection;
use std::collections::BTreeMap;

/// Spend read from the `executions` table.
pub(crate) struct Costs<'c>(pub &'c Connection);

impl Costs<'_> {
    /// Dollars executions spent that started at or after `since`.
    pub fn since(&self, since: DateTime<Utc>) -> Result<f64, StoreError> {
        Ok(self.0.query_row(
            "SELECT COALESCE(SUM(cost_usd), 0) FROM executions WHERE started_at >= ?1",
            [ts(since)],
            |r| r.get(0),
        )?)
    }

    /// Spend grouped four ways, over executions that started at or after `since` (all when `None`).
    pub fn report(&self, since: Option<DateTime<Utc>>) -> Result<CostReport, StoreError> {
        let rows = query_all(
            self.0,
            "SELECT session_id, stage, agent, executor, usage FROM executions WHERE started_at >= ?1",
            [since.map(ts).unwrap_or_default()],
            |r| {
                Ok((
                    r.get::<_, Option<String>>("session_id")?,
                    r.get::<_, Option<String>>("stage")?,
                    r.get::<_, String>("agent")?,
                    r.get::<_, String>("executor")?,
                    r.json::<Usage>("usage")?,
                ))
            },
        )?;
        let mut by_session = Groups::new();
        let mut by_stage = Groups::new();
        let mut by_agent = Groups::new();
        let mut by_executor = Groups::new();
        let mut total = (0u32, Usage::default());
        for (session, stage, agent, executor, usage) in rows {
            add(
                &mut by_session,
                session.unwrap_or_else(|| "side-panel".into()),
                &usage,
            );
            add(
                &mut by_stage,
                stage.unwrap_or_else(|| "none".into()),
                &usage,
            );
            add(&mut by_agent, agent, &usage);
            add(&mut by_executor, executor, &usage);
            total.0 += 1;
            total.1.add(&usage);
        }
        Ok(CostReport {
            since,
            by_session: rows_of(by_session),
            by_stage: rows_of(by_stage),
            by_agent: rows_of(by_agent),
            by_executor: rows_of(by_executor),
            total: cost_row("total".into(), total.0, total.1),
        })
    }
}

/// Key to execution count and summed usage.
type Groups = BTreeMap<String, (u32, Usage)>;

fn add(groups: &mut Groups, key: String, usage: &Usage) {
    let entry = groups.entry(key).or_default();
    entry.0 += 1;
    entry.1.add(usage);
}

/// Most expensive first, ties by key.
fn rows_of(groups: Groups) -> Vec<CostRow> {
    let mut v: Vec<CostRow> = groups
        .into_iter()
        .map(|(k, (n, u))| cost_row(k, n, u))
        .collect();
    v.sort_by(|a, b| {
        b.usage
            .cost_usd
            .total_cmp(&a.usage.cost_usd)
            .then(a.key.cmp(&b.key))
    });
    v
}

fn cost_row(key: String, executions: u32, usage: Usage) -> CostRow {
    let per_call = if usage.tool_calls == 0 {
        0.0
    } else {
        usage.cache_read_tokens as f64 / usage.tool_calls as f64
    };
    CostRow {
        key,
        executions,
        usage,
        cache_reads_per_tool_call: per_call,
    }
}
