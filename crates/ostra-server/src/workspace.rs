//! One workspace: its settings, database, engine, and projects.

use crate::app::Shared;
use crate::services::ServerServices;
use ostra_core::agent::JUDGE_ROUTE;
use ostra_core::api::{ImportProject, InitStatus, ProjectView, ProviderStatus, WorkspaceDetail};
use ostra_core::config::{
    Environment, ProjectEntry, ProjectProfile, ValidationIssue, WorkspaceSettings, load_toml, load_toml_required,
    save_toml, validate_workspace,
};
use ostra_core::ids::WorkspaceId;
use ostra_core::model::Tier;
use ostra_core::paths;
use ostra_core::slug::is_project_key;
use ostra_engine::Engine;
use ostra_store::{ProjectRow, WorkspaceDb};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct WorkspaceRt {
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub db: WorkspaceDb,
    pub engine: Engine,
    pub shared: Arc<Shared>,
}

pub fn default_tier(key: &str) -> Tier {
    if key == JUDGE_ROUTE {
        return Tier::Fast;
    }
    key.parse().map(|a| ostra_agents::agent_def(a).default_tier).unwrap_or(Tier::Balanced)
}

fn ultracode_bootstrap(path: &Path) -> bool {
    let uc = path.join(".ultracode");
    uc.join("INVENTORY.md").exists() && uc.join("repo-profile.json").exists()
}

impl WorkspaceRt {
    pub fn open(shared: Arc<Shared>, id: WorkspaceId, root: &Path) -> anyhow::Result<Self> {
        let db = WorkspaceDb::open(&paths::workspace_db(root))?;
        db.set_workspace_id(&id)?;
        let services = Arc::new(ServerServices { shared: shared.clone(), root: root.to_path_buf(), workspace: id.clone() });
        let engine = Engine::new(root.to_path_buf(), id.clone(), db.clone(), services);
        let rt = WorkspaceRt { id, root: root.to_path_buf(), db, engine, shared };
        rt.sync_projects();
        Ok(rt)
    }

    pub fn settings(&self) -> WorkspaceSettings {
        load_toml_required(&paths::workspace_toml(&self.root)).unwrap_or_else(|e| {
            tracing::warn!("{e}");
            WorkspaceSettings::seeded("workspace")
        })
    }

    pub fn environment(&self) -> Environment {
        Environment {
            installed_harnesses: self.shared.env.read().installed(),
            providers_with_keys: self.shared.providers.status().into_iter().filter(|p| p.has_key).map(|p| p.name).collect(),
        }
    }

    /// Settings validation: routes, harness availability, keys, projects, and permission rules.
    pub fn validate(&self, settings: &WorkspaceSettings) -> Vec<ValidationIssue> {
        let global = self.shared.global();
        let mut issues = validate_workspace(&global, settings, &self.environment(), default_tier);
        let lists = [("allow", &settings.permissions.allow), ("ask", &settings.permissions.ask), ("deny", &settings.permissions.deny)];
        for (name, list) in lists {
            for (i, rule) in list.iter().enumerate() {
                if let Err(e) = ostra_policy::validate_rule(rule) {
                    issues.push(ValidationIssue { path: format!("permissions.{name}[{i}]"), message: e });
                }
            }
        }
        for (i, p) in settings.projects.iter().enumerate() {
            if !p.path.is_dir() {
                issues.push(ValidationIssue { path: format!("projects[{i}].path"), message: format!("{} is not a folder.", p.path.display()) });
            }
        }
        issues
    }

    pub fn save_settings(&self, settings: &WorkspaceSettings) -> Result<(), Vec<ValidationIssue>> {
        let issues = self.validate(settings);
        if !issues.is_empty() {
            return Err(issues);
        }
        save_toml(&paths::workspace_toml(&self.root), settings)
            .map_err(|e| vec![ValidationIssue { path: String::new(), message: e.to_string() }])?;
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
            let status = if paths::project_inventory(&p.path).exists() { InitStatus::Initialized } else { InitStatus::NotInitialized };
            let _ = self.db.upsert_project(&ProjectRow { key: p.key.clone(), path: p.path.clone(), init_status: status, stack: p.stack.clone() });
        }
    }

    pub fn project_views(&self) -> Vec<ProjectView> {
        let settings = self.settings();
        let initializing: Vec<String> = self
            .db
            .list_sessions()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| matches!(s.status, ostra_core::api::SessionStatus::Running | ostra_core::api::SessionStatus::Waiting))
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
                let profile: Option<ProjectProfile> =
                    paths::project_profile(&p.path).exists().then(|| load_toml(&paths::project_profile(&p.path)).ok()).flatten();
                ProjectView {
                    key: p.key.clone(),
                    path: p.path.clone(),
                    init_status,
                    ultracode_bootstrap: exists && ultracode_bootstrap(&p.path),
                    is_git: p.path.join(".git").exists(),
                    stack: p.stack.clone().or_else(|| profile.as_ref().and_then(|pr| pr.stack.language.clone())),
                    profile,
                }
            })
            .collect()
    }

    pub fn import_project(&self, req: &ImportProject) -> Result<(), String> {
        let path = req.path.clone();
        if !path.is_absolute() {
            return Err("Use an absolute path.".into());
        }
        let path = std::fs::canonicalize(&path).map_err(|_| format!("{} does not exist.", path.display()))?;
        if !path.is_dir() {
            return Err(format!("{} is not a folder.", path.display()));
        }
        if !is_project_key(&req.key) {
            return Err(format!("`{}` is not a project key. Use lowercase letters, digits, and dashes.", req.key));
        }
        let mut settings = self.settings();
        if settings.project(&req.key).is_some() {
            return Err(format!("A project named `{}` already exists in this workspace.", req.key));
        }
        if let Some(p) = settings.projects.iter().find(|p| p.path == path) {
            return Err(format!("That folder is already imported as `{}`.", p.key));
        }
        if paths::is_inside(&self.root.join(paths::RUNTIME_DIR), &path) {
            return Err("A project cannot live inside the workspace's .ostra directory.".into());
        }
        settings.projects.push(ProjectEntry { key: req.key.clone(), path, stack: req.stack.clone().filter(|s| !s.trim().is_empty()) });
        save_toml(&paths::workspace_toml(&self.root), &settings).map_err(|e| e.to_string())?;
        self.sync_projects();
        Ok(())
    }

    /// Removing a project from a workspace deletes nothing on disk.
    pub fn remove_project(&self, key: &str) -> Result<(), String> {
        let mut settings = self.settings();
        let before = settings.projects.len();
        settings.projects.retain(|p| p.key != key);
        if settings.projects.len() == before {
            return Err(format!("No project `{key}`."));
        }
        save_toml(&paths::workspace_toml(&self.root), &settings).map_err(|e| e.to_string())?;
        self.sync_projects();
        Ok(())
    }

    pub fn project_path(&self, key: &str) -> Option<PathBuf> {
        self.settings().project(key).map(|p| p.path.clone())
    }

    pub fn detail(&self) -> WorkspaceDetail {
        let settings = self.settings();
        let validation = self.validate(&settings);
        let providers: Vec<ProviderStatus> = self.shared.providers.status();
        WorkspaceDetail {
            id: self.id.clone(),
            root: self.root.clone(),
            projects: self.project_views(),
            harnesses: self.shared.env.read().harnesses.clone(),
            providers,
            validation,
            settings,
        }
    }
}
