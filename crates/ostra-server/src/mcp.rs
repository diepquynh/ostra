//! The gateway to each workspace's external MCP servers (HANDOVER 10.6). Ostra is the only MCP
//! client: it keeps one connection per server, signs in, discovers tools, and serves them to
//! native runs directly and to harnesses through its own MCP server, so one policy and one
//! allowlist cover every executor.

use ostra_core::api::{McpConnState, McpServerStatus, McpToolInfo};
use ostra_core::config::{McpServerConfig, WorkspaceSettings, load_toml_required};
use ostra_core::exec::ExecutionSpec;
use ostra_core::mcp;
use ostra_core::paths;
use ostra_mcp::oauth::{self, ClientCredentials, Discovery, Tokens};
use ostra_mcp::{Client, Endpoint, McpError, TokenSource, ToolInfo};
use ostra_store::RegistryDb;
use ostra_tools::{McpConnector, McpOpened, McpTools, ToolDefinition};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// How long starting a server and its handshake may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// A tool list older than this is fetched again when an execution opens.
const RELIST_AFTER: Duration = Duration::from_secs(30);
/// A failed server is not retried sooner than this, because parallel executions would each
/// start it again.
const RETRY_AFTER: Duration = Duration::from_secs(20);
/// A sign-in link stays valid this long.
const LOGIN_TTL: Duration = Duration::from_secs(600);
pub const CALLBACK_PATH: &str = "/mcp/oauth/callback";

#[derive(Debug, Clone)]
enum ConnError {
    NeedsAuth(Option<String>),
    Other(String),
}

impl ConnError {
    fn text(&self, server: &str) -> String {
        match self {
            ConnError::NeedsAuth(_) => format!(
                "Sign in to MCP server `{server}` in Settings, MCP servers, or give it a token in a header, because it refuses requests without one."
            ),
            ConnError::Other(e) => format!("MCP server `{server}` is not available: {e}"),
        }
    }
}

#[derive(Default)]
struct Inner {
    client: Option<Arc<Client>>,
    tools: Vec<ToolInfo>,
    listed: Option<Instant>,
    error: Option<(ConnError, Instant)>,
}

struct Conn {
    fingerprint: String,
    inner: tokio::sync::Mutex<Inner>,
}

type Key = (PathBuf, String);

struct PendingLogin {
    key: Key,
    url: String,
    discovery: Discovery,
    client: ClientCredentials,
    verifier: String,
    redirect_uri: String,
    created: Instant,
}

/// OAuth state kept in the registry per workspace server.
#[derive(Serialize, Deserialize)]
struct OAuthRecord {
    url: String,
    discovery: Discovery,
    client: ClientCredentials,
    redirect_uri: String,
    tokens: Option<Tokens>,
}

pub struct McpGateway {
    registry: RegistryDb,
    http: reqwest::Client,
    conns: Mutex<HashMap<Key, Arc<Conn>>>,
    logins: Mutex<HashMap<String, PendingLogin>>,
}

fn settings(root: &Path) -> WorkspaceSettings {
    load_toml_required(&paths::workspace_toml(root)).unwrap_or_default()
}

fn oauth_key(key: &Key) -> String {
    format!("mcp_oauth:{}:{}", key.0.display(), key.1)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The parts of a server's settings that need a new connection when they change.
fn fingerprint(s: &McpServerConfig) -> String {
    serde_json::json!([s.command, s.env, s.url, s.headers, s.oauth]).to_string()
}

fn expand(field: &str, value: &str) -> Result<String, ConnError> {
    mcp::expand_env(value).map_err(|var| {
        ConnError::Other(format!(
            "set `{var}` in the Ostra server's environment, because `{field}` names it, then restart the server"
        ))
    })
}

impl McpGateway {
    pub fn new(registry: RegistryDb) -> Arc<Self> {
        Arc::new(McpGateway {
            registry,
            http: reqwest::Client::builder()
                .user_agent(concat!("ostra/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            conns: Mutex::default(),
            logins: Mutex::default(),
        })
    }

    fn conn(&self, key: &Key, cfg: &McpServerConfig) -> Arc<Conn> {
        let fp = fingerprint(cfg);
        let mut conns = self.conns.lock();
        match conns.get(key) {
            Some(c) if c.fingerprint == fp => c.clone(),
            _ => {
                let c = Arc::new(Conn {
                    fingerprint: fp,
                    inner: tokio::sync::Mutex::default(),
                });
                conns.insert(key.clone(), c.clone());
                c
            }
        }
    }

    /// Drop every connection of a workspace, for example when it is deleted.
    pub fn forget(&self, root: &Path) {
        self.conns.lock().retain(|(r, _), _| r != root);
    }

    fn load_record(&self, key: &Key) -> Option<OAuthRecord> {
        let bytes = self.registry.kv_get(&oauth_key(key)).ok()??;
        serde_json::from_slice(&bytes).ok()
    }

    fn save_record(&self, key: &Key, record: &OAuthRecord) {
        if let Ok(bytes) = serde_json::to_vec(record)
            && let Err(e) = self.registry.kv_set(&oauth_key(key), &bytes)
        {
            tracing::warn!("could not save MCP sign-in for {}: {e}", key.1);
        }
    }

    fn endpoint(self: &Arc<Self>, key: &Key, cfg: &McpServerConfig) -> Result<Endpoint, ConnError> {
        if let Some(url) = &cfg.url {
            let mut headers = vec![];
            for (k, v) in &cfg.headers {
                headers.push((k.clone(), expand(&format!("headers.{k}"), v)?));
            }
            let auth: Arc<dyn TokenSource> = Arc::new(Stored {
                gateway: self.clone(),
                key: key.clone(),
                url: url.clone(),
                lock: tokio::sync::Mutex::new(()),
            });
            return Ok(Endpoint::Http {
                url: url.clone(),
                headers,
                auth: Some(auth),
            });
        }
        let mut env = vec![];
        for (k, v) in &cfg.env {
            env.push((k.clone(), expand(&format!("env.{k}"), v)?));
        }
        Ok(Endpoint::Stdio {
            program: cfg.command[0].clone(),
            args: cfg.command[1..].to_vec(),
            env,
            cwd: key.0.clone(),
        })
    }

    /// A live client and its tool list, connecting or listing again when needed.
    async fn ready(
        self: &Arc<Self>,
        root: &Path,
        cfg: &McpServerConfig,
        force: bool,
    ) -> Result<(Arc<Client>, Vec<ToolInfo>), ConnError> {
        let key: Key = (root.to_path_buf(), cfg.name.clone());
        let conn = self.conn(&key, cfg);
        let mut inner = conn.inner.lock().await;
        if force {
            *inner = Inner::default();
        }
        if let Some(client) = inner.client.clone().filter(|c| c.is_alive()) {
            let stale = inner.listed.is_none_or(|t| t.elapsed() > RELIST_AFTER);
            if !(stale || client.take_tools_changed()) {
                return Ok((client, inner.tools.clone()));
            }
            match client.list_tools(CONNECT_TIMEOUT).await {
                Ok(tools) => {
                    inner.tools = tools.clone();
                    inner.listed = Some(Instant::now());
                    return Ok((client, tools));
                }
                Err(e) => tracing::info!(
                    "MCP server {} failed to list tools, reconnecting: {e}",
                    cfg.name
                ),
            }
        }
        if let Some((e, at)) = &inner.error
            && at.elapsed() < RETRY_AFTER
        {
            return Err(e.clone());
        }
        let result: Result<(Arc<Client>, Vec<ToolInfo>), ConnError> = async {
            let endpoint = self.endpoint(&key, cfg)?;
            let client = Client::connect(endpoint, CONNECT_TIMEOUT)
                .await
                .map_err(conn_error)?;
            let tools = client
                .list_tools(CONNECT_TIMEOUT)
                .await
                .map_err(conn_error)?;
            Ok((Arc::new(client), tools))
        }
        .await;
        match result {
            Ok((client, tools)) => {
                *inner = Inner {
                    client: Some(client.clone()),
                    tools: tools.clone(),
                    listed: Some(Instant::now()),
                    error: None,
                };
                Ok((client, tools))
            }
            Err(e) => {
                *inner = Inner {
                    error: Some((e.clone(), Instant::now())),
                    ..Inner::default()
                };
                Err(e)
            }
        }
    }

    /// Connect to every enabled server of the workspace and report each.
    pub async fn status(
        self: &Arc<Self>,
        root: &Path,
        force: Option<&str>,
    ) -> Vec<McpServerStatus> {
        let servers = settings(root).mcp_servers;
        let futs = servers.iter().map(|cfg| async move {
            let force = force == Some(cfg.name.as_str());
            self.server_status(root, cfg, force).await
        });
        futures::future::join_all(futs).await
    }

    /// One server's status. `force` reconnects and lists its tools again.
    pub async fn one(
        self: &Arc<Self>,
        root: &Path,
        name: &str,
        force: bool,
    ) -> Option<McpServerStatus> {
        let cfg = settings(root)
            .mcp_servers
            .into_iter()
            .find(|c| c.name == name)?;
        Some(self.server_status(root, &cfg, force).await)
    }

    async fn server_status(
        self: &Arc<Self>,
        root: &Path,
        cfg: &McpServerConfig,
        force: bool,
    ) -> McpServerStatus {
        let key: Key = (root.to_path_buf(), cfg.name.clone());
        let signed_in = cfg
            .url
            .as_ref()
            .and_then(|url| self.load_record(&key).filter(|r| &r.url == url))
            .map(|r| r.tokens.is_some());
        let mut status = McpServerStatus {
            name: cfg.name.clone(),
            transport: if cfg.url.is_some() { "http" } else { "stdio" }.into(),
            state: McpConnState::Disabled,
            message: None,
            server_info: None,
            signed_in,
            tools: vec![],
        };
        if !cfg.enabled {
            return status;
        }
        match self.ready(root, cfg, force).await {
            Ok((client, tools)) => {
                status.state = McpConnState::Connected;
                status.server_info = client.server_info.clone();
                status.tools = tools
                    .iter()
                    .map(|t| McpToolInfo {
                        name: t.name.clone(),
                        canonical: mcp::canonical(&mcp::bare_name(&cfg.name, &t.name)),
                        description: t.description.clone(),
                        enabled: !cfg.disabled_tools.contains(&t.name),
                        read_only: t.read_only,
                    })
                    .collect();
            }
            Err(e) => {
                status.state = match e {
                    ConnError::NeedsAuth(_) => {
                        status.signed_in = Some(false);
                        McpConnState::NeedsAuth
                    }
                    ConnError::Other(_) => McpConnState::Error,
                };
                status.message = Some(e.text(&cfg.name));
            }
        }
        status
    }

    /// Start an OAuth sign-in. `origin` is where the browser reaches this server.
    pub async fn login(
        self: &Arc<Self>,
        root: &Path,
        name: &str,
        origin: &str,
    ) -> Result<String, String> {
        let s = settings(root);
        let cfg = s
            .mcp_servers
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| format!("This workspace has no MCP server `{name}`."))?;
        let url = cfg
            .url
            .clone()
            .ok_or("Sign-in applies to remote servers only: this one runs as a local command.")?;
        let key: Key = (root.to_path_buf(), name.to_string());
        let challenge = {
            let conn = self.conn(&key, cfg);
            let inner = conn.inner.lock().await;
            match &inner.error {
                Some((ConnError::NeedsAuth(c), _)) => c.clone(),
                _ => None,
            }
        };
        let discovery = oauth::discover(&self.http, &url, challenge.as_deref()).await?;
        let redirect_uri = format!("{}{CALLBACK_PATH}", origin.trim_end_matches('/'));
        let configured = cfg.oauth.clone().unwrap_or_default();
        let client = match &configured.client_id {
            Some(id) => ClientCredentials {
                client_id: id.clone(),
                client_secret: configured
                    .client_secret_env
                    .as_deref()
                    .map(|v| {
                        std::env::var(v).map_err(|_| {
                            format!("Set `{v}` in the Ostra server's environment, because `oauth.client_secret_env` names it.")
                        })
                    })
                    .transpose()?,
            },
            None => match self.load_record(&key).filter(|r| {
                r.url == url && r.redirect_uri == redirect_uri && r.discovery.server.issuer == discovery.server.issuer
            }) {
                Some(r) => r.client,
                None => oauth::register(&self.http, &discovery.server, &redirect_uri)
                    .await
                    .map_err(|e| {
                        if discovery.guessed {
                            format!(
                                "MCP server `{name}` asks for a sign-in but publishes no OAuth metadata, so Ostra cannot sign in to it. Give it a token in a header instead, such as `Authorization: Bearer ${{TOKEN_VAR}}`."
                            )
                        } else {
                            e
                        }
                    })?,
            },
        };
        let scopes = if configured.scopes.is_empty() {
            discovery.scopes.clone()
        } else {
            configured.scopes.clone()
        };
        let pkce = oauth::pkce();
        let state = oauth::random_token();
        let auth_url = oauth::authorization_url(&oauth::AuthRequest {
            discovery: &discovery,
            client: &client,
            redirect_uri: &redirect_uri,
            scopes: &scopes,
            state: &state,
            challenge: &pkce.challenge,
        })?;
        let mut logins = self.logins.lock();
        logins.retain(|_, p| p.created.elapsed() < LOGIN_TTL);
        logins.insert(
            state,
            PendingLogin {
                key,
                url,
                discovery,
                client,
                verifier: pkce.verifier,
                redirect_uri,
                created: Instant::now(),
            },
        );
        Ok(auth_url)
    }

    /// Finish a sign-in from the authorization server's redirect. Returns the server name.
    pub async fn callback(self: &Arc<Self>, state: &str, code: &str) -> Result<String, String> {
        let pending = self
            .logins
            .lock()
            .remove(state)
            .filter(|p| p.created.elapsed() < LOGIN_TTL)
            .ok_or("This sign-in link has expired or was already used. Start the sign-in again from Ostra.")?;
        let tokens = oauth::exchange_code(
            &self.http,
            &pending.discovery,
            &pending.client,
            code,
            &pending.verifier,
            &pending.redirect_uri,
            now(),
        )
        .await?;
        self.save_record(
            &pending.key,
            &OAuthRecord {
                url: pending.url,
                discovery: pending.discovery,
                client: pending.client,
                redirect_uri: pending.redirect_uri,
                tokens: Some(tokens),
            },
        );
        self.reset(&pending.key).await;
        Ok(pending.key.1)
    }

    pub async fn logout(&self, root: &Path, name: &str) {
        let key: Key = (root.to_path_buf(), name.to_string());
        let _ = self.registry.kv_delete(&oauth_key(&key));
        self.reset(&key).await;
    }

    async fn reset(&self, key: &Key) {
        let conn = self.conns.lock().get(key).cloned();
        if let Some(c) = conn {
            *c.inner.lock().await = Inner::default();
        }
    }
}

fn conn_error(e: McpError) -> ConnError {
    match e {
        McpError::NeedsAuth { www_authenticate } => ConnError::NeedsAuth(www_authenticate),
        other => ConnError::Other(other.to_string()),
    }
}

/// Tokens from the registry, refreshed when they expire or the server refuses them.
struct Stored {
    gateway: Arc<McpGateway>,
    key: Key,
    url: String,
    /// One refresh at a time, because a refresh token may be single-use.
    lock: tokio::sync::Mutex<()>,
}

impl Stored {
    fn record(&self) -> Option<OAuthRecord> {
        self.gateway
            .load_record(&self.key)
            .filter(|r| r.url == self.url)
    }

    async fn refresh(&self, mut record: OAuthRecord) -> Option<String> {
        let refresh = record.tokens.as_ref()?.refresh_token.clone()?;
        match oauth::refresh(
            &self.gateway.http,
            &record.discovery,
            &record.client,
            &refresh,
            now(),
        )
        .await
        {
            Ok(t) => {
                let access = t.access_token.clone();
                record.tokens = Some(t);
                self.gateway.save_record(&self.key, &record);
                Some(access)
            }
            Err(e) => {
                tracing::info!("MCP server {}: token refresh failed: {e}", self.key.1);
                record.tokens = None;
                self.gateway.save_record(&self.key, &record);
                None
            }
        }
    }
}

#[async_trait::async_trait]
impl TokenSource for Stored {
    async fn token(&self) -> Option<String> {
        let _g = self.lock.lock().await;
        let record = self.record()?;
        let tokens = record.tokens.clone()?;
        if !tokens.expired(now()) {
            return Some(tokens.access_token);
        }
        self.refresh(record).await
    }

    async fn refused(&self, token: &str) -> Option<String> {
        let _g = self.lock.lock().await;
        let record = self.record()?;
        let current = record.tokens.as_ref()?.access_token.clone();
        // Another call refreshed while this one waited.
        if current != token {
            return Some(current);
        }
        self.refresh(record).await
    }
}

// ---------------------------------------------------------------------------------------------
// Executions
// ---------------------------------------------------------------------------------------------

struct ExecTool {
    def: ToolDefinition,
    read_only: bool,
    server: McpServerConfig,
    tool: String,
    client: Arc<Client>,
}

/// One execution's MCP tools.
struct ExecMcp {
    gateway: Arc<McpGateway>,
    root: PathBuf,
    tools: Vec<ExecTool>,
}

#[async_trait::async_trait]
impl McpTools for ExecMcp {
    fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.def.clone()).collect()
    }

    fn read_only(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter(|t| t.read_only)
            .map(|t| t.def.name.clone())
            .collect()
    }

    async fn call(
        &self,
        tool: &str,
        input: &Value,
        cancel: CancellationToken,
    ) -> Result<String, String> {
        let t = self
            .tools
            .iter()
            .find(|t| t.def.name == tool)
            .ok_or_else(|| format!("Unknown tool `{tool}`."))?;
        let timeout = Duration::from_secs(u64::from(t.server.timeout_secs));
        let mut client = t.client.clone();
        // A server that exited since the execution opened is started once more.
        if !client.is_alive() {
            client = self
                .gateway
                .ready(&self.root, &t.server, false)
                .await
                .map_err(|e| e.text(&t.server.name))?
                .0;
        }
        match client.call_tool(&t.tool, input, timeout, cancel).await {
            Ok(r) => r,
            Err(McpError::NeedsAuth { .. }) => Err(ConnError::NeedsAuth(None).text(&t.server.name)),
            Err(e) => Err(format!("MCP server `{}` failed: {e}", t.server.name)),
        }
    }
}

/// The gateway as the executors see it.
pub struct Connector(pub Arc<McpGateway>);

#[async_trait::async_trait]
impl McpConnector for Connector {
    async fn open(&self, spec: &ExecutionSpec) -> McpOpened {
        let gateway = &self.0;
        let none = McpOpened {
            tools: None,
            notes: vec![],
        };
        if spec.resume.as_ref().is_some_and(|r| r.inspect) {
            return none;
        }
        let root = spec.ctx.workspace_root.clone();
        let servers: Vec<McpServerConfig> = settings(&root)
            .mcp_servers
            .into_iter()
            .filter(|s| mcp::serves(s, spec.agent))
            .collect();
        if servers.is_empty() {
            return none;
        }
        let results =
            futures::future::join_all(servers.iter().map(|s| gateway.ready(&root, s, false))).await;
        let mut tools = vec![];
        let mut notes = vec![];
        let mut seen = std::collections::HashSet::new();
        for (server, r) in servers.into_iter().zip(results) {
            let (client, listed) = match r {
                Ok(v) => v,
                Err(e) => {
                    notes.push(e.text(&server.name));
                    continue;
                }
            };
            for t in listed {
                if server.disabled_tools.contains(&t.name) {
                    continue;
                }
                let name = mcp::canonical(&mcp::bare_name(&server.name, &t.name));
                if !seen.insert(name.clone()) {
                    continue;
                }
                let description = if t.description.trim().is_empty() {
                    format!("The `{}` tool of MCP server `{}`.", t.name, server.name)
                } else {
                    t.description.clone()
                };
                tools.push(ExecTool {
                    def: ToolDefinition {
                        name,
                        description,
                        input_schema: t.input_schema.clone(),
                    },
                    read_only: t.read_only,
                    server: server.clone(),
                    tool: t.name,
                    client: client.clone(),
                });
            }
        }
        McpOpened {
            tools: (!tools.is_empty()).then(|| {
                Arc::new(ExecMcp {
                    gateway: gateway.clone(),
                    root,
                    tools,
                }) as Arc<dyn McpTools>
            }),
            notes,
        }
    }
}
