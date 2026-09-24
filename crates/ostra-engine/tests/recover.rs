//! Restart recovery (HANDOVER 11.2): a running execution becomes interrupted, keeps what it spent,
//! and its step re-runs with the same spawn block.

use async_trait::async_trait;
use ostra_core::api::CreateSession;
use ostra_core::config::{GlobalConfig, ProjectEntry, ResolvedRoute, WorkspaceSettings};
use ostra_core::event::{ExecPurpose, SessionEvent, SessionOptions};
use ostra_core::exec::{
    CancellationToken, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec,
    ExecutionStatus, Executor, Usage,
};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::WorkspaceId;
use ostra_core::model::Effort;
use ostra_engine::factory::AgentsFactory;
use ostra_engine::{Engine, Notice, Services, SpawnFactory};
use ostra_store::WorkspaceDb;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Spends $0.50, then runs until cancelled.
struct Hanging;

#[async_trait]
impl Executor for Hanging {
    async fn run(
        &self,
        _spec: ExecutionSpec,
        host: Arc<dyn ExecutionHost>,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        host.emit(ExecutionDelta::Usage {
            usage: Usage {
                cost_usd: 0.5,
                ..Default::default()
            },
        });
        cancel.cancelled().await;
        ExecutionResult::with_status(ExecutionStatus::Cancelled)
    }
}

struct Fake {
    ws: WorkspaceSettings,
}

#[async_trait]
impl Services for Fake {
    fn global(&self) -> GlobalConfig {
        let mut g = GlobalConfig::default();
        let t = g.tiers.get_mut("native").unwrap();
        t.fast = Some("mock:m".into());
        t.balanced = Some("mock:m".into());
        t.advanced = Some("mock:m".into());
        t.frontier = Some("mock:m".into());
        g
    }
    fn workspace(&self) -> WorkspaceSettings {
        self.ws.clone()
    }
    fn executor(&self, kind: ExecutorKind) -> Option<Arc<dyn Executor>> {
        (kind == ExecutorKind::Native).then(|| Arc::new(Hanging) as Arc<dyn Executor>)
    }
    fn factory(&self) -> Arc<dyn SpawnFactory> {
        Arc::new(AgentsFactory)
    }
    async fn judge(
        &self,
        _: &ResolvedRoute,
        _: &str,
        _: &str,
        _: Value,
        _: Effort,
    ) -> Result<(Value, Usage), String> {
        Ok((
            json!({"category": "RESEARCH", "projects": ["app"], "explore_tasks": [{"project": "app", "task": "t"}], "opts_in": {"tests": false, "docs": false}, "reason": "r"}),
            Usage::default(),
        ))
    }
    fn notify(&self, _: Notice) {}
    fn protected_paths(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn add_allow_rule(&self, _: &str) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_keeps_usage_and_reruns() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    std::fs::create_dir_all(app.join(".ostra")).unwrap();
    std::fs::write(app.join(".ostra/INVENTORY.md"), "# Inventory").unwrap();
    let mut ws = WorkspaceSettings::seeded("t");
    ws.projects.push(ProjectEntry {
        key: "app".into(),
        path: app,
        stack: None,
        code_provider: None,
    });
    let services = Arc::new(Fake { ws });
    let db = WorkspaceDb::open(&dir.path().join("workspace.db")).unwrap();
    let id = WorkspaceId::new();
    let first = Engine::new(dir.path().to_path_buf(), id.clone(), db, services.clone());
    let s = first
        .create_session(CreateSession {
            request: "Explain greet".into(),
            options: SessionOptions::default(),
            projects: vec![],
        })
        .unwrap();
    let running = || first.state(&s.id).unwrap().running_executions().count();
    for _ in 0..100 {
        if running() == 1
            && first
                .db()
                .list_executions(&s.id)
                .unwrap()
                .iter()
                .any(|e| e.usage.cost_usd > 0.0)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(running(), 1);

    // A second process opens the same database, as after a crash.
    let db2 = WorkspaceDb::open(&dir.path().join("workspace.db")).unwrap();
    let second = Engine::new(dir.path().to_path_buf(), id, db2, services);
    second.recover().unwrap();
    let events = second.db().events(&s.id).unwrap();
    let interrupted = events
        .iter()
        .find_map(|e| match &e.event {
            SessionEvent::ExecutionFinished { result, .. }
                if result.status == ExecutionStatus::Interrupted =>
            {
                Some(result.clone())
            }
            _ => None,
        })
        .expect("an interrupted execution");
    assert!(
        (interrupted.usage.cost_usd - 0.5).abs() < 1e-9,
        "usage kept: {:?}",
        interrupted.usage
    );
    for _ in 0..100 {
        let explores = second
            .db()
            .events(&s.id)
            .unwrap()
            .iter()
            .filter(|e| {
                matches!(
                    &e.event,
                    SessionEvent::ExecutionStarted {
                        purpose: ExecPurpose::Explore { .. },
                        ..
                    }
                )
            })
            .count();
        if explores == 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the explore step did not re-run after recovery");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_stop_prevents_rerun_on_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    std::fs::create_dir_all(app.join(".ostra")).unwrap();
    std::fs::write(app.join(".ostra/INVENTORY.md"), "# Inventory").unwrap();
    let mut ws = WorkspaceSettings::seeded("t");
    ws.projects.push(ProjectEntry {
        key: "app".into(),
        path: app,
        stack: None,
        code_provider: None,
    });
    let services = Arc::new(Fake { ws });
    let path = dir.path().join("workspace.db");
    let id = WorkspaceId::new();
    let first = Engine::new(
        dir.path().to_path_buf(),
        id.clone(),
        WorkspaceDb::open(&path).unwrap(),
        services.clone(),
    );
    let s = first
        .create_session(CreateSession {
            request: "Explain greet".into(),
            options: SessionOptions::default(),
            projects: vec![],
        })
        .unwrap();
    for _ in 0..100 {
        if first.state(&s.id).unwrap().running_executions().count() == 1
            && first
                .db()
                .list_executions(&s.id)
                .unwrap()
                .iter()
                .any(|e| e.usage.cost_usd > 0.0)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The server "died"; stop the session before starting again.
    let db = WorkspaceDb::open(&path).unwrap();
    assert_eq!(
        ostra_engine::runner::stop_session_offline(&db, &s.id).unwrap(),
        1
    );
    let second = Engine::new(
        dir.path().to_path_buf(),
        id,
        WorkspaceDb::open(&path).unwrap(),
        services,
    );
    second.recover().unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let st = second.state(&s.id).unwrap();
    assert!(st.failed.is_some());
    let started = st.executions.len();
    assert_eq!(started, 1, "no execution re-ran after an offline stop");
    let finished = st
        .executions
        .values()
        .next()
        .unwrap()
        .result
        .clone()
        .unwrap();
    assert_eq!(finished.status, ExecutionStatus::Cancelled);
    assert!((finished.usage.cost_usd - 0.5).abs() < 1e-9);
}
