use crate::StoreError;
use crate::sqlite::{RowExt, query_all};
use crate::util::now;
use chrono::{DateTime, Utc};
use ostra_core::api::{PushKeys, PushSubscription};
use ostra_core::ids::WorkspaceId;
use rusqlite::{Connection, named_params};

#[derive(Debug, Clone, PartialEq)]
pub struct StoredPushSubscription {
    pub subscription: PushSubscription,
    pub workspace: Option<WorkspaceId>,
    pub created_at: DateTime<Utc>,
}

pub(crate) struct PushSubscriptions<'c>(pub &'c Connection);

impl PushSubscriptions<'_> {
    /// Adds or refreshes a subscription, keyed by endpoint.
    pub fn upsert(
        &self,
        sub: &PushSubscription,
        workspace: Option<&WorkspaceId>,
    ) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO push_subscriptions (endpoint, p256dh, auth, workspace_id, created_at)
             VALUES (:endpoint, :p256dh, :auth, :workspace, :created_at)
             ON CONFLICT(endpoint) DO UPDATE SET p256dh = excluded.p256dh, auth = excluded.auth,
             workspace_id = excluded.workspace_id",
            named_params! {
                ":endpoint": sub.endpoint,
                ":p256dh": sub.keys.p256dh,
                ":auth": sub.keys.auth,
                ":workspace": workspace.map(WorkspaceId::as_str),
                ":created_at": now(),
            },
        )?;
        Ok(())
    }

    /// Oldest first.
    pub fn list(&self) -> Result<Vec<StoredPushSubscription>, StoreError> {
        query_all(
            self.0,
            "SELECT endpoint, p256dh, auth, workspace_id, created_at FROM push_subscriptions
             ORDER BY created_at, endpoint",
            [],
            |r| {
                Ok(StoredPushSubscription {
                    subscription: PushSubscription {
                        endpoint: r.get("endpoint")?,
                        keys: PushKeys {
                            p256dh: r.get("p256dh")?,
                            auth: r.get("auth")?,
                        },
                    },
                    workspace: r.get::<_, Option<String>>("workspace_id")?.map(WorkspaceId),
                    created_at: r.time("created_at")?,
                })
            },
        )
    }

    pub fn delete(&self, endpoint: &str) -> Result<bool, StoreError> {
        Ok(self.0.execute(
            "DELETE FROM push_subscriptions WHERE endpoint = ?1",
            [endpoint],
        )? > 0)
    }

    pub fn delete_for_workspace(&self, id: &WorkspaceId) -> Result<(), StoreError> {
        self.0.execute(
            "DELETE FROM push_subscriptions WHERE workspace_id = ?1",
            [id.as_str()],
        )?;
        Ok(())
    }
}
