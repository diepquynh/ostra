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
use ostra_core::coord::RunEnd;
use ostra_tools::{
    CodeNav, CoordConnector, ManageConnector, McpConnector, SkillResolver, ToolEnv, ToolEnvConfig,
};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Upper bound on model turns in one execution, a guard against a loop that never submits.
pub const MAX_TURNS: usize = 400;
/// "Continue" turns allowed after the model hits its output limit without a tool call.
pub const MAX_TOKEN_CONTINUES: usize = 3;
/// Share of the model's context window at which the loop compacts the conversation.
pub const COMPACT_AT: f64 = 0.95;
/// Context window assumed for a model the models.dev catalog does not list.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
/// After a compaction that returned no summary, the next try waits until the context grows by
/// this share of the window, because each try sends the whole conversation.
const COMPACT_RETRY_GROWTH: f64 = 0.02;
const COMPACT_INSTRUCTIONS: &str = "Summarize this conversation so that you can continue the task from the summary alone, because every earlier message is replaced by it. Keep the task and every requirement from the first message; the files you read and the facts from them you still need; every file you created or changed and how; the decisions you made and why; commands that failed and their errors; what is left to do; and the exact next step. Keep paths, identifiers, and error messages verbatim. Do not call tools. Answer with the summary text only.";
const AFTER_COMPACTION: &str = "The conversation was compacted into the summary above because the context was nearly full. Continue the task from the next step it names. Earlier tool results are gone, so read a file again when you need its content.";
/// Emitted Activity output is truncated to this size; the model gets the full output.
pub const EMIT_LIMIT: usize = 8 * 1024;
/// How long a cancelled or timed-out run may take to kill its tools before returning.
const CLEANUP_GRACE: Duration = Duration::from_secs(3);

pub struct NativeExecutor {
    providers: Arc<Providers>,
    skill_resolver: SkillResolver,
    code: Option<Arc<dyn CodeNav>>,
    mcp: Option<Arc<dyn McpConnector>>,
    manage: Option<Arc<dyn ManageConnector>>,
    coord: Option<Arc<dyn CoordConnector>>,
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
            mcp: None,
            manage: None,
            coord: None,
        }
    }

    pub fn with_mcp(mut self, mcp: Arc<dyn McpConnector>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    pub fn with_manage(mut self, manage: Arc<dyn ManageConnector>) -> Self {
        self.manage = Some(manage);
        self
    }

    pub fn with_coord(mut self, coord: Arc<dyn CoordConnector>) -> Self {
        self.coord = Some(coord);
        self
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
            mcp: self.mcp.clone(),
            manage: self.manage.clone(),
            coord: self.coord.clone(),
            host: host.clone(),
            usage: usage.clone(),
            cancel: inner.clone(),
            git_repos: ostra_sandbox::git_repos(&[
                &spec.ctx.repo_root,
                &spec.ctx.workspace_root,
            ]),
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
    mcp: Option<Arc<dyn McpConnector>>,
    manage: Option<Arc<dyn ManageConnector>>,
    coord: Option<Arc<dyn CoordConnector>>,
    host: Arc<dyn ExecutionHost>,
    usage: Arc<Mutex<Usage>>,
    cancel: CancellationToken,
    /// The repos the sandbox protects, found once at the start.
    git_repos: Vec<std::path::PathBuf>,
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
        read_only_mcp_tools: vec![],
    }
}

fn input_message(input: &Value) -> &str {
    input.get("message").and_then(Value::as_str).unwrap_or_default()
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

/// Rough token count of messages not yet measured by a response, at four characters a token.
fn estimate_tokens(messages: &[Message]) -> u64 {
    let chars: usize = messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|b| match b {
            Block::Text { text } => text.len(),
            Block::Thinking { text, .. } => text.len(),
            Block::ToolUse { input, .. } => input.to_string().len(),
            Block::ToolResult { content, .. } => content.len(),
            Block::RedactedThinking { data } => data.len(),
            Block::Opaque { value, .. } => value.to_string().len(),
            Block::Compaction { summary, value, .. } => {
                summary.len() + value.as_ref().map_or(0, |v| v.to_string().len())
            }
        })
        .sum();
    chars as u64 / 4
}

/// The prompt tokens of a response plus its output: the size the next request starts from.
fn context_of(u: &Usage) -> u64 {
    u.input_tokens + u.cache_read_tokens + u.cache_write_tokens + u.output_tokens
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

const COMPACTION_RECORD: &str = "compaction";

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
    let mut messages = stored_messages(transcript);
    let turn = resume_turn(&messages, note);
    append_turn(&mut messages, turn);
    messages
}

/// The stored messages, with adjacent messages of one role merged, because a resume that stopped
/// after a user turn stores its note as a message of its own. A `compaction` record holds the
/// window that replaced every message before it.
fn stored_messages(transcript: &[(String, Value)]) -> Vec<Message> {
    let mut out: Vec<Message> = vec![];
    for (role, content) in transcript {
        let role = match role.as_str() {
            "assistant" => Role::Assistant,
            "user" => Role::User,
            COMPACTION_RECORD => {
                if let Ok(window) = serde_json::from_value::<Vec<Message>>(content.clone()) {
                    out = window;
                }
                continue;
            }
            _ => continue,
        };
        let Ok(content) = serde_json::from_value::<Vec<Block>>(content.clone()) else {
            continue;
        };
        if content.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == role => last.content.extend(content),
            _ => out.push(Message { role, content }),
        }
    }
    out
}

/// The user turn a resume adds: a result for each tool call the interruption left open, then
/// the note.
fn resume_turn(messages: &[Message], note: Option<&str>) -> Message {
    let mut content: Vec<Block> = match messages.last() {
        Some(m) if m.role == Role::Assistant => m
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
            .collect(),
        _ => vec![],
    };
    content.push(Block::text(note.unwrap_or(
        "The previous run was interrupted. Continue from where it stopped.",
    )));
    Message {
        role: Role::User,
        content,
    }
}

fn append_turn(messages: &mut Vec<Message>, turn: Message) {
    match messages.last_mut() {
        Some(last) if last.role == Role::User => last.content.extend(turn.content),
        _ => messages.push(turn),
    }
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
        let mcp = match &self.mcp {
            Some(c) => {
                let opened = c.open(spec).await;
                for message in opened.notes {
                    self.host.emit(ExecutionDelta::Status { message });
                }
                opened.tools
            }
            None => None,
        };
        let mut inputs = policy_inputs(ctx);
        inputs.read_only_mcp_tools = mcp.iter().flat_map(|m| m.read_only()).collect();
        let policy = ExecutionPolicy::new(ctx.clone(), inputs);
        let env = ToolEnv::new(ToolEnvConfig {
            agent: spec.agent,
            repo_root: ctx.repo_root.clone(),
            workspace_root: ctx.workspace_root.clone(),
            session_dir: ctx.session_dir.clone(),
            report_file: ctx.report_file.clone(),
            memory_db: ctx.memory_db.clone(),
            memory_source: format!("{} {}", spec.agent, spec.id),
            skill_resolver: self.skill_resolver.clone(),
            code: self.code.clone(),
            mcp: mcp.clone(),
            manage: self.manage.as_ref().and_then(|m| m.open(spec)),
            coord: self.coord.as_ref().and_then(|c| c.open(spec)),
        })
        .with_private_hosts(ostra_tools::webfetch_hosts(&ctx.permissions.allow))
        .with_scrub_env(mcp_secret_vars(ctx));
        let mut _scratch = None;
        let env = match sandbox_for(ctx, self.host.clone()) {
            Ok(Sandbox::On(backend, profile)) => {
                _scratch = profile.scratch_dir().map(|d| Scratch(d.to_path_buf()));
                env.with_sandbox(backend, *profile)
            }
            Ok(Sandbox::Off(warning)) => {
                if let Some(w) = warning {
                    if ostra_sandbox::first_warning() {
                        tracing::warn!("agent commands run without a sandbox: {w}");
                    }
                    self.host.emit(ExecutionDelta::Status {
                        message: format!("Bash runs without a sandbox. {w}"),
                    });
                }
                env
            }
            Err(e) => return self.fail(e),
        };

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
        tools.extend(mcp.iter().flat_map(|m| m.definitions()).map(|d| ToolDef {
            name: d.name,
            description: d.description,
            input_schema: d.input_schema,
            cache: false,
        }));
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
                let mut m = stored_messages(&self.host.transcript(&resume.from));
                let fresh = m.is_empty();
                if fresh {
                    m.push(Message::user_text(spec.first_message.clone()));
                }
                let turn = resume_turn(&m, resume.note.as_deref());
                // Rule P2: resumed in place, the stored transcript is this run's, so only the
                // new turn is added and the replayed prefix hits the prompt cache.
                if resume.from == spec.id && !fresh {
                    self.record(&turn);
                    append_turn(&mut m, turn);
                } else {
                    append_turn(&mut m, turn);
                    for msg in &m {
                        self.record(msg);
                    }
                }
                m
            }
            None => {
                let m = vec![Message::user_text(spec.first_message.clone())];
                self.record(&m[0]);
                m
            }
        };

        let coalescer = Arc::new(Coalescer {
            host: self.host.clone(),
            buf: Mutex::new((false, String::new())),
        });
        let mut final_text = String::new();
        let mut reminded = false;
        let mut continues = 0;
        let window = ostra_core::pricing::context_window(&spec.route.model)
            .unwrap_or(DEFAULT_CONTEXT_WINDOW);
        // The last response's context, and how many messages it covered.
        let (mut measured, mut measured_len) = (0u64, 0usize);
        let mut failed_compaction_at: Option<u64> = None;
        for _ in 0..MAX_TURNS {
            if self.cancel.is_cancelled() {
                let mut r = ExecutionResult::with_status(ExecutionStatus::Cancelled);
                r.usage = *self.usage.lock();
                return r;
            }
            let mut req = ChatRequest::new(model.clone());
            req.system = vec![SystemBlock::cached(spec.system_prompt.clone())];
            req.tools = tools.clone();
            req.server_tools = server_tools;
            req.effort = spec.effort;
            req.tool_choice = ToolChoice::Auto;

            let next = measured + estimate_tokens(&messages[measured_len.min(messages.len())..]);
            let retry_ok = failed_compaction_at.is_none_or(|at| {
                next as f64 >= at as f64 + COMPACT_RETRY_GROWTH * window as f64
            });
            // A user turn last means no tool call is open, as compaction requires.
            if next as f64 >= COMPACT_AT * window as f64
                && retry_ok
                && messages.last().is_some_and(|m| m.role == Role::User)
            {
                let mut ask = req.clone();
                ask.messages = messages.clone();
                match self.compact(provider.as_ref(), ask, next, window).await {
                    Ok(Some(mut compacted)) => {
                        append_turn(&mut compacted, Message::user_text(AFTER_COMPACTION));
                        self.host.record_message(
                            COMPACTION_RECORD,
                            &serde_json::to_value(&compacted).unwrap_or(Value::Null),
                        );
                        messages = compacted;
                        failed_compaction_at = None;
                    }
                    Ok(None) => failed_compaction_at = Some(next),
                    Err(ProviderError::Cancelled) => {
                        let mut r = ExecutionResult::with_status(ExecutionStatus::Cancelled);
                        r.usage = *self.usage.lock();
                        return r;
                    }
                    Err(e) => {
                        self.host.emit(ExecutionDelta::Status {
                            message: format!("Compaction failed, so the run continues without it: {e}"),
                        });
                        failed_compaction_at = Some(next);
                    }
                }
            }
            req.messages = messages.clone();

            let resp = match self.chat(provider.as_ref(), req, &coalescer).await {
                Ok(r) => r,
                Err(ProviderError::Cancelled) => {
                    let mut r = ExecutionResult::with_status(ExecutionStatus::Cancelled);
                    r.usage = *self.usage.lock();
                    return r;
                }
                Err(e) => return self.fail(format!("The model call failed: {e}")),
            };
            let mut usage = resp.usage;
            usage.context_tokens = context_of(&usage);
            self.add_usage(&usage);
            let text = resp.text();
            if !text.trim().is_empty() {
                final_text = text.clone();
            }
            let assistant = Message::assistant(resp.content.clone());
            self.record(&assistant);
            messages.push(assistant);
            (measured, measured_len) = (usage.context_tokens, messages.len());

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
                let m = Message::user_text(if spec.ctx.owes_reply {
                    ostra_core::coord::reply_instruction(ostra_core::coord::SUBAGENT_REPLY)
                } else {
                    format!(
                        "Call {submit_name} now with your result. The engine reads only that call, so a result given in text is lost."
                    )
                });
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

    /// Ask the provider to compact `req.messages`. `None` when no summary came back.
    async fn compact(
        &self,
        provider: &dyn Provider,
        req: ChatRequest,
        tokens: u64,
        window: u64,
    ) -> Result<Option<Vec<Message>>, ProviderError> {
        self.host.emit(ExecutionDelta::Status {
            message: format!(
                "Compacting the conversation: about {tokens} of {window} context tokens used."
            ),
        });
        let out = provider
            .compact(req, COMPACT_INSTRUCTIONS, self.cancel.clone())
            .await?;
        self.add_usage(&out.usage);
        self.host.emit(ExecutionDelta::Status {
            message: match out.messages {
                Some(_) => "Compacted the conversation into a summary.".into(),
                None => "Compaction returned no summary, so the run continues without it.".into(),
            },
        });
        Ok(out.messages)
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
            } else if ostra_core::coord::is_coord_tool(name) {
                let (block, end) = self.coord_tool(id, name, input, policy, env).await;
                results[i] = Some(block);
                i += 1;
                // Rule H2: a run that asked waits, and a consult run that answered ends.
                let status = match end {
                    RunEnd::Continue => continue,
                    RunEnd::Wait => ExecutionStatus::Waiting,
                    RunEnd::Finish => ExecutionStatus::Ok,
                };
                for (j, (oid, _, _)) in calls.iter().enumerate() {
                    if results[j].is_none() {
                        results[j] = Some(Block::tool_result(
                            oid.clone(),
                            "Not run: the run stopped to wait for another subagent.",
                            true,
                        ));
                    }
                }
                return TurnEnd::Submitted {
                    results: results.into_iter().flatten().collect(),
                    submit: ostra_core::coord::end_payload(name, input_message(input)),
                    status,
                };
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
        self.tool_end(id, name, input, policy, env).await.0
    }

    async fn coord_tool(
        &self,
        id: &str,
        name: &str,
        input: &Value,
        policy: &ExecutionPolicy,
        env: &ToolEnv,
    ) -> (Block, RunEnd) {
        self.tool_end(id, name, input, policy, env).await
    }

    async fn tool_end(
        &self,
        id: &str,
        name: &str,
        input: &Value,
        policy: &ExecutionPolicy,
        env: &ToolEnv,
    ) -> (Block, RunEnd) {
        // The policy checks, the user approves, and the tool runs this one call, so relative
        // paths and the Bash working directory mean the same target in all three.
        let call = env.canonical_call(&ToolCall::new(name, input.clone()));
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
            (Block::tool_result(id, text, true), RunEnd::Continue)
        };
        if let Some(raw) = input.get("__invalid_json") {
            return err(format!(
                "The tool input was not valid JSON, so the call did not run: {}",
                truncate(&raw.to_string(), 500)
            ));
        }
        policy.set_yolo(self.host.yolo());
        let decision = policy.check(&call);
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
        let mut repaired = vec![];
        if name == "Bash" {
            repaired = ostra_sandbox::repair_git_dirs(&self.git_repos);
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
        for path in &repaired {
            let note = ostra_sandbox::git_repair_note(path);
            // Counted as a containment signal, like the guard's own refusals.
            self.host.emit(ExecutionDelta::Policy {
                call_id: id.into(),
                decision: PolicyDecision::deny(
                    ostra_core::policy::RuleRef::guard(ostra_core::containment::GIT_METADATA),
                    &note,
                ),
            });
            text.push_str("\n\n");
            text.push_str(&note);
        }
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
        let end = if out.is_error { RunEnd::Continue } else { out.end };
        (Block::tool_result(id, text, out.is_error), end)
    }
}

enum Sandbox {
    On(
        ostra_sandbox::Backend,
        Box<ostra_sandbox::Profile>,
    ),
    Off(Option<String>),
}

/// The execution's scratch dir (its `/tmp` under bubblewrap, its `TMPDIR` under Seatbelt), removed when the execution's future ends or is dropped.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The workspace's MCP `oauth.client_secret_env` names, which agent commands must not inherit.
fn mcp_secret_vars(ctx: &ostra_core::exec::ExecContext) -> Vec<String> {
    if ctx.workspace_root.as_os_str().is_empty() {
        return vec![];
    }
    let ws: ostra_core::config::WorkspaceSettings =
        ostra_core::config::load_toml(&ostra_core::paths::workspace_toml(&ctx.workspace_root))
            .unwrap_or_default();
    let global: ostra_core::config::GlobalConfig =
        ostra_core::config::load_toml(&ostra_core::paths::global_config_path()).unwrap_or_default();
    ostra_core::config::credential_env_names(&global, &ws.mcp_servers)
        .into_iter()
        .collect()
}

/// The `[sandbox]` table is read fresh, so a change applies to the next execution. The workspace's
/// own mode, when it sets one, takes the place of the global mode. Every Bash call of the
/// execution shares one egress proxy and one set of decoys, which report to `host`.
fn sandbox_for(
    ctx: &ostra_core::exec::ExecContext,
    host: Arc<dyn ExecutionHost>,
) -> Result<Sandbox, String> {
    let global: ostra_core::config::GlobalConfig =
        ostra_core::config::load_toml(&ostra_core::paths::global_config_path()).unwrap_or_default();
    let sandbox = global.sandbox.for_workspace(&ctx.sandbox());
    let home = ostra_core::paths::home()
        .unwrap_or_else(|| "/".into());
    Ok(match ostra_sandbox::decide(&sandbox)? {
        ostra_sandbox::Decision::Sandboxed(backend) => {
            let scratch = ostra_sandbox::new_scratch()
                .map_err(|e| format!("Cannot create the sandbox's /tmp: {e}"))?;
            let profile = ostra_sandbox::Profile::for_execution(ctx, &sandbox, &home)
                .scratch(&scratch)
                .map_err(|e| format!("Cannot create the sandbox's /tmp: {e}"))?
                .start_egress(&backend, ostra_sandbox::report::egress(host.clone()))
                .map_err(|e| format!("Cannot start the sandbox's egress proxy: {e}"))?
                .watch_decoys(
                    &backend,
                    &home,
                    &ctx.sandbox_decoys,
                    ostra_sandbox::report::decoys(host),
                )
                .map_err(|e| format!("Cannot plant the sandbox's decoy files: {e}"))?;
            Sandbox::On(backend, Box::new(profile))
        }
        ostra_sandbox::Decision::Unsandboxed(w) => Sandbox::Off(w),
    })
}

#[cfg(test)]
mod tests;
