//! The engine's view of the server: settings, executors, judges, push, protected paths.

use crate::app::Shared;
use ostra_core::config::{GlobalConfig, ResolvedRoute, WorkspaceSettings, load_toml_required};
use ostra_core::exec::{Executor, Usage};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::WorkspaceId;
use ostra_core::model::Effort;
use ostra_core::paths;
use ostra_engine::factory::AgentsFactory;
use ostra_engine::{Notice, Services, SpawnFactory};
use ostra_notify::{Notification, SendOutcome};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ServerServices {
    pub shared: Arc<Shared>,
    pub root: PathBuf,
    pub workspace: WorkspaceId,
}

impl ServerServices {
    fn file_settings(&self) -> WorkspaceSettings {
        load_toml_required(&paths::workspace_toml(&self.root))
            .unwrap_or_else(|_| WorkspaceSettings::seeded("workspace"))
    }

    fn settings(&self) -> WorkspaceSettings {
        ostra_workspace::trust::effective(&self.shared.registry, &self.root, self.file_settings())
    }
}

#[async_trait::async_trait]
impl Services for ServerServices {
    fn global(&self) -> GlobalConfig {
        self.shared.global()
    }

    fn workspace(&self) -> WorkspaceSettings {
        self.settings()
    }

    fn executor(&self, kind: ExecutorKind) -> Option<Arc<dyn Executor>> {
        match kind {
            ExecutorKind::Native => Some(self.shared.native.clone()),
            ExecutorKind::Harness(h) => self.shared.harness.executor(h, &self.shared.global()),
        }
    }

    fn factory(&self) -> Arc<dyn SpawnFactory> {
        Arc::new(AgentsFactory)
    }

    async fn judge(
        &self,
        route: &ResolvedRoute,
        system: &str,
        user: &str,
        schema: Value,
        effort: Effort,
    ) -> Result<(Value, Usage), String> {
        let (provider, model) = self
            .shared
            .providers
            .for_model(&route.model)
            .map_err(|e| e.to_string())?;
        ostra_providers::structured(provider.as_ref(), &model, system, user, schema, effort)
            .await
            .map_err(|e| e.to_string())
    }

    fn notify(&self, notice: Notice) {
        if !self.settings().notifications.push {
            return;
        }
        let shared = self.shared.clone();
        let workspace = self.workspace.clone();
        tokio::spawn(async move {
            let subs = shared
                .registry
                .list_push_subscriptions()
                .unwrap_or_default();
            let url = format!("/w/{workspace}{}", notice.url);
            let payload = Notification {
                title: notice.title,
                body: notice.body,
                url,
                tag: notice.tag,
            };
            for s in subs
                .into_iter()
                .filter(|s| s.workspace.as_ref().is_none_or(|w| *w == workspace))
            {
                match shared.notifier.send(&s.subscription, &payload).await {
                    Ok(SendOutcome::Gone) => {
                        let _ = shared
                            .registry
                            .remove_push_subscription(&s.subscription.endpoint);
                    }
                    Ok(SendOutcome::Failed { status, message }) => {
                        tracing::warn!("push failed ({status:?}): {message}")
                    }
                    Ok(SendOutcome::Delivered) => {}
                    Err(e) => tracing::warn!("push error: {e}"),
                }
            }
        });
    }

    fn protected_paths(&self) -> Vec<PathBuf> {
        let mut v = self.shared.protected_paths();
        v.push(paths::workspace_toml(&self.root));
        v.push(paths::workspace_db(&self.root));
        v
    }

    fn command_approved(&self, project: &std::path::Path, command: &str) -> bool {
        ostra_workspace::trust::format_approved(&self.shared.registry, project, command)
    }

    fn add_allow_rule(&self, rule: &str) {
        let mut s = self.file_settings();
        ostra_workspace::trust::overlay(&self.shared.registry, &self.root, &mut s);
        if !s.permissions.allow.iter().any(|r| r == rule) {
            s.permissions.allow.push(rule.to_string());
            if let Err(e) =
                ostra_workspace::trust::save_workspace(&self.shared.registry, &self.root, &s)
            {
                tracing::warn!("could not save the allow rule: {e}");
            }
        }
    }
}
