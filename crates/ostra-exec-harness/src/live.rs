//! Running harness executions, addressed by execution id. The bridge records what arrives through
//! hooks and the MCP shim here, and the executor waits on it.

use ostra_core::exec::ExecutionHost;
use ostra_core::{AgentName, ExecutionId, HarnessKind};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// A read-only reopened session: no submit is expected, so a Stop is never turned back.
    inspect: AtomicBool,
    /// Rule SM6: the messages the run was handed came from senders that wait for its reply.
    owes_reply: AtomicBool,
    /// Rule CA3: the submit schema the run was given.
    submit_schema: Mutex<Option<serde_json::Value>>,
    /// Rule CA5: the result contract the run submits.
    contract: Mutex<ostra_core::Contract>,
    /// The report file an `ok` submit needs written first, when the contract requires one.
    report_file: Mutex<Option<PathBuf>>,
    /// The engine's side of the run, for its queued messages and its reply duty (Rules SM2, SM6).
    host: Mutex<Option<HostRef>>,
}

#[derive(Clone)]
struct HostRef(Arc<dyn ExecutionHost>);

impl std::fmt::Debug for HostRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExecutionHost")
    }
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
    /// Rule H2: the run asked another subagent and waits for Ostra to type the answer in.
    pub waiting: bool,
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

    /// Rule CA3: the schema the run's submit tool takes, from its `ExecutionSpec`.
    pub fn set_submit_schema(&self, schema: serde_json::Value) {
        if !schema.is_null() {
            *self.submit_schema.lock() = Some(schema);
        }
    }

    pub fn submit_schema(&self) -> serde_json::Value {
        self.submit_schema
            .lock()
            .clone()
            .unwrap_or_else(|| ostra_core::submit::submit_schema(self.contract()))
    }

    pub fn set_contract(&self, contract: ostra_core::Contract) {
        *self.contract.lock() = contract;
    }

    pub fn contract(&self) -> ostra_core::Contract {
        *self.contract.lock()
    }

    pub fn set_report_file(&self, path: Option<PathBuf>) {
        *self.report_file.lock() = path;
    }

    /// Rule CA5: the report an `ok` submit of this contract needs, when it is not written yet.
    pub fn missing_report(&self, submit: &serde_json::Value) -> Option<PathBuf> {
        let ok = submit
            .get("status")
            .and_then(|s| s.as_str())
            .is_none_or(|s| s == "ok");
        let path = self.report_file.lock().clone()?;
        (ok && self.contract().report_required() && !path.exists()).then_some(path)
    }

    pub fn set_host(&self, host: Arc<dyn ExecutionHost>) {
        *self.host.lock() = Some(HostRef(host));
    }

    fn host(&self) -> Option<Arc<dyn ExecutionHost>> {
        self.host.lock().clone().map(|h| h.0)
    }

    /// Rule SM6: why the run may not submit yet.
    pub fn submit_blocked(&self) -> Option<String> {
        self.host()?.submit_blocked()
    }

    /// The pipeline's checks of the run's submit, after its shape check.
    pub fn check_submit(&self, input: &serde_json::Value) -> Vec<String> {
        self.host()
            .map(|h| h.check_submit(self.contract(), input))
            .unwrap_or_default()
    }

    pub fn set_inspect(&self) {
        self.inspect.store(true, Ordering::SeqCst);
    }

    pub fn set_owes_reply(&self, owes: bool) {
        self.owes_reply.store(owes, Ordering::SeqCst);
    }

    pub fn owes_reply(&self) -> bool {
        self.owes_reply.load(Ordering::SeqCst)
    }

    /// What a run that tries to end without its result is told: reply first when a sender waits
    /// for it (Rule SM6), else submit.
    pub fn nudge(&self) -> String {
        self.submit_blocked()
            .unwrap_or_else(|| missing_submit_instruction(self.agent))
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

    pub fn set_waiting(&self, waiting: bool) {
        self.state.lock().waiting = waiting;
        self.changed.notify_waiters();
    }

    pub fn has_submit(&self) -> bool {
        self.state.lock().submit.is_some()
    }

    pub fn on_stop(&self, last_message: Option<String>) -> StopVerdict {
        // Asked before the state lock, because the host reads the engine's state.
        let host = self.host();
        let mail = host.as_ref().is_some_and(|h| h.has_messages());
        let nudge = self.nudge();
        let mut s = self.state.lock();
        s.last_activity = Instant::now();
        if last_message
            .as_deref()
            .is_some_and(|m| !m.trim().is_empty())
        {
            s.last_message = last_message;
        }
        let verdict = if self.inspect.load(Ordering::SeqCst) || s.waiting {
            StopVerdict::Allow
        } else if s.submit.is_some() {
            s.stopped_after_submit = true;
            StopVerdict::Allow
        } else if mail {
            // Rule SM2: the turn ended, so the messages queued for the run are typed in now.
            s.waiting = true;
            StopVerdict::Allow
        } else if s.stops_without_submit < MAX_STOP_NUDGES {
            s.stops_without_submit += 1;
            StopVerdict::Block(nudge)
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
                waiting: false,
            }),
            changed: Notify::new(),
            inspect: AtomicBool::new(false),
            owes_reply: AtomicBool::new(false),
            submit_schema: Mutex::new(None),
            contract: Mutex::new(ostra_core::Contract::Stage),
            report_file: Mutex::new(None),
            host: Mutex::new(None),
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

    #[test]
    fn a_waiting_run_ends_its_turn_without_a_submit() {
        let reg = LiveRegistry::new();
        let live = reg.register(
            ExecutionId::new(),
            AgentName::GenerateSpec,
            HarnessKind::Claude,
        );
        live.set_waiting(true);
        assert_eq!(live.on_stop(None), StopVerdict::Allow);
        let s = live.snapshot();
        assert!(s.waiting && !s.gave_up && s.stops_without_submit == 0);
        live.set_waiting(false);
        assert!(matches!(live.on_stop(None), StopVerdict::Block(_)));
    }

    struct Host {
        mail: bool,
        owes: bool,
    }

    #[async_trait::async_trait]
    impl ExecutionHost for Host {
        fn emit(&self, _: ostra_core::exec::ExecutionDelta) {}
        async fn ask_permission(
            &self,
            _: &ostra_core::policy::ToolCall,
            _: &str,
            _: &ostra_core::policy::RuleRef,
        ) -> ostra_core::policy::PermissionAnswer {
            ostra_core::policy::PermissionAnswer::Deny
        }
        fn has_messages(&self) -> bool {
            self.mail
        }
        fn submit_blocked(&self) -> Option<String> {
            self.owes.then(|| {
                ostra_core::coord::reply_instruction("mcp__ostra__send_message", &["x_1".into()])
            })
        }
    }

    #[test]
    fn sm6_a_run_that_owes_a_reply_is_told_to_send_it() {
        let reg = LiveRegistry::new();
        let live = reg.register(
            ExecutionId::new(),
            AgentName::GenerateSpec,
            HarnessKind::Claude,
        );
        live.set_host(Arc::new(Host {
            mail: false,
            owes: true,
        }));
        assert!(
            matches!(live.on_stop(None), StopVerdict::Block(ref m) if m.contains("mcp__ostra__send_message"))
        );
        assert!(live.nudge().contains("`x_1`"));
        assert!(live.submit_blocked().is_some());
        live.set_host(Arc::new(Host {
            mail: false,
            owes: false,
        }));
        assert!(live.nudge().contains("submit_generate_spec"));
    }

    #[test]
    fn sm2_queued_mail_is_typed_in_after_the_turn_ends() {
        let reg = LiveRegistry::new();
        let live = reg.register(
            ExecutionId::new(),
            AgentName::GenerateSpec,
            HarnessKind::Claude,
        );
        live.set_host(Arc::new(Host {
            mail: true,
            owes: false,
        }));
        assert_eq!(live.on_stop(None), StopVerdict::Allow);
        let s = live.snapshot();
        assert!(s.waiting && !s.gave_up, "the supervisor types the mail in");
    }

    #[test]
    fn a_read_only_session_is_never_asked_to_submit() {
        let reg = LiveRegistry::new();
        let live = reg.register(
            ExecutionId::new(),
            AgentName::Implementer,
            HarnessKind::Grok,
        );
        live.set_inspect();
        assert_eq!(live.on_stop(None), StopVerdict::Allow);
        assert!(!live.snapshot().gave_up);
    }
}
