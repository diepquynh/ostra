use crate::StoreError;
use crate::sqlite::{RowExt, query_all, query_one};
use crate::util::{enum_str, now};
use ostra_core::api::GateView;
use ostra_core::event::{AnswerSource, GateAnswer, GatePayload};
use ostra_core::ids::{GateId, SessionId};
use rusqlite::{Connection, Row, named_params, params};

const COLS: &str =
    "id, session_id, title, explanation, payload, answer, source, reason, opened_at, answered_at";

pub(crate) struct Gates<'c>(pub &'c Connection);

impl Gates<'_> {
    /// Inserts a gate, or refreshes its title, explanation, and payload if it exists.
    pub fn upsert(
        &self,
        session: &SessionId,
        id: &GateId,
        title: &str,
        explanation: &str,
        payload: &GatePayload,
    ) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO gates (id, session_id, title, explanation, payload, opened_at)
             VALUES (:id, :session, :title, :explanation, :payload, :opened_at)
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, explanation = excluded.explanation,
             payload = excluded.payload",
            named_params! {
                ":id": id.as_str(),
                ":session": session.as_str(),
                ":title": title,
                ":explanation": explanation,
                ":payload": serde_json::to_string(payload)?,
                ":opened_at": now(),
            },
        )?;
        Ok(())
    }

    /// Returns false when no gate has the id.
    pub fn answer(
        &self,
        id: &GateId,
        source: AnswerSource,
        answer: &GateAnswer,
        reason: Option<&str>,
    ) -> Result<bool, StoreError> {
        let n = self.0.execute(
            "UPDATE gates SET answer = :answer, source = :source, reason = :reason, answered_at = :at
             WHERE id = :id",
            named_params! {
                ":answer": serde_json::to_string(answer)?,
                ":source": enum_str(&source)?,
                ":reason": reason,
                ":at": now(),
                ":id": id.as_str(),
            },
        )?;
        Ok(n > 0)
    }

    pub fn get(&self, id: &GateId) -> Result<Option<GateView>, StoreError> {
        query_one(
            self.0,
            &format!("SELECT {COLS} FROM gates WHERE id = ?1"),
            [id.as_str()],
            gate,
        )
    }

    /// Oldest first.
    pub fn of_session(&self, session: &SessionId) -> Result<Vec<GateView>, StoreError> {
        query_all(
            self.0,
            &format!("SELECT {COLS} FROM gates WHERE session_id = ?1 ORDER BY opened_at, id"),
            [session.as_str()],
            gate,
        )
    }

    /// Unanswered gates of one session, or of the whole workspace with `None`. Oldest first.
    pub fn open(&self, session: Option<&SessionId>) -> Result<Vec<GateView>, StoreError> {
        query_all(
            self.0,
            &format!(
                "SELECT {COLS} FROM gates WHERE answer IS NULL AND (?1 IS NULL OR session_id = ?1)
                 ORDER BY opened_at, id"
            ),
            params![session.map(SessionId::as_str)],
            gate,
        )
    }
}

fn gate(r: &Row<'_>) -> Result<GateView, StoreError> {
    Ok(GateView {
        id: GateId(r.get("id")?),
        session: SessionId(r.get("session_id")?),
        title: r.get("title")?,
        explanation: r.get("explanation")?,
        payload: r.json("payload")?,
        answer: r.json_opt("answer")?,
        source: r.variant_opt("source")?,
        reason: r.get("reason")?,
        opened_at: r.time("opened_at")?,
        answered_at: r.time_opt("answered_at")?,
    })
}
