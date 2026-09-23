use ostra_core::agent::AgentName;
use ostra_core::api::{InitStatus, PushKeys, PushSubscription, SessionStatus};
use ostra_core::event::{
    AnswerSource, ExecPurpose, GateAnswer, GatePayload, JudgeKind, SessionEvent, SessionKind, SessionOptions,
};
use ostra_core::exec::{ExecutionDelta, ExecutionResult, ExecutionStatus, Usage};
use ostra_core::executor::{ExecutorKind, HarnessKind};
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
use ostra_core::pipeline::{Category, Lane, StageKind};
use ostra_core::policy::{PolicyDecision, RuleRef, ToolCall};
use ostra_store::*;
use std::path::PathBuf;

fn db() -> (tempfile::TempDir, WorkspaceDb) {
    let dir = tempfile::tempdir().unwrap();
    let db = WorkspaceDb::open(&dir.path().join(".ostra/workspace.db")).unwrap();
    db.set_workspace_id(&WorkspaceId::from("ws_1")).unwrap();
    (dir, db)
}

fn new_session(db: &WorkspaceDb) -> SessionId {
    let id = SessionId::new();
    db.create_session(&NewSession {
        id: id.clone(),
        kind: SessionKind::Pipeline,
        request: "add cancel".into(),
        category: None,
        projects: vec!["backend".into()],
        yolo: false,
    })
    .unwrap();
    id
}

fn new_exec(db: &WorkspaceDb, session: Option<&SessionId>, executor: ExecutorKind) -> ExecutionId {
    let id = ExecutionId::new();
    db.insert_execution(&NewExecution {
        id: id.clone(),
        session: session.cloned(),
        agent: AgentName::Implementer,
        purpose: Some(ExecPurpose::Implement { phase: 1, work: ostra_core::event::WorkKind::Initial }),
        stage: Some(StageKind::Implement),
        project: "backend".into(),
        executor,
        model: "m".into(),
        params: serde_json::json!({"report_file": "/r.md"}),
        spawn_block: "Repo key: backend".into(),
        report_path: Some(PathBuf::from("/r.md")),
        native_session_id: None,
    })
    .unwrap();
    id
}

#[test]
fn sessions_create_update_list() {
    let (_d, db) = db();
    let id = new_session(&db);
    let s = db.get_session(&id).unwrap().unwrap();
    assert_eq!(s.workspace.as_str(), "ws_1");
    assert_eq!(s.status, SessionStatus::Running);
    assert_eq!(s.lane, Lane::Research);
    let s = db
        .update_session(
            &id,
            &SessionUpdate {
                category: Some(Some(Category::Implement)),
                status: Some(SessionStatus::Waiting),
                lane: Some(Lane::Build),
                stage_label: Some("Phase 2".into()),
                cost_usd: Some(1.5),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(s.category, Some(Category::Implement));
    assert_eq!(s.stage_label, "Phase 2");
    assert_eq!(s.cost_usd, 1.5);
    let second = new_session(&db);
    let all = db.list_sessions().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, second);
    assert!(db.update_session(&SessionId::from("nope"), &SessionUpdate::default()).is_err());
}

#[test]
fn session_title_is_stored() {
    let (_d, db) = db();
    let id = new_session(&db);
    assert_eq!(db.get_session(&id).unwrap().unwrap().title, None);
    let s = db.update_session(&id, &SessionUpdate { title: Some(Some("Order cancellation".into())), ..Default::default() }).unwrap();
    assert_eq!(s.title.as_deref(), Some("Order cancellation"));
    let s = db.update_session(&id, &SessionUpdate { cost_usd: Some(1.0), ..Default::default() }).unwrap();
    assert_eq!(s.title.as_deref(), Some("Order cancellation"), "an update without a title keeps it");
    assert_eq!(db.list_sessions().unwrap()[0].title.as_deref(), Some("Order cancellation"));
}

#[test]
fn execution_view_carries_group_stream_and_summary() {
    let (_d, db) = db();
    let s = new_session(&db);
    let native = new_exec(&db, Some(&s), ExecutorKind::Native);
    let harness = new_exec(&db, Some(&s), ExecutorKind::Harness(HarnessKind::Claude));
    let v = db.get_execution(&native).unwrap().unwrap();
    assert_eq!(v.group, "implementer:backend");
    assert_eq!(v.run_label, "Phase 1");
    assert_eq!(v.stream, ostra_core::executor::ExecStream::Activity);
    assert_eq!(v.summary, None);
    assert!(!v.has_transcript);
    db.set_execution_summary(&native, "Edit src/lib.rs").unwrap();
    assert_eq!(db.get_execution(&native).unwrap().unwrap().summary.as_deref(), Some("Edit src/lib.rs"));
    let h = db.get_execution(&harness).unwrap().unwrap();
    assert_eq!(h.stream, ostra_core::executor::ExecStream::Terminal);
    assert_eq!(db.list_executions(&s).unwrap()[0].summary.as_deref(), Some("Edit src/lib.rs"));
}

#[test]
fn a_version_one_database_gains_the_new_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE sessions (id TEXT PRIMARY KEY, kind TEXT NOT NULL, request TEXT NOT NULL, category TEXT, status TEXT NOT NULL, lane TEXT NOT NULL, stage_label TEXT NOT NULL, projects TEXT NOT NULL, yolo INTEGER NOT NULL, cost_usd REAL NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL); CREATE TABLE executions (id TEXT PRIMARY KEY); CREATE TABLE gates (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, answer TEXT); PRAGMA user_version = 1;").unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES ('s_old', '{\"kind\":\"pipeline\"}', 'r', NULL, 'running', 'research', 'Intake', '[]', 0, 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
            [],
        )
        .unwrap();
    }
    let db = WorkspaceDb::open(&path).unwrap();
    let s = db.get_session(&SessionId::from("s_old")).unwrap().unwrap();
    assert_eq!(s.title, None);
}

#[test]
fn events_are_sequenced_per_session() {
    let (_d, db) = db();
    let a = new_session(&db);
    let b = new_session(&db);
    let ev = SessionEvent::SessionCreated {
        kind: SessionKind::Pipeline,
        request: "x".into(),
        options: SessionOptions::default(),
        projects: vec![],
        workspace_root: "/w".into(),
        session_root: "/w/.ostra/sessions/a".into(),
    };
    assert_eq!(db.append_event(&a, &ev).unwrap().seq, 1);
    assert_eq!(db.append_event(&a, &SessionEvent::YoloSet { enabled: true }).unwrap().seq, 2);
    assert_eq!(db.append_event(&b, &ev).unwrap().seq, 1);
    let all = db.events(&a).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].event, SessionEvent::YoloSet { enabled: true });
    let after = db.events_after(&a, 1).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].seq, 2);
}

#[test]
fn concurrent_appends_get_distinct_seqs() {
    let (_d, db) = db();
    let s = new_session(&db);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let db = db.clone();
            let s = s.clone();
            std::thread::spawn(move || {
                for _ in 0..10 {
                    db.append_event(&s, &SessionEvent::Note { message: "n".into() }).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let seqs: Vec<i64> = db.events(&s).unwrap().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=80).collect::<Vec<_>>());
}

#[test]
fn executions_lifecycle_and_restart() {
    let (_d, db) = db();
    let s = new_session(&db);
    let native = new_exec(&db, Some(&s), ExecutorKind::Native);
    let harness = new_exec(&db, Some(&s), ExecutorKind::Harness(HarnessKind::Codex));
    let side = new_exec(&db, None, ExecutorKind::Native);
    let v = db.get_execution(&native).unwrap().unwrap();
    assert_eq!(v.status, ExecutionStatus::Running);
    assert_eq!(v.agent, AgentName::Implementer);
    assert!(!v.can_resume);

    db.set_native_session_id(&harness, "abc").unwrap();
    assert!(db.get_execution(&harness).unwrap().unwrap().can_resume);

    let usage = Usage { input_tokens: 10, cache_read_tokens: 100, tool_calls: 4, cost_usd: 0.25, ..Default::default() };
    db.update_execution_usage(&native, &usage).unwrap();
    let result = ExecutionResult {
        status: ExecutionStatus::Ok,
        submit: Some(serde_json::json!({"status": "ok"})),
        final_text: "done".into(),
        usage,
        native_session_id: None,
        error: None,
    };
    let v = db.finish_execution(&native, &result).unwrap();
    assert_eq!(v.status, ExecutionStatus::Ok);
    assert!(v.ended_at.is_some());
    assert_eq!(v.usage.cost_usd, 0.25);
    let (params, out) = db.execution_output(&native).unwrap().unwrap();
    assert_eq!(params["report_file"], "/r.md");
    assert_eq!(out.submit, result.submit);
    assert_eq!(out.final_text, "done");

    assert_eq!(db.list_executions(&s).unwrap().len(), 2);
    assert_eq!(db.list_sessionless_executions(10).unwrap()[0].id, side);
    let interrupted = db.mark_running_interrupted().unwrap();
    assert_eq!(interrupted.len(), 2);
    assert!(db.running_executions().unwrap().is_empty());
    assert_eq!(db.get_execution(&harness).unwrap().unwrap().status, ExecutionStatus::Interrupted);
}

#[test]
fn messages_and_activity() {
    let (_d, db) = db();
    let e = new_exec(&db, None, ExecutorKind::Native);
    assert_eq!(db.append_message(&e, "user", &serde_json::json!([{"type":"text","text":"hi"}])).unwrap(), 1);
    assert_eq!(db.append_message(&e, "assistant", &serde_json::json!([])).unwrap(), 2);
    let m = db.messages(&e).unwrap();
    assert_eq!(m[0].role, "user");
    assert_eq!(m[1].seq, 2);

    let a = db.append_activity(&e, &ExecutionDelta::Text { text: "a".into() }).unwrap();
    assert_eq!(a.seq, 1);
    db.append_activity(&e, &ExecutionDelta::Status { message: "b".into() }).unwrap();
    let after = db.activity_after(&e, 1).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].delta, ExecutionDelta::Status { message: "b".into() });
}

#[test]
fn gates_open_and_answer() {
    let (_d, db) = db();
    let s = new_session(&db);
    let g = GateId::new();
    let payload = GatePayload::PhaseBlocked { project: "backend".into(), phase: 2, reason: "cap".into() };
    db.upsert_gate(&s, &g, "Phase 2 blocked", "why", &payload).unwrap();
    db.upsert_gate(&s, &g, "Phase 2 blocked again", "why", &payload).unwrap();
    assert_eq!(db.open_gates(Some(&s)).unwrap().len(), 1);
    assert_eq!(db.open_gates(None).unwrap()[0].title, "Phase 2 blocked again");
    assert_eq!(db.get_session(&s).unwrap().unwrap().open_gates, 1);
    let answer = GateAnswer::Choice { option: "leave".into(), text: None };
    let v = db.answer_gate(&g, AnswerSource::User, &answer, None).unwrap();
    assert_eq!(v.answer, Some(answer));
    assert_eq!(v.source, Some(AnswerSource::User));
    assert!(v.answered_at.is_some());
    assert!(db.open_gates(Some(&s)).unwrap().is_empty());
    assert_eq!(db.gates(&s).unwrap().len(), 1);
    assert_eq!(db.get_session(&s).unwrap().unwrap().open_gates, 0);
}

#[test]
fn decisions_override() {
    let (_d, db) = db();
    let s = new_session(&db);
    let d = DecisionId::new();
    db.insert_decision(&s, &d, JudgeKind::Stakes, None, "request", &serde_json::json!({"stakes":"low"}), "small").unwrap();
    let v = db.get_decision(&d).unwrap().unwrap();
    assert!(!v.overridden && v.can_override);
    db.set_can_override(&d, false).unwrap();
    let v = db.mark_overridden(&d, &serde_json::json!({"stakes":"high"}), "user").unwrap();
    assert!(v.overridden && !v.can_override);
    assert_eq!(v.output["stakes"], "high");
    assert_eq!(db.decisions(&s).unwrap().len(), 1);
    assert_eq!(db.decision_session(&d).unwrap(), Some(s));
}

#[test]
fn tool_calls_truncate_output() {
    let (_d, db) = db();
    let e = new_exec(&db, None, ExecutorKind::Native);
    db.record_tool_call(&ToolCallRecord {
        execution: e.clone(),
        call_id: "c1".into(),
        tool: "Bash".into(),
        input: serde_json::json!({"command": "ls"}),
        decision: Some(PolicyDecision::deny(RuleRef::guard("write-scope"), "no")),
        rule: Some("write-scope".into()),
        duration_ms: Some(3),
        output: Some("x".repeat(20_000)),
        at: None,
    })
    .unwrap();
    let calls = db.tool_calls(&e).unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].output.as_ref().unwrap().len() < 9_000);
    assert!(calls[0].decision.as_ref().unwrap().is_deny());
    let _ = ToolCall::new("Bash", serde_json::json!({}));
}

#[test]
fn projects_crud() {
    let (_d, db) = db();
    db.upsert_project(&ProjectRow {
        key: "web".into(),
        path: "/code/web".into(),
        init_status: InitStatus::NotInitialized,
        stack: Some("typescript-node".into()),
    })
    .unwrap();
    assert!(db.set_project_init_status("web", InitStatus::Initialized).unwrap());
    assert_eq!(db.get_project("web").unwrap().unwrap().init_status, InitStatus::Initialized);
    assert_eq!(db.list_projects().unwrap().len(), 1);
    assert!(db.delete_project("web").unwrap());
    assert!(db.get_project("web").unwrap().is_none());
}

#[test]
fn cost_report_groups() {
    let (_d, db) = db();
    let s = new_session(&db);
    for (cost, calls) in [(1.0, 4u64), (2.0, 6u64)] {
        let e = new_exec(&db, Some(&s), ExecutorKind::Native);
        let usage = Usage { cache_read_tokens: 1000, tool_calls: calls, cost_usd: cost, ..Default::default() };
        db.finish_execution(
            &e,
            &ExecutionResult {
                status: ExecutionStatus::Ok,
                submit: None,
                final_text: String::new(),
                usage,
                native_session_id: None,
                error: None,
            },
        )
        .unwrap();
    }
    new_exec(&db, None, ExecutorKind::Harness(HarnessKind::Claude));
    let r = db.cost_report(None).unwrap();
    assert_eq!(r.since, None);
    assert_eq!(r.total.executions, 3);
    let hour = chrono::Duration::hours(1);
    let recent = db.cost_report(Some(chrono::Utc::now() - hour)).unwrap();
    assert_eq!((recent.total.executions, recent.by_agent.len()), (3, 1));
    let later = db.cost_report(Some(chrono::Utc::now() + hour)).unwrap();
    assert_eq!((later.total.executions, later.total.usage.cost_usd), (0, 0.0));
    assert!(later.by_session.is_empty() && later.since.is_some());
    assert_eq!(r.total.usage.cost_usd, 3.0);
    assert_eq!(r.total.cache_reads_per_tool_call, 200.0);
    assert_eq!(r.by_session[0].key, s.as_str());
    assert!(r.by_session.iter().any(|row| row.key == "side-panel"));
    assert_eq!(r.by_agent[0].key, "implementer");
    assert_eq!(r.by_executor.len(), 2);
    assert_eq!(r.by_stage[0].key, "implement");
}

#[test]
fn reopen_keeps_data_and_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.db");
    let s = {
        let db = WorkspaceDb::open(&path).unwrap();
        new_session(&db)
    };
    let db = WorkspaceDb::open(&path).unwrap();
    assert!(db.get_session(&s).unwrap().is_some());
}

#[test]
fn registry_workspaces_push_kv() {
    let dir = tempfile::tempdir().unwrap();
    let reg = RegistryDb::open(&dir.path().join("registry.db")).unwrap();
    let id = WorkspaceId::new();
    reg.add_workspace(&id, "shop", std::path::Path::new("/w/shop")).unwrap();
    assert!(reg.add_workspace(&WorkspaceId::new(), "dup", std::path::Path::new("/w/shop")).is_err());
    assert_eq!(reg.list_workspaces().unwrap().len(), 1);
    assert_eq!(reg.workspace_by_root(std::path::Path::new("/w/shop")).unwrap().unwrap().id, id);
    assert!(reg.rename_workspace(&id, "store").unwrap());
    assert_eq!(reg.get_workspace(&id).unwrap().unwrap().name, "store");

    let sub = PushSubscription {
        endpoint: "https://push.example/1".into(),
        keys: PushKeys { p256dh: "p".into(), auth: "a".into() },
    };
    reg.add_push_subscription(&sub, Some(&id)).unwrap();
    reg.add_push_subscription(&sub, None).unwrap();
    let subs = reg.list_push_subscriptions().unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].workspace, None);
    assert!(reg.remove_push_subscription(&sub.endpoint).unwrap());

    reg.kv_set("vapid", &[1, 2, 3]).unwrap();
    assert_eq!(reg.kv_get("vapid").unwrap(), Some(vec![1, 2, 3]));
    assert!(reg.kv_delete("vapid").unwrap());
    assert_eq!(reg.kv_get("vapid").unwrap(), None);
    assert!(reg.remove_workspace(&id).unwrap());
}

#[test]
fn registry_onboarding_keeps_the_first_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.db");
    let first = {
        let reg = RegistryDb::open(&path).unwrap();
        assert_eq!(reg.onboarded_at().unwrap(), None);
        let first = reg.mark_onboarded().unwrap();
        assert_eq!(reg.onboarded_at().unwrap(), Some(first));
        first
    };
    let reg = RegistryDb::open(&path).unwrap();
    assert_eq!(reg.onboarded_at().unwrap(), Some(first));
    assert_eq!(reg.mark_onboarded().unwrap(), first);
}

#[test]
fn meta_values_round_trip() {
    let (_dir, db) = db();
    assert_eq!(db.meta_get("ui_state").unwrap(), None);
    db.meta_set("ui_state", "{\"a\":1}").unwrap();
    db.meta_set("ui_state", "{\"a\":2}").unwrap();
    assert_eq!(db.meta_get("ui_state").unwrap().as_deref(), Some("{\"a\":2}"));
}

#[test]
fn text_search_over_sessions_and_artifacts() {
    let (_d, db) = db();
    let id = new_session(&db);
    let hits = db.search_text("canc", 10).unwrap();
    assert_eq!(hits.iter().map(|h| (h.kind.as_str(), h.reference.as_str())).collect::<Vec<_>>(), [("session", id.as_str())]);
    assert!(db.search_text("order", 10).unwrap().is_empty());

    db.update_session(&id, &SessionUpdate { title: Some(Some("Order cancellation".into())), ..Default::default() }).unwrap();
    let hits = db.search_text("ord canc", 10).unwrap();
    assert_eq!(hits.len(), 1, "every word must match, as a prefix");
    assert_eq!(hits[0].label, "Order cancellation");
    assert_eq!(hits[0].body, "add cancel");

    db.index_artifact(&id, "/s/ostra-spec-1.md", "Spec", "Requirements\nRefund rules", 10).unwrap();
    db.index_artifact(&id, "/s/ostra-spec-1.md", "Spec", "Requirements\nRefund policy", 20).unwrap();
    let hits = db.search_text("refund", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].kind.as_str(), hits[0].reference.as_str(), hits[0].session.clone()), ("artifact", "/s/ostra-spec-1.md", id.clone()));
    assert!(db.search_text("rules", 10).unwrap().is_empty(), "a reindex replaces the old text");
    let indexed = db.indexed_artifacts(&id).unwrap();
    assert_eq!(indexed.get("/s/ostra-spec-1.md"), Some(&("Spec".to_string(), 20)));
    assert!(db.search_text("\"zz*) OR (", 10).unwrap().is_empty(), "query syntax in the text is quoted away");
}

#[test]
fn search_index_backfills_sessions_from_before_the_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    {
        let db = WorkspaceDb::open(&path).unwrap();
        db.set_workspace_id(&WorkspaceId::from("ws_1")).unwrap();
        new_session(&db);
    }
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE search_fts; DROP TABLE search_docs; DROP TRIGGER sessions_search_ai; DROP TRIGGER sessions_search_au; PRAGMA user_version = 2;").unwrap();
    drop(conn);
    let db = WorkspaceDb::open(&path).unwrap();
    assert_eq!(db.search_text("cancel", 10).unwrap().len(), 1);
}

#[test]
fn spend_since_sums_executions_started_in_the_window() {
    let (_d, db) = db();
    let s = new_session(&db);
    let e = new_exec(&db, Some(&s), ExecutorKind::Native);
    db.update_execution_usage(&e, &Usage { cost_usd: 1.25, ..Default::default() }).unwrap();
    let hour = chrono::Duration::hours(1);
    assert_eq!(db.spend_since(chrono::Utc::now() - hour).unwrap(), 1.25);
    assert_eq!(db.spend_since(chrono::Utc::now() + hour).unwrap(), 0.0);
}
