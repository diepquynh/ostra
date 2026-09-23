//! OpenAI Responses API, streamed over SSE. Runs stateless (`store: false`) and round-trips
//! reasoning items through `reasoning.encrypted_content`.

use crate::retry::{self, RetryPolicy, error_from_response, network_error};
use crate::{
    ApiKey, Block, ChatRequest, ChatResponse, EventSink, Message, Provider, ProviderError, Role, ServerTools,
    StopReason, StreamEvent, ToolChoice, pricing, sse,
};
use futures::StreamExt;
use ostra_core::exec::Usage;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";

pub struct OpenAi {
    key: ApiKey,
    base_url: String,
    client: reqwest::Client,
    retry: RetryPolicy,
}

impl std::fmt::Debug for OpenAi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAi").field("base_url", &self.base_url).finish_non_exhaustive()
    }
}

impl OpenAi {
    pub fn new(key: ApiKey, base_url: Option<String>) -> Self {
        OpenAi {
            key,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()).trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(20))
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

fn is_reasoning_model(model: &str) -> bool {
    model.starts_with("gpt-5") || model.starts_with("gpt-6") || model.starts_with('o')
}

fn input_items(messages: &[Message]) -> Vec<Value> {
    let mut items = vec![];
    for m in messages {
        for b in &m.content {
            match (m.role, b) {
                (_, Block::Text { text }) if text.is_empty() => {}
                (Role::User, Block::Text { text }) => items.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}],
                })),
                (Role::Assistant, Block::Text { text }) => {
                    items.push(json!({"type": "message", "role": "assistant", "content": text}))
                }
                (_, Block::ToolUse { id, name, input }) => items.push(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": input.to_string(),
                })),
                (_, Block::ToolResult { tool_use_id, content, is_error }) => {
                    let output = if *is_error && !content.starts_with("Error") {
                        format!("Error: {content}")
                    } else {
                        content.clone()
                    };
                    items.push(json!({"type": "function_call_output", "call_id": tool_use_id, "output": output}));
                }
                (_, Block::Opaque { provider, value }) if provider == "openai" && value["type"] == "reasoning" => {
                    let mut item = json!({
                        "type": "reasoning",
                        "id": value["id"],
                        "summary": value.get("summary").cloned().unwrap_or_else(|| json!([])),
                    });
                    if let Some(enc) = value.get("encrypted_content").filter(|v| !v.is_null()) {
                        item["encrypted_content"] = enc.clone();
                    }
                    items.push(item);
                }
                _ => {}
            }
        }
    }
    items
}

pub(crate) fn request_body(req: &ChatRequest) -> Value {
    let mut body = json!({
        "model": req.model,
        "input": input_items(&req.messages),
        "max_output_tokens": req.max_tokens,
        "stream": true,
        "store": false,
    });
    let instructions: Vec<&str> = req.system.iter().map(|s| s.text.as_str()).filter(|t| !t.is_empty()).collect();
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions.join("\n\n"));
    }
    if is_reasoning_model(&req.model) {
        body["reasoning"] = json!({"effort": req.effort.as_str(), "summary": "auto"});
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    let mut tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| json!({"type": "function", "name": t.name, "description": t.description, "parameters": t.input_schema}))
        .collect();
    if req.server_tools.web_search {
        tools.push(json!({"type": "web_search"}));
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        body["parallel_tool_calls"] = json!(true);
        body["tool_choice"] = match &req.tool_choice {
            ToolChoice::Auto => json!("auto"),
            ToolChoice::Any => json!("required"),
            ToolChoice::None => json!("none"),
            ToolChoice::Tool(name) => json!({"type": "function", "name": name}),
        };
    }
    body
}

#[derive(Default)]
struct Accumulator {
    items: BTreeMap<u64, Block>,
    call_ids: HashMap<String, String>,
    usage: Usage,
    model: String,
    web_searches: u64,
    refusal: bool,
    stop: Option<StopReason>,
    failure: Option<ProviderError>,
}

fn read_usage(u: &Value) -> Usage {
    let get = |p: &str| u.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0);
    let input = get("/input_tokens");
    let cached = get("/input_tokens_details/cached_tokens");
    let written = get("/input_tokens_details/cache_write_tokens");
    Usage {
        input_tokens: input.saturating_sub(cached).saturating_sub(written),
        output_tokens: get("/output_tokens"),
        cache_read_tokens: cached,
        cache_write_tokens: written,
        ..Default::default()
    }
}

fn message_text(item: &Value, refusal: &mut bool) -> String {
    let mut out = String::new();
    for part in item.get("content").and_then(|c| c.as_array()).into_iter().flatten() {
        match part.get("type").and_then(|t| t.as_str()) {
            Some("output_text") => out.push_str(part.get("text").and_then(|t| t.as_str()).unwrap_or_default()),
            Some("refusal") => {
                *refusal = true;
                out.push_str(part.get("refusal").and_then(|t| t.as_str()).unwrap_or_default());
            }
            _ => {}
        }
    }
    out
}

impl Accumulator {
    fn apply(&mut self, ev: &Value, on_event: EventSink<'_>, started: &AtomicBool) -> Result<(), ProviderError> {
        let s = |k: &str| ev.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let output_index = ev.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0);
        match ev.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "response.created" | "response.in_progress" => {
                if let Some(m) = ev.pointer("/response/model").and_then(|v| v.as_str()) {
                    self.model = m.to_string();
                }
            }
            "response.output_item.added" => {
                started.store(true, Ordering::SeqCst);
                let item = &ev["item"];
                if item["type"] == "function_call" {
                    let call_id = item["call_id"].as_str().unwrap_or_default().to_string();
                    let item_id = item["id"].as_str().unwrap_or_default().to_string();
                    let name = item["name"].as_str().unwrap_or_default().to_string();
                    self.call_ids.insert(item_id, call_id.clone());
                    on_event(StreamEvent::ToolUseStarted { id: call_id, name });
                }
            }
            "response.output_text.delta" => {
                started.store(true, Ordering::SeqCst);
                on_event(StreamEvent::TextDelta(s("delta")));
            }
            "response.reasoning_summary_text.delta" => {
                started.store(true, Ordering::SeqCst);
                on_event(StreamEvent::ThinkingDelta(s("delta")));
            }
            "response.function_call_arguments.delta" => {
                let item_id = s("item_id");
                let id = self.call_ids.get(&item_id).cloned().unwrap_or(item_id);
                on_event(StreamEvent::ToolInputDelta { id, partial_json: s("delta") });
            }
            "response.output_item.done" => {
                let item = &ev["item"];
                let block = match item["type"].as_str().unwrap_or("") {
                    "message" => Some(Block::Text { text: message_text(item, &mut self.refusal) }),
                    "function_call" => {
                        let args = item["arguments"].as_str().unwrap_or("{}");
                        let input = if args.trim().is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(args)
                                .unwrap_or_else(|_| json!({ crate::anthropic::INVALID_INPUT_KEY: args }))
                        };
                        Some(Block::ToolUse {
                            id: item["call_id"].as_str().unwrap_or_default().to_string(),
                            name: item["name"].as_str().unwrap_or_default().to_string(),
                            input,
                        })
                    }
                    "reasoning" => Some(Block::Opaque {
                        provider: "openai".into(),
                        value: json!({
                            "type": "reasoning",
                            "id": item["id"],
                            "summary": item.get("summary").cloned().unwrap_or_else(|| json!([])),
                            "encrypted_content": item.get("encrypted_content").cloned().unwrap_or(Value::Null),
                        }),
                    }),
                    "web_search_call" => {
                        self.web_searches += 1;
                        let summary = item
                            .pointer("/action/query")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        on_event(StreamEvent::ServerToolUsed { name: "web_search".into(), summary });
                        None
                    }
                    _ => None,
                };
                if let Some(b) = block {
                    self.items.insert(output_index, b);
                }
            }
            "response.completed" | "response.incomplete" => {
                let resp = &ev["response"];
                if let Some(m) = resp.get("model").and_then(|v| v.as_str()) {
                    self.model = m.to_string();
                }
                if let Some(u) = resp.get("usage") {
                    self.usage = read_usage(u);
                }
                self.stop = Some(if ev["type"] == "response.incomplete" {
                    match resp.pointer("/incomplete_details/reason").and_then(|v| v.as_str()) {
                        Some("max_output_tokens") => StopReason::MaxTokens,
                        Some("content_filter") => StopReason::Refusal,
                        Some(other) => StopReason::Other(other.to_string()),
                        None => StopReason::Other("incomplete".into()),
                    }
                } else if self.refusal {
                    StopReason::Refusal
                } else if self.items.values().any(|b| matches!(b, Block::ToolUse { .. })) {
                    StopReason::ToolUse
                } else {
                    StopReason::EndTurn
                });
            }
            "response.failed" => {
                let message = ev
                    .pointer("/response/error/message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("the response failed")
                    .to_string();
                self.failure = Some(ProviderError::Server { status: 500, message, retry_after: None });
            }
            "error" => {
                let message = s("message");
                let code = s("code");
                return Err(match code.as_str() {
                    "rate_limit_exceeded" => ProviderError::RateLimited { message, retry_after: None },
                    "server_error" | "server_is_overloaded" | "slow_down" => {
                        ProviderError::Server { status: 500, message, retry_after: None }
                    }
                    _ => ProviderError::InvalidRequest { status: 400, message: format!("{code}: {message}") },
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self, requested_model: &str) -> ChatResponse {
        let model = if self.model.is_empty() { requested_model.to_string() } else { self.model };
        let mut usage = self.usage;
        usage.cost_usd = pricing::cost(&model, &usage, 0, self.web_searches);
        ChatResponse {
            content: self.items.into_values().collect(),
            stop: self.stop.unwrap_or(StopReason::Other("unknown".into())),
            usage,
            model,
            refusal_category: None,
        }
    }
}

#[async_trait::async_trait]
impl Provider for OpenAi {
    fn name(&self) -> &str {
        "openai"
    }

    fn server_tools(&self, _model: &str) -> ServerTools {
        ServerTools { web_search: true, web_fetch: false }
    }

    async fn chat(
        &self,
        req: ChatRequest,
        on_event: EventSink<'_>,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, ProviderError> {
        let body = request_body(&req);
        let started = AtomicBool::new(false);
        let url = format!("{}/v1/responses", self.base_url);
        retry::run(self.retry, &cancel, &started, || async {
            let resp = self
                .client
                .post(&url)
                .bearer_auth(self.key.expose())
                .header("accept", "text/event-stream")
                .json(&body)
                .send()
                .await
                .map_err(network_error)?;
            if !resp.status().is_success() {
                return Err(error_from_response(resp).await);
            }
            let mut acc = Accumulator::default();
            let mut events = sse::json_events(resp);
            while let Some(ev) = events.next().await {
                acc.apply(&ev?, on_event, &started)?;
                if let Some(e) = acc.failure.take() {
                    return Err(e);
                }
                if acc.stop.is_some() {
                    break;
                }
            }
            if acc.stop.is_none() {
                return Err(ProviderError::Network("stream ended before response.completed".into()));
            }
            Ok(acc.finish(&req.model))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SystemBlock, ToolDef};
    use ostra_core::Effort;

    #[test]
    fn body_shape() {
        let mut r = ChatRequest::new("gpt-5.6-terra");
        r.system = vec![SystemBlock::new("one"), SystemBlock::cached("two")];
        r.effort = Effort::Xhigh;
        r.server_tools = ServerTools { web_search: true, web_fetch: true };
        r.tools = vec![ToolDef { name: "Read".into(), description: "d".into(), input_schema: json!({"type":"object"}), cache: true }];
        r.tool_choice = ToolChoice::Tool("Read".into());
        r.messages = vec![
            Message::user_text("q"),
            Message::assistant(vec![
                Block::Opaque {
                    provider: "openai".into(),
                    value: json!({"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "enc", "status": "completed"}),
                },
                Block::Thinking { text: "x".into(), signature: "y".into() },
                Block::text("calling"),
                Block::ToolUse { id: "call_1".into(), name: "Read".into(), input: json!({"file_path": "/a"}) },
            ]),
            Message::tool_results(vec![Block::tool_result("call_1", "nope", true)]),
        ];
        let b = request_body(&r);
        assert_eq!(b["instructions"], "one\n\ntwo");
        assert_eq!(b["reasoning"], json!({"effort": "xhigh", "summary": "auto"}));
        assert_eq!(b["include"][0], "reasoning.encrypted_content");
        assert_eq!(b["store"], false);
        assert_eq!(b["tools"][1], json!({"type": "web_search"}));
        assert_eq!(b["tools"].as_array().unwrap().len(), 2);
        assert_eq!(b["tool_choice"], json!({"type": "function", "name": "Read"}));
        let input = b["input"].as_array().unwrap();
        assert_eq!(input.len(), 5);
        assert_eq!(input[1], json!({"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "enc"}));
        assert_eq!(input[3]["arguments"], "{\"file_path\":\"/a\"}");
        assert_eq!(input[4]["output"], "Error: nope");
    }

    #[test]
    fn non_reasoning_model_omits_reasoning() {
        let b = request_body(&ChatRequest::new("gpt-4.1"));
        assert!(b.get("reasoning").is_none());
        assert!(b.get("tools").is_none());
    }
}
