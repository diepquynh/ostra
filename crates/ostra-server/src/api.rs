//! REST routes (HANDOVER 13) and the security middleware (HANDOVER 15).

use crate::app::App;
use crate::auth::{Auth, cookie_value};
use crate::files;
use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use ostra_code::Answer;
use ostra_core::api::*;
use ostra_core::code::{CodeDeps, CodeExternalFile, CodeFile, CodeSymbols, CodeUsages};
use ostra_core::config::{ValidationIssue, WorkspaceSettings};
use ostra_core::event::StoredEvent;
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
use ostra_core::paths;
use ostra_engine::EngineError;
use ostra_store::MemoryStore;
use ostra_workspace::WorkspaceRt;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

type AppState = State<Arc<App>>;

pub struct ApiErr {
    status: StatusCode,
    body: ApiError,
}

impl ApiErr {
    pub fn message(&self) -> &str {
        &self.body.error
    }

    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        ApiErr {
            status,
            body: ApiError {
                error: message.into(),
                issues: vec![],
            },
        }
    }
    fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }
    /// A 409 naming the request field that no longer matches the server's state.
    pub fn conflict_on(field: &str, message: impl Into<String>) -> Self {
        let message = message.into();
        ApiErr {
            status: StatusCode::CONFLICT,
            body: ApiError {
                error: message.clone(),
                issues: vec![ValidationIssue {
                    path: field.into(),
                    message,
                }],
            },
        }
    }
    /// A 422 with a message of its own and one issue per field.
    pub fn invalid_with(message: impl Into<String>, issues: Vec<ValidationIssue>) -> Self {
        ApiErr {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: ApiError {
                error: message.into(),
                issues,
            },
        }
    }
    fn invalid(issues: Vec<ValidationIssue>) -> Self {
        ApiErr {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: ApiError {
                error: "The settings have problems. Fix them and save again.".into(),
                issues,
            },
        }
    }
    fn invalid_workspace(issues: Vec<ValidationIssue>) -> Self {
        Self::invalid_request(issues, "create the workspace")
    }

    /// A 422 whose message carries the issues too, for clients that show only `error`.
    fn invalid_request(issues: Vec<ValidationIssue>, retry: &str) -> Self {
        let error = match issues.as_slice() {
            [one] => one.message.clone(),
            _ => format!(
                "Fix these problems and {retry} again: {}",
                issues
                    .iter()
                    .map(|i| i.message.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        };
        ApiErr {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: ApiError { error, issues },
        }
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<EngineError> for ApiErr {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::NotFound(m) => ApiErr::not_found(m),
            EngineError::Invalid(m) => ApiErr::new(StatusCode::CONFLICT, m),
            EngineError::Ended => ApiErr::new(StatusCode::CONFLICT, e.to_string()),
            EngineError::Store(e) => ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }
}

impl From<ostra_store::StoreError> for ApiErr {
    fn from(e: ostra_store::StoreError) -> Self {
        ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

type Res<T> = Result<Json<T>, ApiErr>;

async fn guard(State(app): AppState, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let api = path.starts_with("/api/") || path == "/ws";
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| app.auth.allowed_host(Some(h)))
        .map(String::from);
    let private = app.auth.private_redirect(host.as_deref()).map(String::from);
    let mut res = match host {
        None => ApiErr::new(StatusCode::MISDIRECTED_REQUEST, "This host is not allowed.")
            .into_response(),
        // A cookie set on 127.0.0.1 would reach every other port on this machine, so the app
        // lives at its private name. A page load moves there, keeping the `#token=` fragment,
        // and the API is refused here.
        Some(_) if private.is_some() && !path.starts_with("/internal/") => {
            let target = private.unwrap_or_default();
            if api {
                ApiErr::new(
                    StatusCode::MISDIRECTED_REQUEST,
                    format!("Open Ostra at http://{target}/, because a sign-in on 127.0.0.1 would share its cookie with every other local port."),
                )
                .into_response()
            } else {
                let query = req
                    .uri()
                    .query()
                    .map(|q| format!("?{q}"))
                    .unwrap_or_default();
                match HeaderValue::from_str(&format!("http://{target}{path}{query}")) {
                    Ok(loc) => {
                        let mut r = StatusCode::TEMPORARY_REDIRECT.into_response();
                        r.headers_mut().insert(header::LOCATION, loc);
                        r
                    }
                    Err(_) => StatusCode::BAD_REQUEST.into_response(),
                }
            }
        }
        Some(_) => match refusal(&app, &req, &path, api) {
            Some(r) => r,
            None => next.run(req).await,
        },
    };
    // Refusals get these headers too, so a page elsewhere cannot frame or sniff them.
    secure_headers(res.headers_mut(), host.as_deref(), api);
    res
}

/// Why a request that reached an allowed host is refused, if it is.
fn refusal(app: &App, req: &Request, path: &str, api: bool) -> Option<Response> {
    let headers = req.headers();
    // Harnesses always run on this machine, so the hook bridge and MCP shim refuse other peers
    // even when the server listens on every interface.
    if path.starts_with("/internal/")
        && let Some(axum::extract::ConnectInfo(peer)) = req
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        && !crate::auth::is_local_address(peer.ip())
    {
        return Some(
            ApiErr::new(
                StatusCode::FORBIDDEN,
                "This endpoint accepts local connections only.",
            )
            .into_response(),
        );
    }
    // Only Ostra's own hook and MCP clients call these. A browser page on this machine is a local
    // peer too, but it always sends `Origin` or `Sec-Fetch-Site`, so refuse either.
    if path.starts_with("/internal/")
        && (headers.contains_key(header::ORIGIN) || headers.contains_key("sec-fetch-site"))
    {
        return Some(
            ApiErr::new(
                StatusCode::FORBIDDEN,
                "This endpoint is for Ostra's harness bridge, not for web pages.",
            )
            .into_response(),
        );
    }
    if api {
        let origin = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok());
        // A request without `Origin` still carries `Sec-Fetch-Site`, which tells a page on another
        // localhost port (same-site, so the SameSite cookie rides along) from this app's own page.
        let fetch_site = headers.get("sec-fetch-site").and_then(|h| h.to_str().ok());
        // The app calls the API with fetch (`empty`), the socket (`websocket`), and download
        // links (`document`). An image, script, or frame request is page content reaching the
        // API, such as a Markdown image an agent wrote, so it is refused.
        let fetch_dest = headers.get("sec-fetch-dest").and_then(|h| h.to_str().ok());
        let dest_ok = fetch_dest.is_none_or(|d| matches!(d, "empty" | "websocket" | "document"));
        if !app.auth.allowed_origin(origin)
            || matches!(fetch_site, Some("cross-site" | "same-site"))
            || !dest_ok
        {
            return Some(
                ApiErr::new(StatusCode::FORBIDDEN, "This origin is not allowed.").into_response(),
            );
        }
        if path != "/api/auth/exchange" {
            let cookie = cookie_value(headers.get(header::COOKIE).and_then(|h| h.to_str().ok()));
            let sent = cookie.is_some();
            if !cookie.is_some_and(|c| app.auth.check_cookie(&c)) {
                let peer = req
                    .extensions()
                    .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                    .map(|c| c.0);
                let cookies_sent = headers
                    .get(header::COOKIE)
                    .and_then(|h| h.to_str().ok())
                    .map(|c| c.matches('=').count())
                    .unwrap_or(0);
                tracing::info!(
                    "unauthorized {path}: session cookie {}; {cookies_sent} cookies sent; {}",
                    if sent {
                        "sent but not recognized"
                    } else {
                        "missing"
                    },
                    request_facts(headers, peer)
                );
                return Some(
                    ApiErr::new(
                        StatusCode::UNAUTHORIZED,
                        "Sign in with the URL the ostra command printed.",
                    )
                    .into_response(),
                );
            }
        }
    }
    None
}

/// `host` is named in the policy only once it passed the allowlist.
fn secure_headers(h: &mut axum::http::HeaderMap, host: Option<&str>, api: bool) {
    let sockets = host
        .map(|x| format!(" ws://{x} wss://{x}"))
        .unwrap_or_default();
    // Scripts only from this origin, and no remote images, because an injected agent could
    // otherwise send data out through a Markdown image URL.
    if let Ok(csp) = HeaderValue::from_str(&format!(
        "default-src 'self'; script-src 'self'; worker-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; font-src 'self' data:; connect-src 'self'{sockets}; \
         object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; frame-src 'none'; \
         manifest-src 'self'"
    )) {
        h.insert("content-security-policy", csp);
    }
    if api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
}

pub fn router(app: Arc<App>) -> axum::Router {
    use axum::routing::delete;
    axum::Router::new()
        .route("/api/info", get(info))
        .route("/api/auth/exchange", post(exchange))
        .route("/api/auth/sessions", get(list_sign_ins))
        .route("/api/auth/sessions/{id}", delete(revoke_sign_in))
        .route(
            "/api/auth/sessions/revoke-others",
            post(revoke_other_sign_ins),
        )
        .route("/api/auth/signout", post(sign_out))
        .route(
            "/api/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route(
            "/api/workspaces/{ws}",
            get(get_workspace)
                .patch(patch_workspace)
                .delete(delete_workspace),
        )
        .route("/api/workspaces/{ws}/validate", post(validate_workspace))
        .route("/api/workspaces/{ws}/settings/fix", post(fix_settings))
        .route("/api/workspaces/{ws}/approve", post(approve_commands))
        .route("/api/workspaces/{ws}/projects", post(import_project))
        .route("/api/workspaces/{ws}/clone", post(clone_project))
        .route(
            "/api/workspaces/{ws}/projects/{key}/pull",
            post(pull_project),
        )
        .route("/api/workspaces/{ws}/projects/{key}/git", get(git_status))
        .route(
            "/api/workspaces/{ws}/projects/{key}/git/branches",
            get(git_branches),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/git/stage",
            post(git_stage),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/git/unstage",
            post(git_unstage),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/git/commit",
            post(git_commit),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/git/fetch",
            post(git_fetch),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/git/push",
            post(git_push),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/git/checkout",
            post(git_checkout),
        )
        .route(
            "/api/git/credentials",
            get(git_credentials).post(create_git_credential),
        )
        .route(
            "/api/git/credentials/{id}",
            axum::routing::patch(patch_git_credential).delete(delete_git_credential),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}",
            delete(remove_project),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/init",
            post(init_project),
        )
        .route("/api/workspaces/{ws}/skills", get(skills_list))
        .route("/api/workspaces/{ws}/mcp", get(mcp_status))
        .route("/api/workspaces/{ws}/mcp/{name}/refresh", post(mcp_refresh))
        .route("/api/workspaces/{ws}/mcp/{name}/login", post(mcp_login))
        .route("/api/workspaces/{ws}/mcp/{name}/logout", post(mcp_logout))
        // Outside /api: the authorization server's redirect is a cross-site navigation, so the
        // SameSite=Strict cookie is not sent. The single-use `state` authenticates it instead.
        .route(crate::mcp::CALLBACK_PATH, get(mcp_callback))
        .route(
            "/api/workspaces/{ws}/projects/{key}/skills/{name}",
            get(skill_get).put(skill_save).delete(skill_delete),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/skills/{name}/adopt",
            post(skill_adopt),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/commands",
            axum::routing::put(project_commands),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/harness-skill",
            get(harness_skill_get),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/memory",
            get(memory_list).patch(memory_edit).delete(memory_delete),
        )
        .route(
            "/api/workspaces/{ws}/sessions",
            get(list_sessions).post(create_session),
        )
        .route("/api/workspaces/{ws}/cost", get(cost))
        .route("/api/workspaces/{ws}/ask", post(ask))
        .route("/api/sessions/{id}", get(get_session))
        .route("/api/sessions/{id}/events", get(session_events))
        .route("/api/sessions/{id}/yolo", post(set_yolo))
        .route("/api/sessions/{id}/amend", post(amend))
        .route("/api/sessions/{id}/pause", post(pause_session))
        .route("/api/executions/{id}/inspect", post(inspect_execution))
        .route("/api/sessions/{id}/resume", post(resume_session))
        .route("/api/sessions/{id}/stop", post(stop_session))
        .route("/api/sessions/{id}/diff", get(diff))
        .route("/api/gates/{id}/answer", post(answer_gate))
        .route("/api/decisions/{id}/override", post(override_decision))
        .route("/api/executions/{id}", get(get_execution))
        .route("/api/executions/{id}/activity", get(activity))
        .route("/api/executions/{id}/cancel", post(cancel))
        .route("/api/executions/{id}/resume", post(resume))
        .route("/api/artifacts", get(artifact))
        .route("/api/artifacts/download", get(artifact_download))
        .route(
            "/api/workspaces/{ws}/uploads",
            post(upload).layer(axum::extract::DefaultBodyLimit::max(
                ostra_engine::uploads::MAX_UPLOAD_BYTES + 1024,
            )),
        )
        .route(
            "/api/workspaces/{ws}/artifacts",
            get(artifacts_list)
                .post(artifact_upload)
                .delete(artifact_delete)
                .layer(axum::extract::DefaultBodyLimit::max(
                    ostra_core::artifacts::MAX_ARTIFACT_BYTES + 1024,
                )),
        )
        .route(
            "/api/workspaces/{ws}/artifacts/download",
            get(workspace_artifact_download),
        )
        .route(
            "/api/workspaces/{ws}/artifacts/hidden",
            post(artifact_hidden),
        )
        .route("/api/workspaces/{ws}/artifacts/move", post(artifact_move))
        .route("/api/workspaces/{ws}/docs", get(books_list))
        .route(
            "/api/workspaces/{ws}/docs/{book}",
            get(book_get).delete(book_delete),
        )
        .route("/api/push/subscribe", post(push_subscribe))
        .route("/api/fs/list", get(fs_list))
        .route("/api/environment", get(environment))
        .route(
            "/api/providers/{name}",
            axum::routing::patch(patch_provider),
        )
        .route("/api/workspaces/validate", post(validate_new_workspace))
        .route("/api/onboarding", get(onboarding))
        .route("/api/onboarding/complete", post(complete_onboarding))
        .route(
            "/api/workspaces/{ws}/ui",
            get(get_ui_state).patch(patch_ui_state),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/tree",
            get(project_tree),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/file",
            // JSON escaping can double a text at the edit cap, past axum's 2 MB default.
            get(project_file)
                .put(save_project_file)
                .layer(axum::extract::DefaultBodyLimit::max(8 * files::TEXT_CAP)),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/mkdir",
            post(project_mkdir),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/files",
            get(project_files),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/diff",
            get(project_diff),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/code/file",
            get(code_file),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/code/usages",
            get(code_usages),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/code/deps",
            get(code_deps),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/code/external",
            get(code_external),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/code/symbols",
            get(code_symbols),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/code/graph",
            get(code_graph),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/code/reindex",
            post(code_reindex),
        )
        .route(
            "/api/workspaces/{ws}/projects/{key}/changes",
            get(project_changes),
        )
        .route("/api/fs", get(fs_browse))
        .route("/api/fs/mkdir", post(fs_mkdir))
        .route("/api/harnesses/{harness}/setup", post(harness_setup))
        .route("/api/workspaces/{ws}/tree", get(workspace_tree))
        .route("/api/workspaces/{ws}/search", get(search))
        .route("/api/workspaces/{ws}/activity", get(workspace_activity))
        .route("/ws", get(crate::ws::handler))
        .merge(crate::bridge::internal_routes())
        .fallback(crate::assets::static_handler)
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

// ---------------------------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------------------------

pub(crate) fn ws(app: &App, id: &str) -> Result<Arc<WorkspaceRt>, ApiErr> {
    app.workspace(&WorkspaceId::from(id))
        .ok_or_else(|| ApiErr::not_found(format!("No workspace {id}.")))
}

/// Hold while starting work, so a removal cannot pass its busy check in between.
async fn start_work(w: &WorkspaceRt) -> Result<tokio::sync::RwLockReadGuard<'_, bool>, ApiErr> {
    let work = w.work.read().await;
    if *work {
        return Err(ApiErr::not_found(format!("No workspace {}.", w.id)));
    }
    Ok(work)
}

fn ws_of_session(app: &App, id: &SessionId) -> Result<Arc<WorkspaceRt>, ApiErr> {
    app.all_workspaces()
        .into_iter()
        .find(|w| w.db.get_session(id).ok().flatten().is_some())
        .ok_or_else(|| ApiErr::not_found(format!("No session {id}.")))
}

fn ws_of_gate(app: &App, id: &GateId) -> Result<Arc<WorkspaceRt>, ApiErr> {
    app.all_workspaces()
        .into_iter()
        .find(|w| w.db.get_gate(id).ok().flatten().is_some())
        .ok_or_else(|| ApiErr::not_found(format!("No gate {id}.")))
}

pub fn ws_of_execution(app: &App, id: &ExecutionId) -> Result<Arc<WorkspaceRt>, ApiErr> {
    app.all_workspaces()
        .into_iter()
        .find(|w| w.db.get_execution(id).ok().flatten().is_some())
        .ok_or_else(|| ApiErr::not_found(format!("No execution {id}.")))
}

fn ws_of_decision(app: &App, id: &DecisionId) -> Result<Arc<WorkspaceRt>, ApiErr> {
    app.all_workspaces()
        .into_iter()
        .find(|w| w.db.decision_session(id).ok().flatten().is_some())
        .ok_or_else(|| ApiErr::not_found(format!("No decision {id}.")))
}

// ---------------------------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------------------------

async fn info(State(app): AppState) -> Json<ServerInfo> {
    Json(ServerInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        vapid_public_key: app.shared.notifier.keys().public_key_b64url(),
    })
}

/// Request facts worth logging when sign-in misbehaves. Never includes token or cookie values.
fn request_facts(
    req_headers: &axum::http::HeaderMap,
    peer: Option<std::net::SocketAddr>,
) -> String {
    let h = |name: &str| {
        req_headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string()
    };
    format!(
        "peer={} host={} origin={} sec-fetch-site={} sec-fetch-mode={} sec-fetch-dest={} sec-purpose={} ua={:?}",
        peer.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
        h("host"),
        h("origin"),
        h("sec-fetch-site"),
        h("sec-fetch-mode"),
        h("sec-fetch-dest"),
        h("sec-purpose"),
        h("user-agent"),
    )
}

async fn exchange(State(app): AppState, req: Request) -> Response {
    let peer = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0);
    let facts = request_facts(req.headers(), peer);
    let ip = peer.map(|p| p.ip());
    if let Some(wait) = app.auth.exchange_wait(ip) {
        tracing::info!("sign-in: rate limited; {facts}");
        return ApiErr::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "Wait {wait} s before trying another sign-in link, because several failed from this address in the last minute. Then run `ostra url` for a fresh one."
            ),
        )
        .into_response();
    }
    let meta = crate::auth::SignInMeta {
        user_agent: req
            .headers()
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(String::from),
        ip: ip.map(|i| i.to_string()),
    };
    // The guard already accepted this Origin, so https here means a TLS proxy in front of Ostra.
    let secure = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .is_some_and(|o| o.to_ascii_lowercase().starts_with("https://"));
    let bytes = axum::body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    let Ok(body) = serde_json::from_slice::<AuthExchange>(&bytes) else {
        tracing::info!("sign-in: unreadable body; {facts}");
        return ApiErr::bad("Send {\"token\": \"...\"}.").into_response();
    };
    let result = app.auth.exchange(&body.token, &meta);
    app.auth.note_exchange(ip, result.is_ok());
    match result {
        Ok(cookie) => {
            tracing::info!("sign-in: token exchanged for a session cookie; {facts}");
            let mut res = StatusCode::NO_CONTENT.into_response();
            if let Ok(v) = HeaderValue::from_str(&Auth::cookie_header(&cookie, secure)) {
                res.headers_mut().insert(header::SET_COOKIE, v);
            }
            res
        }
        Err(e) => {
            tracing::info!("sign-in: refused ({}, {e:?}); {facts}", e.kind());
            ApiErr::new(StatusCode::UNAUTHORIZED, e.message()).into_response()
        }
    }
}

fn caller_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    cookie_value(headers.get(header::COOKIE).and_then(|h| h.to_str().ok()))
}

fn auth_err(e: anyhow::Error) -> ApiErr {
    ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn list_sign_ins(
    State(app): AppState,
    headers: axum::http::HeaderMap,
) -> Res<Vec<SignInSession>> {
    let mine = caller_cookie(&headers);
    Ok(Json(app.auth.sessions(mine.as_deref()).map_err(auth_err)?))
}

async fn revoke_sign_in(
    State(app): AppState,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiErr> {
    if app.auth.revoke(&id).map_err(auth_err)? {
        tracing::info!("sign-in {id} revoked from the browser");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiErr::not_found(format!(
            "No sign-in {id}. It may have expired or been revoked already."
        )))
    }
}

async fn revoke_other_sign_ins(
    State(app): AppState,
    headers: axum::http::HeaderMap,
) -> Res<RevokedSignIns> {
    // The guard let this request through, so the caller has a valid cookie.
    let mine = caller_cookie(&headers).unwrap_or_default();
    let n = app.auth.revoke_others(&mine).map_err(auth_err)?;
    tracing::info!("{n} other sign-ins revoked from the browser");
    Ok(Json(RevokedSignIns { revoked: n as u32 }))
}

async fn sign_out(
    State(app): AppState,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiErr> {
    if let Some(mine) = caller_cookie(&headers) {
        app.auth.sign_out(&mine).map_err(auth_err)?;
    }
    let secure = headers
        .get(header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .is_some_and(|o| o.to_ascii_lowercase().starts_with("https://"));
    let mut res = StatusCode::NO_CONTENT.into_response();
    if let Ok(v) = HeaderValue::from_str(&Auth::clear_cookie_header(secure)) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    Ok(res)
}

async fn list_workspaces(State(app): AppState) -> Res<Vec<WorkspaceSummary>> {
    let mut out = vec![];
    for r in app.shared.registry.list_workspaces()? {
        let rt = app.workspace(&r.id);
        let (projects, active) = match &rt {
            Some(w) => {
                let sessions = w.db.list_sessions().unwrap_or_default();
                (
                    w.settings().projects.len() as u32,
                    sessions
                        .iter()
                        .filter(|s| {
                            matches!(
                                s.status,
                                SessionStatus::Running
                                    | SessionStatus::Waiting
                                    | SessionStatus::Paused
                            )
                        })
                        .count() as u32,
                )
            }
            None => (0, 0),
        };
        out.push(WorkspaceSummary {
            id: r.id,
            name: r.name,
            root: r.root.clone(),
            projects,
            active_sessions: active,
            available: rt.is_some(),
        });
    }
    Ok(Json(out))
}

async fn create_workspace(
    State(app): AppState,
    Json(body): Json<CreateWorkspace>,
) -> Res<WorkspaceDetail> {
    refresh_env(&app).await;
    match crate::setup::create(&app, &body) {
        Ok(rt) => Ok(Json(rt.detail())),
        Err(ostra_workspace::CreateError::Invalid(issues)) => {
            Err(ApiErr::invalid_workspace(issues))
        }
        Err(ostra_workspace::CreateError::Failed(m)) => {
            Err(ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, m))
        }
    }
}

pub(crate) async fn refresh_env(app: &App) {
    if app.shared.env.read().stale() {
        let fresh = crate::env::EnvStatus::detect(&app.shared.global()).await;
        *app.shared.env.write() = fresh;
    }
}

async fn validate_new_workspace(
    State(app): AppState,
    Json(body): Json<CreateWorkspace>,
) -> Res<Vec<ValidationIssue>> {
    refresh_env(&app).await;
    Ok(Json(ostra_workspace::create::validate(
        app.shared.as_ref(),
        &body,
    )))
}

async fn environment(State(app): AppState) -> Json<EnvironmentStatus> {
    Json(crate::setup::environment(&app).await)
}

async fn harness_setup(
    State(app): AppState,
    Path(harness): Path<String>,
    Json(body): Json<HarnessSetupRequest>,
) -> Res<HarnessSetupTerminal> {
    let harness: ostra_core::executor::HarnessKind = harness.parse().map_err(ApiErr::not_found)?;
    crate::harness_setup::start(&app, harness, body.action)
        .map(Json)
        .map_err(|m| ApiErr::new(StatusCode::CONFLICT, m))
}

async fn patch_provider(
    State(app): AppState,
    Path(name): Path<String>,
    Json(edit): Json<ProviderCredentialsEdit>,
) -> Res<ProviderStatus> {
    if !matches!(name.as_str(), "anthropic" | "openai")
        || !app.shared.global().providers.contains_key(&name)
    {
        return Err(ApiErr::not_found(format!(
            "No provider {name} in the global config."
        )));
    }
    let registry = &app.shared.registry;
    let saved = crate::credentials::apply(crate::credentials::load(registry, &name)?, &edit)
        .map_err(|issues| ApiErr::invalid_request(issues, "save the provider"))?;
    crate::credentials::save(registry, &name, &saved)?;
    app.shared.reload_providers()?;
    let status = app
        .shared
        .providers
        .status()
        .into_iter()
        .find(|p| p.name == name);
    status
        .map(Json)
        .ok_or_else(|| ApiErr::not_found(format!("No provider {name} in the global config.")))
}

async fn onboarding(State(app): AppState) -> Res<OnboardingState> {
    Ok(Json(crate::setup::onboarding(&app)?))
}

async fn complete_onboarding(State(app): AppState) -> Res<OnboardingState> {
    app.shared.registry.mark_onboarded()?;
    Ok(Json(crate::setup::onboarding(&app)?))
}

async fn get_ui_state(State(app): AppState, Path(id): Path<String>) -> Res<WorkspaceUiState> {
    Ok(Json(ws(&app, &id)?.ui_state()))
}

async fn patch_ui_state(
    State(app): AppState,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Res<WorkspaceUiState> {
    use ostra_workspace::ui_state::UiStateError;
    ws(&app, &id)?.patch_ui_state(&body).map(Json).map_err(|e| {
        let status = match e {
            UiStateError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            UiStateError::Invalid(_) => StatusCode::BAD_REQUEST,
            UiStateError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiErr::new(status, e.message())
    })
}

async fn get_workspace(State(app): AppState, Path(id): Path<String>) -> Res<WorkspaceDetail> {
    refresh_env(&app).await;
    Ok(Json(ws(&app, &id)?.detail()))
}

async fn patch_workspace(
    State(app): AppState,
    Path(id): Path<String>,
    Json(settings): Json<WorkspaceSettings>,
) -> Res<WorkspaceDetail> {
    let w = ws(&app, &id)?;
    w.save_settings(&settings).map_err(ApiErr::invalid)?;
    if let Some(r) = app.shared.registry.get_workspace(&w.id)?
        && r.name != settings.name
    {
        app.shared
            .registry
            .rename_workspace(&w.id, &settings.name)?;
    }
    Ok(Json(w.detail()))
}

/// Apply the fixes the workspace detail lists, such as a route for an agent added since the
/// settings were saved.
async fn fix_settings(State(app): AppState, Path(id): Path<String>) -> Res<WorkspaceDetail> {
    let w = ws(&app, &id)?;
    w.apply_fixes()
        .map_err(|e| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::git::workspace_updated(&app, &w);
    Ok(Json(w.detail()))
}

async fn delete_workspace(
    State(app): AppState,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiErr> {
    use ostra_workspace::DeleteError;
    app.delete_workspace(&WorkspaceId::from(id.as_str()))
        .map_err(|e| match e {
            DeleteError::NotFound(m) => ApiErr::not_found(m),
            DeleteError::Busy(m) => ApiErr::new(StatusCode::CONFLICT, m),
            DeleteError::Store(e) => e.into(),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Approve the commands of one folder file, as `pending_commands` showed them.
async fn approve_commands(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<ApproveCommands>,
) -> Res<WorkspaceDetail> {
    let w = ws(&app, &id)?;
    ostra_workspace::trust::approve(
        &app.shared.registry,
        &w.root,
        &w.settings(),
        body.project.as_deref(),
        &body.hash,
    )
    .map_err(|e| match e {
        ostra_workspace::trust::ApproveError::NoProject(k) => {
            ApiErr::not_found(format!("No project `{k}` in this workspace."))
        }
        ostra_workspace::trust::ApproveError::Changed => ApiErr::new(
            StatusCode::CONFLICT,
            "Review the commands again, because the file changed after they were shown.",
        ),
    })?;
    app.shared.mcp.forget(&w.root);
    crate::git::workspace_updated(&app, &w);
    Ok(Json(w.detail()))
}

async fn validate_workspace(
    State(app): AppState,
    Path(id): Path<String>,
    Json(settings): Json<WorkspaceSettings>,
) -> Res<Vec<ValidationIssue>> {
    Ok(Json(ws(&app, &id)?.validate(&settings)))
}

async fn import_project(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<ImportProject>,
) -> Res<WorkspaceDetail> {
    let w = ws(&app, &id)?;
    w.import_project(&body).map_err(|e| match e {
        ostra_workspace::CreateError::Invalid(issues) => {
            ApiErr::invalid_request(issues, "import the project")
        }
        ostra_workspace::CreateError::Failed(m) => {
            ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, m)
        }
    })?;
    Ok(Json(w.detail()))
}

async fn clone_project(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<CloneProject>,
) -> Res<WorkspaceDetail> {
    let w = ws(&app, &id)?;
    let plan = crate::git::clone_plan(&w, &app.shared.registry, &body)
        .map_err(|issues| ApiErr::invalid_request(issues, "clone the project"))?;
    // A task of its own, so a closed tab does not kill git halfway and leave a partial checkout.
    let (app2, w2) = (app.clone(), w.clone());
    tokio::spawn(async move { crate::git::clone(&app2, &w2, plan).await })
        .await
        .map_err(|e| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(git_err)?;
    Ok(Json(w.detail()))
}

fn git_err(e: crate::git::GitError) -> ApiErr {
    match e {
        crate::git::GitError::Busy(m) => ApiErr::new(StatusCode::CONFLICT, m),
        crate::git::GitError::Git(m) => ApiErr::new(StatusCode::BAD_GATEWAY, m),
        crate::git::GitError::Invalid(m) => ApiErr::bad(m),
        crate::git::GitError::Import(ostra_workspace::CreateError::Invalid(issues)) => {
            ApiErr::invalid_request(issues, "import the cloned project")
        }
        crate::git::GitError::Import(ostra_workspace::CreateError::Failed(m)) => {
            ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, m)
        }
    }
}

async fn pull_project(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
) -> Res<GitPullResult> {
    let w = ws(&app, &id)?;
    let dir = w
        .project_path(&key)
        .ok_or_else(|| ApiErr::not_found(format!("No project {key}.")))?;
    if !dir.join(".git").exists() {
        return Err(ApiErr::bad(format!(
            "`{key}` is not a git repository, so there is nothing to pull."
        )));
    }
    if let Some(s) = crate::git::session_in(&w, &key)? {
        return Err(ApiErr::new(
            StatusCode::CONFLICT,
            crate::repo::busy_text(&s, &key),
        ));
    }
    let (app2, w2) = (app.clone(), w.clone());
    let out = tokio::spawn(async move { crate::git::pull(&app2, &w2, &key, &dir).await })
        .await
        .map_err(|e| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(git_err)?;
    Ok(Json(out))
}

async fn git_status(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
) -> Res<GitRepoStatus> {
    let w = ws(&app, &id)?;
    let root = crate::files::project_root(&w, &key)?;
    Ok(Json(crate::repo::status(&w, &key, &root).await))
}

async fn git_branches(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
) -> Res<Vec<GitBranch>> {
    let w = ws(&app, &id)?;
    let root = crate::files::project_root(&w, &key)?;
    Ok(Json(crate::repo::branches(&root).await))
}

/// Run one Git dock command in its own task, so a closed tab does not stop git halfway.
async fn git_op<F, Fut>(app: Arc<App>, id: String, key: String, f: F) -> Res<GitOpResult>
where
    F: FnOnce(Arc<App>, Arc<WorkspaceRt>, String, std::path::PathBuf) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<GitOpResult, crate::git::GitError>> + Send,
{
    let w = ws(&app, &id)?;
    let root = crate::files::project_root(&w, &key)?;
    tokio::spawn(async move { f(app, w, key, root).await })
        .await
        .map_err(|e| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(Json)
        .map_err(git_err)
}

async fn git_stage(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Json(body): Json<GitPaths>,
) -> Res<GitOpResult> {
    git_op(app, id, key, |app, w, key, root| async move {
        crate::repo::Op::new(&app, &w, &key, &root)
            .stage(&body)
            .await
    })
    .await
}

async fn git_unstage(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Json(body): Json<GitPaths>,
) -> Res<GitOpResult> {
    git_op(app, id, key, |app, w, key, root| async move {
        crate::repo::Op::new(&app, &w, &key, &root)
            .unstage(&body)
            .await
    })
    .await
}

async fn git_commit(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Json(body): Json<GitCommitRequest>,
) -> Res<GitOpResult> {
    git_op(app, id, key, |app, w, key, root| async move {
        crate::repo::Op::new(&app, &w, &key, &root)
            .commit(&body.message)
            .await
    })
    .await
}

async fn git_fetch(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
) -> Res<GitOpResult> {
    git_op(app, id, key, |app, w, key, root| async move {
        crate::repo::Op::new(&app, &w, &key, &root).fetch().await
    })
    .await
}

async fn git_push(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
) -> Res<GitOpResult> {
    git_op(app, id, key, |app, w, key, root| async move {
        crate::repo::Op::new(&app, &w, &key, &root).push().await
    })
    .await
}

async fn git_checkout(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Json(body): Json<GitCheckoutRequest>,
) -> Res<GitOpResult> {
    git_op(app, id, key, |app, w, key, root| async move {
        crate::repo::Op::new(&app, &w, &key, &root)
            .checkout(&body)
            .await
    })
    .await
}

fn credential_err(e: crate::git::EditError) -> ApiErr {
    match e {
        crate::git::EditError::Invalid(issues) => {
            ApiErr::invalid_request(issues, "save the git credential")
        }
        crate::git::EditError::NotFound => ApiErr::not_found("No such git credential."),
        crate::git::EditError::Store(e) => e.into(),
    }
}

async fn git_credentials(State(app): AppState) -> Res<Vec<GitCredentialView>> {
    Ok(Json(crate::git::views(&app.shared.registry)?))
}

async fn create_git_credential(
    State(app): AppState,
    Json(edit): Json<GitCredentialEdit>,
) -> Res<Vec<GitCredentialView>> {
    crate::git::save(&app.shared.registry, None, &edit).map_err(credential_err)?;
    git_credentials(State(app)).await
}

async fn patch_git_credential(
    State(app): AppState,
    Path(id): Path<String>,
    Json(edit): Json<GitCredentialEdit>,
) -> Res<Vec<GitCredentialView>> {
    crate::git::save(&app.shared.registry, Some(&id), &edit).map_err(credential_err)?;
    git_credentials(State(app)).await
}

async fn delete_git_credential(
    State(app): AppState,
    Path(id): Path<String>,
) -> Res<Vec<GitCredentialView>> {
    if !crate::git::delete(&app.shared.registry, &id)? {
        return Err(ApiErr::not_found("No such git credential."));
    }
    git_credentials(State(app)).await
}

async fn remove_project(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
) -> Res<WorkspaceDetail> {
    let w = ws(&app, &id)?;
    use ostra_workspace::RemoveProjectError;
    w.remove_project(&key).map_err(|e| match e {
        RemoveProjectError::NotFound(m) => ApiErr::not_found(m),
        RemoveProjectError::Busy(m) => ApiErr::new(StatusCode::CONFLICT, m),
        RemoveProjectError::Failed(m) => ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, m),
    })?;
    Ok(Json(w.detail()))
}

async fn init_project(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
) -> Res<SessionSummary> {
    let w = ws(&app, &id)?;
    let _work = start_work(&w).await?;
    Ok(Json(w.engine.create_init_session(&key, None)?))
}

async fn list_sessions(State(app): AppState, Path(id): Path<String>) -> Res<Vec<SessionSummary>> {
    Ok(Json(ws(&app, &id)?.db.list_sessions()?))
}

async fn create_session(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<CreateSession>,
) -> Res<SessionSummary> {
    let w = ws(&app, &id)?;
    let _work = start_work(&w).await?;
    Ok(Json(w.engine.create_session(body)?))
}

async fn get_session(State(app): AppState, Path(id): Path<String>) -> Res<SessionDetail> {
    let sid = SessionId::from(id);
    let w = ws_of_session(&app, &sid)?;
    Ok(Json(w.engine.detail(&sid)?))
}

#[derive(Deserialize)]
struct After {
    after: Option<i64>,
}

async fn session_events(
    State(app): AppState,
    Path(id): Path<String>,
    Query(q): Query<After>,
) -> Res<Vec<StoredEvent>> {
    let sid = SessionId::from(id);
    let w = ws_of_session(&app, &sid)?;
    Ok(Json(w.db.events_after(&sid, q.after.unwrap_or(0))?))
}

async fn set_yolo(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<SetYolo>,
) -> Res<SessionSummary> {
    let sid = SessionId::from(id);
    Ok(Json(
        ws_of_session(&app, &sid)?
            .engine
            .set_yolo(&sid, body.enabled)?,
    ))
}

async fn stop_session(State(app): AppState, Path(id): Path<String>) -> Res<SessionSummary> {
    let sid = SessionId::from(id);
    Ok(Json(ws_of_session(&app, &sid)?.engine.stop_session(&sid)?))
}

async fn amend(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<AmendRequest>,
) -> Res<SessionSummary> {
    let sid = SessionId::from(id);
    Ok(Json(ws_of_session(&app, &sid)?.engine.amend(
        &sid,
        body.text,
        body.files,
        body.uploads,
        body.delivery,
    )?))
}

/// Reopen an ended harness run's session read-only. Returns the new execution.
async fn inspect_execution(State(app): AppState, Path(id): Path<String>) -> Res<ExecutionView> {
    let eid = ExecutionId::from(id);
    let w = ws_of_execution(&app, &eid)?;
    let _work = start_work(&w).await?;
    let new = w.engine.inspect_execution(&eid)?;
    Ok(Json(w.engine.execution(&new)?))
}

async fn pause_session(State(app): AppState, Path(id): Path<String>) -> Res<SessionSummary> {
    let sid = SessionId::from(id);
    Ok(Json(ws_of_session(&app, &sid)?.engine.pause_session(&sid)?))
}

async fn resume_session(State(app): AppState, Path(id): Path<String>) -> Res<SessionSummary> {
    let sid = SessionId::from(id);
    Ok(Json(
        ws_of_session(&app, &sid)?.engine.resume_session(&sid)?,
    ))
}

async fn answer_gate(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<AnswerGate>,
) -> Res<GateView> {
    let gid = GateId::from(id);
    Ok(Json(
        ws_of_gate(&app, &gid)?
            .engine
            .answer_gate(&gid, body.answer)?,
    ))
}

async fn override_decision(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<OverrideDecision>,
) -> Res<DecisionView> {
    let did = DecisionId::from(id);
    let w = ws_of_decision(&app, &did)?;
    w.engine.override_decision(&did, body.output, body.reason)?;
    Ok(Json(
        w.db.get_decision(&did)?
            .ok_or_else(|| ApiErr::not_found("decision"))?,
    ))
}

async fn get_execution(State(app): AppState, Path(id): Path<String>) -> Res<ExecutionView> {
    let eid = ExecutionId::from(id);
    let w = ws_of_execution(&app, &eid)?;
    let mut v = w.engine.execution(&eid)?;
    v.has_terminal = v.has_terminal || app.shared.harness.has_terminal(&eid);
    Ok(Json(v))
}

async fn activity(
    State(app): AppState,
    Path(id): Path<String>,
    Query(q): Query<After>,
) -> Res<Vec<ActivityItem>> {
    let eid = ExecutionId::from(id);
    let w = ws_of_execution(&app, &eid)?;
    Ok(Json(w.db.activity_after(&eid, q.after.unwrap_or(0))?))
}

async fn cancel(State(app): AppState, Path(id): Path<String>) -> Res<ExecutionView> {
    let eid = ExecutionId::from(id);
    let w = ws_of_execution(&app, &eid)?;
    w.engine.cancel_execution(&eid)?;
    Ok(Json(w.engine.execution(&eid)?))
}

async fn resume(State(app): AppState, Path(id): Path<String>) -> Res<ExecutionView> {
    let eid = ExecutionId::from(id);
    let w = ws_of_execution(&app, &eid)?;
    w.engine.resume_execution(&eid)?;
    Ok(Json(w.engine.execution(&eid)?))
}

#[derive(Deserialize)]
struct PathQuery {
    path: String,
}

const ARTIFACT_LIMIT: u64 = 4 * 1024 * 1024;

/// A file inside some workspace's sessions folder, canonicalized (HANDOVER 13).
fn session_file(app: &App, raw: &str) -> Result<(PathBuf, std::fs::Metadata), ApiErr> {
    let path = ostra_core::paths::canonical(PathBuf::from(raw))
        .map_err(|_| ApiErr::not_found(format!("{raw} does not exist.")))?;
    let allowed = app.all_workspaces().iter().any(|w| {
        ostra_core::paths::canonical(paths::sessions_root(&w.root))
            .is_ok_and(|root| paths::is_inside(&root, &path))
    });
    if !allowed {
        return Err(ApiErr::new(
            StatusCode::FORBIDDEN,
            "Only files inside a session directory can be opened here.",
        ));
    }
    let meta = std::fs::metadata(&path).map_err(|e| ApiErr::not_found(e.to_string()))?;
    if !meta.is_file() {
        return Err(ApiErr::bad("That path is not a file."));
    }
    Ok((path, meta))
}

/// Session-dir files only (HANDOVER 13). A binary or large file comes back without content, for
/// download.
async fn artifact(State(app): AppState, Query(q): Query<PathQuery>) -> Res<Artifact> {
    let (path, meta) = session_file(&app, &q.path)?;
    let text = (meta.len() <= ARTIFACT_LIMIT)
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten();
    let Some(content) = text else {
        return Ok(Json(Artifact {
            path,
            content: String::new(),
            headings: vec![],
            document: None,
            binary: true,
            size: meta.len(),
        }));
    };
    let headings = ostra_core::outline::headings(&content);
    let document = ostra_core::doc::load_view(&path);
    Ok(Json(Artifact {
        path,
        content,
        headings,
        document,
        binary: false,
        size: meta.len(),
    }))
}

/// A session file's bytes as a download (Rule C3).
async fn artifact_download(
    State(app): AppState,
    Query(q): Query<PathQuery>,
) -> Result<Response, ApiErr> {
    let (path, _) = session_file(&app, &q.path)?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ApiErr::not_found(e.to_string()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".into());
    Ok(attachment(&name, bytes))
}

// Workspace artifacts (HANDOVER 6.5). The logic lives in `crate::artifacts`.

async fn artifacts_list(State(app): AppState, Path(id): Path<String>) -> Res<WorkspaceArtifacts> {
    Ok(Json(crate::artifacts::list(&*ws(&app, &id)?)))
}

async fn artifact_upload(
    State(app): AppState,
    Path(id): Path<String>,
    Query(q): Query<files::FileQuery>,
    body: axum::body::Bytes,
) -> Res<WorkspaceArtifact> {
    let w = ws(&app, &id)?;
    let a = crate::artifacts::upload(&w, &q.path, &body)?;
    files::announce_save(&app, &w.id, ostra_core::artifacts::TAG_ROOT, &a.path);
    Ok(Json(a))
}

async fn artifact_hidden(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<SetArtifactHidden>,
) -> Res<WorkspaceArtifacts> {
    let w = ws(&app, &id)?;
    crate::artifacts::set_hidden(&w, &body.path, body.hidden)?;
    files::announce_git(&app, &w.id, ostra_core::artifacts::TAG_ROOT);
    Ok(Json(crate::artifacts::list(&w)))
}

async fn artifact_move(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<MoveArtifact>,
) -> Res<WorkspaceArtifacts> {
    let w = ws(&app, &id)?;
    // Rule W4: no session starts between the busy check and the move.
    let Ok(_work) = w.work.try_write() else {
        return Err(ApiErr::new(
            StatusCode::CONFLICT,
            "Try again in a moment, because work is starting in this workspace.",
        ));
    };
    crate::artifacts::move_to(&w, &body.from, &body.to)?;
    files::announce_git(&app, &w.id, ostra_core::artifacts::TAG_ROOT);
    Ok(Json(crate::artifacts::list(&w)))
}

async fn artifact_delete(
    State(app): AppState,
    Path(id): Path<String>,
    Query(q): Query<files::FileQuery>,
) -> Res<WorkspaceArtifacts> {
    let w = ws(&app, &id)?;
    // Rule W4: no session starts between the busy check and the removal.
    let Ok(_work) = w.work.try_write() else {
        return Err(ApiErr::new(
            StatusCode::CONFLICT,
            "Try again in a moment, because work is starting in this workspace.",
        ));
    };
    crate::artifacts::delete(&w, &q.path)?;
    files::announce_git(&app, &w.id, ostra_core::artifacts::TAG_ROOT);
    Ok(Json(crate::artifacts::list(&w)))
}

fn book_param(book: &str) -> Result<&str, ApiErr> {
    if ostra_core::book::is_book_id(book) {
        Ok(book)
    } else {
        Err(ApiErr::new(
            StatusCode::BAD_REQUEST,
            "Name a book by its ID from the book list: lowercase letters, digits, dashes, and underscores.",
        ))
    }
}

async fn books_list(
    State(app): AppState,
    Path(id): Path<String>,
) -> Res<Vec<ostra_core::book::BookSummary>> {
    Ok(Json(ostra_core::book::list(&ws(&app, &id)?.root)))
}

async fn book_get(
    State(app): AppState,
    Path((id, book)): Path<(String, String)>,
) -> Res<ostra_core::book::Book> {
    let w = ws(&app, &id)?;
    ostra_core::book::read(&w.root, book_param(&book)?)
        .map(Json)
        .ok_or_else(|| ApiErr::new(StatusCode::NOT_FOUND, "No such book in this workspace."))
}

async fn book_delete(
    State(app): AppState,
    Path((id, book)): Path<(String, String)>,
) -> Res<Vec<ostra_core::book::BookSummary>> {
    let w = ws(&app, &id)?;
    let book = book_param(&book)?;
    let dir = ostra_core::book::book_dir(&w.root, book);
    if !dir.join("book.json").is_file() {
        return Err(ApiErr::new(StatusCode::NOT_FOUND, "No such book in this workspace."));
    }
    // Rule W4: no session starts between the check and the removal.
    let Ok(_work) = w.work.try_write() else {
        return Err(ApiErr::new(
            StatusCode::CONFLICT,
            "Try again in a moment, because work is starting in this workspace.",
        ));
    };
    ostra_core::book::remove(&w.root, book).map_err(|e| {
        ApiErr::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("The book could not be deleted: {e}"),
        )
    })?;
    Ok(Json(ostra_core::book::list(&w.root)))
}

async fn workspace_artifact_download(
    State(app): AppState,
    Path(id): Path<String>,
    Query(q): Query<files::FileQuery>,
) -> Result<Response, ApiErr> {
    let (name, bytes) = crate::artifacts::download(&*ws(&app, &id)?, &q.path)?;
    Ok(attachment(&name, bytes))
}

/// A download response. The RFC 5987 file name survives any character.
fn attachment(name: &str, bytes: Vec<u8>) -> Response {
    let encoded: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename*=UTF-8''{encoded}"),
            ),
        ],
        bytes,
    )
        .into_response()
}

#[derive(Deserialize)]
struct UploadQuery {
    name: String,
}

/// Stage a file the user uploads as context; a new session or an addition claims it (Rule C3).
async fn upload(
    State(app): AppState,
    Path(id): Path<String>,
    Query(q): Query<UploadQuery>,
    body: axum::body::Bytes,
) -> Res<UploadRef> {
    Ok(Json(ws(&app, &id)?.engine.stage_upload(&q.name, &body)?))
}

#[derive(Deserialize)]
struct DiffQuery {
    project: String,
    phase: Option<u32>,
}

async fn git_show(root: &std::path::Path, rev_path: &str) -> String {
    let filters = ostra_core::git::filter_overrides(root).await;
    let out = tokio::process::Command::new("git")
        .args(ostra_core::git::AUTOMATIC.iter())
        .args(&filters)
        .arg("-C")
        .arg(root)
        .args(["show", "--no-textconv", "--end-of-options"])
        .arg(rev_path)
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => String::new(),
    }
}

/// The review loop's changed files against HEAD, for the ledger's diff view.
async fn diff(
    State(app): AppState,
    Path(id): Path<String>,
    Query(q): Query<DiffQuery>,
) -> Res<Vec<DiffFile>> {
    let sid = SessionId::from(id);
    let w = ws_of_session(&app, &sid)?;
    let st = w.engine.state(&sid)?;
    let root = st
        .project_path(&q.project)
        .ok_or_else(|| ApiErr::not_found("No such project in this session."))?;
    let mut files: std::collections::BTreeSet<String> = Default::default();
    for p in st
        .phases
        .values()
        .filter(|p| p.info.project == q.project && q.phase.is_none_or(|n| n == p.info.id))
    {
        files.extend(p.impl_loop.changed.iter().cloned());
        files.extend(p.test_loop.changed.iter().cloned());
    }
    let mut out = vec![];
    for f in files.into_iter().take(200) {
        // Resolved, because an agent can leave a symlink in the project that points outside it.
        let full = paths::resolve(&root, std::path::Path::new(&f));
        if !paths::is_inside(&paths::resolve(&root, &root), &full) {
            continue;
        }
        out.push(DiffFile {
            original: git_show(&root, &format!("HEAD:{f}")).await,
            modified: std::fs::read_to_string(&full).unwrap_or_default(),
            path: f,
        });
    }
    Ok(Json(out))
}

fn memory_store(w: &WorkspaceRt, key: &str) -> Result<MemoryStore, ApiErr> {
    let path = w
        .project_path(key)
        .ok_or_else(|| ApiErr::not_found(format!("No project {key}.")))?;
    Ok(MemoryStore::open(&paths::project_memory_db(&path))?)
}

#[derive(Deserialize)]
struct MemQuery {
    q: Option<String>,
}

async fn memory_list(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<MemQuery>,
) -> Res<Vec<Lesson>> {
    let w = ws(&app, &id)?;
    Ok(Json(memory_store(&w, &key)?.list(
        q.q.as_deref(),
        500,
        0,
    )?))
}

async fn memory_edit(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Json(body): Json<LessonEdit>,
) -> Res<Lesson> {
    let w = ws(&app, &id)?;
    let store = memory_store(&w, &key)?;
    if body.area.trim().is_empty() || body.lesson.trim().is_empty() {
        return Err(ApiErr::bad("A lesson needs an area and text."));
    }
    let lesson = match body.id {
        Some(lid) => store.update(lid, body.area.trim(), body.lesson.trim())?,
        None => {
            store.record(body.area.trim(), body.lesson.trim(), "user")?;
            store
                .list(None, 500, 0)?
                .into_iter()
                .find(|l| l.area == body.area.trim() && l.lesson == body.lesson.trim())
                .ok_or_else(|| ApiErr::not_found("lesson"))?
        }
    };
    Ok(Json(lesson))
}

#[derive(Deserialize)]
struct IdQuery {
    id: i64,
}

async fn memory_delete(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<IdQuery>,
) -> Result<StatusCode, ApiErr> {
    let w = ws(&app, &id)?;
    memory_store(&w, &key)?.delete(q.id)?;
    Ok(StatusCode::NO_CONTENT)
}

// Skills. The logic lives in `crate::skills`.

async fn mcp_status(State(app): AppState, Path(id): Path<String>) -> Res<Vec<McpServerStatus>> {
    let w = ws(&app, &id)?;
    Ok(Json(app.shared.mcp.status(&w.root, None).await))
}

async fn mcp_one(app: &App, w: &WorkspaceRt, name: &str, force: bool) -> Res<McpServerStatus> {
    app.shared
        .mcp
        .one(&w.root, name, force)
        .await
        .map(Json)
        .ok_or_else(|| ApiErr::not_found(format!("This workspace has no MCP server `{name}`.")))
}

async fn mcp_refresh(
    State(app): AppState,
    Path((id, name)): Path<(String, String)>,
) -> Res<McpServerStatus> {
    let w = ws(&app, &id)?;
    mcp_one(&app, &w, &name, true).await
}

async fn mcp_login(
    State(app): AppState,
    Path((id, name)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
) -> Res<McpLogin> {
    let w = ws(&app, &id)?;
    // The guard has checked Origin and Host, so either names this server as the browser sees it.
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|h| h.to_str().ok())
        .filter(|o| o.starts_with("http"))
        .map(String::from)
        .or_else(|| {
            headers
                .get(header::HOST)
                .and_then(|h| h.to_str().ok())
                .map(|h| format!("http://{h}"))
        })
        .ok_or_else(|| ApiErr::bad("The request names no host to return to after sign-in."))?;
    let url = app
        .shared
        .mcp
        .login(&w.root, &name, &origin)
        .await
        .map_err(|e| ApiErr::new(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(McpLogin {
        authorization_url: url,
    }))
}

async fn mcp_logout(
    State(app): AppState,
    Path((id, name)): Path<(String, String)>,
) -> Res<McpServerStatus> {
    let w = ws(&app, &id)?;
    app.shared.mcp.logout(&w.root, &name).await;
    mcp_one(&app, &w, &name, false).await
}

#[derive(Deserialize)]
struct OAuthCallback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn mcp_callback(State(app): AppState, Query(q): Query<OAuthCallback>) -> Response {
    let outcome = match (&q.state, &q.code, &q.error) {
        (_, _, Some(e)) => Err(format!(
            "The authorization server refused the sign-in: {e}{}",
            q.error_description
                .as_deref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default()
        )),
        (Some(state), Some(code), None) => app.shared.mcp.callback(state, code).await,
        _ => {
            Err("The sign-in link is missing its code. Start the sign-in again from Ostra.".into())
        }
    };
    let (status, title, body) = match outcome {
        Ok(name) => (
            StatusCode::OK,
            "Signed in",
            format!(
                "Ostra is signed in to MCP server <code>{}</code>. Close this tab and return to Ostra.",
                html_escape(&name)
            ),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, "Sign-in failed", html_escape(&e)),
    };
    let page = format!(
        "<!doctype html><meta charset=utf-8><title>{title}</title><body style=\"font-family:system-ui;max-width:36rem;margin:4rem auto;padding:0 1rem\"><h1>{title}</h1><p>{body}</p>"
    );
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        page,
    )
        .into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn skills_list(State(app): AppState, Path(id): Path<String>) -> Res<Vec<ProjectSkills>> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::skills::list(&w)))
}

async fn skill_get(
    State(app): AppState,
    Path((id, key, name)): Path<(String, String, String)>,
) -> Res<SkillDoc> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::skills::get(&w, &key, &name)?))
}

async fn project_commands(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Json(body): Json<ostra_core::config::Commands>,
) -> Res<ostra_core::config::Commands> {
    let w = ws(&app, &id)?;
    let saved = crate::commands::save(&w, &key, body)?;
    // ProjectView carries the profile, so browsers holding the workspace refetch it.
    crate::git::workspace_updated(&app, &w);
    Ok(Json(saved))
}

async fn skill_save(
    State(app): AppState,
    Path((id, key, name)): Path<(String, String, String)>,
    Json(body): Json<SkillSave>,
) -> Res<SkillDoc> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::skills::save(&w, &key, &name, &body)?))
}

async fn skill_delete(
    State(app): AppState,
    Path((id, key, name)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiErr> {
    let w = ws(&app, &id)?;
    crate::skills::delete(&w, &key, &name)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn skill_adopt(
    State(app): AppState,
    Path((id, key, name)): Path<(String, String, String)>,
    Json(body): Json<SkillAdopt>,
) -> Res<SkillDoc> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::skills::adopt(&w, &key, &name, &body)?))
}

async fn harness_skill_get(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<PathQuery>,
) -> Res<SkillDoc> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::skills::get_harness(&w, &key, &q.path)?))
}

#[derive(Deserialize)]
struct CostQuery {
    since: Option<String>,
}

async fn cost(
    State(app): AppState,
    Path(id): Path<String>,
    Query(q): Query<CostQuery>,
) -> Res<CostReport> {
    let since = match q.since.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|_| {
                    ApiErr::bad(format!(
                        "`since` must be an RFC 3339 time such as 2026-09-24T00:00:00Z, not `{s}`."
                    ))
                })?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };
    Ok(Json(ws(&app, &id)?.db.cost_report(since)?))
}

async fn ask(
    State(app): AppState,
    Path(id): Path<String>,
    Json(body): Json<AskQuestion>,
) -> Res<AskStarted> {
    let w = ws(&app, &id)?;
    if body.question.trim().is_empty() {
        return Err(ApiErr::bad("Ask a question first."));
    }
    let _work = start_work(&w).await?;
    Ok(Json(AskStarted {
        execution: w.engine.ask(body.question, body.session).await?,
    }))
}

async fn push_subscribe(
    State(app): AppState,
    Json(body): Json<PushSubscription>,
) -> Result<StatusCode, ApiErr> {
    if !body.endpoint.starts_with("https://") {
        return Err(ApiErr::bad("Push endpoints must use https."));
    }
    app.shared.registry.add_push_subscription(&body, None)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct FsQuery {
    path: Option<String>,
}

async fn fs_list(Query(q): Query<FsQuery>) -> Res<FsListing> {
    let start = q
        .path
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
    let path = ostra_core::paths::canonical(&start)
        .map_err(|_| ApiErr::not_found(format!("{} does not exist.", start.display())))?;
    let mut entries = vec![];
    let read = std::fs::read_dir(&path)
        .map_err(|e| ApiErr::bad(format!("Cannot read {}: {e}", path.display())))?;
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        entries.push(FsEntry {
            is_git: p.join(".git").exists(),
            is_ostra_project: paths::project_inventory(&p).exists(),
            name,
            is_dir: true,
        });
    }
    entries.sort_by_key(|e| e.name.to_lowercase());
    Ok(Json(FsListing {
        parent: path.parent().map(|p| p.to_path_buf()),
        path,
        entries,
    }))
}

// Project files (read-only). The logic lives in `crate::files`.

async fn project_tree(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<files::TreeQuery>,
) -> Res<ProjectTree> {
    let w = ws(&app, &id)?;
    Ok(Json(app.files.tree(&w, &key, q).await?))
}

async fn project_file(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<files::FileQuery>,
) -> Res<ProjectFile> {
    let w = ws(&app, &id)?;
    Ok(Json(app.files.file(&w, &key, &q.path).await?))
}

async fn project_mkdir(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Json(body): Json<CreateProjectFolder>,
) -> Res<ProjectTree> {
    let w = ws(&app, &id)?;
    let (rel, listing) = app.files.mkdir(&w, &key, &body.path).await?;
    files::announce_save(&app, &w.id, &key, &rel);
    Ok(Json(listing))
}

async fn save_project_file(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Json(body): Json<SaveProjectFile>,
) -> Res<ProjectFile> {
    let w = ws(&app, &id)?;
    let saved = app.files.save(&w, &key, body).await?;
    files::announce_save(&app, &w.id, &key, &saved.path);
    Ok(Json(saved))
}

/// Responds with a [`FileIndex`].
async fn project_files(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
) -> Result<Response, ApiErr> {
    let w = ws(&app, &id)?;
    let index = app.files.index(&w, &key).await?;
    Ok(Json(index.as_ref()).into_response())
}

async fn project_diff(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<files::DiffQuery>,
) -> Res<FileDiff> {
    let w = ws(&app, &id)?;
    Ok(Json(app.files.diff(&w, &key, q).await?))
}

async fn project_changes(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
) -> Res<Vec<ProjectChange>> {
    let w = ws(&app, &id)?;
    Ok(Json(app.files.changes(&w, &key).await?))
}

// Code navigation. The logic lives in `crate::code` and `ostra-code`.

#[derive(Deserialize)]
struct CodePathQuery {
    path: String,
}

#[derive(Deserialize)]
struct CodeUsagesQuery {
    symbol: String,
    path: Option<String>,
    /// An outside file a language server pointed at, instead of `path`.
    uri: Option<String>,
    line: Option<u32>,
    col: Option<u32>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct CodeSymbolsQuery {
    q: Option<String>,
    limit: Option<u32>,
}

async fn ask_code(
    app: &Arc<App>,
    id: &str,
    key: &str,
    ask: crate::code::Ask,
) -> Result<Answer, ApiErr> {
    let w = ws(app, id)?;
    let answer = app.code.ask(&app.files, &w, key, ask).await?;
    if let (Ok(root), Ok(list)) = (files::project_root(&w, key), app.files.index(&w, key).await) {
        app.code
            .ensure_watch(app, &(w.id.clone(), key.to_string()), &root, &list);
    }
    Ok(answer)
}

fn wrong_answer() -> ApiErr {
    ApiErr::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "The code provider answered a different request.",
    )
}

async fn code_file(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<CodePathQuery>,
) -> Res<CodeFile> {
    match ask_code(&app, &id, &key, crate::code::Ask::File { path: q.path }).await? {
        Answer::File(f) => Ok(Json(f)),
        _ => Err(wrong_answer()),
    }
}

async fn code_usages(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<CodeUsagesQuery>,
) -> Res<CodeUsages> {
    if let Some(uri) = q.uri.filter(|u| !u.is_empty()) {
        let line = q.line.filter(|&l| l > 0).ok_or_else(|| {
            ApiErr::new(
                StatusCode::BAD_REQUEST,
                "Name the 1-based line of the symbol in the dependency file.",
            )
        })?;
        let w = ws(&app, &id)?;
        let u = app
            .code
            .external_usages(&w, &key, &uri, q.symbol.trim(), (line, q.col), q.limit)
            .await?;
        return Ok(Json(u));
    }
    let ask = crate::code::Ask::Usages {
        symbol: q.symbol,
        path: q.path,
        line: q.line,
        col: q.col,
        limit: q.limit,
    };
    match ask_code(&app, &id, &key, ask).await? {
        Answer::Usages(u) => Ok(Json(u)),
        _ => Err(wrong_answer()),
    }
}

#[derive(Deserialize)]
struct CodeExternalQuery {
    uri: String,
}

async fn code_external(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<CodeExternalQuery>,
) -> Res<CodeExternalFile> {
    let w = ws(&app, &id)?;
    Ok(Json(app.code.external_file(&w, &key, &q.uri).await?))
}

async fn code_deps(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<CodePathQuery>,
) -> Res<CodeDeps> {
    match ask_code(&app, &id, &key, crate::code::Ask::Deps { path: q.path }).await? {
        Answer::Deps(d) => Ok(Json(d)),
        _ => Err(wrong_answer()),
    }
}

#[derive(Deserialize)]
struct CodeGraphQuery {
    package: Option<String>,
    path: Option<String>,
    symbol: Option<String>,
    line: Option<u32>,
    depth: Option<u32>,
}

async fn code_graph(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<CodeGraphQuery>,
) -> Res<ostra_core::code::CodeGraph> {
    let ask = match (q.path.filter(|p| !p.is_empty()), q.package) {
        (Some(_), Some(_)) => {
            return Err(ApiErr::new(
                StatusCode::BAD_REQUEST,
                "Give either package or path, not both.",
            ));
        }
        (Some(path), None) => match q.symbol.filter(|s| !s.is_empty()) {
            Some(symbol) => crate::code::GraphAsk::Symbol {
                path,
                symbol,
                line: q.line,
                depth: q.depth.unwrap_or(1),
            },
            None => crate::code::GraphAsk::File {
                path,
                depth: q.depth.unwrap_or(1),
            },
        },
        // `.` names the package at the project top, whose folder is the empty string.
        (None, Some(package)) => crate::code::GraphAsk::Package(if package == "." {
            String::new()
        } else {
            package
        }),
        (None, None) => crate::code::GraphAsk::Packages,
    };
    let w = ws(&app, &id)?;
    Ok(Json(app.code.graph(&app, &w, &key, ask).await?))
}

async fn code_reindex(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
) -> Res<ostra_core::code::CodeReindex> {
    let w = ws(&app, &id)?;
    Ok(Json(app.code.reindex(&app, &w, &key).await?))
}

async fn code_symbols(
    State(app): AppState,
    Path((id, key)): Path<(String, String)>,
    Query(q): Query<CodeSymbolsQuery>,
) -> Res<CodeSymbols> {
    let ask = crate::code::Ask::Symbols {
        query: q.q.unwrap_or_default(),
        limit: q.limit,
    };
    match ask_code(&app, &id, &key, ask).await? {
        Answer::Symbols(s) => Ok(Json(s)),
        _ => Err(wrong_answer()),
    }
}

// Navigation. The logic lives in `crate::nav`.

fn blocking_failed(e: tokio::task::JoinError) -> ApiErr {
    ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn workspace_tree(State(app): AppState, Path(id): Path<String>) -> Res<WorkspaceTree> {
    let w = ws(&app, &id)?;
    let tree = tokio::task::spawn_blocking(move || app.nav.tree(&w))
        .await
        .map_err(blocking_failed)??;
    Ok(Json(tree))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<usize>,
}

async fn search(
    State(app): AppState,
    Path(id): Path<String>,
    Query(q): Query<SearchQuery>,
) -> Res<SearchResults> {
    let w = ws(&app, &id)?;
    Ok(Json(
        crate::nav::search::search(&app, &w, q.q.as_deref().unwrap_or(""), q.limit).await?,
    ))
}

async fn workspace_activity(
    State(app): AppState,
    Path(id): Path<String>,
) -> Res<WorkspaceActivity> {
    let w = ws(&app, &id)?;
    let activity =
        tokio::task::spawn_blocking(move || crate::nav::activity::build(&w, chrono::Local::now()))
            .await
            .map_err(blocking_failed)??;
    Ok(Json(activity))
}

async fn fs_mkdir(State(app): AppState, Json(body): Json<FsMkdir>) -> Res<FsBrowse> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    tokio::task::spawn_blocking(move || app.files.browse.mkdir(&body.path, &home))
        .await
        .map_err(blocking_failed)?
        .map(Json)
        .map_err(ApiErr::bad)
}

async fn fs_browse(State(app): AppState, Query(q): Query<files::BrowseQuery>) -> Json<FsBrowse> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    Json(
        app.files
            .browse
            .browse(q.path.as_deref(), q.prefix.as_deref(), q.limit, &home),
    )
}
