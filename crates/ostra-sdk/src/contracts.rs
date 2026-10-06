//! Typed result contracts (Rule CA5): each contract tied to the struct its submit holds, so a plugin
//! fills, reads, and handles results as Rust values, not as JSON.
//!
//! The structs are the ones in `ostra-core`, which the engine and Ostra's own stages read. A
//! programmatic agent that returns [`Review`] fills the same `review` contract as Ostra's code
//! reviewer, and any built-in stage that reads `review` can run it (Rule WF8).
//!
//! - [`ContractType`] ties a contract to its submit struct and its schema. This module has one
//!   type per built-in contract; a plugin implements it for each contract of its own.
//! - [`ContractAgent`] is a programmatic agent that returns a contract's struct. [`TypedAgents`]
//!   lists such agents in the manifest and runs them from [`Plugin::run_agent`](crate::Plugin).
//! - [`SubmitExt`] and [`StageViewExt`] read a run's submit as a contract's struct.
//! - [`ResultHandler`] handles a result of a plugin contract as its struct (Rule PL5).

use crate::{AgentContext, AgentOutcome, AgentTask, PluginAgent, PluginContractDef};
use ostra_core::Contract;
use ostra_core::plugin::{AgentCalls, Checkpoints, ResultView, StageRunView, StageView};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

pub use ostra_core::pipeline::Question;
pub use ostra_core::submit::{
    AdviceAction, AdvisorSubmit, CodeReviewerSubmit, CustomFinding, CustomSubmit, ExploreSubmit,
    FactCheckFinding, FactCheckSubmit, GenerateSpecSubmit, HandoffInfo, ImplementerSubmit,
    InitializerSubmit, PlanPhaseSubmit, PlanSubmit, QuickAnswerSubmit, ReportSubmit, ReviewFinding,
    Severity, StageVerdict, StuckInfo, SubmitStatus, Verdict,
};

/// A result contract and the struct its submit holds.
pub trait ContractType: Send + Sync + 'static {
    type Submit: Serialize + DeserializeOwned + Send + Sync;

    fn contract() -> Contract;

    /// The submit's JSON Schema. A built-in contract's comes from its struct; a plugin contract
    /// gives the schema it declares in its manifest.
    fn schema() -> Value {
        ostra_core::submit::submit_schema(Self::contract())
    }

    /// Read a submit as this contract's struct.
    fn parse(submit: &Value) -> Result<Self::Submit, String> {
        serde_json::from_value(submit.clone())
            .map_err(|e| format!("The submit is not a `{}` result: {e}", Self::contract()))
    }

    /// The submit as JSON, checked as the engine checks it: the contract's own rules, and the
    /// run's schema for a stage's `data` or a plugin contract.
    fn to_submit(submit: &Self::Submit, schema: &Value) -> Result<Value, String> {
        let value = serde_json::to_value(submit).map_err(|e| e.to_string())?;
        ostra_core::submit::validate_submit_with(Self::contract(), schema, &value)?;
        Ok(value)
    }
}

macro_rules! builtin_contract {
    ($(#[$doc:meta] $name:ident => $variant:ident, $submit:ty;)*) => {
        $(
            #[$doc]
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct $name;

            impl ContractType for $name {
                type Submit = $submit;
                fn contract() -> Contract {
                    Contract::$variant
                }
            }
        )*
    };
}

builtin_contract! {
    /// `research`: a research document and its findings.
    Research => Research, ExploreSubmit;
    /// `spec`: a spec document.
    Spec => Spec, GenerateSpecSubmit;
    /// `fact-check`: a verdict and findings on a spec, a plan, or a page.
    FactCheck => FactCheck, FactCheckSubmit;
    /// `plan`: a master plan and its phases.
    Plan => Plan, PlanSubmit;
    /// `implementation`: a phase's code change and its report.
    Implementation => Implementation, ImplementerSubmit;
    /// `review`: the findings of one review loop.
    Review => Review, CodeReviewerSubmit;
    /// `path-analysis`: a test plan from the changed code's execution paths.
    PathAnalysis => PathAnalysis, ReportSubmit;
    /// `tests`: tests and their report.
    Tests => Tests, ReportSubmit;
    /// `prompt`: instruction files and their report.
    Prompt => Prompt, ReportSubmit;
    /// `advice`: what to do with a failed or stuck step.
    Advice => Advice, AdvisorSubmit;
    /// `answer`: an answer to one question.
    Answer => Answer, QuickAnswerSubmit;
    /// `setup`: one step of a project's setup.
    Setup => Setup, InitializerSubmit;
    /// `stage`: a workflow stage's verdict, summary, findings, and `data`.
    Stage => Stage, CustomSubmit;
}

/// The definition of a plugin's own contract `C`, for [`PluginManifest::contracts`](crate::PluginManifest).
pub fn contract_def<C: ContractType>(description: impl Into<String>) -> PluginContractDef {
    let contract = C::contract();
    let name = match contract {
        Contract::Plugin(p) => p.name().to_string(),
        other => other.as_str().to_string(),
    };
    PluginContractDef {
        name,
        description: description.into(),
        schema: C::schema(),
    }
}

/// Builder methods on [`PluginAgent`] that take a contract type.
pub trait PluginAgentExt {
    /// The agent returns contract `C`.
    fn returns_type<C: ContractType>(self) -> Self;
}

impl PluginAgentExt for PluginAgent {
    fn returns_type<C: ContractType>(self) -> Self {
        self.returns(C::contract().as_str())
    }
}

/// Rule PL2: a programmatic agent that fills contract `Contract` with its struct.
#[async_trait::async_trait]
pub trait ContractAgent: Send + Sync {
    type Contract: ContractType;

    /// The agent's name, description, and grants. [`TypedAgents`] sets `returns` from the type.
    fn definition(&self) -> PluginAgent;

    async fn run(
        &self,
        task: AgentTask,
        ctx: AgentContext,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<<Self::Contract as ContractType>::Submit, String>;
}

/// Run a typed agent to the outcome `Plugin::run_agent` returns, checked against the run's schema.
pub async fn run_contract_agent<A: ContractAgent + ?Sized>(
    agent: &A,
    task: AgentTask,
    calls: Arc<dyn AgentCalls>,
    checkpoints: Arc<dyn Checkpoints>,
) -> Result<AgentOutcome, String> {
    let schema = task.submit_schema.clone();
    let submit = agent
        .run(task, AgentContext::new(calls), checkpoints)
        .await?;
    Ok(AgentOutcome {
        submit: A::Contract::to_submit(&submit, &schema)?,
    })
}

#[async_trait::async_trait]
trait ErasedAgent: Send + Sync {
    fn definition(&self) -> PluginAgent;
    async fn run(
        &self,
        task: AgentTask,
        calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<AgentOutcome, String>;
}

struct Erased<A>(A);

#[async_trait::async_trait]
impl<A: ContractAgent> ErasedAgent for Erased<A> {
    fn definition(&self) -> PluginAgent {
        self.0.definition().returns_type::<A::Contract>()
    }

    async fn run(
        &self,
        task: AgentTask,
        calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<AgentOutcome, String> {
        run_contract_agent(&self.0, task, calls, checkpoints).await
    }
}

/// A plugin's typed programmatic agents, by name.
#[derive(Clone, Default)]
pub struct TypedAgents {
    agents: BTreeMap<String, Arc<dyn ErasedAgent>>,
}

impl TypedAgents {
    pub fn new() -> Self {
        TypedAgents::default()
    }

    /// Add an agent. A later agent of the same name replaces the earlier one.
    pub fn with<A: ContractAgent + 'static>(mut self, agent: A) -> Self {
        let name = agent.definition().name;
        self.agents.insert(name, Arc::new(Erased(agent)));
        self
    }

    /// Each agent's definition with `returns` set, for the manifest's `agents`.
    pub fn definitions(&self) -> Vec<PluginAgent> {
        self.agents.values().map(|a| a.definition()).collect()
    }

    /// Run agent `name`, or `None` when no typed agent has that name.
    pub async fn run(
        &self,
        name: &str,
        task: AgentTask,
        calls: Arc<dyn AgentCalls>,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Option<Result<AgentOutcome, String>> {
        let agent = self.agents.get(name)?.clone();
        Some(agent.run(task, calls, checkpoints).await)
    }
}

/// Read a submit as a contract's struct.
pub trait SubmitExt {
    /// The submit as `C`'s struct: `None` when there is no submit, an error when it is another shape.
    fn submit_as<C: ContractType>(&self) -> Option<Result<C::Submit, String>>;
}

impl SubmitExt for StageRunView {
    fn submit_as<C: ContractType>(&self) -> Option<Result<C::Submit, String>> {
        self.submit.as_ref().map(C::parse)
    }
}

impl SubmitExt for ResultView {
    fn submit_as<C: ContractType>(&self) -> Option<Result<C::Submit, String>> {
        Some(C::parse(&self.submit))
    }
}

/// Rule PL5: what a plugin's handler made of a run's result, as the stage outcome it returned.
pub fn handled(run: &StageRunView) -> Option<CustomSubmit> {
    run.handled
        .as_ref()
        .and_then(|h| serde_json::from_value(h.clone()).ok())
}

/// Read a plugin stage's runs as a contract's structs.
pub trait StageViewExt {
    /// Each run whose submit reads as `C`'s struct, oldest first.
    fn submits_of<C: ContractType>(&self) -> Vec<(&StageRunView, C::Submit)>;

    /// The newest run whose submit reads as `C`'s struct.
    fn last_submit_of<C: ContractType>(&self) -> Option<(&StageRunView, C::Submit)>;
}

impl StageViewExt for StageView {
    fn submits_of<C: ContractType>(&self) -> Vec<(&StageRunView, C::Submit)> {
        self.runs
            .iter()
            .filter_map(|r| Some((r, r.submit_as::<C>()?.ok()?)))
            .collect()
    }

    fn last_submit_of<C: ContractType>(&self) -> Option<(&StageRunView, C::Submit)> {
        self.runs
            .iter()
            .rev()
            .find_map(|r| Some((r, r.submit_as::<C>()?.ok()?)))
    }
}

/// Rule PL5: a handler for the results of a plugin's own contract, which reads each as its struct.
#[async_trait::async_trait]
pub trait ResultHandler: Send + Sync {
    type Contract: ContractType;

    async fn handle(
        &self,
        submit: <Self::Contract as ContractType>::Submit,
        result: ResultView,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<CustomSubmit, String>;
}

/// Parse a result as the handler's contract and handle it, for `Plugin::handle_result`.
pub async fn handle_result<H: ResultHandler + ?Sized>(
    handler: &H,
    result: ResultView,
    checkpoints: Arc<dyn Checkpoints>,
) -> Result<CustomSubmit, String> {
    let submit = H::Contract::parse(&result.submit)?;
    handler.handle(submit, result, checkpoints).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompleteRequest, NoCheckpoints, ToolReply};
    use serde::Deserialize;
    use serde_json::json;

    struct NoCalls;

    #[async_trait::async_trait]
    impl AgentCalls for NoCalls {
        async fn tool(&self, _: &str, _: Value) -> ToolReply {
            ToolReply {
                output: String::new(),
                is_error: true,
            }
        }
        async fn complete(&self, _: CompleteRequest) -> Result<String, String> {
            Err("no model".into())
        }
        fn status(&self, _: &str) {}
    }

    fn task(agent: &str, schema: Value) -> AgentTask {
        AgentTask {
            execution: "x_1".into(),
            session: None,
            agent: agent.into(),
            first_message: String::new(),
            repo_root: "/repo".into(),
            session_dir: "/s".into(),
            workspace_root: "/w".into(),
            model: "mock:m".into(),
            tools: vec![],
            submit_schema: schema,
            resumed: false,
        }
    }

    struct Reviewer {
        blocker: bool,
        security_block: bool,
    }

    #[async_trait::async_trait]
    impl ContractAgent for Reviewer {
        type Contract = Review;
        fn definition(&self) -> PluginAgent {
            PluginAgent::new("typed-reviewer", "Reviews in code.")
        }
        async fn run(
            &self,
            _: AgentTask,
            _: AgentContext,
            _: Arc<dyn Checkpoints>,
        ) -> Result<CodeReviewerSubmit, String> {
            Ok(CodeReviewerSubmit {
                findings: if self.blocker {
                    vec![ReviewFinding {
                        severity: Severity::Blocker,
                        file: "src/a.rs".into(),
                        rule: "SEC-BLOCK-1".into(),
                        description: "Secret in code.".into(),
                        fix: "Read it from the environment.".into(),
                        guidance: None,
                    }]
                } else {
                    vec![]
                },
                security_block: self.security_block,
                ledger_path: "/s/ledger.md".into(),
                summary: "Reviewed.".into(),
            })
        }
    }

    #[test]
    fn every_builtin_contract_but_documentation_has_a_type() {
        let typed = [
            Research::contract(),
            Spec::contract(),
            FactCheck::contract(),
            Plan::contract(),
            Implementation::contract(),
            Review::contract(),
            PathAnalysis::contract(),
            Tests::contract(),
            Prompt::contract(),
            Advice::contract(),
            Answer::contract(),
            Setup::contract(),
            Stage::contract(),
        ];
        for c in Contract::BUILTIN {
            assert_eq!(
                typed.contains(&c),
                c != Contract::Documentation,
                "contract {c}"
            );
        }
        assert_eq!(Review::schema()["type"], "object");
    }

    #[tokio::test]
    async fn a_typed_agent_returns_its_contract_and_is_checked_like_a_model_run() {
        let agents = TypedAgents::new().with(Reviewer {
            blocker: true,
            security_block: true,
        });
        let defs = agents.definitions();
        assert_eq!(defs[0].returns.as_deref(), Some("review"));
        let out = agents
            .run(
                "typed-reviewer",
                task("typed-reviewer", Review::schema()),
                Arc::new(NoCalls),
                Arc::new(NoCheckpoints),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.submit["findings"][0]["severity"], "BLOCKER");
        assert!(
            agents
                .run(
                    "other",
                    task("other", Value::Null),
                    Arc::new(NoCalls),
                    Arc::new(NoCheckpoints)
                )
                .await
                .is_none()
        );
        // The engine's own check runs before the result leaves the plugin.
        let err = run_contract_agent(
            &Reviewer {
                blocker: true,
                security_block: false,
            },
            task("typed-reviewer", Value::Null),
            Arc::new(NoCalls),
            Arc::new(NoCheckpoints),
        )
        .await
        .unwrap_err();
        assert!(err.contains("security_block"), "{err}");
    }

    #[test]
    fn stage_views_read_runs_as_contract_structs() {
        let view: StageView = serde_json::from_value(json!({
            "session": "s_1", "node": "n", "stage": "s", "scope": null, "request": "r",
            "workspace_root": "/w", "projects": [], "instructions": null, "decisions": [],
            "runs": [
                {"execution": "x_1", "agent": "code-reviewer", "status": "ok",
                 "submit": {"findings": [], "security_block": false, "ledger_path": "/l", "summary": "first"}},
                {"execution": "x_2", "agent": "a", "status": "ok",
                 "submit": {"verdict": "pass", "summary": "stage"},
                 "handled": {"verdict": "fail", "summary": "handled"}},
                {"execution": "x_3", "agent": "code-reviewer", "status": "ok",
                 "submit": {"findings": [], "security_block": false, "ledger_path": "/l", "summary": "second"}}
            ],
            "answers": [], "earlier_stages": [], "spec_file": null, "master_plan": null
        }))
        .unwrap();
        let reviews = view.submits_of::<Review>();
        assert_eq!(reviews.len(), 2);
        assert_eq!(view.last_submit_of::<Review>().unwrap().1.summary, "second");
        assert_eq!(
            view.last_submit_of::<Stage>().unwrap().0.execution.as_str(),
            "x_2"
        );
        assert!(view.runs[1].submit_as::<Review>().unwrap().is_err());
        assert_eq!(handled(&view.runs[1]).unwrap().verdict, StageVerdict::Fail);
    }

    #[derive(Serialize, Deserialize)]
    struct Note {
        text: String,
    }

    struct ReleaseNote;

    impl ContractType for ReleaseNote {
        type Submit = Note;
        fn contract() -> Contract {
            "acme:release-note".parse().expect("valid contract name")
        }
        fn schema() -> Value {
            json!({"type": "object", "required": ["text"], "properties": {"text": {"type": "string"}}})
        }
    }

    struct NoteHandler;

    #[async_trait::async_trait]
    impl ResultHandler for NoteHandler {
        type Contract = ReleaseNote;
        async fn handle(
            &self,
            submit: Note,
            _: ResultView,
            _: Arc<dyn Checkpoints>,
        ) -> Result<CustomSubmit, String> {
            Ok(CustomSubmit {
                verdict: if submit.text.is_empty() {
                    StageVerdict::Fail
                } else {
                    StageVerdict::Pass
                },
                summary: submit.text,
                findings: vec![],
                question: None,
                options: vec![],
                report_path: None,
                data: None,
            })
        }
    }

    #[tokio::test]
    async fn plugin_contracts_are_declared_checked_and_handled_as_structs() {
        let def = contract_def::<ReleaseNote>("A release note.");
        assert_eq!(def.name, "release-note");
        assert_eq!(def.schema["required"][0], "text");
        let agent = PluginAgent::new("writer", "Writes notes.").returns_type::<ReleaseNote>();
        assert_eq!(agent.returns.as_deref(), Some("acme:release-note"));
        assert!(
            ReleaseNote::to_submit(&Note { text: "v2".into() }, &ReleaseNote::schema()).is_ok()
        );
        let view = |submit: Value| ResultView {
            session: "s_1".into(),
            execution: "x_1".into(),
            agent: "writer".into(),
            contract: "release-note".into(),
            stage: None,
            scope: None,
            submit,
        };
        let out = handle_result(
            &NoteHandler,
            view(json!({"text": "v2"})),
            Arc::new(NoCheckpoints),
        )
        .await
        .unwrap();
        assert_eq!(
            (out.verdict, out.summary.as_str()),
            (StageVerdict::Pass, "v2")
        );
        let err = handle_result(
            &NoteHandler,
            view(json!({"txt": 1})),
            Arc::new(NoCheckpoints),
        )
        .await
        .unwrap_err();
        assert!(err.contains("acme:release-note"), "{err}");
    }
}
