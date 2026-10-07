//! An Ostra plugin as its own program: a release gate for workflows.
//!
//! It offers one programmatic agent, `changelog-check`, one stage, `release`, one transform
//! function, `bump`, and a workflow built in code, `release-gate:implement-and-release`, which a
//! session can name as it is. In a workflow of the workspace's own:
//!
//! ```toml
//! # <workspace>/.ostra/workflows/release.toml
//! extends = "implement"
//!
//! [[stage]]
//! id = "release-gate"
//! plugin = "release-gate:release"
//! after = ["build"]
//! before = ["closing"]
//! ```
//!
//! And in `<workspace>/.ostra/workspace.toml`:
//!
//! ```toml
//! [[plugins]]
//! name = "release-gate"
//! command = ["/path/to/release_gate"]
//! ```
//!
//! Build it with `cargo build -p ostra-sdk --example release_gate`.

use ostra_sdk::stdio::serve;
use ostra_sdk::*;
use std::sync::Arc;

struct ReleaseGate;

#[async_trait::async_trait]
impl Plugin for ReleaseGate {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            workflows: vec![
                workflow::Workflow::extending("implement-and-release", "ostra:implement")
                    .description("The implement pipeline, then the release gate and the next version.")
                    .node(
                        workflow::Node::plugin_stage("gate", "release-gate:release")
                            .after(["build"])
                            .before(["closing"]),
                    )
                    .node(
                        workflow::Node::transform("version", "release-gate:bump")
                            .after(["gate"])
                            .before(["closing"])
                            .arg("current", serde_json::json!("1.4.2"))
                            .arg("part", serde_json::json!("minor")),
                    )
                    .build(),
            ],
            transforms: vec![
                workflow::TransformFn::new("bump", "The next semantic version.")
                    .arg("current", ValueKind::String, "The version now, such as `1.4.2`.")
                    .optional_arg("part", ValueKind::String, "`major`, `minor`, or `patch` (the default).")
                    .output(ValueKind::String)
                    .build(),
            ],
            name: "release-gate".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Checks that each project's changelog describes the change.".into(),
            agents: vec![PluginAgent::new(
                "changelog-check",
                "Reads CHANGELOG.md and the change, and fails when the changelog does not describe it.",
            )
            .tier(Tier::Fast)
            .capabilities([Capability::Read, Capability::Shell, Capability::Coordinate])
            .write_scope(WriteScope::ReadOnly)
            .timeout_seconds(600)],
            stages: vec![PluginStage {
                name: "release".into(),
                description: "Runs changelog-check; after a failure asks the implementer to fix the changelog and checks again once.".into(),
            }],
            contracts: vec![],
        }
    }

    /// Rule PL7: transform functions run here, in code.
    async fn transform(
        &self,
        name: &str,
        _inputs: serde_json::Map<String, serde_json::Value>,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        if name != "bump" {
            return Err(format!("no transform `{name}`"));
        }
        let current = args["current"].as_str().ok_or("`current` is missing")?;
        let mut parts: Vec<u64> = current
            .split('.')
            .map(|p| {
                p.parse()
                    .map_err(|_| format!("`{current}` is not a version"))
            })
            .collect::<Result<_, _>>()?;
        if parts.len() != 3 {
            return Err(format!("`{current}` is not a version such as 1.4.2"));
        }
        match args.get("part").and_then(|v| v.as_str()).unwrap_or("patch") {
            "major" => parts = vec![parts[0] + 1, 0, 0],
            "minor" => parts = vec![parts[0], parts[1] + 1, 0],
            _ => parts[2] += 1,
        }
        Ok(serde_json::json!(format!(
            "{}.{}.{}",
            parts[0], parts[1], parts[2]
        )))
    }

    /// One decision at a time: run the check, then pass, retry once, or give up.
    async fn decide_stage(
        &self,
        _stage: &str,
        view: StageView,
        _checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<StageDecision, String> {
        let passed = |r: &StageRunView| r.submit.as_ref().is_some_and(|s| s["verdict"] == "pass");
        Ok(match view.runs.as_slice() {
            [] => StageDecision::Run {
                agent: "changelog-check".into(),
                instructions: None,
            },
            [.., last] if passed(last) => StageDecision::Pass {
                summary: "The changelog describes the change.".into(),
            },
            [_] => StageDecision::Run {
                agent: "changelog-check".into(),
                instructions: Some(
                    "The first check failed. Message the implementer of the change to fix the changelog, wait for its reply, then check again.".into(),
                ),
            },
            _ => StageDecision::Ask {
                question: "The changelog still does not describe the change. Release anyway?"
                    .into(),
                options: vec!["no".into(), "yes".into()],
            },
        })
    }

    async fn run_agent(
        &self,
        _agent: &str,
        task: AgentTask,
        calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<AgentOutcome, String> {
        let ctx = AgentContext::new(calls);
        let repo = task.repo_root.display().to_string();
        let changelog = ctx.read(&format!("{repo}/CHANGELOG.md")).await;
        if changelog.is_error {
            return Ok(fail(
                "The project has no CHANGELOG.md.",
                vec![CustomFinding {
                    description: "CHANGELOG.md is missing.".into(),
                    file: Some("CHANGELOG.md".into()),
                    fix: Some("Add a CHANGELOG.md with an entry for this change.".into()),
                }],
            ));
        }
        let diff = ctx.bash("git diff --stat HEAD").await;
        // The checkpoint survives this program stopping, so a restarted run does not ask twice.
        let asked = format!("asked:{}", task.execution);
        if task.first_message.contains("Message the implementer")
            && checkpoints.get(&asked).is_none()
        {
            // Find the implementer and wait for its reply before checking again.
            let agents = ctx.list_agents().await.output;
            if let Some(id) = agents
                .lines()
                .find(|l| l.contains("implementer"))
                .and_then(|l| l.trim_start_matches("- ").split(':').next())
            {
                checkpoints
                    .save(&asked, serde_json::json!(id.trim()))
                    .await?;
                ctx.send_message(
                    id.trim(),
                    "CHANGELOG.md does not describe your change. Add an entry for it, then reply to me.",
                    true,
                )
                .await;
            }
        }
        let verdict = ctx
            .complete(
                "You check changelogs. Answer PASS when the changelog's newest entry describes the change, else FAIL and one sentence why.",
                &format!(
                    "Changelog:\n{}\n\nChange:\n{}",
                    changelog.output, diff.output
                ),
            )
            .await?;
        Ok(if verdict.trim_start().starts_with("PASS") {
            pass(verdict)
        } else {
            fail(verdict, vec![])
        })
    }
}

#[tokio::main]
async fn main() {
    serve(Arc::new(ReleaseGate)).await;
}
