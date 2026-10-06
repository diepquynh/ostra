//! One open workspace: its settings, database, engine, and projects.

use crate::create::CreateError;
use crate::host::WorkspaceHost;
use crate::projects::{git_branch, import_entry, ultracode_bootstrap};
use crate::settings::{agent_infos, field_issue, validate_settings};
use ostra_core::api::{ImportProject, InitStatus, ProjectView, SessionStatus, WorkspaceDetail};
use ostra_core::config::{
    ConfigError, Environment, ProjectProfile, ValidationIssue, WorkspaceSettings, load_toml,
    load_toml_required,
};
use ostra_core::ids::WorkspaceId;
use ostra_core::paths;
use ostra_engine::Engine;
use ostra_store::{ProjectRow, RegistryDb, WorkspaceDb};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A session that has not been scoped yet may still pick any project it started with.
fn may_use(st: &ostra_engine::SessionState, key: &str) -> bool {
    if !st.created {
        return true;
    }
    if st.scope.is_empty() {
        st.projects.iter().any(|p| p.key == key)
    } else {
        st.scope.iter().any(|p| p == key)
    }
}

pub const STARTING: &str =
    "Wait a moment and try again, because Ostra is starting work in this workspace.";

pub enum RemoveProjectError {
    NotFound(String),
    Busy(String),
    Failed(String),
}

#[derive(Debug)]
pub enum DeleteError {
    NotFound(String),
    Busy(String),
    Store(ostra_store::StoreError),
}

impl From<ostra_store::StoreError> for DeleteError {
    fn from(e: ostra_store::StoreError) -> Self {
        DeleteError::Store(e)
    }
}

/// Unregister a workspace and delete `workspace.toml` and `workspace.db`. Project folders,
/// session artifact folders, and per-project `.ostra/` files stay on disk.
pub fn unregister(
    registry: &RegistryDb,
    id: &WorkspaceId,
    root: &Path,
) -> Result<(), ostra_store::StoreError> {
    registry.remove_workspace(id)?;
    crate::trust::forget(registry, root);
    let db = paths::workspace_db(root);
    let mut files = vec![paths::workspace_toml(root), db.clone()];
    files.extend(["-wal", "-shm"].map(|s| {
        let mut p = db.clone().into_os_string();
        p.push(s);
        PathBuf::from(p)
    }));
    for f in files {
        match std::fs::remove_file(&f) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("deleting {}: {e}", f.display()),
        }
    }
    Ok(())
}

pub struct WorkspaceRt {
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub db: WorkspaceDb,
    pub engine: Engine,
    pub host: Arc<dyn WorkspaceHost>,
    /// Project keys and folders with a clone or pull in progress.
    pub cloning: parking_lot::Mutex<Vec<(String, PathBuf)>>,
    /// Held shared while a request starts work and exclusively while a project or the workspace
    /// is removed, so no work starts between the busy check and the removal. True once deleted.
    pub work: tokio::sync::RwLock<bool>,
}

impl WorkspaceRt {
    pub fn open(
        host: Arc<dyn WorkspaceHost>,
        id: WorkspaceId,
        root: &Path,
    ) -> Result<Self, ostra_store::StoreError> {
        crate::trust::migrate(host.registry(), root);
        crate::trust::seal_file_secrets(host.registry(), root);
        let db = WorkspaceDb::open(&paths::workspace_db(root))?;
        db.set_workspace_id(&id)?;
        let services = host.clone().services(&id, root);
        ostra_default_plugin::install();
        let engine = Engine::new(root.to_path_buf(), id.clone(), db.clone(), services);
        let rt = WorkspaceRt {
            id,
            root: root.to_path_buf(),
            db,
            engine,
            host,
            cloning: Default::default(),
            work: Default::default(),
        };
        rt.sync_projects();
        Ok(rt)
    }

    /// The settings as the user edits them: the file, with the mode and YOLO from the registry.
    pub fn settings(&self) -> WorkspaceSettings {
        let mut s = self.file_settings();
        crate::trust::overlay(self.host.registry(), &self.root, &mut s);
        s
    }

    fn file_settings(&self) -> WorkspaceSettings {
        load_toml_required(&paths::workspace_toml(&self.root)).unwrap_or_else(|e| {
            tracing::warn!("{e}");
            WorkspaceSettings::seeded("workspace")
        })
    }

    /// The settings programs start from, without commands that wait for approval.
    pub fn effective_settings(&self) -> WorkspaceSettings {
        crate::trust::effective(self.host.registry(), &self.root, self.file_settings())
    }

    fn write_settings(&self, settings: &WorkspaceSettings) -> Result<(), ConfigError> {
        crate::trust::save_workspace(self.host.registry(), &self.root, settings)
    }

    pub fn environment(&self) -> Environment {
        let mut env = self.host.environment();
        env.agents = crate::settings::agent_routes(&self.host.agents(&self.root).0);
        env
    }

    /// Rule CA1: problems in the workspace's custom agent definitions, as settings issues.
    fn agent_issues(&self) -> Vec<ValidationIssue> {
        self.host
            .agents(&self.root)
            .1
            .into_iter()
            .map(|i| ValidationIssue {
                path: "agents".into(),
                message: format!("{}: {}", i.source, i.message),
            })
            .collect()
    }

    /// Rule WF1: every workflow file resolves and names only agents and plugin stages the
    /// workspace has, and no category has two default workflows.
    fn workflow_issues(&self) -> Vec<ValidationIssue> {
        let (set, unreadable) = self.workflow_set();
        let issue = |path: String, message: String| ValidationIssue { path, message };
        let mut out: Vec<ValidationIssue> = unreadable
            .into_iter()
            .map(|(name, m)| issue(format!("workflows.{name}"), m))
            .collect();
        // Rule WB7: every composite function file parses and checks.
        let (functions, unreadable) = ostra_core::workflow::WorkflowSet::load_functions(&self.root);
        out.extend(
            unreadable
                .into_iter()
                .map(|(name, m)| issue(format!("transforms.{name}"), m)),
        );
        for (name, f) in &functions {
            let problems = ostra_core::transform::check_function(name, f, &functions);
            if !problems.is_empty() {
                out.push(issue(format!("transforms.{name}"), problems.join(" ")));
            }
        }
        let agents = self.host.agents(&self.root).0;
        let plugin_stages = self.host.plugin_stages(&self.root);
        for name in set.files.keys() {
            let checked = set.resolve(name).and_then(|wf| {
                ostra_engine::workflow::check_runnable(&wf, &agents, &plugin_stages)
            });
            if let Err(m) = checked {
                out.push(issue(format!("workflows.{name}"), m));
            }
        }
        // Rule PL6: a plugin's workflow that cannot run is the plugin's problem to report.
        for name in set.plugin_files.keys() {
            let checked = set.resolve(name).and_then(|wf| {
                ostra_engine::workflow::check_runnable(&wf, &agents, &plugin_stages)
            });
            if let Err(m) = checked {
                out.push(issue(
                    "plugins".into(),
                    format!("Plugin workflow `{name}` cannot run: {m}"),
                ));
            }
        }
        for c in ostra_core::workflow::BUILTIN_BASES {
            if let Err(m) = set.default_for(c) {
                out.push(issue("workflows".into(), m));
            }
        }
        out
    }

    /// Rule WF1: the workflows a session can run: the workspace's valid ones, and Ostra's defaults
    /// it has no copy of (Rule WF9).
    pub fn workflows(&self) -> Vec<ostra_core::workflow::WorkflowInfo> {
        use ostra_core::workflow::{BUILTIN_BASES, WorkflowInfo, category_name, parse_category};
        let (set, _) = self.workflow_set();
        let mut out: Vec<WorkflowInfo> = BUILTIN_BASES
            .iter()
            .filter(|c| !set.files.contains_key(&category_name(**c)))
            .map(|c| {
                let wf = ostra_default_plugin::workflow(*c);
                WorkflowInfo {
                    name: wf.name,
                    description: wf.description,
                    base: *c,
                    default_for: vec![],
                    builtin: true,
                    plugin: None,
                    stages: wf.stages,
                }
            })
            .collect();
        for (name, file) in &set.files {
            if let Ok(wf) = set.resolve(name) {
                out.push(WorkflowInfo {
                    name: wf.name,
                    description: wf.description,
                    base: wf.base,
                    default_for: file
                        .default_for
                        .iter()
                        .filter_map(|c| parse_category(c))
                        .collect(),
                    builtin: false,
                    plugin: None,
                    stages: wf.stages,
                });
            }
        }
        // Rule PL6: the workflows plugins build, which a session names in full.
        for name in set.plugin_files.keys() {
            if let Ok(wf) = set.resolve(name) {
                out.push(WorkflowInfo {
                    plugin: name.split_once(':').map(|(p, _)| p.to_string()),
                    name: wf.name,
                    description: wf.description,
                    base: wf.base,
                    default_for: vec![],
                    builtin: false,
                    stages: wf.stages,
                });
            }
        }
        out
    }

    /// Rules WF1, WB7, PL6, and PL7: the workspace's workflows and transform functions, with those
    /// of the plugins that run for it.
    pub fn workflow_set(&self) -> (ostra_core::workflow::WorkflowSet, Vec<(String, String)>) {
        let (mut set, issues) = ostra_core::workflow::WorkflowSet::load(&self.root);
        ostra_default_plugin::add_workflows(&mut set);
        for (name, m) in self.host.plugin_manifests(&self.root) {
            set.add_plugin(&name, &m);
        }
        (set, issues)
    }

    pub fn validate(&self, settings: &WorkspaceSettings) -> Vec<ValidationIssue> {
        let mut issues = validate_settings(&self.host.global(), &self.environment(), settings);
        issues.extend(self.agent_issues());
        issues.extend(self.workflow_issues());
        issues.extend(self.host.plugin_issues(&self.root, settings));
        // Rule W3: a tag in an instruction names an artifact every agent can read.
        let texts = settings
            .instructions
            .all
            .iter()
            .map(|t| ("instructions.all".to_string(), t))
            .chain(
                settings
                    .instructions
                    .agents
                    .iter()
                    .map(|(a, t)| (format!("instructions.agents.{a}"), t)),
            );
        for (path, text) in texts {
            for message in ostra_core::artifacts::tag_issues(text, &self.root, self.id.as_str()) {
                issues.push(field_issue(&path, message));
            }
        }
        issues
    }

    /// Apply every fix [`crate::settings::settings_fixes`] finds in the saved settings, without
    /// validating the rest, because an unrelated problem must not block a fix. Returns how many.
    pub fn apply_fixes(&self) -> Result<usize, ConfigError> {
        let mut settings = self.settings();
        let fixes = crate::settings::settings_fixes(&settings, &self.environment().agents);
        for f in &fixes {
            if let Some(key) = f.path.strip_prefix("routing.model.byAgent.") {
                settings
                    .routing
                    .model
                    .by_agent
                    .insert(key.to_string(), f.value.as_str().into());
            }
        }
        if !fixes.is_empty() {
            self.write_settings(&settings)?;
        }
        Ok(fixes.len())
    }

    pub fn save_settings(&self, settings: &WorkspaceSettings) -> Result<(), Vec<ValidationIssue>> {
        let issues = self.validate(settings);
        if !issues.is_empty() {
            return Err(issues);
        }
        self.write_settings(settings).map_err(|e| {
            vec![ValidationIssue {
                path: String::new(),
                message: e.to_string(),
            }]
        })?;
        self.sync_projects();
        Ok(())
    }

    fn sync_projects(&self) {
        let settings = self.settings();
        let rows = self.db.list_projects().unwrap_or_default();
        for row in &rows {
            if settings.project(&row.key).is_none() {
                let _ = self.db.delete_project(&row.key);
            }
        }
        for p in &settings.projects {
            let status = if paths::project_inventory(&p.path).exists() {
                InitStatus::Initialized
            } else {
                InitStatus::NotInitialized
            };
            let _ = self.db.upsert_project(&ProjectRow {
                key: p.key.clone(),
                path: p.path.clone(),
                init_status: status,
                stack: p.stack.clone(),
            });
        }
    }

    pub fn project_views(&self) -> Vec<ProjectView> {
        let settings = self.settings();
        let initializing: Vec<String> = self
            .db
            .list_sessions()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| {
                matches!(
                    s.status,
                    ostra_core::api::SessionStatus::Running
                        | ostra_core::api::SessionStatus::Waiting
                        | ostra_core::api::SessionStatus::Paused
                )
            })
            .filter_map(|s| match s.kind {
                ostra_core::event::SessionKind::Init { project } => Some(project),
                _ => None,
            })
            .collect();
        settings
            .projects
            .iter()
            .map(|p| {
                let exists = p.path.is_dir();
                let init_status = if !exists {
                    InitStatus::Missing
                } else if initializing.contains(&p.key) {
                    InitStatus::Initializing
                } else if paths::project_inventory(&p.path).exists() {
                    InitStatus::Initialized
                } else {
                    InitStatus::NotInitialized
                };
                let profile: Option<ProjectProfile> = paths::project_profile(&p.path)
                    .exists()
                    .then(|| load_toml(&paths::project_profile(&p.path)).ok())
                    .flatten();
                ProjectView {
                    key: p.key.clone(),
                    path: p.path.clone(),
                    init_status,
                    ultracode_bootstrap: exists && ultracode_bootstrap(&p.path),
                    is_git: p.path.join(".git").exists(),
                    git_branch: if exists { git_branch(&p.path) } else { None },
                    stack: p
                        .stack
                        .clone()
                        .or_else(|| profile.as_ref().and_then(|pr| pr.stack.clone())),
                    profile,
                }
            })
            .collect()
    }

    /// Add a project. Refusals name the request field they are about: `key`, `path`, or `stack`.
    pub fn import_project(&self, req: &ImportProject) -> Result<(), CreateError> {
        let mut settings = self.settings();
        let entry = import_entry(&settings, &self.root, req).map_err(CreateError::Invalid)?;
        settings.projects.push(entry);
        self.write_settings(&settings)
            .map_err(|e| CreateError::Failed(e.to_string()))?;
        self.sync_projects();
        Ok(())
    }

    /// Removing a project from a workspace deletes nothing on disk.
    pub fn remove_project(&self, key: &str) -> Result<(), RemoveProjectError> {
        let mut settings = self.settings();
        let before = settings.projects.len();
        settings.projects.retain(|p| p.key != key);
        if settings.projects.len() == before {
            return Err(RemoveProjectError::NotFound(format!("No project `{key}`.")));
        }
        let Ok(_work) = self.work.try_write() else {
            return Err(RemoveProjectError::Busy(STARTING.into()));
        };
        if let Some(reason) = self
            .project_busy(key)
            .map_err(|e| RemoveProjectError::Failed(e.to_string()))?
        {
            return Err(RemoveProjectError::Busy(reason));
        }
        self.write_settings(&settings)
            .map_err(|e| RemoveProjectError::Failed(e.to_string()))?;
        self.sync_projects();
        Ok(())
    }

    /// Why a project cannot be removed now: a live session on it, a running execution, or a clone.
    pub fn project_busy(&self, key: &str) -> Result<Option<String>, ostra_store::StoreError> {
        if let Some(s) = self.db.list_sessions()?.into_iter().find(|s| {
            matches!(
                s.status,
                SessionStatus::Running | SessionStatus::Waiting | SessionStatus::Paused
            ) && self.session_may_use(&s.id, key)
        }) {
            return Ok(Some(format!(
                "Stop session {} or wait for it to finish, then try again, because it works on `{key}`.",
                s.id
            )));
        }
        if let Some(e) = self
            .db
            .running_executions()?
            .into_iter()
            .find(|e| e.project == key)
        {
            return Ok(Some(format!(
                "Cancel execution {} or wait for it to finish, then try again, because it is running in `{key}`.",
                e.id
            )));
        }
        if self.cloning.lock().iter().any(|(k, _)| k == key) {
            return Ok(Some(format!(
                "Wait for the git operation on `{key}` to finish, then try again."
            )));
        }
        Ok(None)
    }

    fn session_may_use(&self, id: &ostra_core::ids::SessionId, key: &str) -> bool {
        self.engine.state(id).map_or(true, |st| may_use(&st, key))
    }

    /// Mark the workspace deleted once nothing runs in it, so no request starts work after this.
    pub fn retire(&self) -> Result<(), DeleteError> {
        let Ok(mut deleted) = self.work.try_write() else {
            return Err(DeleteError::Busy(STARTING.into()));
        };
        if let Some(reason) = self.busy()? {
            return Err(DeleteError::Busy(reason));
        }
        *deleted = true;
        Ok(())
    }

    /// Why the workspace cannot be deleted now: a live session, a running execution, or a clone.
    pub fn busy(&self) -> Result<Option<String>, ostra_store::StoreError> {
        if let Some(s) = self.db.list_sessions()?.into_iter().find(|s| {
            matches!(
                s.status,
                SessionStatus::Running | SessionStatus::Waiting | SessionStatus::Paused
            )
        }) {
            return Ok(Some(format!(
                "Stop session {} or wait for it to finish, then try again, because deleting the workspace would lose its state.",
                s.id
            )));
        }
        if let Some(e) = self.db.running_executions()?.into_iter().next() {
            return Ok(Some(format!(
                "Cancel execution {} or wait for it to finish, then try again, because it is still running.",
                e.id
            )));
        }
        if let Some((key, _)) = self.cloning.lock().first() {
            return Ok(Some(format!(
                "Wait for the git operation on `{key}` to finish, then try again."
            )));
        }
        Ok(None)
    }

    pub fn project_path(&self, key: &str) -> Option<PathBuf> {
        self.settings().project(key).map(|p| p.path.clone())
    }

    pub fn detail(&self) -> WorkspaceDetail {
        let settings = self.settings();
        let global = self.host.global();
        let mut validation = validate_settings(&global, &self.environment(), &settings);
        validation.extend(self.agent_issues());
        validation.extend(self.workflow_issues());
        validation.extend(self.host.plugin_issues(&self.root, &settings));
        WorkspaceDetail {
            id: self.id.clone(),
            root: self.root.clone(),
            projects: self.project_views(),
            harnesses: self.host.harnesses(),
            providers: self.host.providers(),
            validation,
            fixes: crate::settings::settings_fixes(&settings, &self.environment().agents),
            agents: agent_infos(&global, &settings, &self.host.agents(&self.root).0),
            stacks: ostra_agents::stack_names(),
            global_permissions: global.permissions.clone(),
            workflows: self.workflows(),
            missing_workflows: ostra_core::workflow::WorkflowSet::load(&self.root)
                .0
                .missing_defaults(),
            pending_commands: crate::trust::pending(self.host.registry(), &self.root, &settings),
            sandbox: ostra_sandbox::status(&global.sandbox.for_workspace(&settings.sandbox())),
            global_sandbox: ostra_core::api::GlobalSandbox {
                network: global.sandbox.network,
                allowed_hosts: global.sandbox.allowed_hosts.clone(),
            },
            global_tool_enforcement: global.tool_enforcement,
            settings: browser_view(settings),
        }
    }
}

/// The settings as the browser may see them: a literal MCP header or env value that reached
/// the file since the last save (a `git pull`, a hand edit) shows as the saved-value marker,
/// because only the connection may read it.
fn browser_view(mut s: WorkspaceSettings) -> WorkspaceSettings {
    for m in &mut s.mcp_servers {
        for v in m.headers.values_mut().chain(m.env.values_mut()) {
            if ostra_core::mcp::is_literal_value(v) {
                *v = ostra_core::mcp::SAVED_SECRET.to_string();
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unscoped_session_may_use_every_project_it_started_with() {
        let mut st = ostra_engine::SessionState::new("s_1".into());
        assert!(
            may_use(&st, "app"),
            "a session with no log is assumed to use anything"
        );
        st.created = true;
        st.projects = ["app", "lib"]
            .map(|k| ostra_core::event::ProjectRef {
                key: k.into(),
                path: PathBuf::from("/x").join(k),
            })
            .to_vec();
        assert!(may_use(&st, "app") && may_use(&st, "lib"));
        assert!(!may_use(&st, "other"));
        st.scope = vec!["lib".into()];
        assert!(!may_use(&st, "app"));
        assert!(may_use(&st, "lib"));
    }

    #[test]
    fn the_browser_never_sees_a_literal_mcp_value() {
        let mut s = WorkspaceSettings::seeded("w");
        let mut m = ostra_core::config::McpServerConfig::local("gh", &["gh-mcp"]);
        m.env.insert("TOKEN".into(), "ghp_pulled".into());
        m.env.insert("FROM_ENV".into(), "${GH_TOKEN}".into());
        s.mcp_servers.push(m);
        let v = browser_view(s);
        assert_eq!(v.mcp_servers[0].env["TOKEN"], ostra_core::mcp::SAVED_SECRET);
        assert_eq!(v.mcp_servers[0].env["FROM_ENV"], "${GH_TOKEN}");
    }
}
