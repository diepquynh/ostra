//! First-run setup: the machine check, creating a workspace in one call, and the onboarding flag.

use crate::app::App;
use crate::workspace::{WorkspaceRt, validate_settings};
use ostra_core::api::{CreateWorkspace, EnvironmentStatus, OnboardingState, RoutingPreset};
use ostra_core::config::{
    Environment, GlobalConfig, PermissionMode, ProjectEntry, ValidationIssue, WorkspaceSettings,
    load_toml_required,
};
use ostra_core::ids::WorkspaceId;
use ostra_core::paths;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub async fn environment(app: &App) -> EnvironmentStatus {
    crate::api::refresh_env(app).await;
    EnvironmentStatus {
        providers: app.shared.providers.status(),
        harnesses: app.shared.env.read().harnesses.clone(),
        stacks: ostra_agents::stack_names(),
        sandbox: ostra_core::api::SandboxStatus::check(&app.shared.global().sandbox),
    }
}

pub fn onboarding(app: &App) -> Result<OnboardingState, ostra_store::StoreError> {
    Ok(OnboardingState {
        onboarded_at: app.shared.registry.onboarded_at()?,
        workspaces: app.shared.registry.list_workspaces()?.len() as u32,
    })
}

/// What validation needs from outside the request body.
pub struct DraftCtx<'a> {
    pub global: &'a GlobalConfig,
    pub env: &'a Environment,
    /// Whether a folder is already a registered workspace.
    pub registered: &'a dyn Fn(&Path) -> bool,
}

/// The workspace a create request would write, and every problem with it.
#[derive(Debug)]
pub struct Draft {
    pub root: PathBuf,
    pub settings: WorkspaceSettings,
    pub issues: Vec<ValidationIssue>,
    /// The folder already held a `workspace.toml`, which these settings build on.
    pub adopted: bool,
}

fn issue(path: impl Into<String>, message: impl Into<String>) -> ValidationIssue {
    ValidationIssue {
        path: path.into(),
        message: message.into(),
    }
}

fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).ok()
}

/// Build the full settings from a create request and validate them as a whole. Reads the file
/// system and writes nothing. When the folder already holds a `workspace.toml`, the request
/// applies on top of it, so a folder whose registration was lost can be registered again.
pub fn draft(body: &CreateWorkspace, ctx: &DraftCtx<'_>) -> Draft {
    let mut issues = vec![];
    let name = body.name.trim().to_string();

    let mut reusable = false;
    let root = if body.root.as_os_str().is_empty() {
        issues.push(issue("root", "Choose a folder for the workspace."));
        None
    } else if !body.root.is_absolute() {
        issues.push(issue(
            "root",
            "Choose an absolute folder for the workspace.",
        ));
        None
    } else {
        let root =
            std::fs::canonicalize(&body.root).unwrap_or_else(|_| paths::normalize(&body.root));
        if root.exists() && !root.is_dir() {
            issues.push(issue(
                "root",
                format!("{} is a file. Choose a folder.", root.display()),
            ));
        } else if (ctx.registered)(&root) {
            issues.push(issue(
                "root",
                "That folder is already a registered workspace.",
            ));
        } else {
            reusable = true;
        }
        Some(root)
    };

    let existing = root
        .as_deref()
        .filter(|_| reusable)
        .map(paths::workspace_toml)
        .filter(|p| p.is_file());
    let mut adopted = false;
    let mut settings = match existing
        .as_deref()
        .map(load_toml_required::<WorkspaceSettings>)
    {
        // Rule A2: a folder file's mode, YOLO, spend limits, and sandbox settings are ignored;
        // only the request sets them.
        Some(Ok(mut s)) => {
            adopted = true;
            s.permissions.mode = PermissionMode::default();
            s.yolo.default = false;
            s.limits = Default::default();
            s.sandbox_mode = None;
            s.sandbox_network = None;
            s.sandbox_allowed_hosts.clear();
            s.sandbox_decoys.clear();
            s
        }
        Some(Err(e)) => {
            issues.push(issue("root", format!("The folder's .ostra/workspace.toml cannot be read, so Ostra cannot reuse it: {e}")));
            WorkspaceSettings::seeded(&name)
        }
        None => WorkspaceSettings::seeded(&name),
    };
    settings.name = name;
    let base_projects = settings.projects.len();

    let runtime_dir = root.as_ref().map(|r| r.join(paths::RUNTIME_DIR));
    for (i, p) in body.projects.iter().flatten().enumerate() {
        let path = if p.path.is_absolute() {
            std::fs::canonicalize(&p.path).unwrap_or_else(|_| p.path.clone())
        } else {
            p.path.clone()
        };
        if let Some(other) = settings.projects.iter().find(|o| o.path == path) {
            issues.push(issue(
                format!("projects[{i}].path"),
                format!("That folder is already added as `{}`.", other.key),
            ));
        }
        if runtime_dir
            .as_ref()
            .is_some_and(|d| paths::is_inside(d, &path))
        {
            issues.push(issue(
                format!("projects[{i}].path"),
                "A project cannot live inside the workspace's .ostra directory.",
            ));
        }
        settings.projects.push(ProjectEntry {
            key: p.key.trim().to_string(),
            path,
            stack: p.stack.clone().filter(|s| !s.trim().is_empty()),
            code_provider: None,
            language_servers: vec![],
        });
    }

    if let Some(mode) = body.permissions.as_ref().and_then(|p| p.mode.as_deref()) {
        match parse_enum::<PermissionMode>(mode) {
            Some(m) => settings.permissions.mode = m,
            None => issues.push(issue(
                "permissions.mode",
                format!(
                    "`{mode}` is not a permission mode. Use default, acceptEdits, plan, or bypass."
                ),
            )),
        }
    }
    if let Some(yolo) = body.yolo.as_ref().and_then(|y| y.default) {
        settings.yolo.default = yolo;
    }
    if let Some(push) = body.notifications.as_ref().and_then(|n| n.push) {
        settings.notifications.push = push;
    }
    if let Some(preset) = body.routing_preset.as_deref() {
        match parse_enum::<RoutingPreset>(preset) {
            Some(p) => p.apply(&mut settings.routing),
            None => issues.push(issue(
                "routing_preset",
                format!("`{preset}` is not a routing preset. Use native, codex, or claude."),
            )),
        }
    }

    for mut i in validate_settings(ctx.global, ctx.env, &settings) {
        i.path = request_path(&i.path, base_projects);
        issues.push(i);
    }
    issues.sort_by(|a, b| a.path.cmp(&b.path).then(a.message.cmp(&b.message)));
    issues.dedup();
    Draft {
        root: root.unwrap_or_default(),
        settings,
        issues,
        adopted,
    }
}

/// Map a settings path back to the request: projects from the body follow the ones an existing
/// `workspace.toml` already had, and problems with those older entries belong to the folder.
fn request_path(path: &str, base_projects: usize) -> String {
    let Some(rest) = path.strip_prefix("projects[") else {
        return path.to_string();
    };
    let Some((index, tail)) = rest.split_once(']') else {
        return path.to_string();
    };
    match index.parse::<usize>() {
        Ok(n) if n >= base_projects => format!("projects[{}]{tail}", n - base_projects),
        Ok(_) => "root".into(),
        Err(_) => path.to_string(),
    }
}

pub enum CreateError {
    Invalid(Vec<ValidationIssue>),
    Failed(String),
}

impl From<ostra_store::StoreError> for CreateError {
    fn from(e: ostra_store::StoreError) -> Self {
        CreateError::Failed(e.to_string())
    }
}

fn registered(app: &App, root: &Path) -> bool {
    app.shared
        .registry
        .workspace_by_root(root)
        .ok()
        .flatten()
        .is_some()
}

fn draft_for(app: &App, body: &CreateWorkspace) -> Draft {
    let global = app.shared.global();
    let env = crate::workspace::environment(&app.shared);
    draft(
        body,
        &DraftCtx {
            global: &global,
            env: &env,
            registered: &|root| registered(app, root),
        },
    )
}

pub fn validate(app: &App, body: &CreateWorkspace) -> Vec<ValidationIssue> {
    draft_for(app, body).issues
}

/// Validate the request as a whole, then write the folder, `workspace.toml`, and the registry
/// entry. Nothing is written when there is any issue.
pub fn create(app: &Arc<App>, body: &CreateWorkspace) -> Result<Arc<WorkspaceRt>, CreateError> {
    let d = draft_for(app, body);
    if !d.issues.is_empty() {
        return Err(CreateError::Invalid(d.issues));
    }
    let registry = &app.shared.registry;
    std::fs::create_dir_all(&d.root)
        .map_err(|e| CreateError::Failed(format!("Could not create {}: {e}", d.root.display())))?;
    let root = std::fs::canonicalize(&d.root).map_err(|e| CreateError::Failed(e.to_string()))?;
    if registered(app, &root) {
        return Err(CreateError::Invalid(vec![issue(
            "root",
            "That folder is already a registered workspace.",
        )]));
    }
    crate::trust::create_workspace(registry, &root, &d.settings, d.adopted)
        .map_err(|e| CreateError::Failed(e.to_string()))?;
    let id = WorkspaceId::new();
    registry.add_workspace(&id, &d.settings.name, &root)?;
    let rt = app
        .attach(id, &root)
        .map_err(|e| CreateError::Failed(format!("{e:#}")))?;
    registry.mark_onboarded()?;
    Ok(rt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostra_core::api::{CreateNotifications, CreatePermissions, CreateYolo, ImportProject};
    use ostra_core::config::save_toml;
    use ostra_core::executor::{ExecutorKind, HarnessKind};

    fn env() -> Environment {
        Environment {
            installed_harnesses: vec![HarnessKind::Codex],
            providers_with_keys: vec!["anthropic".into()],
        }
    }

    fn body(root: &Path) -> CreateWorkspace {
        CreateWorkspace {
            name: "shop".into(),
            root: root.to_path_buf(),
            projects: None,
            permissions: None,
            yolo: None,
            routing_preset: None,
            notifications: None,
        }
    }

    fn project(path: &Path, key: &str) -> ImportProject {
        ImportProject {
            path: path.to_path_buf(),
            key: key.into(),
            stack: None,
        }
    }

    fn run(b: &CreateWorkspace) -> Draft {
        run_with(b, &|_| false)
    }

    fn run_with(b: &CreateWorkspace, registered: &dyn Fn(&Path) -> bool) -> Draft {
        let global = GlobalConfig::default();
        let env = env();
        draft(
            b,
            &DraftCtx {
                global: &global,
                env: &env,
                registered,
            },
        )
    }

    fn paths_of(d: &Draft) -> Vec<&str> {
        d.issues.iter().map(|i| i.path.as_str()).collect()
    }

    #[test]
    fn full_body_builds_settings_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let root = dir.path().join("ws");
        let mut req = body(&root);
        req.projects = Some(vec![
            project(&a, "a"),
            ImportProject {
                stack: Some("rust-axum".into()),
                ..project(&b, "b")
            },
        ]);
        req.permissions = Some(CreatePermissions {
            mode: Some("acceptEdits".into()),
        });
        req.yolo = Some(CreateYolo {
            default: Some(true),
        });
        req.notifications = Some(CreateNotifications { push: Some(false) });
        req.routing_preset = Some("codex".into());
        let d = run(&req);
        assert_eq!(d.issues, vec![]);
        assert!(!root.exists(), "validation writes nothing");
        let s = &d.settings;
        assert_eq!(s.name, "shop");
        assert_eq!(
            s.projects
                .iter()
                .map(|p| p.key.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(s.projects[1].stack.as_deref(), Some("rust-axum"));
        assert_eq!(s.permissions.mode, PermissionMode::AcceptEdits);
        assert!(s.yolo.default);
        assert!(!s.notifications.push);
        assert_eq!(
            s.routing.executor.by_agent.get("implementer"),
            Some(&ExecutorKind::Harness(HarnessKind::Codex))
        );
    }

    #[test]
    fn old_body_gets_seeded_settings() {
        let dir = tempfile::tempdir().unwrap();
        let d = run(&body(&dir.path().join("ws")));
        assert_eq!(d.issues, vec![]);
        assert_eq!(d.settings, WorkspaceSettings::seeded("shop"));
    }

    #[test]
    fn name_and_root_issues() {
        let mut req = body(Path::new(""));
        req.name = "  ".into();
        assert_eq!(paths_of(&run(&req)), ["name", "root"]);
        assert_eq!(paths_of(&run(&body(Path::new("rel/ws")))), ["root"]);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(paths_of(&run(&body(&file))), ["root"]);
        let d = run_with(&body(dir.path()), &|_| true);
        assert_eq!(paths_of(&d), ["root"]);
        assert!(
            d.issues[0]
                .message
                .contains("already a registered workspace")
        );
    }

    #[test]
    fn project_issues_point_at_their_entry() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::create_dir_all(&a).unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(root.join(".ostra/inner")).unwrap();
        let mut req = body(&root);
        req.projects = Some(vec![
            project(&a, "a"),
            project(&a, "Bad_Key"),
            project(&dir.path().join("missing"), "a"),
            project(Path::new("relative"), "r"),
            project(&root.join(".ostra/inner"), "inner"),
        ]);
        let d = run(&req);
        assert_eq!(
            paths_of(&d),
            [
                "projects[1].key",
                "projects[1].path",
                "projects[2].key",
                "projects[2].path",
                "projects[3].path",
                "projects[4].path"
            ]
        );
        assert!(
            d.issues
                .iter()
                .any(|i| i.path == "projects[2].key" && i.message.contains("used twice"))
        );
        assert!(
            d.issues
                .iter()
                .any(|i| i.path == "projects[4].path" && i.message.contains(".ostra"))
        );
    }

    #[test]
    fn enum_and_routing_issues() {
        let dir = tempfile::tempdir().unwrap();
        let mut req = body(&dir.path().join("ws"));
        req.permissions = Some(CreatePermissions {
            mode: Some("yolo".into()),
        });
        req.routing_preset = Some("grok".into());
        assert_eq!(paths_of(&run(&req)), ["permissions.mode", "routing_preset"]);
        req.permissions = None;
        req.routing_preset = Some("claude".into());
        let d = run(&req);
        assert_eq!(
            paths_of(&d),
            [
                "routing.executor.byAgent.implementer",
                "routing.executor.byAgent.write-test"
            ]
        );
        assert!(d.issues[0].message.contains("Claude Code"));
    }

    #[test]
    fn existing_workspace_toml_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::create_dir_all(&a).unwrap();
        let root = dir.path().join("ws");
        let mut old = WorkspaceSettings::seeded("old");
        old.projects.push(ProjectEntry {
            key: "gone".into(),
            path: dir.path().join("gone"),
            stack: None,
            code_provider: None,
            language_servers: vec![],
        });
        old.instructions.all = Some("Keep it short.".into());
        old.permissions.mode = PermissionMode::Bypass;
        old.yolo.default = true;
        old.sandbox_mode = Some(ostra_core::config::SandboxMode::Off);
        save_toml(&paths::workspace_toml(&root), &old).unwrap();
        let mut req = body(&root);
        req.projects = Some(vec![project(&a, "gone")]);
        let d = run(&req);
        assert_eq!(d.settings.name, "shop");
        assert_eq!(
            d.settings.instructions.all.as_deref(),
            Some("Keep it short.")
        );
        assert_eq!(paths_of(&d), ["projects[0].key", "root"]);
        assert!(d.adopted);
        assert_eq!(d.settings.permissions.mode, PermissionMode::Default);
        assert!(
            !d.settings.yolo.default,
            "a folder file never turns YOLO on"
        );
        assert_eq!(
            d.settings.sandbox_mode, None,
            "a folder file never turns the sandbox off"
        );

        std::fs::write(paths::workspace_toml(&root), "name = [").unwrap();
        assert_eq!(paths_of(&run(&body(&root))), ["root"]);
    }

    #[test]
    fn request_paths_skip_existing_projects() {
        assert_eq!(request_path("projects[3].key", 2), "projects[1].key");
        assert_eq!(request_path("projects[1].path", 2), "root");
        assert_eq!(
            request_path("routing.model.byAgent.plan", 2),
            "routing.model.byAgent.plan"
        );
    }
}
