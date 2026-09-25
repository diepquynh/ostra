//! The streamable HTTP transport: each message is a POST, answered with JSON or an SSE stream.

use crate::{McpError, Shared, TokenSource, Transport, is_response, on_server_message};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use parking_lot::Mutex;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const SESSION_HEADER: &str = "mcp-session-id";
const VERSION_HEADER: &str = "mcp-protocol-version";

pub(crate) struct Http {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    auth: Option<Arc<dyn TokenSource>>,
    session: Mutex<Option<String>>,
    version: Mutex<Option<String>>,
    shared: Arc<Shared>,
}

impl Http {
    pub fn new(
        url: String,
        headers: Vec<(String, String)>,
        auth: Option<Arc<dyn TokenSource>>,
        shared: Arc<Shared>,
    ) -> Result<Http, McpError> {
        let mut map = HeaderMap::new();
        for (k, v) in headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| McpError::Transport(format!("`{k}` is not a header name")))?;
            let value = HeaderValue::from_str(&v).map_err(|_| {
                McpError::Transport(format!("the value of header `{k}` is not valid"))
            })?;
            map.insert(name, value);
        }
        let client = reqwest::Client::builder()
            .user_agent(concat!("ostra/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| McpError::Transport(e.to_string()))?;
        Ok(Http {
            client,
            url,
            headers: map,
            auth,
            session: Mutex::new(None),
            version: Mutex::new(None),
            shared,
        })
    }

    async fn post(
        &self,
        message: &Value,
        token: Option<&str>,
    ) -> Result<reqwest::Response, McpError> {
        let mut req = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(message.to_string());
        if let Some(t) = token {
            req = req.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        if let Some(s) = self.session.lock().clone() {
            req = req.header(SESSION_HEADER, s);
        }
        if let Some(v) = self.version.lock().clone() {
            req = req.header(VERSION_HEADER, v);
        }
        req.send()
            .await
            .map_err(|e| McpError::Transport(network(&e)))
    }

    /// POST with the current token, refreshing it once if the server refuses it.
    async fn send(&self, message: &Value) -> Result<reqwest::Response, McpError> {
        let mut token = match &self.auth {
            Some(a) => a.token().await,
            None => None,
        };
        let mut retried = false;
        loop {
            let resp = self.post(message, token.as_deref()).await?;
            let status = resp.status().as_u16();
            if status == 401 || status == 403 {
                let www = resp
                    .headers()
                    .get(reqwest::header::WWW_AUTHENTICATE)
                    .and_then(|h| h.to_str().ok())
                    .map(String::from);
                // A 403 without a Bearer challenge is a plain refusal, not a sign-in request.
                if status == 403
                    && !www
                        .as_deref()
                        .is_some_and(|w| w.contains("insufficient_scope"))
                {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(McpError::Transport(format!(
                        "the server refused the request (403): {}",
                        snippet(&body)
                    )));
                }
                if !retried
                    && let (Some(auth), Some(t)) = (&self.auth, token.as_deref())
                    && let Some(fresh) = auth.refused(t).await
                {
                    token = Some(fresh);
                    retried = true;
                    continue;
                }
                return Err(McpError::NeedsAuth {
                    www_authenticate: www,
                });
            }
            if status == 404 && self.session.lock().is_some() {
                self.shared.alive.store(false, Ordering::SeqCst);
                return Err(McpError::Closed("the server ended the session".into()));
            }
            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(McpError::Transport(format!(
                    "the server answered {status}: {}",
                    snippet(&body)
                )));
            }
            if let Some(s) = resp
                .headers()
                .get(SESSION_HEADER)
                .and_then(|h| h.to_str().ok())
            {
                *self.session.lock() = Some(s.to_string());
            }
            return Ok(resp);
        }
    }

    /// Handle a message from the server that is not the awaited response.
    async fn side_message(&self, m: &Value) {
        if let Some(reply) = on_server_message(&self.shared, m) {
            let _ = self.send(&reply).await;
        }
    }
}

#[async_trait::async_trait]
impl Transport for Http {
    async fn request(&self, id: i64, message: Value) -> Result<Value, McpError> {
        let resp = self.send(&message).await?;
        let sse = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|c| c.starts_with("text/event-stream"));
        let matches = |m: &Value| is_response(m) && m.get("id").and_then(Value::as_i64) == Some(id);
        if !sse {
            let body: Value = resp.json().await.map_err(|e| {
                McpError::Transport(format!("the server sent a body that is not JSON: {e}"))
            })?;
            let items = match body {
                Value::Array(items) => items,
                one => vec![one],
            };
            let mut found = None;
            for m in items {
                if found.is_none() && matches(&m) {
                    found = Some(m);
                } else {
                    self.side_message(&m).await;
                }
            }
            return found.ok_or_else(|| {
                McpError::Transport("the server's answer carried no response".into())
            });
        }
        let mut events = resp.bytes_stream().eventsource();
        while let Some(ev) = events.next().await {
            let ev = ev.map_err(|e| McpError::Transport(format!("the event stream broke: {e}")))?;
            if ev.data.trim().is_empty() {
                continue;
            }
            let Ok(m) = serde_json::from_str::<Value>(&ev.data) else {
                continue;
            };
            if matches(&m) {
                return Ok(m);
            }
            self.side_message(&m).await;
        }
        Err(McpError::Closed(
            "the event stream ended before the response".into(),
        ))
    }

    async fn notify(&self, message: Value) -> Result<(), McpError> {
        self.send(&message).await.map(|_| ())
    }

    fn initialized(&self, version: &str) {
        *self.version.lock() = Some(version.to_string());
    }
}

fn network(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(inner) = src {
        s.push_str(&format!(": {inner}"));
        src = inner.source();
    }
    s
}

fn snippet(body: &str) -> String {
    let t: String = body.trim().chars().take(300).collect();
    if t.is_empty() { "no body".into() } else { t }
}
