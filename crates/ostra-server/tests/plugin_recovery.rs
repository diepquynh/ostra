//! Rule PL8 end to end: a workspace plugin program that exits during a stage decision and again in
//! the middle of a programmatic run. Ostra starts it again each time, the new process reads the
//! checkpoints the old one saved, and the session completes with one run of the agent.
//!
//! The test binary starts itself again as the plugin, with `OSTRA_TEST_PLUGIN` set.

use ostra_core::api::WorkspaceDetail;
use ostra_core::api::{PluginInfo, PluginState, SessionDetail, SessionStatus, SessionSummary};
use ostra_core::config::{
    GlobalConfig, TierTable, WorkspaceSettings, load_toml_required, save_toml,
};
use ostra_providers::mock::{response, tool_use_response};
use ostra_providers::{
    Block, ChatRequest, ChatResponse, ProviderError, Role, ScriptedProvider, StopReason,
};
use ostra_sdk::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

struct Phoenix;

#[async_trait::async_trait]
impl Plugin for Phoenix {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            name: "phoenix".into(),
            version: "1.0.0".into(),
            agents: vec![
                PluginAgent::new("rebuilder", "Reads the changelog, exits once, then passes.")
                    .capabilities([Capability::Read]),
            ],
            stages: vec![PluginStage {
                name: "gate".into(),
                description: "Exits on its first decision, then runs the rebuilder.".into(),
            }],
            ..Default::default()
        }
    }

    async fn decide_stage(
        &self,
        _: &str,
        view: StageView,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<StageDecision, String> {
        if checkpoints.get("decide-crashed").is_none() {
            checkpoints.save("decide-crashed", json!(true)).await?;
            std::process::exit(3);
        }
        Ok(match view.runs.last() {
            None => StageDecision::Run {
                agent: "rebuilder".into(),
                instructions: None,
            },
            Some(r) => StageDecision::Pass {
                summary: r
                    .submit
                    .as_ref()
                    .and_then(|s| s["summary"].as_str())
                    .unwrap_or("no summary")
                    .to_string(),
            },
        })
    }

    async fn run_agent(
        &self,
        _: &str,
        task: AgentTask,
        calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<AgentOutcome, String> {
        let ctx = AgentContext::new(calls);
        match checkpoints.get("rebuild") {
            None => {
                let file = ctx
                    .read(&format!("{}/CHANGELOG.md", task.repo_root.display()))
                    .await;
                checkpoints
                    .save(
                        "rebuild",
                        json!({"lines": file.output.lines().count(), "execution": task.execution}),
                    )
                    .await?;
                std::process::exit(4)
            }
            Some(saved) => Ok(pass(format!(
                "resumed={} lines={} same_execution={}",
                task.resumed,
                saved["lines"],
                saved["execution"] == json!(task.execution)
            ))),
        }
    }
}

#[test]
fn plugin_main() {
    if std::env::var_os("OSTRA_TEST_PLUGIN").is_none() {
        return;
    }
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(stdio::serve(Arc::new(Phoenix)));
}

static IDS: AtomicU64 = AtomicU64::new(1);

fn id() -> String {
    format!("toolu_pr_{}", IDS.fetch_add(1, Ordering::SeqCst))
}

fn session_dir(req: &ChatRequest) -> PathBuf {
    let text: String = req
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    PathBuf::from(
        text.lines()
            .find_map(|l| l.strip_prefix("Session dir: "))
            .unwrap_or_default()
            .trim(),
    )
}

fn respond(req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
    let tools: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    if tools == ["decide"] {
        let props = &req.tools[0].input_schema["properties"];
        let out = if props.get("category").is_some() {
            json!({"category": "IMPLEMENT", "projects": ["app"], "explore_tasks": [{"project": "app", "task": "Research the changelog"}], "opts_in": {"tests": false, "docs": false}, "reason": "r", "title": "Release"})
        } else if props.get("report_markdown").is_some() {
            json!({"report_markdown": "# Released\n\nEvery stage ran.", "reason": "done"})
        } else {
            json!({"sufficient": true, "reason": "r", "tasks": []})
        };
        return Ok(tool_use_response(&id(), "decide", out));
    }
    let research = session_dir(req).join("ostra-research-1.md");
    let turns = req
        .messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .count();
    Ok(match (tools.contains(&"submit_explore"), turns) {
        (true, 0) => tool_use_response(
            &id(),
            "Document",
            json!({"path": research, "document": {"title": "Changelog", "date": "2026-10-04", "repo": "app", "scope": "The changelog.", "problem": "None."}}),
        ),
        (true, _) => tool_use_response(
            &id(),
            "submit_explore",
            json!({"research_path": research, "scope_covered": "s", "findings_summary": "The changelog exists.", "sources_retrieved": 0, "open_questions": 0, "not_covered": []}),
        ),
        _ => response(vec![Block::text("unexpected")], StopReason::EndTurn),
    })
}

const WORKFLOW: &str = "extends = \"research\"\ndescription = \"Research, then the phoenix gate.\"\n\n[[stage]]\nid = \"phoenix-gate\"\nplugin = \"phoenix:gate\"\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pl8_a_plugin_program_that_stops_resumes_from_its_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // SAFETY: this binary's other test returns at once without the plugin variable, so nothing
    // else reads the environment meanwhile.
    unsafe {
        std::env::set_var("OSTRA_CONFIG", root.join("config.toml"));
        std::env::set_var("OSTRA_DATA_DIR", root.join("data"));
        std::env::set_var("OSTRA_MASTER_KEY_FILE", root.join("master.key"));
        std::env::set_var("OSTRA_SANDBOX_CACHE", root.join("sandbox-cache"));
        std::env::set_var("OSTRA_MODELS_DEV_URL", "");
    }
    let mut global = GlobalConfig::default();
    global.server.use_ip_host = true;
    global.tiers.insert(
        "native".into(),
        TierTable {
            fast: Some("mock:m".into()),
            balanced: Some("mock:m".into()),
            advanced: Some("mock:m".into()),
            frontier: Some("mock:m".into()),
        },
    );
    save_toml(&root.join("config.toml"), &global).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let opts = ostra_server::app::ServeOptions {
        port: Some(port),
        open_browser: false,
        dev: false,
        exe: PathBuf::from("/nonexistent/ostra"),
        bind: None,
        allow_hosts: vec![],
        plugins: Registry::new(),
    };
    let app = ostra_server::app::build(&opts, port).await.unwrap();
    app.shared.providers.register(
        "mock",
        Arc::new(ScriptedProvider::named("mock").with_responder(respond)),
    );
    let router = ostra_server::api::router(app.clone())
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let url = app.auth.sign_in_url().unwrap();
    let token = url.split("#token=").nth(1).unwrap();
    let r = client
        .post(format!("{base}/api/auth/exchange"))
        .json(&json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);

    let app_dir = root.join("app");
    std::fs::create_dir_all(app_dir.join(".ostra")).unwrap();
    std::fs::write(app_dir.join(".ostra/INVENTORY.md"), "# app Inventory\n").unwrap();
    std::fs::write(app_dir.join("CHANGELOG.md"), "# Changes\n- one\n- two\n").unwrap();
    let ws_root = root.join("ws");
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "release", "root": ws_root}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let wsp = format!("{base}/api/workspaces/{}", ws.id);
    let _: Value = client
        .post(format!("{wsp}/projects"))
        .json(&json!({"path": app_dir, "key": "app", "stack": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let toml_path = ostra_core::paths::workspace_toml(&ws_root);
    let mut settings: WorkspaceSettings = load_toml_required(&toml_path).unwrap();
    settings.plugins.push(ostra_core::plugin::PluginConfig {
        name: "phoenix".into(),
        command: vec![
            std::env::current_exe().unwrap().display().to_string(),
            "--exact".into(),
            "plugin_main".into(),
            "--quiet".into(),
            "--test-threads=1".into(),
        ],
        env: [("OSTRA_TEST_PLUGIN".to_string(), "1".to_string())].into(),
        enabled: true,
        timeout_secs: 30,
    });
    save_toml(&toml_path, &settings).unwrap();
    let workflows = ostra_core::paths::workspace_workflows_dir(&ws_root);
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("phoenix.toml"), WORKFLOW).unwrap();

    // Rule PL1: the program starts once the workspace file is approved.
    let pending: WorkspaceDetail = client.get(&wsp).send().await.unwrap().json().await.unwrap();
    let waits = pending
        .pending_commands
        .iter()
        .find(|p| p.project.is_none())
        .expect("the workspace file waits for approval");
    let approved = client
        .post(format!("{wsp}/approve"))
        .json(&json!({"hash": waits.hash}))
        .send()
        .await
        .unwrap();
    assert!(approved.status().is_success());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let plugins: Vec<PluginInfo> = client
            .get(format!("{wsp}/plugins"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if plugins
            .iter()
            .any(|p| p.name == "phoenix" && p.state == PluginState::Running)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the plugin did not start: {plugins:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let s: SessionSummary = client
        .post(format!("{wsp}/sessions"))
        .json(&json!({"request": "Check the release", "workflow": "phoenix", "options": {"yolo": true}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let done = loop {
        let d: SessionDetail = client
            .get(format!("{base}/api/sessions/{}", s.id))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if matches!(
            d.summary.status,
            SessionStatus::Completed | SessionStatus::Failed
        ) {
            break d;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out at {:?}: {}",
            d.summary.status,
            d.summary.stage_label
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let st = app
        .all_workspaces()
        .into_iter()
        .next()
        .unwrap()
        .engine
        .state(&s.id)
        .unwrap();
    assert_eq!(
        done.summary.status,
        SessionStatus::Completed,
        "{:?} {:#?}",
        st.failed,
        st.executions
            .values()
            .map(|r| (
                r.agent.to_string(),
                r.result.as_ref().map(|x| (x.status, x.error.clone()))
            ))
            .collect::<Vec<_>>()
    );
    let runs: Vec<_> = st
        .executions
        .values()
        .filter(|r| r.agent.as_str() == "rebuilder")
        .collect();
    assert_eq!(runs.len(), 1, "the restarted run kept its execution");
    let gate = st.stage_track("phoenix-gate", None).unwrap();
    assert_eq!(
        gate.outcome,
        Some(ostra_engine::workflow::StageOutcome::Passed)
    );
    let kinds: Vec<&str> = gate
        .decisions
        .iter()
        .map(|d| match d {
            StageDecision::Run { .. } => "run",
            StageDecision::Pass { .. } => "pass",
            StageDecision::Fail { .. } => "fail",
            StageDecision::Ask { .. } => "ask",
        })
        .collect();
    assert_eq!(
        kinds,
        ["run", "pass"],
        "the decision the stopped program never gave was asked again, not recorded as a failure"
    );
    let summary = runs[0].result.as_ref().unwrap().submit.as_ref().unwrap()["summary"].clone();
    assert_eq!(
        summary,
        json!("resumed=true lines=3 same_execution=true"),
        "the second process read what the first one saved"
    );
    let kept = st.plugin_checkpoints.get("phoenix").unwrap();
    assert_eq!(kept.get("decide-crashed"), Some(&json!(true)));
    assert!(kept.contains_key("rebuild"));
}
