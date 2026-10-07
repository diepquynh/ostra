//! The standard pipeline's `impl Pipeline`. Each method forwards to the stage that owns the answer
//! (`stages/<stage>/hooks.rs` and the stage's fold, planner, and view) or to the shared code in
//! this folder.

use super::{effects, judges, partners, spawn};
use crate::judge;
use crate::judge_input;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::stages::{book, build, init, plan, research, spec, track};
use crate::view;
use ostra_core::config::ProjectProfile;
use ostra_core::event::{
    ExecPurpose, GateAnswer, GatePayload, JudgeKind, SessionEvent, StoredEvent,
};
use ostra_core::exec::ExecutionResult;
use ostra_core::ids::{DecisionId, ExecutionId, GateId, MessageId, SessionId, WorkspaceId};
use ostra_core::manage::CreatedProject;
use ostra_core::pipeline::Category;
use ostra_core::workflow::{BuiltinStage, WorkflowDef, WorkflowSet};
use ostra_core::{AgentName, Contract};
use ostra_engine::pipeline::{
    HelperResult, PhaseFacts, Pipeline, PipelineBox, ProjectFacts, YoloPlan,
};
use ostra_engine::plan::{Planner, SpawnRequest, Step};
use ostra_engine::runner::{EngineError, StepHost};
use ostra_engine::state::{ExecRecord, SessionState};
use ostra_store::WorkspaceDb;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

/// The built-in stages of the standard plugin `ostra`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OstraPipeline;

#[async_trait::async_trait]
impl Pipeline for OstraPipeline {
    // The fold of the built-in stages' state.

    fn new_state(&self) -> PipelineBox {
        PipelineBox::new(OstraState::default())
    }

    fn created(&self, s: &mut SessionState) {
        track::hooks::created(s);
        init::hooks::created(s);
    }

    fn project_created(&self, s: &mut SessionState, project: &CreatedProject) {
        init::hooks::project_created(s, project);
    }

    fn event(&self, s: &mut SessionState, stored: &StoredEvent) {
        s.fold_event(stored);
    }

    fn loop_key(&self, s: &SessionState, purpose: &ExecPurpose) -> Option<(u32, bool)> {
        build::hooks::loop_key(s, purpose)
    }

    fn started(
        &self,
        s: &mut SessionState,
        id: &ExecutionId,
        purpose: &ExecPurpose,
        loop_key: Option<(u32, bool)>,
        resumed: bool,
    ) {
        s.on_started(id, purpose, loop_key, resumed);
    }

    fn finished(&self, s: &mut SessionState, rec: &ExecRecord, result: &ExecutionResult) {
        s.on_finished(rec, result);
    }

    fn can_skip(&self, s: &SessionState, exec: &ExecutionId) -> bool {
        s.can_skip(exec)
    }

    fn skip_task(&self, s: &mut SessionState, exec: &ExecutionId) {
        s.skip_task(exec);
    }

    fn decision(
        &self,
        s: &mut SessionState,
        id: &DecisionId,
        judge: JudgeKind,
        subject: Option<&str>,
        output: &Value,
        overridden: bool,
    ) {
        s.on_decision(id, judge, subject, output, overridden);
    }

    fn gate_opened(&self, s: &mut SessionState, id: &GateId, payload: &GatePayload) {
        s.on_gate_opened(id, payload);
    }

    fn gate_answered(
        &self,
        s: &mut SessionState,
        id: &GateId,
        payload: &GatePayload,
        answer: &GateAnswer,
        routed: bool,
    ) {
        s.on_gate_answered(id, payload, answer, routed);
    }

    fn amended(&self, s: &mut SessionState, text: &str) {
        s.on_amended(text);
    }

    // Planning.

    fn plan(&self, p: &mut Planner<'_>) {
        p.session_flow();
    }

    fn builtin_stage(&self, p: &mut Planner<'_>, stage: BuiltinStage, wf: &WorkflowDef) -> bool {
        p.builtin_stage(stage, wf)
    }

    fn completion(&self, p: &mut Planner<'_>) {
        p.completion();
    }

    fn awaiting_init(&self, s: &SessionState, project: &str) -> bool {
        s.awaiting_init(project)
    }

    fn holds(&self, s: &SessionState, step: &Step) -> bool {
        init::hooks::holds(s, step)
    }

    // Agents, contracts, and workflows.

    fn check_submit(&self, contract: Contract, input: &Value, params: &Value) -> Vec<String> {
        match contract {
            // Rule B10: each docs step's own shape, with the correction first.
            Contract::Documentation => book::parts::check_with_params(input, params),
            Contract::Plan => plan::limits::check_plan(input, params),
            _ => vec![],
        }
    }

    fn agents(&self) -> ostra_agents::AgentCatalog {
        ostra_agents::AgentCatalog::builtin()
    }

    fn legacy_contract(&self, agent: AgentName) -> Contract {
        ostra_agents::builtin_def(agent)
            .map(|d| d.returns)
            .unwrap_or(Contract::Stage)
    }

    fn default_agent(&self, contract: Contract) -> AgentName {
        crate::Standard::default_for(contract)
            .expect("the standard plugin returns every built-in contract")
    }

    fn workflow_set(&self) -> WorkflowSet {
        crate::workflow_set()
    }

    fn default_workflow(&self, base: Category) -> WorkflowDef {
        crate::workflow(base)
    }

    // Facts that generic engine code reads.

    fn removed_phases(&self, s: &SessionState) -> BTreeSet<u32> {
        crate::planner::removed_phases(s)
    }

    fn forced_executor(
        &self,
        s: &SessionState,
        agent: AgentName,
    ) -> Option<ostra_core::ExecutorKind> {
        s.forced_executor(agent)
    }

    fn project_creation_refusal(
        &self,
        s: &SessionState,
        execution: &ExecutionId,
        key: &str,
    ) -> Option<String> {
        s.project_creation_refusal(execution, key)
    }

    fn can_override(&self, s: &SessionState, id: &DecisionId) -> bool {
        s.can_override(id)
    }

    fn stage_value(&self, s: &SessionState, stage: BuiltinStage, scope: Option<&str>) -> Value {
        crate::planner::stage_value(s, stage, scope)
    }

    fn session_facts(&self, s: &SessionState) -> serde_json::Map<String, Value> {
        spawn::session_facts(s)
    }

    fn workflow_resolved(&self, s: &mut SessionState, wf: &WorkflowDef) {
        track::hooks::workflow_resolved(s, wf);
    }

    fn book_update(&self, s: &SessionState) -> ostra_core::book::BookUpdate {
        s.book_update()
    }

    fn research_docs(&self, s: &SessionState) -> Vec<PathBuf> {
        s.research_docs()
    }

    fn spec_file(&self, s: &SessionState) -> Option<PathBuf> {
        spec::hooks::spec_file(s)
    }

    fn master_plan(&self, s: &SessionState) -> Option<PathBuf> {
        plan::hooks::master_plan(s)
    }

    fn work_projects(&self, s: &SessionState, purpose: &ExecPurpose) -> Option<Vec<String>> {
        book::hooks::work_projects(s, purpose)
    }

    fn phase(&self, s: &SessionState, id: u32) -> Option<PhaseFacts> {
        build::hooks::phase(s, id)
    }

    fn passed_phases(&self, s: &SessionState) -> Vec<u32> {
        build::hooks::passed_phases(s)
    }

    fn changed_files(
        &self,
        s: &SessionState,
        project: &str,
        phase: Option<u32>,
    ) -> BTreeSet<String> {
        build::hooks::changed_files(s, project, phase)
    }

    // Messages between subagents (Rules SM3 to SM7).

    fn helper_task(
        &self,
        s: &mut SessionState,
        ask: &MessageId,
        agent: AgentName,
        contract: Option<Contract>,
        project: &str,
        asker: &str,
        text: &str,
    ) -> Option<u32> {
        research::helpers::helper_task(s, ask, agent, contract, project, asker, text)
    }

    fn helper_ask(&self, s: &SessionState, purpose: &ExecPurpose) -> Option<MessageId> {
        research::helpers::helper_ask(s, purpose)
    }

    fn helper_result(
        &self,
        s: &SessionState,
        rec: &ExecRecord,
        result: &ExecutionResult,
    ) -> Option<HelperResult> {
        research::helpers::helper_result(s, rec, result)
    }

    fn previous_run(&self, s: &SessionState, purpose: &ExecPurpose) -> Option<ExecutionId> {
        partners::previous_run(s, purpose)
    }

    fn partners(&self, s: &SessionState, rec: &ExecRecord) -> Vec<(ExecutionId, String)> {
        partners::partners(s, rec)
    }

    fn stage_partners(&self, s: &SessionState, stage: BuiltinStage) -> Vec<(ExecutionId, String)> {
        partners::stage_partners(s, stage)
    }

    fn work_partners(&self, s: &SessionState, scope: Option<&str>) -> Vec<(ExecutionId, String)> {
        partners::work_partners(s, scope)
    }

    fn routes_amendments(&self, s: &SessionState) -> bool {
        research::hooks::routes_amendments(s)
    }

    fn session_wide(&self, purpose: &ExecPurpose) -> bool {
        spawn::session_wide(purpose)
    }

    // Judges, YOLO, and gate answers.

    fn judge_skips_work(&self, kind: JudgeKind) -> bool {
        research::hooks::judge_skips_work(kind)
    }

    fn judge_fallback(&self, s: &SessionState, kind: JudgeKind, error: &str) -> Option<Value> {
        judges::fallback(s, kind, error)
    }

    fn judge_input(
        &self,
        s: &SessionState,
        kind: JudgeKind,
        subject: Option<&str>,
        projects: &[ProjectFacts],
    ) -> (String, String) {
        judge_input::judge_input(s, kind, subject, projects)
    }

    fn judge_schema(&self, kind: JudgeKind, answer: Option<Value>) -> Value {
        judge::output_schema(kind, answer)
    }

    fn judge_output_ok(&self, kind: JudgeKind, v: &Value) -> bool {
        judges::output_ok(kind, v)
    }

    fn judge_reason(&self, output: &Value) -> String {
        judge::reason_of(output)
    }

    fn yolo_leaves_open(&self, s: &SessionState, payload: &GatePayload) -> bool {
        judge_input::yolo_leaves_open(s, payload)
    }

    fn yolo_plan(&self, s: &SessionState, gate: &GateId) -> Option<YoloPlan> {
        judge_input::yolo_plan(s, gate)
    }

    fn yolo_answer_from_judge(
        &self,
        s: &SessionState,
        gate: &GateId,
        answer: &Value,
    ) -> Result<GateAnswer, String> {
        judge_input::yolo_answer_from_judge(s, gate, answer)
    }

    fn validate_answer(
        &self,
        s: &SessionState,
        payload: &GatePayload,
        answer: &GateAnswer,
    ) -> Option<Result<(), String>> {
        crate::fold::validate_answer(s, payload, answer)
    }

    fn answer_needs_route(&self, payload: &GatePayload, answer: &GateAnswer) -> bool {
        crate::fold::answer_needs_route(payload, answer)
    }

    fn override_ok(&self, kind: JudgeKind, output: &Value) -> bool {
        judges::override_ok(kind, output)
    }

    // Step effects and spawns.

    async fn perform(
        &self,
        host: &StepHost,
        session: &SessionId,
        step: Step,
    ) -> Result<Result<(), EngineError>, Step> {
        effects::perform(host, session, step).await.map_err(|s| *s)
    }

    fn completing(
        &self,
        host: &StepHost,
        session: &SessionId,
        st: &SessionState,
    ) -> Result<bool, EngineError> {
        init::hooks::completing(host, session, st)
    }

    fn spawn_dropped(&self, st: &SessionState, req: &SpawnRequest) -> bool {
        research::hooks::spawn_dropped(st, req)
    }

    fn before_spawn(&self, st: &SessionState, req: &SpawnRequest) -> Result<(), String> {
        spawn::before_spawn(st, req)
    }

    fn spawn_files(&self, st: &SessionState, req: &SpawnRequest) {
        spawn::spawn_files(st, req);
    }

    fn tier_override(&self, req: &SpawnRequest) -> Option<ostra_core::model::Tier> {
        init::hooks::tier_override(req)
    }

    fn project_to_create(&self, s: &SessionState, key: &str) -> Option<PathBuf> {
        s.project_to_create(key)
    }

    fn spawn_params(
        &self,
        req: &SpawnRequest,
        profile: Option<&ProjectProfile>,
        params: &mut Value,
    ) {
        build::hooks::spawn_params(req, profile, params);
    }

    fn after_run(&self, req: &SpawnRequest, result: &ExecutionResult) -> Vec<SessionEvent> {
        build::hooks::after_run(req, result)
    }

    // Views.

    fn summary(
        &self,
        s: &SessionState,
        workspace: &WorkspaceId,
        judge_cost: f64,
    ) -> ostra_core::api::SessionSummary {
        view::summary(s, workspace, judge_cost)
    }

    fn detail(
        &self,
        s: &SessionState,
        db: &WorkspaceDb,
        workspace: &WorkspaceId,
        live: HashSet<ExecutionId>,
    ) -> Result<ostra_core::api::SessionDetail, EngineError> {
        view::detail(s, db, workspace, live)
    }

    fn tree_session(
        &self,
        s: &SessionState,
        summary: &ostra_core::api::SessionSummary,
        executions: Vec<ostra_core::api::ExecutionView>,
    ) -> ostra_core::api::TreeSession {
        view::tree_session(s, summary, executions)
    }

    fn run_labels(&self, s: &SessionState) -> HashMap<ExecutionId, String> {
        view::run_labels(s)
    }

    fn artifacts(&self, s: &SessionState) -> Vec<ostra_core::api::ArtifactRef> {
        view::artifacts(s)
    }

    fn decorate(
        &self,
        s: &SessionState,
        labels: &HashMap<ExecutionId, String>,
        view: &mut ostra_core::api::ExecutionView,
    ) {
        view::decorate(s, labels, view)
    }
}
