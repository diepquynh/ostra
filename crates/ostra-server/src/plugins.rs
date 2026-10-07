//! Plugins wired into the server (HANDOVER 10.10): the plugins built into this binary, and each
//! workspace's plugin programs from `[[plugins]]`, which start only while the workspace's folder
//! file is approved (Rule A1) and restart when their entry changes.

use ostra_agents::{AgentDef, AgentOrigin};
use ostra_core::AgentName;
use ostra_core::config::{ValidationIssue, WorkspaceSettings};
use ostra_core::exec::{
    CancellationToken, ExecutionHost, ExecutionResult, ExecutionSpec, Executor,
};
use ostra_core::plugin::{Plugin, PluginConfig, PluginManifest};
use ostra_sdk::Registry;
use ostra_sdk::stdio::StdioPlugin;
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A workspace plugin program, started or failed, for the entry it was started from.
struct Started {
    entry: PluginConfig,
    result: Result<Arc<StdioPlugin>, String>,
}

pub struct PluginHost {
    builtin: Registry,
    started: Mutex<BTreeMap<(PathBuf, String), Started>>,
    starting: Mutex<HashSet<(PathBuf, String)>>,
}

fn key(root: &Path, name: &str) -> (PathBuf, String) {
    (ostra_core::paths::fold(root), name.to_string())
}

impl PluginHost {
    pub fn new(builtin: Registry) -> Self {
        PluginHost {
            builtin,
            started: Mutex::new(BTreeMap::new()),
            starting: Mutex::new(HashSet::new()),
        }
    }

    /// The `[[plugins]]` entries that may run: enabled, and the file approved (`effective`
    /// disables every entry of a file that waits for approval).
    fn wanted(settings: &WorkspaceSettings) -> Vec<PluginConfig> {
        settings
            .plugins
            .iter()
            .filter(|p| p.enabled && ostra_core::plugin::valid_plugin_name(&p.name))
            .cloned()
            .collect()
    }

    /// Start every wanted plugin program that is not running with its current entry, and stop the
    /// ones no longer wanted.
    pub async fn prepare(&self, root: &Path, settings: &WorkspaceSettings) {
        let wanted = Self::wanted(settings);
        let folded = ostra_core::paths::fold(root);
        self.started.lock().retain(|(r, name), s| {
            *r != folded
                || wanted.iter().any(|w| {
                    &w.name == name
                        && *w == s.entry
                        && s.result.as_ref().is_ok_and(|p| p.is_alive())
                })
        });
        for entry in wanted {
            let k = key(root, &entry.name);
            if self.started.lock().contains_key(&k) || !self.starting.lock().insert(k.clone()) {
                continue;
            }
            let env: Vec<(String, String)> = entry
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let result = StdioPlugin::start(
                &entry.command,
                &env,
                root,
                Duration::from_secs(entry.timeout_secs.max(1)),
            )
            .await
            .and_then(|p| {
                if p.manifest().name == entry.name {
                    Ok(Arc::new(p))
                } else {
                    Err(format!(
                        "it calls itself `{}` in its manifest; name the entry the same",
                        p.manifest().name
                    ))
                }
            });
            if let Err(e) = &result {
                tracing::warn!("plugin {} did not start: {e}", entry.name);
            }
            self.starting.lock().remove(&k);
            self.started.lock().insert(k, Started { entry, result });
        }
    }

    /// Start what [`PluginHost::prepare`] would, in the background.
    pub fn prepare_soon(self: &Arc<Self>, root: &Path, settings: WorkspaceSettings) {
        // A program whose entry was removed, disabled, or now waits for approval stops at once.
        let wanted = Self::wanted(&settings);
        let folded = ostra_core::paths::fold(root);
        self.started.lock().retain(|(r, name), s| {
            *r != folded || wanted.iter().any(|w| &w.name == name && *w == s.entry)
        });
        let needs = Self::wanted(&settings).iter().any(|w| {
            let k = key(root, &w.name);
            !self.starting.lock().contains(&k)
                && !self.started.lock().get(&k).is_some_and(|s| s.entry == *w)
        });
        if needs {
            let me = self.clone();
            let root = root.to_path_buf();
            tokio::spawn(async move { me.prepare(&root, &settings).await });
        }
    }

    /// The plugins workspace `root` has now: built in, then its running programs.
    pub fn plugins(&self, root: &Path) -> Vec<(String, Arc<dyn Plugin>)> {
        let folded = ostra_core::paths::fold(root);
        let mut out: Vec<(String, Arc<dyn Plugin>)> = self
            .builtin
            .all()
            .map(|(n, p)| (n.clone(), p.clone()))
            .collect();
        for ((r, name), s) in self.started.lock().iter() {
            if *r == folded
                && let Ok(p) = &s.result
                && p.is_alive()
                && !out.iter().any(|(n, _)| n == name)
            {
                out.push((name.clone(), p.clone() as Arc<dyn Plugin>));
            }
        }
        out
    }

    pub fn get(&self, root: &Path, name: &str) -> Option<Arc<dyn Plugin>> {
        self.plugins(root)
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p)
    }

    /// Why plugin program `name` did not start, when it did not.
    fn start_error(&self, root: &Path, name: &str) -> Option<String> {
        match self.started.lock().get(&key(root, name)) {
            Some(Started { result: Err(e), .. }) => Some(e.clone()),
            _ => None,
        }
    }

    /// Rule PL2: the plugin that defines agent `agent`.
    pub fn owner_of(&self, root: &Path, agent: AgentName) -> Option<Arc<dyn Plugin>> {
        self.plugins(root)
            .into_iter()
            .find(|(_, p)| p.manifest().agents.iter().any(|a| a.name == agent.as_str()))
            .map(|(_, p)| p)
    }

    /// Rule PL2: the agents the workspace's plugins define.
    pub fn agent_defs(&self, root: &Path) -> Vec<AgentDef> {
        self.plugins(root)
            .into_iter()
            .flat_map(|(name, p)| {
                p.manifest().agents.into_iter().filter_map(move |a| {
                    // A plugin agent the catalog refuses is left out; its checks report why.
                    ostra_agents::catalog::from_plugin_agent(a, AgentOrigin::Plugin(name.clone()))
                        .ok()
                })
            })
            .collect()
    }

    /// Rule PL5: the result contracts the workspace's plugins define, with their schemas. A
    /// contract whose name is not kebab-case is left out.
    pub fn contracts(&self, root: &Path) -> Vec<(ostra_core::Contract, serde_json::Value)> {
        self.plugins(root)
            .into_iter()
            .flat_map(|(name, p)| {
                p.manifest().contracts.into_iter().filter_map(move |c| {
                    ostra_core::contract::PluginContract::new(&name, &c.name)
                        .ok()
                        .map(|k| (ostra_core::Contract::Plugin(k), c.schema))
                })
            })
            .collect()
    }

    /// Rule PL3: the stages the workspace's plugins serve, as `(plugin, stage)`.
    pub fn stages(&self, root: &Path) -> Vec<(String, String)> {
        self.plugins(root)
            .into_iter()
            .flat_map(|(name, p)| {
                p.manifest()
                    .stages
                    .into_iter()
                    .map(move |s| (name.clone(), s.name))
            })
            .collect()
    }

    /// Rule PL1: wanted plugin programs that are not running, as settings issues.
    pub fn issues(&self, root: &Path, settings: &WorkspaceSettings) -> Vec<ValidationIssue> {
        let started = self.started.lock();
        Self::wanted(settings)
            .into_iter()
            .filter_map(|w| {
                let i = settings.plugins.iter().position(|p| p.name == w.name)?;
                let k = key(root, &w.name);
                let message = match started.get(&k) {
                    Some(Started { result: Err(e), .. }) => {
                        format!("Plugin `{}` did not start: {e}.", w.name)
                    }
                    Some(Started { result: Ok(p), .. }) if !p.is_alive() => {
                        let tail = p.stderr_tail();
                        format!(
                            "Plugin `{}` stopped.{}",
                            w.name,
                            if tail.trim().is_empty() {
                                String::new()
                            } else {
                                format!(" Its stderr: {}", tail.trim())
                            }
                        )
                    }
                    _ => return None,
                };
                Some(ValidationIssue {
                    path: format!("plugins[{i}]"),
                    message,
                })
            })
            .collect()
    }

    /// Rule AG3: the built-in plugins and every `[[plugins]]` entry of `file`, with what each is
    /// doing. Env values never leave the server.
    pub fn infos(
        &self,
        root: &Path,
        file: &WorkspaceSettings,
        approved: bool,
    ) -> Vec<ostra_core::api::PluginInfo> {
        use ostra_core::api::{PluginInfo, PluginState};
        let mut out: Vec<PluginInfo> = self
            .builtin
            .all()
            .map(|(n, p)| PluginInfo {
                name: n.clone(),
                builtin: true,
                config: None,
                state: PluginState::Running,
                error: None,
                manifest: Some(p.manifest()),
            })
            .collect();
        let started = self.started.lock();
        for entry in &file.plugins {
            let k = key(root, &entry.name);
            let mut config = entry.clone();
            for v in config.env.values_mut() {
                *v = ostra_core::mcp::SAVED_SECRET.to_string();
            }
            let (state, error, manifest) = if !entry.enabled {
                (PluginState::Disabled, None, None)
            } else if !approved {
                (PluginState::WaitingApproval, None, None)
            } else {
                match started.get(&k) {
                    Some(Started { result: Ok(p), .. }) if p.is_alive() => {
                        (PluginState::Running, None, Some(p.manifest()))
                    }
                    Some(Started { result: Ok(p), .. }) => (
                        PluginState::Failed,
                        Some(
                            format!("It stopped. {}", p.stderr_tail().trim())
                                .trim()
                                .to_string(),
                        ),
                        None,
                    ),
                    Some(Started { result: Err(e), .. }) => {
                        (PluginState::Failed, Some(e.clone()), None)
                    }
                    None => (PluginState::Starting, None, None),
                }
            };
            out.push(PluginInfo {
                name: entry.name.clone(),
                builtin: false,
                config: Some(config),
                state,
                error,
                manifest,
            });
        }
        out
    }

    pub fn manifests(&self, root: &Path) -> Vec<PluginManifest> {
        self.plugins(root)
            .into_iter()
            .map(|(_, p)| p.manifest())
            .collect()
    }
}

/// Rule PL2: runs a programmatic agent in its plugin, through the native executor's tool path.
pub struct ProgramExecutor {
    pub shared: Arc<crate::app::Shared>,
    pub root: PathBuf,
}

#[async_trait::async_trait]
impl Executor for ProgramExecutor {
    async fn run(
        &self,
        spec: ExecutionSpec,
        host: Arc<dyn ExecutionHost>,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        let settings = self.shared.workspace_settings(&self.root);
        self.shared.plugins.prepare(&self.root, &settings).await;
        match self.shared.plugins.owner_of(&self.root, spec.agent) {
            Some(plugin) => {
                let restart = Arc::new(Restart {
                    shared: self.shared.clone(),
                    root: self.root.clone(),
                    name: plugin.manifest().name,
                });
                self.shared
                    .native
                    .run_program(spec, host, cancel, plugin, restart)
                    .await
            }
            None => ExecutionResult::error(format!(
                "No plugin of this workspace runs agent `{}` now. Check the plugin in Settings.",
                spec.agent
            )),
        }
    }
}

/// Rule PL8: starts a workspace's plugin program again after it stopped during a run.
struct Restart {
    shared: Arc<crate::app::Shared>,
    root: PathBuf,
    name: String,
}

#[async_trait::async_trait]
impl ostra_exec_native::PluginRestart for Restart {
    async fn restart(&self) -> Result<Arc<dyn Plugin>, String> {
        let settings = self.shared.workspace_settings(&self.root);
        self.shared.plugins.prepare(&self.root, &settings).await;
        match self.shared.plugins.get(&self.root, &self.name) {
            Some(p) if p.is_alive() => Ok(p),
            _ => Err(self
                .shared
                .plugins
                .start_error(&self.root, &self.name)
                .unwrap_or_else(|| format!("plugin `{}` is not running", self.name))),
        }
    }
}
