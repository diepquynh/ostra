//! The pipeline: the built-in stages as a plugin provides them. The engine holds the generic
//! parts: the event log, the runner, slots, budget, gates, workflows, and messages. A pipeline
//! adds the stages' own state changes, planning, judges, views, and step effects, in process.
//!
//! The fold stays a pure function of the log (pattern 1), because a pipeline is deterministic
//! Rust code in the process, never a call over stdio. One pipeline serves the process. The server
//! installs the standard plugin's pipeline at startup, and tests install it before they fold.

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
use std::sync::{Arc, OnceLock};

/// A project as the judges see it.
pub struct ProjectFacts {
    pub key: String,
    pub path: String,
    pub initialized: bool,
    pub stack: Option<String>,
    /// Module map rows from `project.toml`, as `area (glob)`.
    pub areas: Vec<String>,
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

    // Facts that generic code reads.

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
    /// `scope`, for the stages whose facts the pipeline settles.
    fn stage_value(&self, s: &SessionState, stage: BuiltinStage, scope: Option<&str>) -> Value;

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

static PIPELINE: OnceLock<Arc<dyn Pipeline>> = OnceLock::new();

/// Install the process's pipeline. The first install wins, because a session folded with one
/// pipeline must not fold with another.
pub fn install(pipeline: Arc<dyn Pipeline>) {
    let _ = PIPELINE.set(pipeline);
}

/// The process's pipeline.
pub fn get() -> &'static dyn Pipeline {
    PIPELINE
        .get()
        .map(|p| p.as_ref())
        .expect("no pipeline installed: call ostra_default_plugin::install() at startup")
}
