//! The whole stack with a scripted model: real listener, REST with cookie auth, engine, native
//! loop, policy, tools, and git staging. The model plays each agent: it writes its files with the
//! Write tool, then calls its submit tool.

use ostra_core::api::{SessionDetail, SessionStatus, SessionSummary, WorkspaceDetail};
use ostra_core::config::{GlobalConfig, TierTable, save_toml};
use ostra_providers::mock::{response, tool_use_response};
use ostra_providers::{Block, ChatRequest, ChatResponse, ProviderError, Role, ScriptedProvider, StopReason};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static IDS: AtomicU64 = AtomicU64::new(1);

fn id() -> String {
    format!("toolu_e2e_{}", IDS.fetch_add(1, Ordering::SeqCst))
}

fn first_user_text(req: &ChatRequest) -> String {
    req.messages
        .iter()
        .find(|m| m.role == Role::User)
        .map(|m| m.content.iter().filter_map(|b| if let Block::Text { text } = b { Some(text.clone()) } else { None }).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

fn label(text: &str, name: &str) -> String {
    text.lines().find_map(|l| l.strip_prefix(&format!("{name}: "))).unwrap_or_default().trim().to_string()
}

fn assistant_turns(req: &ChatRequest) -> usize {
    req.messages.iter().filter(|m| m.role == Role::Assistant).count()
}

fn judge(req: &ChatRequest) -> Value {
    let schema = &req.tools[0].input_schema;
    let props = &schema["properties"];
    if props.get("category").is_some() {
        json!({"category": "IMPLEMENT", "projects": ["app"], "explore_tasks": [{"project": "app", "task": "Research the greeting"}], "opts_in": {"tests": false, "docs": false}, "reason": "The request changes code."})
    } else if props.get("stakes").is_some() {
        json!({"stakes": "high", "reason": "Plan it."})
    } else if props.get("report_markdown").is_some() {
        json!({"report_markdown": "# Greeting added\n\nEvery stage ran.", "reason": "done"})
    } else {
        match props["answer"]["properties"]["kind"]["const"].as_str() {
            Some("approval") => json!({"answer": {"kind": "approval", "approved": true}, "reason": "The fact-check passed."}),
            _ => json!({"answer": {"kind": "choice", "option": "block"}, "reason": "r"}),
        }
    }
}

fn respond(req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
    let tools: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    if tools == ["decide"] {
        return Ok(tool_use_response(&id(), "decide", judge(req)));
    }
    let submit = tools.iter().find(|t| t.starts_with("submit_")).copied().unwrap_or_default().to_string();
    let text = first_user_text(req);
    let session = PathBuf::from(label(&text, "Session dir"));
    let repo = PathBuf::from(label(&text, "Repo root"));
    let report = label(&text, "Report file");
    let turn = assistant_turns(req);
    let write = |path: &Path, content: &str| tool_use_response(&id(), "Write", json!({"file_path": path, "content": content}));
    let spec = session.join("ostra-spec-1.md");
    let plan = session.join("ostra-plan-1.md");
    let phase = session.join("ostra-plan-1-phase-1-greeting.md");
    let out = match (submit.as_str(), turn) {
        ("submit_explore", 0) => write(&session.join("ostra-research-1.md"), "# Research: greeting\n"),
        ("submit_explore", _) => tool_use_response(&id(), &submit, json!({"research_path": session.join("ostra-research-1.md"), "scope_covered": "s", "findings_summary": "No greeting exists.", "sources_retrieved": 0, "open_questions": 0, "not_covered": []})),
        ("submit_generate_spec", 0) => write(&spec, "# Spec\n"),
        ("submit_generate_spec", _) => tool_use_response(&id(), &submit, json!({"spec_path": spec, "open_questions": [], "external_evidence_rows": 0, "deliverables": 1, "requirements": 1, "summary": "Print a greeting."})),
        ("submit_fact_check", _) => {
            let target = label(&text, "Target type");
            tool_use_response(&id(), &submit, json!({"verdict": "PASS", "target": target, "findings": []}))
        }
        ("submit_plan", 0) => write(&plan, "# Plan\n"),
        ("submit_plan", 1) => write(&phase, "# Phase 1: greeting\n**Complexity:** Low\n"),
        ("submit_plan", _) => tool_use_response(&id(), &submit, json!({"spec_path": spec, "master_plan_path": plan, "phases": [{"id": 1, "deliverable": "D1", "project": "app", "title": "greeting", "complexity": "Low", "test_policy": "Required", "depends_on": [], "file": phase}], "stakes": "High", "summary": "One phase.", "step_count": 1, "requirement_coverage": "1 of 1"})),
        ("submit_implementer", 0) => write(&repo.join("greeting.txt"), "hello\n"),
        ("submit_implementer", 1) => write(Path::new(&report), "# Report\n\nAdded greeting.txt\n"),
        ("submit_implementer", _) => tool_use_response(&id(), &submit, json!({"status": "ok", "report_path": report, "changed_files": ["greeting.txt"], "summary": "Added the greeting."})),
        ("submit_code_reviewer", _) => tool_use_response(&id(), &submit, json!({"findings": [], "security_block": false, "ledger_path": session.join("ostra-review-ledger-phase-1.md"), "summary": "Code review passed"})),
        (other, _) => response(vec![Block::text(format!("unexpected agent {other}"))], StopReason::EndTurn),
    };
    Ok(out)
}

async fn wait_for<F: Fn(&SessionDetail) -> bool>(client: &reqwest::Client, base: &str, id: &str, f: F) -> SessionDetail {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let d: SessionDetail = client.get(format!("{base}/api/sessions/{id}")).send().await.unwrap().json().await.unwrap();
        if f(&d) {
            return d;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out at {:?}: {}", d.summary.status, d.summary.stage_label);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn yolo_implement_session_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // SAFETY: this is the only test in this binary, so no other thread reads the environment.
    unsafe {
        std::env::set_var("OSTRA_CONFIG", root.join("config.toml"));
        std::env::set_var("OSTRA_DATA_DIR", root.join("data"));
    }
    let mut global = GlobalConfig::default();
    global.tiers.insert(
        "native".into(),
        TierTable { fast: Some("mock:m".into()), balanced: Some("mock:m".into()), advanced: Some("mock:m".into()), frontier: Some("mock:m".into()) },
    );
    save_toml(&root.join("config.toml"), &global).unwrap();

    let app_dir = root.join("app");
    std::fs::create_dir_all(app_dir.join(".ostra")).unwrap();
    std::fs::write(app_dir.join(".ostra/INVENTORY.md"), "# app Inventory\n").unwrap();
    std::fs::write(app_dir.join(".ostra/project.toml"), "[commands]\nformat = \"true\"\nbuild = \"true\"\n").unwrap();
    let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&app_dir).output().unwrap();
    git(&["init", "-q"]);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let opts = ostra_server::app::ServeOptions { port: Some(port), open_browser: false, dev: false, exe: PathBuf::from("/nonexistent/ostra"), bind: None, allow_hosts: vec![] };
    let app = ostra_server::app::build(&opts, port).await.unwrap();
    app.shared.providers.register("mock", Arc::new(ScriptedProvider::named("mock").with_responder(respond)));
    let router = ostra_server::api::router(app.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let url = app.auth.sign_in_url().unwrap();
    let token = url.split("#token=").nth(1).unwrap();
    let r = client.post(format!("{base}/api/auth/exchange")).json(&json!({"token": token})).send().await.unwrap();
    assert_eq!(r.status(), 204);

    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "e2e", "root": root.join("ws")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces/{}/projects", ws.id))
        .json(&json!({"path": app_dir, "key": "app", "stack": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ws.projects.len(), 1);
    assert!(ws.validation.is_empty(), "{:?}", ws.validation);

    let s: SessionSummary = client
        .post(format!("{base}/api/workspaces/{}/sessions", ws.id))
        .json(&json!({"request": "Add a greeting file", "options": {"tests": false, "docs": false, "yolo": true}, "projects": []}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let d = wait_for(&client, &base, s.id.as_str(), |d| matches!(d.summary.status, SessionStatus::Completed | SessionStatus::Failed)).await;
    assert_eq!(d.summary.status, SessionStatus::Completed, "{:#?}", d.gates);
    assert!(d.completion.as_deref().unwrap_or_default().contains("Greeting added"));
    assert_eq!(std::fs::read_to_string(app_dir.join("greeting.txt")).unwrap(), "hello\n");
    let staged = String::from_utf8(git(&["diff", "--cached", "--name-only"]).stdout).unwrap();
    assert!(staged.contains("greeting.txt"), "staged: {staged}");
    let agents: Vec<String> = d.executions.iter().map(|e| e.agent.to_string()).collect();
    assert_eq!(agents, ["explore", "generate-spec", "fact-check", "plan", "fact-check", "implementer", "code-reviewer"]);
    let imp = d.executions.iter().find(|e| e.agent == ostra_core::AgentName::Implementer).unwrap();
    let activity: Vec<Value> = client.get(format!("{base}/api/executions/{}/activity", imp.id)).send().await.unwrap().json().await.unwrap();
    // The project write would have asked in default mode; under YOLO the policy allowed it and
    // recorded the rule.
    assert!(
        activity.iter().any(|a| a["delta"]["kind"] == "policy" && a["delta"]["decision"]["rule"]["rule"] == "yolo"),
        "{activity:#?}"
    );
    let art = client.get(reqwest::Url::parse_with_params(&format!("{base}/api/artifacts"), &[("path", imp.report_path.clone().unwrap().display().to_string())]).unwrap()).send().await.unwrap();
    assert_eq!(art.status(), 200);
    let outside = client.get(reqwest::Url::parse_with_params(&format!("{base}/api/artifacts"), &[("path", app_dir.join("greeting.txt").display().to_string())]).unwrap()).send().await.unwrap();
    assert_eq!(outside.status(), 403, "only session files are served");
}
