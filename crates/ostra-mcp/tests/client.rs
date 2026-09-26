//! The client against fake servers: stdio (this test binary re-run as the child), streamable
//! HTTP answering with JSON and with SSE, and a full OAuth sign-in.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use ostra_mcp::{Client, Endpoint, McpError, TokenSource, oauth};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const T: Duration = Duration::from_secs(10);

/// The fake server: two pages of tools, `echo`, `fail`, and `slow`.
fn handle(m: &Value) -> Option<Value> {
    let id = m.get("id")?.clone();
    let method = m.get("method")?.as_str()?;
    let result = match method {
        "initialize" => json!({
            "protocolVersion": m.pointer("/params/protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
            "capabilities": {"tools": {"listChanged": true}},
            "serverInfo": {"name": "fake", "version": "1.0"},
        }),
        "tools/list" => match m.pointer("/params/cursor").and_then(Value::as_str) {
            None => json!({"tools": [
                {"name": "echo", "description": "Echo text", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}, "annotations": {"readOnlyHint": true}},
                {"name": "fail", "description": "Always fails", "inputSchema": {"type": "object"}},
            ], "nextCursor": "p2"}),
            Some(_) => json!({"tools": [{"name": "slow", "inputSchema": {"type": "object"}}]}),
        },
        "tools/call" => {
            let args = m.pointer("/params/arguments").cloned().unwrap_or_default();
            match m.pointer("/params/name").and_then(Value::as_str) {
                Some("echo") => {
                    json!({"content": [{"type": "text", "text": args["text"].as_str().unwrap_or("")}]})
                }
                Some("fail") => {
                    json!({"content": [{"type": "text", "text": "it broke"}], "isError": true})
                }
                Some("slow") => {
                    std::thread::sleep(Duration::from_secs(5));
                    json!({"content": []})
                }
                _ => {
                    return Some(
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "unknown tool"}}),
                    );
                }
            }
        }
        _ => {
            return Some(
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "no"}}),
            );
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

async fn exercise(client: &Client) {
    assert_eq!(client.server_info.as_deref(), Some("fake 1.0"));
    let tools = client.list_tools(T).await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["echo", "fail", "slow"]);
    assert!(tools[0].read_only && !tools[1].read_only);
    let r = client
        .call_tool("echo", &json!({"text": "hi"}), T, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.unwrap(), "hi");
    let r = client
        .call_tool("fail", &json!({}), T, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r.unwrap_err(), "it broke");
    let e = client
        .call_tool("nope", &json!({}), T, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(e, McpError::Rpc { code: -32602, .. }), "{e:?}");
}

// ---------------------------------------------------------------------------------------------
// stdio
// ---------------------------------------------------------------------------------------------

const CHILD: &str = "OSTRA_MCP_TEST_CHILD";

/// Runs as the server when this binary is started with `CHILD` set; does nothing otherwise.
#[test]
fn stdio_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    // libtest has printed `test stdio_child ... ` without a newline.
    writeln!(out).unwrap();
    for line in stdin.lock().lines() {
        let Ok(m) = serde_json::from_str::<Value>(&line.unwrap()) else {
            continue;
        };
        if let Some(r) = handle(&m) {
            writeln!(out, "{r}").unwrap();
            out.flush().unwrap();
        }
    }
    std::process::exit(0);
}

fn stdio_endpoint() -> Endpoint {
    Endpoint::Stdio {
        program: std::env::current_exe().unwrap().to_string_lossy().into(),
        args: vec![
            "--exact".into(),
            "stdio_child".into(),
            "--nocapture".into(),
            "--test-threads=1".into(),
        ],
        env: vec![(CHILD.into(), "1".into())],
        env_remove: vec![],
        cwd: std::env::temp_dir(),
        guard: None,
    }
}

#[tokio::test]
async fn stdio_server() {
    let client = Client::connect(stdio_endpoint(), T).await.unwrap();
    exercise(&client).await;
    let e = client
        .call_tool(
            "slow",
            &json!({}),
            Duration::from_millis(200),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(e, McpError::Timeout(_)), "{e:?}");
    assert!(client.is_alive());
}

#[tokio::test]
async fn stdio_crash_is_reported() {
    let e = Client::connect(
        Endpoint::Stdio {
            program: "sh".into(),
            args: vec!["-c".into(), "echo 'bad token' >&2; exit 3".into()],
            env: vec![],
            env_remove: vec![],
            cwd: std::env::temp_dir(),
            guard: None,
        },
        T,
    )
    .await
    .err()
    .unwrap();
    assert!(e.to_string().contains("bad token"), "{e}");
}

#[tokio::test]
async fn stdio_server_does_not_inherit_removed_variables() {
    // SAFETY: no other test reads this variable.
    unsafe { std::env::set_var("OSTRA_MCP_TEST_SECRET", "leaked") };
    let e = Client::connect(
        Endpoint::Stdio {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "echo \"secret=[${OSTRA_MCP_TEST_SECRET-unset}] keep=[$KEEP]\" >&2; exit 3".into(),
            ],
            env: vec![("KEEP".into(), "yes".into())],
            env_remove: vec!["OSTRA_MCP_TEST_SECRET".into()],
            cwd: std::env::temp_dir(),
            guard: None,
        },
        T,
    )
    .await
    .err()
    .unwrap();
    assert!(e.to_string().contains("secret=[unset] keep=[yes]"), "{e}");
}

// ---------------------------------------------------------------------------------------------
// Streamable HTTP
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Fake {
    sse: bool,
    /// When set, `/mcp` requires `Bearer <token>`.
    token: Mutex<Option<String>>,
    base: Mutex<String>,
    codes: Mutex<HashMap<String, String>>,
    sessions: Mutex<Vec<Option<String>>>,
}

async fn mcp(State(f): State<Arc<Fake>>, headers: HeaderMap, body: String) -> Response {
    if let Some(want) = f.token.lock().clone() {
        let got = headers.get("authorization").and_then(|h| h.to_str().ok());
        if got != Some(&format!("Bearer {want}")) {
            let base = f.base.lock().clone();
            return (
                StatusCode::UNAUTHORIZED,
                [("www-authenticate", format!(r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource/mcp""#))],
            )
                .into_response();
        }
    }
    f.sessions.lock().push(
        headers
            .get("mcp-session-id")
            .and_then(|h| h.to_str().ok())
            .map(String::from),
    );
    let m: Value = serde_json::from_str(&body).unwrap();
    let Some(reply) = handle(&m) else {
        return StatusCode::ACCEPTED.into_response();
    };
    let session = [("mcp-session-id", "sess-1")];
    if f.sse {
        let ping = json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"});
        let body = format!("event: message\ndata: {ping}\n\nevent: message\ndata: {reply}\n\n");
        (session, [("content-type", "text/event-stream")], body).into_response()
    } else {
        (session, axum::Json(reply)).into_response()
    }
}

async fn serve(f: Arc<Fake>) -> String {
    let app = axum::Router::new()
        .route("/mcp", post(mcp))
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(|State(f): State<Arc<Fake>>| async move {
                let base = f.base.lock().clone();
                axum::Json(json!({"resource": format!("{base}/mcp"), "authorization_servers": [base], "scopes_supported": ["tools"]}))
            }),
        )
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
            post(|axum::Json(v): axum::Json<Value>| async move {
                assert_eq!(v["token_endpoint_auth_method"], "none");
                axum::Json(json!({"client_id": "client-1"}))
            }),
        )
        .route(
            "/authorize",
            get(|State(f): State<Arc<Fake>>, Query(q): Query<HashMap<String, String>>| async move {
                assert_eq!(q["code_challenge_method"], "S256");
                assert!(q["resource"].ends_with("/mcp"));
                assert_eq!(q["scope"], "tools");
                f.codes.lock().insert("code-1".into(), q["code_challenge"].clone());
                format!("{}?code=code-1&state={}", q["redirect_uri"], q["state"])
            }),
        )
        .route(
            "/token",
            post(|State(f): State<Arc<Fake>>, body: String| async move {
                let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
                let token = match form["grant_type"].as_str() {
                    "authorization_code" => {
                        use base64::Engine;
                        use sha2::Digest;
                        let challenge = f.codes.lock().remove(&form["code"]).unwrap();
                        let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .encode(sha2::Sha256::digest(form["code_verifier"].as_bytes()));
                        assert_eq!(challenge, computed);
                        "access-1"
                    }
                    "refresh_token" => {
                        assert_eq!(form["refresh_token"], "refresh-1");
                        "access-2"
                    }
                    _ => unreachable!(),
                };
                *f.token.lock() = Some(token.into());
                axum::Json(json!({"access_token": token, "token_type": "Bearer", "expires_in": 3600, "refresh_token": "refresh-1"}))
            }),
        )
        .with_state(f.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    *f.base.lock() = base.clone();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn http(url: String, auth: Option<Arc<dyn TokenSource>>) -> Endpoint {
    Endpoint::Http {
        url,
        headers: vec![("x-extra".into(), "1".into())],
        auth,
    }
}

#[tokio::test]
async fn http_json_and_sse() {
    for sse in [false, true] {
        let f = Arc::new(Fake {
            sse,
            ..Default::default()
        });
        let base = serve(f.clone()).await;
        let client = Client::connect(http(format!("{base}/mcp"), None), T)
            .await
            .unwrap();
        exercise(&client).await;
        assert_eq!(client.take_tools_changed(), sse);
        let sessions = f.sessions.lock().clone();
        assert_eq!(sessions[0], None);
        assert!(sessions[1..].iter().all(|s| s.as_deref() == Some("sess-1")));
    }
}

/// Holds one token and refreshes it through the real OAuth helpers.
struct Held {
    http: reqwest::Client,
    discovery: oauth::Discovery,
    client: oauth::ClientCredentials,
    tokens: Mutex<oauth::Tokens>,
}

#[async_trait::async_trait]
impl TokenSource for Held {
    async fn token(&self) -> Option<String> {
        Some(self.tokens.lock().access_token.clone())
    }
    async fn refused(&self, _: &str) -> Option<String> {
        let refresh = self.tokens.lock().refresh_token.clone()?;
        let t = oauth::refresh(&self.http, &self.discovery, &self.client, &refresh, 0)
            .await
            .ok()?;
        let access = t.access_token.clone();
        *self.tokens.lock() = t;
        Some(access)
    }
}

#[tokio::test]
async fn oauth_sign_in_and_refresh() {
    let f = Arc::new(Fake {
        token: Mutex::new(Some("unset".into())),
        ..Default::default()
    });
    let base = serve(f.clone()).await;
    let url = format!("{base}/mcp");
    let Err(McpError::NeedsAuth { www_authenticate }) =
        Client::connect(http(url.clone(), None), T).await
    else {
        panic!("expected a sign-in request");
    };
    let rq = reqwest::Client::new();
    let discovery = oauth::discover(&rq, &url, www_authenticate.as_deref())
        .await
        .unwrap();
    assert_eq!(discovery.resource, url);
    assert_eq!(discovery.scopes, ["tools"]);
    let redirect = "http://127.0.0.1:1/mcp/oauth/callback";
    let client = oauth::register(&rq, &discovery.server, redirect)
        .await
        .unwrap();
    let pkce = oauth::pkce();
    let state = oauth::random_token();
    let auth_url = oauth::authorization_url(&oauth::AuthRequest {
        discovery: &discovery,
        client: &client,
        redirect_uri: redirect,
        scopes: &discovery.scopes,
        state: &state,
        challenge: &pkce.challenge,
    })
    .unwrap();
    // The browser step: the fake authorization server answers with the redirect target.
    let back = rq
        .get(&auth_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let back = url::Url::parse(&back).unwrap();
    let q: HashMap<String, String> = back.query_pairs().into_owned().collect();
    assert_eq!(q["state"], state);
    let tokens = oauth::exchange_code(
        &rq,
        &discovery,
        &client,
        &q["code"],
        &pkce.verifier,
        redirect,
        100,
    )
    .await
    .unwrap();
    assert_eq!(tokens.access_token, "access-1");
    assert_eq!(tokens.expires_at, Some(3700));

    let held = Arc::new(Held {
        http: rq,
        discovery,
        client,
        tokens: Mutex::new(tokens),
    });
    let mcp = Client::connect(http(url, Some(held.clone())), T)
        .await
        .unwrap();
    exercise(&mcp).await;
    // The server rotates its token; the client refreshes once and carries on.
    *f.token.lock() = Some("access-2".into());
    held.tokens.lock().access_token = "stale".into();
    mcp.list_tools(T).await.unwrap();
    assert_eq!(held.tokens.lock().access_token, "access-2");
}

/// A server that publishes only authorization server metadata, built from its own base URL.
async fn serve_metadata(meta: fn(&str) -> Value) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let body = meta(&base);
    let app = axum::Router::new().route(
        "/.well-known/oauth-authorization-server",
        get(move || async move { axum::Json(body) }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[tokio::test]
async fn oauth_discovery_refuses_a_script_endpoint() {
    let base = serve_metadata(|base| {
        json!({
            "issuer": base,
            "authorization_endpoint": "javascript:fetch('/api/x')//",
            "token_endpoint": format!("{base}/token"),
        })
    })
    .await;
    let e = oauth::discover(&reqwest::Client::new(), &format!("{base}/mcp"), None)
        .await
        .unwrap_err();
    assert!(e.contains("authorization endpoint") && e.contains("https"), "{e}");
}

#[tokio::test]
async fn oauth_discovery_refuses_another_issuer() {
    let base = serve_metadata(|base| {
        json!({
            "issuer": "https://evil.example",
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
        })
    })
    .await;
    let e = oauth::discover(&reqwest::Client::new(), &format!("{base}/mcp"), None)
        .await
        .unwrap_err();
    assert!(e.contains("issuer `https://evil.example`"), "{e}");
}

#[test]
fn oauth_endpoint_schemes() {
    let origin = Some("http://10.0.0.5:8080");
    assert!(oauth::check_endpoint("x", "https://auth.example/authorize", None).is_ok());
    assert!(oauth::check_endpoint("x", "http://127.0.0.1:9/authorize", None).is_ok());
    assert!(oauth::check_endpoint("x", "http://localhost:9/authorize", None).is_ok());
    assert!(oauth::check_endpoint("x", "http://10.0.0.5:8080/authorize", origin).is_ok());
    assert!(oauth::check_endpoint("x", "http://evil.example/authorize", origin).is_err());
    assert!(oauth::check_endpoint("x", "javascript:alert(1)", origin).is_err());
    assert!(oauth::check_endpoint("x", "data:text/html,hi", None).is_err());
    assert!(oauth::check_endpoint("x", "file:///etc/passwd", None).is_err());
}

#[tokio::test]
async fn redirects_stay_on_the_first_origin() {
    let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other_base = format!("http://{}", other.local_addr().unwrap());
    tokio::spawn(async move {
        let app = axum::Router::new().route("/leak", get(|| async { "reached the other host" }));
        axum::serve(other, app).await.unwrap()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let target = format!("{other_base}/leak");
    tokio::spawn(async move {
        let app = axum::Router::new()
            .route("/away", get(move || async move { axum::response::Redirect::temporary(&target) }))
            .route("/here", get(|| async { axum::response::Redirect::temporary("/ok") }))
            .route("/ok", get(|| async { "same origin" }));
        axum::serve(listener, app).await.unwrap()
    });
    let rq = reqwest::Client::builder()
        .redirect(ostra_mcp::same_origin_redirects())
        .build()
        .unwrap();
    let away = rq.get(format!("{base}/away")).header("x-api-key", "secret").send().await.unwrap();
    assert_eq!(away.status(), StatusCode::TEMPORARY_REDIRECT);
    let here = rq.get(format!("{base}/here")).send().await.unwrap();
    assert_eq!(here.text().await.unwrap(), "same origin");
}
