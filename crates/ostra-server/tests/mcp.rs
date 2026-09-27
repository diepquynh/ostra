//! Workspace MCP servers through the whole stack: settings, discovery, the status API, OAuth
//! sign-in through the browser callback, and a native agent calling a server's tool.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use ostra_core::api::{
    ActivityItem, ExecutionView, McpConnState, McpLogin, McpServerStatus, WorkspaceDetail,
};
use ostra_core::config::{GlobalConfig, TierTable, ValidationIssue, save_toml};
use ostra_core::exec::{ExecutionDelta, ExecutionStatus};
use ostra_providers::mock::tool_use_response;
use ostra_providers::{Block, ChatRequest, ChatResponse, ProviderError, Role, ScriptedProvider};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const TOKEN_VAR: &str = "OSTRA_TEST_MCP_TOKEN";

/// A remote MCP server with `echo` and `secret`. With `oauth` set it needs a token issued by its
/// own authorization server; otherwise it needs `Bearer static-secret`.
#[derive(Default)]
struct Fake {
    oauth: bool,
    base: Mutex<String>,
    issued: Mutex<Option<String>>,
}

async fn mcp(
    State(f): State<Arc<Fake>>,
    headers: HeaderMap,
    body: String,
) -> axum::response::Response {
    let want = if f.oauth {
        f.issued.lock().clone().unwrap_or_else(|| "none yet".into())
    } else {
        "static-secret".into()
    };
    let got = headers.get("authorization").and_then(|h| h.to_str().ok());
    if got != Some(&format!("Bearer {want}")) {
        return (StatusCode::UNAUTHORIZED, [("www-authenticate", "Bearer")]).into_response();
    }
    let m: Value = serde_json::from_str(&body).unwrap();
    let Some(id) = m.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let result = match m["method"].as_str().unwrap() {
        "initialize" => {
            json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "fake", "version": "2"}})
        }
        "tools/list" => json!({"tools": [
            {"name": "echo", "description": "Echo text back", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}, "annotations": {"readOnlyHint": true}},
            {"name": "secret", "description": "Not for agents", "inputSchema": {"type": "object"}},
        ]}),
        "tools/call" => {
            json!({"content": [{"type": "text", "text": format!("echo: {}", m["params"]["arguments"]["text"].as_str().unwrap_or(""))}]})
        }
        _ => json!({}),
    };
    axum::Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

async fn serve(f: Arc<Fake>) -> String {
    let app = axum::Router::new()
        .route("/mcp", post(mcp))
        .route(
            "/.well-known/oauth-authorization-server",
            get(|State(f): State<Arc<Fake>>| async move {
                let base = f.base.lock().clone();
                axum::Json(json!({
                    "issuer": base,
                    "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "registration_endpoint": format!("{base}/register"),
                }))
            }),
        )
        .route(
            "/register",
            post(|| async { axum::Json(json!({"client_id": "ostra-client"})) }),
        )
        .route(
            "/authorize",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                // Signs the user in at once and sends the browser back to Ostra.
                axum::response::Redirect::to(&format!(
                    "{}?code=the-code&state={}",
                    q["redirect_uri"], q["state"]
                ))
            }),
        )
        .route(
            "/token",
            post(|State(f): State<Arc<Fake>>, body: String| async move {
                assert!(body.contains("code=the-code"), "{body}");
                assert!(body.contains("client_id=ostra-client"), "{body}");
                *f.issued.lock() = Some("issued-token".into());
                axum::Json(json!({"access_token": "issued-token", "token_type": "Bearer", "expires_in": 3600}))
            }),
        )
        .with_state(f.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    *f.base.lock() = base.clone();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn tool_result(req: &ChatRequest) -> Option<String> {
    req.messages
        .iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| m.content.iter())
        .find_map(|b| match b {
            Block::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
}

/// The quick-answer agent: call `mcp__fake__echo`, then submit what it returned.
fn respond(req: &ChatRequest) -> Result<ChatResponse, ProviderError> {
    let tools: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(tools.contains(&"mcp__fake__echo"), "{tools:?}");
    assert!(
        !tools.contains(&"mcp__fake__secret"),
        "a disabled tool was offered"
    );
    let echo = req
        .tools
        .iter()
        .find(|t| t.name == "mcp__fake__echo")
        .unwrap();
    assert_eq!(echo.description, "Echo text back");
    Ok(match tool_result(req) {
        None => tool_use_response("toolu_1", "mcp__fake__echo", json!({"text": "pong"})),
        Some(out) => tool_use_response(
            "toolu_2",
            "submit_quick_answer",
            json!({"answer": out, "sources": []}),
        ),
    })
}

async fn boot(root: &Path) -> (Arc<ostra_server::app::App>, String, reqwest::Client) {
    // SAFETY: this binary has one test, so nothing else reads the environment meanwhile.
    unsafe {
        std::env::set_var("OSTRA_CONFIG", root.join("config.toml"));
        std::env::set_var("OSTRA_DATA_DIR", root.join("data"));
        std::env::set_var("OSTRA_MASTER_KEY_FILE", root.join("master.key"));
        std::env::set_var("OSTRA_SANDBOX_CACHE", root.join("sandbox-cache"));
        std::env::set_var("OSTRA_MODELS_DEV_URL", "");
        std::env::set_var(TOKEN_VAR, "static-secret");
    }
    let mut global = GlobalConfig::default();
    let m = Some("mock:m".to_string());
    global.tiers.insert(
        "native".into(),
        TierTable {
            fast: m.clone(),
            balanced: m.clone(),
            advanced: m.clone(),
            frontier: m,
        },
    );
    global.server.use_ip_host = true;
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
    };
    let app = ostra_server::app::build(&opts, port).await.unwrap();
    app.shared.providers.register(
        "mock",
        Arc::new(ScriptedProvider::named("mock").with_responder(respond)),
    );
    let router = ostra_server::api::router(app.clone());
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
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
    (app, base, client)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_mcp_servers_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    // Not canonicalized: on macOS `/private` would push the bridge socket past the 104-byte limit.
    let root = dir.path().to_path_buf();
    let (_app, base, client) = boot(&root).await;
    let plain = serve(Arc::new(Fake::default())).await;
    let oauth_fake = Arc::new(Fake {
        oauth: true,
        ..Default::default()
    });
    let guarded = serve(oauth_fake.clone()).await;

    let project = root.join("app");
    std::fs::create_dir_all(&project).unwrap();
    let r = client
        .post(format!("{base}/api/workspaces"))
        .json(&json!({"name": "shop", "root": root.join("ws"), "projects": [{"path": project, "key": "app"}], "routing_preset": "native"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    let detail: WorkspaceDetail = r.json().await.unwrap();
    let ws = detail.id.to_string();

    let mut settings = serde_json::to_value(&detail.settings).unwrap();
    settings["mcp_servers"] = json!([{"name": "Bad_Name", "url": "ftp://x"}]);
    let issues: Vec<ValidationIssue> = client
        .post(format!("{base}/api/workspaces/{ws}/validate"))
        .json(&settings)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let paths: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(paths, ["mcp_servers[0].name", "mcp_servers[0].url"]);

    settings["mcp_servers"] = json!([
        {"name": "fake", "url": format!("{plain}/mcp"), "headers": {"Authorization": format!("Bearer ${{{TOKEN_VAR}}}")}, "disabled_tools": ["secret"]},
        {"name": "guarded", "url": format!("{guarded}/mcp"), "agents": ["explore"]},
    ]);
    let r = client
        .patch(format!("{base}/api/workspaces/{ws}"))
        .json(&settings)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    let toml = std::fs::read_to_string(root.join("ws/.ostra/workspace.toml")).unwrap();
    assert!(
        toml.contains("[[mcp_servers]]") && toml.contains("${OSTRA_TEST_MCP_TOKEN}"),
        "{toml}"
    );

    let status: Vec<McpServerStatus> = client
        .get(format!("{base}/api/workspaces/{ws}/mcp"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status[0].state, McpConnState::Connected, "{:?}", status[0]);
    assert_eq!(status[0].server_info.as_deref(), Some("fake 2"));
    let tools: Vec<(&str, &str, bool, bool)> = status[0]
        .tools
        .iter()
        .map(|t| {
            (
                t.name.as_str(),
                t.canonical.as_str(),
                t.enabled,
                t.read_only,
            )
        })
        .collect();
    assert_eq!(
        tools,
        [
            ("echo", "mcp__fake__echo", true, true),
            ("secret", "mcp__fake__secret", false, false)
        ]
    );
    assert_eq!(status[1].state, McpConnState::NeedsAuth, "{:?}", status[1]);
    assert_eq!(status[1].signed_in, Some(false));

    // Sign in the way a browser does: open the link, follow the redirect back to Ostra.
    let login: McpLogin = client
        .post(format!("{base}/api/workspaces/{ws}/mcp/guarded/login"))
        .header("origin", &base)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        login
            .authorization_url
            .contains("code_challenge_method=S256")
    );
    let browser = reqwest::Client::new();
    let page = browser.get(&login.authorization_url).send().await.unwrap();
    assert_eq!(page.status(), 200, "the callback needs no session cookie");
    assert!(
        page.text()
            .await
            .unwrap()
            .contains("signed in to MCP server <code>guarded</code>")
    );
    let again = browser
        .get(format!("{base}/mcp/oauth/callback?code=x&state=used"))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 400);
    let one: McpServerStatus = client
        .post(format!("{base}/api/workspaces/{ws}/mcp/guarded/refresh"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(one.state, McpConnState::Connected, "{one:?}");
    assert_eq!(one.signed_in, Some(true));

    // A native agent calls the tool. `guarded` serves only explore, so the quick answer does
    // not see it.
    let started: Value = client
        .post(format!("{base}/api/workspaces/{ws}/ask"))
        .json(&json!({"question": "What does echo say?"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let exec = started["execution"].as_str().unwrap().to_string();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let view = loop {
        let v: ExecutionView = client
            .get(format!("{base}/api/executions/{exec}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if v.status != ExecutionStatus::Running && v.ended_at.is_some() {
            break v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the execution did not end"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let activity: Vec<ActivityItem> = client
        .get(format!("{base}/api/executions/{exec}/activity"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(view.status, ExecutionStatus::Ok, "{view:?} {activity:?}");
    assert!(activity.iter().any(|a| matches!(&a.delta, ExecutionDelta::ToolCall { call, .. } if call.tool == "mcp__fake__echo")));
    let results: Vec<&ExecutionDelta> = activity
        .iter()
        .map(|a| &a.delta)
        .filter(|d| matches!(d, ExecutionDelta::ToolResult { .. }))
        .collect();
    assert!(results.iter().any(|d| matches!(d, ExecutionDelta::ToolResult { output, is_error: false, .. } if output == "echo: pong")), "{results:?}");
    assert!(!activity.iter().any(
        |a| matches!(&a.delta, ExecutionDelta::Status { message } if message.contains("guarded"))
    ));
}
