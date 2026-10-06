//! The Workflow builder and the agent screen over HTTP (HANDOVER 10.11 and 9.5): agents and
//! workflows saved through the API keep the workspace approved, broken saves are refused with the
//! fix, and a session runs a builder workflow whose transform and condition pick its branch.

use ostra_core::api::{
    AgentDetail, BuilderPalette, PluginInfo, SessionDetail, SessionStatus, SessionSummary,
    WorkflowDoc, WorkspaceDetail,
};
use ostra_core::config::{GlobalConfig, TierTable, save_toml};
use ostra_providers::mock::{response, tool_use_response};
use ostra_providers::{
    Block, ChatRequest, ChatResponse, ProviderError, Role, ScriptedProvider, StopReason,
};
use ostra_sdk::workflow::{Node, TransformFn, Workflow};
use ostra_sdk::{Plugin, PluginManifest, Registry, ValueKind};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static IDS: AtomicU64 = AtomicU64::new(1);

fn id() -> String {
    format!("toolu_pl_{}", IDS.fetch_add(1, Ordering::SeqCst))
}

fn first_user_text(req: &ChatRequest) -> String {
    req.messages
        .iter()
        .find(|m| m.role == Role::User)
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| match b {
                    Block::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn label(text: &str, name: &str) -> String {
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{name}: ")))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn respond(req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
    let tools: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    if tools == ["decide"] {
        let props = &req.tools[0].input_schema["properties"];
        let out = if props.get("category").is_some() {
            json!({"category": "RESEARCH", "projects": ["app"], "explore_tasks": [{"project": "app", "task": "Research the changelog"}], "opts_in": {"tests": false, "docs": false}, "reason": "r", "title": "Audit"})
        } else if props.get("report_markdown").is_some() {
            json!({"report_markdown": "# Done", "reason": "done"})
        } else if props.get("advice").is_some() {
            // Rule WB3: the prompt node is shown its inputs.
            let text = first_user_text(req);
            assert!(text.contains("\"risk\": \"low\""), "{text}");
            json!({"advice": "Ship it."})
        } else {
            json!({"sufficient": true, "reason": "r", "tasks": []})
        };
        return Ok(tool_use_response(&id(), "decide", out));
    }
    let submit = tools
        .iter()
        .find(|t| t.starts_with("submit_"))
        .copied()
        .unwrap_or_default()
        .to_string();
    let text = first_user_text(req);
    let session = PathBuf::from(label(&text, "Session dir"));
    let research = session.join("ostra-research-1.md");
    let turns = req
        .messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .count();
    Ok(match (submit.as_str(), turns) {
        ("submit_explore", 0) => tool_use_response(
            &id(),
            "Document",
            json!({"path": research, "document": {"title": "Changelog", "date": "2026-10-03", "repo": "app", "scope": "The changelog.", "problem": "None."}}),
        ),
        ("submit_explore", _) => tool_use_response(
            &id(),
            &submit,
            json!({"research_path": research, "scope_covered": "s", "findings_summary": "The changelog exists.", "sources_retrieved": 0, "open_questions": 0, "not_covered": []}),
        ),
        ("submit_auditor", _) => {
            assert_eq!(
                label(&text, "Stage"),
                "audit",
                "only the audit runs: {text}"
            );
            tool_use_response(
                &id(),
                &submit,
                json!({"verdict": "pass", "summary": "No secret.", "data": {"risk": "low"}}),
            )
        }
        (other, _) => response(
            vec![Block::text(format!("unexpected agent {other}"))],
            StopReason::EndTurn,
        ),
    })
}

async fn wait_for<F: Fn(&SessionDetail) -> bool>(
    client: &reqwest::Client,
    base: &str,
    id: &str,
    f: F,
) -> SessionDetail {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let d: SessionDetail = client
            .get(format!("{base}/api/sessions/{id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if f(&d) {
            return d;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out at {:?}: {}",
            d.summary.status,
            d.summary.stage_label
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Rules PL6 and PL7: a plugin with a workflow and a transform function built in Rust.
struct Lib;

#[async_trait::async_trait]
impl Plugin for Lib {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            name: "lib".into(),
            workflows: vec![
                Workflow::extending("flow", "ostra:research")
                    .description("Research, then a number doubled in code.")
                    .node(Node::transform("n", "constant").arg("value", json!(21)))
                    .node(Node::transform("twice", "lib:double").input("value", "n.output"))
                    .build(),
            ],
            transforms: vec![
                TransformFn::new("double", "Twice a number.")
                    .input("value", ValueKind::Number, "The number.")
                    .output(ValueKind::Number)
                    .build(),
            ],
            ..Default::default()
        }
    }

    async fn transform(
        &self,
        _: &str,
        inputs: serde_json::Map<String, Value>,
        _: serde_json::Map<String, Value>,
    ) -> Result<Value, String> {
        Ok(json!(inputs["value"].as_i64().unwrap_or(0) * 2))
    }
}

fn auditor_doc() -> Value {
    json!({
        "name": "auditor",
        "description": "Audits the changelog for secrets.",
        "returns": "stage",
        "capabilities": ["read", "search_text"],
        "brief": [],
        "effort": {},
        "helper": false,
        "data_schema": {"type": "object", "required": ["risk"], "properties": {"risk": {"type": "string", "enum": ["low", "high"]}}},
        "prompt": "Read the changelog and report any secret in it, with {{ tool_read }}.\n"
    })
}

/// A research workflow as the builder saves it: every node written out, a transform that picks
/// the audit's risk, and two branches on it.
fn builder_workflow() -> Value {
    json!({
        "description": "Research, an audit, and a branch on its risk.",
        "base": "research",
        "layout": {"audit": [200.0, 40.0]},
        "stage": [
            {"id": "research", "uses": "ostra:research", "after": []},
            {"id": "audit", "agent": "auditor", "after": ["research"]},
            {"id": "risk", "transform": "risk-of", "after": ["audit"],
             "inputs": {"audit": "audit.output"}},
            {"id": "advise", "prompt": "Say whether to ship.", "tier": "fast", "after": ["risk"],
             "inputs": {"risk": "risk.output"},
             "when": [{"ref": "risk.output", "op": "eq", "value": "low"}],
             "output_schema": {"type": "object", "required": ["advice"], "properties": {"advice": {"type": "string"}}}},
            {"id": "escalate", "agent": "auditor", "after": ["risk"],
             "when": [{"ref": "risk.output", "op": "eq", "value": "high"}]}
        ]
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_builder_saves_agents_and_workflows_and_a_session_runs_the_branch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // SAFETY: this binary holds one test, so nothing else reads the environment meanwhile.
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
        plugins: Registry::new().with(Arc::new(Lib)),
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
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "builder", "root": root.join("ws")}))
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

    // Rule WB1: the palette lists every node kind besides agents.
    let palette: BuilderPalette = client
        .get(format!("{wsp}/builder"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(palette.builtin_stages.len(), 9);
    assert!(palette.transforms.iter().any(|t| t.name == "filter"));
    let plugins: Vec<PluginInfo> = client
        .get(format!("{wsp}/plugins"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(plugins.len(), 1);
    assert!(plugins[0].builtin && plugins[0].manifest.as_ref().unwrap().workflows.len() == 1);

    // Rule WF9: a new workspace starts with copies of Ostra's default workflows.
    let detail: WorkspaceDetail = client.get(&wsp).send().await.unwrap().json().await.unwrap();
    assert!(detail.missing_workflows.is_empty());
    assert!(
        detail.pending_commands.is_empty(),
        "the seeded copies are approved"
    );
    let implement: WorkflowDoc = client
        .get(format!("{wsp}/workflows/implement"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!implement.builtin, "the workspace has its own copy");
    assert_eq!(implement.file.stages.len(), 9);
    let mut rebased = implement.file.clone();
    rebased.base = Some("research".into());
    let r = client
        .put(format!("{wsp}/workflows/implement"))
        .json(&rebased)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422, "a copy keeps the base it is named after");
    let r = client
        .delete(format!("{wsp}/workflows/implement"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let gone: WorkflowDoc = client
        .get(format!("{wsp}/workflows/implement"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        gone.builtin,
        "without a copy the workspace runs Ostra's default"
    );
    let detail: WorkspaceDetail = client.get(&wsp).send().await.unwrap().json().await.unwrap();
    assert_eq!(detail.missing_workflows, vec!["implement"]);
    let restored: WorkspaceDetail = client
        .post(format!("{wsp}/workflows/defaults"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(restored.missing_workflows.is_empty());
    assert!(restored.pending_commands.is_empty());

    // Rule AG2: an agent saved in Ostra joins the catalog, and the workspace stays approved.
    let saved: AgentDetail = client
        .put(format!("{wsp}/agents/auditor"))
        .json(&auditor_doc())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(saved.editable);
    assert!(saved.prompt_preview.unwrap().contains("Read the changelog"));
    let bad = client
        .put(format!("{wsp}/agents/code-reviewer"))
        .json(&auditor_doc())
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 422, "a built-in agent's name is refused");
    let builtin: AgentDetail = client
        .get(format!("{wsp}/agents/code-reviewer"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!builtin.editable && builtin.doc.returns == "review");

    // Rule WB7: a composite function saved in Ostra, which the workflow below calls.
    let r = client
        .put(format!("{wsp}/transforms/risk-of"))
        .json(&json!({
            "description": "The risk an audit reported.",
            "output": "risk",
            "input": [{"name": "audit", "kind": "object"}],
            "step": [{"id": "risk", "transform": "pick", "inputs": {"value": "input.audit"}, "args": {"path": "risk"}}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let r = client
        .put(format!("{wsp}/transforms/loop"))
        .json(&json!({"output": "a", "step": [{"id": "a", "transform": "loop"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422, "a function that calls itself is refused");
    let palette: BuilderPalette = client
        .get(format!("{wsp}/builder"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        palette
            .transforms
            .iter()
            .any(|t| t.name == "risk-of" && t.custom)
    );

    // Rule WB6: a reference to a field the agent never gives is refused at save time.
    let mut misspelled = builder_workflow();
    misspelled["stage"][3]["when"][0]["ref"] = json!("audit.data.riks");
    misspelled["stage"][3]["after"] = json!(["risk", "audit"]);
    let r = client
        .put(format!("{wsp}/workflows/branch"))
        .json(&misspelled)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    let body = r.text().await.unwrap();
    assert!(body.contains("has no field `riks`"), "{body}");
    let found: Vec<String> = client
        .post(format!("{wsp}/workflows/branch/check"))
        .json(&misspelled)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        found.iter().any(|i| i.contains("riks")),
        "the builder can check a graph before saving it: {found:?}"
    );

    // Rule WB1: a workflow that names a missing agent is refused with the fix, and nothing is written.
    let mut broken = builder_workflow();
    broken["stage"][1]["agent"] = json!("nobody");
    let r = client
        .put(format!("{wsp}/workflows/branch"))
        .json(&broken)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    assert!(r.text().await.unwrap().contains("nobody"));
    let doc: WorkflowDoc = client
        .put(format!("{wsp}/workflows/branch"))
        .json(&builder_workflow())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(doc.issues.is_empty(), "{:?}", doc.issues);
    assert_eq!(doc.file.layout["audit"], [200.0, 40.0]);
    let detail: WorkspaceDetail = client.get(&wsp).send().await.unwrap().json().await.unwrap();
    assert!(
        detail.pending_commands.is_empty(),
        "Rule A1: saves made in Ostra keep the workspace approved"
    );
    assert!(detail.validation.is_empty(), "{:?}", detail.validation);
    assert!(detail.workflows.iter().any(|w| w.name == "branch"));
    let used: AgentDetail = client
        .get(format!("{wsp}/agents/auditor"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(used.used_by, vec!["branch/audit", "branch/escalate"]);
    let r = client
        .delete(format!("{wsp}/agents/auditor"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409, "a workflow uses the agent");
    let r = client
        .delete(format!("{wsp}/transforms/risk-of"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409, "a workflow calls the function");

    let s: SessionSummary = client
        .post(format!("{wsp}/sessions"))
        .json(&json!({"request": "Audit the changelog", "workflow": "branch", "options": {"yolo": true}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let done = wait_for(&client, &base, s.id.as_str(), |d| {
        matches!(
            d.summary.status,
            SessionStatus::Completed | SessionStatus::Failed
        )
    })
    .await;
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
        "{:?}",
        st.failed
    );
    use ostra_engine::workflow::StageOutcome;
    assert_eq!(
        st.stage_track("risk", None).unwrap().output,
        Some(json!("low"))
    );
    let advise = st.stage_track("advise", None).unwrap();
    assert_eq!(advise.output, Some(json!({"advice": "Ship it."})));
    assert_eq!(
        st.stage_track("escalate", None).unwrap().outcome,
        Some(StageOutcome::Skipped)
    );

    // Rules PL6 and PL7: a plugin's workflow and transform are listed, read only, and run.
    let detail: WorkspaceDetail = client.get(&wsp).send().await.unwrap().json().await.unwrap();
    assert!(
        detail
            .workflows
            .iter()
            .any(|w| w.name == "lib:flow" && w.plugin.as_deref() == Some("lib"))
    );
    let palette: BuilderPalette = client
        .get(format!("{wsp}/builder"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        palette
            .transforms
            .iter()
            .any(|t| t.name == "lib:double" && t.plugin.as_deref() == Some("lib"))
    );
    let flow: WorkflowDoc = client
        .get(format!("{wsp}/workflows/lib:flow"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(flow.plugin.as_deref(), Some("lib"));
    assert!(flow.issues.is_empty(), "{:?}", flow.issues);
    let r = client
        .put(format!("{wsp}/workflows/lib:flow"))
        .json(&flow.file)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        422,
        "a plugin's workflow is not edited in Ostra"
    );
    let s: SessionSummary = client
        .post(format!("{wsp}/sessions"))
        .json(&json!({"request": "Double it", "workflow": "lib:flow", "options": {"yolo": true}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let done = wait_for(&client, &base, s.id.as_str(), |d| {
        matches!(
            d.summary.status,
            SessionStatus::Completed | SessionStatus::Failed
        )
    })
    .await;
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
        "{:?}",
        st.failed
    );
    assert_eq!(
        st.stage_track("twice", None).unwrap().output,
        Some(json!(42))
    );

    // Rule A1: a file changed outside Ostra waits, and a save in Ostra does not approve it.
    let agents = ostra_core::paths::workspace_agents_dir(&root.join("ws"));
    std::fs::write(
        agents.join("auditor.md"),
        "+++\ndescription = \"Changed by hand.\"\n+++\nx\n",
    )
    .unwrap();
    let waits: WorkspaceDetail = client.get(&wsp).send().await.unwrap().json().await.unwrap();
    assert!(!waits.pending_commands.is_empty());
    let mut changed = auditor_doc();
    changed["description"] = json!("Audits the changelog and the readme.");
    let _: AgentDetail = client
        .put(format!("{wsp}/agents/auditor"))
        .json(&changed)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let still: WorkspaceDetail = client.get(&wsp).send().await.unwrap().json().await.unwrap();
    assert!(!still.pending_commands.is_empty());
    let r = client
        .delete(format!("{wsp}/workflows/branch"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
}
