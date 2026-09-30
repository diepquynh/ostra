//! Anthropic Messages API, streamed over SSE.

use crate::retry::{self, RetryPolicy, error_from_response, network_error};
use crate::{
    ApiKey, Block, ChatRequest, ChatResponse, Compacted, EventSink, Message, Provider,
    ProviderError, Role, ServerTools, StopReason, StreamEvent, ToolChoice, compact_on_client,
    pricing, sse,
};
use futures::StreamExt;
use ostra_core::Effort;
use ostra_core::exec::Usage;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const COMPACT_BETA: &str = "compact-2026-09-04";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Budget thinking between tool calls, on the budget models that support it.
const INTERLEAVED_BETA: &str = "interleaved-thinking-2025-05-14";
/// Input plus `max_tokens` past the context window stops generation at the window instead of
/// failing validation, which Claude 4.5 and later models do without it.
const WINDOW_BETA: &str = "model-context-window-exceeded-2025-08-26";
/// Room kept for the answer under a thinking budget, because the budget must stay below
/// `max_tokens` and an answer can be a whole file written through a tool call.
const ANSWER_RESERVE: u32 = 16_000;
/// The API's smallest thinking budget.
const MIN_BUDGET: u32 = 1024;
/// Key set on a tool-use input whose streamed JSON did not parse. The agent loop answers such a
/// call with an error result instead of running the tool.
pub const INVALID_INPUT_KEY: &str = "__invalid_json";

pub struct Anthropic {
    key: ApiKey,
    base_url: String,
    client: reqwest::Client,
    retry: RetryPolicy,
}

impl std::fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Anthropic")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl Anthropic {
    pub fn new(key: ApiKey, base_url: Option<String>) -> Self {
        Anthropic {
            key,
            base_url: base_url
                .unwrap_or_else(|| DEFAULT_BASE_URL.into())
                .trim_end_matches('/')
                .to_string(),
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(20))
                // The API key header would follow a redirect to another host.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Thinking {
    /// `thinking: {type: "adaptive"}` plus `output_config.effort`.
    Adaptive,
    /// `thinking: {type: "enabled", budget_tokens}`; older models.
    Budget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffortSupport {
    None,
    /// `low`, `medium`, `high` only.
    Basic,
    /// No `xhigh`.
    NoXhigh,
    Full,
}

#[derive(Debug, Clone, Copy)]
struct Caps {
    thinking: Thinking,
    effort: EffortSupport,
    forced_tool_choice: bool,
    fallbacks: bool,
    /// On-demand compaction (`compaction: {type: "summarize"}`).
    compaction: bool,
    /// Budget thinking interleaves with tool calls behind `INTERLEAVED_BETA`.
    interleaved: bool,
    /// Models before Claude 4.5, which need `WINDOW_BETA`.
    window_beta: bool,
}

fn is_model(model: &str, id: &str) -> bool {
    // A dated snapshot (`-20251001`) is the same model.
    model == id
        || model.strip_prefix(id).is_some_and(|rest| {
            rest.len() == 9
                && rest.starts_with('-')
                && rest[1..].chars().all(|c| c.is_ascii_digit())
        })
}

fn caps(model: &str) -> Caps {
    // Adaptive thinking interleaves with tool calls on its own.
    let full = |forced, fallbacks| Caps {
        thinking: Thinking::Adaptive,
        effort: EffortSupport::Full,
        forced_tool_choice: forced,
        fallbacks,
        compaction: true,
        interleaved: false,
        window_beta: false,
    };
    if is_model(model, "claude-fable-5-1")
        || is_model(model, "claude-mythos-5-1")
        || is_model(model, "claude-opus-5-5")
    {
        return full(false, true);
    }
    if is_model(model, "claude-opus-5") {
        return full(true, true);
    }
    // Sonnet 5.5: no forced tool choice, as with Opus 5.5, and no server-side fallbacks, as with Sonnet 5.
    if is_model(model, "claude-sonnet-5-5") {
        return full(false, false);
    }
    if [
        "claude-fable-5",
        "claude-mythos-5",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-sonnet-5",
    ]
    .iter()
    .any(|id| is_model(model, id))
    {
        return full(true, false);
    }
    // Both modes work on the 4.6 models, and adaptive is the one that interleaves on both (budget
    // thinking on Opus 4.6 never thinks between tool calls), so they stay adaptive.
    if is_model(model, "claude-opus-4-6") || is_model(model, "claude-sonnet-4-6") {
        return Caps {
            effort: EffortSupport::NoXhigh,
            ..full(true, false)
        };
    }
    // The models that take only a fixed budget reject adaptive thinking.
    let budget = |effort, interleaved, window_beta| Caps {
        thinking: Thinking::Budget,
        effort,
        forced_tool_choice: true,
        fallbacks: false,
        compaction: false,
        interleaved,
        window_beta,
    };
    if is_model(model, "claude-opus-4-5") {
        return budget(EffortSupport::Basic, true, false);
    }
    // Longest prefix first, so `claude-sonnet-4-5` is not read as `claude-sonnet-4`. Haiku 4.5
    // cannot interleave.
    let older = [
        ("claude-haiku-4-5", false, false),
        ("claude-sonnet-4-5", true, false),
        ("claude-opus-4-1", true, true),
        ("claude-opus-4", true, true),
        ("claude-sonnet-4", true, true),
        ("claude-3", false, true),
    ];
    if let Some((_, interleaved, window_beta)) = older.iter().find(|(id, ..)| model.starts_with(id))
    {
        return budget(EffortSupport::None, *interleaved, *window_beta);
    }
    // Unknown, presumably newer models: the surface every current model accepts.
    full(false, false)
}

fn effort_str(effort: Effort, support: EffortSupport) -> Option<&'static str> {
    match support {
        EffortSupport::None => None,
        EffortSupport::Basic => Some(match effort {
            Effort::Low => "low",
            Effort::Medium => "medium",
            _ => "high",
        }),
        EffortSupport::NoXhigh => Some(match effort {
            Effort::Xhigh => "high",
            e => e.as_str(),
        }),
        EffortSupport::Full => Some(effort.as_str()),
    }
}

/// The thinking budget for an effort on a budget model: a share of its output limit, and all of it
/// but the answer's room at `max`, because the chosen effort is what the user agreed to spend.
fn budget_tokens(effort: Effort, max_output: u32) -> Option<u32> {
    Some(match effort {
        Effort::Low => return None,
        Effort::Medium => max_output / 8,
        Effort::High => max_output / 4,
        Effort::Xhigh => max_output / 2,
        Effort::Max => max_output.saturating_sub(ANSWER_RESERVE),
    })
}

fn block_json(block: &Block) -> Option<Value> {
    Some(match block {
        Block::Text { text } if text.is_empty() => return None,
        Block::Text { text } => json!({"type": "text", "text": text}),
        Block::Thinking { text, signature } => {
            json!({"type": "thinking", "thinking": text, "signature": signature})
        }
        Block::RedactedThinking { data } => json!({"type": "redacted_thinking", "data": data}),
        Block::ToolUse { id, name, input } => {
            json!({"type": "tool_use", "id": id, "name": name, "input": input})
        }
        Block::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let mut v =
                json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content});
            if *is_error {
                v["is_error"] = json!(true);
            }
            v
        }
        Block::Opaque { provider, value } if provider == "anthropic" => value.clone(),
        Block::Opaque { .. } => return None,
        Block::Compaction {
            provider,
            value: Some(value),
            ..
        } if provider == "anthropic" => value.clone(),
        Block::Compaction { summary, .. } if summary.is_empty() => return None,
        Block::Compaction { summary, .. } => json!({"type": "text", "text": summary}),
    })
}

fn messages_json(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = messages
        .iter()
        .filter_map(|m| {
            let content: Vec<Value> = m.content.iter().filter_map(block_json).collect();
            if content.is_empty() {
                return None;
            }
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            Some(json!({"role": role, "content": content}))
        })
        .collect();
    // Cache breakpoint on the most recent user turn.
    if let Some(last_user) = out.iter_mut().rev().find(|m| m["role"] == "user")
        && let Some(last_block) = last_user["content"]
            .as_array_mut()
            .and_then(|c| c.last_mut())
        && matches!(last_block["type"].as_str(), Some("text" | "tool_result"))
    {
        last_block["cache_control"] = cache_mark();
    }
    out
}

/// Every breakpoint uses the 1-hour TTL: an execution's turns can sit more than five minutes apart
/// (a long build, a permission ask), and mixing TTLs would need the longer entries first.
fn cache_mark() -> Value {
    json!({"type": "ephemeral", "ttl": "1h"})
}

pub(crate) fn request_body(req: &ChatRequest) -> (Value, Vec<&'static str>) {
    let caps = caps(&req.model);
    let mut betas = vec![];
    // models.dev holds each model's output limit; a model it does not list gets what the caller asked.
    let ceiling = ostra_core::pricing::max_output(&req.model)
        .map_or(req.max_tokens, |n| u32::try_from(n).unwrap_or(u32::MAX));
    let max_tokens = req.max_tokens.min(ceiling);
    let mut body = json!({
        "model": req.model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": messages_json(&req.messages),
    });

    if !req.system.is_empty() {
        let flagged: Vec<usize> = req
            .system
            .iter()
            .enumerate()
            .filter(|(_, s)| s.cache)
            .map(|(i, _)| i)
            .collect();
        let marks: Vec<usize> = if flagged.is_empty() {
            vec![req.system.len() - 1]
        } else {
            flagged.into_iter().rev().take(2).collect()
        };
        let system: Vec<Value> = req
            .system
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.text.is_empty())
            .map(|(i, s)| {
                let mut v = json!({"type": "text", "text": s.text});
                if marks.contains(&i) {
                    v["cache_control"] = cache_mark();
                }
                v
            })
            .collect();
        if !system.is_empty() {
            body["system"] = Value::Array(system);
        }
    }

    let mut tools: Vec<Value> = vec![];
    // The 20260209 web tools run inside code execution, whose failures cost a turn each; the
    // earlier versions call the search and fetch backends directly.
    if req.server_tools.web_search {
        tools.push(json!({"type": "web_search_20250305", "name": "web_search"}));
    }
    if req.server_tools.web_fetch {
        tools.push(json!({"type": "web_fetch_20250910", "name": "web_fetch"}));
    }
    let client_tools = req.tools.len();
    for (i, t) in req.tools.iter().enumerate() {
        let mut v = json!({
            "name": t.name,
            "description": t.description,
            "input_schema": t.input_schema,
            "eager_input_streaming": true,
        });
        if i + 1 == client_tools {
            v["cache_control"] = cache_mark();
        }
        tools.push(v);
    }

    let mut forced = false;
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        let choice = match &req.tool_choice {
            ToolChoice::Auto => json!({"type": "auto"}),
            ToolChoice::None => json!({"type": "none"}),
            ToolChoice::Any if caps.forced_tool_choice => {
                forced = true;
                json!({"type": "any"})
            }
            ToolChoice::Tool(name) if caps.forced_tool_choice => {
                forced = true;
                json!({"type": "tool", "name": name})
            }
            ToolChoice::Any | ToolChoice::Tool(_) => json!({"type": "auto"}),
        };
        body["tool_choice"] = choice;
    }

    match caps.thinking {
        Thinking::Adaptive => {
            // A forced tool call runs without an explicit thinking config.
            if !forced {
                body["thinking"] = json!({"type": "adaptive", "display": "summarized"});
            }
        }
        Thinking::Budget => {
            // Interleaved, the budget spans the whole turn and may pass `max_tokens`.
            let interleaved = caps.interleaved && client_tools > 0;
            let room = if interleaved {
                u32::MAX
            } else {
                max_tokens.saturating_sub(ANSWER_RESERVE)
            };
            if !forced
                && let Some(budget) = budget_tokens(req.effort, ceiling)
                    .map(|b| b.min(room))
                    .filter(|b| *b >= MIN_BUDGET)
            {
                body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
                if interleaved {
                    betas.push(INTERLEAVED_BETA);
                }
            }
        }
    }
    if caps.window_beta {
        betas.push(WINDOW_BETA);
    }
    if let Some(effort) = effort_str(req.effort, caps.effort) {
        body["output_config"] = json!({"effort": effort});
    }
    if req.messages.iter().flat_map(|m| &m.content).any(is_signed_compaction) {
        betas.push(COMPACT_BETA);
    }
    if req.refusal_fallback && caps.fallbacks {
        body["fallbacks"] = json!("default");
        betas.push(FALLBACK_BETA);
    }
    (body, betas)
}

fn is_signed_compaction(b: &Block) -> bool {
    matches!(b, Block::Compaction { provider, value: Some(_), .. } if provider == "anthropic")
}

/// A request that asks for a summary of `req.messages` in place of a reply.
pub(crate) fn compaction_body(req: &ChatRequest, instructions: &str) -> (Value, Vec<&'static str>) {
    let mut req = req.clone();
    req.tool_choice = ToolChoice::Auto;
    let (mut body, mut betas) = request_body(&req);
    body["compaction"] = json!({"type": "summarize", "instructions": instructions});
    if !betas.contains(&COMPACT_BETA) {
        betas.push(COMPACT_BETA);
    }
    (body, betas)
}

// ---------------------------------------------------------------------------------------------
// Stream accumulation
// ---------------------------------------------------------------------------------------------

enum Partial {
    Text(String),
    Thinking {
        text: String,
        signature: String,
    },
    Redacted(String),
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
    ServerToolUse {
        value: Value,
        json: String,
    },
    /// Arrives whole in `content_block_start`.
    Compaction(Value),
    Fallback,
    Other(Value),
}

#[derive(Default)]
struct Accumulator {
    blocks: Vec<(usize, Option<Partial>, Option<Block>)>,
    usage: Usage,
    compaction_usage: Usage,
    web_searches: u64,
    model: String,
    stop: Option<String>,
    refusal_category: Option<String>,
    done: bool,
}

fn parse_tool_input(json_text: &str) -> Value {
    if json_text.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(json_text).unwrap_or_else(|_| json!({ INVALID_INPUT_KEY: json_text }))
}

fn server_tool_summary(input: &Value) -> String {
    ["query", "url"]
        .iter()
        .find_map(|k| input.get(*k).and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string()
}

fn read_usage(u: &Value, usage: &mut Usage, web_searches: &mut u64) {
    let get = |k: &str| u.get(k).and_then(|v| v.as_u64());
    if let Some(v) = get("input_tokens") {
        usage.input_tokens = v;
    }
    if let Some(v) = get("output_tokens") {
        usage.output_tokens = v;
    }
    if let Some(v) = get("cache_read_input_tokens") {
        usage.cache_read_tokens = v;
    }
    if let Some(v) = get("cache_creation_input_tokens") {
        usage.cache_write_tokens = v;
    }
    if let Some(v) = u
        .pointer("/cache_creation/ephemeral_1h_input_tokens")
        .and_then(|v| v.as_u64())
    {
        usage.cache_write_1h_tokens = v;
    }
    if let Some(v) = u
        .pointer("/server_tool_use/web_search_requests")
        .and_then(|v| v.as_u64())
    {
        *web_searches = v;
    }
}

/// A compaction request reports zero at the top level and bills its summarization call in
/// `usage.iterations`.
fn read_compaction_usage(u: &Value) -> Option<Usage> {
    let iterations = u.get("iterations")?.as_array()?;
    let mut usage = Usage::default();
    for i in iterations.iter().filter(|i| i["type"] == "compaction") {
        let get = |k: &str| i.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        usage.input_tokens += get("input_tokens");
        usage.output_tokens += get("output_tokens");
        usage.cache_read_tokens += get("cache_read_input_tokens");
        usage.cache_write_tokens += get("cache_creation_input_tokens");
        usage.cache_write_1h_tokens += i
            .pointer("/cache_creation/ephemeral_1h_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
    }
    Some(usage)
}

impl Accumulator {
    fn read_usage(&mut self, u: &Value) {
        read_usage(u, &mut self.usage, &mut self.web_searches);
        if let Some(c) = read_compaction_usage(u) {
            self.compaction_usage = c;
        }
    }

    fn slot(&mut self, index: usize) -> Option<&mut (usize, Option<Partial>, Option<Block>)> {
        self.blocks.iter_mut().find(|(i, _, _)| *i == index)
    }

    /// Apply one event. Returns an error event's failure.
    fn apply(
        &mut self,
        ev: &Value,
        on_event: EventSink<'_>,
        started: &AtomicBool,
    ) -> Result<(), ProviderError> {
        let index = ev.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        match ev.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "message_start" => {
                if let Some(m) = ev.get("message") {
                    self.model = m
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if let Some(u) = m.get("usage") {
                        self.read_usage(u);
                    }
                }
            }
            "content_block_start" => {
                started.store(true, Ordering::SeqCst);
                let cb = ev.get("content_block").cloned().unwrap_or(Value::Null);
                let s = |k: &str| {
                    cb.get(k)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string()
                };
                let partial = match cb.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text" => {
                        let t = s("text");
                        if !t.is_empty() {
                            on_event(StreamEvent::TextDelta(t.clone()));
                        }
                        Partial::Text(t)
                    }
                    "thinking" => Partial::Thinking {
                        text: s("thinking"),
                        signature: s("signature"),
                    },
                    "redacted_thinking" => Partial::Redacted(s("data")),
                    "tool_use" => {
                        on_event(StreamEvent::ToolUseStarted {
                            id: s("id"),
                            name: s("name"),
                        });
                        Partial::ToolUse {
                            id: s("id"),
                            name: s("name"),
                            json: String::new(),
                        }
                    }
                    "server_tool_use" => Partial::ServerToolUse {
                        value: cb.clone(),
                        json: String::new(),
                    },
                    "fallback" => Partial::Fallback,
                    "compaction" => Partial::Compaction(cb.clone()),
                    _ => Partial::Other(cb.clone()),
                };
                self.blocks.push((index, Some(partial), None));
            }
            "content_block_delta" => {
                let delta = ev.get("delta").cloned().unwrap_or(Value::Null);
                let text = |k: &str| {
                    delta
                        .get(k)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string()
                };
                let kind = delta
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let Some((_, Some(partial), _)) = self.slot(index) else {
                    return Ok(());
                };
                match (kind.as_str(), partial) {
                    ("text_delta", Partial::Text(t)) => {
                        let d = text("text");
                        t.push_str(&d);
                        on_event(StreamEvent::TextDelta(d));
                    }
                    ("thinking_delta", Partial::Thinking { text: t, .. }) => {
                        let d = text("thinking");
                        t.push_str(&d);
                        on_event(StreamEvent::ThinkingDelta(d));
                    }
                    ("signature_delta", Partial::Thinking { signature, .. }) => {
                        signature.push_str(&text("signature"))
                    }
                    ("input_json_delta", Partial::ToolUse { id, json, .. }) => {
                        let d = text("partial_json");
                        json.push_str(&d);
                        on_event(StreamEvent::ToolInputDelta {
                            id: id.clone(),
                            partial_json: d,
                        });
                    }
                    ("input_json_delta", Partial::ServerToolUse { json, .. }) => {
                        json.push_str(&text("partial_json"))
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let Some(slot) = self.slot(index) else {
                    return Ok(());
                };
                let block = match slot.1.take() {
                    Some(Partial::Text(text)) => Some(Block::Text { text }),
                    Some(Partial::Thinking { text, signature }) => {
                        Some(Block::Thinking { text, signature })
                    }
                    Some(Partial::Redacted(data)) => Some(Block::RedactedThinking { data }),
                    Some(Partial::ToolUse { id, name, json }) => Some(Block::ToolUse {
                        id,
                        name,
                        input: parse_tool_input(&json),
                    }),
                    Some(Partial::ServerToolUse { mut value, json }) => {
                        if !json.trim().is_empty() {
                            value["input"] = parse_tool_input(&json);
                        }
                        let name = value
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("server_tool")
                            .to_string();
                        let summary = server_tool_summary(&value["input"]);
                        on_event(StreamEvent::ServerToolUsed { name, summary });
                        Some(Block::Opaque {
                            provider: "anthropic".into(),
                            value,
                        })
                    }
                    Some(Partial::Compaction(value)) => Some(Block::Compaction {
                        provider: "anthropic".into(),
                        summary: value
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        value: Some(value),
                    }),
                    Some(Partial::Fallback) => Some(Block::Opaque {
                        provider: "anthropic".into(),
                        value: json!({"type": "fallback"}),
                    }),
                    Some(Partial::Other(value)) => Some(Block::Opaque {
                        provider: "anthropic".into(),
                        value,
                    }),
                    None => None,
                };
                slot.2 = block;
            }
            "message_delta" => {
                if let Some(reason) = ev.pointer("/delta/stop_reason").and_then(|v| v.as_str()) {
                    self.stop = Some(reason.to_string());
                }
                if let Some(cat) = ev
                    .pointer("/delta/stop_details/category")
                    .and_then(|v| v.as_str())
                {
                    self.refusal_category = Some(cat.to_string());
                }
                if let Some(u) = ev.get("usage") {
                    self.read_usage(u);
                }
            }
            "message_stop" => self.done = true,
            "error" => {
                let kind = ev
                    .pointer("/error/type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("error");
                let message = ev
                    .pointer("/error/message")
                    .and_then(|v| v.as_str())
                    .unwrap_or(kind)
                    .to_string();
                return Err(match kind {
                    "overloaded_error" => ProviderError::Overloaded {
                        message,
                        retry_after: None,
                    },
                    "rate_limit_error" => ProviderError::RateLimited {
                        message,
                        retry_after: None,
                    },
                    "api_error" => ProviderError::Server {
                        status: 500,
                        message,
                        retry_after: None,
                    },
                    _ => ProviderError::InvalidRequest {
                        status: 400,
                        message,
                    },
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self, requested_model: &str) -> ChatResponse {
        let mut content: Vec<Block> = self.blocks.into_iter().filter_map(|(_, _, b)| b).collect();
        content = strip_before_fallback(content);
        let stop = match self.stop.as_deref() {
            Some("end_turn") | Some("stop_sequence") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") | Some("model_context_window_exceeded") => StopReason::MaxTokens,
            Some("pause_turn") => StopReason::PauseTurn,
            Some("refusal") => StopReason::Refusal,
            Some(other) => StopReason::Other(other.to_string()),
            None => StopReason::Other("unknown".into()),
        };
        let model = if self.model.is_empty() {
            requested_model.to_string()
        } else {
            self.model
        };
        let mut usage = self.usage;
        usage.add(&self.compaction_usage);
        usage.cost_usd = pricing::cost(&model, &usage, self.web_searches);
        ChatResponse {
            content,
            stop,
            usage,
            model,
            refusal_category: self.refusal_category,
        }
    }
}

fn is_fallback(b: &Block) -> bool {
    matches!(b, Block::Opaque { value, .. } if value["type"] == "fallback")
}

/// After a mid-output fallback, drop thinking, tool use, and unpaired server tool use before the
/// last `fallback` marker, so the declined partial is neither run nor echoed.
fn strip_before_fallback(content: Vec<Block>) -> Vec<Block> {
    let Some(boundary) = content.iter().rposition(is_fallback) else {
        return content;
    };
    let result_ids: Vec<String> = content
        .iter()
        .filter_map(|b| match b {
            Block::Opaque { value, .. } => value
                .get("tool_use_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            _ => None,
        })
        .collect();
    content
        .into_iter()
        .enumerate()
        .filter(|(i, b)| {
            if is_fallback(b) {
                return false;
            }
            if *i > boundary {
                return true;
            }
            match b {
                Block::Thinking { .. } | Block::RedactedThinking { .. } | Block::ToolUse { .. } => {
                    false
                }
                Block::Opaque { value, .. } if value["type"] == "server_tool_use" => value
                    .get("id")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| result_ids.iter().any(|r| r == id)),
                Block::Opaque { value, .. } => {
                    value.get("tool_use_id").is_some() || value["type"] == "text"
                }
                _ => true,
            }
        })
        .map(|(_, b)| b)
        .collect()
}

#[async_trait::async_trait]
impl Provider for Anthropic {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn server_tools(&self, _model: &str) -> ServerTools {
        ServerTools {
            web_search: true,
            web_fetch: true,
        }
    }

    async fn chat(
        &self,
        req: ChatRequest,
        on_event: EventSink<'_>,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, ProviderError> {
        let (body, betas) = request_body(&req);
        self.send(&body, &betas, &req.model, on_event, &cancel).await
    }

    async fn compact(
        &self,
        req: ChatRequest,
        instructions: &str,
        cancel: CancellationToken,
    ) -> Result<Compacted, ProviderError> {
        if !caps(&req.model).compaction {
            return compact_on_client(self, req, instructions, cancel).await;
        }
        let (body, betas) = compaction_body(&req, instructions);
        let resp = match self.send(&body, &betas, &req.model, &|_| {}, &cancel).await {
            // A model this build takes for newer may not offer on-demand compaction.
            Err(ProviderError::InvalidRequest { .. }) => {
                return compact_on_client(self, req, instructions, cancel).await;
            }
            r => r?,
        };
        let block = resp
            .content
            .into_iter()
            .find(is_signed_compaction)
            .filter(|_| resp.stop == StopReason::Other("compaction".into()));
        Ok(Compacted {
            messages: block.map(|b| vec![Message::assistant(vec![b])]),
            usage: resp.usage,
        })
    }
}

impl Anthropic {
    async fn send(
        &self,
        body: &Value,
        betas: &[&str],
        model: &str,
        on_event: EventSink<'_>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse, ProviderError> {
        let started = AtomicBool::new(false);
        let url = format!("{}/v1/messages", self.base_url);
        retry::run(self.retry, cancel, &started, || async {
            let mut builder = self.client.post(&url);
            builder = if self.key.is_bearer() {
                builder.bearer_auth(self.key.expose())
            } else {
                builder.header("x-api-key", self.key.expose())
            };
            let mut builder = builder
                .header("anthropic-version", API_VERSION)
                .header("content-type", "application/json")
                .header("accept", "text/event-stream");
            if !betas.is_empty() {
                builder = builder.header("anthropic-beta", betas.join(","));
            }
            let resp = builder.json(body).send().await.map_err(network_error)?;
            if !resp.status().is_success() {
                return Err(error_from_response(resp).await);
            }
            let mut acc = Accumulator::default();
            let mut events = sse::json_events(resp);
            while let Some(ev) = events.next().await {
                acc.apply(&ev?, on_event, &started)?;
                if acc.done {
                    break;
                }
            }
            if !acc.done {
                return Err(ProviderError::Network(
                    "stream ended before message_stop".into(),
                ));
            }
            Ok(acc.finish(model))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SystemBlock, ToolDef};

    fn req(model: &str) -> ChatRequest {
        let mut r = ChatRequest::new(model);
        r.system = vec![SystemBlock::new("a"), SystemBlock::new("b")];
        r.messages = vec![Message::user_text("hi")];
        r.tools = vec![
            ToolDef {
                name: "Read".into(),
                description: "d".into(),
                input_schema: json!({"type":"object"}),
                cache: false,
            },
            ToolDef {
                name: "Bash".into(),
                description: "d".into(),
                input_schema: json!({"type":"object"}),
                cache: false,
            },
        ];
        r
    }

    #[test]
    fn adaptive_body() {
        let mut r = req("claude-sonnet-5");
        r.effort = Effort::Xhigh;
        r.server_tools = ServerTools {
            web_search: true,
            web_fetch: true,
        };
        let (b, betas) = request_body(&r);
        assert_eq!(
            b["thinking"],
            json!({"type": "adaptive", "display": "summarized"})
        );
        assert_eq!(b["output_config"]["effort"], "xhigh");
        assert_eq!(b["system"][1]["cache_control"]["type"], "ephemeral");
        assert!(b["system"][0].get("cache_control").is_none());
        assert_eq!(b["tools"][0]["type"], "web_search_20250305");
        assert_eq!(b["tools"][1]["type"], "web_fetch_20250910");
        assert_eq!(b["tools"][3]["cache_control"]["type"], "ephemeral");
        assert!(b["tools"][2].get("cache_control").is_none());
        assert_eq!(
            b["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(b["messages"][0]["content"][0]["cache_control"]["ttl"], "1h");
        assert_eq!(b["system"][1]["cache_control"]["ttl"], "1h");
        assert_eq!(b["tools"][3]["cache_control"]["ttl"], "1h");
        assert!(betas.is_empty());
    }

    #[test]
    fn older_models_use_budget_and_basic_tools() {
        let mut r = req("claude-haiku-4-5-20251001");
        r.effort = Effort::High;
        r.server_tools.web_search = true;
        let (b, _) = request_body(&r);
        assert_eq!(b["thinking"]["type"], "enabled");
        assert!(b.get("output_config").is_none());
        assert_eq!(b["tools"][0]["type"], "web_search_20250305");
        r.effort = Effort::Low;
        assert!(request_body(&r).0.get("thinking").is_none());
    }

    #[test]
    fn budget_models_think_against_a_share_of_their_whole_output() {
        ostra_core::pricing::install_test_prices();
        let budget = |model: &str, effort, max_tokens, tools: bool| {
            let mut r = req(model);
            r.effort = effort;
            r.max_tokens = max_tokens;
            if !tools {
                r.tools.clear();
            }
            let (b, betas) = request_body(&r);
            (b["max_tokens"].as_u64(), b["thinking"]["budget_tokens"].as_u64(), betas)
        };
        // Haiku 4.5, whose 64k limit models.dev lists, asked for all of it as the native loop does.
        let haiku = "claude-haiku-4-5-20251001";
        assert_eq!(budget(haiku, Effort::Low, 64_000, true).1, None);
        assert_eq!(budget(haiku, Effort::Medium, 64_000, true).1, Some(8_000));
        assert_eq!(budget(haiku, Effort::High, 64_000, true).1, Some(16_000));
        assert_eq!(budget(haiku, Effort::Xhigh, 64_000, true).1, Some(32_000));
        let (max_tokens, max, betas) = budget(haiku, Effort::Max, 64_000, true);
        assert_eq!((max_tokens, max), (Some(64_000), Some(48_000)));
        assert!(betas.is_empty(), "Haiku 4.5 cannot interleave");
        // A request past the models.dev limit is held to it, and a small one leaves the answer its room.
        assert_eq!(budget(haiku, Effort::High, 200_000, true).0, Some(64_000));
        assert_eq!(budget(haiku, Effort::Max, 32_000, true).1, Some(16_000));
        assert_eq!(budget(haiku, Effort::High, 16_000, true).1, None);

        // Sonnet 4.5 is not in the sample, so what the caller asked is its limit. With tools it
        // interleaves, and the budget spans the turn.
        let (max_tokens, b, betas) = budget("claude-sonnet-4-5", Effort::Max, 64_000, true);
        assert_eq!((max_tokens, b, betas), (Some(64_000), Some(48_000), vec![INTERLEAVED_BETA]));
        let (_, b, betas) = budget("claude-sonnet-4-5", Effort::Max, 64_000, false);
        assert_eq!((b, betas), (Some(48_000), vec![]));
        // Before Claude 4.5, input plus max_tokens past the window needs a beta to be accepted.
        assert_eq!(budget("claude-opus-4-1-20250805", Effort::High, 32_000, false).2, vec![WINDOW_BETA]);
    }

    #[test]
    fn adaptive_models_get_their_whole_output_and_no_budget() {
        ostra_core::pricing::install_test_prices();
        for model in ["claude-opus-5-5", "claude-sonnet-5-5", "claude-opus-5", "claude-fable-5-1"] {
            let mut r = req(model);
            r.effort = Effort::Max;
            r.max_tokens = 1_000_000;
            let b = request_body(&r).0;
            assert_eq!(b["max_tokens"], 128_000, "{model}: the models.dev limit");
            assert_eq!(b["thinking"]["type"], "adaptive", "{model}");
            assert!(b["thinking"].get("budget_tokens").is_none(), "{model}");
        }
        // Opus 4.6 is not in the sample: it keeps what the caller asked, and stays adaptive.
        let mut r = req("claude-opus-4-6");
        r.max_tokens = 32_000;
        let b = request_body(&r).0;
        assert_eq!((b["max_tokens"].as_u64(), b["thinking"]["type"].as_str()), (Some(32_000), Some("adaptive")));
    }

    #[test]
    fn forced_tool_choice_downgrades_on_models_that_reject_it() {
        let mut r = req("claude-opus-5-5");
        r.tool_choice = ToolChoice::Tool("Read".into());
        let (b, betas) = request_body(&r);
        assert_eq!(b["tool_choice"], json!({"type": "auto"}));
        assert!(b.get("thinking").is_some());
        assert_eq!(b["fallbacks"], "default");
        assert_eq!(betas, vec![FALLBACK_BETA]);

        let mut r = req("claude-haiku-4-5");
        r.tool_choice = ToolChoice::Tool("Read".into());
        let (b, _) = request_body(&r);
        assert_eq!(b["tool_choice"], json!({"type": "tool", "name": "Read"}));
        assert!(b.get("thinking").is_none());
    }

    #[test]
    fn effort_mapping() {
        let mut r = req("claude-opus-4-6");
        r.effort = Effort::Xhigh;
        assert_eq!(request_body(&r).0["output_config"]["effort"], "high");
        assert!(!caps("claude-opus-50").forced_tool_choice);
        assert!(caps("claude-opus-5").fallbacks);
        assert!(!caps("claude-sonnet-5").fallbacks);
        assert!(!caps("claude-sonnet-5-5").forced_tool_choice);
        assert!(!caps("claude-sonnet-5-5").fallbacks);
    }

    #[test]
    fn messages_shape() {
        let mut r = req("claude-sonnet-5");
        r.messages = vec![
            Message::user_text("q"),
            Message::assistant(vec![
                Block::Thinking {
                    text: "t".into(),
                    signature: "s".into(),
                },
                Block::text(""),
                Block::ToolUse {
                    id: "1".into(),
                    name: "Read".into(),
                    input: json!({"file_path": "/a"}),
                },
                Block::Opaque {
                    provider: "openai".into(),
                    value: json!({"type": "reasoning"}),
                },
            ]),
            Message::tool_results(vec![Block::tool_result("1", "boom", true)]),
        ];
        let (b, betas) = request_body(&r);
        assert!(betas.is_empty());
        assert!(b.get("context_management").is_none());
        let asst = &b["messages"][1]["content"];
        assert_eq!(asst.as_array().unwrap().len(), 2);
        assert_eq!(asst[0]["signature"], "s");
        let res = &b["messages"][2]["content"][0];
        assert_eq!(res["is_error"], true);
        assert_eq!(res["cache_control"]["type"], "ephemeral");
        assert!(
            b["messages"][0]["content"][0]
                .get("cache_control")
                .is_none()
        );
    }

    #[test]
    fn fallback_strips_declined_partial() {
        let content = vec![
            Block::Thinking {
                text: "t".into(),
                signature: "s".into(),
            },
            Block::text("partial "),
            Block::ToolUse {
                id: "1".into(),
                name: "Read".into(),
                input: json!({}),
            },
            Block::Opaque {
                provider: "anthropic".into(),
                value: json!({"type": "server_tool_use", "id": "s1"}),
            },
            Block::Opaque {
                provider: "anthropic".into(),
                value: json!({"type": "fallback"}),
            },
            Block::text("rest"),
            Block::ToolUse {
                id: "2".into(),
                name: "Read".into(),
                input: json!({}),
            },
        ];
        let out = strip_before_fallback(content);
        assert_eq!(out.len(), 3);
        assert!(matches!(&out[0], Block::Text { text } if text == "partial "));
        assert!(matches!(&out[2], Block::ToolUse { id, .. } if id == "2"));
    }

    #[test]
    fn reads_one_hour_cache_writes() {
        let u = json!({
            "input_tokens": 10,
            "cache_creation_input_tokens": 3000,
            "cache_creation": {"ephemeral_5m_input_tokens": 2000, "ephemeral_1h_input_tokens": 1000},
        });
        let (mut usage, mut searches) = (Usage::default(), 0);
        read_usage(&u, &mut usage, &mut searches);
        assert_eq!(
            (usage.cache_write_tokens, usage.cache_write_1h_tokens),
            (3000, 1000)
        );
    }

    fn compaction(provider: &str, summary: &str, value: Option<Value>) -> Block {
        Block::Compaction {
            provider: provider.into(),
            summary: summary.into(),
            value,
        }
    }

    #[test]
    fn compaction_request_asks_for_a_summary() {
        let mut r = req("claude-opus-5-5");
        r.tool_choice = ToolChoice::Tool("x".into());
        let (b, betas) = compaction_body(&r, "Keep the paths.");
        assert_eq!(
            b["compaction"],
            json!({"type": "summarize", "instructions": "Keep the paths."})
        );
        assert_eq!(b["tool_choice"]["type"], "auto");
        assert!(betas.contains(&COMPACT_BETA));
        assert!(caps("claude-sonnet-4-6").compaction);
        assert!(!caps("claude-haiku-4-5").compaction);
        assert!(!caps("claude-opus-4-5").compaction);
    }

    #[test]
    fn compaction_blocks_go_back_signed_or_as_text() {
        let signed = json!({"type": "compaction", "content": "sum", "signature": "sig"});
        let mut r = req("claude-opus-5-5");
        r.messages = vec![
            Message::assistant(vec![compaction("anthropic", "sum", Some(signed.clone()))]),
            Message::user_text("go on"),
        ];
        let (b, betas) = request_body(&r);
        assert_eq!(b["messages"][0]["content"][0], signed);
        assert!(betas.contains(&COMPACT_BETA));

        r.messages = vec![Message {
            role: Role::User,
            content: vec![
                compaction("", "client summary", None),
                compaction("openai", "", Some(json!({"type": "compaction"}))),
                Block::text("go on"),
            ],
        }];
        let (b, betas) = request_body(&r);
        let content = b["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2, "an unreadable foreign block is dropped");
        assert_eq!(content[0], json!({"type": "text", "text": "client summary"}));
        assert!(!betas.contains(&COMPACT_BETA));
    }

    #[test]
    fn reads_a_streamed_compaction_and_its_usage() {
        crate::pricing::install_test_prices();
        let block = json!({"type": "compaction", "content": "sum", "signature": "sig"});
        let events = [
            json!({"type": "message_start", "message": {"model": "claude-opus-5-5", "usage": {"input_tokens": 0, "output_tokens": 0}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": block}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "message_delta", "delta": {"stop_reason": "compaction"}, "usage": {
                "input_tokens": 0, "output_tokens": 0,
                "iterations": [{"type": "compaction", "input_tokens": 144, "output_tokens": 276, "cache_read_input_tokens": 1000}],
            }}),
            json!({"type": "message_stop"}),
        ];
        let mut acc = Accumulator::default();
        let started = AtomicBool::new(false);
        for ev in &events {
            acc.apply(ev, &|_| {}, &started).unwrap();
        }
        let resp = acc.finish("claude-opus-5-5");
        assert_eq!(resp.stop, StopReason::Other("compaction".into()));
        assert_eq!(resp.content, vec![compaction("anthropic", "sum", Some(block))]);
        assert_eq!(
            (resp.usage.input_tokens, resp.usage.output_tokens, resp.usage.cache_read_tokens),
            (144, 276, 1000),
            "counted once, although usage arrives twice"
        );
        assert!(resp.usage.cost_usd > 0.0);
    }
}
