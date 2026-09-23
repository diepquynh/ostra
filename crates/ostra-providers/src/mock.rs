//! A scripted provider for tests: queued responses in order, or a responder function when
//! executions run in parallel and order is not fixed.

use crate::{Block, ChatRequest, ChatResponse, EventSink, Provider, ProviderError, StopReason, StreamEvent};
use ostra_core::exec::Usage;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

type Responder = dyn Fn(&ChatRequest) -> Result<ChatResponse, ProviderError> + Send + Sync;

pub struct ScriptedProvider {
    name: String,
    queue: Mutex<VecDeque<Result<ChatResponse, ProviderError>>>,
    responder: Option<Arc<Responder>>,
    requests: Mutex<Vec<ChatRequest>>,
    next_id: AtomicU64,
}

impl Default for ScriptedProvider {
    fn default() -> Self {
        ScriptedProvider::new()
    }
}

impl ScriptedProvider {
    pub fn new() -> Self {
        ScriptedProvider::named("mock")
    }

    pub fn named(name: impl Into<String>) -> Self {
        ScriptedProvider {
            name: name.into(),
            queue: Mutex::new(VecDeque::new()),
            responder: None,
            requests: Mutex::new(vec![]),
            next_id: AtomicU64::new(1),
        }
    }

    /// Answer every request with `f`, after the queue is empty.
    pub fn with_responder(
        mut self,
        f: impl Fn(&ChatRequest) -> Result<ChatResponse, ProviderError> + Send + Sync + 'static,
    ) -> Self {
        self.responder = Some(Arc::new(f));
        self
    }

    pub fn push(&self, response: ChatResponse) {
        self.queue.lock().push_back(Ok(response));
    }

    pub fn push_error(&self, error: ProviderError) {
        self.queue.lock().push_back(Err(error));
    }

    pub fn push_text(&self, text: &str) {
        self.push(response(vec![Block::text(text)], StopReason::EndTurn));
    }

    pub fn push_tool_use(&self, name: &str, input: serde_json::Value) {
        self.push_tool_uses(vec![(name, input)]);
    }

    pub fn push_tool_uses(&self, calls: Vec<(&str, serde_json::Value)>) {
        let content = calls
            .into_iter()
            .map(|(name, input)| Block::ToolUse { id: self.tool_id(), name: name.into(), input })
            .collect();
        self.push(response(content, StopReason::ToolUse));
    }

    /// A fresh tool-use id, for responses built by hand.
    pub fn tool_id(&self) -> String {
        format!("toolu_mock_{}", self.next_id.fetch_add(1, Ordering::SeqCst))
    }

    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().clone()
    }

    pub fn remaining(&self) -> usize {
        self.queue.lock().len()
    }
}

/// A response with the given content and stop reason and zero usage.
pub fn response(content: Vec<Block>, stop: StopReason) -> ChatResponse {
    ChatResponse { content, stop, usage: Usage::default(), model: "mock".into(), refusal_category: None }
}

/// A tool-use response, for responders.
pub fn tool_use_response(id: &str, name: &str, input: serde_json::Value) -> ChatResponse {
    response(vec![Block::ToolUse { id: id.into(), name: name.into(), input }], StopReason::ToolUse)
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn chat(
        &self,
        req: ChatRequest,
        on_event: EventSink<'_>,
        cancel: CancellationToken,
    ) -> Result<ChatResponse, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        self.requests.lock().push(req.clone());
        let next = self.queue.lock().pop_front();
        let result = match (next, &self.responder) {
            (Some(r), _) => r,
            (None, Some(f)) => f(&req),
            (None, None) => Err(ProviderError::Decode("the scripted provider has no response left".into())),
        }?;
        for block in &result.content {
            match block {
                Block::Text { text } => on_event(StreamEvent::TextDelta(text.clone())),
                Block::Thinking { text, .. } => on_event(StreamEvent::ThinkingDelta(text.clone())),
                Block::ToolUse { id, name, input } => {
                    on_event(StreamEvent::ToolUseStarted { id: id.clone(), name: name.clone() });
                    on_event(StreamEvent::ToolInputDelta { id: id.clone(), partial_json: input.to_string() });
                }
                _ => {}
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queue_then_responder() {
        let p = ScriptedProvider::new().with_responder(|req| {
            Ok(response(vec![Block::text(format!("echo {}", req.model))], StopReason::EndTurn))
        });
        p.push_tool_use("Read", serde_json::json!({"file_path": "/x"}));
        let seen = Mutex::new(vec![]);
        let sink = |e: StreamEvent| seen.lock().push(e);
        let r1 = p.chat(ChatRequest::new("m1"), &sink, CancellationToken::new()).await.unwrap();
        assert_eq!(r1.stop, StopReason::ToolUse);
        let r2 = p.chat(ChatRequest::new("m2"), &sink, CancellationToken::new()).await.unwrap();
        assert_eq!(r2.text(), "echo m2");
        assert_eq!(p.requests().len(), 2);
        assert_eq!(seen.lock().len(), 3);
    }
}
