//! Subagent coordination tools wired into the server (HANDOVER 10.8). The engine checks and
//! records each call; this only finds the run's workspace and session.

use crate::app::App;
use ostra_core::agent::Capability;
use ostra_core::coord::CoordReply;
use ostra_core::exec::ExecutionSpec;
use ostra_core::ids::{ExecutionId, SessionId};
use ostra_core::paths;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

#[derive(Default)]
pub struct Coordination {
    app: OnceLock<Weak<App>>,
}

impl Coordination {
    pub fn bind(&self, app: &Arc<App>) {
        let _ = self.app.set(Arc::downgrade(app));
    }
}

impl ostra_tools::CoordConnector for Coordination {
    fn open(&self, spec: &ExecutionSpec) -> Option<Arc<dyn ostra_tools::Coordinate>> {
        if !spec.capabilities.contains(&Capability::Coordinate) {
            return None;
        }
        Some(Arc::new(ExecCoord {
            app: self.app.get()?.clone(),
            root: spec.ctx.workspace_root.clone(),
            session: spec.ctx.session_id.clone()?,
            execution: spec.id.clone(),
        }))
    }
}

/// One execution's coordination calls.
struct ExecCoord {
    app: Weak<App>,
    root: PathBuf,
    session: SessionId,
    execution: ExecutionId,
}

#[async_trait::async_trait]
impl ostra_tools::Coordinate for ExecCoord {
    async fn call(&self, tool: &str, input: &Value) -> Result<CoordReply, String> {
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
        w.engine
            .coordinate(&self.session, &self.execution, tool, input)
    }
}
