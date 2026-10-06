//! The standard pipeline: the engine's `Pipeline` hooks, each answered by the built-in stages'
//! code in this crate.

use crate::judge;
use crate::judge_input;
use crate::view;
use ostra_core::config::{ProjectProfile, load_toml};
use ostra_core::event::{
    ExecPurpose, GateAnswer, GatePayload, JudgeKind, SessionEvent, SessionKind, StoredEvent,
};
use ostra_core::exec::ExecutionResult;
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
use ostra_core::manage::CreatedProject;
use ostra_core::paths;
use ostra_core::pipeline::Category;
use ostra_core::workflow::{BuiltinStage, WorkflowDef, WorkflowSet};
use ostra_core::{AgentName, Contract};
use ostra_engine::pipeline::{Pipeline, ProjectFacts, YoloPlan};
use ostra_engine::plan::{Planner, SpawnRequest, Step};
use ostra_engine::runner::{EngineError, StepHost};
use ostra_engine::state::{AUTO_FIXABLE_PARAM, ExecRecord, InitTrack, SessionState};
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
    fn created(&self, s: &mut SessionState) {
        s.track = s.options.track;
        if let SessionKind::Init { project } = &s.kind {
            s.init = Some(InitTrack {
                project: project.clone(),
                ..Default::default()
            });
        }
    }

    fn project_created(&self, s: &mut SessionState, project: &CreatedProject) {
        s.project_inits.insert(
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
        match step {
            Step::Autofix { .. }
            | Step::AnnounceBlocked { .. }
            | Step::FinishInit { .. }
            | Step::RecordInitProblem { .. }
            | Step::ScanDocs { .. }
            | Step::WriteBook { .. } => Ok(perform_own(host, session, step).await),
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
            if st.explore.get(*task as usize).is_some_and(|t| t.abandoned))
    }

    fn before_spawn(&self, st: &SessionState, req: &SpawnRequest) -> Result<(), String> {
        if req
            .inputs
            .context_files
            .contains(&st.session_context_path())
        {
            write_session_context(st).map_err(|e| e.to_string())?;
        }
        // Rule D4a: rendered for each spawn, so every mark reflects the files as they are now.
        if req.inputs.wants_code_facts_file() {
            let path = st.code_facts_path();
            std::fs::write(
                &path,
                ostra_core::doc::render_code_facts(&req.inputs.code_facts),
            )
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
            for p in req
                .inputs
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

/// Rule F2: the session context file is rewritten from the fold before anything reads it.
fn write_session_context(st: &SessionState) -> Result<(), EngineError> {
    let path = st.session_context_path();
    std::fs::write(&path, crate::context::render(st))
        .map_err(|e| EngineError::Invalid(format!("Could not write {}: {e}", path.display())))
}

async fn perform_own(host: &StepHost, session: &SessionId, step: Step) -> Result<(), EngineError> {
    match step {
        Step::Autofix {
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
        Step::AnnounceBlocked {
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
        Step::FinishInit { project } => {
            let st = host.snapshot(session)?;
            let root = st.project_path(&project).unwrap_or_default();
            let inventory = st
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
        Step::RecordInitProblem {
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
        Step::ScanDocs { project } => {
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
        Step::WriteBook { book } => {
            let st = host.snapshot(session)?;
            let update = st.book_update();
            let projects = update.parts.iter().map(|(k, _)| k.clone()).collect();
            let ws = &st.workspace_root;
            let error = ostra_core::book::apply(ws, &book, &update, chrono::Utc::now())
                .err()
                .map(|e| {
                    format!(
                        "The book could not be written to {}: {e}",
                        ostra_core::book::book_dir(ws, &book).display()
                    )
                });
            host.append(
                session,
                SessionEvent::BookWritten {
                    book,
                    projects,
                    error,
                },
            )?;
            Ok(())
        }
        _ => Ok(()),
    }
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
    let Some(track) = st.project_tracks.get(project) else {
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
