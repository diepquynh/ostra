//! The planner: a pure function from session state to the next steps. The runner performs the
//! steps and appends the resulting events, and the planner runs again. Conformance fixtures test
//! this function directly: event history in, expected next steps out.

use crate::state::*;
use ostra_core::agent::AgentName;
use ostra_core::event::{ExecPurpose, GatePayload, JudgeKind};
use ostra_core::ids::{ExecutionId, GateId};
use ostra_core::model::Complexity;
use ostra_core::pipeline::{Category, PhaseInfo, StageKind};
use ostra_core::workflow::{StageDef, StageRun, StageScope, WorkflowDef};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Facts from outside the event log the planner needs: each project's format command.
#[derive(Debug, Clone, Default)]
pub struct PlanCtx {
    pub format_commands: BTreeMap<String, Option<String>>,
    /// The session budget from workspace settings. `None` means no limit.
    pub budget_usd: Option<f64>,
}

/// Everything a spawn needs beyond its agent and purpose. The spawn factory turns this into the
/// agent's parameter struct.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SpawnInputs {
    pub task: Option<String>,
    pub research_docs: Vec<PathBuf>,
    pub target: Option<PathBuf>,
    pub prior_findings: Option<String>,
    pub spec_file: Option<PathBuf>,
    pub phase: Option<PhaseInfo>,
    pub instructions: Option<String>,
    /// `Phase:` value for reviews: `N` or `N-tests`.
    pub phase_value: Option<String>,
    pub implementer_report: Option<PathBuf>,
    pub report_file: Option<PathBuf>,
    /// Rule J1: answers the judge kept for this stage.
    pub user_notes: Vec<String>,
    /// Rule WF4: the workflow node a custom agent's run serves.
    pub stage_id: Option<String>,
    pub stage_round: Option<u32>,
    /// Rule WF4: one line per earlier workflow stage, with its verdict, summary, and report.
    pub earlier_stages: Vec<String>,
    /// Rule WB4: the node's inputs, one `name = <json>` line each.
    pub stage_inputs: Vec<String>,
    /// The pipeline's own inputs for its contracts' spawns, which only the pipeline reads.
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpawnRequest {
    pub agent: AgentName,
    pub purpose: ExecPurpose,
    pub stage: StageKind,
    pub project: String,
    /// Rule WD1: the other projects the run works in, after `project`.
    pub also: Vec<String>,
    /// The `Session dir:` of the spawn.
    pub session_dir: PathBuf,
    pub inputs: SpawnInputs,
    pub resumes: Option<ExecutionId>,
    /// Rules H3 and H5: a new run that continues this run's conversation.
    pub continues: Option<ExecutionId>,
}

impl SpawnRequest {
    /// Rule WD1: every project the run works in, `project` first.
    pub fn projects(&self) -> Vec<String> {
        let mut all = vec![self.project.clone()];
        for p in &self.also {
            if !all.contains(p) {
                all.push(p.clone());
            }
        }
        all
    }

    /// Rule WD1: the list `ExecutionStarted` records. A run in one project records none, so its
    /// event keeps the shape it had before work dirs.
    pub fn recorded_projects(&self) -> Vec<String> {
        let all = self.projects();
        if all.len() > 1 { all } else { Vec::new() }
    }

    /// The phase complexity routes see. `None` outside a phase; a phase without a file is `Low`.
    pub fn complexity(&self) -> Option<Complexity> {
        self.inputs.phase.as_ref().map(|p| {
            if p.file.is_some() {
                p.complexity
            } else {
                Complexity::Low
            }
        })
    }
}

/// A step only the pipeline knows: its identity, its form in a fixture, and the data its
/// `perform` reads back.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PipelineStep {
    pub key: String,
    pub summary: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    Judge {
        judge: JudgeKind,
        subject: Option<String>,
    },
    Spawn(Box<SpawnRequest>),
    OpenGate {
        title: String,
        explanation: String,
        payload: GatePayload,
    },
    YoloAnswer {
        gate: GateId,
    },
    /// A step of the pipeline's own, which only the pipeline performs.
    Pipeline(PipelineStep),
    /// Rule B5: write the session's documentation into the workspace book `book`.
    WriteBook {
        book: String,
    },
    /// Rule PL5: ask the plugin that owns a run's contract to handle its result.
    HandleResult {
        execution: ExecutionId,
    },
    /// Rule PL3: ask a plugin's stage logic for its next decision about workflow node `node`.
    DecideStage {
        node: String,
        scope: Option<String>,
        /// How many decisions it made before, so each request is its own step.
        seq: u32,
    },
    /// Rule WB5: record that node `node` is skipped because its conditions do not hold.
    SkipStage {
        node: String,
        scope: Option<String>,
    },
    /// Rules WB2 and WB3: run transform or prompt node `node` in the engine.
    RunNode {
        node: String,
        scope: Option<String>,
        round: u32,
        /// A prompt node calls a model, so the session budget applies to it.
        model: bool,
    },
    /// Rule WF1: resolve the session's workflow from the workspace's files and record it.
    ResolveWorkflow {
        name: Option<String>,
        category: Option<Category>,
    },
    Complete {
        report_markdown: Option<String>,
    },
    Fail {
        error: String,
    },
}

impl Step {
    /// Identity used by the runner so a step already in flight is not started twice.
    pub fn key(&self) -> String {
        match self {
            Step::Judge { judge, subject } => format!(
                "judge:{}:{}",
                judge.as_str(),
                subject.clone().unwrap_or_default()
            ),
            // A purpose can repeat across projects, such as one init per created project.
            Step::Spawn(s) => format!(
                "spawn:{}:{}",
                s.project,
                serde_json::to_string(&s.purpose).unwrap_or_default()
            ),
            Step::OpenGate { payload, .. } => {
                format!("gate:{}:{}", payload.kind_str(), payload.owner())
            }
            Step::YoloAnswer { gate } => format!("yolo:{gate}"),
            Step::Pipeline(p) => format!("pipeline:{}", p.key),
            Step::WriteBook { book } => format!("book:{book}"),
            Step::ResolveWorkflow { .. } => "resolve-workflow".into(),
            Step::HandleResult { execution } => format!("handle:{execution}"),
            Step::DecideStage { node, scope, seq } => format!(
                "decide:{}:{seq}",
                crate::workflow::stage_key(node, scope.as_deref())
            ),
            Step::SkipStage { node, scope } => format!(
                "skip:{}",
                crate::workflow::stage_key(node, scope.as_deref())
            ),
            Step::RunNode {
                node, scope, round, ..
            } => format!(
                "node:{}:{round}",
                crate::workflow::stage_key(node, scope.as_deref())
            ),
            Step::Complete { .. } => "complete".into(),
            Step::Fail { .. } => "fail".into(),
        }
    }

    /// Compact form for conformance fixtures.
    pub fn summary(&self) -> String {
        match self {
            Step::Judge { judge, subject } => match subject {
                Some(s) => format!("judge {} {s}", judge.as_str()),
                None => format!("judge {}", judge.as_str()),
            },
            Step::Spawn(s) => format!(
                "spawn {} {}{}",
                s.agent,
                s.purpose.summary(),
                if s.continues.is_some() {
                    " (continues)"
                } else {
                    ""
                }
            ),
            Step::OpenGate { payload, .. } => format!("gate {}", payload.kind_str()),
            Step::YoloAnswer { .. } => "yolo-answer".into(),
            Step::Pipeline(p) => p.summary.clone(),
            Step::WriteBook { book } => format!("write-book {book}"),
            Step::HandleResult { .. } => "handle-result".into(),
            Step::DecideStage { node, scope, .. } => match scope {
                Some(s) => format!("decide-stage {node} {s}"),
                None => format!("decide-stage {node}"),
            },
            Step::SkipStage { node, scope } => match scope {
                Some(s) => format!("skip-stage {node} {s}"),
                None => format!("skip-stage {node}"),
            },
            Step::RunNode {
                node, scope, round, ..
            } => match scope {
                Some(s) => format!("run-node {node} {s} #{round}"),
                None => format!("run-node {node} #{round}"),
            },
            Step::ResolveWorkflow { name, category } => match (name, category) {
                (Some(n), _) => format!("resolve-workflow {n}"),
                (None, Some(c)) => format!("resolve-workflow {}", c.as_str().to_lowercase()),
                _ => "resolve-workflow".into(),
            },
            Step::Complete { .. } => "complete".into(),
            Step::Fail { .. } => "fail".into(),
        }
    }
}

pub struct Planner<'a> {
    pub s: &'a SessionState,
    pub ctx: &'a PlanCtx,
    pub out: Vec<Step>,
}

/// Compute the next steps for a session.
pub fn next_steps(s: &SessionState, ctx: &PlanCtx) -> Vec<Step> {
    let mut p = Planner {
        s,
        ctx,
        out: vec![],
    };
    p.run();
    p.out
}

impl<'a> Planner<'a> {
    pub fn push_step(&mut self, step: Step) {
        self.push(step);
    }

    pub fn session(&self) -> &'a SessionState {
        self.s
    }

    pub fn push(&mut self, mut step: Step) {
        // Rule O4: nothing but its init and the advisor runs in a created project until the init
        // ends, because every other agent routes its work by the project's inventory and profile.
        if self.s.pipeline.holds(self.s, &step) {
            return;
        }
        if let Step::Spawn(req) = &mut step {
            let key = purpose_key(&req.purpose);
            // Rule H2: a run that waits for a message holds its stage, and the message wakes it in
            // place.
            match self.s.wake_for_key(&key) {
                Some(None) => return,
                Some(Some(exec)) => req.resumes = Some(exec),
                // Rule P2: a spawn that replaces a paused run resumes it.
                None if req.resumes.is_none() => {
                    req.resumes = self.s.resume_from.get(&key).cloned()
                }
                None => {}
            }
            // Rules H5 and H6: a pair loop continues the conversation of the run before it.
            if req.resumes.is_none() && req.continues.is_none() {
                req.continues = self.s.continuation(req.agent, &req.purpose);
            }
            // Rule H7: one live run per conversation.
            if let Some(head) = &req.continues
                && self.s.conversation_busy(head)
            {
                return;
            }
        }
        // Budget guard: once the session has spent its budget, no new execution starts until
        // the user raises it. Running executions finish.
        if matches!(step, Step::Spawn(_) | Step::RunNode { model: true, .. })
            && let Some(budget) = self.ctx.budget_usd
        {
            let limit = budget + self.s.budget_raised;
            let spent = self.s.spent_usd();
            if spent >= limit {
                if self.s.budget_gate.is_none() {
                    let step = Step::OpenGate {
                        title: "The session reached its budget".into(),
                        explanation: format!(
                            "This session has spent ${spent:.2} of its ${limit:.2} budget, so no new execution starts. Raise the budget to continue, or stop the session."
                        ),
                        payload: GatePayload::BudgetReached {
                            spent_usd: spent,
                            budget_usd: limit,
                        },
                    };
                    let key = step.key();
                    if !self.out.iter().any(|s| s.key() == key) {
                        self.out.push(step);
                    }
                }
                return;
            }
        }
        let key = step.key();
        if !self.out.iter().any(|s| s.key() == key) {
            self.out.push(step);
        }
    }

    pub fn spawn(
        &mut self,
        agent: AgentName,
        purpose: ExecPurpose,
        project: &str,
        session_dir: PathBuf,
        inputs: SpawnInputs,
    ) {
        let stage = stage_of(&purpose);
        self.push(Step::Spawn(Box::new(SpawnRequest {
            also: Vec::new(),
            agent,
            purpose,
            stage,
            project: project.to_string(),
            session_dir,
            inputs,
            resumes: None,
            continues: None,
        })));
    }

    pub fn gate(
        &mut self,
        title: impl Into<String>,
        explanation: impl Into<String>,
        payload: GatePayload,
    ) {
        self.push(Step::OpenGate {
            title: title.into(),
            explanation: explanation.into(),
            payload,
        });
    }

    pub fn run(&mut self) {
        self.flows();
        self.coordination();
    }

    /// Rules SM3, SM4, and SM7: wake waiting native runs that have messages, continue ended
    /// subagents that were sent messages, and start the custom helpers messages asked for. A
    /// running run reads its messages at its own turn boundary, and a waiting harness run is woken
    /// by its executor, so neither needs a step.
    pub fn coordination(&mut self) {
        let s = self.s;
        if !s.created || s.is_terminal() || s.paused {
            return;
        }
        // Rule PL5: a plugin contract's result goes to its plugin before anything reads it.
        for execution in s.results_due() {
            self.push(Step::HandleResult { execution });
        }
        for exec in s.wakes_due() {
            // A stage that spawns again wakes its waiting run through that spawn (see `push`).
            let staged = self
                .out
                .iter()
                .any(|st| matches!(st, Step::Spawn(r) if r.resumes.as_ref() == Some(&exec)));
            let Some(rec) = s.executions.get(&exec).filter(|_| !staged) else {
                continue;
            };
            self.push(Step::Spawn(Box::new(SpawnRequest {
                also: Vec::new(),
                agent: rec.agent,
                purpose: rec.purpose.clone(),
                stage: rec.stage,
                project: rec.project.clone(),
                session_dir: s.session_dir_of(rec),
                inputs: SpawnInputs::default(),
                resumes: Some(exec.clone()),
                continues: None,
            })));
        }
        for (head, first) in s.continuations_due() {
            let Some(rec) = s.executions.get(&head) else {
                continue;
            };
            let root = s.subagent_of(&head);
            self.push(Step::Spawn(Box::new(SpawnRequest {
                also: Vec::new(),
                agent: rec.agent,
                purpose: ExecPurpose::Message {
                    subagent: root,
                    first,
                },
                stage: rec.stage,
                project: rec.project.clone(),
                session_dir: s.session_dir_of(rec),
                inputs: SpawnInputs::default(),
                resumes: None,
                continues: Some(head.clone()),
            })));
        }
        let helpers: Vec<(AgentName, String, ostra_core::ids::MessageId, String)> = s
            .helpers_due()
            .filter_map(|m| match &m.target {
                ostra_core::coord::MessageTarget::Agent { agent, project, .. } => {
                    Some((*agent, project.clone(), m.id.clone(), m.text.clone()))
                }
                _ => None,
            })
            .collect();
        for (agent, project, message, text) in helpers {
            let from = s
                .messages
                .iter()
                .find(|m| m.id == message)
                .map(|m| s.subagent_of(&m.from))
                .unwrap_or_default();
            self.spawn(
                agent,
                ExecPurpose::Helper { message },
                &project,
                s.project_session_dir(&project),
                SpawnInputs {
                    task: Some(text),
                    stage_id: Some(format!("helper for {from}")),
                    ..Default::default()
                },
            );
        }
    }

    pub fn flows(&mut self) {
        let s = self.s;
        // Rule P1: a paused session starts nothing, not even a YOLO answer.
        if !s.created || s.is_terminal() || s.paused {
            return;
        }
        // Under YOLO every open gate is answered by the engine, with no wait (HANDOVER 10.5).
        // Permission asks belong to a live execution, which answers them itself.
        if s.yolo {
            for g in s.open_gates() {
                // A budget is never raised by YOLO: spending more is the user's decision.
                if !matches!(
                    g.payload,
                    GatePayload::Permission { .. } | GatePayload::BudgetReached { .. }
                ) && !self.s.pipeline.yolo_leaves_open(s, &g.payload)
                {
                    self.push(Step::YoloAnswer { gate: g.id.clone() });
                }
            }
        }
        self.s.pipeline.plan(self);
    }

    // -------------------------------------------------------------------------------------
    // Workflows (Rules WF1 to WF8)
    // -------------------------------------------------------------------------------------

    /// Rule WF4: every stage whose stages before it are done runs; the session completes when every
    /// stage is done.
    pub fn workflow_flow(&mut self) {
        let s = self.s;
        let Some(wf) = s.active_workflow() else {
            return;
        };
        let mut done: BTreeSet<String> = BTreeSet::new();
        let mut all = true;
        for d in wf.ordered() {
            if !d.after.iter().all(|a| done.contains(a)) {
                all = false;
                continue;
            }
            let ok = match &d.run {
                StageRun::Builtin { stage } => self.s.pipeline.builtin_stage(self, *stage, &wf),
                // A phase stage runs inside the build stage, which is done only after it.
                _ if d.scope == StageScope::Phase => true,
                StageRun::Agent { agent } => self.agent_stage(&wf, d, *agent),
                StageRun::Plugin { .. } => self.plugin_stage(&wf, d),
                StageRun::Transform { .. } | StageRun::Prompt { .. } => self.data_stage(d),
            };
            if ok {
                done.insert(d.id.clone());
            } else {
                all = false;
            }
        }
        if all {
            self.s.pipeline.completion(self);
        }
    }

    /// Rule WF4: the phase stages of every phase that passed its review.
    pub fn phase_stages(&mut self, wf: &WorkflowDef) {
        for d in wf
            .stages
            .iter()
            .filter(|d| d.scope == StageScope::Phase && d.builtin().is_none())
        {
            match &d.run {
                StageRun::Agent { agent } => {
                    self.agent_stage(wf, d, *agent);
                }
                StageRun::Plugin { .. } => {
                    self.plugin_stage(wf, d);
                }
                StageRun::Transform { .. } | StageRun::Prompt { .. } => {
                    self.data_stage(d);
                }
                StageRun::Builtin { .. } => {}
            }
        }
    }

    /// Rules WF4 and WF5: run, ask about, or finish each instance of a custom agent stage.
    pub fn agent_stage(&mut self, wf: &WorkflowDef, d: &StageDef, agent: AgentName) -> bool {
        let s = self.s;
        let mut done = true;
        for scope in crate::workflow::stage_scopes(s, d) {
            match crate::workflow::agent_stage_action(s, wf, d, agent, scope.as_deref()) {
                crate::workflow::Action::Done => {}
                crate::workflow::Action::Wait => done = false,
                crate::workflow::Action::Fail(error) => {
                    self.push(Step::Fail { error });
                    done = false;
                }
                crate::workflow::Action::Gate(step) | crate::workflow::Action::Skip(step) => {
                    self.push(step);
                    done = false;
                }
                crate::workflow::Action::ExecFailed {
                    exec,
                    agent,
                    project,
                    error,
                } => {
                    self.exec_failed_gate(&exec, agent, &project, &error);
                    done = false;
                }
                crate::workflow::Action::Spawn(req) => {
                    self.push(Step::Spawn(req));
                    done = false;
                }
            }
        }
        done
    }

    /// Rules WB2 and WB3: run, ask about, or finish each instance of a transform or prompt node.
    pub fn data_stage(&mut self, d: &StageDef) -> bool {
        let s = self.s;
        let mut done = true;
        for scope in crate::workflow::stage_scopes(s, d) {
            match crate::workflow::data_stage_action(s, d, scope.as_deref()) {
                crate::workflow::Action::Done => {}
                crate::workflow::Action::Wait => done = false,
                crate::workflow::Action::Fail(error) => {
                    self.push(Step::Fail { error });
                    done = false;
                }
                crate::workflow::Action::Gate(step) | crate::workflow::Action::Skip(step) => {
                    self.push(step);
                    done = false;
                }
                crate::workflow::Action::ExecFailed { .. } | crate::workflow::Action::Spawn(_) => {
                    done = false
                }
            }
        }
        done
    }

    /// Rule PL3: a plugin's stage logic decides each next step of its stage.
    pub fn plugin_stage(&mut self, wf: &WorkflowDef, d: &StageDef) -> bool {
        crate::plugin_stage::plan(self, wf, d)
    }

    // -------------------------------------------------------------------------------------
    // Explore and sufficiency (Rules D1, D2, M1)
    // -------------------------------------------------------------------------------------

    // -------------------------------------------------------------------------------------
    // Spec (Rules D1, D3, D3a, D3b, D10)
    // -------------------------------------------------------------------------------------

    // -------------------------------------------------------------------------------------
    // Plan (Rules D4, D5, D10)
    // -------------------------------------------------------------------------------------

    // -------------------------------------------------------------------------------------
    // Phases (Rules D6, D7, D9, M2 to M6, Step 4)
    // -------------------------------------------------------------------------------------

    pub fn exec_failed_gate(
        &mut self,
        exec: &ExecutionId,
        agent: AgentName,
        project: &str,
        error: &str,
    ) {
        if let Some(rest) = error.strip_prefix("harness-") {
            let harness = match self.s.executions.get(exec).map(|r| r.executor) {
                Some(ostra_core::ExecutorKind::Harness(h)) => h,
                _ => ostra_core::HarnessKind::Claude,
            };
            self.gate(
                format!("{} could not run", harness.display_name()),
                "The harness failed to start or is not logged in. Log in from its terminal and retry, or run this execution on the native executor.",
                GatePayload::HarnessFailure { execution: exec.clone(), harness, error: rest.to_string() },
            );
            return;
        }
        let (title, explanation) = if self.s.stopped_by_user(exec) {
            (
                format!("You stopped {agent}"),
                "Ostra leaves a stopped execution to you, YOLO included. Retry it, or abandon this step.",
            )
        } else {
            (
                format!("{agent} failed"),
                "The execution ended without a usable result. Retry it, or abandon this step.",
            )
        };
        self.gate(
            title,
            explanation,
            GatePayload::ExecutionFailed {
                execution: exec.clone(),
                agent,
                project: project.to_string(),
                error: error.to_string(),
            },
        );
    }

    // -------------------------------------------------------------------------------------
    // Implementation review: feedback rounds until the user accepts (Rule F1)
    // -------------------------------------------------------------------------------------

    // -------------------------------------------------------------------------------------
    // Format, closing gate, tests, docs (Rules D8, T1 to T7)
    // -------------------------------------------------------------------------------------

    // -------------------------------------------------------------------------------------
    // Completion and quick answers
    // -------------------------------------------------------------------------------------

    // -------------------------------------------------------------------------------------
    // Init flow (HANDOVER 8.4)
    // -------------------------------------------------------------------------------------
}
