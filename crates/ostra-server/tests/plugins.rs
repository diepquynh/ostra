//! The whole stack for custom agents, workflows, and plugins (HANDOVER 10.9 and 10.10): a real
//! server with a plugin built in, a markdown agent and a workflow file in the workspace that wait
//! for approval, and a session that runs a model-driven custom stage and a plugin stage whose
//! programmatic agent reads a file through Ostra's tools and calls the model.

use ostra_core::api::{SessionDetail, SessionStatus, SessionSummary, WorkspaceDetail};
use ostra_core::config::{GlobalConfig, TierTable, save_toml};
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
            json!({"category": "IMPLEMENT", "projects": ["app"], "explore_tasks": [{"project": "app", "task": "Research the changelog"}], "opts_in": {"tests": false, "docs": false}, "reason": "r", "title": "Release"})
        } else if props.get("report_markdown").is_some() {
            json!({"report_markdown": "# Released\n\nEvery stage ran.", "reason": "done"})
        } else {
            json!({"sufficient": true, "reason": "r", "tasks": []})
        };
        return Ok(tool_use_response(&id(), "decide", out));
    }
    // A programmatic agent's model call: no tools at all.
    if tools.is_empty() {
        return Ok(response(
            vec![Block::text("The changelog reads well.")],
            StopReason::EndTurn,
        ));
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
            assert_eq!(label(&text, "Stage"), "audit", "{text}");
            tool_use_response(
                &id(),
                &submit,
                json!({"verdict": "pass", "summary": "No secret in the changelog.", "data": {"risk": "low"}}),
            )
        }
        (other, _) => response(
            vec![Block::text(format!("unexpected agent {other}"))],
            StopReason::EndTurn,
        ),
    })
}

/// A plugin with one stage and one programmatic agent.
struct Gate;

#[async_trait::async_trait]
impl Plugin for Gate {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            workflows: vec![],
            transforms: vec![],
            name: "gate".into(),
            version: "1.0.0".into(),
            description: "Release checks.".into(),
            agents: vec![
                PluginAgent::new(
                    "changelog-counter",
                    "Counts the changelog's lines and asks the model about them.",
                )
                .capabilities([Capability::Read])
                .returns("gate:count"),
            ],
            stages: vec![PluginStage {
                name: "release".into(),
                description: "Runs the counter, then passes when it passed.".into(),
            }],
            contracts: vec![PluginContractDef {
                name: "count".into(),
                description: "A changelog's line count and the model's opinion of it.".into(),
                schema: json!({"type": "object", "required": ["lines", "opinion"], "properties": {
                    "lines": {"type": "integer"}, "opinion": {"type": "string"}
                }}),
            }],
        }
    }

    /// Rule PL5: a count passes when the changelog has lines.
    async fn handle_result(
        &self,
        contract: &str,
        result: ResultView,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<CustomSubmit, String> {
        assert_eq!(contract, "count");
        checkpoints
            .save("handled", json!(result.execution.as_str()))
            .await?;
        let lines = result.submit["lines"].as_u64().unwrap_or(0);
        Ok(CustomSubmit {
            verdict: if lines > 0 {
                StageVerdict::Pass
            } else {
                StageVerdict::Fail
            },
            summary: format!(
                "{lines} lines. {}",
                result.submit["opinion"].as_str().unwrap_or("")
            ),
            findings: vec![],
            question: None,
            options: vec![],
            report_path: None,
            data: None,
        })
    }

    async fn decide_stage(
        &self,
        _: &str,
        view: StageView,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<StageDecision, String> {
        Ok(match view.runs.last() {
            None => {
                // Rule PL8: the decision saves where the stage is, and the run reads it.
                checkpoints.save("phase", json!("counting")).await?;
                StageDecision::Run {
                    agent: "changelog-counter".into(),
                    instructions: Some("Count CHANGELOG.md.".into()),
                }
            }
            // Rule PL5: the stage reads the handled result of its own contract.
            Some(r) if r.handled.as_ref().is_some_and(|h| h["verdict"] == "pass") => {
                StageDecision::Pass {
                    summary: format!("Counted: {}", r.handled.as_ref().unwrap()["summary"]),
                }
            }
            Some(r) => StageDecision::Fail {
                summary: format!("The counter ended {:?}: {:?}", r.status, r.error),
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
        assert!(task.first_message.contains("Count CHANGELOG.md."));
        assert!(!task.resumed);
        assert_eq!(checkpoints.get("phase"), Some(json!("counting")));
        checkpoints.save("phase", json!("counted")).await?;
        checkpoints.save("scratch", json!(1)).await?;
        checkpoints.remove("scratch").await?;
        let file = ctx
            .read(&format!("{}/CHANGELOG.md", task.repo_root.display()))
            .await;
        if file.is_error {
            return Err(file.output);
        }
        let denied = ctx
            .write(&format!("{}/x.txt", task.repo_root.display()), "x")
            .await;
        assert!(
            denied.is_error,
            "the agent has no write tool: {}",
            denied.output
        );
        let opinion = ctx.complete("You judge changelogs.", &file.output).await?;
        ctx.status("counted");
        Ok(AgentOutcome {
            submit: json!({
                "lines": file.output.lines().filter(|l| l.contains('\t')).count(),
                "opinion": opinion,
            }),
        })
    }
}

const AUDITOR: &str = "+++\ndescription = \"Audits the changelog for secrets.\"\ncapabilities = [\"read\", \"search_text\"]\n[data_schema]\ntype = \"object\"\nrequired = [\"risk\"]\n[data_schema.properties.risk]\ntype = \"string\"\n+++\nRead the changelog and report any secret in it.\n";

const WORKFLOW: &str = "extends = \"research\"\ndescription = \"Research, an audit, and the release gate.\"\n\n[[stage]]\nid = \"audit\"\nagent = \"auditor\"\n\n[[stage]]\nid = \"release-gate\"\nplugin = \"gate:release\"\n";

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_runs_a_custom_agent_and_a_plugin_stage_after_approval() {
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
        plugins: Registry::new().with(Arc::new(Gate)),
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
    let ws: WorkspaceDetail = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "release", "root": root.join("ws")}))
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

    // Rule A1: files dropped into the workspace wait for approval, and nothing runs them.
    let ws_root = root.join("ws");
    let agents = ostra_core::paths::workspace_agents_dir(&ws_root);
    let workflows = ostra_core::paths::workspace_workflows_dir(&ws_root);
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(agents.join("auditor.md"), AUDITOR).unwrap();
    std::fs::write(workflows.join("release.toml"), WORKFLOW).unwrap();
    let pending: WorkspaceDetail = client.get(&wsp).send().await.unwrap().json().await.unwrap();
    let waits = pending
        .pending_commands
        .iter()
        .find(|p| p.project.is_none())
        .expect("the workspace file waits for approval");
    let kinds: Vec<String> = waits
        .items
        .iter()
        .map(|i| format!("{:?} {}", i.kind, i.name))
        .collect();
    assert!(
        kinds.contains(&"AgentFile agents/auditor.md".to_string()),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&"WorkflowFile workflows/release.toml".to_string()),
        "{kinds:?}"
    );
    assert!(
        !pending.agents.iter().any(|a| a.name.as_str() == "auditor"),
        "an agent file waiting for approval is not in the catalog"
    );
    assert!(
        pending
            .agents
            .iter()
            .any(|a| a.name.as_str() == "changelog-counter"),
        "a built-in plugin's agents need no approval"
    );
    let refused = client
        .post(format!("{wsp}/sessions"))
        .json(&json!({"request": "Check the release", "workflow": "release"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 409);
    let body = refused.text().await.unwrap();
    assert!(
        body.contains("`release` is none of them"),
        "the workflow file is not approved yet: {body}"
    );

    let approved: WorkspaceDetail = client
        .post(format!("{wsp}/approve"))
        .json(&json!({"hash": waits.hash}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(approved.pending_commands.is_empty());
    assert!(approved.agents.iter().any(|a| a.name.as_str() == "auditor"));
    assert!(
        approved
            .workflows
            .iter()
            .any(|w| w.name == "release" && !w.builtin),
        "{:?}",
        approved
            .workflows
            .iter()
            .map(|w| &w.name)
            .collect::<Vec<_>>()
    );
    assert!(approved.validation.is_empty(), "{:?}", approved.validation);

    let s: SessionSummary = client
        .post(format!("{wsp}/sessions"))
        .json(&json!({"request": "Check the release", "workflow": "release", "options": {"yolo": true}}))
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
    assert_eq!(st.category, Some(ostra_core::pipeline::Category::Research));
    assert_eq!(st.workflow.as_ref().unwrap().name, "release");
    let audit = st.stage_track("audit", None).unwrap();
    assert_eq!(
        audit.outcome,
        Some(ostra_engine::workflow::StageOutcome::Passed)
    );
    assert_eq!(
        audit.last.as_ref().unwrap().data,
        Some(json!({"risk": "low"}))
    );
    let gate = st.stage_track("release-gate", None).unwrap();
    assert_eq!(
        gate.outcome,
        Some(ostra_engine::workflow::StageOutcome::Passed)
    );
    let counter = st
        .executions
        .values()
        .find(|r| r.agent.as_str() == "changelog-counter")
        .unwrap();
    assert_eq!(counter.contract.as_str(), "gate:count");
    let summary = counter.handled.as_ref().unwrap().summary.clone();
    assert!(
        summary.ends_with("The changelog reads well."),
        "the plugin's model call ran on the agent's route, and its handler shaped the outcome: {summary}"
    );
    // Rule PL8: what the stage, the run, and the handler saved is in the session's log.
    assert_eq!(
        st.plugin_checkpoints.get("gate"),
        Some(&std::collections::BTreeMap::from([
            ("handled".to_string(), json!(counter.id.as_str())),
            ("phase".to_string(), json!("counted")),
        ]))
    );
    assert_eq!(st.checkpoint_saves.get("gate"), Some(&5));
}
