//! REST routes (HANDOVER 13) and the security middleware (HANDOVER 15).

use crate::app::App;
use crate::auth::{Auth, cookie_value};
use crate::files;
use crate::workspace::WorkspaceRt;
use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use ostra_core::api::*;
use ostra_core::config::{ValidationIssue, WorkspaceSettings};
use ostra_core::event::StoredEvent;
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
use ostra_core::paths;
use ostra_engine::EngineError;
use ostra_store::MemoryStore;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

type AppState = State<Arc<App>>;

pub struct ApiErr {
    status: StatusCode,
    body: ApiError,
}

impl ApiErr {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        ApiErr { status, body: ApiError { error: message.into(), issues: vec![] } }
    }
    fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }
    fn invalid(issues: Vec<ValidationIssue>) -> Self {
        ApiErr {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body: ApiError { error: "The settings have problems. Fix them and save again.".into(), issues },
        }
    }
    fn invalid_workspace(issues: Vec<ValidationIssue>) -> Self {
        Self::invalid_request(issues, "create the workspace")
    }

    /// A 422 whose message carries the issues too, for clients that show only `error`.
    fn invalid_request(issues: Vec<ValidationIssue>, retry: &str) -> Self {
        let error = match issues.as_slice() {
            [one] => one.message.clone(),
            _ => format!("Fix these problems and {retry} again: {}", issues.iter().map(|i| i.message.as_str()).collect::<Vec<_>>().join(" ")),
        };
        ApiErr { status: StatusCode::UNPROCESSABLE_ENTITY, body: ApiError { error, issues } }
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
    let headers = req.headers();
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    if !app.auth.allowed_host(host) {
        return ApiErr::new(StatusCode::MISDIRECTED_REQUEST, "This host is not allowed.").into_response();
    }
    let path = req.uri().path().to_string();
    // Harnesses always run on this machine, so the hook bridge and MCP shim refuse other peers
    // even when the server listens on every interface.
    if path.starts_with("/internal/")
        && let Some(axum::extract::ConnectInfo(peer)) = req.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        && !crate::auth::is_local_address(peer.ip())
    {
        return ApiErr::new(StatusCode::FORBIDDEN, "This endpoint accepts local connections only.").into_response();
    }
    let api = path.starts_with("/api/") || path == "/ws";
    if api {
        let origin = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok());
        if !app.auth.allowed_origin(origin) {
            return ApiErr::new(StatusCode::FORBIDDEN, "This origin is not allowed.").into_response();
        }
        if path != "/api/auth/exchange" {
            let cookie = cookie_value(headers.get(header::COOKIE).and_then(|h| h.to_str().ok()));
            let sent = cookie.is_some();
            if !cookie.is_some_and(|c| app.auth.check_cookie(&c)) {
                let peer = req.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0);
                let cookies_sent = headers.get(header::COOKIE).and_then(|h| h.to_str().ok()).map(|c| c.matches('=').count()).unwrap_or(0);
                tracing::info!(
                    "unauthorized {path}: session cookie {}; {cookies_sent} cookies sent; {}",
                    if sent { "sent but not recognized" } else { "missing" },
                    request_facts(headers, peer)
                );
                return ApiErr::new(StatusCode::UNAUTHORIZED, "Sign in with the URL the ostra command printed.").into_response();
            }
        }
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    res
}

pub fn router(app: Arc<App>) -> axum::Router {
    use axum::routing::delete;
    axum::Router::new()
        .route("/api/info", get(info))
        .route("/api/auth/exchange", post(exchange))
        .route("/api/workspaces", get(list_workspaces).post(create_workspace))
        .route("/api/workspaces/{ws}", get(get_workspace).patch(patch_workspace))
        .route("/api/workspaces/{ws}/validate", post(validate_workspace))
        .route("/api/workspaces/{ws}/projects", post(import_project))
        .route("/api/workspaces/{ws}/projects/{key}", delete(remove_project))
        .route("/api/workspaces/{ws}/projects/{key}/init", post(init_project))
        .route("/api/workspaces/{ws}/skills", get(skills_list))
        .route("/api/workspaces/{ws}/projects/{key}/skills/{name}", get(skill_get).put(skill_save).delete(skill_delete))
        .route("/api/workspaces/{ws}/projects/{key}/skills/{name}/adopt", post(skill_adopt))
        .route("/api/workspaces/{ws}/projects/{key}/harness-skill", get(harness_skill_get))
        .route("/api/workspaces/{ws}/projects/{key}/memory", get(memory_list).patch(memory_edit).delete(memory_delete))
        .route("/api/workspaces/{ws}/sessions", get(list_sessions).post(create_session))
        .route("/api/workspaces/{ws}/cost", get(cost))
        .route("/api/workspaces/{ws}/ask", post(ask))
        .route("/api/sessions/{id}", get(get_session))
        .route("/api/sessions/{id}/events", get(session_events))
        .route("/api/sessions/{id}/yolo", post(set_yolo))
        .route("/api/sessions/{id}/amend", post(amend))
        .route("/api/sessions/{id}/stop", post(stop_session))
        .route("/api/sessions/{id}/diff", get(diff))
        .route("/api/gates/{id}/answer", post(answer_gate))
        .route("/api/decisions/{id}/override", post(override_decision))
        .route("/api/executions/{id}", get(get_execution))
        .route("/api/executions/{id}/activity", get(activity))
        .route("/api/executions/{id}/cancel", post(cancel))
        .route("/api/executions/{id}/resume", post(resume))
        .route("/api/artifacts", get(artifact))
        .route("/api/push/subscribe", post(push_subscribe))
        .route("/api/fs/list", get(fs_list))
        .route("/api/environment", get(environment))
        .route("/api/workspaces/validate", post(validate_new_workspace))
        .route("/api/onboarding", get(onboarding))
        .route("/api/onboarding/complete", post(complete_onboarding))
        .route("/api/workspaces/{ws}/ui", get(get_ui_state).patch(patch_ui_state))
        .route("/api/workspaces/{ws}/projects/{key}/tree", get(project_tree))
        .route("/api/workspaces/{ws}/projects/{key}/file", get(project_file))
        .route("/api/workspaces/{ws}/projects/{key}/files", get(project_files))
        .route("/api/workspaces/{ws}/projects/{key}/diff", get(project_diff))
        .route("/api/workspaces/{ws}/projects/{key}/changes", get(project_changes))
        .route("/api/fs", get(fs_browse))
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

fn ws(app: &App, id: &str) -> Result<Arc<WorkspaceRt>, ApiErr> {
    app.workspace(&WorkspaceId::from(id)).ok_or_else(|| ApiErr::not_found(format!("No workspace {id}.")))
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
fn request_facts(req_headers: &axum::http::HeaderMap, peer: Option<std::net::SocketAddr>) -> String {
    let h = |name: &str| req_headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
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
    let peer = req.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0);
    let facts = request_facts(req.headers(), peer);
    // The guard already accepted this Origin, so https here means a TLS proxy in front of Ostra.
    let secure = req.headers().get(header::ORIGIN).and_then(|o| o.to_str().ok()).is_some_and(|o| o.to_ascii_lowercase().starts_with("https://"));
    let bytes = axum::body::to_bytes(req.into_body(), 64 * 1024).await.unwrap_or_default();
    let Ok(body) = serde_json::from_slice::<AuthExchange>(&bytes) else {
        tracing::info!("sign-in: unreadable body; {facts}");
        return ApiErr::bad("Send {\"token\": \"...\"}.").into_response();
    };
    match app.auth.exchange(&body.token) {
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

async fn list_workspaces(State(app): AppState) -> Res<Vec<WorkspaceSummary>> {
    let mut out = vec![];
    for r in app.shared.registry.list_workspaces()? {
        let rt = app.workspace(&r.id);
        let (projects, active) = match &rt {
            Some(w) => {
                let sessions = w.db.list_sessions().unwrap_or_default();
                (
                    w.settings().projects.len() as u32,
                    sessions.iter().filter(|s| matches!(s.status, SessionStatus::Running | SessionStatus::Waiting)).count() as u32,
                )
            }
            None => (0, 0),
        };
        out.push(WorkspaceSummary { id: r.id, name: r.name, root: r.root.clone(), projects, active_sessions: active, available: rt.is_some() });
    }
    Ok(Json(out))
}

async fn create_workspace(State(app): AppState, Json(body): Json<CreateWorkspace>) -> Res<WorkspaceDetail> {
    refresh_env(&app).await;
    match crate::setup::create(&app, &body) {
        Ok(rt) => Ok(Json(rt.detail())),
        Err(crate::setup::CreateError::Invalid(issues)) => Err(ApiErr::invalid_workspace(issues)),
        Err(crate::setup::CreateError::Failed(m)) => Err(ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, m)),
    }
}

pub(crate) async fn refresh_env(app: &App) {
    if app.shared.env.read().stale() {
        let fresh = crate::env::EnvStatus::detect(&app.shared.global()).await;
        *app.shared.env.write() = fresh;
    }
}

async fn validate_new_workspace(State(app): AppState, Json(body): Json<CreateWorkspace>) -> Res<Vec<ValidationIssue>> {
    refresh_env(&app).await;
    Ok(Json(crate::setup::validate(&app, &body)))
}

async fn environment(State(app): AppState) -> Json<EnvironmentStatus> {
    Json(crate::setup::environment(&app).await)
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

async fn patch_ui_state(State(app): AppState, Path(id): Path<String>, body: axum::body::Bytes) -> Res<WorkspaceUiState> {
    use crate::ui_state::UiStateError;
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

async fn patch_workspace(State(app): AppState, Path(id): Path<String>, Json(settings): Json<WorkspaceSettings>) -> Res<WorkspaceDetail> {
    let w = ws(&app, &id)?;
    w.save_settings(&settings).map_err(ApiErr::invalid)?;
    if let Some(r) = app.shared.registry.get_workspace(&w.id)?
        && r.name != settings.name
    {
        app.shared.registry.rename_workspace(&w.id, &settings.name)?;
    }
    Ok(Json(w.detail()))
}

async fn validate_workspace(State(app): AppState, Path(id): Path<String>, Json(settings): Json<WorkspaceSettings>) -> Res<Vec<ValidationIssue>> {
    Ok(Json(ws(&app, &id)?.validate(&settings)))
}

async fn import_project(State(app): AppState, Path(id): Path<String>, Json(body): Json<ImportProject>) -> Res<WorkspaceDetail> {
    let w = ws(&app, &id)?;
    w.import_project(&body).map_err(|e| match e {
        crate::setup::CreateError::Invalid(issues) => ApiErr::invalid_request(issues, "import the project"),
        crate::setup::CreateError::Failed(m) => ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, m),
    })?;
    Ok(Json(w.detail()))
}

async fn remove_project(State(app): AppState, Path((id, key)): Path<(String, String)>) -> Res<WorkspaceDetail> {
    let w = ws(&app, &id)?;
    w.remove_project(&key).map_err(ApiErr::not_found)?;
    Ok(Json(w.detail()))
}

async fn init_project(State(app): AppState, Path((id, key)): Path<(String, String)>) -> Res<SessionSummary> {
    let w = ws(&app, &id)?;
    Ok(Json(w.engine.create_init_session(&key, None)?))
}

async fn list_sessions(State(app): AppState, Path(id): Path<String>) -> Res<Vec<SessionSummary>> {
    Ok(Json(ws(&app, &id)?.db.list_sessions()?))
}

async fn create_session(State(app): AppState, Path(id): Path<String>, Json(body): Json<CreateSession>) -> Res<SessionSummary> {
    Ok(Json(ws(&app, &id)?.engine.create_session(body)?))
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

async fn session_events(State(app): AppState, Path(id): Path<String>, Query(q): Query<After>) -> Res<Vec<StoredEvent>> {
    let sid = SessionId::from(id);
    let w = ws_of_session(&app, &sid)?;
    Ok(Json(w.db.events_after(&sid, q.after.unwrap_or(0))?))
}

async fn set_yolo(State(app): AppState, Path(id): Path<String>, Json(body): Json<SetYolo>) -> Res<SessionSummary> {
    let sid = SessionId::from(id);
    Ok(Json(ws_of_session(&app, &sid)?.engine.set_yolo(&sid, body.enabled)?))
}

async fn stop_session(State(app): AppState, Path(id): Path<String>) -> Res<SessionSummary> {
    let sid = SessionId::from(id);
    Ok(Json(ws_of_session(&app, &sid)?.engine.stop_session(&sid)?))
}

async fn amend(State(app): AppState, Path(id): Path<String>, Json(body): Json<AmendRequest>) -> Res<SessionSummary> {
    let sid = SessionId::from(id);
    Ok(Json(ws_of_session(&app, &sid)?.engine.amend(&sid, body.text)?))
}

async fn answer_gate(State(app): AppState, Path(id): Path<String>, Json(body): Json<AnswerGate>) -> Res<GateView> {
    let gid = GateId::from(id);
    Ok(Json(ws_of_gate(&app, &gid)?.engine.answer_gate(&gid, body.answer)?))
}

async fn override_decision(State(app): AppState, Path(id): Path<String>, Json(body): Json<OverrideDecision>) -> Res<DecisionView> {
    let did = DecisionId::from(id);
    let w = ws_of_decision(&app, &did)?;
    w.engine.override_decision(&did, body.output, body.reason)?;
    Ok(Json(w.db.get_decision(&did)?.ok_or_else(|| ApiErr::not_found("decision"))?))
}

async fn get_execution(State(app): AppState, Path(id): Path<String>) -> Res<ExecutionView> {
    let eid = ExecutionId::from(id);
    let w = ws_of_execution(&app, &eid)?;
    let mut v = w.engine.execution(&eid)?;
    v.has_terminal = v.has_terminal || app.shared.harness.has_terminal(&eid);
    Ok(Json(v))
}

async fn activity(State(app): AppState, Path(id): Path<String>, Query(q): Query<After>) -> Res<Vec<ActivityItem>> {
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

/// Session-dir files only (HANDOVER 13).
async fn artifact(State(app): AppState, Query(q): Query<PathQuery>) -> Res<Artifact> {
    let path = std::fs::canonicalize(PathBuf::from(&q.path)).map_err(|_| ApiErr::not_found(format!("{} does not exist.", q.path)))?;
    let allowed = app.all_workspaces().iter().any(|w| {
        std::fs::canonicalize(paths::sessions_root(&w.root)).is_ok_and(|root| paths::is_inside(&root, &path))
    });
    if !allowed {
        return Err(ApiErr::new(StatusCode::FORBIDDEN, "Only files inside a session directory can be opened here."));
    }
    let meta = std::fs::metadata(&path).map_err(|e| ApiErr::not_found(e.to_string()))?;
    if !meta.is_file() || meta.len() > ARTIFACT_LIMIT {
        return Err(ApiErr::bad("That path is not a readable file of at most 4 MB."));
    }
    let content = std::fs::read_to_string(&path).map_err(|_| ApiErr::bad("The file is not text."))?;
    let headings = ostra_core::outline::headings(&content);
    Ok(Json(Artifact { path, content, headings }))
}

#[derive(Deserialize)]
struct DiffQuery {
    project: String,
    phase: Option<u32>,
}

async fn git_show(root: &std::path::Path, rev_path: &str) -> String {
    let out = tokio::process::Command::new("git").arg("-C").arg(root).arg("show").arg(rev_path).output().await;
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => String::new(),
    }
}

/// The review loop's changed files against HEAD, for the ledger's diff view.
async fn diff(State(app): AppState, Path(id): Path<String>, Query(q): Query<DiffQuery>) -> Res<Vec<DiffFile>> {
    let sid = SessionId::from(id);
    let w = ws_of_session(&app, &sid)?;
    let st = w.engine.state(&sid)?;
    let root = st.project_path(&q.project).ok_or_else(|| ApiErr::not_found("No such project in this session."))?;
    let mut files: std::collections::BTreeSet<String> = Default::default();
    for p in st.phases.values().filter(|p| p.info.project == q.project && q.phase.is_none_or(|n| n == p.info.id)) {
        files.extend(p.impl_loop.changed.iter().cloned());
        files.extend(p.test_loop.changed.iter().cloned());
    }
    let mut out = vec![];
    for f in files.into_iter().take(200) {
        let full = paths::normalize(&root.join(&f));
        if !paths::is_inside(&root, &full) {
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
    let path = w.project_path(key).ok_or_else(|| ApiErr::not_found(format!("No project {key}.")))?;
    Ok(MemoryStore::open(&paths::project_memory_db(&path))?)
}

#[derive(Deserialize)]
struct MemQuery {
    q: Option<String>,
}

async fn memory_list(State(app): AppState, Path((id, key)): Path<(String, String)>, Query(q): Query<MemQuery>) -> Res<Vec<Lesson>> {
    let w = ws(&app, &id)?;
    Ok(Json(memory_store(&w, &key)?.list(q.q.as_deref(), 500, 0)?))
}

async fn memory_edit(State(app): AppState, Path((id, key)): Path<(String, String)>, Json(body): Json<LessonEdit>) -> Res<Lesson> {
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

async fn memory_delete(State(app): AppState, Path((id, key)): Path<(String, String)>, Query(q): Query<IdQuery>) -> Result<StatusCode, ApiErr> {
    let w = ws(&app, &id)?;
    memory_store(&w, &key)?.delete(q.id)?;
    Ok(StatusCode::NO_CONTENT)
}

// Skills. The logic lives in `crate::skills`.

async fn skills_list(State(app): AppState, Path(id): Path<String>) -> Res<Vec<ProjectSkills>> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::skills::list(&w)))
}

async fn skill_get(State(app): AppState, Path((id, key, name)): Path<(String, String, String)>) -> Res<SkillDoc> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::skills::get(&w, &key, &name)?))
}

async fn skill_save(State(app): AppState, Path((id, key, name)): Path<(String, String, String)>, Json(body): Json<SkillSave>) -> Res<SkillDoc> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::skills::save(&w, &key, &name, &body)?))
}

async fn skill_delete(State(app): AppState, Path((id, key, name)): Path<(String, String, String)>) -> Result<StatusCode, ApiErr> {
    let w = ws(&app, &id)?;
    crate::skills::delete(&w, &key, &name)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn skill_adopt(State(app): AppState, Path((id, key, name)): Path<(String, String, String)>, Json(body): Json<SkillAdopt>) -> Res<SkillDoc> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::skills::adopt(&w, &key, &name, &body)?))
}

async fn harness_skill_get(State(app): AppState, Path((id, key)): Path<(String, String)>, Query(q): Query<PathQuery>) -> Res<SkillDoc> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::skills::get_harness(&w, &key, &q.path)?))
}

#[derive(Deserialize)]
struct CostQuery {
    since: Option<String>,
}

async fn cost(State(app): AppState, Path(id): Path<String>, Query(q): Query<CostQuery>) -> Res<CostReport> {
    let since = match q.since.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|_| ApiErr::bad(format!("`since` must be an RFC 3339 time such as 2026-09-24T00:00:00Z, not `{s}`.")))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };
    Ok(Json(ws(&app, &id)?.db.cost_report(since)?))
}

async fn ask(State(app): AppState, Path(id): Path<String>, Json(body): Json<AskQuestion>) -> Res<AskStarted> {
    let w = ws(&app, &id)?;
    if body.question.trim().is_empty() {
        return Err(ApiErr::bad("Ask a question first."));
    }
    Ok(Json(AskStarted { execution: w.engine.ask(body.question, body.session).await? }))
}

async fn push_subscribe(State(app): AppState, Json(body): Json<PushSubscription>) -> Result<StatusCode, ApiErr> {
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
    let start = q.path.filter(|p| !p.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
    let path = std::fs::canonicalize(&start).map_err(|_| ApiErr::not_found(format!("{} does not exist.", start.display())))?;
    let mut entries = vec![];
    let read = std::fs::read_dir(&path).map_err(|e| ApiErr::bad(format!("Cannot read {}: {e}", path.display())))?;
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        entries.push(FsEntry { is_git: p.join(".git").exists(), is_ostra_project: paths::project_inventory(&p).exists(), name, is_dir: true });
    }
    entries.sort_by_key(|e| e.name.to_lowercase());
    Ok(Json(FsListing { parent: path.parent().map(|p| p.to_path_buf()), path, entries }))
}

// Project files (read-only). The logic lives in `crate::files`.

async fn project_tree(State(app): AppState, Path((id, key)): Path<(String, String)>, Query(q): Query<files::TreeQuery>) -> Res<ProjectTree> {
    let w = ws(&app, &id)?;
    Ok(Json(app.files.tree(&w, &key, q).await?))
}

async fn project_file(State(app): AppState, Path((id, key)): Path<(String, String)>, Query(q): Query<files::FileQuery>) -> Res<ProjectFile> {
    let w = ws(&app, &id)?;
    Ok(Json(app.files.file(&w, &key, &q.path).await?))
}

/// Responds with a [`FileIndex`].
async fn project_files(State(app): AppState, Path((id, key)): Path<(String, String)>) -> Result<Response, ApiErr> {
    let w = ws(&app, &id)?;
    let index = app.files.index(&w, &key).await?;
    Ok(Json(index.as_ref()).into_response())
}

async fn project_diff(State(app): AppState, Path((id, key)): Path<(String, String)>, Query(q): Query<files::DiffQuery>) -> Res<FileDiff> {
    let w = ws(&app, &id)?;
    Ok(Json(app.files.diff(&w, &key, q).await?))
}

async fn project_changes(State(app): AppState, Path((id, key)): Path<(String, String)>) -> Res<Vec<ProjectChange>> {
    let w = ws(&app, &id)?;
    Ok(Json(app.files.changes(&w, &key).await?))
}

// Navigation. The logic lives in `crate::nav`.

fn blocking_failed(e: tokio::task::JoinError) -> ApiErr {
    ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn workspace_tree(State(app): AppState, Path(id): Path<String>) -> Res<WorkspaceTree> {
    let w = ws(&app, &id)?;
    let tree = tokio::task::spawn_blocking(move || app.nav.tree(&w)).await.map_err(blocking_failed)??;
    Ok(Json(tree))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<usize>,
}

async fn search(State(app): AppState, Path(id): Path<String>, Query(q): Query<SearchQuery>) -> Res<SearchResults> {
    let w = ws(&app, &id)?;
    Ok(Json(crate::nav::search::search(&app, &w, q.q.as_deref().unwrap_or(""), q.limit).await?))
}

async fn workspace_activity(State(app): AppState, Path(id): Path<String>) -> Res<WorkspaceActivity> {
    let w = ws(&app, &id)?;
    let activity = tokio::task::spawn_blocking(move || crate::nav::activity::build(&w, chrono::Local::now())).await.map_err(blocking_failed)??;
    Ok(Json(activity))
}

async fn fs_browse(State(app): AppState, Query(q): Query<files::BrowseQuery>) -> Json<FsBrowse> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    Json(app.files.browse.browse(q.path.as_deref(), q.prefix.as_deref(), q.limit, &home))
}
