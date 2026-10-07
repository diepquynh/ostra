//! The standard pipeline: the engine's `Pipeline` hooks, each answered by the built-in stages'
//! code in this crate.

use crate::book::DocsTrack;
use crate::data::{AUTO_FIXABLE_PARAM, InitTrack};
use crate::inputs::OstraInputs;
use crate::judge;
use crate::judge_input;
use crate::steps::OstraStep;
use crate::view;
use ostra_core::config::{ProjectProfile, load_toml};
use ostra_core::event::CommandPurpose;
use ostra_core::event::{
    ExecPurpose, GateAnswer, GatePayload, JudgeKind, SessionEvent, SessionKind, StoredEvent,
};
use ostra_core::event::{FactTarget, WorkKind};
use ostra_core::exec::ExecutionResult;
use ostra_core::exec::ExecutionStatus;
use ostra_core::ids::MessageId;
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
use ostra_core::manage::CreatedProject;
use ostra_core::paths;
use ostra_core::pipeline::Category;
use ostra_core::workflow::{BuiltinStage, WorkflowDef, WorkflowSet};
use ostra_core::{AgentName, Contract};
use ostra_engine::pipeline::{
    HelperResult, PhaseFacts, Pipeline, PipelineBox, ProjectFacts, YoloPlan,
};
use ostra_engine::plan::{Planner, SpawnRequest, Step};
use ostra_engine::runner::{EngineError, StepHost, run_host, run_shell};
use ostra_engine::state::{ExecRecord, SessionState};
use ostra_store::WorkspaceDb;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[allow(unused_imports)]
use crate::prelude::*;

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
        // Rule SM7: a research helper runs as a research task, whose document joins the
        // session's research. Logs from before contracts name only `explore`.
        if !contract.map_or(agent == AgentName::Explore, |c| c == Contract::Research) {
            return None;
        }
        let task = format!("{asker} asks for this research and waits for your findings:\n{text}");
        let t = s.push_explore(
            project.to_string(),
            task,
            ExploreOrigin::Ask { ask: ask.clone() },
        );
        if let Some(x) = s.ext.os_mut().explore.get_mut(t as usize) {
            x.agent = Some(agent);
        }
        Some(t)
    }

    fn helper_ask(&self, s: &SessionState, purpose: &ExecPurpose) -> Option<MessageId> {
        let ExecPurpose::Explore { task } = purpose else {
            return None;
        };
        match &s.ext.os().explore.get(*task as usize)?.origin {
            ExploreOrigin::Ask { ask } => Some(ask.clone()),
            _ => None,
        }
    }

    fn helper_result(
        &self,
        s: &SessionState,
        rec: &ExecRecord,
        result: &ExecutionResult,
    ) -> Option<HelperResult> {
        let ExecPurpose::Explore { task } = &rec.purpose else {
            return None;
        };
        let t = s.ext.os().explore.get(*task as usize);
        match result.status {
            ExecutionStatus::Ok => {
                let Some(sub) = t.and_then(|t| t.result.as_ref()) else {
                    return Some(HelperResult::NotYet);
                };
                let mut text = format!(
                    "{}\n\nResearch document: {}",
                    sub.findings_summary, sub.research_path
                );
                if !sub.not_covered.is_empty() {
                    text.push_str(&format!("\nNot covered: {}", sub.not_covered.join("; ")));
                }
                Some(HelperResult::Text(text))
            }
            // Interrupted or waiting for its own message: the helper is not done yet.
            ExecutionStatus::Interrupted | ExecutionStatus::Waiting => Some(HelperResult::NotYet),
            // An explore that errors is retried once before it counts as failed.
            _ if t.is_some_and(|t| t.failed.is_none() && !t.abandoned) => {
                Some(HelperResult::NotYet)
            }
            _ => None,
        }
    }

    fn previous_run(&self, s: &SessionState, purpose: &ExecPurpose) -> Option<ExecutionId> {
        let last = |f: &dyn Fn(&ExecPurpose) -> bool| {
            s.executions
                .values()
                .filter(|r| f(&r.purpose))
                .max_by_key(|r| r.id.clone())
                .map(|r| r.id.clone())
        };
        let os = s.ext.os();
        match purpose {
            ExecPurpose::Spec { .. } if os.spec.current.is_some() => {
                last(&|p| matches!(p, ExecPurpose::Spec { .. }))
            }
            ExecPurpose::Plan { .. } if os.plan.current.is_some() => {
                last(&|p| matches!(p, ExecPurpose::Plan { .. }))
            }
            ExecPurpose::FactCheck { target, .. } => {
                let t = *target;
                last(&|p| matches!(p, ExecPurpose::FactCheck { target, .. } if *target == t))
            }
            ExecPurpose::Review { phase, tests, .. } => {
                let (ph, te) = (*phase, *tests);
                last(
                    &|p| matches!(p, ExecPurpose::Review { phase, tests, .. } if *phase == ph && *tests == te),
                )
            }
            ExecPurpose::Implement { phase, work } | ExecPurpose::WriteTest { phase, work }
                if matches!(
                    work,
                    WorkKind::Fix | WorkKind::BlockerFix | WorkKind::Rescue | WorkKind::Resume
                ) =>
            {
                let (ph, test) = (*phase, matches!(purpose, ExecPurpose::WriteTest { .. }));
                last(&|p| match p {
                    ExecPurpose::Implement { phase, .. } | ExecPurpose::Verify { phase } => {
                        !test && *phase == ph
                    }
                    ExecPurpose::WriteTest { phase, .. } => test && *phase == ph,
                    _ => false,
                })
            }
            _ => None,
        }
    }

    fn partners(&self, s: &SessionState, rec: &ExecRecord) -> Vec<(ExecutionId, String)> {
        let latest = |f: &dyn Fn(&ExecRecord) -> bool| {
            s.executions
                .values()
                .filter(|r| f(r))
                .max_by_key(|r| r.id.clone())
                .map(|r| s.subagent_of(&r.id))
        };
        let mut out: Vec<(ExecutionId, String)> = vec![];
        let checked = |target: FactTarget| -> Option<ExecutionId> {
            latest(
                &|r| matches!(&r.purpose, ExecPurpose::FactCheck { target: t, .. } if *t == target),
            )
        };
        match &rec.purpose {
            ExecPurpose::Spec { .. } => {
                out.extend(checked(FactTarget::Spec).map(|c| (c, "your fact checker".to_string())))
            }
            ExecPurpose::Plan { .. } => {
                out.extend(checked(FactTarget::Plan).map(|c| (c, "your fact checker".to_string())))
            }
            ExecPurpose::FactCheck { target, .. } => {
                let author = match target {
                    FactTarget::Spec => latest(&|r| matches!(r.purpose, ExecPurpose::Spec { .. })),
                    FactTarget::Plan => latest(&|r| matches!(r.purpose, ExecPurpose::Plan { .. })),
                };
                out.extend(author.map(|a| (a, "the author of the document you check".to_string())));
            }
            _ if rec.loop_key.is_some() => {
                let key = rec.loop_key;
                let review = matches!(rec.purpose, ExecPurpose::Review { .. });
                let other = latest(&|r| {
                    r.loop_key == key && matches!(r.purpose, ExecPurpose::Review { .. }) != review
                });
                out.extend(other.map(|o| {
                    (
                        o,
                        if review {
                            "the implementer of the phase you review"
                        } else {
                            "the reviewer of your phase"
                        }
                        .to_string(),
                    )
                }));
            }
            _ => {}
        }
        out
    }

    fn stage_partners(&self, s: &SessionState, stage: BuiltinStage) -> Vec<(ExecutionId, String)> {
        let (purpose, role): (fn(&ExecPurpose) -> bool, &str) = match stage {
            BuiltinStage::Spec => (
                |p| matches!(p, ExecPurpose::Spec { .. }),
                "the author of the spec",
            ),
            BuiltinStage::Plan => (
                |p| matches!(p, ExecPurpose::Plan { .. }),
                "the author of the plan",
            ),
            _ => return vec![],
        };
        latest_subagent(s, |r| purpose(&r.purpose))
            .map(|x| (x, role.to_string()))
            .into_iter()
            .collect()
    }

    fn work_partners(&self, s: &SessionState, scope: Option<&str>) -> Vec<(ExecutionId, String)> {
        let phase = scope
            .and_then(|s| s.strip_prefix("phase:"))
            .and_then(|p| p.parse::<u32>().ok());
        let project = scope.and_then(|s| s.strip_prefix("project:"));
        latest_subagent(s, |r| match (&r.purpose, phase, project) {
            (ExecPurpose::Implement { phase: p, .. }, Some(want), _) => *p == want,
            (ExecPurpose::Implement { .. }, None, Some(key)) => r.project == key,
            _ => false,
        })
        .map(|x| (x, "the implementer of the work you check".to_string()))
        .into_iter()
        .collect()
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

/// The subagent of the latest run that `f` picks.
fn latest_subagent(s: &SessionState, f: impl Fn(&ExecRecord) -> bool) -> Option<ExecutionId> {
    s.executions
        .values()
        .filter(|r| f(r))
        .max_by_key(|r| r.id.clone())
        .map(|r| s.subagent_of(&r.id))
}

/// Rule F2: the session context file is rewritten from the fold before anything reads it.
fn write_session_context(st: &SessionState) -> Result<(), EngineError> {
    let path = st.session_context_path();
    std::fs::write(&path, crate::context::render(st))
        .map_err(|e| EngineError::Invalid(format!("Could not write {}: {e}", path.display())))
}

async fn perform_own(
    host: &StepHost,
    session: &SessionId,
    step: OstraStep,
) -> Result<(), EngineError> {
    match step {
        OstraStep::Autofix {
            project,
            phase,
            tests,
            findings,
        } => {
            let root = host
                .snapshot(session)?
                .project_path(&project)
                .unwrap_or_default();
            let (applied, failed) = tokio::task::spawn_blocking(move || {
                crate::autofix::apply_findings(&root, &findings)
            })
            .await
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
            host.append(
                session,
                SessionEvent::AutofixApplied {
                    project,
                    phase,
                    tests,
                    applied,
                    failed,
                },
            )?;
            Ok(())
        }
        OstraStep::AnnounceBlocked {
            project,
            phase,
            tests,
            reason,
        } => {
            host.append(
                session,
                SessionEvent::PhaseBlocked {
                    project,
                    phase,
                    tests,
                    reason,
                },
            )?;
            Ok(())
        }
        OstraStep::FinishInit { project } => {
            let st = host.snapshot(session)?;
            let root = st.project_path(&project).unwrap_or_default();
            let inventory = st
                .ext
                .os()
                .project_inits
                .get(&project)
                .and_then(|i| i.inventory.clone());
            match (init_problem(&root), inventory) {
                (Some(p), Some(execution)) => {
                    host.append(
                        session,
                        SessionEvent::InitStepFailed {
                            project,
                            execution,
                            error: format!("The init did not finish: {p}."),
                        },
                    )?;
                }
                _ => {
                    let _ = host.db().set_project_init_status(
                        &project,
                        ostra_core::api::InitStatus::Initialized,
                    );
                    host.projects_changed();
                    host.append(session, SessionEvent::ProjectInitFinished { project })?;
                }
            }
            Ok(())
        }
        OstraStep::RecordInitProblem {
            project,
            execution,
            error,
        } => {
            host.append(
                session,
                SessionEvent::InitStepFailed {
                    project,
                    execution,
                    error,
                },
            )?;
            Ok(())
        }
        OstraStep::ScanDocs { project } => {
            let st = host.snapshot(session)?;
            let path = st.project_path(&project).ok_or_else(|| {
                EngineError::Invalid(format!("Project `{project}` is not in this workspace."))
            })?;
            let profile: ProjectProfile =
                load_toml(&paths::project_profile(&path)).unwrap_or_default();
            let map = profile.module_map;
            let (modules, refs) =
                tokio::task::spawn_blocking(move || crate::docs_scan::scan(&path, &map))
                    .await
                    .unwrap_or_default();
            host.append(
                session,
                SessionEvent::DocsScanned {
                    project,
                    modules,
                    refs,
                },
            )?;
            Ok(())
        }
        OstraStep::Command {
            purpose,
            project,
            command,
            files,
        } => perform_command(host, session, purpose, &project, command, files).await,
    }
}

/// The output of a format step skipped because its command waits for approval.
pub const FORMAT_NOT_APPROVED: &str = "The format command in project.toml changed outside Ostra, so format was skipped. Approve it in the project's settings to run it next time.";

/// Rule D8 and Step 2: run the project's format command, or stage the files a loop changed, and
/// record what ran.
async fn perform_command(
    host: &StepHost,
    session: &SessionId,
    purpose: CommandPurpose,
    project: &str,
    command: Option<String>,
    files: Vec<String>,
) -> Result<(), EngineError> {
    let root = host
        .snapshot(session)?
        .project_path(project)
        .unwrap_or_default();
    let started = |command: &str| {
        host.append(
            session,
            SessionEvent::CommandStarted {
                purpose,
                project: project.into(),
                command: command.into(),
            },
        )
        .map(|_| ())
    };
    let (cmd_text, exit, tail) = match purpose {
        CommandPurpose::Format => match command {
            None => (
                String::new(),
                None,
                "No format command in project.toml, so format was skipped.".to_string(),
            ),
            // Rule A1: a format command runs only once the user approved it.
            Some(cmd) if !host.services().command_approved(&root, &cmd) => {
                (cmd, None, FORMAT_NOT_APPROVED.to_string())
            }
            Some(cmd) => {
                started(&cmd)?;
                // The project's own program, so it runs under the agent sandbox.
                let (code, out) = match ostra_sandbox::host_command(
                    "bash",
                    &["-c".into(), cmd.clone()],
                    &root,
                    &[&root],
                    &host.services().workspace().sandbox(),
                ) {
                    Ok(hc) => run_host(&root, &hc, 600).await,
                    Err(e) => (None, e),
                };
                (cmd, code, out)
            }
        },
        CommandPurpose::Stage => {
            if files.is_empty() {
                (String::new(), Some(0), "No files to stage.".to_string())
            } else {
                // Staging keeps each review focused on the unstaged diff (Step 2).
                for p in ostra_sandbox::repair_git_dirs(&ostra_sandbox::git_repos(&[&root])) {
                    tracing::warn!("removed a planted {} before staging", p.display());
                }
                let mut args: Vec<String> = ostra_core::git::AUTOMATIC
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                args.extend(ostra_core::git::filter_overrides(&root).await);
                args.extend([
                    "-C".into(),
                    root.display().to_string(),
                    "add".into(),
                    "-A".into(),
                    "--".into(),
                ]);
                args.extend(files.iter().cloned());
                let text = format!("git {}", args.join(" "));
                started(&text)?;
                let (code, out) = run_shell(&root, "git", &args, 60).await;
                (text, code, out)
            }
        }
        CommandPurpose::Autofix => (String::new(), Some(0), String::new()),
    };
    host.append(
        session,
        SessionEvent::CommandRan {
            purpose,
            project: project.into(),
            command: cmd_text,
            exit_code: exit,
            output_tail: tail,
        },
    )?;
    Ok(())
}

/// Why an init left the project without a usable inventory or profile.
fn init_problem(root: &Path) -> Option<String> {
    if !paths::project_inventory(root).exists() {
        return Some("the initializer did not write .ostra/INVENTORY.md".into());
    }
    ostra_core::config::load_toml_required::<ProjectProfile>(&paths::project_profile(root))
        .err()
        .map(|e| format!("the generated .ostra/project.toml is not valid: {e}"))
}

/// Rule B10: the drafts of a project as Markdown, the inventory, and an index of the page plan.
fn write_docs_drafts(st: &SessionState, project: &str) {
    let Some(track) = st.ext.os().project_tracks.get(project) else {
        return;
    };
    let dir = st.docs_drafts_dir(project);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Some(scan) = &track.docs_scan {
        let mut text = format!(
            "# Reference sheet for `{project}`\n\nThe modules and the named constants that the engine found in the source. The book must cover each module, and a reader looks up each constant.\n\n## Modules\n\n"
        );
        for m in &scan.modules {
            text.push_str(&format!("- `{}`: {}\n", m.name, m.globs.join(", ")));
        }
        text.push_str("\n## Named constants, by file\n\n");
        let mut file = "";
        for r in &scan.refs {
            if r.file != file {
                file = &r.file;
                text.push_str(&format!("\n### `{file}`\n\n"));
            }
            text.push_str(&format!("- `{}`\n", r.name));
        }
        let _ = std::fs::write(dir.join("reference.md"), text);
    }
    let Some(survey) = track.survey_plan() else {
        return;
    };
    let drafts = track.placed_drafts();
    for d in &drafts {
        let _ = std::fs::write(
            st.docs_draft_path(project, &d.id),
            ostra_core::book::render_section(project, d),
        );
        // Rule B10: the draft before the last revision, so a fact-check re-pass diffs the two.
        if let Some(prev) = track.page_docs.get(&d.id).and_then(|p| p.previous.as_ref()) {
            let mut prev = prev.clone();
            prev.id = d.id.clone();
            let _ = std::fs::write(
                dir.join(format!("{}.prev.md", d.id)),
                ostra_core::book::render_section(project, &prev),
            );
        }
    }
    let mut index = format!(
        "# Page plan for `{project}`\n\n{}\n\n",
        survey.overview.trim()
    );
    for p in &survey.pages {
        let state = if drafts.iter().any(|d| d.id == p.id) {
            format!("draft: `{}.md`", p.id)
        } else if p.rewrite {
            "no draft".into()
        } else {
            "kept from the book".into()
        };
        index.push_str(&format!(
            "- `{}` {} (group: {}, {state}): {}\n",
            p.id, p.title, p.group, p.covers
        ));
    }
    let _ = std::fs::write(dir.join("index.md"), index);
    let mut inv = format!(
        "# Inventory for `{project}`\n\nEvery item the book must cover, with its owning page.\n\n| Item | Name | Owner | Sources | Settings | Names |\n| --- | --- | --- | --- | --- | --- |\n"
    );
    for i in &track.current_inventory() {
        let owner = match &i.out_of_scope {
            Some(why) => format!("out of scope: {why}"),
            None => format!("`{}`", i.owner),
        };
        let code = |v: &[String]| {
            v.iter()
                .map(|n| format!("`{}`", n.replace('|', "\\|")))
                .collect::<Vec<_>>()
                .join(", ")
        };
        inv.push_str(&format!(
            "| `{}` | {} | {owner} | {} | {} | {} |\n",
            i.id,
            i.name.replace('|', "\\|"),
            i.sources.join(", ").replace('|', "\\|"),
            code(&i.settings),
            code(&i.names)
        ));
    }
    let _ = std::fs::write(st.docs_inventory_path(project), inv);
}
