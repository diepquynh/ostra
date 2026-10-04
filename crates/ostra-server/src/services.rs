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

    /// Rule PL8: call plugin `plugin`, and call it again in a fresh program when its program
    /// stopped during the call, at most `PLUGIN_RESTARTS` times. What the plugin saved before it
    /// stopped reaches the next call through its checkpoints.
    async fn call_plugin<T, F, Fut>(&self, plugin: &str, f: F) -> Result<T, String>
    where
        F: Fn(Arc<dyn ostra_core::plugin::Plugin>) -> Fut,
        Fut: std::future::Future<Output = Result<T, String>>,
    {
        let mut restarts = 0;
        loop {
            self.prepare_plugins().await;
            let p = self
                .shared
                .plugins
                .get(&self.root, plugin)
                .ok_or_else(|| format!("plugin `{plugin}` is not running"))?;
            match f(p.clone()).await {
                Err(e) if !p.is_alive() && restarts < ostra_core::plugin::PLUGIN_RESTARTS => {
                    restarts += 1;
                    tracing::warn!("plugin {plugin} stopped during a call; starting it again: {e}");
                }
                other => return other,
            }
        }
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

    fn agents(&self) -> ostra_agents::AgentCatalog {
        ostra_workspace::WorkspaceHost::agents(self.shared.as_ref(), &self.root).0
    }

    fn workflows(&self) -> ostra_core::workflow::WorkflowSet {
        // Rule A1: the workspace's workflow files run only while its folder file is approved.
        let mut set =
            if ostra_workspace::trust::definitions_approved(&self.shared.registry, &self.root) {
                ostra_core::workflow::WorkflowSet::load(&self.root).0
            } else {
                ostra_core::workflow::WorkflowSet::default()
            };
        // Rule PL6: a plugin's workflows and transforms come with it; a plugin program starts only
        // while the workspace file is approved (Rule PL1).
        for (name, p) in self.shared.plugins.plugins(&self.root) {
            set.add_plugin(&name, &p.manifest());
        }
        set
    }

    async fn plugin_transform(
        &self,
        plugin: &str,
        name: &str,
        inputs: serde_json::Map<String, serde_json::Value>,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        self.call_plugin(plugin, |p| {
            let (inputs, args) = (inputs.clone(), args.clone());
            async move { p.transform(name, inputs, args).await }
        })
        .await
    }

    fn plugin_stages(&self) -> Vec<(String, String)> {
        ostra_workspace::WorkspaceHost::plugin_stages(self.shared.as_ref(), &self.root)
    }

    async fn prepare_plugins(&self) {
        let settings = self.shared.workspace_settings(&self.root);
        self.shared.plugins.prepare(&self.root, &settings).await;
    }

    async fn handle_result(
        &self,
        plugin: &str,
        result: ostra_core::plugin::ResultView,
        checkpoints: Arc<dyn ostra_core::plugin::Checkpoints>,
    ) -> Result<ostra_core::submit::CustomSubmit, String> {
        self.call_plugin(plugin, |p| {
            let (result, checkpoints) = (result.clone(), checkpoints.clone());
            async move {
                let contract = result.contract.clone();
                p.handle_result(&contract, result, checkpoints).await
            }
        })
        .await
    }

    async fn decide_stage(
        &self,
        plugin: &str,
        stage: &str,
        view: ostra_core::plugin::StageView,
        checkpoints: Arc<dyn ostra_core::plugin::Checkpoints>,
    ) -> Result<ostra_core::plugin::StageDecision, String> {
        self.call_plugin(plugin, |p| {
            let (view, checkpoints) = (view.clone(), checkpoints.clone());
            async move { p.decide_stage(stage, view, checkpoints).await }
        })
        .await
    }

    fn program_executor(&self, _agent: ostra_core::AgentName) -> Option<Arc<dyn Executor>> {
        Some(Arc::new(crate::plugins::ProgramExecutor {
            shared: self.shared.clone(),
            root: self.root.clone(),
        }))
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
        v.push(paths::workspace_agents_dir(&self.root));
        v.push(paths::workspace_workflows_dir(&self.root));
        v.push(paths::workspace_transforms_dir(&self.root));
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
