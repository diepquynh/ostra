//! Pause, resume, containment, and mid-session context (Rules C1, C2, P1, P2, P3) on a real engine.

use async_trait::async_trait;
use ostra_core::api::{CreateSession, SessionStatus};
use ostra_core::config::{GlobalConfig, ProjectEntry, ResolvedRoute, WorkspaceSettings};
use ostra_core::event::{ContextDelivery, ContextFile, SessionEvent, SessionOptions};
use ostra_core::exec::{
    CancellationToken, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec,
    ExecutionStatus, Executor, ResumeInfo, Usage,
};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, SessionId, WorkspaceId};
use ostra_core::model::Effort;
use ostra_engine::factory::AgentsFactory;
use ostra_engine::runner::{PAUSE_RESUME_NOTE, RESUMED_STATUS};
use ostra_engine::{Engine, Notice, Services, SpawnFactory};
use ostra_store::WorkspaceDb;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Runs until cancelled, spending one dollar, and records the id, resume, and capabilities each
/// run was given. The first run emits `deltas` before it waits.
#[derive(Default)]
struct Hanging {
    ids: Mutex<Vec<ExecutionId>>,
    resumes: Mutex<Vec<Option<ResumeInfo>>>,
    caps: Mutex<Vec<usize>>,
    deltas: Mutex<Vec<ExecutionDelta>>,
}

#[async_trait]
impl Executor for Hanging {
    async fn run(
        &self,
        spec: ExecutionSpec,
        host: Arc<dyn ExecutionHost>,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        self.ids.lock().unwrap().push(spec.id.clone());
        self.resumes.lock().unwrap().push(spec.resume.clone());
        self.caps.lock().unwrap().push(spec.capabilities.len());
        let deltas = std::mem::take(&mut *self.deltas.lock().unwrap());
        for d in deltas {
            host.emit(d);
        }
        let spent = Usage {
            cost_usd: 1.0,
            ..Default::default()
        };
        host.emit(ExecutionDelta::Usage { usage: spent });
        cancel.cancelled().await;
        let mut r = ExecutionResult::with_status(ExecutionStatus::Cancelled);
        r.native_session_id = Some("sid-1".into());
        r.usage = spent;
        r
    }
}

struct Fake {
    ws: WorkspaceSettings,
    exec: Arc<Hanging>,
    harness: bool,
    /// Holds the first executor lookup, which a spawn makes before it records its start, until
    /// the test sends on the channel; `held` says it is waiting.
    hold: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    held: std::sync::atomic::AtomicBool,
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
        let c = g.tiers.entry("claude".into()).or_default();
        c.fast = Some("haiku".into());
        c.balanced = Some("haiku".into());
        c.advanced = Some("haiku".into());
        c.frontier = Some("haiku".into());
        g
    }
    fn workspace(&self) -> WorkspaceSettings {
        self.ws.clone()
    }
    fn executor(&self, kind: ExecutorKind) -> Option<Arc<dyn Executor>> {
        let hold = self.hold.lock().unwrap().take();
        if let Some(rx) = hold {
            self.held.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = rx.recv();
        }
        (kind == ExecutorKind::Native || self.harness)
            .then(|| self.exec.clone() as Arc<dyn Executor>)
    }
    fn factory(&self) -> Arc<dyn SpawnFactory> {
        Arc::new(AgentsFactory)
    }
    async fn judge(
        &self,
        _: &ResolvedRoute,
        _: &str,
        user: &str,
        _: Value,
        _: Effort,
    ) -> Result<(Value, Usage), String> {
        let out = if user.contains("# Context the user added") {
            json!({"route": "requirement_change", "items": [{"id": "answer", "disposition": "deliver"}], "research": [], "reason": "r"})
        } else {
            json!({"category": "RESEARCH", "projects": ["app"], "explore_tasks": [{"project": "app", "task": "t"}], "opts_in": {"tests": false, "docs": false}, "reason": "r"})
        };
        Ok((out, Usage::default()))
    }
    fn notify(&self, _: Notice) {}
    fn protected_paths(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn command_approved(&self, _: &std::path::Path, _: &str) -> bool {
        true
    }
    fn add_allow_rule(&self, _: &str) {}
}

struct Setup {
    _dir: tempfile::TempDir,
    engine: Engine,
    exec: Arc<Hanging>,
    services: Arc<Fake>,
}

fn setup() -> Setup {
    setup_with(false)
}

fn setup_with(harness: bool) -> Setup {
    static CACHE: std::sync::Once = std::sync::Once::new();
    // SAFETY: set once, before any engine here starts; nothing in this file starts a sandbox.
    CACHE.call_once(|| unsafe {
        std::env::set_var(
            "OSTRA_SANDBOX_CACHE",
            std::env::temp_dir().join("ostra-test-pause-cache"),
        );
    });
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    std::fs::create_dir_all(app.join(".ostra")).unwrap();
    std::fs::create_dir_all(app.join("docs")).unwrap();
    std::fs::write(app.join(".ostra/INVENTORY.md"), "# Inventory").unwrap();
    std::fs::write(app.join("docs/orders.md"), "# Orders").unwrap();
    std::fs::write(dir.path().join("secret.txt"), "x").unwrap();
    let mut ws = WorkspaceSettings::seeded("t");
    ws.projects.push(ProjectEntry {
        key: "app".into(),
        path: app,
        stack: None,
        code_provider: None,
        language_servers: vec![],
    });
    if harness {
        ws.routing.executor.by_agent.insert(
            "explore".into(),
            ExecutorKind::Harness(ostra_core::HarnessKind::Claude),
        );
    }
    let exec = Arc::new(Hanging::default());
    let services = Arc::new(Fake {
        ws,
        exec: exec.clone(),
        harness,
        hold: Mutex::new(None),
        held: Default::default(),
    });
    let db = WorkspaceDb::open(&dir.path().join("workspace.db")).unwrap();
    let engine = Engine::new(
        dir.path().to_path_buf(),
        WorkspaceId::new(),
        db,
        services.clone(),
    );
    Setup {
        _dir: dir,
        engine,
        exec,
        services,
    }
}

fn file(path: &str) -> ContextFile {
    ContextFile {
        project: "app".into(),
        path: path.into(),
    }
}

fn start(engine: &Engine, files: Vec<ContextFile>) -> SessionId {
    engine
        .create_session(CreateSession {
            request: "Explain orders".into(),
            options: SessionOptions::default(),
            projects: vec![],
            files,
            uploads: vec![],
            docs_book: None,
        })
        .unwrap()
        .id
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting until {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_interrupts_and_resume_continues_the_run() {
    let t = setup();
    let e = &t.engine;
    let s = start(e, vec![]);
    until("the explore runs", || {
        t.exec.resumes.lock().unwrap().len() == 1
    })
    .await;

    let summary = e.pause_session(&s).unwrap();
    assert_eq!(summary.status, SessionStatus::Paused);
    until("the explore is interrupted", || {
        e.state(&s).unwrap().running_executions().count() == 0
    })
    .await;
    let st = e.state(&s).unwrap();
    let paused = st.executions.values().next().unwrap();
    let result = paused.result.as_ref().unwrap();
    assert_eq!(result.status, ExecutionStatus::Interrupted);
    assert_eq!(result.error.as_deref(), Some("The session was paused."));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        t.exec.resumes.lock().unwrap().len(),
        1,
        "nothing starts while paused"
    );
    assert!(e.pause_session(&s).is_err());

    e.resume_session(&s).unwrap();
    until("the explore resumes", || {
        t.exec.resumes.lock().unwrap().len() == 2
    })
    .await;
    let resume = t.exec.resumes.lock().unwrap()[1].clone().expect("a resume");
    assert_eq!(resume.from, paused.id);
    assert_eq!(resume.native_session_id.as_deref(), Some("sid-1"));
    assert_eq!(resume.note.as_deref(), Some(PAUSE_RESUME_NOTE));
    assert!(e.resume_session(&s).is_err());

    // Rule P2: the run continues under its own id, so the session keeps one row for it.
    let ids = t.exec.ids.lock().unwrap().clone();
    assert_eq!(ids, vec![paused.id.clone(), paused.id.clone()]);
    let rows = e.db().list_executions(&s).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, ExecutionStatus::Running);
    until("the resumed run reports its spend", || {
        e.db()
            .get_execution(&paused.id)
            .unwrap()
            .unwrap()
            .usage
            .cost_usd
            == 2.0
    })
    .await;
    e.stop_session(&s).unwrap();
    until("the resumed run ends", || {
        e.state(&s).unwrap().running_executions().count() == 0
    })
    .await;
    let st = e.state(&s).unwrap();
    assert_eq!(st.executions.len(), 1);
    assert_eq!(st.spent_usd(), 2.0, "the resumed run adds to what it spent");
    let lines: Vec<String> = e
        .db()
        .activity_after(&paused.id, 0)
        .unwrap()
        .into_iter()
        .filter_map(|i| match i.delta {
            ExecutionDelta::Status { message } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(lines, vec!["The session was paused.", RESUMED_STATUS]);
}

fn refused(host: &str, port: u16, local: bool) -> ExecutionDelta {
    ExecutionDelta::Egress {
        host: host.into(),
        port,
        allowed: false,
        reason: Some("not listed".into()),
        local,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn containment_signals_pause_the_session() {
    let t = setup();
    let e = &t.engine;
    *t.exec.deltas.lock().unwrap() = vec![
        refused("telemetry.example.com", 443, false),
        refused("127.0.0.1", 8001, true),
        refused("10.0.0.1", 22, true),
        refused("169.254.169.254", 80, true),
        refused("127.0.0.1", 8002, true),
    ];
    let s = start(e, vec![]);
    until("the session pauses", || e.state(&s).unwrap().paused).await;
    until("the explore is interrupted", || {
        e.state(&s).unwrap().running_executions().count() == 0
    })
    .await;
    let st = e.state(&s).unwrap();
    let explore = st.executions.values().next().unwrap();
    assert_eq!(st.contained.as_ref(), Some(&explore.id));
    assert_eq!(
        explore.result.as_ref().unwrap().status,
        ExecutionStatus::Interrupted
    );
    let signals = e
        .db()
        .events(&s)
        .unwrap()
        .iter()
        .filter(|ev| matches!(ev.event, SessionEvent::ContainmentSignal { .. }))
        .count();
    assert_eq!(
        signals, 3,
        "a public refusal is no signal, and the log stops at the pause"
    );
    let summary = e.db().get_session(&s).unwrap().unwrap();
    assert_eq!(summary.status, SessionStatus::Paused);
    assert!(
        summary.stage_label.contains("containment"),
        "{}",
        summary.stage_label
    );

    e.resume_session(&s).unwrap();
    until("the explore resumes", || {
        t.exec.resumes.lock().unwrap().len() == 2
    })
    .await;
    assert_eq!(e.state(&s).unwrap().contained, None);
    e.stop_session(&s).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ended_session_loses_its_tool_caches() {
    let t = setup();
    let e = &t.engine;
    let s = start(e, vec![]);
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    let dir = ostra_sandbox::session_cache(&home, &s);
    assert!(dir.starts_with(std::env::temp_dir().join("ostra-test-pause-cache")));
    std::fs::create_dir_all(dir.join("cargo/registry")).unwrap();
    until("the explore runs", || {
        t.exec.resumes.lock().unwrap().len() == 1
    })
    .await;
    e.stop_session(&s).unwrap();
    until("the cache is removed", || !dir.exists()).await;
}

/// A stop that lands while a spawn is being set up starts nothing: the spawn finds the session
/// ended when it records its start.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_during_spawn_setup_starts_nothing() {
    let t = setup();
    let e = &t.engine;
    let (release, rx) = std::sync::mpsc::channel();
    *t.services.hold.lock().unwrap() = Some(rx);
    let s = start(e, vec![]);
    until("a spawn is being set up", || {
        t.services.held.load(std::sync::atomic::Ordering::SeqCst)
    })
    .await;
    e.stop_session(&s).unwrap();
    release.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let st = e.state(&s).unwrap();
    assert!(st.executions.is_empty(), "{:?}", st.executions.keys());
    assert!(t.exec.resumes.lock().unwrap().is_empty(), "no executor ran");
    assert!(e.db().running_executions().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn context_sent_now_restarts_running_work() {
    let t = setup();
    let e = &t.engine;
    let s = start(e, vec![file("docs/orders.md")]);
    until("the explore runs", || {
        t.exec.resumes.lock().unwrap().len() == 1
    })
    .await;
    e.amend(
        &s,
        "see @app/docs/orders.md".into(),
        vec![file("./docs/orders.md")],
        vec![],
        ContextDelivery::Now,
    )
    .unwrap();
    until("the first explore is interrupted", || {
        e.state(&s).unwrap().executions.values().any(|r| {
            r.result
                .as_ref()
                .is_some_and(|r| r.status == ExecutionStatus::Interrupted)
        })
    })
    .await;
    until("work starts again", || {
        t.exec.resumes.lock().unwrap().len() >= 2
    })
    .await;
    assert!(
        t.exec.resumes.lock().unwrap().iter().all(Option::is_none),
        "context sent now re-runs, it does not resume"
    );
    let st = e.state(&s).unwrap();
    assert_eq!(st.files, vec![file("docs/orders.md")]);
    assert_eq!(st.amendments[0].files, vec![file("docs/orders.md")]);
    let amended = e.db().events(&s).unwrap().iter().any(|ev| {
        matches!(
            &ev.event,
            SessionEvent::RequestAmended {
                delivery: ContextDelivery::Now,
                ..
            }
        )
    });
    assert!(amended);
    e.stop_session(&s).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attached_files_and_folders_must_be_inside_a_project() {
    let t = setup();
    let e = &t.engine;
    for bad in [
        file("../secret.txt"),
        file("/etc/passwd"),
        file("docs/missing.md"),
        file("docs/orders.md/"),
        ContextFile {
            project: "other".into(),
            path: "docs/orders.md".into(),
        },
    ] {
        let r = e.create_session(CreateSession {
            request: "Explain orders".into(),
            options: SessionOptions::default(),
            projects: vec![],
            files: vec![bad.clone()],
            uploads: vec![],
            docs_book: None,
        });
        assert!(r.is_err(), "{bad:?} was accepted");
    }
    let s = start(e, vec![]);
    assert!(
        e.amend(&s, String::new(), vec![], vec![], ContextDelivery::Queue)
            .is_err()
    );
    e.amend(
        &s,
        String::new(),
        vec![file("docs/orders.md")],
        vec![],
        ContextDelivery::Queue,
    )
    .unwrap();
    e.amend(
        &s,
        String::new(),
        vec![file("./docs")],
        vec![],
        ContextDelivery::Queue,
    )
    .unwrap();
    let st = e.state(&s).unwrap();
    assert_eq!(st.amendments[1].files, vec![file("docs/")]);
    assert!(st.full_request().contains("(folder, @app/docs/)"));
    e.stop_session(&s).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ended_harness_run_reopens_read_only() {
    let t = setup_with(true);
    let e = &t.engine;
    let s = start(e, vec![]);
    until("the explore runs", || {
        t.exec.resumes.lock().unwrap().len() == 1
    })
    .await;
    let explore = e
        .state(&s)
        .unwrap()
        .executions
        .keys()
        .next()
        .unwrap()
        .clone();
    assert!(
        e.inspect_execution(&explore).is_err(),
        "a running run is refused"
    );
    e.stop_session(&s).unwrap();
    until("the explore ends", || {
        e.db().get_execution(&explore).unwrap().unwrap().status != ExecutionStatus::Running
    })
    .await;

    let view = e.inspect_execution(&explore).unwrap();
    until("the read-only session starts", || {
        t.exec.resumes.lock().unwrap().len() == 2
    })
    .await;
    let resume = t.exec.resumes.lock().unwrap()[1].clone().unwrap();
    assert!(resume.inspect);
    assert_eq!(resume.from, explore);
    assert_eq!(resume.native_session_id.as_deref(), Some("sid-1"));
    assert_eq!(t.exec.caps.lock().unwrap()[1], 0, "no tools");
    let row = e.db().get_execution(&view).unwrap().unwrap();
    assert_eq!(row.session, None, "outside the pipeline");
    assert_eq!(
        row.purpose,
        Some(ostra_core::event::ExecPurpose::Inspect {
            of: explore.clone()
        })
    );
    assert!(
        e.inspect_execution(&view).is_err(),
        "not a read-only session of a read-only session"
    );
    e.cancel_execution(&view).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_are_kept_in_the_session_and_listed_as_artifacts() {
    let t = setup();
    let e = &t.engine;
    let first = e.stage_upload("notes.pdf", b"%PDF-1.7 bytes").unwrap();
    let s = e
        .create_session(CreateSession {
            request: "Explain orders".into(),
            options: SessionOptions::default(),
            projects: vec![],
            files: vec![],
            uploads: vec![first.id.clone()],
            docs_book: None,
        })
        .unwrap()
        .id;
    let st = e.state(&s).unwrap();
    assert_eq!(st.uploads.len(), 1);
    let kept = &st.uploads[0];
    assert!(kept.path.starts_with(st.session_root.join("uploads")));
    assert_eq!(std::fs::read(&kept.path).unwrap(), b"%PDF-1.7 bytes");
    assert!(st.full_request().contains(&kept.path.display().to_string()));

    let second = e.stage_upload("notes.pdf", b"more").unwrap();
    e.amend(
        &s,
        String::new(),
        vec![],
        vec![second.id],
        ContextDelivery::Queue,
    )
    .unwrap();
    let detail = e.detail(&s).unwrap();
    let uploads: Vec<&str> = detail
        .artifacts
        .iter()
        .filter(|a| a.kind == "upload")
        .map(|a| a.label.as_str())
        .collect();
    assert_eq!(uploads, ["Upload: notes.pdf", "Upload: notes (2).pdf"]);
    assert_eq!(detail.additions[0].uploads[0].name, "notes (2).pdf");
    // A claimed upload cannot be claimed again.
    assert!(
        e.amend(
            &s,
            String::new(),
            vec![],
            vec![first.id],
            ContextDelivery::Queue
        )
        .is_err()
    );
    e.stop_session(&s).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_skipped_research_task_stops_and_opens_no_gate() {
    let t = setup();
    let e = &t.engine;
    let s = start(e, vec![]);
    until("the explore runs", || {
        t.exec.resumes.lock().unwrap().len() == 1
    })
    .await;
    let explore = t.exec.ids.lock().unwrap()[0].clone();
    assert!(e.execution(&explore).unwrap().can_skip);
    e.skip_execution(&explore).unwrap();
    until("the explore ends", || {
        e.db().get_execution(&explore).unwrap().unwrap().status != ExecutionStatus::Running
    })
    .await;
    let row = e.execution(&explore).unwrap();
    assert_eq!(row.status, ExecutionStatus::Interrupted);
    assert!(!row.can_skip);
    assert!(e.skip_execution(&explore).is_err(), "skipped once");
    until("the session moves on", || {
        e.state(&s).unwrap().failed.is_some()
    })
    .await;
    let st = e.state(&s).unwrap();
    assert!(st.explore[0].abandoned);
    assert_eq!(st.open_gates().count(), 0, "no failure gate");
    // Rule D1: the only research task was skipped, so there is nothing to answer from.
    assert!(st.failed.unwrap().contains("no research document"));
    assert_eq!(t.exec.ids.lock().unwrap().len(), 1, "nothing re-ran it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_correction_resumes_the_run_in_place_with_it() {
    let t = setup();
    let e = &t.engine;
    let s = start(e, vec![]);
    until("the explore runs", || {
        t.exec.resumes.lock().unwrap().len() == 1
    })
    .await;
    let id = t.exec.ids.lock().unwrap()[0].clone();
    assert!(e.steer_execution(&id, "  ".into()).is_err());
    e.steer_execution(&id, "Read src/refund.rs first.".into())
        .unwrap();
    until("the explore resumes", || {
        t.exec.resumes.lock().unwrap().len() == 2
    })
    .await;
    // Rule U2: the same run continues, and its first message is the correction.
    let resume = t.exec.resumes.lock().unwrap()[1].clone().expect("a resume");
    assert_eq!(resume.from, id);
    assert_eq!(
        resume.note,
        Some(ostra_engine::runner::steer_note(
            "Read src/refund.rs first."
        ))
    );
    assert_eq!(t.exec.ids.lock().unwrap()[1], id);
    assert!(
        e.withdraw_steer(&id).is_err(),
        "a correction sent to a running run is final"
    );
    e.stop_session(&s).unwrap();
    until("the run ends", || {
        e.state(&s).unwrap().running_executions().count() == 0
    })
    .await;
    assert!(e.steer_execution(&id, "Too late.".into()).is_err());
}
