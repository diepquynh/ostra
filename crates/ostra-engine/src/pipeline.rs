//! The pipeline: the built-in stages as a plugin provides them. The engine holds the generic
//! parts: the event log, the runner, slots, budget, gates, workflows, and messages. A pipeline
//! adds the stages' own state changes, planning, judges, views, and step effects, in process.
//!
//! The fold stays a pure function of the log (pattern 1), because a pipeline is deterministic
//! Rust code in the process, never a call over stdio. The server gives each engine the standard
//! plugin's pipeline through `Services::pipeline`, and each session state carries it.

use crate::plan::{Planner, SpawnRequest, Step};
use crate::runner::{EngineError, StepHost};
use crate::state::{ExecRecord, SessionState};
use ostra_core::Contract;
use ostra_core::agent::AgentName;
use ostra_core::api::{ArtifactRef, ExecutionView, SessionDetail, SessionSummary, TreeSession};
use ostra_core::event::{ExecPurpose, GateAnswer, GatePayload, JudgeKind, StoredEvent};
use ostra_core::exec::ExecutionResult;
use ostra_core::ids::{DecisionId, ExecutionId, GateId, WorkspaceId};
use ostra_core::manage::CreatedProject;
use ostra_core::pipeline::Category;
use ostra_core::workflow::{BuiltinStage, WorkflowDef, WorkflowSet};
use ostra_store::WorkspaceDb;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// A project as the judges see it.
pub struct ProjectFacts {
    pub key: String,
    pub path: String,
    pub initialized: bool,
    pub stack: Option<String>,
    /// Module map rows from `project.toml`, as `area (glob)`.
    pub areas: Vec<String>,
}

/// One phase of the plan, as generic stages see it.
#[derive(Debug, Clone)]
pub struct PhaseFacts {
    pub info: ostra_core::pipeline::PhaseInfo,
    pub implementer_report: Option<PathBuf>,
}

/// What a helper task gives back to the run that asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperResult {
    /// The text of the result message.
    Text(String),
    /// The task is not finished, for example because it runs again after an error.
    NotYet,
}

/// How the engine answers a gate under YOLO.
pub enum YoloPlan {
    /// No judgment needed.
    Fixed { answer: GateAnswer, reason: String },
    /// The YOLO judge answers, against this schema.
    Judge { schema: Value },
}

/// The built-in stages of a plugin, run in process.
#[async_trait::async_trait]
pub trait Pipeline: Send + Sync {
    // Fold. Each hook runs at the point of `SessionState::apply` where the built-in stages read
    // the event, so the order of state changes is the same as one fold.

    /// The pipeline's state of a session before its first event.
    fn new_state(&self) -> PipelineBox;
    /// `SessionCreated`, after the generic fields are set.
    fn created(&self, s: &mut SessionState);
    /// Rule O3: a run created `project`, before the session records it.
    fn project_created(&self, s: &mut SessionState, project: &CreatedProject);
    /// An event only the built-in stages read.
    fn event(&self, s: &mut SessionState, stored: &StoredEvent);
    /// The phase loop a starting execution belongs to.
    fn loop_key(&self, s: &SessionState, purpose: &ExecPurpose) -> Option<(u32, bool)>;
    fn started(
        &self,
        s: &mut SessionState,
        id: &ExecutionId,
        purpose: &ExecPurpose,
        loop_key: Option<(u32, bool)>,
        resumed: bool,
    );
    fn finished(&self, s: &mut SessionState, rec: &ExecRecord, result: &ExecutionResult);
    /// Rule U1: whether the session can do without a running execution's task.
    fn can_skip(&self, s: &SessionState, exec: &ExecutionId) -> bool;
    /// Rule U1: end the execution's task without a result.
    fn skip_task(&self, s: &mut SessionState, exec: &ExecutionId);
    fn decision(
        &self,
        s: &mut SessionState,
        id: &DecisionId,
        judge: JudgeKind,
        subject: Option<&str>,
        output: &Value,
        overridden: bool,
    );
    fn gate_opened(&self, s: &mut SessionState, id: &GateId, payload: &GatePayload);
    fn gate_answered(
        &self,
        s: &mut SessionState,
        id: &GateId,
        payload: &GatePayload,
        answer: &GateAnswer,
        routed: bool,
    );
    /// Rule C2: whether context the user adds goes through the Route answer judge.
    fn routes_amendments(&self, s: &SessionState) -> bool;
    /// Rule U1: a decision of `kind` can skip running work, so the engine stops what it skipped.
    fn judge_skips_work(&self, kind: JudgeKind) -> bool;
    /// The output a failed judge call of `kind` records instead, or `None` to fail the session.
    fn judge_fallback(&self, s: &SessionState, kind: JudgeKind, error: &str) -> Option<Value>;
    /// Rule C2: context the user added reached the session unrouted.
    fn amended(&self, s: &mut SessionState, text: &str);

    // Planning.

    /// Everything after the YOLO answers.
    fn plan(&self, p: &mut Planner<'_>);
    /// Rule WF2: plan a built-in stage by its own rules. Returns `true` when the stage is done.
    fn builtin_stage(&self, p: &mut Planner<'_>, stage: BuiltinStage, wf: &WorkflowDef) -> bool;
    /// Every workflow stage is done.
    fn completion(&self, p: &mut Planner<'_>);
    /// Rule O4: a created project whose init has not ended, so its other steps wait.
    fn awaiting_init(&self, s: &SessionState, project: &str) -> bool;
    /// Rule O4: a step that must wait, such as one in a created project whose init has not ended.
    fn holds(&self, s: &SessionState, step: &Step) -> bool;

    // Facts that generic code reads.

    /// Rule PL4: the pipeline's own agents, the catalog a workspace without custom agents has.
    fn agents(&self) -> ostra_agents::AgentCatalog;
    /// Rule CA5: the contract of a run logged before contracts: the pipeline agent's own, else
    /// the custom stage contract, the only one custom agents had then.
    fn legacy_contract(&self, agent: AgentName) -> Contract;
    /// Rule PL4: the standard agent for a contract no built-in stage binds.
    fn default_agent(&self, contract: Contract) -> AgentName;
    /// Rule WF9: the default workflows.
    fn workflow_set(&self) -> WorkflowSet;
    /// Rule WF9: the default workflow of a base pipeline.
    fn default_workflow(&self, base: Category) -> WorkflowDef;
    /// Rule D9: the phases removed because a phase they depend on is blocked.
    fn removed_phases(&self, s: &SessionState) -> BTreeSet<u32>;
    /// The executor an agent must run on regardless of routing.
    fn forced_executor(
        &self,
        s: &SessionState,
        agent: AgentName,
    ) -> Option<ostra_core::ExecutorKind>;
    /// Rule O2: why `execution` cannot add project `key` to this session now.
    fn project_creation_refusal(
        &self,
        s: &SessionState,
        execution: &ExecutionId,
        key: &str,
    ) -> Option<String>;
    /// Whether overriding a decision can still change what happens.
    fn can_override(&self, s: &SessionState, id: &DecisionId) -> bool;
    /// Rule WB4: what a reference to a built-in stage's node reads, seen from an instance in
    /// `scope`: the facts the stage settled.
    fn stage_value(&self, s: &SessionState, stage: BuiltinStage, scope: Option<&str>) -> Value;
    /// Rule WB4: the pipeline's facts in the `session` value, such as the track.
    fn session_facts(&self, s: &SessionState) -> serde_json::Map<String, Value>;
    /// Rules WF1 and WF3: the session recorded its workflow.
    fn workflow_resolved(&self, s: &mut SessionState, wf: &WorkflowDef);
    /// Research documents, oldest first (Rule D2).
    fn research_docs(&self, s: &SessionState) -> Vec<PathBuf>;
    /// Rule B5: what this session adds to its book, from the log.
    fn book_update(&self, s: &SessionState) -> ostra_core::book::BookUpdate;
    /// The current spec, once one exists.
    fn spec_file(&self, s: &SessionState) -> Option<PathBuf>;
    /// The current master plan, once one exists.
    fn master_plan(&self, s: &SessionState) -> Option<PathBuf>;
    /// One phase of the plan, as a phase-scoped stage sees it.
    fn phase(&self, s: &SessionState, id: u32) -> Option<PhaseFacts>;
    /// Rule WF4: the phases whose phase-scoped stages run: built, reviewed, and not removed.
    fn passed_phases(&self, s: &SessionState) -> Vec<u32>;
    /// The files the work loops of `project` changed, in one phase or in all, for the diff view.
    fn changed_files(
        &self,
        s: &SessionState,
        project: &str,
        phase: Option<u32>,
    ) -> BTreeSet<String>;

    // Messages between subagents (Rules SM3 to SM7).

    /// Rule SM7: take a message to an agent as one of the pipeline's own tasks, such as a
    /// research task. Returns the task's number, or `None` to start the agent as a helper.
    #[allow(clippy::too_many_arguments)]
    fn helper_task(
        &self,
        s: &mut SessionState,
        ask: &ostra_core::ids::MessageId,
        agent: AgentName,
        contract: Option<Contract>,
        project: &str,
        asker: &str,
        text: &str,
    ) -> Option<u32>;
    /// Rule SM7: the message a run of one of the pipeline's tasks answers.
    fn helper_ask(
        &self,
        s: &SessionState,
        purpose: &ExecPurpose,
    ) -> Option<ostra_core::ids::MessageId>;
    /// Rule SM7: what a run of one of the pipeline's helper tasks returns to its asker. `None`
    /// when the run is no such task.
    fn helper_result(
        &self,
        s: &SessionState,
        rec: &ExecRecord,
        result: &ExecutionResult,
    ) -> Option<HelperResult>;
    /// Rules H5 and H6: the earlier run of the same loop whose conversation a run of `purpose`
    /// continues, for the pipeline's own purposes.
    fn previous_run(&self, s: &SessionState, purpose: &ExecPurpose) -> Option<ExecutionId>;
    /// Rule SM2: the subagents a run of the pipeline works with, and their role toward it.
    fn partners(&self, s: &SessionState, rec: &ExecRecord) -> Vec<(ExecutionId, String)>;
    /// Rule WF6: the subagents of built-in stage `stage` that a later stage's agent works with,
    /// such as the author of the spec.
    fn stage_partners(&self, s: &SessionState, stage: BuiltinStage) -> Vec<(ExecutionId, String)>;
    /// Rule WF6: the subagents whose work a stage instance in `scope` checks, such as the
    /// implementer of its phase.
    fn work_partners(&self, s: &SessionState, scope: Option<&str>) -> Vec<(ExecutionId, String)>;
    /// The session dir of a run that works for the whole session, not for its project.
    fn session_wide(&self, purpose: &ExecPurpose) -> bool;

    // Judges and YOLO.

    fn judge_input(
        &self,
        s: &SessionState,
        kind: JudgeKind,
        subject: Option<&str>,
        projects: &[ProjectFacts],
    ) -> (String, String);
    fn judge_schema(&self, kind: JudgeKind, answer: Option<Value>) -> Value;
    /// Whether a judge's answer has the shape its kind needs.
    fn judge_output_ok(&self, kind: JudgeKind, output: &Value) -> bool;
    fn judge_reason(&self, output: &Value) -> String;
    fn yolo_leaves_open(&self, s: &SessionState, payload: &GatePayload) -> bool;
    fn yolo_plan(&self, s: &SessionState, gate: &GateId) -> Option<YoloPlan>;
    fn yolo_answer_from_judge(
        &self,
        s: &SessionState,
        gate: &GateId,
        answer: &Value,
    ) -> Result<GateAnswer, String>;
    /// Whether an answer fits a built-in gate. `None` for a gate the pipeline does not own.
    fn validate_answer(
        &self,
        s: &SessionState,
        payload: &GatePayload,
        answer: &GateAnswer,
    ) -> Option<Result<(), String>>;
    /// Rule J1: whether an answer waits for the Route answer judge.
    fn answer_needs_route(&self, payload: &GatePayload, answer: &GateAnswer) -> bool;
    /// Whether a user's override of a decision has the decision's output shape.
    fn override_ok(&self, judge: JudgeKind, output: &Value) -> bool;

    /// The pipeline's checks of a contract's submit at submit time, after its shape check.
    fn check_submit(&self, contract: Contract, input: &Value) -> Vec<String>;

    // Effects.

    /// Perform a built-in step, or prepare a generic one. Returns the step back for the engine
    /// to perform.
    async fn perform(
        &self,
        host: &StepHost,
        session: &ostra_core::ids::SessionId,
        step: Step,
    ) -> Result<Result<(), EngineError>, Step>;
    /// The session's work is done and its report written. Returns `true` when the pipeline ended
    /// the session another way, so the engine records no completion.
    fn completing(
        &self,
        host: &StepHost,
        session: &ostra_core::ids::SessionId,
        s: &SessionState,
    ) -> Result<bool, EngineError>;
    /// A spawn that waited for a slot and must not start any more.
    fn spawn_dropped(&self, s: &SessionState, req: &SpawnRequest) -> bool;
    /// Files the runner writes before a spawn resolves its route.
    fn before_spawn(&self, s: &SessionState, req: &SpawnRequest) -> Result<(), String>;
    /// Files the runner writes once the spawn's session dir exists.
    fn spawn_files(&self, s: &SessionState, req: &SpawnRequest);
    /// The tier a spawn must run on regardless of routing.
    fn tier_override(&self, req: &SpawnRequest) -> Option<ostra_core::model::Tier>;
    /// Rule O2: the planned folder of a project the approved plan names as new.
    fn project_to_create(&self, s: &SessionState, key: &str) -> Option<PathBuf>;
    /// Facts the fold reads later, recorded in the execution's params.
    fn spawn_params(
        &self,
        req: &SpawnRequest,
        profile: Option<&ostra_core::config::ProjectProfile>,
        params: &mut Value,
    );
    /// Events the runner appends after a run's `ExecutionFinished`.
    fn after_run(
        &self,
        req: &SpawnRequest,
        result: &ExecutionResult,
    ) -> Vec<ostra_core::event::SessionEvent>;

    // Views.

    fn summary(&self, s: &SessionState, workspace: &WorkspaceId, judge_cost: f64)
    -> SessionSummary;
    fn detail(
        &self,
        s: &SessionState,
        db: &WorkspaceDb,
        workspace: &WorkspaceId,
        live: HashSet<ExecutionId>,
    ) -> Result<SessionDetail, EngineError>;
    fn tree_session(
        &self,
        s: &SessionState,
        summary: &SessionSummary,
        executions: Vec<ExecutionView>,
    ) -> TreeSession;
    fn run_labels(&self, s: &SessionState) -> HashMap<ExecutionId, String>;
    fn artifacts(&self, s: &SessionState) -> Vec<ArtifactRef>;
    fn decorate(
        &self,
        s: &SessionState,
        labels: &HashMap<ExecutionId, String>,
        view: &mut ExecutionView,
    );
}

/// The pipeline's own part of a session's state. The engine clones it with the session state and
/// never reads it.
pub trait PipelineData: std::any::Any + Send + Sync + std::fmt::Debug {
    fn clone_data(&self) -> Box<dyn PipelineData>;
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

impl<T: std::any::Any + Send + Sync + std::fmt::Debug + Clone> PipelineData for T {
    fn clone_data(&self) -> Box<dyn PipelineData> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// A pipeline's state inside `SessionState::ext`. Only its pipeline knows the type.
pub struct PipelineBox(Box<dyn PipelineData>);

impl PipelineBox {
    pub fn new<T: PipelineData>(data: T) -> Self {
        PipelineBox(Box::new(data))
    }

    /// The state as its pipeline's type. A session always holds the state its pipeline made.
    pub fn get<T: 'static>(&self) -> &T {
        self.0
            .as_any()
            .downcast_ref()
            .expect("the session holds its pipeline's own state")
    }

    pub fn get_mut<T: 'static>(&mut self) -> &mut T {
        self.0
            .as_any_mut()
            .downcast_mut()
            .expect("the session holds its pipeline's own state")
    }
}

impl Clone for PipelineBox {
    fn clone(&self) -> Self {
        PipelineBox(self.0.clone_data())
    }
}

impl std::fmt::Debug for PipelineBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// The pipeline a session folds and plans with. The engine gets it from `Services::pipeline`, and
/// every `SessionState` carries it, so a fold needs no process-wide state.
#[derive(Clone)]
pub struct PipelineRef(pub Arc<dyn Pipeline>);

impl std::fmt::Debug for PipelineRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PipelineRef")
    }
}

impl std::ops::Deref for PipelineRef {
    type Target = dyn Pipeline;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl From<Arc<dyn Pipeline>> for PipelineRef {
    fn from(p: Arc<dyn Pipeline>) -> Self {
        PipelineRef(p)
    }
}
