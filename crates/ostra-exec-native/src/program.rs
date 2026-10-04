//! Rule PL2: programmatic agents. A plugin's code runs the agent; each tool it calls goes through
//! the same `tool_end` path, policy, sandbox, and permission asks as a model's call, and its model
//! calls run on the agent's route.
//!
//! Rule PL8: a run whose plugin program stops is not lost. Ostra starts the program again, at most
//! `PLUGIN_RESTARTS` times per run, and runs the agent once more under the same execution, with
//! `resumed` set and the checkpoints the plugin saved, so it continues where it was.

use crate::{Coalescer, Run, Setup};
use ostra_core::coord::RunEnd;
use ostra_core::exec::{ExecutionDelta, ExecutionResult, ExecutionStatus};
use ostra_core::plugin::{
    AgentCalls, AgentTask, CompleteRequest, PLUGIN_RESTARTS, Plugin, ToolReply,
};
use ostra_providers::{Block, ChatRequest, Message, SystemBlock, ToolChoice};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The most tool and model calls one programmatic run may make, like the model loop's turn cap.
pub const MAX_PROGRAM_CALLS: u64 = 2000;

struct Calls {
    run: Run,
    setup: Setup,
    tools: Vec<String>,
    count: AtomicU64,
}

/// Rule PL8: starts a plugin's program again after it stopped.
#[async_trait::async_trait]
pub trait PluginRestart: Send + Sync {
    async fn restart(&self) -> Result<Arc<dyn Plugin>, String>;
}

pub(crate) async fn execute(
    run: Run,
    plugin: Arc<dyn Plugin>,
    restart: Arc<dyn PluginRestart>,
) -> ExecutionResult {
    let setup = match run.setup().await {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let spec = run.spec.clone();
    let mut tools: Vec<String> = ostra_tools::definitions(&spec.capabilities)
        .into_iter()
        .map(|d| d.name)
        .collect();
    tools.extend(
        setup
            .mcp
            .iter()
            .flat_map(|m| m.definitions())
            .map(|d| d.name),
    );
    let task = AgentTask {
        execution: spec.id.clone(),
        session: spec.ctx.session_id.clone(),
        agent: spec.agent.to_string(),
        first_message: match spec.resume.as_ref().and_then(|r| r.note.clone()) {
            Some(note) => format!("{}\n\n{note}", spec.first_message),
            None => spec.first_message.clone(),
        },
        repo_root: spec.ctx.repo_root.clone(),
        session_dir: spec.ctx.session_dir.clone(),
        workspace_root: spec.ctx.workspace_root.clone(),
        model: spec.route.model.clone(),
        tools: tools.clone(),
        submit_schema: spec.submit_schema.clone(),
        resumed: spec.resume.as_ref().is_some_and(|r| r.from == spec.id),
    };
    let checkpoints = run.host.checkpoints(&plugin.manifest().name);
    run.host.emit(ExecutionDelta::Status {
        message: format!("{} runs in its plugin's code.", spec.agent),
    });
    let calls = Arc::new(Calls {
        run,
        setup,
        tools,
        count: AtomicU64::new(0),
    });
    let mut plugin = plugin;
    let mut task = task;
    let mut restarts = 0;
    let outcome = loop {
        let outcome = plugin
            .run_agent(
                spec.agent.as_str(),
                task.clone(),
                calls.clone(),
                checkpoints.clone(),
            )
            .await;
        match outcome {
            Err(e) if !plugin.is_alive() && !calls.run.cancel.is_cancelled() => {
                if restarts >= PLUGIN_RESTARTS {
                    break Err(format!(
                        "{e}. Its program stopped {} times during this run",
                        restarts + 1
                    ));
                }
                restarts += 1;
                calls.run.host.emit(ExecutionDelta::Status {
                    message: format!(
                        "The plugin's program stopped ({e}). Ostra starts it again, and the agent resumes from its checkpoints."
                    ),
                });
                match restart.restart().await {
                    Ok(p) => {
                        plugin = p;
                        task.resumed = true;
                    }
                    Err(why) => break Err(format!("{e}. Its program did not start again: {why}")),
                }
            }
            other => break other,
        }
    };
    let run = &calls.run;
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => return run.fail(format!("The plugin's agent failed: {e}")),
    };
    // Rule SM6: a run that owes a waiting sender a reply sends it before it submits.
    if let Some(why) = run.host.submit_blocked() {
        return run.fail(format!("The plugin's agent ended without its reply. {why}"));
    }
    let submit = outcome.submit;
    if let Err(e) =
        ostra_core::submit::validate_submit_with(spec.ctx.contract, &spec.submit_schema, &submit)
    {
        return run.fail(format!(
            "The plugin's agent returned an invalid result: {e}"
        ));
    }
    if let Some(report) = &spec.ctx.report_file
        && spec.ctx.contract.report_required()
        && !report.exists()
    {
        return run.fail(format!(
            "The plugin's agent ended without writing its report to {}.",
            report.display()
        ));
    }
    let status = match submit.get("status").and_then(Value::as_str) {
        Some("stuck") => ExecutionStatus::Stuck,
        _ => ExecutionStatus::Ok,
    };
    let mut r = ExecutionResult::with_status(status);
    r.submit = Some(submit);
    r.usage = *run.usage.lock();
    r
}

impl Calls {
    fn over_limit(&self) -> Option<ToolReply> {
        (self.count.fetch_add(1, Ordering::SeqCst) >= MAX_PROGRAM_CALLS).then(|| ToolReply {
            output: format!(
                "Stop and return your result: this run made {MAX_PROGRAM_CALLS} calls, the limit."
            ),
            is_error: true,
        })
    }
}

#[async_trait::async_trait]
impl AgentCalls for Calls {
    async fn tool(&self, name: &str, input: Value) -> ToolReply {
        if let Some(r) = self.over_limit() {
            return r;
        }
        if name.starts_with("submit_") {
            return ToolReply {
                output: "Return your result from run_agent instead of calling the submit tool."
                    .into(),
                is_error: true,
            };
        }
        if !self.tools.iter().any(|t| t == name) {
            return ToolReply {
                output: format!(
                    "This agent has no tool `{name}`. Its tools: {}.",
                    self.tools.join(", ")
                ),
                is_error: true,
            };
        }
        let id = format!("p{}", self.count.load(Ordering::SeqCst));
        let (block, end) = self
            .run
            .tool_end(&id, name, &input, &self.setup.policy, &self.setup.env)
            .await;
        let (mut output, is_error) = match block {
            Block::ToolResult {
                content, is_error, ..
            } => (content, is_error),
            _ => (String::new(), true),
        };
        // Rule SM3: a programmatic agent that pauses waits here, alive, until its message comes.
        if end == RunEnd::Wait {
            match self.run.host.wait_for_wake().await {
                Some(w) => {
                    output.push_str("\n\n");
                    output.push_str(&w.note);
                }
                None => output.push_str("\n\nNo message will come. Continue without one."),
            }
        } else if let Some(w) = self.run.host.take_messages() {
            // Rule SM2: messages queued during the call follow its output.
            output.push_str("\n\n");
            output.push_str(&w.note);
        }
        ToolReply { output, is_error }
    }

    async fn complete(&self, request: CompleteRequest) -> Result<String, String> {
        if self.over_limit().is_some() {
            return Err(format!(
                "This run made {MAX_PROGRAM_CALLS} calls, the limit."
            ));
        }
        let mut req = ChatRequest::new(self.setup.model.clone());
        req.system = vec![SystemBlock::cached(request.system)];
        req.messages = request
            .messages
            .into_iter()
            .map(|m| match m.role.as_str() {
                "assistant" => Message::assistant(vec![Block::text(m.text)]),
                _ => Message::user_text(m.text),
            })
            .collect();
        req.effort = self.run.spec.effort;
        req.max_tokens = request.max_tokens.unwrap_or(crate::DEFAULT_MAX_OUTPUT);
        req.tool_choice = ToolChoice::Auto;
        let coalescer = Arc::new(Coalescer {
            host: self.run.host.clone(),
            buf: Mutex::new((false, String::new())),
        });
        let resp = self
            .run
            .chat(self.setup.provider.as_ref(), req, &coalescer)
            .await
            .map_err(|e| e.to_string())?;
        let mut usage = resp.usage;
        usage.context_tokens = crate::context_of(&usage);
        self.run.add_usage(&usage, vec![]);
        Ok(resp.text())
    }

    fn status(&self, text: &str) {
        self.run.host.emit(ExecutionDelta::Status {
            message: text.to_string(),
        });
    }
}
