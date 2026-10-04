//! An Ostra plugin as its own program: web development with end-to-end browser tests.
//!
//! It offers:
//! - `e2e-tester`, a model agent that writes and runs Playwright tests for the flows a request changes;
//! - `e2e-runner`, a programmatic agent that sends app failures to the implementer, waits for its fix, and
//!   runs the same tests again with no model call;
//! - the stage `e2e`, whose logic loops the two until the tests pass or the user decides;
//! - the contract `e2e`, the tests' result, which this plugin turns into the stage's verdict;
//! - the workflow `web-dev:web-app`: Ostra's implement pipeline with the `e2e` stage, once per project, between
//!   the build and your feedback.
//!
//! Build it with `cargo build -p ostra-sdk --example web_dev`, then register it in a workspace and start a
//! session with the workflow `web-dev:web-app`:
//!
//! ```bash
//! ostra plugin add web-dev -- /path/to/target/debug/examples/web_dev
//! ```
//!
//! The tests start the app on `127.0.0.1` from inside the sandbox, so the workspace's sandbox must allow
//! loopback connections, and installing Playwright needs the package registry and the browser download hosts.

use ostra_sdk::stdio::serve;
use ostra_sdk::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

const PLUGIN: &str = "web-dev";
const TESTER: &str = "e2e-tester";
const RUNNER: &str = "e2e-runner";
const CONTRACT: &str = "e2e";
/// Runs the stage starts before it asks the user, counted from the last answer.
const RUNS_BEFORE_ASK: usize = 4;
const SUMMARIZER: &str = include_str!("summarize.cjs");

/// One result of the `e2e` contract, from either agent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct E2eReport {
    web_app: bool,
    #[serde(default)]
    dir: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    scenarios: Vec<Scenario>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blocker: Option<String>,
    summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Scenario {
    name: String,
    #[serde(default)]
    file: String,
    status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cause: Option<Cause>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Pass,
    Fail,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Cause {
    App,
    Test,
    Environment,
    /// The runner saw the failure but cannot say whose it is.
    Unknown,
}

/// What the stage hands `e2e-runner`, as a JSON block in its instructions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Rerun {
    project: Option<String>,
    dir: String,
    command: String,
    failures: Vec<Scenario>,
}

fn contract_schema() -> Value {
    let scenario = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "file": {"type": "string"},
            "status": {"type": "string", "enum": ["pass", "fail", "skipped"]},
            "error": {"type": "string"},
            "cause": {"type": "string", "enum": ["app", "test", "environment", "unknown"]}
        },
        "required": ["name", "status"]
    });
    json!({
        "type": "object",
        "properties": {
            "web_app": {"type": "boolean", "description": "Whether the project has a web interface."},
            "dir": {"type": "string", "description": "The absolute folder Playwright ran in."},
            "command": {"type": "string", "description": "Runs only these scenarios from `dir`; Playwright options may follow."},
            "scenarios": {"type": "array", "items": scenario},
            "blocker": {"type": "string", "description": "What stopped the suite from running."},
            "summary": {"type": "string"}
        },
        "required": ["web_app", "scenarios", "summary"]
    })
}

struct WebDev;

#[async_trait::async_trait]
impl Plugin for WebDev {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            name: PLUGIN.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Web development with end-to-end browser tests: a Playwright tester, a fix loop with the implementer, and a workflow that runs them after the build.".into(),
            agents: vec![
                PluginAgent::new(
                    TESTER,
                    "Writes and runs Playwright end-to-end tests for the user-visible flows a request adds or changes, and classifies each failure as the app's, the test's, or the environment's.",
                )
                .prompt(include_str!("e2e-tester.md"))
                .tier(Tier::Balanced)
                .effort("native", Effort::High)
                .capabilities([
                    Capability::Read,
                    Capability::Write,
                    Capability::Edit,
                    Capability::Shell,
                    Capability::SearchText,
                    Capability::Glob,
                    Capability::Skill,
                    Capability::MemoryRecall,
                    Capability::TestFiles,
                ])
                .write_scope(WriteScope::Project)
                .brief(["stack", "commands", "testing", "skills", "conventions"])
                .timeout_seconds(3600)
                .returns(format!("{PLUGIN}:{CONTRACT}")),
                PluginAgent::new(
                    RUNNER,
                    "Sends failing end-to-end scenarios to the implementer, waits for its fix, and runs the same Playwright tests again.",
                )
                .tier(Tier::Fast)
                .capabilities([
                    Capability::Read,
                    Capability::Write,
                    Capability::Shell,
                    Capability::Coordinate,
                ])
                .write_scope(WriteScope::Project)
                .timeout_seconds(3600)
                .returns(format!("{PLUGIN}:{CONTRACT}")),
            ],
            stages: vec![PluginStage {
                name: "e2e".into(),
                description: "Runs e2e-tester; sends app failures to the implementer through e2e-runner and test failures back to e2e-tester, then asks you after four runs or when the suite cannot run.".into(),
            }],
            contracts: vec![PluginContractDef {
                name: CONTRACT.into(),
                description: "End-to-end scenarios and their results for one project.".into(),
                schema: contract_schema(),
            }],
            workflows: vec![
                workflow::Workflow::extending("web-app", "ostra:implement")
                    .description("The implement pipeline with Playwright end-to-end tests of each web project between the build and your feedback.")
                    .node(
                        workflow::Node::plugin_stage("e2e", "web-dev:e2e")
                            .after(["build"])
                            .before(["feedback"])
                            .scope(StageScope::Project)
                            // A failure you continue past is recorded and the session goes on.
                            .on_fail(OnFail::Continue, ostra_core::workflow::MAX_STAGE_ROUNDS),
                    )
                    .build(),
            ],
            transforms: vec![],
        }
    }

    async fn decide_stage(
        &self,
        _stage: &str,
        view: StageView,
        _checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<StageDecision, String> {
        Ok(decide(&view))
    }

    /// Rule PL5: the verdict is computed here from the scenarios, never taken from the agent.
    async fn handle_result(
        &self,
        _contract: &str,
        result: ResultView,
        _checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<CustomSubmit, String> {
        let report: E2eReport = serde_json::from_value(result.submit)
            .map_err(|e| format!("the e2e result does not parse: {e}"))?;
        Ok(verdict(report))
    }

    async fn run_agent(
        &self,
        agent: &str,
        task: AgentTask,
        calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<AgentOutcome, String> {
        if agent != RUNNER {
            return Err(format!("no agent `{agent}` runs in code"));
        }
        let report = rerun(&task, &AgentContext::new(calls), checkpoints.as_ref()).await?;
        Ok(AgentOutcome {
            submit: serde_json::to_value(report).map_err(|e| e.to_string())?,
        })
    }
}

/// The stage's outcome from one result.
fn verdict(report: E2eReport) -> CustomSubmit {
    let base = |verdict, summary: String| CustomSubmit {
        verdict,
        summary,
        findings: vec![],
        question: None,
        options: vec![],
        report_path: None,
        data: None,
    };
    let data = serde_json::to_value(&report).ok();
    if !report.web_app {
        return base(StageVerdict::Pass, report.summary);
    }
    if let Some(blocker) = &report.blocker {
        return CustomSubmit {
            question: Some(format!(
                "The end-to-end tests cannot run: {blocker} Fix it and answer retry, or continue without these tests."
            )),
            options: vec!["retry".into(), "continue".into(), "stop".into()],
            data,
            ..base(StageVerdict::NeedsUser, report.summary)
        };
    }
    let failures: Vec<&Scenario> = report
        .scenarios
        .iter()
        .filter(|s| s.status == Status::Fail)
        .collect();
    let passed = report
        .scenarios
        .iter()
        .filter(|s| s.status == Status::Pass)
        .count();
    if failures.is_empty() && passed > 0 {
        return CustomSubmit {
            data,
            ..base(
                StageVerdict::Pass,
                format!("{passed} end-to-end scenarios pass. {}", report.summary),
            )
        };
    }
    let findings = failures
        .iter()
        .map(|s| CustomFinding {
            description: format!(
                "{} fails ({}): {}",
                s.name,
                cause_name(s.cause),
                s.error.as_deref().unwrap_or("no error message")
            ),
            file: Some(s.file.clone()).filter(|f| !f.is_empty()),
            fix: Some(
                match s.cause {
                    Some(Cause::App) => "Change the app so the flow behaves as the request says.",
                    Some(Cause::Test) => "Fix the test's locator, setup, or assertion.",
                    _ => "Read the error and the code the scenario exercises to find whose failure it is.",
                }
                .into(),
            ),
        })
        .collect();
    let summary = if failures.is_empty() {
        format!("No end-to-end scenario ran. {}", report.summary)
    } else {
        format!(
            "{} of {} end-to-end scenarios fail. {}",
            failures.len(),
            failures.len() + passed,
            report.summary
        )
    };
    CustomSubmit {
        findings,
        data,
        ..base(StageVerdict::Fail, summary)
    }
}

fn cause_name(c: Option<Cause>) -> &'static str {
    match c {
        Some(Cause::App) => "app",
        Some(Cause::Test) => "test",
        Some(Cause::Environment) => "environment",
        Some(Cause::Unknown) | None => "unknown",
    }
}

/// Rule PL3: one decision from what the stage did so far.
fn decide(view: &StageView) -> StageDecision {
    let asks = view
        .decisions
        .iter()
        .filter(|d| matches!(d, StageDecision::Ask { .. }))
        .count();
    // Runs since the user last answered, which the ask threshold counts.
    let since_answer = view
        .decisions
        .iter()
        .rev()
        .take_while(|d| !matches!(d, StageDecision::Ask { .. }))
        .filter(|d| matches!(d, StageDecision::Run { .. }))
        .count();
    if matches!(view.decisions.last(), Some(StageDecision::Ask { .. }))
        && view.answers.len() >= asks
    {
        let answer = view.answers.last().map(String::as_str).unwrap_or("retry");
        if answer.starts_with("continue") {
            return StageDecision::Fail {
                summary: "You continued without passing end-to-end tests.".into(),
            };
        }
        return run(
            TESTER,
            Some(format!(
                "The user answered `{answer}` after the last round. Run the end-to-end tests again from where they \
                 stand, and fix what the answer asks for."
            )),
        );
    }
    let Some(last) = view.runs.last() else {
        return run(TESTER, None);
    };
    let handled = last
        .handled
        .clone()
        .and_then(|h| serde_json::from_value::<CustomSubmit>(h).ok());
    let Some(outcome) = handled else {
        let broken = view
            .runs
            .iter()
            .rev()
            .take_while(|r| r.handled.is_none())
            .count();
        if broken < 2 {
            return run(
                &last.agent,
                Some(format!(
                    "The previous run ended without a result ({}). Run the end-to-end tests again.",
                    last.error.as_deref().unwrap_or("no error was recorded")
                )),
            );
        }
        return ask(
            "The end-to-end runs keep ending without a result. Retry, or continue without these tests?".into(),
        );
    };
    match outcome.verdict {
        StageVerdict::Pass => StageDecision::Pass {
            summary: outcome.summary,
        },
        StageVerdict::NeedsUser => StageDecision::Ask {
            question: outcome
                .question
                .unwrap_or_else(|| "The end-to-end tests need a decision.".into()),
            options: outcome.options,
        },
        StageVerdict::Fail if since_answer >= RUNS_BEFORE_ASK => ask(format!(
            "End-to-end tests still fail after {since_answer} runs: {} Retry, or continue without them?",
            outcome.summary
        )),
        StageVerdict::Fail => {
            let report: E2eReport = outcome
                .data
                .and_then(|d| serde_json::from_value(d).ok())
                .unwrap_or_default();
            let failures: Vec<Scenario> = report
                .scenarios
                .into_iter()
                .filter(|s| s.status == Status::Fail)
                .collect();
            let app_only = !failures.is_empty()
                && failures.iter().all(|s| s.cause == Some(Cause::App))
                && !report.command.is_empty();
            if app_only {
                let plan = Rerun {
                    project: view
                        .scope
                        .as_deref()
                        .and_then(|s| s.strip_prefix("project:"))
                        .map(String::from),
                    dir: report.dir,
                    command: report.command,
                    failures,
                };
                return run(
                    RUNNER,
                    Some(format!(
                        "Send these app failures to the implementer, wait for its fix, and run the tests again.\n\n```json\n{}\n```",
                        serde_json::to_string_pretty(&plan).unwrap_or_default()
                    )),
                );
            }
            let list = if failures.is_empty() {
                "No scenario ran or passed. Write scenarios for the request's flows and run them."
                    .to_string()
            } else {
                failures
                    .iter()
                    .map(|s| {
                        format!(
                            "- {} ({}, {}): {}",
                            s.name,
                            s.file,
                            cause_name(s.cause),
                            s.error.as_deref().unwrap_or("no error message")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            run(
                TESTER,
                Some(format!(
                    "The last round failed. Fix the `test` failures, classify the `unknown` ones, and run the tests \
                     again:\n{list}"
                )),
            )
        }
    }
}

fn run(agent: &str, instructions: Option<String>) -> StageDecision {
    StageDecision::Run {
        agent: agent.into(),
        instructions,
    }
}

fn ask(question: String) -> StageDecision {
    StageDecision::Ask {
        question,
        options: vec!["retry".into(), "continue".into(), "stop".into()],
    }
}

/// The last ```json block of the runner's instructions.
fn rerun_plan(first_message: &str) -> Option<Rerun> {
    let start = first_message.rfind("```json")? + "```json".len();
    let end = first_message[start..].find("```")? + start;
    serde_json::from_str(first_message[start..end].trim()).ok()
}

/// The subagent ID of the newest implementer, in `project` when one there exists.
fn implementer(list: &str, project: Option<&str>) -> Option<String> {
    let rows: Vec<(&str, &str)> = list
        .lines()
        .filter_map(|l| l.strip_prefix("- ")?.split_once(": "))
        .filter(|(_, rest)| rest.starts_with("implementer,"))
        .collect();
    let in_project = project.and_then(|p| {
        rows.iter()
            .rev()
            .find(|(_, rest)| rest.contains(&format!(", project {p},")))
    });
    in_project
        .or(rows.last())
        .map(|(id, _)| id.trim().to_string())
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn blocked(plan: &Rerun, blocker: String) -> E2eReport {
    E2eReport {
        web_app: true,
        dir: plan.dir.clone(),
        command: plan.command.clone(),
        scenarios: vec![],
        summary: blocker.clone(),
        blocker: Some(blocker),
    }
}

#[derive(Deserialize)]
struct Summary {
    failed: Vec<Scenario>,
    failed_count: usize,
    passed: Vec<Scenario>,
    passed_count: usize,
    skipped_count: usize,
    errors: Vec<String>,
}

/// `e2e-runner`: message the implementer, wait for its reply, and run the tests again.
async fn rerun(
    task: &AgentTask,
    ctx: &AgentContext,
    checkpoints: &dyn Checkpoints,
) -> Result<E2eReport, String> {
    let Some(plan) = rerun_plan(&task.first_message) else {
        return Err("the instructions carry no rerun plan; the e2e stage starts this agent".into());
    };
    // Rule PL8: where the conversation with the implementer stands survives this program stopping.
    let key = format!("implementer:{}", task.execution);
    match checkpoints
        .get(&key)
        .as_ref()
        .and_then(|v| v["state"].as_str())
    {
        Some("replied") => {}
        Some(_) => {
            ctx.status("Waiting for the implementer's reply");
            ctx.wait_for_message().await;
            checkpoints.save(&key, json!({"state": "replied"})).await?;
        }
        None => {
            let list = ctx.list_agents().await.output;
            let Some(id) = implementer(&list, plan.project.as_deref()) else {
                return Ok(blocked(
                    &plan,
                    "The session has no implementer to fix the app failures.".into(),
                ));
            };
            let failures = plan
                .failures
                .iter()
                .map(|s| {
                    format!(
                        "- {} ({}): {}",
                        s.name,
                        s.file,
                        s.error.as_deref().unwrap_or("no error message")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            checkpoints
                .save(&key, json!({"state": "asked", "to": id}))
                .await?;
            ctx.status(&format!(
                "Asked {id} to fix {} failures",
                plan.failures.len()
            ));
            ctx.send_message(
                &id,
                &format!(
                    "These end-to-end scenarios fail because the app does not behave as the request says. Fix the \
                     app code, leave the tests as they are, and reply to me when you are done.\n{failures}\n\nThe \
                     tests run with `{}` in {}.",
                    plan.command, plan.dir
                ),
                true,
            )
            .await;
            checkpoints
                .save(&key, json!({"state": "replied", "to": id}))
                .await?;
        }
    }

    let dir = task.session_dir.display().to_string();
    let script = format!("{dir}/web-dev-e2e-summary.cjs");
    let report_file = format!("{dir}/web-dev-e2e-{}.json", task.execution);
    let wrote = ctx.write(&script, SUMMARIZER).await;
    if wrote.is_error {
        return Err(format!("could not write the summarizer: {}", wrote.output));
    }
    ctx.status("Running the end-to-end tests");
    let ran = ctx
        .tool(
            "Bash",
            json!({
                "command": format!(
                    // Older Playwright reads the second variable, newer the first.
                    "cd {} && PLAYWRIGHT_JSON_OUTPUT_FILE={report} PLAYWRIGHT_JSON_OUTPUT_NAME={report} {} --reporter=json",
                    sh_quote(&plan.dir),
                    plan.command,
                    report = sh_quote(&report_file),
                ),
                "timeout": 600_000,
            }),
        )
        .await;
    let summarized = ctx
        .bash(&format!(
            "node {} {}",
            sh_quote(&script),
            sh_quote(&report_file)
        ))
        .await;
    let summary: Summary = match serde_json::from_str(summarized.output.trim()) {
        Ok(s) if !summarized.is_error => s,
        _ => {
            let tail: String = {
                let chars: Vec<char> = ran.output.chars().collect();
                chars[chars.len().saturating_sub(1500)..].iter().collect()
            };
            return Ok(blocked(
                &plan,
                format!("The tests wrote no report. The run ended with:\n{tail}"),
            ));
        }
    };
    if summary.failed_count + summary.passed_count == 0 && !summary.errors.is_empty() {
        return Ok(blocked(&plan, summary.errors.join("\n")));
    }
    let scenarios = summary
        .failed
        .into_iter()
        .map(|s| Scenario {
            cause: Some(Cause::Unknown),
            ..s
        })
        .chain(summary.passed)
        .collect();
    Ok(E2eReport {
        web_app: true,
        dir: plan.dir,
        command: plan.command,
        scenarios,
        blocker: None,
        summary: format!(
            "After the implementer's fix: {} pass, {} fail, {} skipped.",
            summary.passed_count, summary.failed_count, summary.skipped_count
        ),
    })
}

#[tokio::main]
async fn main() {
    serve(Arc::new(WebDev)).await;
}

#[cfg(test)]
mod tests;
