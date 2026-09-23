//! Model providers behind one neutral chat interface: Anthropic (Messages API) and OpenAI
//! (Responses API), both streaming, plus a scripted provider for tests.

pub mod anthropic;
pub mod mock;
pub mod openai;
pub mod pricing;
mod retry;
mod sse;

use ostra_core::api::ProviderStatus;
use ostra_core::config::{GlobalConfig, ProviderConfig};
use ostra_core::exec::Usage;
use ostra_core::Effort;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

pub use mock::ScriptedProvider;
pub use pricing::{Pricing, price};
pub use retry::RetryPolicy;
pub use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------------------------
// Neutral request and response types
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemBlock {
    pub text: String,
    /// Mark the end of a stable prefix for prompt caching.
    #[serde(default)]
    pub cache: bool,
}

impl SystemBlock {
    pub fn new(text: impl Into<String>) -> Self {
        SystemBlock { text: text.into(), cache: false }
    }

    pub fn cached(text: impl Into<String>) -> Self {
        SystemBlock { text: text.into(), cache: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(default)]
    pub cache: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ServerTools {
    pub web_search: bool,
    pub web_fetch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToolChoice {
    #[default]
    Auto,
    /// The model must call some tool. Downgraded to `Auto` on models that reject forced calls.
    Any,
    None,
    /// The model must call this tool. Downgraded to `Auto` on models that reject forced calls.
    Tool(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Model id without the provider prefix.
    pub model: String,
    pub system: Vec<SystemBlock>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub server_tools: ServerTools,
    pub max_tokens: u32,
    pub effort: Effort,
    pub tool_choice: ToolChoice,
    /// Ask the provider to clear old tool results before the context fills.
    pub clear_old_tool_results: bool,
    /// On models with server-side refusal fallbacks, let the provider re-run a declined request
    /// on its recommended fallback model.
    pub refusal_fallback: bool,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>) -> Self {
        ChatRequest {
            model: model.into(),
            system: vec![],
            messages: vec![],
            tools: vec![],
            server_tools: ServerTools::default(),
            max_tokens: 32_000,
            effort: Effort::High,
            tool_choice: ToolChoice::Auto,
            clear_old_tool_results: false,
            refusal_fallback: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Block>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Message { role: Role::User, content: vec![Block::Text { text: text.into() }] }
    }

    pub fn assistant(content: Vec<Block>) -> Self {
        Message { role: Role::Assistant, content }
    }

    pub fn tool_results(results: Vec<Block>) -> Self {
        Message { role: Role::User, content: results }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Provider-specific block (server tool calls and results, reasoning items) passed back to the
    /// same provider verbatim and dropped for any other provider.
    Opaque {
        provider: String,
        value: serde_json::Value,
    },
}

impl Block {
    pub fn text(text: impl Into<String>) -> Self {
        Block::Text { text: text.into() }
    }

    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        Block::ToolResult { tool_use_id: tool_use_id.into(), content: content.into(), is_error }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolUseStarted { id: String, name: String },
    ToolInputDelta { id: String, partial_json: String },
    /// A provider-side tool ran (web search, web fetch). `summary` is its query or URL.
    ServerToolUsed { name: String, summary: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    /// A server-side tool loop paused. Send the assistant turn back unchanged to resume.
    PauseTurn,
    Refusal,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: Vec<Block>,
    pub stop: StopReason,
    pub usage: Usage,
    /// The model that produced the response (differs from the request after a fallback).
    pub model: String,
    /// Refusal category, when `stop` is `Refusal` and the provider names one.
    pub refusal_category: Option<String>,
}

impl ChatResponse {
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_uses(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.content
            .iter()
            .filter_map(|b| match b {
                Block::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
                _ => None,
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, thiserror::Error)]
pub enum ProviderError {
    #[error("no API key for provider `{0}`")]
    NoKey(String),
    #[error("unknown provider `{0}`")]
    UnknownProvider(String),
    #[error("model `{0}` is not written as `provider:model`")]
    BadModel(String),
    #[error("authentication failed ({status}): {message}")]
    Auth { status: u16, message: String },
    #[error("request rejected ({status}): {message}")]
    InvalidRequest { status: u16, message: String },
    #[error("rate limited: {message}")]
    RateLimited { message: String, retry_after: Option<u64> },
    #[error("provider overloaded: {message}")]
    Overloaded { message: String, retry_after: Option<u64> },
    #[error("provider error ({status}): {message}")]
    Server { status: u16, message: String, retry_after: Option<u64> },
    #[error("network error: {0}")]
    Network(String),
    /// The stream failed after output started, so it was not retried.
    #[error("stream failed: {0}")]
    Stream(String),
    #[error("could not decode provider response: {0}")]
    Decode(String),
    #[error("cancelled")]
    Cancelled,
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ProviderError::RateLimited { .. }
                | ProviderError::Overloaded { .. }
                | ProviderError::Server { .. }
                | ProviderError::Network(_)
        )
    }

    pub fn retry_after(&self) -> Option<u64> {
        match self {
            ProviderError::RateLimited { retry_after, .. }
            | ProviderError::Overloaded { retry_after, .. }
            | ProviderError::Server { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The provider trait
// ---------------------------------------------------------------------------------------------

pub type EventSink<'a> = &'a (dyn Fn(StreamEvent) + Send + Sync);

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;

    /// Which server-side tools this provider offers for `model`. The agent loop supplies a local
    /// tool for any it lacks (OpenAI has no server-side fetch).
    fn server_tools(&self, _model: &str) -> ServerTools {
        ServerTools::default()
    }

    async fn chat(
        &self,
        req: ChatRequest,
        on_event: EventSink<'_>,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, ProviderError>;
}

/// An API key. Its `Debug` and `Display` never print the value.
#[derive(Clone)]
pub struct ApiKey(String, bool);

impl ApiKey {
    pub fn new(value: impl Into<String>) -> Self {
        ApiKey(value.into(), false)
    }

    /// A bearer token (`Authorization: Bearer`), as gateways and proxies take.
    pub fn bearer(value: impl Into<String>) -> Self {
        ApiKey(value.into(), true)
    }

    pub fn is_bearer(&self) -> bool {
        self.1
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

// ---------------------------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------------------------

/// A resolved key and where it came from (`env:NAME` or `keychain`).
pub struct KeyLookup {
    pub key: ApiKey,
    pub source: String,
}

/// The providers configured on this machine.
pub struct Providers {
    map: RwLock<BTreeMap<String, Arc<dyn Provider>>>,
    statuses: RwLock<Vec<ProviderStatus>>,
}

impl Default for Providers {
    fn default() -> Self {
        Providers::empty()
    }
}

impl fmt::Debug for Providers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Providers").field("names", &self.map.read().keys().collect::<Vec<_>>()).finish()
    }
}

/// Look a key up in the environment, then the OS keychain.
pub fn lookup_key(name: &str, cfg: &ProviderConfig) -> Option<KeyLookup> {
    if let Some(var) = cfg.api_key_env.as_deref()
        && let Ok(value) = std::env::var(var)
        && !value.trim().is_empty()
    {
        return Some(KeyLookup { key: ApiKey::new(value.trim()), source: format!("env:{var}") });
    }
    if let Some(var) = cfg.auth_token_env.as_deref()
        && let Ok(value) = std::env::var(var)
        && !value.trim().is_empty()
    {
        return Some(KeyLookup { key: ApiKey::bearer(value.trim()), source: format!("env:{var}") });
    }
    let service = cfg.keychain_service.as_deref()?;
    let entry = keyring::Entry::new(service, name).ok()?;
    let value = entry.get_password().ok()?;
    if value.trim().is_empty() {
        return None;
    }
    Some(KeyLookup { key: ApiKey::new(value.trim()), source: "keychain".into() })
}

impl Providers {
    pub fn empty() -> Self {
        Providers { map: RwLock::new(BTreeMap::new()), statuses: RwLock::new(vec![]) }
    }

    /// Build from the global config, reading keys from the environment or the OS keychain.
    pub fn from_config(cfg: &GlobalConfig) -> Self {
        Providers::from_config_with(cfg, lookup_key)
    }

    /// Build from the global config with a custom key lookup (tests).
    pub fn from_config_with(cfg: &GlobalConfig, lookup: impl Fn(&str, &ProviderConfig) -> Option<KeyLookup>) -> Self {
        let providers = Providers::empty();
        let mut statuses = vec![];
        for (name, pcfg) in &cfg.providers {
            let found = lookup(name, pcfg);
            statuses.push(ProviderStatus {
                name: name.clone(),
                has_key: found.is_some(),
                source: found.as_ref().map(|k| k.source.clone()).unwrap_or_else(|| "none".into()),
            });
            let Some(found) = found else { continue };
            let base_url = pcfg.base_url.clone().or_else(|| {
                pcfg.base_url_env.as_deref().and_then(|v| std::env::var(v).ok()).filter(|u| !u.trim().is_empty())
            });
            let provider: Arc<dyn Provider> = match name.as_str() {
                "anthropic" => Arc::new(anthropic::Anthropic::new(found.key, base_url)),
                "openai" => Arc::new(openai::OpenAi::new(found.key, base_url)),
                other => {
                    tracing::warn!(provider = other, "unknown provider in config; ignored");
                    continue;
                }
            };
            providers.map.write().insert(name.clone(), provider);
        }
        *providers.statuses.write() = statuses;
        providers
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.map.read().get(name).cloned()
    }

    pub fn register(&self, name: impl Into<String>, provider: Arc<dyn Provider>) {
        let name = name.into();
        let mut statuses = self.statuses.write();
        match statuses.iter_mut().find(|s| s.name == name) {
            Some(s) => {
                s.has_key = true;
                s.source = "registered".into();
            }
            None => statuses.push(ProviderStatus { name: name.clone(), has_key: true, source: "registered".into() }),
        }
        self.map.write().insert(name, provider);
    }

    pub fn status(&self) -> Vec<ProviderStatus> {
        self.statuses.read().clone()
    }

    /// Names of providers with a usable key.
    pub fn available(&self) -> Vec<String> {
        self.map.read().keys().cloned().collect()
    }

    /// Resolve a native `provider:model` string to the provider and the bare model id.
    pub fn for_model(&self, provider_model: &str) -> Result<(Arc<dyn Provider>, String), ProviderError> {
        let (name, model) = provider_model
            .split_once(':')
            .filter(|(p, m)| !p.is_empty() && !m.is_empty())
            .ok_or_else(|| ProviderError::BadModel(provider_model.to_string()))?;
        let provider = self.get(name).ok_or_else(|| {
            if matches!(name, "anthropic" | "openai") {
                ProviderError::NoKey(name.to_string())
            } else {
                ProviderError::UnknownProvider(name.to_string())
            }
        })?;
        Ok((provider, model.to_string()))
    }
}

// ---------------------------------------------------------------------------------------------
// Structured output (judge calls)
// ---------------------------------------------------------------------------------------------

pub const DECIDE_TOOL: &str = "decide";

/// One structured decision: the model must call a tool named `decide` whose input schema is
/// `schema`. Returns the tool input and the usage of every attempt.
pub async fn structured(
    provider: &dyn Provider,
    model: &str,
    system: &str,
    user: &str,
    schema: serde_json::Value,
    effort: Effort,
) -> Result<(serde_json::Value, Usage), ProviderError> {
    structured_cancellable(provider, model, system, user, schema, effort, CancellationToken::new()).await
}

pub async fn structured_cancellable(
    provider: &dyn Provider,
    model: &str,
    system: &str,
    user: &str,
    schema: serde_json::Value,
    effort: Effort,
    cancel: CancellationToken,
) -> Result<(serde_json::Value, Usage), ProviderError> {
    let mut req = ChatRequest::new(model);
    req.system = vec![SystemBlock::cached(system)];
    req.messages = vec![Message::user_text(user)];
    req.tools = vec![ToolDef {
        name: DECIDE_TOOL.into(),
        description: "Record your decision. Call this exactly once with the complete decision.".into(),
        input_schema: schema,
        cache: true,
    }];
    req.tool_choice = ToolChoice::Tool(DECIDE_TOOL.into());
    req.effort = effort;
    req.max_tokens = 16_000;
    let mut usage = Usage::default();
    let noop = |_: StreamEvent| {};
    // Models that reject forced tool calls get `auto`, so one reminder turn is allowed.
    for attempt in 0..2 {
        let resp = provider.chat(req.clone(), &noop, cancel.clone()).await?;
        usage.add(&resp.usage);
        if let Some((_, _, input)) = resp.tool_uses().into_iter().find(|(_, name, _)| *name == DECIDE_TOOL) {
            return Ok((input.clone(), usage));
        }
        if resp.stop == StopReason::Refusal {
            return Err(ProviderError::Decode(format!(
                "the model refused the decision{}",
                resp.refusal_category.map(|c| format!(" (category {c})")).unwrap_or_default()
            )));
        }
        if attempt == 0 {
            req.messages.push(Message::assistant(resp.content.clone()));
            req.messages.push(Message::user_text(
                "Call the `decide` tool now with your decision. Do not answer in text.",
            ));
        }
    }
    Err(ProviderError::Decode("the model did not call the `decide` tool".into()))
}

/// Replace the content of all but the newest `keep` tool results with a short marker. Used by
/// the agent loop for providers without server-side context editing.
pub fn clear_old_tool_results(messages: &mut [Message], keep: usize) -> usize {
    let mut positions = vec![];
    for (mi, m) in messages.iter().enumerate() {
        for (bi, b) in m.content.iter().enumerate() {
            if matches!(b, Block::ToolResult { .. }) {
                positions.push((mi, bi));
            }
        }
    }
    let cut = positions.len().saturating_sub(keep);
    let mut cleared = 0;
    for &(mi, bi) in &positions[..cut] {
        if let Block::ToolResult { content, .. } = &mut messages[mi].content[bi]
            && content != CLEARED_MARKER
        {
            *content = CLEARED_MARKER.to_string();
            cleared += 1;
        }
    }
    cleared
}

pub const CLEARED_MARKER: &str = "[Old tool result cleared to save context. Re-run the tool if you need it again.]";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_never_prints() {
        let k = ApiKey::new("sk-secret-value");
        assert!(!format!("{k:?}").contains("secret"));
        assert!(!format!("{k}").contains("secret"));
    }

    #[test]
    fn registry_resolves_models() {
        let cfg = GlobalConfig::default();
        let providers = Providers::from_config_with(&cfg, |name, _| {
            (name == "anthropic").then(|| KeyLookup { key: ApiKey::new("k"), source: "env:TEST".into() })
        });
        let status = providers.status();
        assert!(status.iter().any(|s| s.name == "anthropic" && s.has_key && s.source == "env:TEST"));
        assert!(status.iter().any(|s| s.name == "openai" && !s.has_key));
        let (p, m) = providers.for_model("anthropic:claude-sonnet-5").unwrap();
        assert_eq!((p.name(), m.as_str()), ("anthropic", "claude-sonnet-5"));
        assert!(matches!(providers.for_model("openai:gpt-5.6-sol"), Err(ProviderError::NoKey(_))));
        assert!(matches!(providers.for_model("nope"), Err(ProviderError::BadModel(_))));
        providers.register("mock", Arc::new(ScriptedProvider::new()));
        assert!(providers.for_model("mock:any").is_ok());
    }

    #[test]
    fn clears_old_results() {
        let mut msgs = vec![
            Message::tool_results(vec![Block::tool_result("a", "one", false)]),
            Message::tool_results(vec![Block::tool_result("b", "two", false), Block::tool_result("c", "three", false)]),
        ];
        assert_eq!(clear_old_tool_results(&mut msgs, 1), 2);
        assert_eq!(clear_old_tool_results(&mut msgs, 1), 0);
        assert!(matches!(&msgs[1].content[1], Block::ToolResult { content, .. } if content == "three"));
    }

    #[tokio::test]
    async fn structured_retries_once_without_call() {
        let p = ScriptedProvider::new();
        p.push_text("I think the answer is low.");
        p.push_tool_use(DECIDE_TOOL, serde_json::json!({"stakes": "low"}));
        let (v, _) = structured(&p, "m", "sys", "user", serde_json::json!({"type": "object"}), Effort::Low)
            .await
            .unwrap();
        assert_eq!(v["stakes"], "low");
        let reqs = p.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1].messages.len(), 3);
    }
}
