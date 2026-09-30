//! The runner end to end: a real Engine, an in-memory workspace database, a scripted executor,
//! and scripted judges, driving an IMPLEMENT session under YOLO to completion.

use async_trait::async_trait;
use ostra_core::agent::AgentName;
use ostra_core::api::{CreateSession, SessionStatus, StageStatus};
use ostra_core::config::{GlobalConfig, ProjectEntry, ResolvedRoute, WorkspaceSettings};
use ostra_core::event::{CommandPurpose, ExecPurpose, JudgeKind, SessionEvent, SessionOptions};
use ostra_core::exec::{
    CancellationToken, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec,
    ExecutionStatus, Executor, Usage,
};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::WorkspaceId;
use ostra_core::model::Effort;
use ostra_core::pipeline::StageKind;
use ostra_engine::factory::AgentsFactory;
use ostra_engine::{Engine, EngineNotice, Notice, Services, SessionState, SpawnFactory, view};
use ostra_store::WorkspaceDb;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Fake {
    ws: WorkspaceSettings,
    executor: Arc<dyn Executor>,
    notices: Mutex<Vec<Notice>>,
}

#[async_trait]
impl Services for Fake {
    fn global(&self) -> GlobalConfig {
        let mut g = GlobalConfig::default();
        for t in g.tiers.values_mut() {
            if t.fast
                .as_deref()
                .is_some_and(|m| m.starts_with("anthropic:"))
            {
                t.fast = Some("mock:fast".into());
                t.balanced = Some("mock:balanced".into());
                t.advanced = Some("mock:advanced".into());
                t.frontier = Some("mock:frontier".into());
            }
        }
        g
    }
    fn workspace(&self) -> WorkspaceSettings {
        self.ws.clone()
    }
    fn executor(&self, kind: ExecutorKind) -> Option<Arc<dyn Executor>> {
        (kind == ExecutorKind::Native).then(|| self.executor.clone())
    }
    fn factory(&self) -> Arc<dyn SpawnFactory> {
        Arc::new(AgentsFactory)
    }
    async fn judge(
        &self,
        _route: &ResolvedRoute,
        system: &str,
        user: &str,
        schema: Value,
        _e: Effort,
    ) -> Result<(Value, Usage), String> {
        let classify = system.contains("\"category\"")
            || user.contains("# Toggles from the New task form");
        let out = if classify && user.contains("Document the greeting") {
            json!({"category": "DOCS", "projects": ["app"], "explore_tasks": [], "opts_in": {"tests": false, "docs": true}, "reason": "The request asks for documentation.", "title": "Greeting docs"})
        } else if classify {
            json!({"category": "IMPLEMENT", "projects": ["app"], "explore_tasks": [{"project": "app", "task": "research"}], "opts_in": {"tests": false, "docs": false}, "reason": "The request changes code.", "title": "Greeting"})
        } else if schema["properties"].get("track").is_some() {
            json!({"track": "full", "reason": "A contract changes."})
        } else if schema["properties"].get("stakes").is_some() {
            json!({"stakes": "high", "reason": "Touches two layers."})
        } else if schema["properties"].get("report_markdown").is_some() {
            json!({"report_markdown": "# Done\n\nEverything ran.", "reason": "r"})
        } else if schema["properties"].get("answer").is_some() {
            let kind = &schema["properties"]["answer"]["properties"]["kind"]["const"];
            match kind.as_str() {
                Some("approval") => {
                    json!({"answer": {"kind": "approval", "approved": true}, "reason": "The fact-check passed."})
                }
                Some("questions") => {
                    json!({"answer": {"kind": "questions", "answers": [{"id": "Q1", "question": "Which?", "answer": "A"}]}, "reason": "Recommended."})
                }
                _ => json!({"answer": {"kind": "choice", "option": "block"}, "reason": "r"}),
            }
        } else {
            return Err(format!("unexpected judge: {schema}"));
        };
        Ok((out, Usage::default()))
    }
    fn notify(&self, notice: Notice) {
        self.notices.lock().unwrap().push(notice);
    }
    fn protected_paths(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn command_approved(&self, _: &std::path::Path, _: &str) -> bool {
        true
    }
    fn add_allow_rule(&self, _rule: &str) {}
}

struct Scripted {
    root: PathBuf,
    runs: Mutex<Vec<(AgentName, ExecPurpose, String)>>,
}

#[async_trait]
impl Executor for Scripted {
    async fn run(
        &self,
        spec: ExecutionSpec,
        host: Arc<dyn ExecutionHost>,
        _cancel: CancellationToken,
    ) -> ExecutionResult {
        host.emit(ExecutionDelta::Text {
            text: format!("{} running", spec.agent),
        });
        host.emit(ExecutionDelta::ToolCall {
            call_id: "c1".into(),
            call: ostra_core::policy::ToolCall::new(
                "Read",
                json!({"file_path": spec.ctx.repo_root.join("src.txt")}),
            ),
        });
        let sess = spec.ctx.session_dir.clone();
        let purpose = self.runs.lock().unwrap().len();
        let _ = purpose;
        self.runs.lock().unwrap().push((
            spec.agent,
            ExecPurpose::QuickAnswer,
            spec.first_message.clone(),
        ));
        let root = spec.ctx.session_root.clone();
        let spec_path = root.join("ostra-spec-1.md");
        let plan_path = root.join("ostra-plan-1.md");
        let submit = match spec.agent {
            AgentName::Explore => {
                let p = sess.join("ostra-research-1.md");
                std::fs::write(&p, "# Research").unwrap();
                json!({"research_path": p, "scope_covered": "s", "findings_summary": "f", "sources_retrieved": 0, "open_questions": 0, "not_covered": []})
            }
            AgentName::GenerateSpec => {
                std::fs::write(&spec_path, "# Spec").unwrap();
                json!({"spec_path": spec_path, "open_questions": [], "external_evidence_rows": 0, "deliverables": 1, "requirements": 2, "summary": "spec"})
            }
            AgentName::FactCheck => {
                json!({"verdict": "PASS", "target": if spec.first_message.contains("Target type: plan") { "plan" } else { "spec" }, "findings": []})
            }
            AgentName::Plan => {
                std::fs::write(&plan_path, "# Plan").unwrap();
                let phase = root.join("ostra-plan-1-phase-1.md");
                std::fs::write(&phase, "# Phase 1\n**Complexity:** Low").unwrap();
                json!({"spec_path": spec_path, "master_plan_path": plan_path, "phases": [{"id": 1, "deliverable": "D1", "project": "app", "title": "core", "complexity": "Low", "test_policy": "Required", "depends_on": [], "file": phase}], "stakes": "High", "summary": "plan", "step_count": 1, "requirement_coverage": "2 of 2"})
            }
            AgentName::Implementer => {
                std::fs::write(self.root.join("src.txt"), "code").unwrap();
                let report = spec.ctx.report_file.clone().unwrap();
                std::fs::write(&report, "# Report").unwrap();
                json!({"status": "ok", "report_path": report, "changed_files": ["src.txt"], "summary": "done"})
            }
            AgentName::CodeReviewer => {
                json!({"findings": [], "security_block": false, "ledger_path": "/l", "summary": "passed"})
            }
            AgentName::Documentation => {
                let stub = sess.join("ostra-docs-request.md");
                let text = std::fs::read_to_string(&stub).unwrap_or_default();
                assert!(text.contains("Document the greeting"), "the stub holds the request: {text}");
                json!({"status": "ok", "summary": "Documented the greeting.", "overview": "app greets users.",
                    "sections": [{"id": "greeting", "title": "Greeting", "purpose": "Prints a greeting.",
                        "assumptions": ["stdout is open."],
                        "diagrams": [{"title": "Greet", "kind": "sequence", "source": "sequenceDiagram\nUser->>app: run\napp-->>User: hello"}],
                        "code_refs": [{"path": "src.txt", "note": "the greeting"}]}],
                    "glossary": [{"term": "Greeting", "definition": "The text app prints."}]})
            }
            other => return ExecutionResult::error(format!("unexpected agent {other}")),
        };
        ExecutionResult {
            status: ExecutionStatus::Ok,
            submit: Some(submit),
            final_text: "done".into(),
            usage: Usage {
                cost_usd: 0.01,
                ..Default::default()
            },
            native_session_id: None,
            error: None,
        }
    }
}

fn project(dir: &Path) -> PathBuf {
    static CACHE: std::sync::Once = std::sync::Once::new();
    // SAFETY: set once, before any test here starts a sandboxed program that reads it.
    CACHE.call_once(|| unsafe {
        std::env::set_var(
            "OSTRA_SANDBOX_CACHE",
            std::env::temp_dir().join("ostra-test-sandbox-cache"),
        );
    });
    let p = dir.join("app");
    std::fs::create_dir_all(p.join(".ostra")).unwrap();
    std::fs::write(p.join(".ostra/INVENTORY.md"), "# Inventory").unwrap();
    std::fs::write(
        p.join(".ostra/project.toml"),
        "[commands]\nformat = \"true\"\n",
    )
    .unwrap();
    p
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn implement_session_runs_to_completion_under_yolo() {
    let dir = tempfile::tempdir().unwrap();
    let app = project(dir.path());
    let ws_root = dir.path().join("ws");
    std::fs::create_dir_all(&ws_root).unwrap();
    let mut ws = WorkspaceSettings::seeded("t");
    ws.projects.push(ProjectEntry {
        key: "app".into(),
        path: app.clone(),
        stack: None,
        code_provider: None,
        language_servers: vec![],
    });
    let exec = Arc::new(Scripted {
        root: app.clone(),
        runs: Mutex::new(vec![]),
    });
    let services = Arc::new(Fake {
        ws,
        executor: exec.clone(),
        notices: Mutex::new(vec![]),
    });
    let db = WorkspaceDb::open_in_memory().unwrap();
    let engine = Engine::new(ws_root.clone(), WorkspaceId::new(), db, services.clone());
    let mut rx = engine.subscribe();
    let summary = engine
        .create_session(CreateSession {
            request: "Add a greeting".into(),
            options: SessionOptions {
                yolo: true,
                ..Default::default()
            },
            projects: vec![],
            files: vec![],
            uploads: vec![],
            docs_book: None,
        })
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let st = engine.state(&summary.id).unwrap();
        if st.is_terminal() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out; events: {:#?}",
            engine
                .db()
                .events(&summary.id)
                .unwrap()
                .iter()
                .map(|e| format!("{:?}", e.event))
                .collect::<Vec<_>>()
        );
        let _ = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    }
    let st = engine.state(&summary.id).unwrap();
    assert!(st.failed.is_none(), "failed: {:?}", st.failed);
    let events = engine.db().events(&summary.id).unwrap();
    let decisions: Vec<JudgeKind> = events
        .iter()
        .filter_map(|e| match &e.event {
            SessionEvent::DecisionMade { judge, .. } => Some(*judge),
            _ => None,
        })
        .collect();
    assert!(decisions.contains(&JudgeKind::Classify));
    assert!(
        decisions.contains(&JudgeKind::YoloAnswer),
        "approvals were answered by the YOLO judge"
    );
    // A running format command is visible before it finishes.
    let started = events
        .iter()
        .position(|e| {
            matches!(
                &e.event,
                SessionEvent::CommandStarted { purpose: CommandPurpose::Format, command, .. }
                    if command == "true"
            )
        })
        .expect("format announced");
    let ran = events
        .iter()
        .position(|e| {
            matches!(
                &e.event,
                SessionEvent::CommandRan {
                    purpose: CommandPurpose::Format,
                    ..
                }
            )
        })
        .unwrap();
    assert!(started < ran);
    let mid = SessionState::fold(summary.id.clone(), &events[..=started]);
    assert_eq!(view::inferred_stage(&mid).1, "Formatting app");
    assert!(view::stages(&mid).iter().any(|c| {
        c.stage == StageKind::Format
            && c.status == StageStatus::Running
            && c.detail.as_deref() == Some("true")
    }));
    assert!(
        SessionState::fold(summary.id.clone(), &events[..=ran]).project_tracks["app"]
            .running
            .is_none()
    );
    let agents: Vec<AgentName> = exec.runs.lock().unwrap().iter().map(|r| r.0).collect();
    assert_eq!(
        agents,
        vec![
            AgentName::Explore,
            AgentName::GenerateSpec,
            AgentName::FactCheck,
            AgentName::Plan,
            AgentName::FactCheck,
            AgentName::Implementer,
            AgentName::CodeReviewer
        ]
    );
    // The implementer saw its phase file and its declared report path in the spawn block.
    let imp = &exec.runs.lock().unwrap()[5].2.clone();
    assert!(imp.contains("Phase file:"), "{imp}");
    assert!(imp.contains("Report file:"), "{imp}");
    let summary = engine.db().get_session(&summary.id).unwrap().unwrap();
    assert_eq!(summary.status, SessionStatus::Completed);
    assert!(summary.cost_usd > 0.0);
    let detail = engine.detail(&summary.id).unwrap();
    assert_eq!(summary.title.as_deref(), Some("Greeting"));
    assert_eq!(detail.summary.title.as_deref(), Some("Greeting"));
    let imp = detail
        .executions
        .iter()
        .find(|e| e.agent == AgentName::Implementer)
        .unwrap();
    assert_eq!(imp.group, "implementer:app");
    assert_eq!(imp.run_label, "Phase 1");
    assert_eq!(imp.stream, ostra_core::executor::ExecStream::Activity);
    assert_eq!(imp.summary.as_deref(), Some("Read src.txt"));
    assert!(!imp.has_transcript);
    let checks: Vec<&str> = detail
        .executions
        .iter()
        .filter(|e| e.agent == AgentName::FactCheck)
        .map(|e| e.run_label.as_str())
        .collect();
    assert_eq!(checks, ["Spec check", "Plan check"]);
    let groups: Vec<&str> = detail
        .execution_groups
        .iter()
        .map(|g| g.group.as_str())
        .collect();
    assert_eq!(
        groups,
        [
            "explore:app",
            "generate-spec:app",
            "fact-check:app",
            "plan:app",
            "implementer:app",
            "code-reviewer:app"
        ]
    );
    assert_eq!(detail.execution_groups[2].executions.len(), 2);
    assert!(
        detail
            .execution_groups
            .iter()
            .all(|g| g.status == ExecutionStatus::Ok)
    );
    assert_eq!(engine.execution(&imp.id).unwrap().run_label, "Phase 1");
    assert!(detail.completion.unwrap().contains("Everything ran"));
    assert!(
        services
            .notices
            .lock()
            .unwrap()
            .iter()
            .any(|n| n.title == "Session complete")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn init_session_start_and_end_notify_project_changes() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    std::fs::create_dir_all(&app).unwrap();
    let ws_root = dir.path().join("ws");
    std::fs::create_dir_all(&ws_root).unwrap();
    let mut ws = WorkspaceSettings::seeded("t");
    ws.projects.push(ProjectEntry {
        key: "app".into(),
        path: app.clone(),
        stack: None,
        code_provider: None,
        language_servers: vec![],
    });
    let exec = Arc::new(Scripted {
        root: app,
        runs: Mutex::new(vec![]),
    });
    let services = Arc::new(Fake {
        ws,
        executor: exec,
        notices: Mutex::new(vec![]),
    });
    let engine = Engine::new(
        ws_root,
        WorkspaceId::new(),
        WorkspaceDb::open_in_memory().unwrap(),
        services,
    );
    let mut rx = engine.subscribe();
    let summary = engine.create_init_session("app", None).unwrap();
    engine.stop_session(&summary.id).unwrap();
    let mut changes = 0;
    while let Ok(n) = rx.try_recv() {
        if matches!(n, EngineNotice::ProjectsChanged) {
            changes += 1;
        }
    }
    assert_eq!(changes, 2);
}

/// Rules H2 and H5: the first spec run asks an explore helper and waits; the first fact-check
/// fails, so the author and then the checker continue their conversations.
struct Coordinating {
    inner: Scripted,
    engine: std::sync::OnceLock<Engine>,
    log: Mutex<Vec<(AgentName, ostra_core::ids::ExecutionId, Option<ostra_core::exec::ResumeInfo>)>>,
}

#[async_trait]
impl Executor for Coordinating {
    async fn run(
        &self,
        spec: ExecutionSpec,
        host: Arc<dyn ExecutionHost>,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        let seen = {
            let mut log = self.log.lock().unwrap();
            let seen = log.iter().filter(|(a, _, _)| *a == spec.agent).count();
            log.push((spec.agent, spec.id.clone(), spec.resume.clone()));
            seen
        };
        match (spec.agent, seen) {
            (AgentName::GenerateSpec, 0) => {
                let engine = self.engine.get().unwrap();
                let session = spec.ctx.session_id.clone().unwrap();
                let reply = engine
                    .coordinate(
                        &session,
                        &spec.id,
                        ostra_core::coord::SUBAGENT_ASK,
                        &json!({"message": "How are greetings localized?", "agent": "explore"}),
                    )
                    .unwrap();
                assert_eq!(reply.end, ostra_core::coord::RunEnd::Wait);
                let mut r = ExecutionResult::with_status(ExecutionStatus::Waiting);
                r.submit = Some(ostra_core::coord::end_payload("SubagentAsk", "q"));
                r
            }
            (AgentName::FactCheck, 0) => ExecutionResult {
                status: ExecutionStatus::Ok,
                submit: Some(json!({"verdict": "FAIL", "target": "spec", "findings": [{"severity": "HIGH", "location": "R1", "claim": "c", "issue": "R1 names a missing file"}]})),
                final_text: String::new(),
                usage: Usage::default(),
                native_session_id: None,
                error: None,
            },
            _ => self.inner.run(spec, host, cancel).await,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagents_wake_each_other_and_pair_loops_continue_conversations() {
    let dir = tempfile::tempdir().unwrap();
    let app = project(dir.path());
    let ws_root = dir.path().join("ws");
    std::fs::create_dir_all(&ws_root).unwrap();
    let mut ws = WorkspaceSettings::seeded("t");
    ws.limits.max_parallel_executions = 1;
    ws.projects.push(ProjectEntry {
        key: "app".into(),
        path: app.clone(),
        stack: None,
        code_provider: None,
        language_servers: vec![],
    });
    let exec = Arc::new(Coordinating {
        inner: Scripted {
            root: app.clone(),
            runs: Mutex::new(vec![]),
        },
        engine: std::sync::OnceLock::new(),
        log: Mutex::new(vec![]),
    });
    let services = Arc::new(Fake {
        ws,
        executor: exec.clone(),
        notices: Mutex::new(vec![]),
    });
    let db = WorkspaceDb::open_in_memory().unwrap();
    let engine = Engine::new(ws_root.clone(), WorkspaceId::new(), db, services.clone());
    let _ = exec.engine.set(engine.clone());
    let mut rx = engine.subscribe();
    let summary = engine
        .create_session(CreateSession {
            request: "Add a greeting".into(),
            options: SessionOptions {
                yolo: true,
                ..Default::default()
            },
            projects: vec![],
            files: vec![],
            uploads: vec![],
            docs_book: None,
        })
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !engine.state(&summary.id).unwrap().is_terminal() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {:#?}", exec.log.lock().unwrap());
        let _ = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    }
    let st = engine.state(&summary.id).unwrap();
    assert!(st.failed.is_none(), "failed: {:?}", st.failed);
    let log = exec.log.lock().unwrap().clone();
    let of = |agent: AgentName| -> Vec<_> { log.iter().filter(|(a, _, _)| *a == agent).cloned().collect() };
    let specs = of(AgentName::GenerateSpec);
    assert_eq!(specs.len(), 3, "{specs:#?}");
    let (_, first, none) = &specs[0];
    assert!(none.is_none());
    // H2: woken in place with the helper's answer, even with a single execution slot.
    let (_, woken, resume) = &specs[1];
    assert_eq!(woken, first);
    let resume = resume.as_ref().unwrap();
    assert_eq!(&resume.from, first);
    assert!(resume.note.as_ref().unwrap().contains("ostra-research-1.md"), "{resume:?}");
    // H5: the fact-check failed, so the author continues its conversation in a new run.
    let (_, revised, resume) = &specs[2];
    assert_ne!(revised, first);
    let resume = resume.as_ref().unwrap();
    assert_eq!(&resume.from, first);
    let note = resume.note.as_ref().unwrap();
    assert!(note.contains("continues your conversation") && note.contains("R1 names a missing file"), "{note}");
    let checks = of(AgentName::FactCheck);
    assert_eq!(checks[1].2.as_ref().unwrap().from, checks[0].1, "the checker continues too");
    assert_eq!(st.subagent_of(revised), *first);
    let events = engine.db().events(&summary.id).unwrap();
    assert!(events.iter().any(|e| matches!(e.event, SessionEvent::AgentAsked { .. })));
    assert!(events.iter().any(|e| matches!(e.event, SessionEvent::MessageDelivered { .. })));
    assert_eq!(of(AgentName::Explore).len(), 2, "the classify explore and the helper");
}

// Rule O6: a pinned session holds only its pinned projects.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_session_holds_only_its_pinned_projects() {
    let dir = tempfile::tempdir().unwrap();
    let app = project(dir.path());
    let lib = dir.path().join("lib");
    std::fs::create_dir_all(lib.join(".ostra")).unwrap();
    std::fs::write(lib.join(".ostra/INVENTORY.md"), "# Inventory").unwrap();
    let ws_root = dir.path().join("ws");
    std::fs::create_dir_all(&ws_root).unwrap();
    let mut ws = WorkspaceSettings::seeded("t");
    for (key, path) in [("app", &app), ("lib", &lib)] {
        ws.projects.push(ProjectEntry {
            key: key.into(),
            path: path.clone(),
            stack: None,
            code_provider: None,
            language_servers: vec![],
        });
    }
    let services = Arc::new(Fake {
        ws,
        executor: Arc::new(Scripted {
            root: app,
            runs: Mutex::new(vec![]),
        }),
        notices: Mutex::new(vec![]),
    });
    let engine = Engine::new(
        ws_root,
        WorkspaceId::new(),
        WorkspaceDb::open_in_memory().unwrap(),
        services,
    );
    let summary = engine
        .create_session(CreateSession {
            request: "Add a greeting".into(),
            options: SessionOptions::default(),
            projects: vec!["lib".into()],
            files: vec![],
            uploads: vec![],
            docs_book: None,
        })
        .unwrap();
    engine.stop_session(&summary.id).unwrap();
    let st = engine.state(&summary.id).unwrap();
    let keys: Vec<&str> = st.projects.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(keys, vec!["lib"]);
    assert_eq!(st.pinned, vec!["lib".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn docs_session_writes_the_book_into_the_workspace() {
    // Rules B1, B5, B6.
    let dir = tempfile::tempdir().unwrap();
    let app = project(dir.path());
    let ws_root = dir.path().join("ws");
    std::fs::create_dir_all(&ws_root).unwrap();
    let mut ws = WorkspaceSettings::seeded("t");
    ws.projects.push(ProjectEntry {
        key: "app".into(),
        path: app.clone(),
        stack: None,
        code_provider: None,
        language_servers: vec![],
    });
    let exec = Arc::new(Scripted {
        root: app.clone(),
        runs: Mutex::new(vec![]),
    });
    let services = Arc::new(Fake {
        ws,
        executor: exec.clone(),
        notices: Mutex::new(vec![]),
    });
    let db = WorkspaceDb::open_in_memory().unwrap();
    let engine = Engine::new(ws_root.clone(), WorkspaceId::new(), db, services.clone());
    let mut rx = engine.subscribe();
    let summary = engine
        .create_session(CreateSession {
            request: "Document the greeting".into(),
            options: SessionOptions::default(),
            projects: vec![],
            files: vec![],
            uploads: vec![],
            docs_book: None,
        })
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !engine.state(&summary.id).unwrap().is_terminal() {
        assert!(tokio::time::Instant::now() < deadline, "timed out");
        let _ = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    }
    let st = engine.state(&summary.id).unwrap();
    assert!(st.failed.is_none(), "failed: {:?}", st.failed);
    let written = st.book_written.clone().expect("the book write is recorded");
    assert_eq!((written.book.as_str(), written.error), ("app", None));
    let book = ostra_core::book::read(&ws_root, "app").expect("book.json");
    assert_eq!(book.projects, ["app"]);
    assert_eq!(book.sessions, [summary.id.to_string()]);
    let root = ostra_core::book::book_dir(&ws_root, "app");
    let section = std::fs::read_to_string(root.join("app/greeting.md")).unwrap();
    assert!(section.contains("## Assumptions\n\n- stdout is open."), "{section}");
    assert!(section.contains("```mermaid\nsequenceDiagram"), "{section}");
    assert!(section.find("## Code references") > section.find("```mermaid"), "code references come last");
    let index = std::fs::read_to_string(root.join("index.md")).unwrap();
    assert!(index.contains("- [Greeting](app/greeting.md): Prints a greeting."), "{index}");
    assert!(std::fs::read_to_string(root.join("glossary.md")).unwrap().contains("| Greeting |"));
    let agents: Vec<AgentName> = exec.runs.lock().unwrap().iter().map(|r| r.0).collect();
    assert_eq!(agents, [AgentName::Documentation], "no implementer runs for a DOCS request");
}
