//! The standard pipeline: the engine's `Pipeline` hooks, each answered by the built-in stages' code in this crate.

use super::*;
use crate::data::{AUTO_FIXABLE_PARAM, InitTrack};
use crate::inputs::OstraInputs;
use crate::judge;
use crate::judge_input;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::steps::OstraStep;
use crate::view;
use ostra_core::config::ProjectProfile;
use ostra_core::event::FactTarget;
use ostra_core::event::{
    ExecPurpose, GateAnswer, GatePayload, JudgeKind, SessionEvent, SessionKind, StoredEvent,
};
use ostra_core::exec::ExecutionResult;
use ostra_core::ids::MessageId;
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
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
    fn new_state(&self) -> PipelineBox {
        PipelineBox::new(OstraState::default())
    }

    fn created(&self, s: &mut SessionState) {
        s.ext.os_mut().track = s.options.track;
        if let SessionKind::Init { project } = &s.kind {
            s.ext.os_mut().init = Some(InitTrack {
                project: project.clone(),
                ..Default::default()
            });
        }
    }

    fn project_created(&self, s: &mut SessionState, project: &CreatedProject) {
        s.ext.os_mut().project_inits.insert(
            project.key.clone(),
            InitTrack {
                project: project.key.clone(),
                ..Default::default()
            },
        );
    }

    fn event(&self, s: &mut SessionState, stored: &StoredEvent) {
        s.fold_event(stored);
    }

    fn loop_key(&self, s: &SessionState, purpose: &ExecPurpose) -> Option<(u32, bool)> {
        match purpose {
            ExecPurpose::PromptGen {
                handoff_for: Some(x),
            } => s.executions.get(x).and_then(|r| r.loop_key),
            ExecPurpose::PromptGen { handoff_for: None } => Some((1, false)),
            other => crate::fold::purpose_loop(other),
        }
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
        // Rule O4: nothing but its init and the advisor runs in a created project until the init
        // ends, because every other agent routes its work by the project's inventory and profile.
        match step {
            Step::Spawn(r) => {
                !matches!(
                    r.purpose,
                    ExecPurpose::Init { .. } | ExecPurpose::Advise { .. }
                ) && s.awaiting_init(&r.project)
            }
            _ => OstraStep::of(step)
                .and_then(|o| o.project().map(|p| s.awaiting_init(p)))
                .unwrap_or(false),
        }
    }

    fn check_submit(&self, contract: Contract, input: &Value) -> Vec<String> {
        match contract {
            // Rule B10: each docs step's own shape, with the correction first.
            Contract::Documentation => crate::book::checks::check_value(input),
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
        let os = s.ext.os();
        let mut m = serde_json::Map::new();
        m.insert("track".into(), serde_json::json!(os.track));
        m.insert(
            "stakes".into(),
            serde_json::json!(os.stakes.as_ref().map(|(_, s)| s)),
        );
        m
    }

    fn workflow_resolved(&self, s: &mut SessionState, wf: &WorkflowDef) {
        // Rule WF3: a workflow's `track` fixes the track unless the New task form forced one.
        let os = s.ext.os_mut();
        if os.track.is_none() {
            os.track = wf.track;
        }
    }

    fn book_update(&self, s: &SessionState) -> ostra_core::book::BookUpdate {
        s.book_update()
    }

    fn research_docs(&self, s: &SessionState) -> Vec<PathBuf> {
        s.research_docs()
    }

    fn spec_file(&self, s: &SessionState) -> Option<PathBuf> {
        s.ext
            .os()
            .spec
            .current
            .as_ref()
            .map(|c| PathBuf::from(&c.spec_path))
    }

    fn master_plan(&self, s: &SessionState) -> Option<PathBuf> {
        s.ext
            .os()
            .plan
            .current
            .as_ref()
            .map(|c| PathBuf::from(&c.master_plan_path))
    }

    fn phase(&self, s: &SessionState, id: u32) -> Option<PhaseFacts> {
        s.ext.os().phases.get(&id).map(|p| PhaseFacts {
            info: p.info.clone(),
            implementer_report: p.implementer_report.clone(),
        })
    }

    fn passed_phases(&self, s: &SessionState) -> Vec<u32> {
        let removed = crate::planner::removed_phases(s);
        s.ext
            .os()
            .phases
            .values()
            .filter(|p| !removed.contains(&p.info.id) && p.impl_loop.is_done())
            .map(|p| p.info.id)
            .collect()
    }

    fn changed_files(
        &self,
        s: &SessionState,
        project: &str,
        phase: Option<u32>,
    ) -> BTreeSet<String> {
        let mut files = BTreeSet::new();
        for p in s
            .ext
            .os()
            .phases
            .values()
            .filter(|p| p.info.project == project && phase.is_none_or(|n| n == p.info.id))
        {
            files.extend(p.impl_loop.changed.iter().cloned());
            files.extend(p.test_loop.changed.iter().cloned());
        }
        files
    }

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
        crate::stages::research::helpers::helper_task(s, ask, agent, contract, project, asker, text)
    }

    fn helper_ask(&self, s: &SessionState, purpose: &ExecPurpose) -> Option<MessageId> {
        crate::stages::research::helpers::helper_ask(s, purpose)
    }

    fn helper_result(
        &self,
        s: &SessionState,
        rec: &ExecRecord,
        result: &ExecutionResult,
    ) -> Option<HelperResult> {
        crate::stages::research::helpers::helper_result(s, rec, result)
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
        s.classify.is_some()
            && matches!(
                s.category,
                Some(Category::Research | Category::Spec | Category::Plan | Category::Implement)
            )
    }

    fn judge_skips_work(&self, kind: JudgeKind) -> bool {
        // Rule U1: research the decision skipped stops now.
        kind == JudgeKind::RouteAnswer
    }

    fn judge_fallback(&self, s: &SessionState, kind: JudgeKind, error: &str) -> Option<Value> {
        if kind != JudgeKind::Completion {
            return None;
        }
        // The session's work is done; a report without prose beats no report.
        let md = format!(
            "# Session complete\n\nThe completion judge failed ({error}), so this report lists the state only.\n\n{}",
            judge_input::judge_input(s, JudgeKind::Completion, None, &[]).0
        );
        Some(serde_json::json!({"report_markdown": md, "reason": "The completion judge failed."}))
    }

    fn session_wide(&self, purpose: &ExecPurpose) -> bool {
        matches!(
            purpose,
            ExecPurpose::Spec { .. }
                | ExecPurpose::Plan { .. }
                | ExecPurpose::FactCheck {
                    target: FactTarget::Spec | FactTarget::Plan,
                    ..
                }
        )
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
        match kind {
            JudgeKind::Classify => serde_json::from_value::<judge::ClassifyOut>(v.clone()).is_ok(),
            JudgeKind::Sufficiency => {
                serde_json::from_value::<judge::SufficiencyOut>(v.clone()).is_ok()
            }
            JudgeKind::Stakes => serde_json::from_value::<judge::StakesOut>(v.clone()).is_ok(),
            JudgeKind::Track => serde_json::from_value::<judge::TrackOut>(v.clone()).is_ok(),
            JudgeKind::Feedback => serde_json::from_value::<judge::FeedbackOut>(v.clone())
                .is_ok_and(|o| !o.targets.is_empty()),
            JudgeKind::RouteAnswer => {
                serde_json::from_value::<judge::RouteAnswerOut>(v.clone()).is_ok()
            }
            JudgeKind::Rescue => serde_json::from_value::<judge::RescueOut>(v.clone()).is_ok(),
            JudgeKind::ResolveReview => {
                serde_json::from_value::<judge::ResolveReviewOut>(v.clone()).is_ok()
            }
            JudgeKind::Completion => {
                serde_json::from_value::<judge::CompletionOut>(v.clone()).is_ok()
            }
            JudgeKind::YoloAnswer => true,
        }
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
        match kind {
            JudgeKind::Classify => {
                serde_json::from_value::<judge::ClassifyOut>(output.clone()).is_ok()
            }
            JudgeKind::Stakes => serde_json::from_value::<judge::StakesOut>(output.clone()).is_ok(),
            JudgeKind::Track => serde_json::from_value::<judge::TrackOut>(output.clone()).is_ok(),
            JudgeKind::Sufficiency => {
                serde_json::from_value::<judge::SufficiencyOut>(output.clone()).is_ok()
            }
            _ => false,
        }
    }

    async fn perform(
        &self,
        host: &StepHost,
        session: &SessionId,
        step: Step,
    ) -> Result<Result<(), EngineError>, Step> {
        if let Some(own) = OstraStep::of(&step) {
            return Ok(perform_own(host, session, own).await);
        }
        match step {
            Step::OpenGate {
                payload: GatePayload::ImplementationReview { .. },
                ..
            } => match host
                .snapshot(session)
                .and_then(|st| write_session_context(&st))
            {
                Ok(()) => Err(step),
                Err(e) => Ok(Err(e)),
            },
            other => Err(other),
        }
    }

    fn completing(
        &self,
        host: &StepHost,
        session: &SessionId,
        st: &SessionState,
    ) -> Result<bool, EngineError> {
        if let SessionKind::Init { project } = &st.kind {
            // Later executions route by these two files, so a broken one fails the init
            // here rather than silently giving every agent an empty profile.
            let root = st.project_path(project).unwrap_or_default();
            if let Some(p) = init_problem(&root) {
                host.append(
                    session,
                    SessionEvent::SessionFailed {
                        error: format!("Init did not finish: {p}. Run init again."),
                    },
                )?;
                return Ok(true);
            }
            let _ = host
                .db()
                .set_project_init_status(project, ostra_core::api::InitStatus::Initialized);
        }
        Ok(false)
    }

    fn spawn_dropped(&self, st: &SessionState, req: &SpawnRequest) -> bool {
        // Rule U1: research skipped while this spawn waited for a slot does not start.
        matches!(&req.purpose, ExecPurpose::Explore { task }
            if st.ext.os().explore.get(*task as usize).is_some_and(|t| t.abandoned))
    }

    fn before_spawn(&self, st: &SessionState, req: &SpawnRequest) -> Result<(), String> {
        let x = OstraInputs::of(&req.inputs);
        if x.context_files.contains(&st.session_context_path()) {
            write_session_context(st).map_err(|e| e.to_string())?;
        }
        // Rule D4a: rendered for each spawn, so every mark reflects the files as they are now.
        if x.wants_code_facts_file(&req.inputs) {
            let path = st.code_facts_path();
            std::fs::write(&path, ostra_core::doc::render_code_facts(&x.code_facts))
                .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        }
        Ok(())
    }

    fn spawn_files(&self, st: &SessionState, req: &SpawnRequest) {
        if st.category == Some(Category::Test)
            && let Some(p) = &req.inputs.implementer_report
            && !p.exists()
        {
            let _ = std::fs::write(
                p,
                format!(
                    "# Test request\n\nNo implementer ran in this session. The user asked for tests directly.\n\n## Request\n\n{}\n\n## Changed files\n\nNone, because no implementer ran. Take the code under test from the request: the files or symbols it names, or the earlier change it refers to, from the git history or the staged changes.\n",
                    st.full_request()
                ),
            );
        }
        if st.category == Some(Category::Docs) {
            for p in OstraInputs::of(&req.inputs)
                .implementer_reports
                .iter()
                .filter(|p| !p.exists())
            {
                let _ = std::fs::write(
                    p,
                    format!(
                        "# Documentation request\n\nNo implementer ran in this session. The user asked for documentation directly.\n\n## Request\n\n{}\n\n## Changed files\n\nNone, because no implementer ran. Take the code to document from the request: the flows, areas, files, or symbols it names. When it names none, document the whole project, starting from its entry points.\n",
                        st.full_request()
                    ),
                );
            }
        }
        // Rule B10: every docs run after the survey reads the current drafts and the inventory.
        if let ExecPurpose::Docs { project, .. }
        | ExecPurpose::DocsSurvey { project }
        | ExecPurpose::DocsCheck { project, .. }
        | ExecPurpose::DocsSynthesis { project, .. } = &req.purpose
        {
            write_docs_drafts(st, project);
        }
    }

    fn tier_override(&self, req: &SpawnRequest) -> Option<ostra_core::model::Tier> {
        matches!(
            req.purpose,
            ExecPurpose::Init {
                mode: ostra_core::agent::InitializerMode::GenerateSkill,
                ..
            }
        )
        .then_some(ostra_core::model::Tier::Advanced)
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
        if matches!(req.purpose, ExecPurpose::Review { .. })
            && let Value::Object(map) = params
        {
            let ids = profile.map(|p| p.auto_fixable_ids()).unwrap_or_default();
            map.insert(
                AUTO_FIXABLE_PARAM.into(),
                serde_json::to_value(ids).unwrap_or_default(),
            );
        }
    }

    fn after_run(&self, req: &SpawnRequest, result: &ExecutionResult) -> Vec<SessionEvent> {
        let block = matches!(req.purpose, ExecPurpose::Review { .. })
            .then(|| {
                result.submit.as_ref().and_then(|v| {
                    serde_json::from_value::<ostra_core::submit::CodeReviewerSubmit>(v.clone()).ok()
                })
            })
            .flatten()
            .filter(|r| r.security_block);
        match (block, &req.purpose) {
            (Some(review), ExecPurpose::Review { phase, tests, .. }) => {
                vec![SessionEvent::SecurityBlock {
                    project: req.project.clone(),
                    phase: *phase,
                    tests: *tests,
                    findings: review
                        .findings
                        .into_iter()
                        .filter(|f| f.severity == ostra_core::submit::Severity::Blocker)
                        .collect(),
                }]
            }
            _ => vec![],
        }
    }

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
