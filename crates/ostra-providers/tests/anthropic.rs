mod common;

use common::{Recorded, fixture, serve};
use ostra_core::Effort;
use ostra_providers::anthropic::Anthropic;
use ostra_providers::{
    ApiKey, Block, CancellationToken, ChatRequest, Message, Provider, ProviderError, RetryPolicy,
    StopReason, StreamEvent, SystemBlock, ToolDef,
};
use parking_lot::Mutex;
use serde_json::json;

fn provider(base: String) -> Anthropic {
    ostra_core::pricing::install_test_prices();
    Anthropic::new(ApiKey::new("sk-ant-test-key"), Some(base)).with_retry(RetryPolicy::fast(2))
}

fn request() -> ChatRequest {
    let mut r = ChatRequest::new("claude-sonnet-5-5");
    r.system = vec![SystemBlock::cached("You are a test.")];
    r.messages = vec![Message::user_text("Read main.rs")];
    r.tools = vec![ToolDef {
        name: "Read".into(),
        description: "Read a file".into(),
        input_schema: json!({"type": "object", "properties": {"file_path": {"type": "string"}}}),
        cache: true,
    }];
    r.effort = Effort::High;
    r
}

#[tokio::test]
async fn streams_thinking_text_and_tool_use() {
    let (base, captured) = serve(vec![Recorded::sse(&fixture("anthropic_tool_use.sse"))]).await;
    let events = Mutex::new(vec![]);
    let sink = |e: StreamEvent| events.lock().push(e);
    let resp = provider(base)
        .chat(request(), &sink, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(resp.stop, StopReason::ToolUse);
    assert_eq!(resp.model, "claude-sonnet-5-5");
    assert!(
        matches!(&resp.content[0], Block::Thinking { text, signature } if text == "Need to read the file." && signature == "sig-abc")
    );
    assert_eq!(resp.text(), "Reading it now.");
    let uses = resp.tool_uses();
    assert_eq!(
        uses[0],
        (
            "toolu_01",
            "Read",
            &json!({"file_path": "/repo/src/main.rs"})
        )
    );
    // Truncated JSON is flagged, not dropped.
    assert_eq!(uses[1].2["__invalid_json"], "{\"command\": \"ls");

    assert_eq!(resp.usage.input_tokens, 120);
    assert_eq!(resp.usage.cache_write_tokens, 2000);
    assert_eq!(resp.usage.cache_read_tokens, 5000);
    assert_eq!(resp.usage.output_tokens, 80);
    let expected = (120.0 * 2.0 + 80.0 * 10.0 + 5000.0 * 0.2 + 2000.0 * 2.5) / 1e6;
    assert!((resp.usage.cost_usd - expected).abs() < 1e-12);

    let events = events.lock();
    assert!(events.contains(&StreamEvent::ThinkingDelta("Need to read ".into())));
    assert!(events.contains(&StreamEvent::TextDelta("it now.".into())));
    assert!(events.contains(&StreamEvent::ToolUseStarted {
        id: "toolu_01".into(),
        name: "Read".into()
    }));

    let cap = captured.lock().unwrap();
    let req = &cap[0];
    assert!(req.head.starts_with("POST /v1/messages"));
    assert_eq!(req.header("x-api-key").as_deref(), Some("sk-ant-test-key"));
    assert_eq!(
        req.header("anthropic-version").as_deref(),
        Some("2023-06-01")
    );
    assert_eq!(req.body["stream"], true);
    assert_eq!(req.body["tools"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(req.body["tools"][0]["eager_input_streaming"], true);
}

#[tokio::test]
async fn server_tool_and_pause_turn() {
    let (base, _) = serve(vec![Recorded::sse(&fixture("anthropic_web_search.sse"))]).await;
    let events = Mutex::new(vec![]);
    let sink = |e: StreamEvent| events.lock().push(e);
    let mut req = request();
    req.model = "claude-opus-5".into();
    req.server_tools.web_search = true;
    let resp = provider(base)
        .chat(req, &sink, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resp.stop, StopReason::PauseTurn);
    assert!(matches!(&resp.content[0], Block::Opaque { provider, value }
        if provider == "anthropic" && value["type"] == "server_tool_use" && value["input"]["query"] == "axum 0.8 websocket"));
    assert!(
        matches!(&resp.content[1], Block::Opaque { value, .. } if value["type"] == "web_search_tool_result")
    );
    assert_eq!(resp.text(), "Axum 0.8 has ws support.");
    assert!(events.lock().contains(&StreamEvent::ServerToolUsed {
        name: "web_search".into(),
        summary: "axum 0.8 websocket".into()
    }));
    let expected = (50.0 * 5.0 + 40.0 * 25.0) / 1e6 + 0.01;
    assert!((resp.usage.cost_usd - expected).abs() < 1e-12);
}

#[tokio::test]
async fn refusal_carries_category() {
    let (base, captured) = serve(vec![Recorded::sse(&fixture("anthropic_refusal.sse"))]).await;
    let mut req = request();
    req.model = "claude-opus-5".into();
    let resp = provider(base)
        .chat(req, &|_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resp.stop, StopReason::Refusal);
    assert_eq!(resp.refusal_category.as_deref(), Some("cyber"));
    let cap = captured.lock().unwrap();
    assert_eq!(cap[0].body["fallbacks"], "default");
    assert_eq!(
        cap[0].header("anthropic-beta").as_deref(),
        Some("server-side-fallback-2026-07-01")
    );
}

#[tokio::test]
async fn retries_overload_before_output() {
    let (base, captured) = serve(vec![
        Recorded::error(
            529,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ),
        Recorded::error(
            429,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#,
        )
        .with_header("retry-after", "0"),
        Recorded::sse(&fixture("anthropic_text.sse")),
    ])
    .await;
    let mut req = request();
    req.model = "claude-haiku-4-5".into();
    let resp = provider(base)
        .chat(req, &|_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resp.text(), "Hello.");
    assert_eq!(resp.stop, StopReason::EndTurn);
    assert_eq!(captured.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn midstream_error_is_not_retried() {
    let (base, captured) = serve(vec![
        Recorded::sse(&fixture("anthropic_midstream_error.sse")),
        Recorded::sse(&fixture("anthropic_text.sse")),
    ])
    .await;
    let err = provider(base)
        .chat(request(), &|_| {}, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Stream(_)), "{err:?}");
    assert_eq!(captured.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_request_surfaces_message_without_key() {
    let (base, _) = serve(vec![Recorded::error(
        400,
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: too large"}}"#,
    )])
    .await;
    let err = provider(base)
        .chat(request(), &|_| {}, CancellationToken::new())
        .await
        .unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(text.contains("max_tokens: too large"));
    assert!(!text.contains("sk-ant-test-key"));
}

#[tokio::test]
async fn cancellation_stops_at_once() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (_sock, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        c2.cancel();
    });
    let started = std::time::Instant::now();
    let err = provider(base)
        .chat(request(), &|_| {}, cancel)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Cancelled));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[tokio::test]
#[ignore = "live: needs ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN"]
async fn live_smoke() {
    let providers = ostra_providers::Providers::from_config(
        &ostra_core::config::GlobalConfig::default(),
        &Default::default(),
    );
    let Some(p) = providers.get("anthropic") else {
        assert!(
            std::env::var("OSTRA_LIVE_REQUIRE").is_err(),
            "no anthropic provider configured"
        );
        return;
    };
    let model =
        std::env::var("OSTRA_LIVE_MODEL").unwrap_or_else(|_| "claude-haiku-4-5-20251001".into());
    let mut req = ChatRequest::new(&model);
    req.max_tokens = 64;
    req.effort = Effort::Low;
    req.messages = vec![Message::user_text("Reply with the single word: ready")];
    let resp = p
        .chat(req, &|_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        resp.text().to_lowercase().contains("ready"),
        "{}",
        resp.text()
    );
    assert!(resp.usage.output_tokens > 0);
    let schema = json!({"type": "object", "properties": {"color": {"type": "string", "enum": ["red", "blue"]}, "reason": {"type": "string"}}, "required": ["color", "reason"]});
    let (v, _) = ostra_providers::structured(
        p.as_ref(),
        &model,
        "Pick a color.",
        "Pick blue.",
        schema,
        Effort::Low,
    )
    .await
    .unwrap();
    assert_eq!(v["color"], "blue", "{v}");
}
