use crate::StoreError;
use crate::sqlite::{RowExt, query_all, query_one};
use crate::util::{enum_str, now};
use ostra_core::api::DecisionView;
use ostra_core::event::JudgeKind;
use ostra_core::ids::{DecisionId, SessionId};
use rusqlite::{Connection, OptionalExtension, Row, named_params, params};

const COLS: &str =
    "id, judge, subject, input_summary, output, reason, overridden, can_override, at";

/// Judge decisions, as the Decisions view lists them.
pub(crate) struct Decisions<'c>(pub &'c Connection);

impl Decisions<'_> {
    #[allow(clippy::too_many_arguments)]
    pub fn insert(
        &self,
        session: &SessionId,
        id: &DecisionId,
        judge: JudgeKind,
        subject: Option<&str>,
        input_summary: &str,
        output: &serde_json::Value,
        reason: &str,
    ) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO decisions (id, session_id, judge, subject, input_summary, output, reason, at)
             VALUES (:id, :session, :judge, :subject, :input_summary, :output, :reason, :at)",
            named_params! {
                ":id": id.as_str(),
                ":session": session.as_str(),
                ":judge": enum_str(&judge)?,
                ":subject": subject,
                ":input_summary": input_summary,
                ":output": output.to_string(),
                ":reason": reason,
                ":at": now(),
            },
        )?;
        Ok(())
    }

    /// Replaces the output with the user's and flags the row. Returns false when no decision has
    /// the id.
    pub fn override_with(
        &self,
        id: &DecisionId,
        output: &serde_json::Value,
        reason: &str,
    ) -> Result<bool, StoreError> {
        let n = self.0.execute(
            "UPDATE decisions SET overridden = 1, output = ?1, reason = ?2 WHERE id = ?3",
            params![output.to_string(), reason, id.as_str()],
        )?;
        Ok(n > 0)
    }

    pub fn set_can_override(&self, id: &DecisionId, can: bool) -> Result<(), StoreError> {
        self.0.execute(
            "UPDATE decisions SET can_override = ?1 WHERE id = ?2",
            params![can, id.as_str()],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &DecisionId) -> Result<Option<DecisionView>, StoreError> {
        query_one(
            self.0,
            &format!("SELECT {COLS} FROM decisions WHERE id = ?1"),
            [id.as_str()],
            decision,
        )
    }

    pub fn session_of(&self, id: &DecisionId) -> Result<Option<SessionId>, StoreError> {
        Ok(self
            .0
            .query_row(
                "SELECT session_id FROM decisions WHERE id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()?
            .map(SessionId))
    }

    /// Oldest first.
    pub fn of_session(&self, session: &SessionId) -> Result<Vec<DecisionView>, StoreError> {
        query_all(
            self.0,
            &format!("SELECT {COLS} FROM decisions WHERE session_id = ?1 ORDER BY at, id"),
            [session.as_str()],
            decision,
        )
    }
}

fn decision(r: &Row<'_>) -> Result<DecisionView, StoreError> {
    Ok(DecisionView {
        id: DecisionId(r.get("id")?),
        judge: r.variant("judge")?,
        subject: r.get("subject")?,
        input_summary: r.get("input_summary")?,
        output: r.json("output")?,
        reason: r.get("reason")?,
        overridden: r.get("overridden")?,
        can_override: r.get("can_override")?,
        at: r.time("at")?,
    })
}
