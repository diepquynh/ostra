//! The built-in stages' part of the fold (HANDOVER 8.1): how each event changes the state of
//! research, the spec, the plan, the phases, the closing stages, and the init flow. The engine
//! calls it through `Pipeline` at the point of `SessionState::apply` where it ran before, so the
//! fold stays one deterministic function of the log.

use crate::book::DocsTrack;
use crate::judge::{
    ANSWER_ITEM, AnswerItem, AnswerRoute, ClassifyOut, Disposition, ExploreTaskSpec, FeedbackOut,
    FeedbackTarget, MAX_ANSWER_RESEARCH, MAX_SUFFICIENCY_RESEARCH, NoteStage, RescueAction,
    RescueOut, ResolveAction, ResolveReviewOut, RouteAnswerOut, StakesOut, SufficiencyOut,
    TrackOut, clean_title, item_for, parts_for,
};
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::Contract;
use ostra_core::agent::AgentName;
use ostra_core::book::{DocsStep, DocumentationSubmit};
use ostra_core::event::{
    CommandPurpose, ExecPurpose, FactTarget, GateAnswer, GatePayload, JudgeKind, SessionEvent,
    SessionKind, StoredEvent, WorkKind,
};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::{DecisionId, ExecutionId, GateId};
use ostra_core::model::Complexity;
use ostra_core::paths;
use ostra_core::pipeline::{Category, PhaseInfo, QuestionAnswer, Stakes, TestPolicy, Track};
use ostra_core::submit::{
    CodeReviewerSubmit, ExploreSubmit, FactCheckSubmit, GenerateSpecSubmit, ImplementerSubmit,
    InitializerSubmit, PlanSubmit, QuickAnswerSubmit, ReportSubmit, ReviewFinding, Severity,
    StuckInfo, SubmitStatus, Verdict,
};
use ostra_engine::state::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// An approval answer's decision and its non-empty text, when it has text.
pub fn approval_text(answer: &GateAnswer) -> Option<(bool, String)> {
    match answer {
        GateAnswer::Approval {
            approved,
            feedback: Some(t),
        } if !t.trim().is_empty() => Some((*approved, t.clone())),
        _ => None,
    }
}

/// Rule J1: whether an answer carries content the judge routes before it is applied. A bare
/// choice, such as approve, stop, retry, or a budget raise, has one meaning and is applied as is.
pub fn answer_needs_route(payload: &GatePayload, answer: &GateAnswer) -> bool {
    let text = |t: &Option<String>| t.as_deref().is_some_and(|t| !t.trim().is_empty());
    match (payload, answer) {
        (GatePayload::OpenQuestions { .. }, GateAnswer::Questions { answers }) => {
            !answers.is_empty()
        }
        (
            GatePayload::SpecApproval { .. } | GatePayload::PlanApproval { .. },
            GateAnswer::Approval { feedback, .. },
        ) => text(feedback),
        (GatePayload::FactCheckRecurring { .. }, GateAnswer::Choice { option, text: t }) => {
            option != "stop" && text(t)
        }
        (GatePayload::ReviewCap { .. }, GateAnswer::Choice { option, text: t }) => {
            option == "another-pass" && text(t)
        }
        (GatePayload::Stuck { .. }, GateAnswer::Choice { option, text: t }) => {
            option == "fact" && text(t)
        }
        (GatePayload::PhaseBlocked { .. }, GateAnswer::Choice { option, text: t }) => {
            option == "retry" && text(t)
        }
        (GatePayload::ImplementationReview { .. }, GateAnswer::Choice { option, text: t }) => {
            option == "feedback" && text(t)
        }
        _ => false,
    }
}

/// The spec or plan track as Rule J1 routing sees it.
pub trait RoutingTrack {
    fn set_routing(&mut self, gate: Option<GateId>);
    fn clear_questions_gate(&mut self);
}

/// HIGH and MEDIUM findings for the fix agent, verbatim, plus the ledger path (Step 4 item 6).
pub fn fix_instructions(findings: &[ReviewFinding], ledger: &str) -> String {
    let mut s = String::from("Fix exactly these review findings, and nothing else:\n");
    for f in findings {
        s.push_str("- ");
        s.push_str(&f.line());
        s.push('\n');
    }
    s.push_str(&format!(
        "\nRecord a FIXED or WONTFIX line with your rationale for each finding in the review ledger at {ledger}, because the reviewer reads it on the next pass."
    ));
    s
}

/// BLOCKER findings only, with a removal instruction (Hard rule 21).
pub fn blocker_instructions(findings: &[ReviewFinding], ledger: &str) -> String {
    let mut s = String::from(
        "The security scan raised BLOCKER findings. Remove the dangerous code each one names. Do not rewrite it to keep its effect, because a secure reimplementation is a separate request the user makes once they understand the risk.\n",
    );
    for f in findings.iter().filter(|f| f.severity == Severity::Blocker) {
        s.push_str("- ");
        s.push_str(&f.line());
        s.push('\n');
    }
    s.push_str(&format!(
        "\nRecord each removal in the review ledger at {ledger}."
    ));
    s
}

/// A docs-stage run ends done only with a submit the engine can read, because the book is built
/// from it.
pub fn stage_run<T>(
    status: ExecutionStatus,
    submit: Option<T>,
    exec: &ExecutionId,
    error: String,
) -> StageRun<T> {
    match (status, submit) {
        (ExecutionStatus::Ok, Some(t)) => StageRun::Done(Box::new(t)),
        (ExecutionStatus::Interrupted, _) => StageRun::NotStarted,
        (ExecutionStatus::Ok, None) => StageRun::Failed {
            exec: exec.clone(),
            error: "The run ended without a readable submit call, so there is nothing to put in the book.".into(),
            gate: None,
            retries: 1,
        },
        _ => StageRun::Failed {
            exec: exec.clone(),
            error,
            gate: None,
            retries: 1,
        },
    }
}

/// Rule B10: a docs run that answered another step of the pipeline failed.
pub fn expect_step(run: DocsState, step: DocsStep, exec: &ExecutionId) -> DocsState {
    match run {
        DocsState::Done(d) if d.step != step => DocsState::Failed {
            exec: exec.clone(),
            error: format!(
                "The run answered the `{}` step, but it was started for the `{}` step. Call the submit tool again with `step: {}` and the fields of that step.",
                d.step.as_str(),
                step.as_str(),
                step.as_str()
            ),
            gate: None,
            retries: 1,
        },
        other => other,
    }
}

pub fn is_instruction_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".md")
        || p.ends_with(".mdx")
        || p.ends_with(".txt")
        || p.ends_with(".toml") && p.contains("agent")
}

/// Map an execution purpose to the work loop it belongs to.
pub fn purpose_loop(purpose: &ExecPurpose) -> Option<(u32, bool)> {
    match purpose {
        ExecPurpose::Implement { phase, .. } | ExecPurpose::Verify { phase } => {
            Some((*phase, false))
        }
        ExecPurpose::WriteTest { phase, .. } => Some((*phase, true)),
        ExecPurpose::Review { phase, tests, .. } => Some((*phase, *tests)),
        _ => None,
    }
}

/// Drop the result of the step `exec` ran, so the step runs again.
pub fn clear_init_result(i: &mut InitTrack, exec: &ExecutionId) {
    if i.detect.as_ref() == Some(exec) {
        i.detect_result = None;
    } else if i.propose.as_ref() == Some(exec) {
        i.propose_result = None;
    } else if i.inventory.as_ref() == Some(exec) {
        i.inventory_result = None;
    } else {
        for x in i.scouts.iter_mut().chain(i.generates.iter_mut()) {
            if x.exec.as_ref() == Some(exec) {
                x.result = None;
            }
        }
    }
}

pub fn reset_init_item(i: &mut InitTrack, exec: &ExecutionId) {
    if i.detect.as_ref() == Some(exec) {
        i.detect = None;
    } else if i.adopt.as_ref() == Some(exec) {
        i.adopt = None;
    } else if i.propose.as_ref() == Some(exec) {
        i.propose = None;
    } else if i.inventory.as_ref() == Some(exec) {
        i.inventory = None;
    } else {
        for x in i.scouts.iter_mut().chain(i.generates.iter_mut()) {
            if x.exec.as_ref() == Some(exec) {
                x.exec = None;
            }
        }
    }
}

pub trait RecurringTrack {
    fn clear_recurring_gate(&mut self);
    fn stop(&mut self);
    fn allow_more(&mut self);
}

pub fn artifact_error<T>(t: &mut ArtifactTrack<T>, status: ExecutionStatus, msg: String) {
    if status == ExecutionStatus::Error && t.error_retries < ERROR_RETRIES {
        t.error_retries += 1;
        t.needs_run = true;
    } else {
        t.failed = Some(msg);
    }
}

pub fn exec_retry<T>(t: &mut ArtifactTrack<T>, retry: bool, generation: bool) {
    t.failed_gate = None;
    if retry {
        t.failed = None;
        t.error_retries = 0;
        if generation {
            t.needs_run = true;
        }
    } else {
        t.stopped = true;
    }
}

pub fn fact_finished<T>(
    t: &mut ArtifactTrack<T>,
    exec: &ExecutionId,
    status: ExecutionStatus,
    result: Option<FactCheckSubmit>,
    failed_msg: String,
) {
    t.check_running = None;
    let valid = result.filter(|r| status == ExecutionStatus::Ok && r.verdict != Verdict::Error);
    match valid {
        Some(r) => {
            if let Some(pass) = t.checks.iter_mut().find(|c| &c.exec == exec) {
                pass.result = Some(r.clone());
            }
            t.error_retries = 0;
            match r.verdict {
                Verdict::Pass => t.consecutive_fails = 0,
                _ => {
                    // FAIL goes back to the owning agent with the findings (HANDOVER 8.1).
                    t.consecutive_fails += 1;
                    t.pending_findings = Some(r.findings_text());
                    t.needs_run = true;
                }
            }
        }
        None => {
            t.checks.retain(|c| &c.exec != exec);
            if status == ExecutionStatus::Interrupted {
                return;
            }
            if status == ExecutionStatus::Error && t.error_retries < ERROR_RETRIES {
                t.error_retries += 1;
            } else {
                t.failed = Some(failed_msg);
            }
        }
    }
}

/// What a stuck init step's failure says: its summary, then the fact or decision it needs.
pub fn stuck_problem(summary: &str, need: &str) -> String {
    let summary = summary.trim().trim_end_matches('.');
    let need = need.trim();
    if need.is_empty() {
        format!("{summary}.")
    } else {
        format!("{summary}. It needs: {need}")
    }
}

pub fn missing_submit(status: ExecutionStatus, error: &str, result: &ExecutionResult) -> String {
    match status {
        ExecutionStatus::Ok if result.submit.is_none() => {
            "The agent finished without calling its submit tool, so Ostra has no result to read."
                .into()
        }
        ExecutionStatus::Ok => "The agent's submit payload did not match its schema.".into(),
        _ => error.to_string(),
    }
}

/// Rescue context: the diagnostic verbatim plus the stated fact (Rule D9, STUCK handling).
pub fn rescue_context(stuck: &StuckInfo, fact: &str) -> String {
    format!(
        "Your previous attempt stopped with STUCK. Its diagnostic, verbatim:\n{}\nIt needed: {}\nWhat changed since then: {}\nUse this fact. Do not repeat the approach that failed.",
        stuck.diagnostic, stuck.need, fact
    )
}

pub fn inline_phase(id: u32, project: &str, title: &str, index: usize) -> PhaseInfo {
    PhaseInfo {
        id,
        deliverable: None,
        project: project.to_string(),
        title: title.to_string(),
        complexity: Complexity::Low,
        test_policy: TestPolicy::Required,
        depends_on: Some(if index == 0 { vec![] } else { vec![id - 1] }),
        file: None,
        test_rationale: None,
    }
}

/// The built-in stages' logic over the session state: the facts the planner and the views read,
/// and the handlers the fold calls.
pub trait OstraFold {
    fn tests_requested(&self) -> bool;

    fn docs_requested(&self) -> bool;

    /// Whether a project's closing stages include docs, recorded or implied by an explicit
    /// request for both tests and docs (Rule T3).
    fn project_docs_on(&self, key: &str) -> bool;

    /// Projects whose part of the book this session writes, sorted.
    fn docs_projects(&self) -> Vec<String>;

    /// Rule B6: what this session adds to its book, from the docs-stage submits in the log.
    fn book_update(&self) -> ostra_core::book::BookUpdate;

    /// Rule B6: the picked book, or the one named after the documented projects.
    fn book_id(&self) -> String;

    fn ledger_path(&self, project: &str, phase: u32, tests: bool) -> PathBuf;

    fn loop_mut(&mut self, key: (u32, bool)) -> Option<&mut WorkLoop>;

    fn loop_ref(&self, key: (u32, bool)) -> Option<&WorkLoop>;

    fn auto_fixable(
        &self,
        _project: &str,
        finding: &ReviewFinding,
        autofix_ids: &BTreeSet<String>,
    ) -> bool;

    /// Why `execution` cannot add project `key` to this session now (Rule O2).
    fn project_creation_refusal(&self, execution: &ExecutionId, key: &str) -> Option<String>;

    /// Rule O2: the planned folder of `key` when the approved plan names it as a new project and
    /// it does not exist in this session yet.
    fn project_to_create(&self, key: &str) -> Option<PathBuf>;

    /// Rule O4: a created project whose init has not ended, so its phases wait.
    fn awaiting_init(&self, project: &str) -> bool;

    /// Rule D10: a requirement change after the spec exists restarts at the spec.
    fn requirement_change(&mut self, text: String);

    fn track_mut(&mut self, target: FactTarget) -> &mut dyn RoutingTrack;

    fn approve_spec(&mut self);

    fn hold_approval(&mut self, id: &GateId, target: FactTarget, approve: bool, text: String);

    /// The implementation review gate a feedback round was answered at, when it was routed.
    fn feedback_gate(&self, round: usize) -> Option<GateId>;

    /// Rule J1: keep every part of an answer the judge named later stages for.
    fn remember_parts(
        &mut self,
        gate: Option<&GateId>,
        items: &[AnswerItem],
        id: &str,
        answer: &str,
    );

    /// Rule J1: keep the part of an answer the judge named later stages for.
    fn remember(&mut self, gate: Option<&GateId>, item: &AnswerItem, answer: &str);

    /// Rule J1: the user took back or replaced notes kept earlier.
    fn forget(&mut self, ids: &[String]);

    /// Rule J1: queue the research the judge asked for, capped, in projects the session has.
    fn queue_research(&mut self, tasks: &[ExploreTaskSpec], origin: ExploreOrigin) -> Vec<u32>;

    /// Rule C2: apply the Route answer judge's decision on context the user added mid-session.
    fn apply_amendment_route(&mut self, i: usize, out: &RouteAnswerOut);

    /// Rule U1: a running execution whose task the session can do without. Research the user or
    /// a judge started, the test analysis, a docs writer, and the architecture overview end
    /// without a result; work, review, spec, plan, and fact-check carry rules a skip would break,
    /// a helper's asker waits for its answer, and a rescue's loop waits for its fact.
    fn can_skip(&self, exec: &ExecutionId) -> bool;

    /// Rule U1: end the execution's task without a result, as abandoning its failure gate does.
    fn skip_task(&mut self, exec: &ExecutionId);

    /// Rule U1: research tasks the Route answer judge may skip, numbered from 1 as it sees them.
    fn skippable_research(&self) -> impl Iterator<Item = &ExploreTask> + '_;

    /// Rule U1: skip the research tasks the user told the judge to drop. A running one stops.
    fn skip_research(&mut self, numbers: &[u32]);

    fn apply_held(&mut self, gate: &GateId, held: HeldAnswer, out: &RouteAnswerOut);

    fn apply_loop_route(
        &mut self,
        key: (u32, bool),
        gate: &GateId,
        next: LoopNext,
        out: &RouteAnswerOut,
    );

    /// Rule J1: once every research task an answer queued has finished, the loop continues with
    /// their documents added to its instructions.
    fn release_answer_research(&mut self, key: (u32, bool));

    /// Rule J1: the notes the judge kept for a later stage, oldest first.
    fn notes_for(&self, stage: NoteStage) -> Vec<String>;

    fn on_amended(&mut self, text: &str);

    fn apply_classify(&mut self, out: &ClassifyOut);

    /// Light track: one inline phase per project, queued in order (Rule M5), built from the
    /// request and the research. Full track: the spec and plan stages create the phases.
    fn set_track(&mut self, track: Track);

    fn add_feedback(&mut self, text: String, routed: bool);

    fn route_feedback(
        &mut self,
        i: usize,
        route: AnswerRoute,
        targets: Vec<FeedbackTarget>,
        reason: Option<String>,
    );

    fn add_revision_phases(&mut self, i: usize);

    /// Rule B10: where the runner writes a project's docs drafts.
    fn docs_drafts_dir(&self, project: &str) -> PathBuf;

    fn docs_draft_path(&self, project: &str, page: &str) -> PathBuf;

    fn docs_inventory_path(&self, project: &str) -> PathBuf;

    /// Rule D4a: where the runner writes the code facts before a spawn that gets them.
    fn code_facts_path(&self) -> PathBuf;

    fn insert_phase(&mut self, info: PhaseInfo, impl_loop: WorkLoop);

    fn any_explore_started(&self) -> bool;

    fn any_phase_started(&self) -> bool;

    /// Whether overriding a decision can still change what happens (HANDOVER 8.3 override button).
    fn can_override(&self, id: &DecisionId) -> bool;

    fn on_decision(
        &mut self,
        id: &DecisionId,
        judge: JudgeKind,
        subject: Option<&str>,
        output: &Value,
        overriding: bool,
    );

    /// The executor an agent must run on regardless of routing: native after a harness failure,
    /// native for a quick change, which skips the harness startup cost, and native for a quick
    /// answer, which runs the side panel's agent (HANDOVER 12.3).
    fn forced_executor(&self, agent: AgentName) -> Option<ostra_core::ExecutorKind>;

    fn can_override_classify_now(&self) -> bool;

    fn on_started(
        &mut self,
        id: &ExecutionId,
        purpose: &ExecPurpose,
        loop_key: Option<(u32, bool)>,
        resumed: bool,
    );

    fn on_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult);

    fn loop_finished(&mut self, key: (u32, bool), rec: &ExecRecord, result: &ExecutionResult);

    fn owner_of_exec_gate(&mut self, exec: &ExecutionId, gate: &GateId);

    fn on_gate_opened(&mut self, id: &GateId, payload: &GatePayload);

    fn on_gate_answered(
        &mut self,
        id: &GateId,
        payload: &GatePayload,
        answer: &GateAnswer,
        routed: bool,
    );

    fn exec_gate_answered(&mut self, execution: &ExecutionId, retry: bool);

    /// Replace the phase set with the approved plan's phases (Rules D6, D7).
    fn adopt_plan_phases(&mut self);

    /// The init track an initializer execution belongs to: the init session's own, or a created
    /// project's (Rule O4).
    fn init_track_mut(&mut self, project: &str) -> Option<&mut InitTrack>;

    fn init_started(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
    );

    fn init_finished(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    );

    /// Rule O5: the step key an initializer execution's advice is counted under.
    fn init_step_key(&self, exec: &ExecutionId) -> String;

    /// The work loop waiting on the advisor's look at its stuck run `exec` (Rule O7).
    fn stuck_loop_mut(&mut self, exec: &ExecutionId) -> Option<&mut WorkLoop>;

    /// The work loop whose stuck run `exec` an implementer the user sent is fixing (Rule O8).
    fn fixing_loop_mut(&mut self, exec: &ExecutionId) -> Option<&mut WorkLoop>;

    /// Rule O8: a fix that finished sends the stuck run back to work with what changed; one that
    /// did not reopens the stuck gate with the reason.
    fn loop_fix_finished(
        &mut self,
        stuck_exec: &ExecutionId,
        fixer_exec: &ExecutionId,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    );

    fn loop_advice_finished(
        &mut self,
        failed: &ExecutionId,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    );

    fn advice_finished(
        &mut self,
        project: &str,
        failed: &ExecutionId,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    );
}

impl OstraFold for SessionState {
    fn tests_requested(&self) -> bool {
        self.options.tests || self.ext.os().opts_in.tests
    }

    fn docs_requested(&self) -> bool {
        self.options.docs || self.ext.os().opts_in.docs
    }

    fn project_docs_on(&self, key: &str) -> bool {
        self.ext
            .os()
            .project_tracks
            .get(key)
            .and_then(|t| t.closing)
            .map(|c| c.1)
            .unwrap_or(self.tests_requested() && self.docs_requested())
    }

    fn docs_projects(&self) -> Vec<String> {
        self.ext
            .os()
            .project_tracks
            .keys()
            .filter(|k| {
                self.project_docs_on(k)
                    && self
                        .ext
                        .os()
                        .phases
                        .values()
                        .any(|p| &p.info.project == *k && p.impl_loop.is_done())
            })
            .cloned()
            .collect()
    }

    fn book_update(&self) -> ostra_core::book::BookUpdate {
        let parts = self
            .ext
            .os()
            .project_tracks
            .iter()
            .filter_map(|(k, t)| match t.docs_aggregate() {
                DocsState::Done(d) => {
                    // Rule B10: every planned page, with the draft this session wrote or `None`
                    // to keep it.
                    let plan: Option<Vec<_>> = t.survey_plan().map(|survey| {
                        survey
                            .pages
                            .iter()
                            .map(|p| {
                                let draft = t.page_docs.get(&p.id).and_then(|d| d.draft.clone());
                                (p.clone(), if p.rewrite { draft } else { None })
                            })
                            .collect()
                    });
                    Some(crate::book::part_update(k, &d, plan.as_deref()))
                }
                _ => None,
            })
            .collect();
        ostra_core::book::BookUpdate {
            session: self.id.to_string(),
            parts,
        }
    }

    fn book_id(&self) -> String {
        self.docs_book
            .clone()
            .filter(|b| ostra_core::book::is_book_id(b))
            .unwrap_or_else(|| ostra_core::book::book_id(&self.docs_projects()))
    }

    fn ledger_path(&self, project: &str, phase: u32, tests: bool) -> PathBuf {
        let value = if tests {
            format!("{phase}-tests")
        } else {
            phase.to_string()
        };
        self.project_session_dir(project)
            .join(paths::report::review_ledger(&value))
    }

    fn loop_mut(&mut self, key: (u32, bool)) -> Option<&mut WorkLoop> {
        self.ext.os_mut().phases.get_mut(&key.0).map(|p| {
            if key.1 {
                &mut p.test_loop
            } else {
                &mut p.impl_loop
            }
        })
    }

    fn loop_ref(&self, key: (u32, bool)) -> Option<&WorkLoop> {
        self.ext
            .os()
            .phases
            .get(&key.0)
            .map(|p| if key.1 { &p.test_loop } else { &p.impl_loop })
    }

    fn auto_fixable(
        &self,
        _project: &str,
        finding: &ReviewFinding,
        autofix_ids: &BTreeSet<String>,
    ) -> bool {
        finding.severity != Severity::Blocker
            && !finding.rule.starts_with("SEC-BLOCK")
            && !finding.rule.starts_with("PHASE-REQ")
            && autofix_ids.contains(&finding.rule)
    }

    fn project_creation_refusal(&self, execution: &ExecutionId, key: &str) -> Option<String> {
        if !matches!(self.kind, SessionKind::Pipeline) {
            return Some("Create projects only from a pipeline session.".into());
        }
        if self.is_terminal() {
            return Some("This session has ended, so it takes no new project.".into());
        }
        if !self
            .executions
            .get(execution)
            .is_some_and(|r| r.result.is_none())
        {
            return Some("The run that asked for this project has ended.".into());
        }
        if self.valid_project(key) {
            return Some(format!(
                "Work in `{key}` as it is: that project is already in this session."
            ));
        }
        if self.project_to_create(key).is_none() {
            return Some(format!(
                "Create only a project the approved plan names as new: `{key}` is not one."
            ));
        }
        let phase_project = self
            .executions
            .get(execution)
            .and_then(|r| r.loop_key)
            .and_then(|(phase, _)| self.ext.os().phases.get(&phase))
            .map(|p| p.info.project.as_str());
        if phase_project != Some(key) {
            return Some(format!(
                "Create `{key}` only from a phase the plan puts in it."
            ));
        }
        None
    }

    fn project_to_create(&self, key: &str) -> Option<PathBuf> {
        let plan = self.ext.os().plan.current.as_ref()?;
        (plan.new_projects.iter().any(|k| k == key)
            && ostra_core::slug::is_project_key(key)
            && !self.valid_project(key))
        .then(|| self.workspace_root.join(key))
    }

    fn awaiting_init(&self, project: &str) -> bool {
        self.ext
            .os()
            .project_inits
            .get(project)
            .is_some_and(|i| !i.finished)
    }

    fn requirement_change(&mut self, text: String) {
        self.ext.os_mut().spec.changes.push(text);
        self.ext.os_mut().spec.needs_run = true;
        self.ext.os_mut().spec.revoke_approval();
        if self.ext.os().plan.current.is_some()
            || self.ext.os().plan.running.is_some()
            || !self.ext.os().plan.runs.is_empty()
        {
            self.ext.os_mut().plan.revoke_approval();
            self.ext.os_mut().plan.invalidated = true;
        }
    }

    fn track_mut(&mut self, target: FactTarget) -> &mut dyn RoutingTrack {
        match target {
            FactTarget::Spec => &mut self.ext.os_mut().spec,
            FactTarget::Plan => &mut self.ext.os_mut().plan,
        }
    }

    fn approve_spec(&mut self) {
        self.ext.os_mut().spec.approved = true;
        self.ext.os_mut().spec.approved_version = self.ext.os_mut().spec.version;
        if self.ext.os().plan.invalidated {
            self.ext.os_mut().plan.needs_run = true;
        }
        for i in 0..self.ext.os().feedback.rounds.len() {
            if self.ext.os().feedback.rounds[i].awaiting_spec {
                self.ext.os_mut().feedback.rounds[i].awaiting_spec = false;
                self.add_revision_phases(i);
            }
        }
    }

    fn hold_approval(&mut self, id: &GateId, target: FactTarget, approve: bool, text: String) {
        let version = match target {
            FactTarget::Spec => self.ext.os().spec.version,
            FactTarget::Plan => self.ext.os().plan.version,
        };
        self.track_mut(target).set_routing(Some(id.clone()));
        self.ext.os_mut().held_answers.insert(
            id.clone(),
            HeldAnswer::Approval {
                target,
                approve,
                version,
                text,
            },
        );
    }

    fn feedback_gate(&self, round: usize) -> Option<GateId> {
        self.gates
            .values()
            .filter(|g| g.answer.is_some())
            .find(|g| matches!(&g.payload, GatePayload::ImplementationReview { round: r, .. } if *r as usize == round + 1))
            .map(|g| g.id.clone())
    }

    fn remember_parts(
        &mut self,
        gate: Option<&GateId>,
        items: &[AnswerItem],
        id: &str,
        answer: &str,
    ) {
        let parts: Vec<AnswerItem> = parts_for(items, id).into_iter().cloned().collect();
        for p in &parts {
            self.remember(gate, p, answer);
        }
    }

    fn remember(&mut self, gate: Option<&GateId>, item: &AnswerItem, answer: &str) {
        if item.stages.is_empty() || item.disposition == Disposition::Discard {
            return;
        }
        let text = if item.note.trim().is_empty() {
            answer.to_string()
        } else {
            item.note.clone()
        };
        let mut stages = item.stages.clone();
        stages.sort();
        stages.dedup();
        let notes = &mut self.ext.os_mut().user_notes;
        notes.push(UserNote {
            id: format!("N{}", notes.len() + 1),
            forgotten: false,
            stages,
            text,
            gate: gate.cloned(),
        });
    }

    fn forget(&mut self, ids: &[String]) {
        for n in &mut self.ext.os_mut().user_notes {
            if ids.iter().any(|i| i.trim() == n.id) {
                n.forgotten = true;
            }
        }
    }

    fn queue_research(&mut self, tasks: &[ExploreTaskSpec], origin: ExploreOrigin) -> Vec<u32> {
        let why = if origin == ExploreOrigin::Amendment {
            "The user asked for this research when they added context to the request"
        } else {
            "The user asked for this research while answering a question"
        };
        let primary = self.primary();
        tasks
            .iter()
            .filter(|t| !t.task.trim().is_empty())
            .take(MAX_ANSWER_RESEARCH)
            .map(|t| {
                let project = if self.valid_project(&t.project) {
                    t.project.clone()
                } else {
                    primary.clone()
                };
                let task = format!(
                    "{}\n\n{why}. The whole request, for context: {}",
                    t.task, self.request
                );
                self.push_explore(project, task, origin.clone())
            })
            .collect()
    }

    fn apply_amendment_route(&mut self, i: usize, out: &RouteAnswerOut) {
        let Some(a) = self.amendments.get(i).filter(|a| a.pending) else {
            return;
        };
        let text = self.added_part(&a.text, &a.files, &a.uploads);
        let item = out.item(ANSWER_ITEM);
        let deliver = item.disposition == Disposition::Deliver;
        self.amendments[i].pending = false;
        self.amendments[i].delivered = deliver;
        self.forget(&out.forget);
        self.skip_research(&out.skip);
        self.remember_parts(None, &out.items, ANSWER_ITEM, &text);
        self.queue_research(&out.research, ExploreOrigin::Amendment);
        // Rule D10: a requirement change after the spec exists restarts at the spec.
        if deliver
            && out.route == AnswerRoute::RequirementChange
            && !self.ext.os().spec.runs.is_empty()
        {
            self.requirement_change(format!("The user extended the request: {text}"));
        }
    }

    fn can_skip(&self, exec: &ExecutionId) -> bool {
        let Some(rec) = self.executions.get(exec) else {
            return false;
        };
        if rec.result.is_some() || rec.loop_key.is_some() || self.is_terminal() {
            return false;
        }
        match &rec.purpose {
            ExecPurpose::Explore { task } => {
                self.ext.os().explore.get(*task as usize).is_some_and(|t| {
                    !t.finished()
                        && !matches!(
                            t.origin,
                            ExploreOrigin::Ask { .. } | ExploreOrigin::Rescue { .. }
                        )
                })
            }
            ExecPurpose::Epa { .. } | ExecPurpose::Docs { .. } => true,
            _ => false,
        }
    }

    fn skip_task(&mut self, exec: &ExecutionId) {
        self.exec_gate_answered(exec, false);
    }

    fn skippable_research(&self) -> impl Iterator<Item = &ExploreTask> + '_ {
        self.ext.os().explore.iter().filter(|t| {
            !t.finished()
                && t.failed.is_none()
                && !matches!(
                    t.origin,
                    ExploreOrigin::Ask { .. } | ExploreOrigin::Rescue { .. }
                )
        })
    }

    fn skip_research(&mut self, numbers: &[u32]) {
        let idx: Vec<u32> = self
            .skippable_research()
            .filter(|t| numbers.contains(&(t.idx + 1)))
            .map(|t| t.idx)
            .collect();
        for i in idx {
            let running = self.ext.os().explore[i as usize]
                .exec
                .clone()
                .filter(|e| self.executions.get(e).is_some_and(|r| r.result.is_none()));
            match running {
                Some(e) => {
                    self.interrupting.insert(e.clone(), Interrupt::Skipped);
                    self.skip_task(&e);
                }
                None => {
                    let t = &mut self.ext.os_mut().explore[i as usize];
                    t.abandoned = true;
                    if let ExploreOrigin::LoopAnswer { phase, tests } = t.origin {
                        self.release_answer_research((phase, tests));
                    }
                }
            }
        }
    }

    fn apply_held(&mut self, gate: &GateId, held: HeldAnswer, out: &RouteAnswerOut) {
        let target = held.target();
        self.track_mut(target).set_routing(None);
        let research = self.queue_research(&out.research, ExploreOrigin::Answer);
        let research_note = (!research.is_empty()).then(|| {
            "The user asked for more research before this step. Fold the new research documents into the spec.".to_string()
        });
        match held {
            HeldAnswer::Questions { target, answers } => {
                let questions = match self.gates.get(gate).map(|g| &g.payload) {
                    Some(GatePayload::OpenQuestions { questions, .. }) => questions.clone(),
                    _ => vec![],
                };
                let mut sent = vec![];
                for a in answers {
                    let item = out.item(&a.id);
                    let in_context = questions
                        .iter()
                        .find(|q| q.id == a.id)
                        .map(|q| q.answer_in_context(&a.answer))
                        .unwrap_or_else(|| a.answer.clone());
                    self.remember_parts(Some(gate), &out.items, &a.id, &in_context);
                    let answer = match item.disposition {
                        Disposition::Deliver => in_context,
                        Disposition::Remember => format!(
                            "The user gave this answer for a later stage, which receives it directly: {}. Remove the question and add no requirement for it.",
                            if item.note.trim().is_empty() { &a.answer } else { &item.note }
                        ),
                        Disposition::Discard => "The user chose not to answer this. Remove the question, and settle the point from the research or record it as an assumption.".into(),
                    };
                    sent.push(QuestionAnswer { answer, ..a });
                }
                match target {
                    FactTarget::Spec => {
                        // Rule D3: every answer re-runs generate-spec.
                        self.ext.os_mut().spec.answers.extend(sent);
                        self.ext.os_mut().spec.needs_run = true;
                    }
                    FactTarget::Plan => {
                        let text = sent
                            .iter()
                            .map(|a| format!("{} {}\nAnswer: {}", a.id, a.question, a.answer))
                            .collect::<Vec<_>>()
                            .join("\n");
                        // After the plan exists, answers go into the spec first (answer routing).
                        self.requirement_change(format!(
                            "Answers to the plan's clarifying questions:\n{text}{}",
                            research_note.map(|n| format!("\n{n}")).unwrap_or_default()
                        ));
                    }
                }
            }
            HeldAnswer::Approval {
                target,
                approve,
                version,
                text,
            } => {
                let item = out.item(ANSWER_ITEM);
                self.remember_parts(Some(gate), &out.items, ANSWER_ITEM, &text);
                let change = match item.disposition {
                    Disposition::Deliver => Some(text),
                    _ => research_note,
                };
                match (target, change) {
                    (FactTarget::Spec, Some(c)) => {
                        self.ext.os_mut().spec.changes.push(c);
                        self.ext.os_mut().spec.needs_run = true;
                    }
                    // Rule D10: a change after the plan exists goes into the spec first.
                    (FactTarget::Plan, Some(c)) => self.requirement_change(c),
                    (FactTarget::Spec, None) => {
                        if approve
                            && self.ext.os().spec.version == version
                            && !self.ext.os().spec.needs_run
                        {
                            self.approve_spec();
                        }
                    }
                    (FactTarget::Plan, None) => {
                        if approve
                            && self.ext.os().plan.version == version
                            && !self.ext.os().plan.needs_run
                            && !self.ext.os().plan.invalidated
                        {
                            self.ext.os_mut().plan.approved = true;
                            self.ext.os_mut().plan.approved_version =
                                self.ext.os_mut().plan.version;
                            self.adopt_plan_phases();
                        }
                    }
                }
            }
            HeldAnswer::Recurring { target, text } => {
                let item = out.item(ANSWER_ITEM);
                self.remember_parts(Some(gate), &out.items, ANSWER_ITEM, &text);
                let change = match item.disposition {
                    Disposition::Deliver => Some(text),
                    _ => research_note,
                };
                match (target, change) {
                    (FactTarget::Spec, Some(c)) => {
                        self.ext.os_mut().spec.changes.push(c);
                        self.ext.os_mut().spec.needs_run = true;
                    }
                    (FactTarget::Plan, Some(c)) => self.requirement_change(c),
                    (_, None) => {}
                }
            }
        }
    }

    fn apply_loop_route(
        &mut self,
        key: (u32, bool),
        gate: &GateId,
        next: LoopNext,
        out: &RouteAnswerOut,
    ) {
        let LoopNext::AwaitRoute {
            text,
            then,
            base,
            stuck,
            fallback,
            ..
        } = next
        else {
            return;
        };
        let item = out.item(ANSWER_ITEM);
        self.remember_parts(Some(gate), &out.items, ANSWER_ITEM, &text);
        let deliver = item.disposition == Disposition::Deliver;
        // With no spec there is nothing to change first, so the answer goes to the phase.
        if deliver
            && out.route == AnswerRoute::RequirementChange
            && self.ext.os().spec.current.is_some()
        {
            // Rule D10: stop the phase and restart at the spec.
            if let Some(l) = self.loop_mut(key) {
                l.next = LoopNext::Blocked {
                    reason: "The user changed a requirement; the spec is being updated (Rule D10)."
                        .into(),
                };
                l.announced_block = true;
                l.block_gate_answered = true;
            }
            self.requirement_change(text);
            return;
        }
        let next = if deliver {
            let instructions = match (&stuck, base) {
                (Some(st), _) => rescue_context(st, &text),
                (None, Some(b)) => format!("{b}\n\nThe user added: {text}"),
                (None, None) => text,
            };
            LoopNext::Work {
                kind: then,
                instructions: Some(instructions),
            }
        } else {
            *fallback
        };
        let tasks = if matches!(next, LoopNext::Work { .. }) {
            self.queue_research(
                &out.research,
                ExploreOrigin::LoopAnswer {
                    phase: key.0,
                    tests: key.1,
                },
            )
        } else {
            vec![]
        };
        if let Some(l) = self.loop_mut(key) {
            if matches!(next, LoopNext::Blocked { .. }) {
                l.block_gate_answered = true;
            }
            l.next = if tasks.is_empty() {
                next
            } else {
                LoopNext::AnswerResearch {
                    tasks,
                    next: Box::new(next),
                }
            };
        }
    }

    fn release_answer_research(&mut self, key: (u32, bool)) {
        let Some(LoopNext::AnswerResearch { tasks, next }) =
            self.loop_ref(key).map(|l| l.next.clone())
        else {
            return;
        };
        let found: Vec<&ExploreTask> = tasks
            .iter()
            .filter_map(|i| self.ext.os().explore.get(*i as usize))
            .collect();
        if found.iter().any(|t| !t.finished()) {
            return;
        }
        let docs: Vec<String> = found
            .iter()
            .filter_map(|t| t.result.as_ref())
            .map(|r| format!("- {}: {}", r.research_path, r.findings_summary))
            .collect();
        let next = match *next {
            LoopNext::Work { kind, instructions } if !docs.is_empty() => LoopNext::Work {
                kind,
                instructions: Some(format!(
                    "{}\n\nResearch the user asked for. Read each document before you start:\n{}",
                    instructions.unwrap_or_default(),
                    docs.join("\n")
                )),
            },
            other => other,
        };
        if let Some(l) = self.loop_mut(key) {
            l.next = next;
        }
    }

    fn notes_for(&self, stage: NoteStage) -> Vec<String> {
        self.ext
            .os()
            .user_notes
            .iter()
            .filter(|n| !n.forgotten && n.stages.contains(&stage))
            .map(|n| n.text.clone())
            .collect()
    }

    fn on_amended(&mut self, text: &str) {
        if self.classify.is_none() {
            return;
        }
        // Rule D2: research the new part before the spec is written again.
        let project = self.primary();
        if matches!(
            self.category,
            Some(Category::Research | Category::Spec | Category::Plan | Category::Implement)
        ) {
            self.push_explore(
                project,
                format!(
                    "The user extended the request. Research the part they added, in the context of the whole request.\nAdded part: {text}\nWhole request: {}",
                    self.request
                ),
                ExploreOrigin::Amendment,
            );
        }
        if !self.ext.os().spec.runs.is_empty() {
            self.requirement_change(format!("The user extended the request: {text}"));
        }
    }

    fn apply_classify(&mut self, out: &ClassifyOut) {
        // Rule WF1: a named workflow's base is the category, whatever the judge picked.
        self.category = Some(self.forced_category().unwrap_or(out.category));
        if let Some(t) = clean_title(&out.title) {
            self.title = Some(t);
        }
        // Rule O6: pinned projects are the whole scope, whatever the judge picked.
        let picked = if self.pinned.is_empty() {
            &out.projects
        } else {
            &self.pinned
        };
        let mut scope: Vec<String> = picked
            .iter()
            .filter(|p| self.valid_project(p))
            .cloned()
            .collect();
        scope.dedup();
        if scope.is_empty()
            && let Some(p) = self.projects.first()
        {
            scope.push(p.key.clone());
        }
        // Rule O3: a created project stays in scope when the request is classified again.
        for c in &self.created_projects {
            if !scope.contains(&c.key) {
                scope.push(c.key.clone());
            }
        }
        self.scope = scope;
        self.ext.os_mut().opts_in = out.opts_in;
        self.ext
            .os_mut()
            .explore
            .retain(|t| t.origin != ExploreOrigin::Classify);
        self.ext.os_mut().phases.clear();
        let explores = matches!(
            out.category,
            Category::Research | Category::Spec | Category::Plan | Category::Implement
        );
        if explores {
            let mut tasks: Vec<(String, String)> = out
                .explore_tasks
                .iter()
                .filter(|t| self.valid_project(&t.project) && !t.task.trim().is_empty())
                .map(|t| (t.project.clone(), t.task.clone()))
                .collect();
            if tasks.is_empty() {
                // Rule D1: the spec derives its criteria from research, so there is always one.
                tasks = self
                    .scope
                    .iter()
                    .map(|p| (p.clone(), self.full_request()))
                    .collect();
            }
            for (project, task) in tasks {
                self.push_explore(project, task, ExploreOrigin::Classify);
            }
        }
        let scope = self.scope.clone();
        match out.category {
            Category::Verify => {
                for (i, key) in scope.iter().enumerate() {
                    let id = i as u32 + 1;
                    let mut l =
                        WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                    l.review = ReviewMode::Never;
                    l.stage = false;
                    self.insert_phase(inline_phase(id, key, "Verification", i), l);
                }
            }
            Category::Docs => {
                for (i, key) in scope.iter().enumerate() {
                    let id = i as u32 + 1;
                    let mut l =
                        WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                    l.next = LoopNext::Done;
                    let report = self
                        .project_session_dir(key)
                        .join(paths::report::docs_request());
                    self.insert_phase(
                        inline_phase(id, key, "Documentation requested by the user", i),
                        l,
                    );
                    if let Some(p) = self.ext.os_mut().phases.get_mut(&id) {
                        p.implementer_report = Some(report);
                        p.info.depends_on = Some(vec![]);
                    }
                    self.ext
                        .os_mut()
                        .project_tracks
                        .entry(key.clone())
                        .or_default()
                        .closing = Some((false, true));
                    self.ext
                        .os_mut()
                        .project_tracks
                        .entry(key.clone())
                        .or_default()
                        .format = Some(None);
                }
            }
            Category::Test => {
                for (i, key) in scope.iter().enumerate() {
                    let id = i as u32 + 1;
                    let mut l =
                        WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                    l.next = LoopNext::Done;
                    let report = self
                        .project_session_dir(key)
                        .join(paths::report::test_request());
                    self.insert_phase(inline_phase(id, key, "Tests requested by the user", i), l);
                    if let Some(p) = self.ext.os_mut().phases.get_mut(&id) {
                        p.implementer_report = Some(report);
                        p.info.depends_on = Some(vec![]);
                    }
                    self.ext
                        .os_mut()
                        .project_tracks
                        .entry(key.clone())
                        .or_default()
                        .closing = Some((true, false));
                    self.ext
                        .os_mut()
                        .project_tracks
                        .entry(key.clone())
                        .or_default()
                        .format = Some(None);
                }
            }
            // Quick change (HANDOVER 8.2): one pass per project, no review, staged when it changed files.
            Category::QuickChange => {
                for (i, key) in scope.iter().enumerate() {
                    let mut l =
                        WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                    l.review = ReviewMode::Never;
                    self.insert_phase(inline_phase(i as u32 + 1, key, "Quick change", i), l);
                }
            }
            Category::Prompt => {
                if let Some(key) = scope.first() {
                    let mut l = WorkLoop::new(false, Contract::Prompt, Contract::Implementation);
                    l.review = ReviewMode::IfCodeChanged;
                    self.insert_phase(inline_phase(1, key, "Prompt change", 0), l);
                }
            }
            _ => {}
        }
        if let Some(track) = self.ext.os().track {
            self.set_track(track);
        }
    }

    fn set_track(&mut self, track: Track) {
        self.ext.os_mut().track = Some(track);
        if self.category != Some(Category::Implement) {
            return;
        }
        self.ext.os_mut().phases.clear();
        if track == Track::Light {
            let scope = self.scope.clone();
            for (i, key) in scope.iter().enumerate() {
                let l = WorkLoop::new(false, Contract::Implementation, Contract::Implementation);
                self.insert_phase(inline_phase(i as u32 + 1, key, "Implementation", i), l);
            }
        }
    }

    fn add_feedback(&mut self, text: String, routed: bool) {
        self.ext.os_mut().feedback.rounds.push(FeedbackRound {
            text: text.clone(),
            route: None,
            reason: None,
            targets: vec![],
            awaiting_spec: false,
            phases: vec![],
        });
        // Before Rule J1, a round with no spec and one project was built without the judge.
        if !routed && self.ext.os().spec.current.is_none() && self.scope.len() == 1 {
            let i = self.ext.os().feedback.rounds.len() - 1;
            let targets = vec![FeedbackTarget {
                project: self.primary(),
                instruction: text,
            }];
            self.route_feedback(i, AnswerRoute::ImplementationDetail, targets, None);
        }
    }

    fn route_feedback(
        &mut self,
        i: usize,
        route: AnswerRoute,
        targets: Vec<FeedbackTarget>,
        reason: Option<String>,
    ) {
        let text = self.ext.os().feedback.rounds[i].text.clone();
        let mut targets: Vec<FeedbackTarget> = targets
            .into_iter()
            .filter(|t| self.valid_project(&t.project) && !t.instruction.trim().is_empty())
            .collect();
        if targets.is_empty() {
            targets.push(FeedbackTarget {
                project: self.primary(),
                instruction: text.clone(),
            });
        }
        // Rule D10: a requirement change goes into the spec before anything is built from it.
        let spec_first =
            route == AnswerRoute::RequirementChange && self.ext.os().spec.current.is_some();
        let r = &mut self.ext.os_mut().feedback.rounds[i];
        r.route = Some(route);
        r.reason = reason;
        r.targets = targets;
        if spec_first {
            r.awaiting_spec = true;
            self.ext.os_mut().spec.changes.push(format!(
                "The user reviewed the implementation and asked for: {text}"
            ));
            self.ext.os_mut().spec.needs_run = true;
            self.ext.os_mut().spec.revoke_approval();
        } else {
            self.add_revision_phases(i);
        }
    }

    fn add_revision_phases(&mut self, i: usize) {
        let round = i as u32 + 1;
        let targets = self.ext.os().feedback.rounds[i].targets.clone();
        for t in targets {
            let id = self
                .ext
                .os()
                .phases
                .keys()
                .chain(self.ext.os().superseded_phases.iter().map(|p| &p.info.id))
                .max()
                .copied()
                .unwrap_or(0)
                + 1;
            let info = PhaseInfo {
                id,
                deliverable: None,
                project: t.project.clone(),
                title: format!("Revision {round}"),
                complexity: Complexity::Low,
                test_policy: TestPolicy::Required,
                depends_on: Some(vec![]),
                file: None,
                test_rationale: None,
            };
            self.insert_phase(
                info,
                WorkLoop::new(false, Contract::Implementation, Contract::Implementation),
            );
            if let Some(p) = self.ext.os_mut().phases.get_mut(&id) {
                p.revision = Some(Revision {
                    round,
                    instruction: t.instruction,
                });
            }
            self.ext.os_mut().feedback.rounds[i].phases.push(id);
        }
    }

    fn docs_drafts_dir(&self, project: &str) -> PathBuf {
        self.session_root
            .join(paths::report::docs_drafts())
            .join(project)
    }

    fn docs_draft_path(&self, project: &str, page: &str) -> PathBuf {
        self.docs_drafts_dir(project).join(format!("{page}.md"))
    }

    fn docs_inventory_path(&self, project: &str) -> PathBuf {
        self.docs_drafts_dir(project).join("inventory.md")
    }

    fn code_facts_path(&self) -> PathBuf {
        self.session_root.join(paths::report::code_facts())
    }

    fn insert_phase(&mut self, info: PhaseInfo, impl_loop: WorkLoop) {
        self.ext
            .os_mut()
            .project_tracks
            .entry(info.project.clone())
            .or_default();
        self.ext.os_mut().phases.insert(
            info.id,
            PhaseRun {
                info,
                impl_loop,
                test_loop: WorkLoop::new(true, Contract::Tests, Contract::Tests),
                epa: EpaState::NotStarted,
                implementer_report: None,
                blocked_gate: None,
                revision: None,
            },
        );
    }

    fn any_explore_started(&self) -> bool {
        self.ext.os().explore.iter().any(|t| t.exec.is_some())
    }

    fn any_phase_started(&self) -> bool {
        self.ext
            .os()
            .phases
            .values()
            .any(|p| !p.impl_loop.is_idle() && p.impl_loop.work_count > 0)
    }

    fn can_override(&self, id: &DecisionId) -> bool {
        let Some(d) = self.decisions.get(id) else {
            return false;
        };
        if self.is_terminal() {
            return false;
        }
        match d.judge {
            JudgeKind::Classify => {
                !self.any_explore_started()
                    && self
                        .ext
                        .os()
                        .phases
                        .values()
                        .all(|p| p.impl_loop.work_count == 0)
            }
            JudgeKind::Stakes => self.ext.os().plan.runs.is_empty() && !self.any_phase_started(),
            JudgeKind::Track => self.ext.os().spec.runs.is_empty() && !self.any_phase_started(),
            JudgeKind::Sufficiency => self.ext.os().spec.runs.is_empty(),
            _ => false,
        }
    }

    fn on_decision(
        &mut self,
        id: &DecisionId,
        judge: JudgeKind,
        subject: Option<&str>,
        output: &Value,
        overriding: bool,
    ) {
        match judge {
            JudgeKind::Classify => {
                if overriding && !self.can_override_classify_now() {
                    return;
                }
                if let Ok(out) = serde_json::from_value::<ClassifyOut>(output.clone()) {
                    self.classify = Some(id.clone());
                    self.apply_classify(&out);
                } else if !overriding {
                    self.classify = Some(id.clone());
                    self.failed = Some(
                        "The Classify judge returned output that does not match its schema.".into(),
                    );
                }
            }
            JudgeKind::Sufficiency => {
                let covered: Vec<u32> = subject
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                if !overriding {
                    self.ext.os_mut().sufficiency_rounds += 1;
                }
                for t in self
                    .ext
                    .os_mut()
                    .explore
                    .iter_mut()
                    .filter(|t| covered.contains(&t.idx))
                {
                    t.judged = true;
                }
                if overriding {
                    self.ext
                        .os_mut()
                        .explore
                        .retain(|t| !(t.origin == ExploreOrigin::Sufficiency && t.exec.is_none()));
                }
                if let Ok(out) = serde_json::from_value::<SufficiencyOut>(output.clone()) {
                    let mut added = 0;
                    for item in out.items.into_iter().filter(|i| i.needed) {
                        if added == MAX_SUFFICIENCY_RESEARCH {
                            break;
                        }
                        let (project, task) = match item.task {
                            Some(t) if self.valid_project(&t.project) => (t.project, t.task),
                            _ => (self.primary(), item.item.clone()),
                        };
                        // Rule D2: the judge often maps several items to one task; research it once.
                        if self
                            .ext
                            .os()
                            .explore
                            .iter()
                            .any(|t| !t.finished() && t.project == project && t.task == task)
                        {
                            continue;
                        }
                        self.push_explore(project, task, ExploreOrigin::Sufficiency);
                        added += 1;
                    }
                }
            }
            JudgeKind::Stakes => {
                if let Ok(out) = serde_json::from_value::<StakesOut>(output.clone()) {
                    if overriding
                        && !(self.ext.os().plan.runs.is_empty() && !self.any_phase_started())
                    {
                        return;
                    }
                    self.ext.os_mut().stakes = Some((id.clone(), out.stakes));
                    if overriding {
                        self.ext.os_mut().phases.clear();
                    }
                    if out.stakes == Stakes::Low && self.category == Some(Category::Implement) {
                        // Plan skipped for a lower-stakes request: inline phases, one per project,
                        // queued in order because there is no graph to read (Rule M5).
                        let scope = self.scope.clone();
                        for (i, key) in scope.iter().enumerate() {
                            let l = WorkLoop::new(
                                false,
                                Contract::Implementation,
                                Contract::Implementation,
                            );
                            self.insert_phase(
                                inline_phase(i as u32 + 1, key, "Implementation", i),
                                l,
                            );
                        }
                    }
                }
            }
            JudgeKind::Rescue => {
                let Some(exec) = subject.map(ExecutionId::from) else {
                    return;
                };
                let Ok(out) = serde_json::from_value::<RescueOut>(output.clone()) else {
                    return;
                };
                let key = self.executions.get(&exec).and_then(|r| r.loop_key);
                let Some(key) = key else { return };
                let project = self
                    .ext
                    .os()
                    .phases
                    .get(&key.0)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let next = self.loop_ref(key).map(|l| l.next.clone());
                let Some(LoopNext::Rescue {
                    exec: stuck_exec,
                    stuck,
                }) = next
                else {
                    return;
                };
                if stuck_exec != exec {
                    return;
                }
                let new_next = match out.action {
                    RescueAction::Explore => {
                        let (p, task) = match out.explore_task {
                            Some(t) if self.valid_project(&t.project) => (t.project, t.task),
                            _ => (
                                project,
                                format!(
                                    "Find the fact this agent needs.\nNeed: {}\nDiagnostic:\n{}",
                                    stuck.need, stuck.diagnostic
                                ),
                            ),
                        };
                        let idx = self.push_explore(
                            p,
                            task,
                            ExploreOrigin::Rescue {
                                phase: key.0,
                                tests: key.1,
                            },
                        );
                        LoopNext::RescueExplore { task: idx, stuck }
                    }
                    RescueAction::Rerun => LoopNext::Work {
                        kind: WorkKind::Rescue,
                        instructions: Some(rescue_context(
                            &stuck,
                            out.fact.as_deref().unwrap_or(&out.reason),
                        )),
                    },
                    // Rule O7: at most MAX_ADVICE advisor rounds per loop before the user is asked.
                    RescueAction::Advise
                        if self
                            .loop_ref(key)
                            .is_some_and(|l| l.advice.len() < crate::init::MAX_ADVICE) =>
                    {
                        LoopNext::RescueAdvise {
                            exec,
                            stuck,
                            advisor: None,
                        }
                    }
                    RescueAction::Advise | RescueAction::Gate => {
                        LoopNext::RescueGate { exec, stuck }
                    }
                };
                if let Some(l) = self.loop_mut(key) {
                    l.next = new_next;
                }
            }
            JudgeKind::ResolveReview => {
                let Some(key) = subject.and_then(parse_loop_key) else {
                    return;
                };
                let Ok(out) = serde_json::from_value::<ResolveReviewOut>(output.clone()) else {
                    return;
                };
                let project = self
                    .ext
                    .os()
                    .phases
                    .get(&key.0)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let ledger = self
                    .ledger_path(&project, key.0, key.1)
                    .display()
                    .to_string();
                let Some(l) = self.loop_mut(key) else { return };
                let LoopNext::Resolve { findings } = l.next.clone() else {
                    return;
                };
                match out.action {
                    ResolveAction::Fix => {
                        l.resolve_rounds += 1;
                        l.open_before_resolve = Some(findings.len());
                        // One verification pass per resolution round (review-cap.js).
                        l.extra_cap += 1;
                        let mut text = String::from(
                            "Resolve these review findings with the instructions given for each:\n",
                        );
                        for i in &out.instructions {
                            text.push_str(&format!(
                                "- Finding: {}\n  Instruction: {}\n",
                                i.finding, i.instruction
                            ));
                        }
                        if out.instructions.is_empty() {
                            text = fix_instructions(&findings, &ledger);
                        } else {
                            text.push_str(&format!("\nRecord a FIXED or WONTFIX line for each in the review ledger at {ledger}."));
                        }
                        l.next = LoopNext::Work {
                            kind: WorkKind::Fix,
                            instructions: Some(text),
                        };
                    }
                    ResolveAction::Block => {
                        l.next = LoopNext::Blocked {
                            reason: format!(
                                "The engine could not resolve the open review findings: {}. Ledger: {ledger}",
                                out.reason
                            ),
                        };
                    }
                }
            }
            JudgeKind::RouteAnswer => {
                let Ok(out) = serde_json::from_value::<RouteAnswerOut>(output.clone()) else {
                    return;
                };
                if let Some(i) = subject.and_then(amendment_index) {
                    self.apply_amendment_route(i, &out);
                    return;
                }
                let Some(gate) = subject.map(GateId::from) else {
                    return;
                };
                if !self.ext.os().held_answers.contains_key(&gate)
                    && !self.ext.os().phases.values().any(|p| {
                        [&p.impl_loop, &p.test_loop].iter().any(
                        |l| matches!(&l.next, LoopNext::AwaitRoute { gate: g, .. } if *g == gate),
                    )
                    })
                {
                    return;
                }
                self.forget(&out.forget);
                self.skip_research(&out.skip);
                if let Some(held) = self.ext.os_mut().held_answers.remove(&gate) {
                    self.apply_held(&gate, held, &out);
                    return;
                }
                let target = self
                    .ext
                    .os()
                    .phases
                    .iter()
                    .flat_map(|(id, p)| [((*id, false), &p.impl_loop), ((*id, true), &p.test_loop)])
                    .find_map(|(k, l)| match &l.next {
                        LoopNext::AwaitRoute { gate: g, .. } if *g == gate => {
                            Some((k, l.next.clone()))
                        }
                        _ => None,
                    });
                if let Some((key, next)) = target {
                    self.apply_loop_route(key, &gate, next, &out);
                }
            }
            JudgeKind::Track => {
                let Ok(out) = serde_json::from_value::<TrackOut>(output.clone()) else {
                    return;
                };
                if overriding && !self.can_override(id) {
                    return;
                }
                self.ext.os_mut().track_decision = Some(id.clone());
                self.set_track(out.track);
            }
            JudgeKind::Feedback => {
                let Some(i) = subject.and_then(|s| s.parse::<usize>().ok()) else {
                    return;
                };
                let Ok(out) = serde_json::from_value::<FeedbackOut>(output.clone()) else {
                    return;
                };
                if self
                    .ext
                    .os()
                    .feedback
                    .rounds
                    .get(i)
                    .is_none_or(|r| r.route.is_some())
                {
                    return;
                }
                let Some(gate) = self.feedback_gate(i) else {
                    self.route_feedback(i, out.route, out.targets, Some(out.reason));
                    return;
                };
                let item = item_for(&out.items, ANSWER_ITEM);
                let text = self.ext.os().feedback.rounds[i].text.clone();
                self.forget(&out.forget);
                self.remember_parts(Some(&gate), &out.items, ANSWER_ITEM, &text);
                self.queue_research(&out.research, ExploreOrigin::Answer);
                match item.disposition {
                    Disposition::Deliver => {
                        self.route_feedback(i, out.route, out.targets, Some(out.reason))
                    }
                    Disposition::Remember | Disposition::Discard => {
                        let r = &mut self.ext.os_mut().feedback.rounds[i];
                        r.route = Some(out.route);
                        r.reason = Some(out.reason);
                        // Keeping the feedback only for later stages accepts what was built.
                        if item.disposition == Disposition::Remember {
                            self.ext.os_mut().feedback.accepted = true;
                        }
                    }
                }
            }
            JudgeKind::Completion => self.completion_decision = Some(id.clone()),
            JudgeKind::YoloAnswer => {}
        }
    }

    fn forced_executor(&self, agent: AgentName) -> Option<ostra_core::ExecutorKind> {
        (self.native_fallback.contains(&agent)
            || self.category == Some(Category::QuickChange)
            || self.category == Some(Category::QuickAnswer))
        .then_some(ostra_core::ExecutorKind::Native)
    }

    fn can_override_classify_now(&self) -> bool {
        !self.any_explore_started()
            && self
                .ext
                .os()
                .phases
                .values()
                .all(|p| p.impl_loop.work_count == 0)
    }

    fn on_started(
        &mut self,
        id: &ExecutionId,
        purpose: &ExecPurpose,
        loop_key: Option<(u32, bool)>,
        resumed: bool,
    ) {
        // A resumed run is the same run: it adds no run, pass, or work count, and keeps the
        // inputs its conversation already saw.
        match purpose {
            ExecPurpose::Explore { task } => {
                if let Some(t) = self.ext.os_mut().explore.get_mut(*task as usize) {
                    t.exec = Some(id.clone());
                    t.running = true;
                    t.failed = None;
                }
            }
            ExecPurpose::Spec { .. } => {
                let docs = self.research_docs();
                let t = &mut self.ext.os_mut().spec;
                t.running = Some(id.clone());
                t.needs_run = false;
                if !resumed {
                    t.runs.push(id.clone());
                    t.sent = InputMark {
                        answers: t.answers.len(),
                        changes: t.changes.len(),
                        docs,
                    };
                }
            }
            ExecPurpose::Plan { .. } => {
                self.ext.os_mut().plan.running = Some(id.clone());
                if !resumed {
                    self.ext.os_mut().plan.runs.push(id.clone());
                }
                self.ext.os_mut().plan.needs_run = false;
                // Rule D10: the plan is revised in place against the changed spec, not replaced.
                if self.ext.os().plan.invalidated {
                    self.ext.os_mut().plan.invalidated = false;
                    self.ext.os_mut().plan.pending_findings = None;
                    self.ext.os_mut().plan.consecutive_fails = 0;
                }
            }
            ExecPurpose::FactCheck { target, .. } => match target {
                FactTarget::Spec => self.ext.os_mut().spec.start_check(id),
                FactTarget::Plan => self.ext.os_mut().plan.start_check(id),
            },
            ExecPurpose::Epa { phase } => {
                if let Some(p) = self.ext.os_mut().phases.get_mut(phase) {
                    p.epa = EpaState::Running(id.clone());
                }
            }
            ExecPurpose::Docs {
                project,
                page,
                round,
            } => {
                *self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_run(page.as_deref(), *round) = DocsState::Running(id.clone());
            }
            ExecPurpose::DocsSurvey { project } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .survey = DocsState::Running(id.clone());
            }
            ExecPurpose::DocsCheck {
                project,
                page,
                round,
            } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .checks
                    .insert(page.clone(), CheckState::Running(id.clone()));
            }
            ExecPurpose::DocsSynthesis { project, round } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .synthesis = DocsState::Running(id.clone());
            }
            ExecPurpose::QuickAnswer => {
                self.ext.os_mut().quick.exec = Some(id.clone());
                self.ext.os_mut().quick.running = true;
            }
            ExecPurpose::Init { mode, item } => self.init_started(id, *mode, item.clone()),
            ExecPurpose::Advise {
                project, execution, ..
            } => {
                if let Some(l) = self.stuck_loop_mut(execution)
                    && let LoopNext::RescueAdvise { advisor, .. } = &mut l.next
                {
                    *advisor = Some(id.clone());
                } else if let Some(i) = self.ext.os_mut().project_inits.get_mut(project) {
                    i.advising = Some(id.clone());
                }
            }
            ExecPurpose::Unblock { execution, .. } => {
                if let Some(l) = self.fixing_loop_mut(execution)
                    && let LoopNext::RescueFix { fixer, .. } = &mut l.next
                {
                    *fixer = Some(id.clone());
                }
            }
            ExecPurpose::PromptGen { handoff_for: None } if !resumed => {
                self.ext.os_mut().prompt_gens += 1
            }
            _ => {}
        }
        if let Some(key) = loop_key {
            let is_handoff = matches!(
                purpose,
                ExecPurpose::PromptGen {
                    handoff_for: Some(_)
                }
            );
            if matches!(purpose, ExecPurpose::PromptGen { .. }) && !resumed {
                self.ext.os_mut().prompt_gens += u32::from(is_handoff);
            }
            if let Some(l) = self.loop_mut(key) {
                l.running = Some(id.clone());
                l.in_flight = Some(l.next.clone());
                if !resumed
                    && matches!(
                        purpose,
                        ExecPurpose::Implement { .. }
                            | ExecPurpose::WriteTest { .. }
                            | ExecPurpose::Verify { .. }
                    )
                    || matches!(purpose, ExecPurpose::PromptGen { handoff_for: None })
                {
                    l.work_count += 1;
                }
            }
        }
    }

    fn on_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult) {
        let status = result.status;
        let error = exec_error(result);
        match &rec.purpose {
            ExecPurpose::Explore { task } => {
                let idx = *task as usize;
                let parsed: Option<ExploreSubmit> = parse(&result.submit);
                let Some(t) = self.ext.os_mut().explore.get_mut(idx) else {
                    return;
                };
                t.running = false;
                match (status, parsed) {
                    (ExecutionStatus::Ok, Some(sub)) => t.result = Some(sub),
                    (ExecutionStatus::Interrupted, _) => t.exec = None,
                    (_, _) => {
                        if t.retries < ERROR_RETRIES && status == ExecutionStatus::Error {
                            t.retries += 1;
                            t.exec = None;
                        } else {
                            t.failed = Some(missing_submit(status, &error, result));
                        }
                    }
                }
                let origin = t.origin.clone();
                let summary = t.result.clone();
                if let ExploreOrigin::LoopAnswer { phase, tests } = origin {
                    self.release_answer_research((phase, tests));
                }
                if let (ExploreOrigin::Rescue { phase, tests }, Some(sub)) = (origin, summary) {
                    let task_idx = *task;
                    if let Some(l) = self.loop_mut((phase, tests))
                        && let LoopNext::RescueExplore {
                            task: waiting,
                            stuck,
                        } = l.next.clone()
                        && waiting == task_idx
                    {
                        let fact = format!(
                            "A targeted explore found: {}\nResearch document: {}",
                            sub.findings_summary, sub.research_path
                        );
                        l.next = LoopNext::Work {
                            kind: WorkKind::Rescue,
                            instructions: Some(rescue_context(&stuck, &fact)),
                        };
                    }
                }
            }
            ExecPurpose::Spec { .. } => {
                self.ext.os_mut().spec.running = None;
                match (status, parse::<GenerateSpecSubmit>(&result.submit)) {
                    (ExecutionStatus::Ok, Some(sub)) => {
                        self.ext.os_mut().spec.current = Some(sub);
                        self.ext.os_mut().spec.applied = self.ext.os_mut().spec.sent.clone();
                        self.ext.os_mut().spec.version += 1;
                        self.ext.os_mut().spec.pending_findings = None;
                        self.ext.os_mut().spec.revoke_approval();
                        self.ext.os_mut().spec.error_retries = 0;
                    }
                    (ExecutionStatus::Interrupted, _) => self.ext.os_mut().spec.needs_run = true,
                    _ => artifact_error(
                        &mut self.ext.os_mut().spec,
                        status,
                        missing_submit(status, &error, result),
                    ),
                }
            }
            ExecPurpose::Plan { .. } => {
                self.ext.os_mut().plan.running = None;
                match (status, parse::<PlanSubmit>(&result.submit)) {
                    (ExecutionStatus::Ok, Some(sub)) => {
                        self.ext.os_mut().plan.current = Some(sub);
                        self.ext.os_mut().plan.version += 1;
                        self.ext.os_mut().plan.pending_findings = None;
                        self.ext.os_mut().plan.revoke_approval();
                        self.ext.os_mut().plan.error_retries = 0;
                    }
                    (ExecutionStatus::Interrupted, _) => self.ext.os_mut().plan.needs_run = true,
                    _ => artifact_error(
                        &mut self.ext.os_mut().plan,
                        status,
                        missing_submit(status, &error, result),
                    ),
                }
            }
            ExecPurpose::FactCheck { target, .. } => {
                let parsed: Option<FactCheckSubmit> = parse(&result.submit);
                let (checks, failed_msg) = (parsed, missing_submit(status, &error, result));
                match target {
                    FactTarget::Spec => fact_finished(
                        &mut self.ext.os_mut().spec,
                        &rec.id,
                        status,
                        checks,
                        failed_msg,
                    ),
                    FactTarget::Plan => fact_finished(
                        &mut self.ext.os_mut().plan,
                        &rec.id,
                        status,
                        checks,
                        failed_msg,
                    ),
                }
            }
            ExecPurpose::Epa { phase } => {
                let report = parse::<ReportSubmit>(&result.submit)
                    .map(|r| PathBuf::from(r.report_path))
                    .or_else(|| rec.report_path.clone());
                if let Some(p) = self.ext.os_mut().phases.get_mut(phase) {
                    let retries = match &p.epa {
                        EpaState::Failed { retries, .. } => *retries,
                        _ => 0,
                    };
                    p.epa = match status {
                        ExecutionStatus::Ok => EpaState::Done(report.unwrap_or_default()),
                        ExecutionStatus::Interrupted => EpaState::NotStarted,
                        ExecutionStatus::Error if retries < ERROR_RETRIES => EpaState::NotStarted,
                        _ => EpaState::Failed {
                            exec: rec.id.clone(),
                            error: error.clone(),
                            gate: None,
                            retries: retries + 1,
                        },
                    };
                }
            }
            ExecPurpose::Docs {
                project,
                page,
                round,
            } => {
                let t = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default();
                let mut run = stage_run(
                    status,
                    parse::<DocumentationSubmit>(&result.submit),
                    &rec.id,
                    error,
                );
                if page.is_some() {
                    run = expect_step(run, DocsStep::Page, &rec.id);
                }
                if let (Some(p), DocsState::Done(sub)) = (page, &run) {
                    // Rule B10: a first draft or a revision replaces the page's draft.
                    let d = t.page_docs.entry(p.clone()).or_default();
                    d.previous = d.draft.take();
                    d.draft = sub.sections.first().cloned();
                    d.glossary = sub.glossary.clone();
                }
                *t.docs_run(page.as_deref(), *round) = run;
            }
            ExecPurpose::DocsSurvey { project } => {
                let t = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default();
                t.survey = expect_step(
                    stage_run(
                        status,
                        parse::<DocumentationSubmit>(&result.submit),
                        &rec.id,
                        error,
                    ),
                    DocsStep::Survey,
                    &rec.id,
                );
                // Rule B10: each page the survey plans to write gets a writer.
                t.page_docs = match &t.survey {
                    DocsState::Done(sv) => sv
                        .pages
                        .iter()
                        .filter(|p| p.rewrite)
                        .map(|p| (p.id.clone(), PageDraft::default()))
                        .collect(),
                    _ => BTreeMap::new(),
                };
                t.docs_rounds.clear();
            }
            ExecPurpose::DocsCheck {
                project,
                page,
                round,
            } => {
                let run = stage_run(
                    status,
                    parse::<FactCheckSubmit>(&result.submit),
                    &rec.id,
                    error,
                );
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .checks
                    .insert(page.clone(), run);
            }
            ExecPurpose::DocsSynthesis { project, round } => {
                let t = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default();
                let run = expect_step(
                    stage_run(
                        status,
                        parse::<DocumentationSubmit>(&result.submit),
                        &rec.id,
                        error,
                    ),
                    DocsStep::Synthesis,
                    &rec.id,
                );
                // Rule B10: the pages to revise are fixed now: the pass's edits, each failed
                // fact-check, each owner of an inventory item the pass added, and each page the
                // engine's own checks name.
                let mut targets: BTreeMap<String, Vec<String>> = BTreeMap::new();
                if let DocsState::Done(syn) = &run {
                    let pages: BTreeSet<String> = t.page_docs.keys().cloned().collect();
                    for item in &syn.inventory {
                        let owned = pages.contains(&item.owner);
                        if !owned && item.out_of_scope.is_none() {
                            continue;
                        }
                        t.inventory_added.retain(|i| i.id != item.id);
                        t.inventory_added.push(item.clone());
                        if owned {
                            targets.entry(item.owner.clone()).or_default().push(format!(
                                "Cover the new inventory item `{}` ({}), from {}.",
                                item.id,
                                item.name,
                                item.sources.join(", ")
                            ));
                        }
                    }
                    for e in &syn.edits {
                        if pages.contains(&e.page) {
                            targets
                                .entry(e.page.clone())
                                .or_default()
                                .extend(e.instructions.clone());
                        }
                    }
                    for (page, check) in &t.round_mut(*round).checks {
                        if let CheckState::Done(c) = check
                            && c.verdict != ostra_core::submit::Verdict::Pass
                        {
                            targets.entry(page.clone()).or_default().push(format!(
                                "Fix each fact-check finding: {}",
                                c.findings_text()
                            ));
                        }
                    }
                    for (page, issues) in t.engine_issues() {
                        if pages.contains(&page) {
                            targets.entry(page).or_default().extend(issues);
                        }
                    }
                }
                let r = t.round_mut(*round);
                r.synthesis = run;
                r.targets = targets;
            }
            ExecPurpose::QuickAnswer => {
                self.ext.os_mut().quick.running = false;
                match (status, parse::<QuickAnswerSubmit>(&result.submit)) {
                    (ExecutionStatus::Ok, Some(a)) => self.ext.os_mut().quick.answer = Some(a),
                    (ExecutionStatus::Ok, None) if !result.final_text.trim().is_empty() => {
                        self.ext.os_mut().quick.answer = Some(QuickAnswerSubmit {
                            answer: result.final_text.clone(),
                            sources: vec![],
                        })
                    }
                    (ExecutionStatus::Interrupted, _) => self.ext.os_mut().quick.exec = None,
                    _ => self.ext.os_mut().quick.failed = Some(error),
                }
            }
            ExecPurpose::Init { mode, item } => {
                self.init_finished(&rec.id, *mode, item.clone(), status, result, error)
            }
            ExecPurpose::Unblock { execution, .. } => {
                self.loop_fix_finished(execution, &rec.id, status, result, error)
            }
            ExecPurpose::Advise {
                project, execution, ..
            } => {
                if self.stuck_loop_mut(execution).is_some() {
                    self.loop_advice_finished(execution, status, result, error);
                } else {
                    self.advice_finished(project, execution, status, result, error);
                }
            }
            _ => {}
        }
        if let Some(key) = rec.loop_key {
            self.loop_finished(key, rec, result);
        }
    }

    fn loop_finished(&mut self, key: (u32, bool), rec: &ExecRecord, result: &ExecutionResult) {
        let yolo = self.yolo;
        let fresh = self.restart_fresh.remove(&rec.id);
        let project = self
            .ext
            .os()
            .phases
            .get(&key.0)
            .map(|p| p.info.project.clone())
            .unwrap_or_default();
        let ledger = self
            .ledger_path(&project, key.0, key.1)
            .display()
            .to_string();
        // Recorded at spawn from the project's Review Rule Set, so the fold stays a pure function
        // of the event log.
        let autofix_ids: BTreeSet<String> = rec
            .params
            .get(AUTO_FIXABLE_PARAM)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let status = result.status;
        let error = exec_error(result);
        let is_review = matches!(rec.purpose, ExecPurpose::Review { .. });
        let is_handoff = matches!(
            rec.purpose,
            ExecPurpose::PromptGen {
                handoff_for: Some(_)
            }
        );
        let review_findings: Option<CodeReviewerSubmit> = if is_review {
            parse(&result.submit)
        } else {
            None
        };
        let auto: Vec<bool> = review_findings
            .as_ref()
            .map(|r| {
                r.findings
                    .iter()
                    .map(|f| self.auto_fixable(&project, f, &autofix_ids))
                    .collect()
            })
            .unwrap_or_default();
        let Some(phase) = self.ext.os_mut().phases.get_mut(&key.0) else {
            return;
        };
        let l = if key.1 {
            &mut phase.test_loop
        } else {
            &mut phase.impl_loop
        };
        if l.running.as_ref() != Some(&rec.id) {
            return;
        }
        l.running = None;
        let in_flight = l.in_flight.take().unwrap_or(LoopNext::Idle);

        if status == ExecutionStatus::Interrupted && fresh {
            // Rule O4: the phase starts over inside the project the run created.
            l.next = LoopNext::Work {
                kind: WorkKind::Initial,
                instructions: None,
            };
            return;
        }
        if status == ExecutionStatus::Interrupted {
            // Re-run with the same spawn block (HANDOVER 11.2).
            l.next = match in_flight {
                LoopNext::Work { instructions, .. } => LoopNext::Work {
                    kind: WorkKind::Rerun,
                    instructions,
                },
                LoopNext::Idle => LoopNext::Work {
                    kind: WorkKind::Rerun,
                    instructions: None,
                },
                other => other,
            };
            return;
        }
        let failed_hard = !matches!(
            status,
            ExecutionStatus::Ok | ExecutionStatus::Stuck | ExecutionStatus::Handoff
        );
        if failed_hard {
            if status == ExecutionStatus::Error
                && l.error_retries < ERROR_RETRIES
                && !error.starts_with("harness-")
            {
                l.error_retries += 1;
                l.next = match in_flight {
                    LoopNext::Idle => LoopNext::Work {
                        kind: WorkKind::Rerun,
                        instructions: None,
                    },
                    LoopNext::Work { instructions, .. } => LoopNext::Work {
                        kind: WorkKind::Rerun,
                        instructions,
                    },
                    other => other,
                };
            } else {
                l.next = LoopNext::Failed {
                    exec: rec.id.clone(),
                    error,
                };
            }
            return;
        }

        if is_review {
            let Some(review) = review_findings else {
                l.next = LoopNext::Failed {
                    exec: rec.id.clone(),
                    error: missing_submit(status, &error, result),
                };
                return;
            };
            l.error_retries = 0;
            l.iterations += 1;
            l.last_review = Some(review.clone());
            if review.security_block
                || review
                    .findings
                    .iter()
                    .any(|f| f.severity == Severity::Blocker)
            {
                // Hard rule 21: BLOCKER findings go alone to the fix agent, with no cap.
                l.blocker_open = true;
                l.next = LoopNext::Work {
                    kind: WorkKind::BlockerFix,
                    instructions: Some(blocker_instructions(&review.findings, &ledger)),
                };
                return;
            }
            l.blocker_open = false;
            let mut apply = vec![];
            let mut remaining = vec![];
            let mut low = vec![];
            for (f, is_auto) in review.findings.iter().zip(auto) {
                if is_auto {
                    apply.push(f.clone());
                } else if matches!(f.severity, Severity::High | Severity::Medium) {
                    remaining.push(f.clone());
                } else {
                    low.push(f.clone());
                }
            }
            l.leftover_low = low;
            if !apply.is_empty() {
                l.next = LoopNext::Autofix { apply, remaining };
            } else {
                l.after_findings(remaining, yolo, &ledger);
            }
            return;
        }

        if is_handoff {
            l.next = match (status, in_flight) {
                (ExecutionStatus::Ok, LoopNext::Handoff { handoff, .. }) => LoopNext::Work {
                    kind: WorkKind::Resume,
                    instructions: Some(format!(
                        "prompt-generation finished the handoff you asked for ({}). Continue: {}",
                        handoff.request, handoff.resume_instructions
                    )),
                },
                _ => LoopNext::Failed {
                    exec: rec.id.clone(),
                    error: missing_submit(status, &error, result),
                },
            };
            return;
        }

        // A work pass: implementer, write-test, prompt-generation, or verify.
        let sub: Option<ImplementerSubmit> = parse(&result.submit);
        let report_sub: Option<ReportSubmit> = parse(&result.submit);
        let (sub_status, changed, report, stuck, handoff) = match (sub, report_sub) {
            (Some(s), _) => (
                s.status,
                s.changed_files,
                Some(s.report_path),
                s.stuck,
                s.handoff,
            ),
            (None, Some(r)) => (
                r.status,
                r.changed_files,
                Some(r.report_path),
                r.stuck,
                None,
            ),
            (None, None) => {
                l.next = LoopNext::Failed {
                    exec: rec.id.clone(),
                    error: missing_submit(status, &error, result),
                };
                return;
            }
        };
        l.error_retries = 0;
        l.changed.extend(changed.iter().cloned());
        if let Some(r) = report.filter(|r| !r.is_empty()) {
            l.report = Some(PathBuf::from(&r));
            if !key.1 {
                phase.implementer_report = Some(PathBuf::from(r));
            }
        } else if !key.1 && rec.report_path.is_some() {
            phase.implementer_report = rec.report_path.clone();
        }
        let l = if key.1 {
            &mut phase.test_loop
        } else {
            &mut phase.impl_loop
        };
        match sub_status {
            SubmitStatus::Stuck => {
                let stuck = stuck.unwrap_or(StuckInfo {
                    diagnostic: result.final_text.clone(),
                    need: "The agent reported STUCK without a diagnostic.".into(),
                });
                l.next = LoopNext::Rescue {
                    exec: rec.id.clone(),
                    stuck,
                };
            }
            SubmitStatus::Handoff => match handoff {
                Some(h) => {
                    l.next = LoopNext::Handoff {
                        exec: rec.id.clone(),
                        handoff: h,
                    }
                }
                None => {
                    l.next = LoopNext::Failed {
                        exec: rec.id.clone(),
                        error: "The agent reported HANDOFF without saying what it needs.".into(),
                    }
                }
            },
            SubmitStatus::Ok => {
                let review = match l.review {
                    ReviewMode::Always => true,
                    ReviewMode::Never => false,
                    ReviewMode::IfCodeChanged => l.changed.iter().any(|f| !is_instruction_file(f)),
                };
                l.rationale = match &in_flight {
                    LoopNext::Work {
                        instructions: Some(i),
                        kind,
                    } if *kind != WorkKind::Initial => Some(i.clone()),
                    _ => None,
                };
                l.next = if review {
                    LoopNext::Review
                } else if l.stage && !l.changed.is_empty() && l.review != ReviewMode::IfCodeChanged
                {
                    LoopNext::Stage
                } else {
                    LoopNext::Done
                };
            }
        }
    }

    fn owner_of_exec_gate(&mut self, exec: &ExecutionId, gate: &GateId) {
        let Some(rec) = self.executions.get(exec).cloned() else {
            return;
        };
        if let Some(key) = rec.loop_key {
            if let Some(l) = self.loop_mut(key) {
                l.gate = Some(gate.clone());
            }
            return;
        }
        match &rec.purpose {
            ExecPurpose::Stage { .. } => self.stage_exec_gate(&rec, Some(gate), false),
            ExecPurpose::Explore { task } => {
                if let Some(t) = self.ext.os_mut().explore.get_mut(*task as usize) {
                    t.gate = Some(gate.clone());
                }
            }
            ExecPurpose::Spec { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Spec,
                ..
            } => self.ext.os_mut().spec.failed_gate = Some(gate.clone()),
            ExecPurpose::Plan { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Plan,
                ..
            } => self.ext.os_mut().plan.failed_gate = Some(gate.clone()),
            ExecPurpose::Epa { phase } => {
                if let Some(p) = self.ext.os_mut().phases.get_mut(phase)
                    && let EpaState::Failed { gate: g, .. } = &mut p.epa
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::Docs {
                project,
                page,
                round,
            } => {
                if let DocsState::Failed { gate: g, .. } = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_run(page.as_deref(), *round)
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::DocsSurvey { project } => {
                if let DocsState::Failed { gate: g, .. } = &mut self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .survey
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::DocsCheck {
                project,
                page,
                round,
            } => {
                if let Some(CheckState::Failed { gate: g, .. }) = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .checks
                    .get_mut(page)
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::DocsSynthesis { project, round } => {
                if let DocsState::Failed { gate: g, .. } = &mut self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .synthesis
                {
                    *g = Some(gate.clone());
                }
            }
            ExecPurpose::Init { .. } => {
                if let Some(i) = self.init_track_mut(&rec.project) {
                    i.failed_gate = Some(gate.clone());
                }
            }
            _ => {}
        }
    }

    fn on_gate_opened(&mut self, id: &GateId, payload: &GatePayload) {
        match payload {
            GatePayload::StageReview { .. } => self.stage_gate_opened(id, payload),
            GatePayload::OpenQuestions { artifact, .. } => {
                if artifact == "plan" {
                    self.ext.os_mut().plan.questions_gate = Some(id.clone());
                    self.ext.os_mut().plan.questions_asked_version = self.ext.os_mut().plan.version;
                } else {
                    self.ext.os_mut().spec.questions_gate = Some(id.clone());
                    self.ext.os_mut().spec.questions_asked_version = self.ext.os_mut().spec.version;
                }
            }
            GatePayload::SpecApproval { .. } => {
                self.ext.os_mut().spec.approval_gate = Some(id.clone());
                self.ext.os_mut().spec.approval_asked_version = self.ext.os_mut().spec.version;
            }
            GatePayload::PlanApproval { .. } => {
                self.ext.os_mut().plan.approval_gate = Some(id.clone());
                self.ext.os_mut().plan.approval_asked_version = self.ext.os_mut().plan.version;
            }
            GatePayload::FactCheckRecurring { target, .. } => match target {
                FactTarget::Spec => self.ext.os_mut().spec.recurring_gate = Some(id.clone()),
                FactTarget::Plan => self.ext.os_mut().plan.recurring_gate = Some(id.clone()),
            },
            GatePayload::ReviewCap { phase, tests, .. } => {
                if let Some(l) = self.loop_mut((*phase, *tests)) {
                    l.gate = Some(id.clone());
                }
            }
            GatePayload::PhaseBlocked { phase, .. } => {
                if let Some(p) = self.ext.os_mut().phases.get_mut(phase) {
                    p.blocked_gate = Some(id.clone());
                }
            }
            GatePayload::Stuck { execution, .. }
            | GatePayload::ExecutionFailed { execution, .. }
            | GatePayload::HarnessFailure { execution, .. } => {
                self.owner_of_exec_gate(execution, id)
            }
            GatePayload::ClosingGate { items } => {
                for item in items {
                    self.ext
                        .os_mut()
                        .project_tracks
                        .entry(item.project.clone())
                        .or_default()
                        .closing_gate = Some(id.clone());
                }
            }
            GatePayload::SkillApproval { project, .. } => {
                if let Some(i) = self.init_track_mut(project) {
                    i.approval_gate = Some(id.clone());
                }
            }
            GatePayload::Permission { .. } => {}
            GatePayload::BudgetReached { .. } => self.budget_gate = Some(id.clone()),
            GatePayload::DocsRounds { project, .. } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_gate = Some(id.clone());
            }
            GatePayload::ImplementationReview { .. } => {
                self.ext.os_mut().feedback.gate = Some(id.clone())
            }
        }
    }

    fn on_gate_answered(
        &mut self,
        id: &GateId,
        payload: &GatePayload,
        answer: &GateAnswer,
        routed: bool,
    ) {
        let choice = match answer {
            GateAnswer::Choice { option, text } => Some((
                option.as_str(),
                text.clone().filter(|t| !t.trim().is_empty()),
            )),
            _ => None,
        };
        match payload {
            GatePayload::StageReview { .. } => self.stage_gate_answered(id, payload, answer),
            GatePayload::OpenQuestions { artifact, .. } => {
                let answers = match answer {
                    GateAnswer::Questions { answers } => answers.clone(),
                    _ => vec![],
                };
                let target = if artifact == "plan" {
                    FactTarget::Plan
                } else {
                    FactTarget::Spec
                };
                if routed && !answers.is_empty() {
                    // Rule J1: the judge decides where each answer goes before any agent sees it.
                    let t = self.track_mut(target);
                    t.clear_questions_gate();
                    t.set_routing(Some(id.clone()));
                    self.ext
                        .os_mut()
                        .held_answers
                        .insert(id.clone(), HeldAnswer::Questions { target, answers });
                    return;
                }
                if artifact == "plan" {
                    self.ext.os_mut().plan.questions_gate = None;
                    if !answers.is_empty() {
                        // After the plan exists, answers go into the spec first (answer routing).
                        let text = answers
                            .iter()
                            .map(|a| format!("{} {}\nAnswer: {}", a.id, a.question, a.answer))
                            .collect::<Vec<_>>()
                            .join("\n");
                        self.requirement_change(format!(
                            "Answers to the plan's clarifying questions:\n{text}"
                        ));
                    }
                } else {
                    self.ext.os_mut().spec.questions_gate = None;
                    // Rule D3: every answer re-runs generate-spec.
                    self.ext.os_mut().spec.answers.extend(answers);
                    self.ext.os_mut().spec.needs_run = true;
                }
            }
            GatePayload::SpecApproval { .. } => {
                // A change that landed after the gate opened revoked it; its answer is stale.
                let current = self.ext.os().spec.approval_gate.as_ref() == Some(id)
                    && !self.ext.os().spec.needs_run;
                self.ext.os_mut().spec.approval_gate = None;
                if routed && let Some((approved, text)) = approval_text(answer) {
                    let approve = approved && current && self.ext.os().spec.passed_current();
                    self.hold_approval(id, FactTarget::Spec, approve, text);
                    return;
                }
                match answer {
                    GateAnswer::Approval { approved: true, .. }
                        if current && self.ext.os().spec.passed_current() =>
                    {
                        self.approve_spec()
                    }
                    GateAnswer::Approval {
                        feedback: Some(text),
                        ..
                    } if !text.trim().is_empty() => {
                        self.ext.os_mut().spec.changes.push(text.clone());
                        self.ext.os_mut().spec.needs_run = true;
                    }
                    _ => {}
                }
            }
            GatePayload::PlanApproval { .. } => {
                let current = self.ext.os().plan.approval_gate.as_ref() == Some(id)
                    && !self.ext.os().plan.needs_run
                    && !self.ext.os().plan.invalidated;
                self.ext.os_mut().plan.approval_gate = None;
                if routed && let Some((approved, text)) = approval_text(answer) {
                    let approve = approved && current && self.ext.os().plan.passed_current();
                    self.hold_approval(id, FactTarget::Plan, approve, text);
                    return;
                }
                match answer {
                    GateAnswer::Approval { approved: true, .. }
                        if current && self.ext.os().plan.passed_current() =>
                    {
                        self.ext.os_mut().plan.approved = true;
                        self.ext.os_mut().plan.approved_version = self.ext.os_mut().plan.version;
                        self.adopt_plan_phases();
                    }
                    GateAnswer::Approval {
                        feedback: Some(text),
                        ..
                    } if !text.trim().is_empty() => {
                        // Rule D10: a change after the plan exists goes into the spec first.
                        self.requirement_change(text.clone());
                    }
                    _ => {}
                }
            }
            GatePayload::FactCheckRecurring { target, .. } => {
                let track_changes: Option<String>;
                {
                    let t_spec;
                    let track: &mut dyn RecurringTrack = match target {
                        FactTarget::Spec => {
                            t_spec = &mut self.ext.os_mut().spec;
                            t_spec
                        }
                        FactTarget::Plan => &mut self.ext.os_mut().plan,
                    };
                    track.clear_recurring_gate();
                    track_changes = match choice {
                        Some(("stop", _)) => {
                            track.stop();
                            None
                        }
                        Some((_, text)) => {
                            track.allow_more();
                            text
                        }
                        None => None,
                    };
                }
                if let Some(text) = track_changes {
                    if routed {
                        self.track_mut(*target).set_routing(Some(id.clone()));
                        self.ext.os_mut().held_answers.insert(
                            id.clone(),
                            HeldAnswer::Recurring {
                                target: *target,
                                text,
                            },
                        );
                        return;
                    }
                    match target {
                        FactTarget::Spec => {
                            self.ext.os_mut().spec.changes.push(text);
                            self.ext.os_mut().spec.needs_run = true;
                        }
                        FactTarget::Plan => self.requirement_change(text),
                    }
                }
            }
            GatePayload::ReviewCap {
                phase,
                tests,
                findings,
                ..
            } => {
                let project = self
                    .ext
                    .os()
                    .phases
                    .get(phase)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let ledger = self
                    .ledger_path(&project, *phase, *tests)
                    .display()
                    .to_string();
                if let Some(l) = self.loop_mut((*phase, *tests)) {
                    l.gate = None;
                    match choice {
                        Some(("another-pass", text)) => {
                            l.extra_cap += 1;
                            let mut instr = fix_instructions(findings, &ledger);
                            match text {
                                Some(t) if routed => {
                                    l.next = LoopNext::AwaitRoute {
                                        gate: id.clone(),
                                        text: t,
                                        then: WorkKind::Fix,
                                        base: Some(instr.clone()),
                                        stuck: None,
                                        fallback: Box::new(LoopNext::Work {
                                            kind: WorkKind::Fix,
                                            instructions: Some(instr),
                                        }),
                                    };
                                }
                                text => {
                                    if let Some(t) = text {
                                        instr.push_str(&format!("\n\nThe user added: {t}"));
                                    }
                                    l.next = LoopNext::Work {
                                        kind: WorkKind::Fix,
                                        instructions: Some(instr),
                                    };
                                }
                            }
                        }
                        _ => {
                            l.next = LoopNext::Blocked {
                                reason: format!(
                                    "The review loop reached its cap with {} findings open. Ledger: {ledger}",
                                    findings.len()
                                ),
                            };
                            l.announced_block = false;
                            l.block_gate_answered = true;
                        }
                    }
                }
            }
            GatePayload::Stuck { execution, .. } => {
                let key = self.executions.get(execution).and_then(|r| r.loop_key);
                if let Some(key) = key
                    && let Some(l) = self.loop_mut(key)
                {
                    l.gate = None;
                    if let LoopNext::RescueGate { exec, stuck } = l.next.clone() {
                        match choice {
                            // Rule O8: the user's words are the implementer's task, not a fact
                            // for the stuck agent, so they skip the Route answer judge.
                            Some(("fix", text)) => {
                                l.next = LoopNext::RescueFix {
                                    exec,
                                    stuck,
                                    instructions: text,
                                    fixer: None,
                                };
                            }
                            Some(("fact", Some(text))) => {
                                l.next = LoopNext::AwaitRoute {
                                    gate: id.clone(),
                                    text,
                                    then: WorkKind::Rescue,
                                    base: None,
                                    fallback: Box::new(LoopNext::Blocked {
                                        reason: format!("Stuck: {}", stuck.need),
                                    }),
                                    stuck: Some(stuck),
                                }
                            }
                            _ => {
                                l.next = LoopNext::Blocked {
                                    reason: format!("Stuck: {}", stuck.need),
                                };
                                l.block_gate_answered = true;
                            }
                        }
                    }
                }
            }
            GatePayload::PhaseBlocked { phase, .. } => {
                let tests = self
                    .ext
                    .os()
                    .phases
                    .get(phase)
                    .is_some_and(|p| p.test_loop.is_blocked() && !p.impl_loop.is_blocked());
                if let Some(p) = self.ext.os_mut().phases.get_mut(phase) {
                    p.blocked_gate = None;
                }
                if let Some(l) = self.loop_mut((*phase, tests)) {
                    l.block_gate_answered = true;
                    if let Some(("retry", text)) = choice {
                        let iterations = l.iterations;
                        l.extra_cap = (iterations + REVIEW_CAP).saturating_sub(REVIEW_CAP);
                        l.announced_block = false;
                        l.block_gate_answered = false;
                        l.resolve_rounds = 0;
                        l.open_before_resolve = None;
                        let plain = LoopNext::Work {
                            kind: WorkKind::Fix,
                            instructions: l.last_review.as_ref().map(|r| {
                                let hm: Vec<ReviewFinding> = r
                                    .findings
                                    .iter()
                                    .filter(|f| {
                                        matches!(
                                            f.severity,
                                            Severity::High | Severity::Medium | Severity::Blocker
                                        )
                                    })
                                    .cloned()
                                    .collect();
                                fix_instructions(&hm, "the review ledger")
                            }),
                        };
                        l.next = match text {
                            Some(t) => LoopNext::AwaitRoute {
                                gate: id.clone(),
                                text: t,
                                then: WorkKind::Fix,
                                base: None,
                                stuck: None,
                                fallback: Box::new(plain),
                            },
                            None => plain,
                        };
                    }
                }
            }
            GatePayload::ClosingGate { items } => {
                let choices = match answer {
                    GateAnswer::Closing { items } => items.clone(),
                    _ => vec![],
                };
                let (opt_tests, opt_docs) = (self.tests_requested(), self.docs_requested());
                for item in items {
                    let c = choices.iter().find(|c| c.project == item.project);
                    let tests = if item.ask_tests {
                        c.is_some_and(|c| c.tests)
                    } else {
                        opt_tests
                    };
                    let docs = if item.ask_docs {
                        c.is_some_and(|c| c.docs)
                    } else {
                        opt_docs
                    };
                    let t = self
                        .ext
                        .os_mut()
                        .project_tracks
                        .entry(item.project.clone())
                        .or_default();
                    t.closing = Some((tests, docs));
                    t.closing_gate = None;
                }
            }
            GatePayload::ExecutionFailed { execution, .. }
            | GatePayload::HarnessFailure { execution, .. } => {
                if let GatePayload::HarnessFailure { .. } = payload
                    && let Some(("native", _)) = choice
                    && let Some(r) = self.executions.get(execution)
                {
                    self.native_fallback.insert(r.agent);
                }
                let retry = matches!(choice, Some(("retry" | "native", _)));
                self.exec_gate_answered(execution, retry);
            }
            GatePayload::SkillApproval { project, .. } => {
                if let (Some(i), GateAnswer::Skills { decisions }) =
                    (self.init_track_mut(project), answer)
                {
                    i.approval_gate = None;
                    i.decisions = Some(
                        decisions
                            .iter()
                            .map(|d| (d.name.clone(), d.disposition.clone()))
                            .collect(),
                    );
                }
            }
            GatePayload::Permission { .. } => {}
            GatePayload::ImplementationReview { .. } => {
                if self.ext.os().feedback.gate.as_ref() != Some(id) {
                    return;
                }
                self.ext.os_mut().feedback.gate = None;
                match choice {
                    Some(("feedback", Some(text))) => self.add_feedback(text, routed),
                    _ => self.ext.os_mut().feedback.accepted = true,
                }
            }
            GatePayload::DocsRounds {
                project, rounds, ..
            } => {
                // Rule B10: another round, or the book as it is.
                let t = self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default();
                t.docs_gate = None;
                match choice {
                    Some(("accept", _)) => t.docs_accepted = true,
                    _ => t.docs_continued = *rounds,
                }
            }
            GatePayload::BudgetReached {
                spent_usd,
                budget_usd,
            } => {
                self.budget_gate = None;
                match choice {
                    Some(("raise", text)) => {
                        // The default raise is the budget again, so the session can spend as much
                        // once more before the next pause.
                        let extra = text
                            .and_then(|t| t.trim().trim_start_matches('$').parse::<f64>().ok())
                            .filter(|v| v.is_finite() && *v > 0.0)
                            .unwrap_or(budget_usd.max(1.0));
                        self.budget_raised += extra + (spent_usd - budget_usd).max(0.0);
                    }
                    _ => {
                        self.failed = Some(format!(
                            "Stopped at the session budget after spending ${spent_usd:.2}."
                        ))
                    }
                }
            }
        }
    }

    fn exec_gate_answered(&mut self, execution: &ExecutionId, retry: bool) {
        let Some(rec) = self.executions.get(execution).cloned() else {
            return;
        };
        if let Some(key) = rec.loop_key {
            if let Some(l) = self.loop_mut(key) {
                l.gate = None;
                if let LoopNext::Failed { .. } = l.next {
                    l.next = if retry {
                        l.error_retries = 0;
                        match rec.purpose {
                            ExecPurpose::Review { .. } => LoopNext::Review,
                            _ => LoopNext::Work {
                                kind: WorkKind::Rerun,
                                instructions: l.rationale.clone(),
                            },
                        }
                    } else {
                        LoopNext::Blocked {
                            reason: format!("{} failed and the user abandoned it.", rec.agent),
                        }
                    };
                }
            }
            return;
        }
        match &rec.purpose {
            ExecPurpose::Stage { .. } => self.stage_exec_gate(&rec, None, retry),
            ExecPurpose::Explore { task } => {
                if let Some(t) = self.ext.os_mut().explore.get_mut(*task as usize) {
                    t.gate = None;
                    if retry {
                        t.failed = None;
                        t.exec = None;
                        t.retries = 0;
                    } else {
                        t.abandoned = true;
                    }
                }
                if let Some(ExploreOrigin::LoopAnswer { phase, tests }) = self
                    .ext
                    .os()
                    .explore
                    .get(*task as usize)
                    .map(|t| t.origin.clone())
                {
                    self.release_answer_research((phase, tests));
                }
            }
            ExecPurpose::Spec { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Spec,
                ..
            } => exec_retry(
                &mut self.ext.os_mut().spec,
                retry,
                matches!(rec.purpose, ExecPurpose::Spec { .. }),
            ),
            ExecPurpose::Plan { .. }
            | ExecPurpose::FactCheck {
                target: FactTarget::Plan,
                ..
            } => exec_retry(
                &mut self.ext.os_mut().plan,
                retry,
                matches!(rec.purpose, ExecPurpose::Plan { .. }),
            ),
            ExecPurpose::Epa { phase } => {
                if let Some(p) = self.ext.os_mut().phases.get_mut(phase) {
                    p.epa = if retry {
                        EpaState::NotStarted
                    } else {
                        EpaState::Abandoned
                    };
                }
            }
            ExecPurpose::Docs {
                project,
                page,
                round,
            } => {
                *self
                    .ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_run(page.as_deref(), *round) = if retry {
                    DocsState::NotStarted
                } else {
                    DocsState::Abandoned
                };
            }
            ExecPurpose::DocsSurvey { project } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .survey = if retry {
                    DocsState::NotStarted
                } else {
                    DocsState::Abandoned
                };
            }
            ExecPurpose::DocsCheck {
                project,
                page,
                round,
            } => {
                let st = if retry {
                    CheckState::NotStarted
                } else {
                    CheckState::Abandoned
                };
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .checks
                    .insert(page.clone(), st);
            }
            ExecPurpose::DocsSynthesis { project, round } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .round_mut(*round)
                    .synthesis = if retry {
                    DocsState::NotStarted
                } else {
                    DocsState::Abandoned
                };
            }
            ExecPurpose::QuickAnswer => {
                if retry {
                    self.ext.os_mut().quick.failed = None;
                    self.ext.os_mut().quick.exec = None;
                }
            }
            ExecPurpose::Init { .. } => {
                let embedded = self.ext.os().project_inits.contains_key(&rec.project);
                let mut abandoned = false;
                if let Some(i) = self.init_track_mut(&rec.project) {
                    i.failed_gate = None;
                    if retry {
                        i.escalated = false;
                        let key = i.failed.take();
                        if let Some((exec, _)) = key {
                            reset_init_item(i, &exec);
                        }
                    } else if embedded {
                        // Rule O4: abandoning a created project's init lets its phases run
                        // without it, because the rest of the session still needs them.
                        i.finished = true;
                        i.note = Some(
                            "The user abandoned the init after an initializer step failed.".into(),
                        );
                    } else {
                        abandoned = true;
                    }
                }
                if abandoned {
                    self.failed =
                        Some("The init was abandoned after an initializer step failed.".into());
                }
            }
            _ => {}
        }
    }

    fn adopt_plan_phases(&mut self) {
        let Some(plan) = self.ext.os().plan.current.clone() else {
            return;
        };
        let old = std::mem::take(&mut self.ext.os_mut().phases);
        self.ext
            .os_mut()
            .superseded_phases
            .extend(old.into_values().filter(|p| p.impl_loop.work_count > 0));
        for p in &plan.phases {
            let complexity = p
                .complexity
                .parse::<Complexity>()
                .unwrap_or(Complexity::Medium);
            let test_policy = if p.test_policy.trim().eq_ignore_ascii_case("skip") {
                TestPolicy::Skip
            } else {
                TestPolicy::Required
            };
            // Rule M5: a dependency on a phase that does not exist is unreadable, so it depends
            // on every earlier phase.
            let ids: BTreeSet<u32> = plan.phases.iter().map(|x| x.id).collect();
            let depends_on = if p.depends_on.iter().all(|d| ids.contains(d) && *d != p.id) {
                Some(p.depends_on.clone())
            } else {
                None
            };
            let info = PhaseInfo {
                id: p.id,
                deliverable: Some(p.deliverable.clone()).filter(|d| !d.is_empty()),
                project: p.project.clone(),
                title: p.title.clone(),
                complexity,
                test_policy,
                depends_on,
                file: Some(PathBuf::from(&p.file)),
                test_rationale: p.test_rationale.clone(),
            };
            let valid = self.valid_project(&info.project)
                || self.project_to_create(&info.project).is_some();
            self.insert_phase(
                info,
                WorkLoop::new(false, Contract::Implementation, Contract::Implementation),
            );
            if !valid && let Some(ph) = self.ext.os_mut().phases.get_mut(&p.id) {
                ph.impl_loop.next = LoopNext::Blocked {
                    reason: format!(
                        "The plan names project `{}`, which is not in this workspace.",
                        ph.info.project
                    ),
                };
            }
        }
        let os = self.ext.os_mut();
        let phases = &os.phases;
        os.project_tracks
            .retain(|k, _| phases.values().any(|p| &p.info.project == k));
    }

    fn init_track_mut(&mut self, project: &str) -> Option<&mut InitTrack> {
        if self
            .ext
            .os()
            .init
            .as_ref()
            .is_some_and(|i| i.project == project)
        {
            return self.ext.os_mut().init.as_mut();
        }
        self.ext.os_mut().project_inits.get_mut(project)
    }

    fn init_started(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
    ) {
        use ostra_core::InitializerMode as M;
        let project = self.exec_project(id);
        let Some(i) = self.init_track_mut(&project) else {
            return;
        };
        match mode {
            M::Detect => i.detect = Some(id.clone()),
            M::Adopt => i.adopt = Some(id.clone()),
            M::Propose => i.propose = Some(id.clone()),
            M::GenerateInventory => i.inventory = Some(id.clone()),
            M::Scout | M::GenerateSkill => {
                let key = item.unwrap_or_default();
                let list = if mode == M::Scout {
                    &mut i.scouts
                } else {
                    &mut i.generates
                };
                match list.iter_mut().find(|x| x.key == key) {
                    Some(x) => x.exec = Some(id.clone()),
                    None => list.push(InitItem {
                        key,
                        exec: Some(id.clone()),
                        result: None,
                    }),
                }
            }
        }
    }

    fn init_finished(
        &mut self,
        id: &ExecutionId,
        mode: ostra_core::InitializerMode,
        item: Option<String>,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    ) {
        use ostra_core::InitializerMode as M;
        let parsed: Option<InitializerSubmit> = parse(&result.submit);
        let project = self.exec_project(id);
        let Some(i) = self.init_track_mut(&project) else {
            return;
        };
        let ok = status == ExecutionStatus::Ok
            && parsed
                .as_ref()
                .is_some_and(|p| p.status == SubmitStatus::Ok);
        if !ok {
            let retry_key = format!("{mode}:{}", item.clone().unwrap_or_default());
            let retries = i.retries.entry(retry_key).or_insert(0);
            if (status == ExecutionStatus::Interrupted)
                || (status == ExecutionStatus::Error && *retries < ERROR_RETRIES)
            {
                if status == ExecutionStatus::Error {
                    *retries += 1;
                }
                reset_init_item(i, id);
            } else {
                let msg = match parsed {
                    Some(p) if p.status == SubmitStatus::Stuck => {
                        stuck_problem(&p.summary, &p.stuck.map(|s| s.need).unwrap_or_default())
                    }
                    _ => missing_submit(status, &error, result),
                };
                i.failed = Some((id.clone(), msg));
            }
            return;
        }
        // Models sometimes send the result object JSON-encoded as a string.
        let value = match parsed.map(|p| p.result).unwrap_or(Value::Null) {
            Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
            v => v,
        };
        match mode {
            M::Detect => i.detect_result = Some(value),
            M::Adopt => i.adopt_result = Some(value),
            M::Propose => i.propose_result = Some(value),
            M::GenerateInventory => i.inventory_result = Some(value),
            M::Scout | M::GenerateSkill => {
                let list = if mode == M::Scout {
                    &mut i.scouts
                } else {
                    &mut i.generates
                };
                if let Some(x) = list.iter_mut().find(|x| x.exec.as_ref() == Some(id)) {
                    x.result = Some(value);
                }
            }
        }
    }

    fn init_step_key(&self, exec: &ExecutionId) -> String {
        match self.executions.get(exec).map(|r| &r.purpose) {
            Some(ExecPurpose::Init { mode, item }) => {
                format!("{mode}:{}", item.clone().unwrap_or_default())
            }
            _ => String::new(),
        }
    }

    fn stuck_loop_mut(&mut self, exec: &ExecutionId) -> Option<&mut WorkLoop> {
        let key = self.executions.get(exec).and_then(|r| r.loop_key)?;
        self.loop_mut(key)
            .filter(|l| matches!(&l.next, LoopNext::RescueAdvise { exec: e, .. } if e == exec))
    }

    fn fixing_loop_mut(&mut self, exec: &ExecutionId) -> Option<&mut WorkLoop> {
        let key = self.executions.get(exec).and_then(|r| r.loop_key)?;
        self.loop_mut(key)
            .filter(|l| matches!(&l.next, LoopNext::RescueFix { exec: e, .. } if e == exec))
    }

    fn loop_fix_finished(
        &mut self,
        stuck_exec: &ExecutionId,
        fixer_exec: &ExecutionId,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    ) {
        let parsed: Option<ImplementerSubmit> = parse(&result.submit);
        let Some(l) = self.fixing_loop_mut(stuck_exec) else {
            return;
        };
        let LoopNext::RescueFix {
            exec,
            stuck,
            instructions,
            fixer,
        } = l.next.clone()
        else {
            return;
        };
        if fixer.as_ref() != Some(fixer_exec) {
            return;
        }
        if status == ExecutionStatus::Interrupted {
            l.next = LoopNext::RescueFix {
                exec,
                stuck,
                instructions,
                fixer: None,
            };
            return;
        }
        l.next = match parsed {
            Some(f) if status == ExecutionStatus::Ok && f.status == SubmitStatus::Ok => {
                l.changed.extend(f.changed_files.iter().cloned());
                let asked = instructions
                    .map(|i| format!("The user asked it to: {i}\n"))
                    .unwrap_or_default();
                let files = if f.changed_files.is_empty() {
                    "none".to_string()
                } else {
                    f.changed_files.join(", ")
                };
                let fact = format!(
                    "The user sent another implementer to fix what stopped you, and it finished.\n{asked}Its summary: {}\nFiles it changed: {files}\nIts report: {}\nContinue your work from where you stopped.",
                    f.summary, f.report_path
                );
                LoopNext::Work {
                    kind: WorkKind::Rescue,
                    instructions: Some(rescue_context(&stuck, &fact)),
                }
            }
            other => {
                let why = match other {
                    Some(f) => format!("The implementer you sent did not fix it: {}", f.summary),
                    None => format!("The implementer you sent did not finish: {error}"),
                };
                LoopNext::RescueGate {
                    exec,
                    stuck: StuckInfo {
                        diagnostic: stuck.diagnostic,
                        need: format!("{}\n\n{why}", stuck.need),
                    },
                }
            }
        };
    }

    fn loop_advice_finished(
        &mut self,
        failed: &ExecutionId,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    ) {
        let parsed: Option<ostra_core::submit::AdvisorSubmit> = parse(&result.submit);
        let Some(l) = self.stuck_loop_mut(failed) else {
            return;
        };
        let LoopNext::RescueAdvise { exec, stuck, .. } = l.next.clone() else {
            return;
        };
        if status == ExecutionStatus::Interrupted {
            l.next = LoopNext::RescueAdvise {
                exec,
                stuck,
                advisor: None,
            };
            return;
        }
        l.next = match parsed {
            Some(a)
                if status == ExecutionStatus::Ok
                    && a.action == ostra_core::submit::AdviceAction::Retry =>
            {
                l.advice.push(a.guidance.clone());
                let fact = format!(
                    "An advisor looked at this failure and says how to get past it:\n{}",
                    a.guidance
                );
                LoopNext::Work {
                    kind: WorkKind::Rescue,
                    instructions: Some(rescue_context(&stuck, &fact)),
                }
            }
            other => {
                let why = match other {
                    Some(a) => format!("Advisor: {}", a.reason),
                    None => format!("The advisor could not help: {error}"),
                };
                LoopNext::RescueGate {
                    exec,
                    stuck: StuckInfo {
                        diagnostic: stuck.diagnostic,
                        need: format!("{}\n\n{why}", stuck.need),
                    },
                }
            }
        };
    }

    fn advice_finished(
        &mut self,
        project: &str,
        failed: &ExecutionId,
        status: ExecutionStatus,
        result: &ExecutionResult,
        error: String,
    ) {
        let key = self.init_step_key(failed);
        let parsed: Option<ostra_core::submit::AdvisorSubmit> = parse(&result.submit);
        let Some(i) = self.ext.os_mut().project_inits.get_mut(project) else {
            return;
        };
        i.advising = None;
        if status == ExecutionStatus::Interrupted {
            return;
        }
        match parsed {
            Some(a)
                if status == ExecutionStatus::Ok
                    && a.action == ostra_core::submit::AdviceAction::Retry =>
            {
                i.advice.entry(key).or_default().push(a.guidance);
                clear_init_result(i, failed);
                reset_init_item(i, failed);
                i.failed = None;
            }
            other => {
                i.escalated = true;
                let why = match other {
                    Some(a) => format!("Advisor: {}", a.reason),
                    None => format!("The advisor could not help: {error}"),
                };
                if let Some((_, msg)) = i.failed.as_mut() {
                    msg.push_str("\n\n");
                    msg.push_str(&why);
                }
            }
        }
    }
}

impl<T> RecurringTrack for ArtifactTrack<T> {
    fn clear_recurring_gate(&mut self) {
        self.recurring_gate = None;
    }
    fn stop(&mut self) {
        self.stopped = true;
    }
    fn allow_more(&mut self) {
        self.fail_limit_extra = self.consecutive_fails;
    }
}

impl<T> RoutingTrack for ArtifactTrack<T> {
    fn set_routing(&mut self, gate: Option<GateId>) {
        self.routing = gate;
    }
    fn clear_questions_gate(&mut self) {
        self.questions_gate = None;
    }
}

pub trait OstraEvents {
    /// The fold of the events only the built-in stages read.
    fn fold_event(&mut self, stored: &StoredEvent);
}

impl OstraEvents for SessionState {
    fn fold_event(&mut self, stored: &StoredEvent) {
        match &stored.event {
            SessionEvent::CommandRan {
                purpose,
                project,
                exit_code,
                ..
            } => {
                if let Some(t) = self.ext.os_mut().project_tracks.get_mut(project) {
                    t.running = None;
                }
                match purpose {
                    CommandPurpose::Format => {
                        self.ext
                            .os_mut()
                            .project_tracks
                            .entry(project.clone())
                            .or_default()
                            .format = Some(*exit_code);
                    }
                    CommandPurpose::Stage => {
                        let key = self
                            .ext
                            .os()
                            .phases
                            .iter()
                            .flat_map(|(id, p)| {
                                [((*id, false), &p.impl_loop), ((*id, true), &p.test_loop)]
                            })
                            .find(|(k, l)| {
                                self.ext
                                    .os()
                                    .phases
                                    .get(&k.0)
                                    .is_some_and(|p| &p.info.project == project)
                                    && l.next == LoopNext::Stage
                            })
                            .map(|(k, _)| k);
                        if let Some(key) = key
                            && let Some(l) = self.loop_mut(key)
                        {
                            l.next = LoopNext::Done;
                            l.staged = *exit_code == Some(0);
                        }
                    }
                    CommandPurpose::Autofix => {}
                }
            }
            SessionEvent::AutofixApplied {
                phase,
                tests,
                failed,
                ..
            } => {
                let yolo = self.yolo;
                let project = self
                    .ext
                    .os()
                    .phases
                    .get(phase)
                    .map(|p| p.info.project.clone())
                    .unwrap_or_default();
                let ledger = self
                    .ledger_path(&project, *phase, *tests)
                    .display()
                    .to_string();
                if let Some(l) = self.loop_mut((*phase, *tests))
                    && let LoopNext::Autofix { apply, remaining } = l.next.clone()
                {
                    let mut rem = remaining;
                    for (rule_line, _) in failed {
                        if let Some(f) = apply.iter().find(|f| &f.line() == rule_line)
                            && matches!(f.severity, Severity::High | Severity::Medium)
                        {
                            rem.push(f.clone());
                        }
                    }
                    l.after_findings(rem, yolo, &ledger);
                }
            }
            SessionEvent::SecurityBlock { .. } => {}
            SessionEvent::PhaseBlocked {
                phase,
                tests,
                reason,
                ..
            } => {
                if let Some(l) = self.loop_mut((*phase, *tests)) {
                    l.announced_block = true;
                    if !l.is_blocked() {
                        l.next = LoopNext::Blocked {
                            reason: reason.clone(),
                        };
                    }
                }
            }

            SessionEvent::CommandStarted {
                purpose,
                project,
                command,
            } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .running = Some((*purpose, command.clone()));
            }
            SessionEvent::DocsPlanned { .. } => {}
            SessionEvent::DocsScanned {
                project,
                modules,
                refs,
            } => {
                self.ext
                    .os_mut()
                    .project_tracks
                    .entry(project.clone())
                    .or_default()
                    .docs_scan = Some(DocsScan {
                    modules: modules.clone(),
                    refs: refs.clone(),
                });
            }
            SessionEvent::ProjectInitFinished { project } => {
                if let Some(i) = self.ext.os_mut().project_inits.get_mut(project) {
                    i.finished = true;
                    i.note = None;
                }
            }
            SessionEvent::InitStepFailed {
                project,
                execution,
                error,
            } => {
                if let Some(i) = self.ext.os_mut().project_inits.get_mut(project) {
                    clear_init_result(i, execution);
                    i.failed = Some((execution.clone(), error.clone()));
                }
            }
            _ => {}
        }
    }
}
/// The review loop's next step after the findings of a pass.
pub trait WorkLoopFold {
    /// Decide what follows a review whose HIGH and MEDIUM findings are `remaining`.
    fn after_findings(&mut self, remaining: Vec<ReviewFinding>, yolo: bool, fix_ledger: &str);
}

impl WorkLoopFold for WorkLoop {
    fn after_findings(&mut self, remaining: Vec<ReviewFinding>, yolo: bool, fix_ledger: &str) {
        if remaining.is_empty() {
            self.next = if self.stage {
                LoopNext::Stage
            } else {
                LoopNext::Done
            };
            return;
        }
        if self.iterations >= self.effective_cap(yolo) {
            if yolo {
                // YOLO resolution repeats while it converges (review-cap.js).
                let converging = match self.open_before_resolve {
                    None => true,
                    Some(before) => remaining.len() < before,
                };
                self.next = if self.resolve_rounds == 0 || converging {
                    LoopNext::Resolve {
                        findings: remaining,
                    }
                } else {
                    LoopNext::Blocked {
                        reason: format!(
                            "The review loop stopped converging after {} resolution rounds; {} findings remain open. Ledger: {fix_ledger}",
                            self.resolve_rounds,
                            remaining.len()
                        ),
                    }
                };
            } else {
                self.next = LoopNext::CapReached {
                    findings: remaining,
                };
            }
            return;
        }
        self.next = LoopNext::Work {
            kind: WorkKind::Fix,
            instructions: Some(fix_instructions(&remaining, fix_ledger)),
        };
    }
}

/// Whether an answer fits a gate of the built-in stages. `None` for a gate they do not own.
pub fn validate_answer(
    s: &SessionState,
    payload: &GatePayload,
    answer: &GateAnswer,
) -> Option<Result<(), String>> {
    let ours = matches!(
        payload,
        GatePayload::OpenQuestions { .. }
            | GatePayload::SpecApproval { .. }
            | GatePayload::PlanApproval { .. }
            | GatePayload::FactCheckRecurring { .. }
            | GatePayload::ReviewCap { .. }
            | GatePayload::Stuck { .. }
            | GatePayload::PhaseBlocked { .. }
            | GatePayload::DocsRounds { .. }
            | GatePayload::ImplementationReview { .. }
            | GatePayload::ClosingGate { .. }
            | GatePayload::SkillApproval { .. }
    );
    ours.then(|| check_answer(s, payload, answer))
}

pub fn check_answer(
    s: &SessionState,
    payload: &GatePayload,
    answer: &GateAnswer,
) -> Result<(), String> {
    let ok = matches!(
        (payload, answer),
        (
            GatePayload::OpenQuestions { .. },
            GateAnswer::Questions { .. }
        ) | (
            GatePayload::SpecApproval { .. } | GatePayload::PlanApproval { .. },
            GateAnswer::Approval { .. }
        ) | (
            GatePayload::FactCheckRecurring { .. }
                | GatePayload::ReviewCap { .. }
                | GatePayload::Stuck { .. }
                | GatePayload::PhaseBlocked { .. }
                | GatePayload::DocsRounds { .. }
                | GatePayload::ImplementationReview { .. },
            GateAnswer::Choice { .. }
        ) | (GatePayload::ClosingGate { .. }, GateAnswer::Closing { .. })
            | (GatePayload::SkillApproval { .. }, GateAnswer::Skills { .. })
    );
    if !ok {
        return Err("That answer does not fit this gate.".into());
    }
    if let (GatePayload::OpenQuestions { questions, .. }, GateAnswer::Questions { answers }) =
        (payload, answer)
    {
        for q in questions {
            if !answers
                .iter()
                .any(|a| a.id == q.id && !a.answer.trim().is_empty())
            {
                return Err(format!("Answer {} before continuing.", q.id));
            }
        }
    }
    if let (
        GatePayload::SpecApproval { .. } | GatePayload::PlanApproval { .. },
        GateAnswer::Approval {
            approved: false,
            feedback,
        },
    ) = (payload, answer)
        && feedback.as_ref().is_none_or(|f| f.trim().is_empty())
    {
        return Err("Say what to change.".into());
    }
    if let (GatePayload::DocsRounds { .. }, GateAnswer::Choice { option, .. }) = (payload, answer)
        && option != "continue"
        && option != "accept"
    {
        return Err(
            "Choose continue for another round, or accept to write the book as it is.".into(),
        );
    }
    if let (GatePayload::ImplementationReview { .. }, GateAnswer::Choice { option, text }) =
        (payload, answer)
    {
        match option.as_str() {
            "done" => {}
            "feedback" if text.as_ref().is_some_and(|t| !t.trim().is_empty()) => {}
            "feedback" => return Err("Say what to change.".into()),
            _ => return Err("Answer done, or feedback with what to change.".into()),
        }
    }
    if let GatePayload::SpecApproval { .. } | GatePayload::PlanApproval { .. } = payload {
        let passed = if matches!(payload, GatePayload::SpecApproval { .. }) {
            s.ext.os().spec.passed_current()
        } else {
            s.ext.os().plan.passed_current()
        };
        if matches!(answer, GateAnswer::Approval { approved: true, .. }) && !passed {
            return Err("Approval requires a fact-check PASS on this version.".into());
        }
    }
    Ok(())
}
