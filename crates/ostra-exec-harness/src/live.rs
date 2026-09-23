//! Running harness executions, addressed by execution id. The bridge records what arrives through
//! hooks and the MCP shim here, and the executor waits on it.

use ostra_core::{AgentName, ExecutionId, HarnessKind};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Notify;

/// How many times a Stop without a submit call is turned back before the run is allowed to end.
pub const MAX_STOP_NUDGES: u32 = 2;

#[derive(Debug)]
pub struct LiveExecution {
    pub id: ExecutionId,
    pub agent: AgentName,
    pub harness: HarnessKind,
    token: String,
    state: Mutex<LiveState>,
    changed: Notify,
}

#[derive(Debug, Clone)]
pub struct LiveState {
    pub submit: Option<serde_json::Value>,
    pub session_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
    /// Stop events seen while no submit was recorded.
    pub stops_without_submit: u32,
    /// A Stop arrived after the submit, so the turn is over.
    pub stopped_after_submit: bool,
    /// A Stop arrived and was let through without a submit.
    pub gave_up: bool,
    pub last_activity: Instant,
    pub last_message: Option<String>,
    pub tool_calls: u64,
}

/// What the bridge should answer to a Stop event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopVerdict {
    /// Let the turn end.
    Allow,
    /// Turn the stop back with this instruction.
    Block(String),
}

impl LiveExecution {
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn snapshot(&self) -> LiveState {
        self.state.lock().clone()
    }

    pub fn touch(&self) {
        self.state.lock().last_activity = Instant::now();
        self.changed.notify_waiters();
    }

    pub fn note_tool_call(&self) {
        let mut s = self.state.lock();
        s.tool_calls += 1;
        s.last_activity = Instant::now();
    }

    pub fn note_session(&self, session_id: Option<String>, transcript: Option<PathBuf>) {
        let mut s = self.state.lock();
        let mut changed = false;
        if let Some(id) = session_id.filter(|v| !v.is_empty())
            && s.session_id.as_deref() != Some(id.as_str())
        {
            s.session_id = Some(id);
            changed = true;
        }
        if let Some(t) = transcript
            && s.transcript_path.as_ref() != Some(&t)
        {
            s.transcript_path = Some(t);
            changed = true;
        }
        drop(s);
        if changed {
            self.changed.notify_waiters();
        }
    }

    /// Record a validated submit payload. The first one wins; later calls are refused.
    pub fn record_submit(&self, payload: serde_json::Value) -> bool {
        let mut s = self.state.lock();
        if s.submit.is_some() {
            return false;
        }
        s.submit = Some(payload);
        s.last_activity = Instant::now();
        drop(s);
        self.changed.notify_waiters();
        true
    }

    pub fn has_submit(&self) -> bool {
        self.state.lock().submit.is_some()
    }

    pub fn on_stop(&self, last_message: Option<String>) -> StopVerdict {
        let mut s = self.state.lock();
        s.last_activity = Instant::now();
        if last_message
            .as_deref()
            .is_some_and(|m| !m.trim().is_empty())
        {
            s.last_message = last_message;
        }
        let verdict = if s.submit.is_some() {
            s.stopped_after_submit = true;
            StopVerdict::Allow
        } else if s.stops_without_submit < MAX_STOP_NUDGES {
            s.stops_without_submit += 1;
            StopVerdict::Block(missing_submit_instruction(self.agent))
        } else {
            s.gave_up = true;
            StopVerdict::Allow
        };
        drop(s);
        self.changed.notify_waiters();
        verdict
    }

    /// Wait until something changes, or the timeout passes.
    pub async fn changed(&self, timeout: std::time::Duration) {
        let _ = tokio::time::timeout(timeout, self.changed.notified()).await;
    }
}

/// Instruction sent when the model tries to finish without its submit call.
pub fn missing_submit_instruction(agent: AgentName) -> String {
    format!(
        "Call the `{tool}` tool now, with your result, before you stop. Ostra reads only that call, so a run \
         that ends without it is recorded as failed. If a required file is not written yet, write it first.",
        tool = agent.submit_tool_name()
    )
}

#[derive(Debug, Default)]
pub struct LiveRegistry {
    map: Mutex<HashMap<ExecutionId, Arc<LiveExecution>>>,
}

impl LiveRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(LiveRegistry::default())
    }

    /// Register an execution and mint its token. The token dies with [`LiveRegistry::remove`].
    pub fn register(
        &self,
        id: ExecutionId,
        agent: AgentName,
        harness: HarnessKind,
    ) -> Arc<LiveExecution> {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let live = Arc::new(LiveExecution {
            id: id.clone(),
            agent,
            harness,
            token,
            state: Mutex::new(LiveState {
                submit: None,
                session_id: None,
                transcript_path: None,
                stops_without_submit: 0,
                stopped_after_submit: false,
                gave_up: false,
                last_activity: Instant::now(),
                last_message: None,
                tool_calls: 0,
            }),
            changed: Notify::new(),
        });
        self.map.lock().insert(id, live.clone());
        live
    }

    pub fn get(&self, id: &ExecutionId) -> Option<Arc<LiveExecution>> {
        self.map.lock().get(id).cloned()
    }

    /// The execution, if `token` is its current token. Comparison runs in constant time.
    pub fn authorize(&self, id: &ExecutionId, token: &str) -> Option<Arc<LiveExecution>> {
        let live = self.get(id)?;
        constant_time_eq(live.token.as_bytes(), token.as_bytes()).then_some(live)
    }

    pub fn remove(&self, id: &ExecutionId) {
        self.map.lock().remove(id);
    }

    pub fn ids(&self) -> Vec<ExecutionId> {
        self.map.lock().keys().cloned().collect()
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_stops() {
        let reg = LiveRegistry::new();
        let id = ExecutionId::new();
        let live = reg.register(id.clone(), AgentName::Implementer, HarnessKind::Claude);
        let token = live.token().to_string();
        assert!(reg.authorize(&id, &token).is_some());
        assert!(reg.authorize(&id, "nope").is_none());

        assert!(
            matches!(live.on_stop(None), StopVerdict::Block(ref m) if m.contains("submit_implementer"))
        );
        assert!(matches!(live.on_stop(None), StopVerdict::Block(_)));
        assert_eq!(live.on_stop(None), StopVerdict::Allow);
        assert!(live.snapshot().gave_up);

        assert!(live.record_submit(serde_json::json!({"a": 1})));
        assert!(!live.record_submit(serde_json::json!({"a": 2})));
        assert_eq!(live.on_stop(Some("done".into())), StopVerdict::Allow);
        assert!(live.snapshot().stopped_after_submit);

        reg.remove(&id);
        assert!(reg.authorize(&id, &token).is_none());
    }
}
