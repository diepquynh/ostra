//! An Ostra plugin that works with typed results: a programmatic reviewer that fills Ostra's
//! `review` contract, a contract of its own, `typed-review:release-note`, with a typed handler, and
//! a stage that reads its runs as structs.
//!
//! A workflow binds the reviewer in Ostra's build stage like any agent that returns `review`
//! (Rule WF8):
//!
//! ```toml
//! # <workspace>/.ostra/workflows/strict.toml
//! extends = "implement"
//!
//! [agents]
//! review = "todo-reviewer"
//!
//! [[stage]]
//! id = "notes"
//! plugin = "typed-review:notes"
//! after = ["build"]
//! before = ["closing"]
//! ```
//!
//! Build it with `cargo build -p ostra-sdk --example typed_review`.

use ostra_sdk::contracts::*;
use ostra_sdk::stdio::serve;
use ostra_sdk::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Fails a change that leaves a `TODO` in a changed file.
struct TodoReviewer;

#[async_trait::async_trait]
impl ContractAgent for TodoReviewer {
    type Contract = Review;

    fn definition(&self) -> PluginAgent {
        PluginAgent::new(
            "todo-reviewer",
            "Reviews a phase's change and reports each TODO left in a changed file.",
        )
        .capabilities([Capability::Read, Capability::Shell])
        .write_scope(WriteScope::Session)
    }

    async fn run(
        &self,
        task: AgentTask,
        ctx: AgentContext,
        _: Arc<dyn Checkpoints>,
    ) -> Result<CodeReviewerSubmit, String> {
        let listing = ctx
            .bash("git diff --name-only HEAD && git ls-files --others --exclude-standard")
            .await;
        let mut findings = vec![];
        for file in listing.output.lines().filter(|l| !l.trim().is_empty()) {
            let path = format!("{}/{file}", task.repo_root.display());
            let text = ctx.read(&path).await;
            for (n, line) in text.output.lines().enumerate() {
                if line.contains("TODO") {
                    findings.push(ReviewFinding {
                        severity: Severity::Medium,
                        file: file.to_string(),
                        rule: "TODO-LEFT".into(),
                        description: format!("A TODO is left on line {}.", n + 1),
                        fix: "Finish the work or remove the TODO.".into(),
                        guidance: None,
                    });
                }
            }
        }
        let ledger = task.session_dir.join("todo-review.md");
        let ledger = ledger.display().to_string();
        ctx.write(&ledger, &format!("{} TODO findings\n", findings.len()))
            .await;
        Ok(CodeReviewerSubmit {
            summary: format!("{} TODOs left in the change.", findings.len()),
            findings,
            security_block: false,
            ledger_path: ledger,
        })
    }
}

/// The plugin's own contract: a release note.
#[derive(Serialize, Deserialize)]
pub struct Note {
    pub text: String,
}

pub struct ReleaseNote;

impl ContractType for ReleaseNote {
    type Submit = Note;
    fn contract() -> Contract {
        "typed-review:release-note"
            .parse()
            .expect("a valid contract name")
    }
    fn schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["text"],
            "properties": {"text": {"type": "string", "description": "The release note."}}
        })
    }
}

struct NoteHandler;

#[async_trait::async_trait]
impl ResultHandler for NoteHandler {
    type Contract = ReleaseNote;

    async fn handle(
        &self,
        note: Note,
        _: ResultView,
        _: Arc<dyn Checkpoints>,
    ) -> Result<CustomSubmit, String> {
        let empty = note.text.trim().is_empty();
        Ok(CustomSubmit {
            verdict: if empty {
                StageVerdict::Fail
            } else {
                StageVerdict::Pass
            },
            summary: if empty {
                "The release note is empty.".into()
            } else {
                note.text
            },
            findings: vec![],
            question: None,
            options: vec![],
            report_path: None,
            data: None,
        })
    }
}

pub struct TypedReview {
    agents: TypedAgents,
}

impl Default for TypedReview {
    fn default() -> Self {
        TypedReview {
            agents: TypedAgents::new().with(TodoReviewer),
        }
    }
}

#[async_trait::async_trait]
impl Plugin for TypedReview {
    fn manifest(&self) -> PluginManifest {
        let mut agents = self.agents.definitions();
        agents.push(
            PluginAgent::new("note-writer", "Writes the release note for the change.")
                .prompt("Read the change and submit a one-paragraph release note as `text`.")
                .tier(Tier::Fast)
                .capabilities([Capability::Read])
                .returns_type::<ReleaseNote>(),
        );
        PluginManifest {
            name: "typed-review".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "A TODO reviewer and a release note, with typed results.".into(),
            agents,
            stages: vec![PluginStage {
                name: "notes".into(),
                description: "Runs the reviewer, then the note writer when it found no TODO."
                    .into(),
            }],
            contracts: vec![contract_def::<ReleaseNote>(
                "A release note for the change.",
            )],
            ..Default::default()
        }
    }

    /// Rule PL3: the runs read as structs, not JSON.
    async fn decide_stage(
        &self,
        _: &str,
        view: StageView,
        _: Arc<dyn Checkpoints>,
    ) -> Result<StageDecision, String> {
        let Some((_, review)) = view.last_submit_of::<Review>() else {
            return Ok(StageDecision::Run {
                agent: "todo-reviewer".into(),
                instructions: None,
            });
        };
        if !review.findings.is_empty() {
            return Ok(StageDecision::Fail {
                summary: review.summary,
            });
        }
        // Rule PL5: a note run is read through its handler's outcome.
        match view.runs.iter().rev().find_map(contracts::handled) {
            None => Ok(StageDecision::Run {
                agent: "note-writer".into(),
                instructions: None,
            }),
            Some(o) if o.verdict == StageVerdict::Pass => {
                Ok(StageDecision::Pass { summary: o.summary })
            }
            Some(o) => Ok(StageDecision::Fail { summary: o.summary }),
        }
    }

    async fn handle_result(
        &self,
        contract: &str,
        result: ResultView,
        checkpoints: Arc<dyn Checkpoints>,
    ) -> Result<CustomSubmit, String> {
        match contract {
            "release-note" => contracts::handle_result(&NoteHandler, result, checkpoints).await,
            other => Err(format!("This plugin handles no contract `{other}`.")),
        }
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
            .unwrap_or_else(|| Err(format!("This plugin runs no agent `{agent}` in code.")))
    }
}

#[tokio::main]
async fn main() {
    serve(Arc::new(TypedReview::default())).await;
}
