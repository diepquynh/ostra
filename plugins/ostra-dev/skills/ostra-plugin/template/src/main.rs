//! An Ostra plugin program. Ostra starts it, speaks JSON-RPC on its stdin and stdout, and calls
//! the methods of `Plugin`. Write logs to stderr only, because stdout carries the protocol.

use ostra_sdk::stdio::serve;
use ostra_sdk::*;
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// Must equal `name` in the `[[plugins]]` entry of the workspace.
const NAME: &str = "PLUGIN_NAME";
const CHECKER: &str = "check";
const WRITER: &str = "writer";
const STAGE: &str = "gate";

struct MyPlugin;

#[async_trait::async_trait]
impl Plugin for MyPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            name: NAME.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Checks the change before the closing stages.".into(),
            agents: vec![
                // No prompt: a programmatic agent. `run_agent` does its work.
                PluginAgent::new(CHECKER, "Runs the project checks and fails when one fails.")
                    .tier(Tier::Fast)
                    .capabilities([Capability::Read, Capability::Shell])
                    .write_scope(WriteScope::ReadOnly)
                    .timeout_seconds(600),
                // A prompt: a model runs it, the same as a Markdown agent.
                PluginAgent::new(WRITER, "Fixes the problems that the check found.")
                    .prompt(include_str!("writer.md"))
                    .tier(Tier::Balanced)
                    .capabilities([
                        Capability::Read,
                        Capability::Edit,
                        Capability::Shell,
                        Capability::SearchText,
                        Capability::Glob,
                    ])
                    .write_scope(WriteScope::Project),
            ],
            stages: vec![PluginStage {
                name: STAGE.into(),
                description: "Runs the check. After a failure, runs the writer and checks again."
                    .into(),
            }],
            workflows: vec![
                workflow::Workflow::extending("implement-checked", "ostra:implement")
                    .description("The implement pipeline with the gate before the closing stages.")
                    .node(
                        workflow::Node::plugin_stage("gate", &format!("{NAME}:{STAGE}"))
                            .after(["build"])
                            .before(["closing"])
                            .scope(StageScope::Project)
                            .on_fail(OnFail::Gate, 6),
                    )
                    .build(),
            ],
            transforms: vec![
                workflow::TransformFn::new("count-failed", "The number of failed findings.")
                    .input(
                        "findings",
                        ValueKind::Array,
                        "Findings from an earlier node.",
                    )
                    .output(ValueKind::Number)
                    .build(),
            ],
            contracts: vec![],
        }
    }

    // Rule PL3: one decision at a time. Ostra records each decision, so this function sees every
    // earlier run in `view.runs` and needs no state of its own.
    async fn decide_stage(
        &self,
        _stage: &str,
        view: StageView,
        _checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<StageDecision, String> {
        Ok(decide(&view))
    }

    async fn run_agent(
        &self,
        agent: &str,
        task: AgentTask,
        calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<AgentOutcome, String> {
        match agent {
            CHECKER => run_check(task, AgentContext::new(calls), checkpoints).await,
            other => Err(format!("This plugin runs no agent `{other}` in code.")),
        }
    }

    // Rule PL7: Ostra records the output, so a restart does not call this again.
    async fn transform(
        &self,
        name: &str,
        inputs: Map<String, Value>,
        _args: Map<String, Value>,
    ) -> Result<Value, String> {
        match name {
            "count-failed" => {
                let findings = inputs["findings"]
                    .as_array()
                    .ok_or("`findings` is not a list")?;
                Ok(json!(findings.len()))
            }
            other => Err(format!("This plugin has no transform `{other}`.")),
        }
    }
}

fn passed(run: &StageRunView) -> bool {
    run.submit.as_ref().is_some_and(|s| s["verdict"] == "pass")
}

fn decide(view: &StageView) -> StageDecision {
    let Some(last) = view.runs.last() else {
        return run(CHECKER, None);
    };
    if last.agent == CHECKER && passed(last) {
        return StageDecision::Pass {
            summary: "The project checks pass.".into(),
        };
    }
    if view.runs.len() >= 5 {
        return StageDecision::Ask {
            question: "The checks still fail after two fixes. Continue without them?".into(),
            options: vec!["no".into(), "yes".into()],
        };
    }
    if last.agent == CHECKER {
        let summary = last
            .submit
            .as_ref()
            .map(|s| s["summary"].to_string())
            .unwrap_or_default();
        return run(WRITER, Some(format!("Fix these check failures: {summary}")));
    }
    run(CHECKER, None)
}

fn run(agent: &str, instructions: Option<String>) -> StageDecision {
    StageDecision::Run {
        agent: agent.into(),
        instructions,
    }
}

async fn run_check(
    task: AgentTask,
    ctx: AgentContext,
    _checkpoints: Arc<dyn Checkpoints>,
) -> Result<AgentOutcome, String> {
    ctx.status("Running the project checks");
    let repo = task.repo_root.display().to_string();
    // Every tool call goes through the policy and the sandbox of Ostra.
    let out = ctx.bash(&format!("cd {repo} && make check")).await;
    if out.is_error {
        return Ok(fail(
            "The project checks failed.",
            vec![CustomFinding {
                description: out.output.chars().take(2000).collect(),
                file: None,
                fix: Some("Fix the failing checks.".into()),
            }],
        ));
    }
    Ok(pass("The project checks pass."))
}

#[tokio::main]
async fn main() {
    serve(Arc::new(MyPlugin)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(runs: Vec<StageRunView>) -> StageView {
        serde_json::from_value(json!({
            "session": "s_1", "node": "gate", "stage": STAGE, "scope": "project:app",
            "request": "r", "workspace_root": "/ws", "projects": [["app", "/ws/app"]],
            "instructions": null, "decisions": [], "runs": runs, "answers": [],
            "earlier_stages": [], "spec_file": null, "master_plan": null
        }))
        .unwrap()
    }

    fn ran(agent: &str, verdict: &str) -> StageRunView {
        serde_json::from_value(json!({
            "execution": "x_1", "agent": agent, "status": "ok",
            "submit": {"verdict": verdict, "summary": "s"}
        }))
        .unwrap()
    }

    #[test]
    fn the_first_decision_runs_the_check() {
        assert!(
            matches!(decide(&view(vec![])), StageDecision::Run { agent, .. } if agent == CHECKER)
        );
    }

    #[test]
    fn a_failed_check_runs_the_writer_and_a_passed_check_passes() {
        let d = decide(&view(vec![ran(CHECKER, "fail")]));
        assert!(matches!(d, StageDecision::Run { agent, .. } if agent == WRITER));
        let d = decide(&view(vec![
            ran(CHECKER, "fail"),
            ran(WRITER, "pass"),
            ran(CHECKER, "pass"),
        ]));
        assert!(matches!(d, StageDecision::Pass { .. }));
    }

    #[tokio::test]
    async fn the_transform_counts_findings() {
        let mut inputs = Map::new();
        inputs.insert("findings".into(), json!([{}, {}]));
        let out = MyPlugin
            .transform("count-failed", inputs, Map::new())
            .await
            .unwrap();
        assert_eq!(out, json!(2));
    }
}
