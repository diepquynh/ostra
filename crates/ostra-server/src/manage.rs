//! Management tools wired into the server (HANDOVER 10.7). The policy has already checked and,
//! for `ProjectCreate`, the user has already approved the call when it arrives here.

use crate::app::App;
use ostra_core::agent::{AgentName, Capability};
use ostra_core::api::{ImportProject, InitStatus};
use ostra_core::exec::ExecutionSpec;
use ostra_core::ids::{ExecutionId, SessionId};
use ostra_core::manage::{CreatedProject, PROJECT_CREATE, PROJECT_LIST, ProjectCreateInput};
use ostra_core::paths;
use ostra_workspace::WorkspaceRt;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

#[derive(Default)]
pub struct Management {
    app: OnceLock<Weak<App>>,
}

impl Management {
    pub fn bind(&self, app: &Arc<App>) {
        let _ = self.app.set(Arc::downgrade(app));
    }
}

impl ostra_tools::ManageConnector for Management {
    fn open(&self, spec: &ExecutionSpec) -> Option<Arc<dyn ostra_tools::Manage>> {
        if !spec.capabilities.contains(&Capability::ManageProjects) {
            return None;
        }
        Some(Arc::new(ExecManage {
            app: self.app.get()?.clone(),
            root: spec.ctx.workspace_root.clone(),
            session: spec.ctx.session_id.clone()?,
            execution: spec.id.clone(),
            agent: spec.agent,
        }))
    }
}

/// One execution's management calls.
struct ExecManage {
    app: Weak<App>,
    root: PathBuf,
    session: SessionId,
    execution: ExecutionId,
    agent: AgentName,
}

#[async_trait::async_trait]
impl ostra_tools::Manage for ExecManage {
    async fn call(&self, tool: &str, input: &Value) -> Result<String, String> {
        let app = self
            .app
            .upgrade()
            .ok_or("The Ostra server is shutting down.")?;
        let root = paths::fold(&self.root);
        let w = app
            .all_workspaces()
            .into_iter()
            .find(|w| paths::fold(&w.root) == root)
            .ok_or("This run's workspace is not open in Ostra.")?;
        match tool {
            PROJECT_LIST => Ok(self.list(&w)),
            PROJECT_CREATE => self.create(&app, &w, input).await,
            other => Err(format!("Unknown tool `{other}`.")),
        }
    }
}

impl ExecManage {
    fn list(&self, w: &WorkspaceRt) -> String {
        let scope = w
            .engine
            .state(&self.session)
            .map(|st| st.scope)
            .unwrap_or_default();
        let mut out = format!(
            "Workspace root: {}\nA new project's folder is relative to it.\n\n",
            w.root.display()
        );
        let views = w.project_views();
        if views.is_empty() {
            out.push_str("No projects yet.\n");
        }
        for p in views {
            out.push_str(&format!(
                "- `{}` at {}: stack {}, {}{}\n",
                p.key,
                p.path.display(),
                p.stack.as_deref().unwrap_or("not set"),
                match p.init_status {
                    InitStatus::Initialized => "initialized",
                    InitStatus::NotInitialized => "not initialized",
                    InitStatus::Initializing => "initializing",
                    InitStatus::Missing => "folder missing",
                },
                if scope.contains(&p.key) {
                    ", in this session's scope"
                } else {
                    ""
                }
            ));
        }
        out
    }

    async fn create(&self, app: &App, w: &WorkspaceRt, input: &Value) -> Result<String, String> {
        let req = ProjectCreateInput::parse(input)?;
        let st = w.engine.state(&self.session).map_err(|e| e.to_string())?;
        if let Some(why) =
            ostra_engine::pipeline::get().project_creation_refusal(&st, &self.execution, &req.key)
        {
            return Err(why);
        }
        let (dest, existed) =
            ostra_workspace::projects::create_target(&w.settings(), &w.root, &req).map_err(
                |issues| {
                    issues
                        .into_iter()
                        .map(|i| i.message)
                        .collect::<Vec<_>>()
                        .join(" ")
                },
            )?;
        let _claim = crate::git::claim(w, &req.key, &dest).map_err(|e| match e {
            crate::git::GitError::Busy(m) | crate::git::GitError::Invalid(m) => m,
            _ => "Another git operation holds this folder. Try again.".into(),
        })?;
        std::fs::create_dir_all(&dest)
            .map_err(|e| format!("Could not create {}: {e}", dest.display()))?;
        if req.git_init() {
            let auth = crate::git::GitAuth::new(None).map_err(|e| e.to_string())?;
            if let Err(e) = crate::git::run_git(
                Some(&dest),
                &auth,
                &["init", "--quiet"],
                crate::git::QUICK_TIMEOUT,
                |_| {},
            )
            .await
            {
                crate::git::clear_failed(&dest, existed);
                return Err(format!("git init failed, so nothing was created: {e}"));
            }
        }
        if let Err(e) = w.import_project(&ImportProject {
            path: dest.clone(),
            key: req.key.clone(),
            stack: Some(req.stack.clone()),
        }) {
            crate::git::clear_failed(&dest, existed);
            return Err(match e {
                ostra_workspace::CreateError::Invalid(issues) => issues
                    .into_iter()
                    .map(|i| i.message)
                    .collect::<Vec<_>>()
                    .join(" "),
                ostra_workspace::CreateError::Failed(m) => m,
            });
        }
        crate::git::workspace_updated(app, w);
        let path = w.project_path(&req.key).unwrap_or(dest);
        w.engine
            .add_created_project(
                &self.session,
                CreatedProject {
                    key: req.key.clone(),
                    path: path.clone(),
                    stack: req.stack.clone(),
                    purpose: req.purpose.clone(),
                    requirements: req.requirements.clone(),
                    execution: self.execution.clone(),
                    agent: self.agent,
                },
            )
            .map_err(|e| {
                format!(
                    "Created project `{}` at {}, but it could not join this session: {e}",
                    req.key,
                    path.display()
                )
            })?;
        Ok(format!(
            "Created project `{key}` at {path}{git}. It is in this session's scope now: tag the deliverables that build it with repo `{key}`. Its folder is empty, and Ostra initializes it from the stack and requirements you gave when the build starts, before any phase runs in it. Write the first deliverable so it creates the project's skeleton: manifest, entry point, and build command.",
            key = req.key,
            path = path.display(),
            git = if req.git_init() {
                ", with git initialized"
            } else {
                ""
            },
        ))
    }
}
