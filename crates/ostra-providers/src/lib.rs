//! Model providers behind one neutral chat interface: Anthropic (Messages API) and OpenAI
//! (Responses API), both streaming, plus a scripted provider for tests.

pub mod anthropic;
pub mod mock;
pub mod openai;
mod retry;
mod sse;

use ostra_core::Effort;
use ostra_core::api::{ProviderStatus, SavedProviderView};
use ostra_core::config::{GlobalConfig, ProviderConfig, SavedCredentials};
use ostra_core::exec::Usage;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

pub use mock::ScriptedProvider;
pub use ostra_core::pricing::{self, Pricing, price};
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
        SystemBlock {
            text: text.into(),
            cache: false,
        }
    }

    pub fn cached(text: impl Into<String>) -> Self {
        SystemBlock {
            text: text.into(),
            cache: true,
        }
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
        Message {
            role: Role::User,
            content: vec![Block::Text { text: text.into() }],
        }
    }

    pub fn assistant(content: Vec<Block>) -> Self {
        Message {
            role: Role::Assistant,
            content,
        }
    }

    pub fn tool_results(results: Vec<Block>) -> Self {
        Message {
            role: Role::User,
            content: results,
        }
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

    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Block::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolUseStarted {
        id: String,
        name: String,
    },
    ToolInputDelta {
        id: String,
        partial_json: String,
    },
    /// A provider-side tool ran (web search, web fetch). `summary` is its query or URL.
    ServerToolUsed {
        name: String,
        summary: String,
    },
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
    RateLimited {
        message: String,
        retry_after: Option<u64>,
    },
    #[error("provider overloaded: {message}")]
    Overloaded {
        message: String,
        retry_after: Option<u64>,
    },
    #[error("provider error ({status}): {message}")]
    Server {
        status: u16,
        message: String,
        retry_after: Option<u64>,
    },
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

/// A resolved key and where it came from (`env:NAME`, `saved`, or `keychain`).
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
        f.debug_struct("Providers")
            .field("names", &self.map.read().keys().collect::<Vec<_>>())
            .finish()
    }
}

fn env_value(var: Option<&str>) -> Option<(String, String)> {
    let var = var?;
    let value = std::env::var(var).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| (value.to_string(), format!("env:{var}")))
}

fn saved_value(value: Option<&String>) -> Option<&str> {
    value.map(|v| v.trim()).filter(|v| !v.is_empty())
}

/// Look a key up in the environment, then what was saved from the browser, then the OS keychain.
pub fn lookup_key(name: &str, cfg: &ProviderConfig, saved: &SavedCredentials) -> Option<KeyLookup> {
    if let Some((value, source)) = env_value(cfg.api_key_env.as_deref()) {
        return Some(KeyLookup {
            key: ApiKey::new(value),
            source,
        });
    }
    if let Some((value, source)) = env_value(cfg.auth_token_env.as_deref()) {
        return Some(KeyLookup {
            key: ApiKey::bearer(value),
            source,
        });
    }
    if let Some(value) = saved_value(saved.api_key.as_ref()) {
        return Some(KeyLookup {
            key: ApiKey::new(value),
            source: "saved".into(),
        });
    }
    if let Some(value) = saved_value(saved.auth_token.as_ref()) {
        return Some(KeyLookup {
            key: ApiKey::bearer(value),
            source: "saved".into(),
        });
    }
    let service = cfg.keychain_service.as_deref()?;
    let entry = keyring::Entry::new(service, name).ok()?;
    let value = entry.get_password().ok()?;
    if value.trim().is_empty() {
        return None;
    }
    Some(KeyLookup {
        key: ApiKey::new(value.trim()),
        source: "keychain".into(),
    })
}

/// A base URL safe to show in the browser: no user, password, query, or fragment, since any of
/// them can carry a credential.
pub fn display_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(u)
            if u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
                && u.fragment().is_none() =>
        {
            url.to_string()
        }
        Ok(mut u) => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => "(not a valid URL)".into(),
    }
}

/// The base URL and where it came from: `config.toml`, then the environment, then what was saved.
pub fn resolve_base_url(
    cfg: &ProviderConfig,
    saved: &SavedCredentials,
) -> (Option<String>, String) {
    if let Some(url) = saved_value(cfg.base_url.as_ref()) {
        return (Some(url.to_string()), "config".into());
    }
    if let Some((url, source)) = env_value(cfg.base_url_env.as_deref()) {
        return (Some(url), source);
    }
    if let Some(url) = saved_value(saved.base_url.as_ref()) {
        return (Some(url.to_string()), "saved".into());
    }
    (None, "default".into())
}

type Lookup<'a> = &'a dyn Fn(&str, &ProviderConfig, &SavedCredentials) -> Option<KeyLookup>;

impl Providers {
    pub fn empty() -> Self {
        Providers {
            map: RwLock::new(BTreeMap::new()),
            statuses: RwLock::new(vec![]),
        }
    }

    /// Build from the global config and the saved credentials, reading keys from the environment,
    /// the saved credentials, or the OS keychain.
    pub fn from_config(cfg: &GlobalConfig, saved: &BTreeMap<String, SavedCredentials>) -> Self {
        Providers::from_config_with(cfg, saved, lookup_key)
    }

    /// Build with a custom key lookup (tests).
    pub fn from_config_with(
        cfg: &GlobalConfig,
        saved: &BTreeMap<String, SavedCredentials>,
        lookup: impl Fn(&str, &ProviderConfig, &SavedCredentials) -> Option<KeyLookup>,
    ) -> Self {
        let providers = Providers::empty();
        providers.rebuild(cfg, saved, &lookup);
        providers
    }

    /// Rebuild in place after the config or the saved credentials change. Executors hold this
    /// registry, so the next model call uses the new keys. Registered providers are kept.
    pub fn reload(&self, cfg: &GlobalConfig, saved: &BTreeMap<String, SavedCredentials>) {
        self.rebuild(cfg, saved, &lookup_key);
    }

    fn rebuild(
        &self,
        cfg: &GlobalConfig,
        saved: &BTreeMap<String, SavedCredentials>,
        lookup: Lookup<'_>,
    ) {
        let mut map = BTreeMap::new();
        let mut statuses = vec![];
        let none = SavedCredentials::default();
        for (name, pcfg) in &cfg.providers {
            let s = saved.get(name).unwrap_or(&none);
            let found = lookup(name, pcfg, s);
            let (base_url, base_url_source) = resolve_base_url(pcfg, s);
            statuses.push(ProviderStatus {
                name: name.clone(),
                has_key: found.is_some(),
                source: found
                    .as_ref()
                    .map(|k| k.source.clone())
                    .unwrap_or_else(|| "none".into()),
                base_url: base_url.as_deref().map(display_url),
                base_url_source,
                saved: SavedProviderView {
                    base_url: s.base_url.as_deref().map(display_url),
                    has_api_key: s.api_key.is_some(),
                    has_auth_token: s.auth_token.is_some(),
                },
            });
            let Some(found) = found else { continue };
            let provider: Arc<dyn Provider> = match name.as_str() {
                "anthropic" => Arc::new(anthropic::Anthropic::new(found.key, base_url)),
                "openai" => Arc::new(openai::OpenAi::new(found.key, base_url)),
                other => {
                    tracing::warn!(provider = other, "unknown provider in config; ignored");
                    continue;
                }
            };
            map.insert(name.clone(), provider);
        }
        let mut cur_statuses = self.statuses.write();
        let mut cur_map = self.map.write();
        for s in cur_statuses.iter().filter(|s| s.source == "registered") {
            if let Some(p) = cur_map.get(&s.name) {
                map.insert(s.name.clone(), p.clone());
                statuses.retain(|x| x.name != s.name);
                statuses.push(s.clone());
            }
        }
        *cur_map = map;
        *cur_statuses = statuses;
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
            None => statuses.push(ProviderStatus {
                name: name.clone(),
                has_key: true,
                source: "registered".into(),
                base_url: None,
                base_url_source: "default".into(),
                saved: SavedProviderView::default(),
            }),
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
    pub fn for_model(
        &self,
        provider_model: &str,
    ) -> Result<(Arc<dyn Provider>, String), ProviderError> {
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
    structured_cancellable(
        provider,
        model,
        system,
        user,
        schema,
        effort,
        CancellationToken::new(),
    )
    .await
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
        description: "Record your decision. Call this exactly once with the complete decision."
            .into(),
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
        if let Some((_, _, input)) = resp
            .tool_uses()
            .into_iter()
            .find(|(_, name, _)| *name == DECIDE_TOOL)
        {
            return Ok((input.clone(), usage));
        }
        if resp.stop == StopReason::Refusal {
            return Err(ProviderError::Decode(format!(
                "the model refused the decision{}",
                resp.refusal_category
                    .map(|c| format!(" (category {c})"))
                    .unwrap_or_default()
            )));
        }
        if attempt == 0 {
            req.messages.push(Message::assistant(resp.content.clone()));
            req.messages.push(Message::user_text(
                "Call the `decide` tool now with your decision. Do not answer in text.",
            ));
        }
    }
    Err(ProviderError::Decode(
        "the model did not call the `decide` tool".into(),
    ))
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

pub const CLEARED_MARKER: &str =
    "[Old tool result cleared to save context. Re-run the tool if you need it again.]";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displayed_base_urls_drop_credentials() {
        assert_eq!(display_url("https://api.example"), "https://api.example");
        assert_eq!(
            display_url("https://u:p@api.example/v1?key=s#f"),
            "https://api.example/v1"
        );
        assert_eq!(display_url("nope"), "(not a valid URL)");
    }

    #[test]
    fn key_never_prints() {
        let k = ApiKey::new("sk-secret-value");
        assert!(!format!("{k:?}").contains("secret"));
        assert!(!format!("{k}").contains("secret"));
    }

    #[test]
    fn registry_resolves_models() {
        let cfg = GlobalConfig::default();
        let providers = Providers::from_config_with(&cfg, &BTreeMap::new(), |name, _, _| {
            (name == "anthropic").then(|| KeyLookup {
                key: ApiKey::new("k"),
                source: "env:TEST".into(),
            })
        });
        let status = providers.status();
        assert!(
            status
                .iter()
                .any(|s| s.name == "anthropic" && s.has_key && s.source == "env:TEST")
        );
        assert!(status.iter().any(|s| s.name == "openai" && !s.has_key));
        let (p, m) = providers.for_model("anthropic:claude-sonnet-5").unwrap();
        assert_eq!((p.name(), m.as_str()), ("anthropic", "claude-sonnet-5"));
        assert!(matches!(
            providers.for_model("openai:gpt-5.6-sol"),
            Err(ProviderError::NoKey(_))
        ));
        assert!(matches!(
            providers.for_model("nope"),
            Err(ProviderError::BadModel(_))
        ));
        providers.register("mock", Arc::new(ScriptedProvider::new()));
        assert!(providers.for_model("mock:any").is_ok());
    }

    #[test]
    fn environment_overrides_saved_credentials() {
        let var = |suffix: &str| format!("OSTRA_TEST_{}_{suffix}", std::process::id());
        let (key_var, url_var) = (var("KEY"), var("URL"));
        let cfg = ProviderConfig {
            api_key_env: Some(key_var.clone()),
            base_url_env: Some(url_var.clone()),
            ..Default::default()
        };
        let saved = SavedCredentials {
            base_url: Some("https://saved.example".into()),
            api_key: None,
            auth_token: Some("saved-token".into()),
        };
        let found = lookup_key("anthropic", &cfg, &saved).unwrap();
        assert_eq!(
            (found.source.as_str(), found.key.is_bearer()),
            ("saved", true)
        );
        assert_eq!(
            resolve_base_url(&cfg, &saved),
            (Some("https://saved.example".into()), "saved".into())
        );

        // SAFETY: the variables are unique to this test.
        unsafe {
            std::env::set_var(&key_var, "env-key");
            std::env::set_var(&url_var, "https://env.example");
        }
        let found = lookup_key("anthropic", &cfg, &saved).unwrap();
        assert_eq!(
            (found.source, found.key.is_bearer()),
            (format!("env:{key_var}"), false)
        );
        assert_eq!(
            resolve_base_url(&cfg, &saved),
            (Some("https://env.example".into()), format!("env:{url_var}"))
        );

        let pinned = ProviderConfig {
            base_url: Some("https://config.example".into()),
            ..cfg
        };
        assert_eq!(resolve_base_url(&pinned, &saved).1, "config");
        unsafe {
            std::env::remove_var(&key_var);
            std::env::remove_var(&url_var);
        }
    }

    #[test]
    fn reload_keeps_registered_providers() {
        let cfg = GlobalConfig::default();
        let providers = Providers::from_config_with(&cfg, &BTreeMap::new(), |_, _, _| None);
        providers.register("mock", Arc::new(ScriptedProvider::new()));
        let mut saved = BTreeMap::new();
        saved.insert(
            "openai".to_string(),
            SavedCredentials {
                api_key: Some("k".into()),
                ..Default::default()
            },
        );
        providers.rebuild(&cfg, &saved, &|_, _, s| {
            s.api_key.clone().map(|k| KeyLookup {
                key: ApiKey::new(k),
                source: "saved".into(),
            })
        });
        assert!(providers.get("mock").is_some());
        assert!(providers.get("openai").is_some());
        let status = providers.status();
        assert!(
            status
                .iter()
                .any(|s| s.name == "openai" && s.source == "saved" && s.saved.has_api_key)
        );
    }

    #[test]
    fn clears_old_results() {
        let mut msgs = vec![
            Message::tool_results(vec![Block::tool_result("a", "one", false)]),
            Message::tool_results(vec![
                Block::tool_result("b", "two", false),
                Block::tool_result("c", "three", false),
            ]),
        ];
        assert_eq!(clear_old_tool_results(&mut msgs, 1), 2);
        assert_eq!(clear_old_tool_results(&mut msgs, 1), 0);
        assert!(
            matches!(&msgs[1].content[1], Block::ToolResult { content, .. } if content == "three")
        );
    }

    #[tokio::test]
    async fn structured_retries_once_without_call() {
        let p = ScriptedProvider::new();
        p.push_text("I think the answer is low.");
        p.push_tool_use(DECIDE_TOOL, serde_json::json!({"stakes": "low"}));
        let (v, _) = structured(
            &p,
            "m",
            "sys",
            "user",
            serde_json::json!({"type": "object"}),
            Effort::Low,
        )
        .await
        .unwrap();
        assert_eq!(v["stakes"], "low");
        let reqs = p.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1].messages.len(), 3);
    }
}
