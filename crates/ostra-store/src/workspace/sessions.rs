use super::meta::Meta;
use crate::StoreError;
use crate::sqlite::{RowExt, query_all, query_one};
use crate::util::{enum_str, now, ts};
use chrono::{DateTime, Utc};
use ostra_core::api::{SessionStatus, SessionSummary};
use ostra_core::event::SessionKind;
use ostra_core::ids::{SessionId, WorkspaceId};
use ostra_core::pipeline::{Category, Lane};
use rusqlite::{Connection, OptionalExtension, Row, ToSql, named_params, params};

/// A new session row.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub id: SessionId,
    pub kind: SessionKind,
    pub request: String,
    pub category: Option<Category>,
    pub projects: Vec<String>,
    pub yolo: bool,
}

/// Fields the engine updates on a session. `None` leaves a field unchanged.
#[derive(Debug, Clone, Default)]
pub struct SessionUpdate {
    pub request: Option<String>,
    pub category: Option<Option<Category>>,
    pub status: Option<SessionStatus>,
    pub lane: Option<Lane>,
    pub stage_label: Option<String>,
    pub projects: Option<Vec<String>>,
    pub yolo: Option<bool>,
    pub cost_usd: Option<f64>,
    pub title: Option<Option<String>>,
}

const COLS: &str = "id, kind, request, category, status, lane, stage_label, projects, yolo, cost_usd, \
     created_at, updated_at, title, \
     (SELECT count(*) FROM gates g WHERE g.session_id = sessions.id AND g.answer IS NULL) AS open_gates";

pub(crate) struct Sessions<'c>(pub &'c Connection);

impl Sessions<'_> {
    pub fn insert(&self, new: &NewSession) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO sessions (id, kind, request, category, status, lane, stage_label, projects, yolo,
             cost_usd, created_at, updated_at)
             VALUES (:id, :kind, :request, :category, :status, :lane, 'Intake', :projects, :yolo, 0, :at, :at)",
            named_params! {
                ":id": new.id.as_str(),
                ":kind": serde_json::to_string(&new.kind)?,
                ":request": new.request,
                ":category": new.category.map(|c| enum_str(&c)).transpose()?,
                ":status": enum_str(&SessionStatus::Running)?,
                ":lane": enum_str(&Lane::Research)?,
                ":projects": serde_json::to_string(&new.projects)?,
                ":yolo": new.yolo,
                ":at": now(),
            },
        )?;
        Ok(())
    }

    /// Run it in a transaction, because it writes one column at a time.
    pub fn update(&self, id: &SessionId, update: &SessionUpdate) -> Result<(), StoreError> {
        if !self.exists(id)? {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let set = |col: &str, value: &dyn ToSql| -> Result<(), StoreError> {
            self.0.execute(
                &format!("UPDATE sessions SET {col} = ?1 WHERE id = ?2"),
                params![value, id.as_str()],
            )?;
            Ok(())
        };
        if let Some(v) = &update.request {
            set("request", v)?;
        }
        if let Some(v) = &update.category {
            set("category", &v.map(|c| enum_str(&c)).transpose()?)?;
        }
        if let Some(v) = &update.status {
            set("status", &enum_str(v)?)?;
        }
        if let Some(v) = &update.lane {
            set("lane", &enum_str(v)?)?;
        }
        if let Some(v) = &update.stage_label {
            set("stage_label", v)?;
        }
        if let Some(v) = &update.projects {
            set("projects", &serde_json::to_string(v)?)?;
        }
        if let Some(v) = &update.yolo {
            set("yolo", v)?;
        }
        if let Some(v) = &update.cost_usd {
            set("cost_usd", v)?;
        }
        if let Some(v) = &update.title {
            set("title", v)?;
        }
        set("updated_at", &now())
    }

    pub fn touch(&self, id: &SessionId, at: DateTime<Utc>) -> Result<(), StoreError> {
        self.0.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            params![ts(at), id.as_str()],
        )?;
        Ok(())
    }

    fn exists(&self, id: &SessionId) -> Result<bool, StoreError> {
        Ok(self
            .0
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                [id.as_str()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn get(&self, id: &SessionId) -> Result<Option<SessionSummary>, StoreError> {
        let ws = Meta(self.0).workspace_id()?;
        query_one(
            self.0,
            &format!("SELECT {COLS} FROM sessions WHERE id = ?1"),
            [id.as_str()],
            |r| session(r, &ws),
        )
    }

    /// Newest first.
    pub fn list(&self) -> Result<Vec<SessionSummary>, StoreError> {
        let ws = Meta(self.0).workspace_id()?;
        query_all(
            self.0,
            &format!("SELECT {COLS} FROM sessions ORDER BY created_at DESC, id DESC"),
            [],
            |r| session(r, &ws),
        )
    }
}

fn session(r: &Row<'_>, ws: &WorkspaceId) -> Result<SessionSummary, StoreError> {
    Ok(SessionSummary {
        id: SessionId(r.get("id")?),
        workspace: ws.clone(),
        kind: r.json("kind")?,
        request: r.get("request")?,
        category: r.variant_opt("category")?,
        status: r.variant("status")?,
        lane: r.variant("lane")?,
        stage_label: r.get("stage_label")?,
        yolo: r.get("yolo")?,
        open_gates: r.get("open_gates")?,
        projects: r.json("projects")?,
        cost_usd: r.get("cost_usd")?,
        created_at: r.time("created_at")?,
        updated_at: r.time("updated_at")?,
        title: r.get("title")?,
    })
}
