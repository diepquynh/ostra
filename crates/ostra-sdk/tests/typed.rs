//! Rule CA5: typed contracts across a real process boundary. The test binary starts itself again
//! as the plugin, with `OSTRA_SDK_TYPED_PLUGIN` set. A typed agent's result and a typed handler's
//! outcome cross the stdio transport as the JSON the engine reads.

use ostra_sdk::contracts::*;
use ostra_sdk::stdio::{StdioPlugin, serve};
use ostra_sdk::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

struct Answerer;

#[async_trait::async_trait]
impl ContractAgent for Answerer {
    type Contract = Answer;
    fn definition(&self) -> PluginAgent {
        PluginAgent::new("answerer", "Answers with the first line of a file.")
    }
    async fn run(
        &self,
        task: AgentTask,
        ctx: AgentContext,
        _: Arc<dyn Checkpoints>,
    ) -> Result<QuickAnswerSubmit, String> {
        let path = format!("{}/a.txt", task.repo_root.display());
        let text = ctx.read(&path).await;
        Ok(QuickAnswerSubmit {
            answer: text.output.lines().next().unwrap_or_default().into(),
            sources: vec![path],
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Count {
    lines: u32,
}

struct Counted;

impl ContractType for Counted {
    type Submit = Count;
    fn contract() -> Contract {
        "typed:count".parse().expect("valid contract name")
    }
    fn schema() -> serde_json::Value {
        json!({"type": "object", "required": ["lines"], "properties": {"lines": {"type": "integer"}}})
    }
}

struct CountHandler;

#[async_trait::async_trait]
impl ResultHandler for CountHandler {
    type Contract = Counted;
    async fn handle(
        &self,
        count: Count,
        _: ResultView,
        _: Arc<dyn Checkpoints>,
    ) -> Result<CustomSubmit, String> {
        Ok(CustomSubmit {
            verdict: StageVerdict::Pass,
            summary: format!("{} lines", count.lines),
            findings: vec![],
            question: None,
            options: vec![],
            report_path: None,
            data: None,
        })
    }
}

struct Typed {
    agents: TypedAgents,
}

#[async_trait::async_trait]
impl Plugin for Typed {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            name: "typed".into(),
            version: "1.0.0".into(),
            agents: self.agents.definitions(),
            contracts: vec![contract_def::<Counted>("A line count.")],
            ..Default::default()
        }
    }

    async fn handle_result(
        &self,
        _: &str,
        result: ResultView,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<CustomSubmit, String> {
        contracts::handle_result(&CountHandler, result, checkpoints).await
    }

    async fn run_agent(
        &self,
        agent: &str,
        task: AgentTask,
        calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<AgentOutcome, String> {
        self.agents
            .run(agent, task, calls, checkpoints)
            .await
            .unwrap_or_else(|| Err(format!("no agent `{agent}`")))
    }
}

#[test]
fn typed_plugin_main() {
    if std::env::var_os("OSTRA_SDK_TYPED_PLUGIN").is_none() {
        return;
    }
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(serve(Arc::new(Typed {
            agents: TypedAgents::new().with(Answerer),
        })));
}

struct Calls;

#[async_trait::async_trait]
impl AgentCalls for Calls {
    async fn tool(&self, _: &str, _: serde_json::Value) -> ToolReply {
        ToolReply {
            output: "first\nsecond".into(),
            is_error: false,
        }
    }
    async fn complete(&self, _: CompleteRequest) -> Result<String, String> {
        Ok(String::new())
    }
    fn status(&self, _: &str) {}
}

#[tokio::test]
async fn typed_agents_and_handlers_cross_the_stdio_boundary() {
    let command = vec![
        std::env::current_exe().unwrap().display().to_string(),
        "--exact".into(),
        "typed_plugin_main".into(),
        "--quiet".into(),
        "--test-threads=1".into(),
    ];
    let dir = tempfile::tempdir().unwrap();
    let plugin = StdioPlugin::start(
        &command,
        &[("OSTRA_SDK_TYPED_PLUGIN".into(), "1".into())],
        dir.path(),
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    let m = plugin.manifest();
    assert_eq!(m.agents[0].returns.as_deref(), Some("answer"));
    assert_eq!(m.contracts[0].name, "count");
    let task = AgentTask {
        execution: "x_1".into(),
        session: None,
        agent: "answerer".into(),
        first_message: String::new(),
        repo_root: "/repo".into(),
        session_dir: "/s".into(),
        workspace_root: "/w".into(),
        model: "mock:m".into(),
        tools: vec!["Read".into()],
        submit_schema: Answer::schema(),
        resumed: false,
    };
    let out = plugin
        .run_agent("answerer", task, Arc::new(Calls), Arc::new(NoCheckpoints))
        .await
        .unwrap();
    let answer = Answer::parse(&out.submit).unwrap();
    assert_eq!(answer.answer, "first");
    let result = ResultView {
        session: "s_1".into(),
        execution: "x_2".into(),
        agent: "counter".into(),
        contract: "count".into(),
        stage: None,
        scope: None,
        submit: json!({"lines": 3}),
    };
    let handled = plugin
        .handle_result("count", result, Arc::new(NoCheckpoints))
        .await
        .unwrap();
    assert_eq!(handled.summary, "3 lines");
}
