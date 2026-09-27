//! First-run setup: the machine check, creating a workspace in one call, and the onboarding flag.

use crate::app::App;
use ostra_core::api::{CreateWorkspace, EnvironmentStatus, OnboardingState};
use ostra_workspace::{CreateError, WorkspaceRt};
use std::sync::Arc;

pub async fn environment(app: &App) -> EnvironmentStatus {
    crate::api::refresh_env(app).await;
    EnvironmentStatus {
        providers: app.shared.providers.status(),
        harnesses: app.shared.env.read().harnesses.clone(),
        stacks: ostra_agents::stack_names(),
        sandbox: ostra_core::api::SandboxStatus::check(&app.shared.global().sandbox),
        shell: ostra_core::api::ShellStatus::check(),
    }
}

pub fn onboarding(app: &App) -> Result<OnboardingState, ostra_store::StoreError> {
    Ok(OnboardingState {
        onboarded_at: app.shared.registry.onboarded_at()?,
        workspaces: app.shared.registry.list_workspaces()?.len() as u32,
    })
}

/// Write and register the workspace, then open it.
pub fn create(app: &Arc<App>, body: &CreateWorkspace) -> Result<Arc<WorkspaceRt>, CreateError> {
    let (id, root) = ostra_workspace::create::register(app.shared.as_ref(), body)?;
    let rt = app
        .attach(id, &root)
        .map_err(|e| CreateError::Failed(format!("{e:#}")))?;
    app.shared.registry.mark_onboarded()?;
    Ok(rt)
}
