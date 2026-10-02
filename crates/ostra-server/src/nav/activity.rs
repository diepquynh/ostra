//! `GET /api/workspaces/:ws/activity`: running executions, open gates, and recent spend.

use crate::api::ApiErr;
use chrono::{DateTime, Days, TimeZone, Utc};
use ostra_core::api::{OpenGateRef, RunningExecution, WorkspaceActivity};
use ostra_core::ids::{ExecutionId, SessionId};
use ostra_workspace::WorkspaceRt;
use std::collections::HashMap;

/// Starts of the spend windows for `now` in its own time zone: local midnight today, and local
/// midnight six days earlier, so "week" is today plus the six days before it.
pub fn windows<Tz: TimeZone>(now: &DateTime<Tz>) -> (DateTime<Utc>, DateTime<Utc>) {
    let tz = now.timezone();
    let midnight = |date: chrono::NaiveDate| {
        let naive = date.and_hms_opt(0, 0, 0).unwrap_or_default();
        // A midnight skipped by a daylight saving change falls back to the first hour that exists.
        tz.from_local_datetime(&naive)
            .earliest()
            .or_else(|| {
                tz.from_local_datetime(&(naive + chrono::Duration::hours(1)))
                    .earliest()
            })
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(|| now.with_timezone(&Utc))
    };
    let today = now.date_naive();
    let week = today.checked_sub_days(Days::new(6)).unwrap_or(today);
    (midnight(today), midnight(week))
}

pub fn build<Tz: TimeZone>(
    w: &WorkspaceRt,
    now: DateTime<Tz>,
) -> Result<WorkspaceActivity, ApiErr> {
    let mut labels: HashMap<SessionId, HashMap<ExecutionId, String>> = HashMap::new();
    let running =
        w.db.running_executions()?
            .into_iter()
            .map(|e| {
                let label = e
                    .session
                    .as_ref()
                    .and_then(|s| {
                        labels
                            .entry(s.clone())
                            .or_insert_with(|| w.engine.run_labels(s).unwrap_or_default())
                            .get(&e.id)
                            .cloned()
                    })
                    .unwrap_or(e.run_label);
                RunningExecution {
                    id: e.id,
                    session: e.session,
                    agent: e.agent,
                    project: (!e.spans_session).then_some(e.project),
                    run_label: label,
                    stream: e.stream,
                    summary: e.summary,
                }
            })
            .collect();
    let mut titles: HashMap<SessionId, Option<String>> = HashMap::new();
    let open_gates =
        w.db.open_gates(None)?
            .into_iter()
            .map(|g| {
                let session_title = titles
                    .entry(g.session.clone())
                    .or_insert_with(|| {
                        w.db.get_session(&g.session)
                            .ok()
                            .flatten()
                            .and_then(|s| s.title)
                    })
                    .clone();
                OpenGateRef {
                    kind: g.payload.kind_str().to_string(),
                    id: g.id,
                    session: g.session,
                    session_title,
                    title: g.title,
                    opened_at: g.opened_at,
                }
            })
            .collect();
    let (today, week) = windows(&now);
    Ok(WorkspaceActivity {
        running,
        open_gates,
        spend_today_usd: w.db.spend_since(today)?,
        spend_week_usd: w.db.spend_since(week)?,
        today_since: today,
        week_since: week,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    #[test]
    fn windows_start_at_local_midnight() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        // 01:30 local on 24 September is 23:30 UTC the day before.
        let now = tz.with_ymd_and_hms(2026, 9, 24, 1, 30, 0).unwrap();
        let (today, week) = windows(&now);
        assert_eq!(today, Utc.with_ymd_and_hms(2026, 9, 23, 22, 0, 0).unwrap());
        assert_eq!(week, Utc.with_ymd_and_hms(2026, 9, 17, 22, 0, 0).unwrap());
        let west = FixedOffset::west_opt(7 * 3600).unwrap();
        let (today, week) = windows(&west.with_ymd_and_hms(2026, 3, 2, 23, 59, 0).unwrap());
        assert_eq!(today, Utc.with_ymd_and_hms(2026, 3, 2, 7, 0, 0).unwrap());
        assert_eq!(
            week,
            Utc.with_ymd_and_hms(2026, 2, 24, 7, 0, 0).unwrap(),
            "the week crosses the month end"
        );
    }
}
