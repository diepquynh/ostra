//! The hook bridge and the MCP shim, both sides. The `ostra hook` and `ostra mcp-stdio`
//! subprocesses run inside the harness and forward to the server; [`HarnessBridge`] is what the
//! server's `/internal/policy` and `/internal/mcp` handlers call.

use crate::adapters::{self, Decision, PreParse};
use crate::live::{LiveExecution, LiveRegistry, StopVerdict};
use crate::protocol::*;
use crate::services::BridgeServices;
use ostra_core::policy::{PolicyDecision, RuleRef, ToolCall};
use ostra_core::submit::{submit_description, submit_schema, validate_submit};
use ostra_core::{ExecutionId, HarnessKind};
use serde_json::{Value, json};
use std::sync::Arc;

/// MCP protocol versions the shim speaks, oldest first. `2026-07-28` is left out on purpose: its
/// results need `resultType`, `ttlMs`, and `cacheScope`, and Claude Code rejects a `tools/list`
/// without them, so the run gets no Ostra tools. Without it in `server/discover`, clients fall
/// back to the `initialize` handshake.
pub const MCP_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
const MCP_INITIALIZE_DEFAULT: &str = "2025-11-25";

#[derive(Clone)]
pub struct HarnessBridge {
    services: Arc<dyn BridgeServices>,
    live: Arc<LiveRegistry>,
}

fn rule_text(rule: &RuleRef) -> String {
    format!("{}: {}", rule.layer, rule.rule)
}

/// A denial as the model reads it: the correction first, then the rule that fired.
pub fn denial_text(reason: &str, rule: &RuleRef) -> String {
    format!("{} (Ostra {})", reason.trim_end(), rule_text(rule))
}

impl HarnessBridge {
    pub fn new(services: Arc<dyn BridgeServices>, live: Arc<LiveRegistry>) -> Self {
        HarnessBridge { services, live }
    }

    pub fn live(&self) -> &Arc<LiveRegistry> {
        &self.live
    }

    fn authorize(&self, execution: &ExecutionId, token: &str) -> Option<Arc<LiveExecution>> {
        let live = self.live.authorize(execution, token)?;
        self.services.authorize(execution, token).then_some(live)
    }

    /// Handle one hook event. Never fails open: anything unreadable on a PreToolUse denies.
    pub async fn handle_policy(&self, token: &str, req: PolicyRequest) -> PolicyResponse {
        let adapter = adapters::for_harness(req.harness);
        let deny = |reason: &str| match req.event {
            HookEvent::PreToolUse => PolicyResponse::json(adapter.pre_response(&Decision::Deny {
                reason: reason.to_string(),
            })),
            _ => PolicyResponse::json(json!({})),
        };
        let Some(live) = self.authorize(&req.execution, token) else {
            return deny(
                "This execution is not running under Ostra any more, so no tool call may run. Stop now.",
            );
        };
        if live.harness != req.harness {
            return deny("The hook's harness does not match the execution's harness. Stop now.");
        }
        let meta = adapter.meta(&req.payload);
        live.note_session(meta.session_id.clone(), meta.transcript_path.clone());

        match req.event {
            HookEvent::PreToolUse => {
                let decision = match adapter.parse_pre(&req.payload) {
                    PreParse::Refuse(reason) => Decision::Deny { reason },
                    PreParse::Call { call, .. } => {
                        live.note_tool_call();
                        self.decide(&live, &call).await
                    }
                };
                PolicyResponse::json(adapter.pre_response(&decision))
            }
            HookEvent::PostToolUse | HookEvent::PostToolUseFailure => {
                live.touch();
                let notes = match adapter.parse_post(req.event, &req.payload) {
                    Some(post) => {
                        self.services
                            .policy_observe(&req.execution, &post.call, &post.outcome)
                    }
                    None => vec![],
                };
                PolicyResponse::json(adapter.post_response(&notes))
            }
            HookEvent::Stop => {
                self.services
                    .stop_event(&req.execution, req.payload.clone());
                let verdict = live.on_stop(meta.last_message.clone());
                let block = match &verdict {
                    StopVerdict::Block(r) => Some(r.as_str()),
                    StopVerdict::Allow => None,
                };
                PolicyResponse::json(adapter.stop_response(block))
            }
            HookEvent::SessionStart | HookEvent::PreInvocation | HookEvent::PostInvocation => {
                live.touch();
                PolicyResponse::json(json!({}))
            }
        }
    }

    async fn decide(&self, live: &LiveExecution, call: &ToolCall) -> Decision {
        if let Some(rest) = call.tool.strip_prefix("submit_")
            && rest != live.agent.snake()
        {
            return Decision::Deny {
                reason: format!(
                    "Call `{}` instead: that is this agent's submit tool.",
                    live.agent.submit_tool_name()
                ),
            };
        }
        match self.services.policy_check(&live.id, call) {
            PolicyDecision::Allow { .. } => Decision::Allow { note: None },
            PolicyDecision::Deny { reason, rule } => Decision::Deny {
                reason: denial_text(&reason, &rule),
            },
            PolicyDecision::Ask { reason, rule } => {
                let answer = self
                    .services
                    .resolve_ask(&live.id, call, &reason, &rule)
                    .await;
                if answer.allows() {
                    Decision::Allow { note: None }
                } else {
                    Decision::Deny {
                        reason: format!(
                            "The user denied this call. Continue without it, or choose another way. ({})",
                            rule_text(&rule)
                        ),
                    }
                }
            }
        }
    }

    /// Handle one JSON-RPC message from the MCP shim. `None` for notifications.
    pub async fn handle_mcp(&self, token: &str, req: McpRequest) -> McpResponse {
        let msg = req.message;
        let id = msg.get("id").cloned();
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let Some(id) = id.filter(|_| !method.is_empty()) else {
            return McpResponse { message: None };
        };
        let Some(live) = self.authorize(&req.execution, token) else {
            return McpResponse {
                message: Some(rpc_error(
                    id,
                    -32001,
                    "execution is not running under Ostra",
                )),
            };
        };
        live.touch();
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let result = match method.as_str() {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = if MCP_VERSIONS.contains(&asked) {
                    asked
                } else {
                    MCP_INITIALIZE_DEFAULT
                };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": MCP_SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
                }))
            }
            "server/discover" => Ok(json!({
                "supportedVersions": MCP_VERSIONS,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": MCP_SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": self.tool_list(&live)})),
            "tools/call" => Ok(self.call_tool(&live, &params).await),
            "resources/list" => Ok(json!({"resources": []})),
            "prompts/list" => Ok(json!({"prompts": []})),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        let message = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => rpc_error(id, code, &message),
        };
        McpResponse {
            message: Some(message),
        }
    }

    fn tool_list(&self, live: &LiveExecution) -> Vec<Value> {
        let mut tools = vec![json!({
            "name": live.agent.submit_tool_name(),
            "description": submit_description(live.agent),
            "inputSchema": object_schema(submit_schema(live.agent)),
        })];
        for (name, description, schema) in self.services.mcp_tools(&live.id) {
            tools.push(json!({"name": name, "description": description, "inputSchema": object_schema(schema)}));
        }
        tools
    }

    async fn call_tool(&self, live: &LiveExecution, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let mut args = params.get("arguments").cloned().unwrap_or(json!({}));
        if name == live.agent.submit_tool_name() {
            ostra_core::args::coerce_json_strings(&mut args, &submit_schema(live.agent));
            if let Err(message) = validate_submit(live.agent, &args) {
                return tool_error(&format!(
                    "Fix the arguments and call `{name}` again: {message}. Nothing was recorded."
                ));
            }
            if let Err(message) = ostra_core::doc::check_submit(live.agent, &args) {
                return tool_error(&format!("{message} Nothing was recorded."));
            }
            return if live.record_submit(args) {
                tool_text(
                    "Recorded. Your run is complete: end your turn now, without further tool calls.",
                )
            } else {
                tool_error("A result was already recorded for this run. End your turn now.")
            };
        }
        if name.starts_with("submit_") {
            return tool_error(&format!(
                "Call `{}` instead: that is this agent's submit tool.",
                live.agent.submit_tool_name()
            ));
        }
        let canonical = adapters::canonical_ostra_tool(name);
        let call = ToolCall::new(canonical, args.clone());
        live.note_tool_call();
        if let Decision::Deny { reason } = self.decide(live, &call).await {
            return tool_error(&reason);
        }
        match self.services.mcp_call(&live.id, name, args).await {
            Ok(text) => tool_text(&text),
            Err(text) => tool_error(&text),
        }
    }
}

fn object_schema(mut schema: Value) -> Value {
    if let Some(m) = schema.as_object_mut() {
        m.remove("$schema");
        m.entry("type").or_insert(json!("object"));
    }
    schema
}

fn tool_text(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": false})
}

fn tool_error(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": true})
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

// ---------------------------------------------------------------------------------------------
// Subprocess side
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct HookArgs {
    pub execution: Option<ExecutionId>,
    pub harness: HarnessKind,
    pub event: HookEvent,
}

/// Parse `--execution <id> --harness <name> --event <event>` (the arguments after `ostra hook`).
/// The execution falls back to `$OSTRA_EXECUTION`, the harness to `$OSTRA_HARNESS`.
pub fn parse_hook_args(args: &[String]) -> Result<HookArgs, String> {
    let (mut execution, mut harness, mut event) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--execution" => execution = Some(value()?),
            "--harness" => harness = Some(value()?),
            "--event" => event = Some(value()?),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let execution = execution
        .or_else(|| std::env::var(ENV_EXECUTION).ok())
        .filter(|s| !s.is_empty());
    let harness = harness
        .or_else(|| std::env::var(ENV_HARNESS).ok())
        .ok_or("--harness is required")?
        .parse::<HarnessKind>()?;
    let event = event
        .as_deref()
        .and_then(HookEvent::parse)
        .ok_or("--event is required")?;
    Ok(HookArgs {
        execution: execution.map(ExecutionId::from),
        harness,
        event,
    })
}

/// `ostra hook`: read the payload on stdin, ask the server, print its answer. Outside an Ostra
/// execution (no `$OSTRA_EXECUTION`) it is a no-op, because the Antigravity integration is
/// installed globally. Inside one, an unreachable server denies.
pub async fn run_hook_cli(args: &[String]) -> i32 {
    let parsed = match parse_hook_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ostra hook: {e}");
            return 1;
        }
    };
    let mut input = String::new();
    let _ = tokio::io::AsyncReadExt::read_to_string(&mut tokio::io::stdin(), &mut input).await;
    let Some(execution) = parsed.execution.clone() else {
        return 0;
    };
    let adapter = adapters::for_harness(parsed.harness);
    let fail_closed = |why: &str| -> i32 {
        if parsed.event == HookEvent::PreToolUse {
            let reason =
                format!("Stop and end your turn: Ostra could not check this call ({why}).");
            println!("{}", adapter.pre_response(&Decision::Deny { reason }));
        }
        0
    };
    let (Ok(url), Ok(token)) = (std::env::var(ENV_URL), std::env::var(ENV_TOKEN)) else {
        return fail_closed("OSTRA_URL or OSTRA_TOKEN is missing");
    };
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::String(input));
    let req = PolicyRequest {
        execution,
        harness: parsed.harness,
        event: parsed.event,
        payload,
    };
    let mut builder = reqwest::Client::builder();
    if parsed.event != HookEvent::PreToolUse {
        builder = builder.timeout(std::time::Duration::from_secs(60));
    }
    let Ok(client) = builder.build() else {
        return fail_closed("no HTTP client");
    };
    let resp = client
        .post(format!("{}{POLICY_PATH}", url.trim_end_matches('/')))
        .bearer_auth(token)
        .json(&req)
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => match r.json::<PolicyResponse>().await {
            Ok(p) => {
                if !p.stdout.is_empty() {
                    println!("{}", p.stdout);
                }
                p.exit_code
            }
            Err(e) => fail_closed(&format!("unreadable server answer: {e}")),
        },
        Ok(r) => fail_closed(&format!("server answered {}", r.status())),
        Err(e) => fail_closed(&format!("server unreachable: {e}")),
    }
}

/// `ostra mcp-stdio`: a stdio MCP server that forwards every message to the server. Outside an
/// Ostra execution it answers the handshake with no tools, so a global registration is inert.
pub async fn run_mcp_stdio(args: &[String]) -> i32 {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut execution = std::env::var(ENV_EXECUTION).ok().filter(|s| !s.is_empty());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--execution" {
            execution = it.next().cloned();
        }
    }
    let url = std::env::var(ENV_URL).ok();
    let token = std::env::var(ENV_TOKEN).ok();
    let target = match (execution, url, token) {
        (Some(e), Some(u), Some(t)) => Some((ExecutionId::from(e), u, t)),
        _ => None,
    };
    let client = reqwest::Client::new();
    let stdout = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut tasks = tokio::task::JoinSet::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            let err = rpc_error(Value::Null, -32700, "parse error");
            let mut out = stdout.lock().await;
            let _ = out.write_all(format!("{err}\n").as_bytes()).await;
            let _ = out.flush().await;
            continue;
        };
        let messages = match message {
            Value::Array(items) => items,
            single => vec![single],
        };
        for message in messages {
            let client = client.clone();
            let stdout = stdout.clone();
            let target = target.clone();
            tasks.spawn(async move {
                let reply = match &target {
                    Some((execution, url, token)) => {
                        forward_mcp(&client, url, token, execution.clone(), message).await
                    }
                    None => offline_mcp(&message),
                };
                if let Some(reply) = reply {
                    let mut out = stdout.lock().await;
                    let _ = out.write_all(format!("{reply}\n").as_bytes()).await;
                    let _ = out.flush().await;
                }
            });
        }
    }
    while tasks.join_next().await.is_some() {}
    0
}

async fn forward_mcp(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    execution: ExecutionId,
    message: Value,
) -> Option<Value> {
    let id = message.get("id").cloned();
    let req = McpRequest { execution, message };
    let resp = client
        .post(format!("{}{MCP_PATH}", url.trim_end_matches('/')))
        .bearer_auth(token)
        .json(&req)
        .send()
        .await;
    let failure = |why: String| {
        id.clone()
            .map(|id| rpc_error(id, -32603, &format!("Ostra server error: {why}")))
    };
    match resp {
        Ok(r) if r.status().is_success() => match r.json::<McpResponse>().await {
            Ok(m) => m.message,
            Err(e) => failure(e.to_string()),
        },
        Ok(r) => failure(format!("status {}", r.status())),
        Err(e) => failure(e.to_string()),
    }
}

fn offline_mcp(message: &Value) -> Option<Value> {
    let id = message.get("id").cloned()?;
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => json!({
            "protocolVersion": message.pointer("/params/protocolVersion").and_then(Value::as_str)
                .filter(|v| MCP_VERSIONS.contains(v)).unwrap_or(MCP_INITIALIZE_DEFAULT),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": MCP_SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
        }),
        "server/discover" => {
            json!({"supportedVersions": MCP_VERSIONS, "capabilities": {"tools": {"listChanged": false}}})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": []}),
        "tools/call" => tool_error("Ostra tools work only inside an Ostra execution."),
        _ => return Some(rpc_error(id, -32601, "method not found")),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostra_core::AgentName;
    use ostra_core::policy::{PermissionAnswer, ToolOutcome};
    use parking_lot::Mutex;

    #[derive(Default)]
    struct Fake {
        asks: Mutex<Vec<String>>,
        observed: Mutex<Vec<ToolOutcome>>,
    }

    #[async_trait::async_trait]
    impl BridgeServices for Fake {
        fn authorize(&self, _: &ExecutionId, _: &str) -> bool {
            true
        }
        fn policy_check(&self, _: &ExecutionId, call: &ToolCall) -> PolicyDecision {
            match call.tool.as_str() {
                "Write"
                    if call
                        .str_field("file_path")
                        .is_some_and(|p| p.contains("tests/")) =>
                {
                    PolicyDecision::deny(
                        RuleRef::guard("no-tests-from-implementer"),
                        "Leave test files to write-test.",
                    )
                }
                "Bash" if call.str_field("command") == Some("git push") => PolicyDecision::ask(
                    RuleRef::permission("Bash(git push *)"),
                    "Push to the remote?",
                ),
                _ => PolicyDecision::allow(),
            }
        }
        async fn resolve_ask(
            &self,
            _: &ExecutionId,
            _: &ToolCall,
            reason: &str,
            _: &RuleRef,
        ) -> PermissionAnswer {
            self.asks.lock().push(reason.into());
            PermissionAnswer::Deny
        }
        fn policy_observe(
            &self,
            _: &ExecutionId,
            _: &ToolCall,
            outcome: &ToolOutcome,
        ) -> Vec<String> {
            self.observed.lock().push(outcome.clone());
            if outcome.is_error {
                vec!["Recalled lesson: run cargo clean.".into()]
            } else {
                vec![]
            }
        }
        async fn mcp_call(&self, _: &ExecutionId, tool: &str, _: Value) -> Result<String, String> {
            Ok(format!("ran {tool}"))
        }
        fn mcp_tools(&self, _: &ExecutionId) -> Vec<(String, String, Value)> {
            vec![(
                "report".into(),
                "Write the report".into(),
                json!({"type": "object", "properties": {}}),
            )]
        }
        fn stop_event(&self, _: &ExecutionId, _: Value) {}
    }

    fn setup() -> (HarnessBridge, Arc<Fake>, Arc<LiveExecution>) {
        let fake = Arc::new(Fake::default());
        let live = LiveRegistry::new();
        let exec = live.register(
            ExecutionId::new(),
            AgentName::Implementer,
            HarnessKind::Claude,
        );
        (HarnessBridge::new(fake.clone(), live), fake, exec)
    }

    fn pre(exec: &LiveExecution, tool: &str, input: Value) -> PolicyRequest {
        PolicyRequest {
            execution: exec.id.clone(),
            harness: HarnessKind::Claude,
            event: HookEvent::PreToolUse,
            payload: json!({"session_id": "sid-1", "cwd": "/repo", "tool_name": tool, "tool_input": input}),
        }
    }

    fn decision_of(resp: &PolicyResponse) -> String {
        let v: Value = serde_json::from_str(&resp.stdout).unwrap();
        v["hookSpecificOutput"]["permissionDecision"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    #[tokio::test]
    async fn pre_tool_use_decisions() {
        let (bridge, fake, exec) = setup();
        let token = exec.token().to_string();
        let r = bridge
            .handle_policy(&token, pre(&exec, "Read", json!({"file_path": "/repo/a"})))
            .await;
        assert_eq!(decision_of(&r), "allow");
        assert_eq!(exec.snapshot().session_id.as_deref(), Some("sid-1"));

        let r = bridge
            .handle_policy(
                &token,
                pre(&exec, "Write", json!({"file_path": "/repo/tests/a.rs"})),
            )
            .await;
        assert_eq!(decision_of(&r), "deny");
        assert!(r.stdout.contains("no-tests-from-implementer"));

        let r = bridge
            .handle_policy(&token, pre(&exec, "Bash", json!({"command": "git push"})))
            .await;
        assert_eq!(decision_of(&r), "deny");
        assert_eq!(fake.asks.lock().len(), 1);

        let r = bridge
            .handle_policy(&token, pre(&exec, "Agent", json!({})))
            .await;
        assert_eq!(decision_of(&r), "deny");

        let r = bridge
            .handle_policy(&token, pre(&exec, "mcp__ostra__submit_plan", json!({})))
            .await;
        assert_eq!(decision_of(&r), "deny", "another agent's submit tool");

        let r = bridge
            .handle_policy("wrong", pre(&exec, "Read", json!({})))
            .await;
        assert_eq!(decision_of(&r), "deny");
    }

    #[tokio::test]
    async fn post_and_stop() {
        let (bridge, fake, exec) = setup();
        let token = exec.token().to_string();
        let mut req = pre(&exec, "Bash", json!({"command": "cargo build"}));
        req.event = HookEvent::PostToolUseFailure;
        req.payload["error"] = json!("Exit code 1\nerror[E0599]");
        let r = bridge.handle_policy(&token, req).await;
        assert!(r.stdout.contains("Recalled lesson"));
        assert_eq!(fake.observed.lock()[0].exit_code, Some(1));

        let stop = PolicyRequest {
            execution: exec.id.clone(),
            harness: HarnessKind::Claude,
            event: HookEvent::Stop,
            payload: json!({"session_id": "sid-1", "stop_hook_active": false}),
        };
        let r = bridge.handle_policy(&token, stop.clone()).await;
        assert!(r.stdout.contains("\"block\"") && r.stdout.contains("submit_implementer"));
    }

    #[tokio::test]
    async fn mcp_submit_flow() {
        let (bridge, _fake, exec) = setup();
        let token = exec.token().to_string();
        let call = |id: i64, method: &str, params: Value| McpRequest {
            execution: exec.id.clone(),
            message: json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        };
        let init = bridge
            .handle_mcp(
                &token,
                call(1, "initialize", json!({"protocolVersion": "2025-06-18"})),
            )
            .await;
        assert_eq!(
            init.message.unwrap()["result"]["protocolVersion"],
            "2025-06-18"
        );
        let newest = bridge
            .handle_mcp(
                &token,
                call(1, "initialize", json!({"protocolVersion": "2026-07-28"})),
            )
            .await;
        assert_eq!(
            newest.message.unwrap()["result"]["protocolVersion"],
            MCP_INITIALIZE_DEFAULT
        );
        // Offering 2026-07-28 makes Claude Code negotiate it and then reject every result
        // without `resultType`; see MCP_VERSIONS.
        let discover = bridge
            .handle_mcp(&token, call(1, "server/discover", json!({})))
            .await
            .message
            .unwrap();
        let offered = discover["result"]["supportedVersions"].as_array().unwrap();
        assert!(!offered.contains(&json!("2026-07-28")), "{offered:?}");

        let note = McpRequest {
            execution: exec.id.clone(),
            message: json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        };
        assert!(bridge.handle_mcp(&token, note).await.message.is_none());

        let list = bridge
            .handle_mcp(&token, call(2, "tools/list", json!({})))
            .await
            .message
            .unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["submit_implementer", "report"]);
        assert_eq!(list["result"]["tools"][0]["inputSchema"]["type"], "object");

        let bad = bridge
            .handle_mcp(
                &token,
                call(
                    3,
                    "tools/call",
                    json!({"name": "submit_implementer", "arguments": {"status": "ok"}}),
                ),
            )
            .await
            .message
            .unwrap();
        assert_eq!(bad["result"]["isError"], true);
        assert!(!exec.has_submit());

        let good_args = json!({"status": "ok", "report_path": "/s/r.md", "changed_files": ["a.rs"], "summary": "done"});
        let good = bridge
            .handle_mcp(
                &token,
                call(
                    4,
                    "tools/call",
                    json!({"name": "submit_implementer", "arguments": good_args}),
                ),
            )
            .await
            .message
            .unwrap();
        assert_eq!(good["result"]["isError"], false);
        assert!(exec.has_submit());

        let report = bridge
            .handle_mcp(
                &token,
                call(
                    5,
                    "tools/call",
                    json!({"name": "report", "arguments": {"content": "x"}}),
                ),
            )
            .await
            .message
            .unwrap();
        assert_eq!(report["result"]["content"][0]["text"], "ran report");

        let unauth = bridge
            .handle_mcp("nope", call(6, "tools/list", json!({})))
            .await
            .message
            .unwrap();
        assert_eq!(unauth["error"]["code"], -32001);
    }

    #[tokio::test]
    async fn mcp_submit_needs_the_document() {
        let fake = Arc::new(Fake::default());
        let live = LiveRegistry::new();
        let exec = live.register(ExecutionId::new(), AgentName::Explore, HarnessKind::Claude);
        let bridge = HarnessBridge::new(fake, live);
        let args = json!({"research_path": "/nowhere/ostra-research-1.md", "scope_covered": "s", "findings_summary": "f", "sources_retrieved": 0, "open_questions": 0});
        let r = bridge
            .handle_mcp(
                exec.token(),
                McpRequest {
                    execution: exec.id.clone(),
                    message: json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "submit_explore", "arguments": args}}),
                },
            )
            .await
            .message
            .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Document tool"),
            "{r}"
        );
        assert!(!exec.has_submit());
    }

    #[test]
    fn hook_args() {
        let args: Vec<String> = [
            "--execution",
            "x_1",
            "--harness",
            "codex",
            "--event",
            "pre_tool_use",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let a = parse_hook_args(&args).unwrap();
        assert_eq!(a.execution, Some(ExecutionId::from("x_1")));
        assert_eq!(
            (a.harness, a.event),
            (HarnessKind::Codex, HookEvent::PreToolUse)
        );
        assert!(parse_hook_args(&["--bogus".to_string()]).is_err());
    }

    #[test]
    fn offline_shim_is_inert() {
        let r = offline_mcp(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap();
        assert_eq!(r["result"]["tools"], json!([]));
        let d =
            offline_mcp(&json!({"jsonrpc": "2.0", "id": 2, "method": "server/discover"})).unwrap();
        assert!(
            !d["result"]["supportedVersions"]
                .as_array()
                .unwrap()
                .contains(&json!("2026-07-28"))
        );
        assert!(
            offline_mcp(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .is_none()
        );
    }
}
