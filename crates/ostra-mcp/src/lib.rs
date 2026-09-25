//! A Model Context Protocol client for the workspace's external MCP servers: stdio and
//! streamable HTTP transports, tool discovery and calls, and OAuth sign-in for remote servers.

pub mod content;
mod http;
pub mod oauth;
mod stdio;

use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const PROTOCOL_VERSION: &str = "2025-11-25";
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Where a server is and how to reach it.
#[derive(Clone)]
pub enum Endpoint {
    Stdio {
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
        cwd: PathBuf,
    },
    Http {
        url: String,
        headers: Vec<(String, String)>,
        auth: Option<Arc<dyn TokenSource>>,
    },
}

/// Bearer tokens for a remote server.
#[async_trait::async_trait]
pub trait TokenSource: Send + Sync {
    /// The access token to send, refreshed first when it has expired. `None` when none is held.
    async fn token(&self) -> Option<String>;
    /// The server refused `token`. Returns a refreshed token, or `None` when sign-in is needed.
    async fn refused(&self, token: &str) -> Option<String>;
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum McpError {
    /// The server answered 401 or 403. Carries its `WWW-Authenticate` header.
    #[error("the server needs a sign-in")]
    NeedsAuth { www_authenticate: Option<String> },
    #[error("{0}")]
    Transport(String),
    #[error("{message} (code {code})")]
    Rpc { code: i64, message: String },
    #[error("no answer after {0} s")]
    Timeout(u64),
    #[error("cancelled")]
    Cancelled,
    #[error("the connection closed: {0}")]
    Closed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
}

/// State both transports share with the client.
#[derive(Default)]
pub(crate) struct Shared {
    pub alive: AtomicBool,
    pub tools_changed: AtomicBool,
}

#[async_trait::async_trait]
pub(crate) trait Transport: Send + Sync {
    /// Send a request and wait for the response message with its id.
    async fn request(&self, id: i64, message: Value) -> Result<Value, McpError>;
    async fn notify(&self, message: Value) -> Result<(), McpError>;
    /// Forget a request that will not be awaited any longer.
    fn abandon(&self, _id: i64) {}
    /// Called once `initialize` succeeded, with the negotiated protocol version.
    fn initialized(&self, _version: &str) {}
}

/// One live connection to one server.
pub struct Client {
    transport: Arc<dyn Transport>,
    shared: Arc<Shared>,
    next_id: AtomicI64,
    pub server_info: Option<String>,
    pub protocol_version: String,
}

impl Client {
    /// Start or reach the server and run the `initialize` handshake.
    pub async fn connect(endpoint: Endpoint, timeout: Duration) -> Result<Client, McpError> {
        let shared = Arc::new(Shared::default());
        shared.alive.store(true, Ordering::SeqCst);
        let transport: Arc<dyn Transport> = match endpoint {
            Endpoint::Stdio {
                program,
                args,
                env,
                cwd,
            } => Arc::new(stdio::Stdio::spawn(
                &program,
                &args,
                &env,
                &cwd,
                shared.clone(),
            )?),
            Endpoint::Http { url, headers, auth } => {
                Arc::new(http::Http::new(url, headers, auth, shared.clone())?)
            }
        };
        let mut client = Client {
            transport,
            shared,
            next_id: AtomicI64::new(1),
            server_info: None,
            protocol_version: PROTOCOL_VERSION.into(),
        };
        let result = client
            .rpc(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "ostra", "version": env!("CARGO_PKG_VERSION")},
                }),
                timeout,
                None,
            )
            .await?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION);
        if !SUPPORTED_VERSIONS.contains(&version) {
            return Err(McpError::Transport(format!(
                "the server speaks MCP {version}, which Ostra does not support"
            )));
        }
        client.protocol_version = version.to_string();
        client.transport.initialized(version);
        client.server_info = result.get("serverInfo").map(|i| {
            let name = i.get("name").and_then(Value::as_str).unwrap_or("server");
            match i.get("version").and_then(Value::as_str) {
                Some(v) => format!("{name} {v}"),
                None => name.to_string(),
            }
        });
        client
            .transport
            .notify(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await?;
        Ok(client)
    }

    pub fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    /// Whether the server said its tool list changed since the last call.
    pub fn take_tools_changed(&self) -> bool {
        self.shared.tools_changed.swap(false, Ordering::SeqCst)
    }

    pub async fn list_tools(&self, timeout: Duration) -> Result<Vec<ToolInfo>, McpError> {
        let mut tools = vec![];
        let mut cursor: Option<String> = None;
        // A server that keeps returning cursors is capped, because each page is a round trip.
        for _ in 0..50 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let result = self.rpc("tools/list", params, timeout, None).await?;
            for t in result
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(name) = t.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let description = t
                    .get("description")
                    .and_then(Value::as_str)
                    .or_else(|| t.get("title").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_string();
                let input_schema = t
                    .get("inputSchema")
                    .cloned()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
                tools.push(ToolInfo {
                    name: name.to_string(),
                    description,
                    input_schema,
                    read_only: t
                        .pointer("/annotations/readOnlyHint")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(String::from);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// Call a tool and return its result as text. A result the server flags as an error is `Err`.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: &Value,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<Result<String, String>, McpError> {
        let arguments = if arguments.is_object() {
            arguments.clone()
        } else {
            json!({})
        };
        let result = self
            .rpc(
                "tools/call",
                json!({"name": name, "arguments": arguments}),
                timeout,
                Some(cancel),
            )
            .await?;
        Ok(content::result_text(&result))
    }

    async fn rpc(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<CancellationToken>,
    ) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let cancel = cancel.unwrap_or_default();
        let outcome = tokio::select! {
            r = tokio::time::timeout(timeout, self.transport.request(id, message)) => match r {
                Ok(r) => r,
                Err(_) => Err(McpError::Timeout(timeout.as_secs())),
            },
            _ = cancel.cancelled() => Err(McpError::Cancelled),
        };
        let response = match outcome {
            Ok(r) => r,
            Err(e @ (McpError::Timeout(_) | McpError::Cancelled)) => {
                self.transport.abandon(id);
                if method != "initialize" {
                    let _ = self
                        .transport
                        .notify(json!({
                            "jsonrpc": "2.0",
                            "method": "notifications/cancelled",
                            "params": {"requestId": id, "reason": e.to_string()},
                        }))
                        .await;
                }
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        if let Some(err) = response.get("error") {
            return Err(McpError::Rpc {
                code: err.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                message: err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("error")
                    .to_string(),
            });
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// The reply to a request the server sent us. Ostra declares no client capabilities, so it
/// answers only `ping`.
pub(crate) fn answer_server_request(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method")?.as_str()?;
    Some(if method == "ping" {
        json!({"jsonrpc": "2.0", "id": id, "result": {}})
    } else {
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("Ostra does not support {method}")}})
    })
}

/// Route one incoming message that is not a response: answer requests, note notifications.
/// Returns the reply to send, if any.
pub(crate) fn on_server_message(shared: &Shared, message: &Value) -> Option<Value> {
    if message.get("id").is_some() {
        return answer_server_request(message);
    }
    if message.get("method").and_then(Value::as_str) == Some("notifications/tools/list_changed") {
        shared.tools_changed.store(true, Ordering::SeqCst);
    }
    None
}

pub(crate) fn is_response(message: &Value) -> bool {
    message.get("method").is_none()
        && (message.get("result").is_some() || message.get("error").is_some())
}
