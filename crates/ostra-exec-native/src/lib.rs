//! Ostra's native agent loop (HANDOVER 10.1): one execution of one leaf agent on an API key.
//! Every tool call passes the policy engine before it runs, and the agent's `submit_<agent>` call
//! ends the run with structured data.

use futures::future::join_all;
use ostra_core::agent::AgentName;
use ostra_core::config::{ProjectProfile, load_toml};
use ostra_core::exec::{
    CancellationToken, ExecContext, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec,
    ExecutionStatus, Executor, Usage,
};
use ostra_core::paths;
use ostra_core::policy::{PermissionAnswer, PolicyDecision, ToolCall, ToolOutcome};
use ostra_policy::{ExecutionPolicy, Observation, PolicyInputs};
use ostra_providers::{
    Block, ChatRequest, ChatResponse, Message, Provider, ProviderError, Providers, Role,
    ServerTools, StopReason, StreamEvent, SystemBlock, ToolChoice, ToolDef,
};
use ostra_store::MemoryStore;
use ostra_tools::{CodeNav, SkillResolver, ToolEnv, ToolEnvConfig};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Upper bound on model turns in one execution, a guard against a loop that never submits.
pub const MAX_TURNS: usize = 400;
/// "Continue" turns allowed after the model hits its output limit without a tool call.
pub const MAX_TOKEN_CONTINUES: usize = 3;
/// Transcript size in characters above which old tool results are cleared. Anthropic clears
/// server-side; for other providers the loop keeps the newest [`KEEP_TOOL_RESULTS`] itself.
/// 300 000 characters is about 75 000 tokens, well below every supported context window.
pub const CLEAR_THRESHOLD_CHARS: usize = 300_000;
pub const KEEP_TOOL_RESULTS: usize = 12;
/// Emitted Activity output is truncated to this size; the model gets the full output.
pub const EMIT_LIMIT: usize = 8 * 1024;
/// How long a cancelled or timed-out run may take to kill its tools before returning.
const CLEANUP_GRACE: Duration = Duration::from_secs(3);

pub struct NativeExecutor {
    providers: Arc<Providers>,
    skill_resolver: SkillResolver,
    code: Option<Arc<dyn CodeNav>>,
}

impl NativeExecutor {
    pub fn new(
        providers: Arc<Providers>,
        skill_resolver: SkillResolver,
        code: Option<Arc<dyn CodeNav>>,
    ) -> Self {
        NativeExecutor {
            providers,
            skill_resolver,
            code,
        }
    }
}

#[async_trait::async_trait]
impl Executor for NativeExecutor {
    async fn run(
        &self,
        spec: ExecutionSpec,
        host: Arc<dyn ExecutionHost>,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        let usage = Arc::new(Mutex::new(Usage::default()));
        let inner = cancel.child_token();
        let budget = Duration::from_secs(spec.timeout_secs.max(1));
        let timeout_secs = spec.timeout_secs.max(1);
        let run = Run {
            providers: self.providers.clone(),
            skill_resolver: self.skill_resolver.clone(),
            code: self.code.clone(),
            host: host.clone(),
            usage: usage.clone(),
            cancel: inner.clone(),
            spec,
        };
        let fut = run.execute();
        tokio::pin!(fut);
        let outcome = tokio::select! {
            r = &mut fut => return r,
            _ = cancel.cancelled() => ExecutionStatus::Cancelled,
            _ = tokio::time::sleep(budget) => ExecutionStatus::Error,
        };
        inner.cancel();
        let _ = tokio::time::timeout(CLEANUP_GRACE, &mut fut).await;
        let mut result = ExecutionResult::with_status(outcome);
        result.usage = *usage.lock();
        if outcome == ExecutionStatus::Error {
            result.error = Some(format!("timed out after {timeout_secs} s"));
            host.emit(ExecutionDelta::Status {
                message: format!("Timed out after {timeout_secs} s."),
            });
        } else {
            host.emit(ExecutionDelta::Status {
                message: "Cancelled.".into(),
            });
        }
        result
    }
}

struct Run {
    providers: Arc<Providers>,
    skill_resolver: SkillResolver,
    code: Option<Arc<dyn CodeNav>>,
    host: Arc<dyn ExecutionHost>,
    usage: Arc<Mutex<Usage>>,
    cancel: CancellationToken,
    spec: ExecutionSpec,
}

/// What processing one turn's tool calls produced.
enum TurnEnd {
    Continue(Vec<Block>),
    Submitted {
        results: Vec<Block>,
        submit: Value,
        status: ExecutionStatus,
    },
}

fn policy_inputs(ctx: &ExecContext) -> PolicyInputs {
    let profile: ProjectProfile =
        load_toml(&paths::project_profile(&ctx.repo_root)).unwrap_or_default();
    let c = &profile.commands;
    let some = |v: &[&Option<String>]| -> Vec<String> {
        v.iter()
            .filter_map(|s| s.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "—")
            .map(String::from)
            .collect()
    };
    PolicyInputs {
        build_commands: some(&[&c.build, &c.test, &c.test_one, &c.lint, &c.typecheck]),
        test_commands: some(&[&c.test, &c.test_one]),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n... ({} more bytes)", &s[..end], s.len() - end)
}

fn transcript_chars(messages: &[Message]) -> usize {
    messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|b| match b {
            Block::Text { text } => text.len(),
            Block::Thinking { text, .. } => text.len(),
            Block::ToolUse { input, .. } => input.to_string().len(),
            Block::ToolResult { content, .. } => content.len(),
            Block::RedactedThinking { data } => data.len(),
            Block::Opaque { value, .. } => value.to_string().len(),
        })
        .sum()
}

/// Read-only tools whose calls in one turn may run concurrently.
fn is_concurrent(name: &str) -> bool {
    matches!(
        name,
        "Read" | "Grep" | "Glob" | "WebFetch" | "MemoryRecall" | "Skill"
    ) || ostra_core::agent::is_code_tool(name)
}

/// Buffers streamed deltas so the Activity log gets readable chunks, not one row per token.
struct Coalescer {
    host: Arc<dyn ExecutionHost>,
    buf: Mutex<(bool, String)>,
}

impl Coalescer {
    const FLUSH_AT: usize = 160;

    fn push(&self, thinking: bool, text: &str) {
        let mut b = self.buf.lock();
        if b.0 != thinking && !b.1.is_empty() {
            let out = std::mem::take(&mut b.1);
            self.send(b.0, out);
        }
        b.0 = thinking;
        b.1.push_str(text);
        if b.1.len() >= Self::FLUSH_AT || b.1.ends_with('\n') {
            let out = std::mem::take(&mut b.1);
            self.send(thinking, out);
        }
    }

    fn flush(&self) {
        let mut b = self.buf.lock();
        if !b.1.is_empty() {
            let out = std::mem::take(&mut b.1);
            self.send(b.0, out);
        }
    }

    fn send(&self, thinking: bool, text: String) {
        self.host.emit(if thinking {
            ExecutionDelta::Thinking { text }
        } else {
            ExecutionDelta::Text { text }
        });
    }
}

fn content_json(content: &[Block]) -> Value {
    serde_json::to_value(content).unwrap_or(Value::Null)
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// Rebuild a message list from a stored transcript and add the resume turn: `note`, or the
/// interruption notice.
pub fn rebuild_transcript(transcript: &[(String, Value)], note: Option<&str>) -> Vec<Message> {
    let mut messages: Vec<Message> = transcript
        .iter()
        .filter_map(|(role, content)| {
            let role = match role.as_str() {
                "assistant" => Role::Assistant,
                "user" => Role::User,
                _ => return None,
            };
            let content: Vec<Block> = serde_json::from_value(content.clone()).ok()?;
            (!content.is_empty()).then_some(Message { role, content })
        })
        .collect();
    let note = Block::text(
        note.unwrap_or("The previous run was interrupted. Continue from where it stopped."),
    );
    match messages.last() {
        Some(m) if m.role == Role::Assistant => {
            let open: Vec<Block> = m
                .content
                .iter()
                .filter_map(|b| match b {
                    Block::ToolUse { id, .. } => Some(Block::tool_result(
                        id.clone(),
                        "Interrupted: this call did not finish.",
                        true,
                    )),
                    _ => None,
                })
                .collect();
            let mut content = open;
            content.push(note);
            messages.push(Message {
                role: Role::User,
                content,
            });
        }
        Some(_) => {
            if let Some(last) = messages.last_mut() {
                last.content.push(note);
            }
        }
        None => messages.push(Message {
            role: Role::User,
            content: vec![note],
        }),
    }
    messages
}

impl Run {
    fn fail(&self, message: String) -> ExecutionResult {
        self.host.emit(ExecutionDelta::Status {
            message: message.clone(),
        });
        let mut r = ExecutionResult::error(message);
        r.usage = *self.usage.lock();
        r
    }

    fn record(&self, m: &Message) {
        self.host
            .record_message(role_str(m.role), &content_json(&m.content));
    }

    fn add_usage(&self, u: &Usage) {
        let snapshot = {
            let mut total = self.usage.lock();
            total.add(u);
            *total
        };
        self.host.emit(ExecutionDelta::Usage { usage: snapshot });
    }

    async fn execute(self) -> ExecutionResult {
        let spec = &self.spec;
        let (provider, model) = match self.providers.for_model(&spec.route.model) {
            Ok(p) => p,
            Err(ProviderError::NoKey(p)) => {
                return self.fail(format!("The `{p}` provider has no API key. Set its key in the environment or the OS keychain."));
            }
            Err(e) => return self.fail(format!("Cannot run model `{}`: {e}", spec.route.model)),
        };
        let ctx = &spec.ctx;
        let policy = ExecutionPolicy::new(ctx.clone(), policy_inputs(ctx));
        let env = ToolEnv::new(ToolEnvConfig {
            agent: spec.agent,
            repo_root: ctx.repo_root.clone(),
            session_dir: ctx.session_dir.clone(),
            report_file: ctx.report_file.clone(),
            memory_db: ctx.memory_db.clone(),
            memory_source: format!("{} {}", spec.agent, spec.id),
            skill_resolver: self.skill_resolver.clone(),
            code: self.code.clone(),
        });

        let offered = provider.server_tools(&model);
        let caps = &spec.capabilities;
        let server_fetch = offered.web_fetch && caps.contains(&ostra_core::Capability::WebFetch);
        let server_tools = ServerTools {
            web_search: offered.web_search && ostra_tools::wants_web_search(caps),
            web_fetch: server_fetch,
        };
        let submit_name = spec.agent.submit_tool_name();
        let mut tools: Vec<ToolDef> = ostra_tools::definitions(caps)
            .into_iter()
            .filter(|d| !(server_fetch && d.name == "WebFetch"))
            .map(|d| ToolDef {
                name: d.name,
                description: d.description,
                input_schema: d.input_schema,
                cache: false,
            })
            .collect();
        if caps.contains(&ostra_core::Capability::Document)
            && let Some(d) = ostra_tools::document_tool_definition(spec.agent)
        {
            tools.push(ToolDef {
                name: d.name,
                description: d.description,
                input_schema: d.input_schema,
                cache: false,
            });
        }
        let submit_def = ostra_tools::submit_tool_definition(spec.agent);
        tools.push(ToolDef {
            name: submit_def.name,
            description: submit_def.description,
            input_schema: if spec.submit_schema.is_null() {
                submit_def.input_schema
            } else {
                spec.submit_schema.clone()
            },
            cache: true,
        });

        let mut messages = match &spec.resume {
            Some(resume) => {
                let transcript = self.host.transcript(&resume.from);
                let mut m = rebuild_transcript(&transcript, resume.note.as_deref());
                if transcript.is_empty() {
                    m.insert(0, Message::user_text(spec.first_message.clone()));
                }
                m
            }
            None => vec![Message::user_text(spec.first_message.clone())],
        };
        for m in &messages {
            self.record(m);
        }

        let coalescer = Arc::new(Coalescer {
            host: self.host.clone(),
            buf: Mutex::new((false, String::new())),
        });
        let mut final_text = String::new();
        let mut reminded = false;
        let mut continues = 0;
        for _ in 0..MAX_TURNS {
            if self.cancel.is_cancelled() {
                let mut r = ExecutionResult::with_status(ExecutionStatus::Cancelled);
                r.usage = *self.usage.lock();
                return r;
            }
            let long = transcript_chars(&messages) > CLEAR_THRESHOLD_CHARS;
            if long && provider.name() != "anthropic" {
                ostra_providers::clear_old_tool_results(&mut messages, KEEP_TOOL_RESULTS);
            }
            let mut req = ChatRequest::new(model.clone());
            req.system = vec![SystemBlock::cached(spec.system_prompt.clone())];
            req.messages = messages.clone();
            req.tools = tools.clone();
            req.server_tools = server_tools;
            req.effort = spec.effort;
            req.tool_choice = ToolChoice::Auto;
            req.clear_old_tool_results = long && provider.name() == "anthropic";

            let resp = match self.chat(provider.as_ref(), req, &coalescer).await {
                Ok(r) => r,
                Err(ProviderError::Cancelled) => {
                    let mut r = ExecutionResult::with_status(ExecutionStatus::Cancelled);
                    r.usage = *self.usage.lock();
                    return r;
                }
                Err(e) => return self.fail(format!("The model call failed: {e}")),
            };
            self.add_usage(&resp.usage);
            let text = resp.text();
            if !text.trim().is_empty() {
                final_text = text.clone();
            }
            let assistant = Message::assistant(resp.content.clone());
            self.record(&assistant);
            messages.push(assistant);

            let has_tools = !resp.tool_uses().is_empty();
            match &resp.stop {
                StopReason::PauseTurn => continue,
                StopReason::Refusal => {
                    let cat = resp
                        .refusal_category
                        .map(|c| format!(" (category {c})"))
                        .unwrap_or_default();
                    return self.fail(format!(
                        "The model refused to continue{cat}: {}",
                        text.trim()
                    ));
                }
                StopReason::MaxTokens if !has_tools => {
                    if continues >= MAX_TOKEN_CONTINUES {
                        return self.fail(
                            "The model kept hitting its output limit without finishing.".into(),
                        );
                    }
                    continues += 1;
                    let m = Message::user_text(
                        "You reached the output limit. Continue from exactly where you stopped.",
                    );
                    self.record(&m);
                    messages.push(m);
                    continue;
                }
                _ => {}
            }
            if !has_tools {
                if reminded {
                    let mut r = ExecutionResult::with_status(ExecutionStatus::Ok);
                    r.final_text = final_text;
                    r.usage = *self.usage.lock();
                    return r;
                }
                reminded = true;
                let m = Message::user_text(format!(
                    "Call {submit_name} now with your result. The engine reads only that call, so a result given in text is lost."
                ));
                self.record(&m);
                messages.push(m);
                continue;
            }
            match self.run_tools(&resp, &policy, &env, &submit_name).await {
                TurnEnd::Continue(results) => {
                    let m = Message::tool_results(results);
                    self.record(&m);
                    messages.push(m);
                }
                TurnEnd::Submitted {
                    results,
                    submit,
                    status,
                } => {
                    self.record(&Message::tool_results(results));
                    let mut r = ExecutionResult::with_status(status);
                    r.submit = Some(submit);
                    r.final_text = final_text;
                    r.usage = *self.usage.lock();
                    return r;
                }
            }
        }
        self.fail(format!(
            "The run passed {MAX_TURNS} model turns without calling {submit_name}."
        ))
    }

    async fn chat(
        &self,
        provider: &dyn Provider,
        req: ChatRequest,
        coalescer: &Arc<Coalescer>,
    ) -> Result<ChatResponse, ProviderError> {
        let c = coalescer.clone();
        let host = self.host.clone();
        let sink = move |e: StreamEvent| match e {
            StreamEvent::TextDelta(t) => c.push(false, &t),
            StreamEvent::ThinkingDelta(t) => c.push(true, &t),
            StreamEvent::ServerToolUsed { name, summary } => {
                c.flush();
                host.emit(ExecutionDelta::Status {
                    message: format!("{name}: {summary}"),
                });
            }
            _ => {}
        };
        let r = provider.chat(req, &sink, self.cancel.clone()).await;
        coalescer.flush();
        r
    }

    async fn run_tools(
        &self,
        resp: &ChatResponse,
        policy: &ExecutionPolicy,
        env: &ToolEnv,
        submit_name: &str,
    ) -> TurnEnd {
        let submit_schema = ostra_core::submit::submit_schema(self.spec.agent);
        let calls: Vec<(String, String, Value)> = resp
            .tool_uses()
            .into_iter()
            .map(|(id, name, input)| {
                let mut input = input.clone();
                if name == submit_name {
                    ostra_core::args::coerce_json_strings(&mut input, &submit_schema);
                }
                (id.to_string(), name.to_string(), input)
            })
            .collect();
        let mut results: Vec<Option<Block>> = vec![None; calls.len()];
        let mut i = 0;
        while i < calls.len() {
            let (id, name, input) = &calls[i];
            if name == submit_name {
                match self.submit(id, name, input, policy) {
                    Ok((block, status)) => {
                        results[i] = Some(block);
                        for (j, (oid, _, _)) in calls.iter().enumerate() {
                            if results[j].is_none() {
                                results[j] = Some(Block::tool_result(
                                    oid.clone(),
                                    "Not run: the run ended with the submit call.",
                                    true,
                                ));
                            }
                        }
                        return TurnEnd::Submitted {
                            results: results.into_iter().flatten().collect(),
                            submit: input.clone(),
                            status,
                        };
                    }
                    Err(block) => {
                        results[i] = Some(block);
                        i += 1;
                        continue;
                    }
                }
            }
            if is_concurrent(name) {
                let mut j = i;
                while j < calls.len() && is_concurrent(&calls[j].1) {
                    j += 1;
                }
                let outs = join_all(
                    calls[i..j]
                        .iter()
                        .map(|(id, name, input)| self.tool(id, name, input, policy, env)),
                )
                .await;
                for (k, out) in outs.into_iter().enumerate() {
                    results[i + k] = Some(out);
                }
                i = j;
            } else {
                results[i] = Some(self.tool(id, name, input, policy, env).await);
                i += 1;
            }
        }
        TurnEnd::Continue(results.into_iter().flatten().collect())
    }

    fn submit(
        &self,
        id: &str,
        name: &str,
        input: &Value,
        policy: &ExecutionPolicy,
    ) -> Result<(Block, ExecutionStatus), Block> {
        let call = ToolCall::new(name, input.clone());
        self.host.emit(ExecutionDelta::ToolCall {
            call_id: id.into(),
            call: call.clone(),
        });
        let err = |text: String| {
            self.host.emit(ExecutionDelta::ToolResult {
                call_id: id.into(),
                output: truncate(&text, EMIT_LIMIT),
                is_error: true,
                duration_ms: 0,
            });
            Block::tool_result(id, text, true)
        };
        if input.get("__invalid_json").is_some() {
            return Err(err(format!(
                "The {name} input was not valid JSON. Call {name} again with a valid object."
            )));
        }
        policy.set_yolo(self.host.yolo());
        let decision = policy.check(&call);
        self.host.emit(ExecutionDelta::Policy {
            call_id: id.into(),
            decision: decision.clone(),
        });
        if let PolicyDecision::Deny { reason, rule } = decision {
            return Err(err(format!(
                "Denied by {} `{}`: {reason}",
                rule.layer, rule.rule
            )));
        }
        if let Err(e) = ostra_core::submit::validate_submit(self.spec.agent, input) {
            return Err(err(format!(
                "The {name} input is invalid: {e}. Fix it and call {name} again."
            )));
        }
        if let Err(e) = ostra_core::doc::check_submit(self.spec.agent, input) {
            return Err(err(format!("{e} Nothing was recorded.")));
        }
        let status = match input.get("status").and_then(|s| s.as_str()) {
            Some("stuck") => ExecutionStatus::Stuck,
            Some("handoff") => ExecutionStatus::Handoff,
            _ => ExecutionStatus::Ok,
        };
        if status == ExecutionStatus::Ok
            && let Some(report) = &self.spec.ctx.report_file
            && !report.exists()
            && self.spec.agent != AgentName::CodeReviewer
        {
            return Err(err(format!(
                "Write your report to {} before you call {name}, because the engine reads the report at that path.",
                report.display()
            )));
        }
        self.host.emit(ExecutionDelta::ToolResult {
            call_id: id.into(),
            output: "Result received.".into(),
            is_error: false,
            duration_ms: 0,
        });
        Ok((Block::tool_result(id, "Result received.", false), status))
    }

    async fn tool(
        &self,
        id: &str,
        name: &str,
        input: &Value,
        policy: &ExecutionPolicy,
        env: &ToolEnv,
    ) -> Block {
        let call = ToolCall::new(name, input.clone());
        self.host.emit(ExecutionDelta::ToolCall {
            call_id: id.into(),
            call: call.clone(),
        });
        self.usage.lock().tool_calls += 1;
        let err = |text: String| {
            self.host.emit(ExecutionDelta::ToolResult {
                call_id: id.into(),
                output: truncate(&text, EMIT_LIMIT),
                is_error: true,
                duration_ms: 0,
            });
            Block::tool_result(id, text, true)
        };
        if let Some(raw) = input.get("__invalid_json") {
            return err(format!(
                "The tool input was not valid JSON, so the call did not run: {}",
                truncate(&raw.to_string(), 500)
            ));
        }
        policy.set_yolo(self.host.yolo());
        // The Bash tool keeps its own working directory, so the guards resolve relative write
        // targets against it rather than against the repo root.
        let checked = match (name, input) {
            ("Bash", Value::Object(map)) if !map.contains_key("cwd") => {
                let mut map = map.clone();
                map.insert("cwd".into(), Value::String(env.cwd().display().to_string()));
                ToolCall::new(name, Value::Object(map))
            }
            _ => call.clone(),
        };
        let decision = policy.check(&checked);
        self.host.emit(ExecutionDelta::Policy {
            call_id: id.into(),
            decision: decision.clone(),
        });
        match decision {
            PolicyDecision::Deny { reason, rule } => {
                return err(format!(
                    "Denied by {} `{}`: {reason}",
                    rule.layer, rule.rule
                ));
            }
            PolicyDecision::Ask { reason, rule } => {
                match self.host.ask_permission(&call, &reason, &rule).await {
                    PermissionAnswer::AllowOnce => {}
                    PermissionAnswer::AlwaysInWorkspace => {
                        if let Some(rule) = policy.allow_rule_suggestion(&call) {
                            policy.add_session_allow(rule);
                        }
                    }
                    PermissionAnswer::Deny => {
                        return err(format!(
                            "The user denied this call ({reason}). Continue without it, or explain in your report why it is needed."
                        ));
                    }
                }
            }
            PolicyDecision::Allow { .. } => {}
        }
        let host = self.host.clone();
        let live: ostra_tools::LiveOutput = Arc::new(move |call_id: &str, chunk: &str| {
            host.emit(ExecutionDelta::ToolOutput {
                call_id: call_id.into(),
                chunk: chunk.into(),
            });
        });
        let out = ostra_tools::execute(env, id, &call, Some(live), self.cancel.clone()).await;
        if name == "Bash" {
            let cmd = call.str_field("command").unwrap_or_default();
            let inputs = policy_inputs(&self.spec.ctx);
            let configured: Vec<String> = inputs
                .build_commands
                .into_iter()
                .chain(inputs.test_commands)
                .collect();
            if ostra_policy::build::is_build_command(cmd, &configured) {
                self.usage.lock().build_ms += out.duration_ms;
            }
        }
        let outcome = ToolOutcome {
            output: out.text.clone(),
            is_error: out.is_error,
            exit_code: out.exit_code,
            result_known: true,
        };
        let mut text = out.text.clone();
        for obs in policy.observe(&call, &outcome) {
            match obs {
                Observation::AppendNote(note) => {
                    text.push_str("\n\n");
                    text.push_str(&note);
                }
                Observation::RecallLessons { query } => {
                    let lessons = MemoryStore::open(&self.spec.ctx.memory_db)
                        .and_then(|m| m.recall(None, Some(&query), 5));
                    if let Ok(lessons) = lessons
                        && !lessons.is_empty()
                    {
                        text.push_str(
                            "\n\nLessons recorded earlier that may apply to this failure:\n",
                        );
                        for (n, l) in lessons.iter().enumerate() {
                            text.push_str(&format!(
                                "{}. [{}] {} (source: {})\n",
                                n + 1,
                                l.area,
                                l.lesson,
                                l.source
                            ));
                        }
                    }
                }
            }
        }
        let shown = match &out.diff {
            Some(diff) => format!("{}\n{}", out.text, diff),
            None => text.clone(),
        };
        self.host.emit(ExecutionDelta::ToolResult {
            call_id: id.into(),
            output: truncate(&shown, EMIT_LIMIT),
            is_error: out.is_error,
            duration_ms: out.duration_ms,
        });
        Block::tool_result(id, text, out.is_error)
    }
}

#[cfg(test)]
mod tests;
