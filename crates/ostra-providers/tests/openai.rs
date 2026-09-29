mod common;

use common::{Recorded, fixture, serve};
use ostra_core::Effort;
use ostra_providers::openai::OpenAi;
use ostra_providers::{
    ApiKey, Block, CancellationToken, ChatRequest, Message, Provider, ProviderError, RetryPolicy,
    StopReason, StreamEvent, ToolDef,
};
use parking_lot::Mutex;
use serde_json::json;

fn provider(base: String) -> OpenAi {
    ostra_core::pricing::install_test_prices();
    OpenAi::new(ApiKey::new("sk-openai-test"), Some(base)).with_retry(RetryPolicy::fast(2))
}

fn request(model: &str) -> ChatRequest {
    let mut r = ChatRequest::new(model);
    r.messages = vec![Message::user_text("Read a.rs")];
    r.tools = vec![ToolDef {
        name: "Read".into(),
        description: "Read a file".into(),
        input_schema: json!({"type": "object"}),
        cache: false,
    }];
    r
}

#[tokio::test]
async fn reasoning_and_function_call() {
    let (base, captured) = serve(vec![Recorded::sse(&fixture("openai_function_call.sse"))]).await;
    let events = Mutex::new(vec![]);
    let sink = |e: StreamEvent| events.lock().push(e);
    let resp = provider(base)
        .chat(request("gpt-5.6-terra"), &sink, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resp.stop, StopReason::ToolUse);
    assert!(matches!(&resp.content[0], Block::Opaque { provider, value }
        if provider == "openai" && value["encrypted_content"] == "gAAAA-final"));
    assert_eq!(
        resp.tool_uses()[0],
        ("call_abc", "Read", &json!({"file_path": "/repo/a.rs"}))
    );
    assert_eq!(resp.usage.cache_read_tokens, 800);
    assert_eq!(resp.usage.cache_write_tokens, 100);
    assert_eq!(resp.usage.input_tokens, 100);
    assert_eq!(resp.usage.output_tokens, 60);
    let expected = (100.0 * 2.0 + 800.0 * 0.2 + 100.0 * 2.5 + 60.0 * 12.0) / 1e6;
    assert!((resp.usage.cost_usd - expected).abs() < 1e-12);

    {
        let events = events.lock();
        assert!(events.contains(&StreamEvent::ThinkingDelta(
            "Look at the file first.".into()
        )));
        assert!(events.contains(&StreamEvent::ToolUseStarted {
            id: "call_abc".into(),
            name: "Read".into()
        }));
        assert!(events.contains(&StreamEvent::ToolInputDelta {
            id: "call_abc".into(),
            partial_json: "{\"file_path\":".into()
        }));
        let cap = captured.lock().unwrap();
        assert!(cap[0].head.starts_with("POST /v1/responses"));
        assert_eq!(
            cap[0].header("authorization").as_deref(),
            Some("Bearer sk-openai-test")
        );
        assert_eq!(cap[0].body["store"], false);
    }

    // The reasoning item round-trips into the next request.
    let mut next = request("gpt-5.6-terra");
    next.messages.push(Message::assistant(resp.content.clone()));
    next.messages
        .push(Message::tool_results(vec![Block::tool_result(
            "call_abc",
            "fn main() {}",
            false,
        )]));
    let (base2, cap2) = serve(vec![Recorded::sse(&fixture("openai_text.sse"))]).await;
    provider(base2)
        .chat(next, &|_| {}, CancellationToken::new())
        .await
        .unwrap();
    let input = cap2.lock().unwrap()[0].body["input"].clone();
    assert_eq!(input[1]["type"], "reasoning");
    assert_eq!(input[1]["encrypted_content"], "gAAAA-final");
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(
        input[3],
        json!({"type": "function_call_output", "call_id": "call_abc", "output": "fn main() {}"})
    );
}

#[tokio::test]
async fn text_with_web_search() {
    let (base, _) = serve(vec![Recorded::sse(&fixture("openai_text.sse"))]).await;
    let events = Mutex::new(vec![]);
    let sink = |e: StreamEvent| events.lock().push(e);
    let mut req = request("gpt-5.6-luna");
    req.server_tools.web_search = true;
    req.effort = Effort::Low;
    let resp = provider(base)
        .chat(req, &sink, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resp.stop, StopReason::EndTurn);
    assert_eq!(resp.text(), "Use select!.");
    assert_eq!(resp.content.len(), 1);
    assert!(events.lock().contains(&StreamEvent::ServerToolUsed {
        name: "web_search".into(),
        summary: "tokio select".into()
    }));
    assert!((resp.usage.cost_usd - ((200.0 * 0.2 + 10.0 * 1.2) / 1e6 + 0.01)).abs() < 1e-12);
}

#[tokio::test]
async fn incomplete_is_max_tokens() {
    let (base, _) = serve(vec![Recorded::sse(&fixture("openai_incomplete.sse"))]).await;
    let resp = provider(base)
        .chat(request("gpt-5.6-sol"), &|_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resp.stop, StopReason::MaxTokens);
    assert_eq!(resp.text(), "Cut off");
}

#[tokio::test]
async fn retries_server_errors_and_maps_auth() {
    let (base, captured) = serve(vec![
        Recorded::error(503, r#"{"error":{"message":"busy","type":"server_error"}}"#),
        Recorded::sse(&fixture("openai_text.sse")),
    ])
    .await;
    let resp = provider(base)
        .chat(request("gpt-5.6-luna"), &|_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(resp.text(), "Use select!.");
    assert_eq!(captured.lock().unwrap().len(), 2);

    let (base, _) = serve(vec![Recorded::error(
        401,
        r#"{"error":{"message":"Incorrect API key provided"}}"#,
    )])
    .await;
    let err = provider(base)
        .chat(request("gpt-5.6-luna"), &|_| {}, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Auth { status: 401, .. }));
    assert!(!format!("{err:?}").contains("sk-openai-test"));
}

#[tokio::test]
#[ignore = "live: needs OPENAI_API_KEY"]
async fn live_smoke() {
    let Ok(key) = std::env::var("OPENAI_API_KEY") else {
        return;
    };
    let p = OpenAi::new(ApiKey::new(key), None);
    let mut req = ChatRequest::new("gpt-5.6-luna");
    req.max_tokens = 256;
    req.effort = Effort::Low;
    req.messages = vec![Message::user_text("Reply with the single word: ready")];
    let resp = p
        .chat(req, &|_| {}, CancellationToken::new())
        .await
        .unwrap();
    assert!(resp.text().to_lowercase().contains("ready"));
}

#[tokio::test]
async fn compacts_through_the_compact_endpoint() {
    let window = json!({
        "id": "resp_1",
        "object": "response.compaction",
        "output": [
            {"id": "msg_0", "type": "message", "role": "user",
             "content": [{"type": "input_text", "text": "Read a.rs"}]},
            {"id": "cmp_1", "type": "compaction", "encrypted_content": "gAAAA-cmp"},
        ],
        "usage": {"input_tokens": 1000, "input_tokens_details": {"cached_tokens": 800},
                  "output_tokens": 200},
    });
    let (base, captured) = serve(vec![
        Recorded::json(&window),
        Recorded::sse(&fixture("openai_text.sse")),
    ])
    .await;
    let p = provider(base);
    let mut req = request("gpt-5.6-terra");
    req.system = vec![ostra_providers::SystemBlock::new("be brief")];
    let out = p
        .compact(req, "ignored", CancellationToken::new())
        .await
        .unwrap();
    let messages = out.messages.expect("a compacted window");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content[0], Block::text("Read a.rs"));
    let item = json!({"type": "compaction", "id": "cmp_1", "encrypted_content": "gAAAA-cmp"});
    assert!(matches!(&messages[0].content[1], Block::Compaction { provider, value: Some(v), .. }
        if provider == "openai" && *v == item));
    assert_eq!((out.usage.input_tokens, out.usage.cache_read_tokens), (200, 800));

    // The item goes back verbatim on the next request.
    let mut next = request("gpt-5.6-terra");
    next.messages = messages;
    next.messages.push(Message::user_text("go on"));
    p.chat(next, &|_| {}, CancellationToken::new()).await.unwrap();
    let cap = captured.lock().unwrap();
    assert!(cap[0].head.starts_with("POST /v1/responses/compact"));
    assert_eq!(cap[0].body["instructions"], "be brief");
    assert!(cap[0].body.get("tools").is_none());
    assert_eq!(cap[1].body["input"][1], item);
}

#[tokio::test]
async fn compacts_on_the_client_when_the_endpoint_is_missing() {
    let (base, captured) = serve(vec![
        Recorded::error(404, r#"{"error":{"message":"not found"}}"#),
        Recorded::sse(&fixture("openai_text.sse")),
    ])
    .await;
    let out = provider(base)
        .compact(request("gpt-5.6-terra"), "Summarize.", CancellationToken::new())
        .await
        .unwrap();
    let messages = out.messages.expect("a client summary");
    assert!(matches!(&messages[0].content[0], Block::Compaction { value: None, summary, .. }
        if !summary.is_empty()));
    let cap = captured.lock().unwrap();
    assert!(cap[1].head.starts_with("POST /v1/responses "));
}
