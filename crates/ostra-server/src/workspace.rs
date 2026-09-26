//! One workspace: its settings, database, engine, and projects.

use crate::app::Shared;
use crate::services::ServerServices;
use ostra_core::agent::{AgentName, JUDGE_ROUTE};
use ostra_core::api::{
    AgentInfo, ImportProject, InitStatus, ProjectView, ProviderStatus, SessionStatus,
    WorkspaceDetail,
};
use ostra_core::config::{
    ConfigError, Environment, GlobalConfig, ProjectEntry, ProjectProfile, RouteQuery,
    ValidationIssue, WorkspaceSettings, load_toml, load_toml_required, resolve_executor,
    resolve_route, validate_workspace,
};
use ostra_core::ids::WorkspaceId;
use ostra_core::model::Tier;
use ostra_core::paths;
use ostra_core::slug::{is_project_key, is_stack_name, stack_issue};
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
    /// Project keys and folders with a clone or pull in progress.
    pub cloning: parking_lot::Mutex<Vec<(String, PathBuf)>>,
}

pub fn default_tier(key: &str) -> Tier {
    if key == JUDGE_ROUTE {
        return Tier::Fast;
    }
    key.parse()
        .map(|a| ostra_agents::agent_def(a).default_tier)
        .unwrap_or(Tier::Balanced)
}

/// Machine facts that settings validation needs.
pub fn environment(shared: &Shared) -> Environment {
    Environment {
        installed_harnesses: shared.env.read().installed(),
        providers_with_keys: shared
            .providers
            .status()
            .into_iter()
            .filter(|p| p.has_key)
            .map(|p| p.name)
            .collect(),
    }
}

/// Settings validation: routes, harness availability, keys, projects, and permission rules.
pub fn validate_settings(
    global: &GlobalConfig,
    env: &Environment,
    settings: &WorkspaceSettings,
) -> Vec<ValidationIssue> {
    let mut issues = validate_workspace(global, settings, env, default_tier);
    let lists = [
        ("allow", &settings.permissions.allow),
        ("ask", &settings.permissions.ask),
        ("deny", &settings.permissions.deny),
    ];
    for (name, list) in lists {
        for (i, rule) in list.iter().enumerate() {
            if let Err(e) = ostra_policy::validate_rule(rule) {
                issues.push(ValidationIssue {
                    path: format!("permissions.{name}[{i}]"),
                    message: e,
                });
            }
        }
    }
    for (i, p) in settings.projects.iter().enumerate() {
        // A relative path already has its own issue from `validate_workspace`.
        if p.path.is_absolute() && !p.path.is_dir() {
            issues.push(ValidationIssue {
                path: format!("projects[{i}].path"),
                message: format!("{} is not a folder.", p.path.display()),
            });
        }
    }
    issues
}

pub fn field_issue(path: &str, message: String) -> ValidationIssue {
    ValidationIssue {
        path: path.into(),
        message,
    }
}

/// The trimmed key and stack of an import or clone request, with an issue on `key` or `stack` for
/// each problem.
pub fn key_and_stack(
    settings: &WorkspaceSettings,
    key: &str,
    stack: Option<&str>,
    issues: &mut Vec<ValidationIssue>,
) -> (String, Option<String>) {
    let key = key.trim();
    if !is_project_key(key) {
        issues.push(field_issue("key", format!("`{key}` is not a project key. Use lowercase letters, digits, and dashes, starting with a letter or digit.")));
    } else if settings.project(key).is_some() {
        issues.push(field_issue(
            "key",
            format!(
                "A project named `{key}` already exists in this workspace. Choose another key."
            ),
        ));
    }
    let stack = stack.map(str::trim).filter(|s| !s.is_empty());
    if let Some(s) = stack.filter(|s| !is_stack_name(s)) {
        issues.push(field_issue("stack", stack_issue(s)));
    }
    (key.to_string(), stack.map(str::to_string))
}

/// The settings entry an import request adds, or every problem with the request, each on its
/// field: `key`, `path`, or `stack`.
pub fn import_entry(
    settings: &WorkspaceSettings,
    root: &Path,
    req: &ImportProject,
) -> Result<ProjectEntry, Vec<ValidationIssue>> {
    let mut issues = vec![];
    let (key, stack) = key_and_stack(settings, &req.key, req.stack.as_deref(), &mut issues);
    let path = if !req.path.is_absolute() {
        issues.push(field_issue("path", "Use an absolute path.".into()));
        None
    } else {
        match std::fs::canonicalize(&req.path) {
            Err(_) => {
                issues.push(field_issue(
                    "path",
                    format!("{} does not exist.", req.path.display()),
                ));
                None
            }
            Ok(p) if !p.is_dir() => {
                issues.push(field_issue(
                    "path",
                    format!("{} is not a folder.", p.display()),
                ));
                None
            }
            Ok(p) => {
                if let Some(other) = settings.projects.iter().find(|o| o.path == p) {
                    issues.push(field_issue(
                        "path",
                        format!("That folder is already imported as `{}`.", other.key),
                    ));
                } else if paths::is_inside(&root.join(paths::RUNTIME_DIR), &p) {
                    issues.push(field_issue(
                        "path",
                        "A project cannot live inside the workspace's .ostra directory.".into(),
                    ));
                }
                Some(p)
            }
        }
    };
    match path {
        Some(path) if issues.is_empty() => Ok(ProjectEntry {
            key,
            path,
            stack,
            code_provider: None,
            language_servers: vec![],
        }),
        _ => Err(issues),
    }
}

/// The branch `.git/HEAD` names, or the short commit id of a detached HEAD. Reads the file
/// directly, because a git process per project per workspace read is too slow. A `.git` file
/// (a worktree or submodule) points at the real git dir.
pub fn git_branch(project: &Path) -> Option<String> {
    let dot = project.join(".git");
    let dir = if dot.is_file() {
        let text = std::fs::read_to_string(&dot).ok()?;
        let target = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if target.is_absolute() {
            target
        } else {
            project.join(target)
        }
    } else {
        dot
    };
    let head = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        return Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string());
    }
    (head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit())).then(|| head[..7].to_string())
}

/// Every agent's definition with its routes under `settings`.
pub fn agent_infos(global: &GlobalConfig, settings: &WorkspaceSettings) -> Vec<AgentInfo> {
    AgentName::ALL
        .into_iter()
        .map(|name| {
            let def = ostra_agents::agent_def(name);
            let q = RouteQuery::new(name.as_str(), def.default_tier);
            let resolved = resolve_route(global, settings, q).ok();
            let default_route = resolve_route(
                global,
                settings,
                RouteQuery {
                    tier_override: Some(def.default_tier),
                    ..q
                },
            )
            .ok();
            let executor = resolve_executor(settings, name.as_str(), None);
            AgentInfo {
                name,
                label: name.label(),
                description: def.description.clone(),
                default_tier: def.default_tier,
                effort: def.effort.clone(),
                default_effort: ostra_agents::effort_for(name, executor),
                capabilities: def.capabilities.clone(),
                timeout_secs: def.timeout_secs,
                resolved,
                default_route,
            }
        })
        .collect()
}

fn ultracode_bootstrap(path: &Path) -> bool {
    let uc = path.join(".ultracode");
    uc.join("INVENTORY.md").exists() && uc.join("repo-profile.json").exists()
}

impl WorkspaceRt {
    pub fn open(shared: Arc<Shared>, id: WorkspaceId, root: &Path) -> anyhow::Result<Self> {
        crate::trust::migrate(&shared.registry, root);
        crate::trust::seal_file_secrets(&shared.registry, root);
        let db = WorkspaceDb::open(&paths::workspace_db(root))?;
        db.set_workspace_id(&id)?;
        let services = Arc::new(ServerServices {
            shared: shared.clone(),
            root: root.to_path_buf(),
            workspace: id.clone(),
        });
        let engine = Engine::new(root.to_path_buf(), id.clone(), db.clone(), services);
        let rt = WorkspaceRt {
            id,
            root: root.to_path_buf(),
            db,
            engine,
            shared,
            cloning: Default::default(),
        };
        rt.sync_projects();
        Ok(rt)
    }

    /// The settings as the user edits them: the file, with the mode and YOLO from the registry.
    pub fn settings(&self) -> WorkspaceSettings {
        let mut s = self.file_settings();
        crate::trust::overlay(&self.shared.registry, &self.root, &mut s);
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
        crate::trust::effective(&self.shared.registry, &self.root, self.file_settings())
    }

    fn write_settings(&self, settings: &WorkspaceSettings) -> Result<(), ConfigError> {
        crate::trust::save_workspace(&self.shared.registry, &self.root, settings)
    }

    pub fn environment(&self) -> Environment {
        environment(&self.shared)
    }

    pub fn validate(&self, settings: &WorkspaceSettings) -> Vec<ValidationIssue> {
        validate_settings(&self.shared.global(), &self.environment(), settings)
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
    pub fn import_project(&self, req: &ImportProject) -> Result<(), crate::setup::CreateError> {
        let mut settings = self.settings();
        let entry =
            import_entry(&settings, &self.root, req).map_err(crate::setup::CreateError::Invalid)?;
        settings.projects.push(entry);
        self.write_settings(&settings)
            .map_err(|e| crate::setup::CreateError::Failed(e.to_string()))?;
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
        self.write_settings(&settings).map_err(|e| e.to_string())?;
        self.sync_projects();
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
        let global = self.shared.global();
        let validation = validate_settings(&global, &self.environment(), &settings);
        let providers: Vec<ProviderStatus> = self.shared.providers.status();
        WorkspaceDetail {
            id: self.id.clone(),
            root: self.root.clone(),
            projects: self.project_views(),
            harnesses: self.shared.env.read().harnesses.clone(),
            providers,
            validation,
            agents: agent_infos(&global, &settings),
            stacks: ostra_agents::stack_names(),
            global_permissions: global.permissions.clone(),
            pending_commands: crate::trust::pending(&self.shared.registry, &self.root, &settings),
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
    use ostra_core::executor::{ExecutorKind, HarnessKind};

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

    fn request(path: &Path, key: &str, stack: Option<&str>) -> ImportProject {
        ImportProject {
            path: path.to_path_buf(),
            key: key.into(),
            stack: stack.map(str::to_string),
        }
    }

    fn fields(r: Result<ProjectEntry, Vec<ValidationIssue>>) -> Vec<String> {
        r.err()
            .unwrap_or_default()
            .into_iter()
            .map(|i| i.path)
            .collect()
    }

    #[test]
    fn import_problems_name_their_field() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let (root, app) = (base.join("ws"), base.join("app"));
        std::fs::create_dir_all(root.join(".ostra/inner")).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(base.join("file.txt"), "x").unwrap();
        let mut settings = WorkspaceSettings::seeded("ws");
        settings.projects.push(ProjectEntry {
            key: "api".into(),
            path: base.join("api"),
            stack: None,
            code_provider: None,
            language_servers: vec![],
        });

        let ok = import_entry(&settings, &root, &request(&app, " app ", Some(" "))).unwrap();
        assert_eq!(
            (ok.key.as_str(), ok.path.clone(), ok.stack.clone()),
            ("app", app.clone(), None)
        );
        assert_eq!(
            import_entry(&settings, &root, &request(&app, "app", Some("rust-axum")))
                .unwrap()
                .stack
                .as_deref(),
            Some("rust-axum")
        );

        assert_eq!(
            fields(import_entry(
                &settings,
                &root,
                &request(&app, "App", Some("Go\u{7}Lang"))
            )),
            ["key", "stack"]
        );
        assert_eq!(
            fields(import_entry(&settings, &root, &request(&app, "api", None))),
            ["key"]
        );
        assert_eq!(
            fields(import_entry(
                &settings,
                &root,
                &request(Path::new("rel"), "b", None)
            )),
            ["path"]
        );
        assert_eq!(
            fields(import_entry(
                &settings,
                &root,
                &request(&base.join("nope"), "b", None)
            )),
            ["path"]
        );
        assert_eq!(
            fields(import_entry(
                &settings,
                &root,
                &request(&base.join("file.txt"), "b", None)
            )),
            ["path"]
        );
        assert_eq!(
            fields(import_entry(
                &settings,
                &root,
                &request(&root.join(".ostra/inner"), "b", None)
            )),
            ["path"]
        );
        settings.projects.push(ok);
        let again = import_entry(&settings, &root, &request(&app, "other", None)).unwrap_err();
        assert_eq!(
            (again[0].path.as_str(), again[0].message.as_str()),
            ("path", "That folder is already imported as `app`.")
        );
    }

    #[test]
    fn branch_comes_from_head() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        assert_eq!(git_branch(p), None, "not a repository");
        std::fs::create_dir_all(p.join(".git")).unwrap();
        std::fs::write(p.join(".git/HEAD"), "ref: refs/heads/feature/login\n").unwrap();
        assert_eq!(git_branch(p).as_deref(), Some("feature/login"));
        std::fs::write(
            p.join(".git/HEAD"),
            "3f2a9c1d0e4b5a6978877665544332211aabbccd\n",
        )
        .unwrap();
        assert_eq!(
            git_branch(p).as_deref(),
            Some("3f2a9c1"),
            "a detached HEAD shows its short id"
        );
        std::fs::write(p.join(".git/HEAD"), "garbage").unwrap();
        assert_eq!(git_branch(p), None);

        let wt = p.join("wt");
        std::fs::create_dir_all(p.join("gitdirs/wt")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(p.join("gitdirs/wt/HEAD"), "ref: refs/heads/topic\n").unwrap();
        std::fs::write(wt.join(".git"), "gitdir: ../gitdirs/wt\n").unwrap();
        assert_eq!(
            git_branch(&wt).as_deref(),
            Some("topic"),
            "a .git file points at the real git dir"
        );
    }

    #[test]
    fn agent_infos_resolve_current_and_default_routes() {
        let global = GlobalConfig::default();
        let mut settings = WorkspaceSettings::seeded("x");
        settings
            .routing
            .model
            .by_agent
            .insert("plan".into(), "fast".into());
        settings.routing.executor.by_agent.insert(
            "implementer".into(),
            ExecutorKind::Harness(HarnessKind::Codex),
        );
        settings.routing.model.by_agent.remove("explore");
        let infos = agent_infos(&global, &settings);
        assert_eq!(infos.len(), AgentName::ALL.len());
        let get = |n: AgentName| infos.iter().find(|i| i.name == n).unwrap();

        let plan = get(AgentName::Plan);
        assert_eq!(
            (plan.label.as_str(), plan.default_tier),
            ("Plan", Tier::Advanced)
        );
        assert_eq!(
            plan.resolved.as_ref().map(|r| (r.tier, r.model.as_str())),
            Some((Some(Tier::Fast), "anthropic:claude-haiku-4-5-20251001"))
        );
        assert_eq!(
            plan.default_route.as_ref().map(|r| r.model.as_str()),
            Some("anthropic:claude-opus-5-5"),
            "Agent default is the agent's own tier"
        );
        assert!(plan.capabilities.contains(&ostra_core::Capability::Read));

        let imp = get(AgentName::Implementer);
        assert_eq!(
            imp.resolved.as_ref().map(|r| r.executor),
            Some(ExecutorKind::Harness(HarnessKind::Codex))
        );
        assert_eq!(
            imp.default_effort,
            ostra_agents::effort_for(
                AgentName::Implementer,
                ExecutorKind::Harness(HarnessKind::Codex)
            )
        );

        let explore = get(AgentName::Explore);
        assert_eq!(
            explore.resolved, None,
            "a route that does not resolve is null"
        );
        assert!(explore.default_route.is_some());
    }
}
