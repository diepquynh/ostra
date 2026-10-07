//! Rule PL1: a plugin program over real stdio. The test binary starts itself again as the plugin,
//! with `OSTRA_SDK_PLUGIN` set, so the transport crosses a process boundary as it does in Ostra.
//! Rule PL8: a program that exits mid-run leaves its checkpoints behind for the next one.

use ostra_sdk::stdio::{StdioPlugin, serve};
use ostra_sdk::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

struct Gate;

#[async_trait::async_trait]
impl Plugin for Gate {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            workflows: vec![
                workflow::Workflow::extending("checked", "ostra:research")
                    .node(workflow::Node::plugin_stage("gate", "gate:release"))
                    .build(),
            ],
            transforms: vec![
                workflow::TransformFn::new("double", "Twice a number.")
                    .input("value", ValueKind::Number, "The number.")
                    .output(ValueKind::Number)
                    .build(),
            ],
            name: "gate".into(),
            version: "1.0.0".into(),
            agents: vec![
                PluginAgent::new("counter", "Counts the lines of a file."),
                PluginAgent::new("crasher", "Exits on its first start, then passes."),
            ],
            stages: vec![PluginStage {
                name: "release".into(),
                description: "Passes once a run passed.".into(),
            }],
            ..Default::default()
        }
    }

    async fn decide_stage(
        &self,
        _: &str,
        view: StageView,
        _: Arc<dyn Checkpoints>,
    ) -> Result<StageDecision, String> {
        Ok(if view.runs.is_empty() {
            StageDecision::Run {
                agent: "counter".into(),
                instructions: None,
            }
        } else {
            StageDecision::Pass {
                summary: format!("{} runs", view.runs.len()),
            }
        })
    }

    async fn transform(
        &self,
        name: &str,
        inputs: serde_json::Map<String, serde_json::Value>,
        _: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        assert_eq!(name, "double");
        let n = inputs["value"].as_f64().ok_or("value is not a number")?;
        Ok(json!(n * 2.0))
    }

    async fn run_agent(
        &self,
        _: &str,
        task: AgentTask,
        calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<AgentOutcome, String> {
        if task.agent == "crasher" {
            return match checkpoints.get("progress") {
                None => {
                    checkpoints.save("progress", json!("read a.txt")).await?;
                    std::process::exit(7)
                }
                Some(p) => Ok(pass(format!("resumed={} at {p}", task.resumed))),
            };
        }
        let ctx = AgentContext::new(calls);
        let file = ctx
            .read(&format!("{}/a.txt", task.repo_root.display()))
            .await;
        Ok(pass(format!("{} lines", file.output.lines().count())))
    }
}

#[test]
fn plugin_main() {
    if std::env::var_os("OSTRA_SDK_PLUGIN").is_none() {
        return;
    }
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(serve(Arc::new(Gate)));
}

struct Calls;

#[async_trait::async_trait]
impl AgentCalls for Calls {
    async fn tool(&self, name: &str, input: serde_json::Value) -> ToolReply {
        assert_eq!(name, "Read");
        assert_eq!(input["file_path"], "/repo/a.txt");
        ToolReply {
            output: "one\ntwo\nthree".into(),
            is_error: false,
        }
    }
    async fn complete(&self, _: CompleteRequest) -> Result<String, String> {
        Ok(String::new())
    }
    fn status(&self, _: &str) {}
}

#[tokio::test]
async fn a_plugin_program_serves_its_manifest_stages_and_agents() {
    let exe = std::env::current_exe().unwrap();
    let command = vec![
        exe.display().to_string(),
        "--exact".into(),
        "plugin_main".into(),
        "--quiet".into(),
        "--test-threads=1".into(),
    ];
    let dir = tempfile::tempdir().unwrap();
    let plugin = StdioPlugin::start(
        &command,
        &[("OSTRA_SDK_PLUGIN".into(), "1".into())],
        dir.path(),
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    let m = plugin.manifest();
    assert_eq!(
        (m.name.as_str(), m.agents.len(), m.stages.len()),
        ("gate", 2, 1)
    );
    // Rules PL6 and PL7: workflows and transforms built in code cross the boundary too.
    assert_eq!(m.workflows[0].name, "checked");
    assert_eq!(m.transforms[0].output, ValueKind::Number);
    let doubled = plugin
        .transform(
            "double",
            json!({"value": 21}).as_object().cloned().unwrap(),
            serde_json::Map::new(),
        )
        .await
        .unwrap();
    assert_eq!(doubled, json!(42.0));
    let view: StageView = serde_json::from_value(json!({
        "session": "s_1", "node": "release-gate", "stage": "release", "scope": null,
        "request": "r", "workspace_root": "/w", "projects": [], "instructions": null,
        "decisions": [], "runs": [], "answers": [], "earlier_stages": [],
        "spec_file": null, "master_plan": null
    }))
    .unwrap();
    let d = plugin
        .decide_stage("release", view, Arc::new(NoCheckpoints))
        .await
        .unwrap();
    assert!(matches!(d, StageDecision::Run { ref agent, .. } if agent == "counter"));
    let task = AgentTask {
        execution: "x_1".into(),
        session: None,
        agent: "counter".into(),
        first_message: String::new(),
        repo_root: "/repo".into(),
        session_dir: "/s".into(),
        workspace_root: "/w".into(),
        model: "mock:m".into(),
        tools: vec!["Read".into()],
        submit_schema: serde_json::Value::Null,
        resumed: false,
    };
    let out = plugin
        .run_agent("counter", task, Arc::new(Calls), Arc::new(NoCheckpoints))
        .await
        .unwrap();
    assert_eq!(out.submit["summary"], "3 lines");
    assert_eq!(out.submit["verdict"], "pass");
    drop(plugin);
}

fn plugin_command() -> Vec<String> {
    vec![
        std::env::current_exe().unwrap().display().to_string(),
        "--exact".into(),
        "plugin_main".into(),
        "--quiet".into(),
        "--test-threads=1".into(),
    ]
}

async fn start(dir: &std::path::Path) -> StdioPlugin {
    StdioPlugin::start(
        &plugin_command(),
        &[("OSTRA_SDK_PLUGIN".into(), "1".into())],
        dir,
        Duration::from_secs(20),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn pl8_a_program_that_exits_mid_run_leaves_its_checkpoint_for_the_next() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryCheckpoints::default());
    let task = |resumed| AgentTask {
        execution: "x_1".into(),
        session: None,
        agent: "crasher".into(),
        first_message: String::new(),
        repo_root: "/repo".into(),
        session_dir: "/s".into(),
        workspace_root: "/w".into(),
        model: "mock:m".into(),
        tools: vec![],
        submit_schema: serde_json::Value::Null,
        resumed,
    };
    let first = start(dir.path()).await;
    let err = first
        .run_agent("crasher", task(false), Arc::new(Calls), store.clone())
        .await
        .unwrap_err();
    assert!(err.contains("closed"), "{err}");
    assert!(!first.is_alive());
    assert_eq!(store.get("progress"), Some(json!("read a.txt")));

    let second = start(dir.path()).await;
    let out = second
        .run_agent("crasher", task(true), Arc::new(Calls), store.clone())
        .await
        .unwrap();
    assert_eq!(out.submit["summary"], "resumed=true at \"read a.txt\"");
}
