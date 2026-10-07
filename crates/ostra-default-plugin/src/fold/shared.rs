//! Helpers every part of the built-in stages' fold uses, and the `OstraFold` facts.

use super::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::agent::AgentName;
use ostra_core::book::DocsStep;
use ostra_core::event::{ExecPurpose, GateAnswer, GatePayload, JudgeKind, SessionKind};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::{DecisionId, ExecutionId, GateId};
use ostra_core::model::Complexity;
use ostra_core::paths;
use ostra_core::pipeline::{Category, PhaseInfo, TestPolicy};
use ostra_core::submit::{FactCheckSubmit, ReviewFinding, Severity, StuckInfo, Verdict};
use ostra_engine::state::*;
use std::collections::BTreeSet;
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

    /// Rule U1: a running execution whose task the session can do without. Research the user or
    /// a judge started, the test analysis, a docs writer, and the architecture overview end
    /// without a result; work, review, spec, plan, and fact-check carry rules a skip would break,
    /// a helper's asker waits for its answer, and a rescue's loop waits for its fact.
    fn can_skip(&self, exec: &ExecutionId) -> bool;

    /// Rule U1: end the execution's task without a result, as abandoning its failure gate does.
    fn skip_task(&mut self, exec: &ExecutionId);

    /// Rule D4a: where the runner writes the code facts before a spawn that gets them.
    fn code_facts_path(&self) -> PathBuf;

    /// Whether overriding a decision can still change what happens (HANDOVER 8.3 override button).
    fn can_override(&self, id: &DecisionId) -> bool;

    /// The executor an agent must run on regardless of routing: native after a harness failure,
    /// native for a quick change, which skips the harness startup cost, and native for a quick
    /// answer, which runs the side panel's agent (HANDOVER 12.3).
    fn forced_executor(&self, agent: AgentName) -> Option<ostra_core::ExecutorKind>;
}

impl OstraFold for SessionState {
    fn tests_requested(&self) -> bool {
        self.options.tests || self.ext.os().opts_in.tests
    }

    fn docs_requested(&self) -> bool {
        self.options.docs || self.ext.os().opts_in.docs
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

    fn code_facts_path(&self) -> PathBuf {
        self.session_root.join(paths::report::code_facts())
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

    fn forced_executor(&self, agent: AgentName) -> Option<ostra_core::ExecutorKind> {
        (self.native_fallback.contains(&agent)
            || self.category == Some(Category::QuickChange)
            || self.category == Some(Category::QuickAnswer))
        .then_some(ostra_core::ExecutorKind::Native)
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
