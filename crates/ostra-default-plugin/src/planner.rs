//! The built-in stages' part of the planner: research, the Track and Stakes judges, the spec and
//! plan with their fact-checks and approvals, the build's phase loops, the user's review, the
//! closing stages, the init flow, and completion. The engine's planner calls it through
//! `Pipeline`; it stays a pure function of the session state.

use crate::book::{DOCS_ROUNDS, DocsTrack};
use crate::inputs::{OstraInputs, OstraInputsExt};
use crate::judge::NoteStage;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::steps::OstraStep;
use ostra_core::Contract;
use ostra_core::agent::AgentName;
use ostra_core::book::DocsStep;
use ostra_core::event::{
    ClosingItem, CommandPurpose, ExecPurpose, FactTarget, GatePayload, JudgeKind, SessionKind,
    WorkKind,
};
use ostra_core::exec::ExecutionStatus;
use ostra_core::ids::ExecutionId;
use ostra_core::paths::report;
use ostra_core::pipeline::{Category, PhaseInfo, Stakes, TestPolicy, Track};
use ostra_core::submit::{Severity, Verdict};
use ostra_core::workflow::{BuiltinStage, StageScope, WorkflowDef};
use ostra_engine::plan::*;
use ostra_engine::state::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

pub struct BookProgress {
    /// Projects whose part of the book is written, sorted.
    parts: Vec<String>,
}

/// Rule B10: the session's workflow has a book stage, so the closing stage writes no docs. A
/// session that recorded no workflow keeps the docs in its closing stage, as it started.
pub fn book_node(s: &SessionState) -> bool {
    s.workflow
        .as_ref()
        .is_some_and(|w| w.has_builtin(BuiltinStage::Book))
}

/// The phases of `project` whose build passed.
fn passed_phases<'a>(s: &'a SessionState, project: &str) -> Vec<&'a PhaseRun> {
    s.ext
        .os()
        .phases
        .values()
        .filter(|p| p.info.project == project && p.impl_loop.is_done())
        .collect()
}

/// Rule B10: the closing gate chose docs for `project`, or the request asked for tests and docs,
/// and no BLOCKER is open (Hard rule 21). `None` while the choice is not made.
pub fn docs_chosen(s: &SessionState, project: &str) -> Option<bool> {
    let track = s.ext.os().project_tracks.get(project)?;
    let passed = passed_phases(s, project);
    if passed.is_empty() {
        return Some(false);
    }
    let docs_on = match track.closing {
        Some(c) => c.1,
        None if s.tests_requested() && s.docs_requested() => true,
        None => return None,
    };
    Some(docs_on && !blocker_open(&passed))
}

/// Rule WB4: the facts the closing and book stages settled. Seen from a project's instance, the
/// closing stage reads whether docs run for that project; from anywhere else, the projects they
/// run for.
pub fn stage_value(
    s: &SessionState,
    stage: BuiltinStage,
    scope: Option<&str>,
) -> serde_json::Value {
    let os = s.ext.os();
    match stage {
        BuiltinStage::Research => serde_json::json!({ "research_docs": s.research_docs() }),
        BuiltinStage::Track => serde_json::json!({ "track": os.track }),
        BuiltinStage::Spec => serde_json::json!({
            "spec_file": os.spec.current.as_ref().map(|c| c.spec_path.clone())
        }),
        BuiltinStage::Stakes => serde_json::json!({ "stakes": os.stakes.as_ref().map(|(_, s)| s) }),
        BuiltinStage::Plan => serde_json::json!({
            "master_plan": os.plan.current.as_ref().map(|c| c.master_plan_path.clone()),
            "phases": os.phases.len(),
        }),
        BuiltinStage::Build => serde_json::json!({ "phases": os.phases.len() }),
        BuiltinStage::Closing => match scope.and_then(|sc| sc.strip_prefix("project:")) {
            Some(p) => serde_json::json!({ "docs": docs_chosen(s, p).unwrap_or(false) }),
            None => serde_json::json!({
                "docs": s
                    .ext.os().project_tracks
                    .keys()
                    .filter(|k| docs_chosen(s, k) == Some(true))
                    .collect::<Vec<_>>()
            }),
        },
        BuiltinStage::Book => {
            serde_json::json!({ "book": s.book_written.as_ref().map(|w| w.book.clone()) })
        }
        BuiltinStage::Feedback => serde_json::json!({}),
    }
}

pub fn blocker_open(passed: &[&PhaseRun]) -> bool {
    passed
        .iter()
        .any(|p| p.impl_loop.blocker_open || p.test_loop.blocker_open)
}

pub fn revision_task(r: &Revision, request: &str) -> String {
    format!(
        "Feedback round {} from the user, after reviewing the implementation:\n{}\n\nThe original request, for context:\n{request}",
        r.round, r.instruction
    )
}

/// Rule O3: the completion report names every project an agent created and how its init ended.
pub fn created_projects_section(s: &SessionState) -> String {
    if s.created_projects.is_empty() {
        return String::new();
    }
    let rows: Vec<String> = s
        .created_projects
        .iter()
        .map(|p| {
            let init = match s.ext.os().project_inits.get(&p.key).and_then(|i| i.note.as_deref()) {
                Some(note) => format!("not initialized: {note} Initialize it from the project list before its next session."),
                None => "initialized".into(),
            };
            format!("- `{}` at `{}` ({}), {init}", p.key, p.path.display(), p.stack)
        })
        .collect();
    format!("\n\n## Projects created\n\n{}\n", rows.join("\n"))
}

pub struct ExploreRef(u32);

impl ExploreRef {
    fn purpose(&self) -> ExecPurpose {
        ExecPurpose::Explore { task: self.0 }
    }
}

pub fn last_exec_of(s: &SessionState, f: impl Fn(&ExecPurpose) -> bool) -> Option<ExecutionId> {
    s.executions
        .values()
        .filter(|r| f(&r.purpose))
        .max_by_key(|r| r.id.clone())
        .map(|r| r.id.clone())
}

/// Phase Index rows as the approval card shows them.
/// Rule D4b: the plan's phases that a failed fact-check's findings name, from each finding's
/// element (`phase 2`, `step 2.3`) or location (`...-phase-2.md`, `Phase 2, Step 2.3`).
pub fn phases_named(
    findings: &[ostra_core::submit::FactCheckFinding],
    plan: &ostra_core::submit::PlanSubmit,
) -> Vec<u32> {
    let mut named = BTreeSet::new();
    for f in findings {
        for text in f.element.iter().chain([&f.location]) {
            let lower = text.to_ascii_lowercase();
            for marker in ["phase-", "phase ", "step "] {
                for (i, _) in lower.match_indices(marker) {
                    let digits: String = lower[i + marker.len()..]
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect();
                    if let Ok(n) = digits.parse::<u32>() {
                        named.insert(n);
                    }
                }
            }
        }
    }
    plan.phases
        .iter()
        .map(|p| p.id)
        .filter(|id| named.contains(id))
        .collect()
}

pub fn plan_phase_infos(plan: &ostra_core::submit::PlanSubmit) -> Vec<PhaseInfo> {
    plan.phases
        .iter()
        .map(|p| PhaseInfo {
            id: p.id,
            deliverable: Some(p.deliverable.clone()),
            project: p.project.clone(),
            title: p.title.clone(),
            complexity: p.complexity.parse().unwrap_or_default(),
            test_policy: if p.test_policy.eq_ignore_ascii_case("skip") {
                TestPolicy::Skip
            } else {
                TestPolicy::Required
            },
            depends_on: Some(p.depends_on.clone()),
            file: Some(PathBuf::from(&p.file)),
            test_rationale: p.test_rationale.clone(),
        })
        .collect()
}

/// Rule D6 and M3: a phase is ready when every phase it depends on completed and passed review.
/// An unreadable dependency means it depends on every earlier phase (Rule M5).
/// The built-in stage a work loop belongs to: the build, or the closing stage for tests.
pub fn loop_stage(tests: bool) -> BuiltinStage {
    if tests {
        BuiltinStage::Closing
    } else {
        BuiltinStage::Build
    }
}

pub fn deps_passed(s: &SessionState, p: &PhaseRun) -> bool {
    let deps: Vec<u32> = match &p.info.depends_on {
        Some(d) => d.clone(),
        None => s
            .ext
            .os()
            .phases
            .keys()
            .copied()
            .filter(|id| *id < p.info.id)
            .collect(),
    };
    // Rule WF4: the phase stages of a dependency are part of it passing.
    deps.iter().all(|d| {
        s.ext
            .os()
            .phases
            .get(d)
            .is_some_and(|dp| dp.impl_loop.is_done())
            && s.phase_stages_done(*d)
    })
}

/// Rule D9: every phase that depends, directly or transitively, on a blocked phase is removed
/// from the queue. Independent phases continue.
pub fn removed_phases(s: &SessionState) -> BTreeSet<u32> {
    let mut failed: BTreeSet<u32> = s
        .ext
        .os()
        .phases
        .values()
        .filter(|p| p.impl_loop.is_blocked())
        .map(|p| p.info.id)
        .collect();
    let mut removed = BTreeSet::new();
    loop {
        let mut changed = false;
        for p in s.ext.os().phases.values() {
            if failed.contains(&p.info.id) || removed.contains(&p.info.id) || !p.impl_loop.is_idle()
            {
                continue;
            }
            let deps: Vec<u32> = match &p.info.depends_on {
                Some(d) => d.clone(),
                None => s
                    .ext
                    .os()
                    .phases
                    .keys()
                    .copied()
                    .filter(|id| *id < p.info.id)
                    .collect(),
            };
            if deps
                .iter()
                .any(|d| failed.contains(d) || removed.contains(d))
            {
                removed.insert(p.info.id);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    failed.clear();
    removed
}

/// Execution status helper for views.
pub fn is_live(status: Option<ExecutionStatus>) -> bool {
    status.is_none()
}

#[allow(dead_code)]
pub fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Blocker => 0,
        Severity::High => 1,
        Severity::Medium => 2,
        Severity::Low => 3,
    }
}

/// The built-in stages' planning rules over the engine's planner.
pub trait OstraPlanner<'a> {
    /// Rule WF2: a built-in stage runs Ostra's own rules for it, exactly as the fixed pipeline did.
    fn builtin_stage(&mut self, b: BuiltinStage, wf: &WorkflowDef) -> bool;

    /// Every phase ended and every phase stage of a passed phase is done (Rule WF4).
    fn build_done(&self, wf: &WorkflowDef) -> bool;

    fn nothing_running(&self) -> bool;

    fn explore_tasks(&mut self);

    /// Rule D2: no explore running and no needed `Not covered` item left unjudged.
    fn explore_complete(&mut self) -> bool;

    /// Returns true once the spec is done for this category: fact-check PASS, and approval when
    /// `needs_approval`.
    fn spec_flow(&mut self, needs_approval: bool) -> bool;

    fn scope_paths(&self) -> Vec<(String, PathBuf)>;

    fn plan_flow(&mut self) -> bool;

    fn phases(&mut self);

    fn loop_work(
        &mut self,
        p: &PhaseRun,
        tests: bool,
        kind: WorkKind,
        instructions: Option<String>,
    );

    fn loop_steps(&mut self, p: &PhaseRun, tests: bool);

    /// Returns true once the user accepted the implementation.
    fn implementation_review(&mut self) -> bool;

    fn project_phases(&self, project: &str) -> Vec<&'a PhaseRun>;

    fn project_code_done(&self, project: &str, removed: &BTreeSet<u32>) -> bool;

    fn closing_stages(&mut self);

    /// Returns true when the project's test stage is finished.
    fn test_stage(&mut self, project: &str, passed: &[&'a PhaseRun]) -> bool;

    fn docs_stage(&mut self, project: &str, passed: &[&PhaseRun]);

    /// One run of the docs stage: start it under the cap, or open the failure gate.
    fn docs_run<T>(
        &mut self,
        project: &str,
        state: &crate::data::StageRun<T>,
        agent: AgentName,
        purpose: ExecPurpose,
        inputs: SpawnInputs,
    );

    /// Where the book stands once every project's closing stages are known. `None` while a
    /// project may still get documentation.
    fn book_progress(&self) -> Option<BookProgress>;

    /// Rule B5: after every part is written, the book write.
    fn book_stage(&mut self);

    /// Rule B10: the book stage: the docs stage of each project the closing gate chose docs
    /// for, then the book write. Returns `true` when it is done.
    fn book_flow(&mut self) -> bool;

    fn all_implement_done(&self) -> bool;

    fn completion(&mut self);

    fn quick_answer(&mut self);

    fn init_flow(&mut self);
}

impl<'a> OstraPlanner<'a> for Planner<'a> {
    fn builtin_stage(&mut self, b: BuiltinStage, wf: &WorkflowDef) -> bool {
        let s = self.s;
        let implement = wf.base == Category::Implement;
        let light = implement && s.ext.os().track == Some(Track::Light);
        match b {
            BuiltinStage::Research => self.explore_complete(),
            // Light by default: the Track judge escalates to the full track on evidence.
            BuiltinStage::Track => {
                if s.ext.os().track.is_none() {
                    self.push(Step::Judge {
                        judge: JudgeKind::Track,
                        subject: None,
                    });
                }
                s.ext.os().track.is_some()
            }
            BuiltinStage::Spec => light || self.spec_flow(wf.base != Category::Spec),
            BuiltinStage::Stakes => {
                if light {
                    return true;
                }
                if s.ext.os().stakes.is_none() {
                    self.push(Step::Judge {
                        judge: JudgeKind::Stakes,
                        subject: None,
                    });
                }
                s.ext.os().stakes.is_some()
            }
            BuiltinStage::Plan if implement => {
                if light {
                    return true;
                }
                let low = matches!(s.ext.os().stakes, Some((_, Stakes::Low)));
                (low || self.plan_flow())
                    && !s.ext.os().plan.invalidated
                    && !s.ext.os().spec.needs_run
            }
            BuiltinStage::Plan => self.plan_flow(),
            BuiltinStage::Build => {
                if implement {
                    // Rule O4: a project a phase created is initialized before its phases go on.
                    crate::init::plan_created(self.s, &mut |step| self.out.push(step));
                }
                self.phases();
                self.phase_stages(wf);
                self.build_done(wf)
            }
            BuiltinStage::Feedback => self.implementation_review(),
            BuiltinStage::Closing => {
                self.closing_stages();
                self.all_implement_done()
            }
            BuiltinStage::Book => self.book_flow(),
        }
    }

    fn build_done(&self, wf: &WorkflowDef) -> bool {
        let s = self.s;
        let removed = removed_phases(s);
        let phases_done = s
            .ext
            .os()
            .phases
            .values()
            .all(|p| removed.contains(&p.info.id) || p.impl_loop.is_terminal());
        let staged = wf
            .stages
            .iter()
            .any(|d| d.scope == StageScope::Phase && d.builtin().is_none());
        phases_done
            && (!staged
                || s.ext
                    .os()
                    .phases
                    .values()
                    .filter(|p| !removed.contains(&p.info.id) && p.impl_loop.is_done())
                    .all(|p| s.phase_stages_done(p.info.id)))
    }

    fn nothing_running(&self) -> bool {
        self.s.running_executions().next().is_none()
            && self.s.open_gates().all(|g| matches!(g.payload, GatePayload::Permission { .. }))
            // A blocked phase is announced before the completion report is written.
            && self.s.ext.os().phases.values().all(|p| {
                [&p.impl_loop, &p.test_loop].iter().all(|l| !l.is_blocked() || l.announced_block)
            })
    }

    fn explore_tasks(&mut self) {
        let s = self.s;
        for t in &s.ext.os().explore {
            if t.exec.is_none() && !t.abandoned && t.failed.is_none() {
                // Rule M1: read-only stages fan out; every ready explore spawns at once.
                let inputs = SpawnInputs {
                    task: Some(format!("{}{}", t.task, s.added_context())),
                    ..Default::default()
                };
                let agent = t
                    .agent
                    .unwrap_or_else(|| s.agent_for(BuiltinStage::Research, Contract::Research));
                self.spawn(
                    agent,
                    ExploreRef(t.idx).purpose(),
                    &t.project,
                    s.project_session_dir(&t.project),
                    inputs,
                );
            } else if let (Some(err), None, Some(exec), false, false) = (
                &t.failed,
                &t.gate,
                &t.exec,
                t.abandoned,
                // Rule H3: a helper's failure is its asker's answer, not a gate.
                matches!(t.origin, ExploreOrigin::Ask { .. }),
            ) {
                let agent = s
                    .executions
                    .get(exec)
                    .map(|r| r.agent)
                    .unwrap_or_else(|| s.agent_for(BuiltinStage::Research, Contract::Research));
                self.exec_failed_gate(exec, agent, &t.project, err);
            }
        }
    }

    fn explore_complete(&mut self) -> bool {
        let s = self.s;
        let tasks: Vec<&ExploreTask> = s
            .ext
            .os()
            .explore
            .iter()
            .filter(|t| !t.origin.loop_bound())
            .collect();
        if tasks.iter().any(|t| !t.finished()) {
            return false;
        }
        let unjudged: Vec<&&ExploreTask> = tasks
            .iter()
            .filter(|t| !t.judged && t.result.as_ref().is_some_and(|r| !r.not_covered.is_empty()))
            .collect();
        if !unjudged.is_empty() && s.ext.os().sufficiency_rounds < SUFFICIENCY_ROUNDS {
            let subject = unjudged
                .iter()
                .map(|t| t.idx.to_string())
                .collect::<Vec<_>>()
                .join(",");
            self.push(Step::Judge {
                judge: JudgeKind::Sufficiency,
                subject: Some(subject),
            });
            return false;
        }
        if s.research_docs().is_empty() {
            // Rule D1: with no research document, the spec is not entered.
            if matches!(
                s.category,
                Some(Category::Research | Category::Spec | Category::Plan | Category::Implement)
            ) {
                self.push(Step::Fail {
                    error: "Every research task failed or was abandoned, so there is no research document to write a spec from.".into(),
                });
            }
            return false;
        }
        true
    }

    fn spec_flow(&mut self, needs_approval: bool) -> bool {
        let s = self.s;
        let t = &s.ext.os().spec;
        let primary = s.primary();
        if t.stopped {
            self.push(Step::Fail {
                error: "The spec stage was stopped.".into(),
            });
            return false;
        }
        if t.busy()
            || t.questions_gate.is_some()
            || t.approval_gate.is_some()
            || t.recurring_gate.is_some()
            || t.routing.is_some()
        {
            return false;
        }
        if let Some(err) = &t.failed {
            if t.failed_gate.is_none()
                && let Some(exec) = t.check_running.clone().or_else(|| t.runs.last().cloned())
            {
                let exec = last_exec_of(s, |p| {
                    matches!(
                        p,
                        ExecPurpose::Spec { .. }
                            | ExecPurpose::FactCheck {
                                target: FactTarget::Spec,
                                ..
                            }
                    )
                })
                .unwrap_or(exec);
                self.exec_failed_gate(
                    &exec,
                    s.agent_for(BuiltinStage::Spec, Contract::Spec),
                    &primary,
                    err,
                );
            }
            return false;
        }
        if t.current.is_none() || t.needs_run {
            if t.pending_findings.is_some()
                && t.consecutive_fails >= FACTCHECK_RECURRING_LIMIT + t.fail_limit_extra
            {
                let findings = t
                    .check_for_current()
                    .map(|c| c.findings.clone())
                    .unwrap_or_default();
                self.gate(
                    "The spec fact-check keeps failing",
                    format!(
                        "The fact-check has failed {} times in a row. Another round may not converge. Choose whether to run another round, add guidance, or stop.",
                        t.consecutive_fails
                    ),
                    GatePayload::FactCheckRecurring { target: FactTarget::Spec, passes: t.consecutive_fails, findings },
                );
                return false;
            }
            let inputs = SpawnInputs {
                task: Some(s.full_request()),
                research_docs: s.research_docs(),
                // Rewriting in place keeps the file name, so a fact-check re-pass compares
                // against its snapshot by name (Rule D3a).
                spec_file: t.current.as_ref().map(|c| PathBuf::from(&c.spec_path)),
                extra: OstraInputs {
                    // Rule D2a: the spec learns which cited files changed since research.
                    code_facts: s.research_docs(),
                    projects_in_scope: self.scope_paths(),
                    // A revision gets only the input its spec does not reflect yet, so it edits
                    // instead of rewriting.
                    answers: t.pending_answers(),
                    changes: t.pending_changes(),
                    new_research_docs: match t.current {
                        Some(_) => s
                            .research_docs()
                            .into_iter()
                            .filter(|d| !t.applied.docs.contains(d))
                            .collect(),
                        None => vec![],
                    },
                    findings: t.pending_findings.clone(),
                    ..Default::default()
                }
                .into_value(),
                ..Default::default()
            };
            let round = t.runs.len() as u32 + 1;
            self.spawn(
                s.agent_for(BuiltinStage::Spec, Contract::Spec),
                ExecPurpose::Spec { round },
                &primary,
                s.session_root.clone(),
                inputs,
            );
            return false;
        }
        let Some(spec) = &t.current else { return false };
        let spec_path = PathBuf::from(&spec.spec_path);
        // Rule D3: open questions are asked before any fact-check.
        if !spec.open_questions.is_empty() && t.questions_asked_version != t.version {
            self.gate(
                "Questions about the requirements",
                "The spec could not settle these from the research. Each answer is written back into the spec, then the spec is fact-checked.",
                GatePayload::OpenQuestions {
                    artifact: "spec".into(),
                    artifact_path: spec_path,
                    questions: spec.open_questions.clone(),
                },
            );
            return false;
        }
        match t.check_for_current() {
            None => {
                let first = !t.has_pass();
                // Rule D3b: refetch only on a spec's first pass with External Evidence rows.
                let source_check = if first && spec.external_evidence_rows > 0 {
                    "refetch"
                } else {
                    "citations"
                };
                let pass = t.checks.len() as u32 + 1;
                let inputs = SpawnInputs {
                    target: Some(spec_path.clone()),
                    prior_findings: Some(t.prior_findings()),
                    spec_file: Some(spec_path),
                    research_docs: s.research_docs(),
                    extra: OstraInputs {
                        target_type: Some(FactTarget::Spec),
                        source_check: Some(source_check.into()),
                        code_facts: s.research_docs(),
                        ..Default::default()
                    }
                    .into_value(),
                    ..Default::default()
                };
                self.spawn(
                    s.agent_for(BuiltinStage::Spec, Contract::FactCheck),
                    ExecPurpose::FactCheck {
                        target: FactTarget::Spec,
                        pass,
                    },
                    &primary,
                    s.session_root.clone(),
                    inputs,
                );
                false
            }
            Some(check) if check.verdict != Verdict::Pass => false,
            Some(check) => {
                if !needs_approval {
                    return true;
                }
                if t.approved && t.approved_version == t.version {
                    return true;
                }
                if t.approval_asked_version != t.version || t.approval_gate.is_none() {
                    self.gate(
                        "Approve the spec",
                        "The spec passed its fact-check. Approve it to continue, or describe what to change. Every change is written back into the spec and checked again.",
                        GatePayload::SpecApproval {
                            spec_path,
                            summary: spec.summary.clone(),
                            findings: check.findings.clone(),
                        },
                    );
                }
                false
            }
        }
    }

    fn scope_paths(&self) -> Vec<(String, PathBuf)> {
        self.s
            .scope
            .iter()
            .filter_map(|k| self.s.project_path(k).map(|p| (k.clone(), p)))
            .collect()
    }

    fn plan_flow(&mut self) -> bool {
        let s = self.s;
        let t = &s.ext.os().plan;
        let primary = s.primary();
        if t.stopped {
            self.push(Step::Fail {
                error: "The plan stage was stopped.".into(),
            });
            return false;
        }
        if t.busy()
            || t.questions_gate.is_some()
            || t.approval_gate.is_some()
            || t.recurring_gate.is_some()
            || t.routing.is_some()
        {
            return false;
        }
        let Some(spec) = &s.ext.os().spec.current else {
            return false;
        };
        let spec_path = PathBuf::from(&spec.spec_path);
        if let Some(err) = &t.failed {
            if t.failed_gate.is_none()
                && let Some(exec) = last_exec_of(s, |p| {
                    matches!(
                        p,
                        ExecPurpose::Plan { .. }
                            | ExecPurpose::FactCheck {
                                target: FactTarget::Plan,
                                ..
                            }
                    )
                })
            {
                self.exec_failed_gate(
                    &exec,
                    s.agent_for(BuiltinStage::Plan, Contract::Plan),
                    &primary,
                    err,
                );
            }
            return false;
        }
        if t.current.is_none() || t.needs_run || t.invalidated {
            if t.pending_findings.is_some()
                && !t.invalidated
                && t.consecutive_fails >= FACTCHECK_RECURRING_LIMIT + t.fail_limit_extra
            {
                let findings = t
                    .check_for_current()
                    .map(|c| c.findings.clone())
                    .unwrap_or_default();
                self.gate(
                    "The plan fact-check keeps failing",
                    format!("The fact-check has failed {} times in a row. Choose whether to run another round, add guidance, or stop.", t.consecutive_fails),
                    GatePayload::FactCheckRecurring { target: FactTarget::Plan, passes: t.consecutive_fails, findings },
                );
                return false;
            }
            // Rule D4: the plan reads the spec and its own earlier plan, never a research
            // document. It gets the code facts the engine extracts from them (Rule D4a), and on a
            // FAIL re-run the findings (Rule D5) and the phases they name (Rule D4b). After a spec
            // change (Rule D10) the earlier plan is revised in place against the spec's diff.
            let findings = if t.invalidated {
                None
            } else {
                t.pending_findings.clone()
            };
            let revise_phases = match (&findings, t.check_for_current(), &t.current) {
                (Some(_), Some(check), Some(plan)) => phases_named(&check.findings, plan),
                _ => vec![],
            };
            let inputs = SpawnInputs {
                spec_file: Some(spec_path),
                target: t
                    .current
                    .as_ref()
                    .map(|c| PathBuf::from(&c.master_plan_path)),
                extra: OstraInputs {
                    projects_in_scope: self.scope_paths(),
                    code_facts: s.research_docs(),
                    findings,
                    revise_phases,
                    ..Default::default()
                }
                .into_value(),
                ..Default::default()
            };
            let round = t.runs.len() as u32 + 1;
            self.spawn(
                s.agent_for(BuiltinStage::Plan, Contract::Plan),
                ExecPurpose::Plan { round },
                &primary,
                s.session_root.clone(),
                inputs,
            );
            return false;
        }
        let Some(plan) = &t.current else { return false };
        let plan_path = PathBuf::from(&plan.master_plan_path);
        if !plan.clarifying_questions.is_empty() && t.questions_asked_version != t.version {
            self.gate(
                "Questions from the plan",
                "The plan raised these questions. Answers change requirements, so they go into the spec first and the plan is written again (Rule D10).",
                GatePayload::OpenQuestions { artifact: "plan".into(), artifact_path: plan_path, questions: plan.clarifying_questions.clone() },
            );
            return false;
        }
        match t.check_for_current() {
            None => {
                let pass = t.checks.len() as u32 + 1;
                // Rules D3b and D5: a plan target always uses citations and gets the approved spec,
                // and the code facts the plan had (Rule D4a).
                let inputs = SpawnInputs {
                    target: Some(plan_path),
                    prior_findings: Some(t.prior_findings()),
                    spec_file: Some(spec_path),
                    extra: OstraInputs {
                        target_type: Some(FactTarget::Plan),
                        source_check: Some("citations".into()),
                        code_facts: s.research_docs(),
                        ..Default::default()
                    }
                    .into_value(),
                    ..Default::default()
                };
                self.spawn(
                    s.agent_for(BuiltinStage::Plan, Contract::FactCheck),
                    ExecPurpose::FactCheck {
                        target: FactTarget::Plan,
                        pass,
                    },
                    &primary,
                    s.session_root.clone(),
                    inputs,
                );
                false
            }
            Some(check) if check.verdict != Verdict::Pass => false,
            Some(check) => {
                if t.approved && t.approved_version == t.version {
                    return true;
                }
                if t.approval_asked_version != t.version || t.approval_gate.is_none() {
                    let phases = plan_phase_infos(plan);
                    self.gate(
                        "Approve the plan",
                        "The plan passed its fact-check. Approve it to start building, or describe what to change. A change goes into the spec first, then a new plan is written (Rule D10).",
                        GatePayload::PlanApproval { plan_path, summary: plan.summary.clone(), phases, findings: check.findings.clone() },
                    );
                }
                false
            }
        }
    }

    fn phases(&mut self) {
        let s = self.s;
        let removed = removed_phases(s);
        let mut busy: BTreeSet<String> = BTreeSet::new();
        for p in s.ext.os().phases.values() {
            let l = &p.impl_loop;
            if !l.is_idle() && !l.is_terminal() {
                busy.insert(p.info.project.clone());
            }
        }
        for p in s.ext.os().phases.values() {
            if removed.contains(&p.info.id) {
                continue;
            }
            let l = &p.impl_loop;
            if l.is_idle() {
                // Rule M2: one implement pipeline per project at a time.
                if busy.contains(&p.info.project) || !deps_passed(s, p) {
                    continue;
                }
                busy.insert(p.info.project.clone());
                self.loop_work(p, false, WorkKind::Initial, None);
                continue;
            }
            self.loop_steps(p, false);
        }
    }

    fn loop_work(
        &mut self,
        p: &PhaseRun,
        tests: bool,
        kind: WorkKind,
        instructions: Option<String>,
    ) {
        let s = self.s;
        let l = if tests { &p.test_loop } else { &p.impl_loop };
        let project = p.info.project.clone();
        let dir = s.project_session_dir(&project);
        let phase = p.info.id;
        let contract = if matches!(kind, WorkKind::Initial)
            || (matches!(kind, WorkKind::Rerun | WorkKind::Resume) && l.work_count <= 1)
        {
            l.work
        } else {
            l.fix
        };
        // Rule WF8: the loop's stage binds the agent that fills the contract.
        let agent = s.agent_for(loop_stage(tests), contract);
        let phase_str = phase.to_string();
        let mut inputs = SpawnInputs {
            phase: Some(p.info.clone()),
            instructions: instructions.clone(),
            user_notes: match contract {
                Contract::Implementation => s.notes_for(NoteStage::Implement),
                Contract::Tests => s.notes_for(NoteStage::Tests),
                _ => vec![],
            },
            extra: OstraInputs {
                work: Some(kind),
                ledger_file: if matches!(kind, WorkKind::Fix | WorkKind::BlockerFix) {
                    Some(s.ledger_path(&project, phase, tests))
                } else {
                    None
                },
                ..Default::default()
            }
            .into_value(),
            ..Default::default()
        };
        let purpose = match contract {
            Contract::Tests => {
                inputs.implementer_report = p.implementer_report.clone();
                let epa = match &p.epa {
                    EpaState::Done(path) => Some(path.clone()),
                    _ => None,
                };
                inputs.set_ox(|x| x.epa_report = epa);
                inputs.report_file = Some(dir.join(report::write_test(&phase_str)));
                ExecPurpose::WriteTest { phase, work: kind }
            }
            Contract::Prompt => {
                inputs.task = Some(s.full_request());
                inputs.set_ox(|x| x.target_files = Some("Determine them from the task.".into()));
                inputs.report_file = Some(dir.join(report::prompt_gen(s.ext.os().prompt_gens + 1)));
                ExecPurpose::PromptGen { handoff_for: None }
            }
            _ => {
                inputs.report_file = Some(dir.join(report::implementer(&phase_str)));
                if s.category == Some(Category::Verify) {
                    inputs.task = Some(format!(
                        "Verification request: {}\nRun the project's test command and report the result.",
                        s.full_request()
                    ));
                    ExecPurpose::Verify { phase }
                } else {
                    if p.info.file.is_none() {
                        inputs.task = Some(s.full_request());
                        if s.category == Some(Category::Implement)
                            && s.ext.os().track == Some(Track::Light)
                        {
                            inputs.research_docs = s.research_docs();
                        }
                    }
                    if let Some(r) = &p.revision {
                        // Rule F2: a revision reads the session context file, not a conversation.
                        inputs.task = Some(revision_task(r, &s.full_request()));
                        let prior: Vec<PathBuf> = s
                            .ext
                            .os()
                            .phases
                            .values()
                            .filter(|q| q.info.project == project && q.info.id != phase)
                            .filter_map(|q| q.implementer_report.clone())
                            .collect();
                        inputs.set_ox(|x| {
                            x.revision = Some(r.round);
                            x.context_files = vec![s.session_context_path()];
                            x.prior_reports = prior;
                        });
                    }
                    ExecPurpose::Implement { phase, work: kind }
                }
            }
        };
        self.spawn(agent, purpose, &project, dir, inputs);
    }

    fn loop_steps(&mut self, p: &PhaseRun, tests: bool) {
        let s = self.s;
        let l = if tests { &p.test_loop } else { &p.impl_loop };
        if l.running.is_some() || l.gate.is_some() {
            return;
        }
        let project = p.info.project.clone();
        let phase = p.info.id;
        let dir = s.project_session_dir(&project);
        match &l.next {
            LoopNext::Idle | LoopNext::Done => {}
            LoopNext::RescueAdvise {
                advisor: Some(_), ..
            } => {}
            LoopNext::RescueAdvise {
                exec,
                stuck,
                advisor: None,
            } => {
                // Rule O7: the advisor looks at a stuck run whose failure is in its environment.
                let rec = s.executions.get(exec);
                let context = format!(
                    "Phase {phase} of the session: {}{}. The step is in the {} loop of this phase.",
                    p.info.title,
                    p.info
                        .file
                        .as_ref()
                        .map(|f| format!(" (phase file {})", f.display()))
                        .unwrap_or_default(),
                    if tests { "test" } else { "build" },
                );
                self.push(Step::Spawn(Box::new(crate::init::advisor_request(
                    crate::init::AdviceInputs {
                        advisor: s.agent_for(loop_stage(tests), Contract::Advice),
                        project: project.clone(),
                        session_dir: dir,
                        execution: exec.clone(),
                        failed_step: rec
                            .map(|r| crate::init::failed_step_label(r.agent, &r.purpose))
                            .unwrap_or_default(),
                        problem: format!(
                            "The step returned stuck.\nDiagnostic:\n{}\nNeed: {}",
                            stuck.diagnostic, stuck.need
                        ),
                        step_inputs: rec.map(|r| r.spawn_block.clone()).unwrap_or_default(),
                        step_result: rec
                            .and_then(|r| r.result.as_ref())
                            .and_then(|r| r.submit.clone()),
                        context,
                        earlier: l.advice.clone(),
                    },
                ))));
            }
            LoopNext::RescueFix { fixer: Some(_), .. } => {}
            LoopNext::RescueFix {
                exec,
                stuck,
                instructions,
                fixer: None,
            } => {
                // Rule O8: the user sent an implementer to fix what stopped the stuck run.
                let round = s
                    .executions
                    .values()
                    .filter(|e| matches!(&e.purpose, ExecPurpose::Unblock { execution, .. } if execution == exec))
                    .count() as u32
                    + 1;
                let agent = s.executions.get(exec).map(|r| r.agent);
                let unblock = format!(
                    "The {} run of phase {phase} ({} loop) stopped with STUCK and waits for this fix. Its diagnostic, verbatim:\n{}\nIt needs: {}\n{}",
                    agent.map(|a| a.as_str()).unwrap_or("implementer"),
                    if tests { "test" } else { "build" },
                    stuck.diagnostic,
                    stuck.need,
                    match instructions {
                        Some(i) => format!("The user's instructions: {i}"),
                        None =>
                            "The user gave no instructions: fix the cause the diagnostic and the need name."
                                .into(),
                    }
                );
                let phase_str = if tests {
                    format!("{phase}-tests")
                } else {
                    phase.to_string()
                };
                let inputs = SpawnInputs {
                    phase: Some(p.info.clone()),
                    instructions: Some(unblock),
                    report_file: Some(dir.join(report::unblock(&phase_str, round))),
                    user_notes: s.notes_for(NoteStage::Implement),
                    extra: OstraInputs {
                        context_files: p.info.file.iter().cloned().collect(),
                        ..Default::default()
                    }
                    .into_value(),
                    ..Default::default()
                };
                self.spawn(
                    s.agent_for(loop_stage(tests), Contract::Implementation),
                    ExecPurpose::Unblock {
                        phase,
                        tests,
                        execution: exec.clone(),
                        round,
                    },
                    &project,
                    dir,
                    inputs,
                );
            }
            LoopNext::Work { kind, instructions } => {
                self.loop_work(p, tests, *kind, instructions.clone())
            }
            LoopNext::Review => {
                let iteration = l.iterations + 1;
                let phase_value = if tests {
                    format!("{phase}-tests")
                } else {
                    phase.to_string()
                };
                let rationale =
                    l.rationale
                        .clone()
                        .unwrap_or_else(|| match (&p.revision, &p.info.file) {
                            (Some(r), _) => revision_task(r, &s.full_request()),
                            (None, Some(_)) => format!("Phase {phase}: {}", p.info.title),
                            (None, None) => s.full_request(),
                        });
                let inputs = SpawnInputs {
                    phase: Some(p.info.clone()),
                    phase_value: Some(phase_value),
                    extra: OstraInputs {
                        changed_files: l.changed.iter().cloned().collect(),
                        rationale: Some(rationale),
                        ledger_file: Some(s.ledger_path(&project, phase, tests)),
                        epa_report: if tests {
                            match &p.epa {
                                EpaState::Done(path) => Some(path.clone()),
                                _ => None,
                            }
                        } else {
                            None
                        },
                        ..Default::default()
                    }
                    .into_value(),
                    ..Default::default()
                };
                self.spawn(
                    s.agent_for(loop_stage(tests), Contract::Review),
                    ExecPurpose::Review {
                        phase,
                        tests,
                        iteration,
                    },
                    &project,
                    dir,
                    inputs,
                );
            }
            LoopNext::Autofix { apply, .. } => {
                self.push(Step::from(OstraStep::Autofix {
                    project,
                    phase,
                    tests,
                    findings: apply.clone(),
                }));
            }
            LoopNext::Rescue { exec, .. } => {
                self.push(Step::Judge {
                    judge: JudgeKind::Rescue,
                    subject: Some(exec.to_string()),
                });
            }
            LoopNext::RescueExplore { .. } | LoopNext::AnswerResearch { .. } => {}
            LoopNext::RescueGate { exec, stuck } => {
                let agent = s
                    .executions
                    .get(exec)
                    .map(|r| r.agent)
                    .unwrap_or_else(|| s.agent_for(loop_stage(tests), l.work));
                self.gate(
                    format!("Phase {phase} is stuck"),
                    "The agent hit its retry ceiling on the same failure. State the missing fact, send an implementer to fix the cause so the agent can continue, or leave the phase blocked.",
                    GatePayload::Stuck {
                        execution: exec.clone(),
                        agent,
                        project,
                        phase: Some(phase),
                        diagnostic: stuck.diagnostic.clone(),
                        need: stuck.need.clone(),
                    },
                );
            }
            LoopNext::AwaitRoute { gate, .. } => {
                self.push(Step::Judge {
                    judge: JudgeKind::RouteAnswer,
                    subject: Some(gate.to_string()),
                });
            }
            LoopNext::Handoff { exec, handoff } => {
                let inputs = SpawnInputs {
                    task: Some(handoff.request.clone()),
                    report_file: Some(dir.join(report::prompt_gen(s.ext.os().prompt_gens + 1))),
                    extra: OstraInputs {
                        target_files: Some(if handoff.target_files.is_empty() {
                            "Determine them from the task.".into()
                        } else {
                            handoff.target_files.join("\n")
                        }),
                        ..Default::default()
                    }
                    .into_value(),
                    ..Default::default()
                };
                self.spawn(
                    s.agent_for(BuiltinStage::Build, Contract::Prompt),
                    ExecPurpose::PromptGen {
                        handoff_for: Some(exec.clone()),
                    },
                    &project,
                    dir,
                    inputs,
                );
            }
            LoopNext::CapReached { findings } => {
                if s.yolo {
                    // YOLO toggled on while the loop waited at its cap.
                    self.push(Step::Judge {
                        judge: JudgeKind::ResolveReview,
                        subject: Some(loop_key_str((phase, tests))),
                    });
                } else {
                    self.gate(
                        format!("Review of phase {phase} reached its cap"),
                        format!(
                            "{} review passes ran and {} findings are still open. Choose another fix-and-review pass, or stop and leave the phase blocked.",
                            l.iterations,
                            findings.len()
                        ),
                        GatePayload::ReviewCap {
                            project,
                            phase,
                            tests,
                            iterations: l.iterations,
                            findings: findings.clone(),
                            ledger_path: s.ledger_path(&p.info.project, phase, tests),
                        },
                    );
                }
            }
            LoopNext::Resolve { .. } => {
                self.push(Step::Judge {
                    judge: JudgeKind::ResolveReview,
                    subject: Some(loop_key_str((phase, tests))),
                });
            }
            LoopNext::Stage => {
                let files: Vec<String> = l.changed.iter().cloned().collect();
                self.push(Step::from(OstraStep::Command {
                    purpose: CommandPurpose::Stage,
                    project,
                    command: None,
                    files,
                }));
            }
            LoopNext::Failed { exec, error } => {
                let agent = s
                    .executions
                    .get(exec)
                    .map(|r| r.agent)
                    .unwrap_or_else(|| s.agent_for(loop_stage(tests), l.work));
                self.exec_failed_gate(exec, agent, &project, error);
            }
            LoopNext::Blocked { reason } => {
                if !l.announced_block {
                    self.push(Step::from(OstraStep::AnnounceBlocked {
                        project: project.clone(),
                        phase,
                        tests,
                        reason: reason.clone(),
                    }));
                } else if !s.yolo && !l.block_gate_answered && p.blocked_gate.is_none() {
                    // Rule D9: report the blocked phase and ask how to proceed.
                    self.gate(
                        format!("Phase {phase} is blocked"),
                        "Phases that depend on it are removed from the queue; independent phases keep running. Retry with instructions, or leave it blocked.",
                        GatePayload::PhaseBlocked { project, phase, reason: reason.clone() },
                    );
                }
            }
        }
    }

    fn implementation_review(&mut self) -> bool {
        let s = self.s;
        let f = &s.ext.os().feedback;
        if f.accepted || s.ext.os().phases.is_empty() {
            return true;
        }
        if f.gate.is_some() || !self.nothing_running() {
            return false;
        }
        let removed = removed_phases(s);
        if !s
            .ext
            .os()
            .phases
            .values()
            .all(|p| removed.contains(&p.info.id) || p.impl_loop.is_terminal())
        {
            return false;
        }
        if let Some(i) = f.rounds.iter().position(|r| r.route.is_none()) {
            self.push(Step::Judge {
                judge: JudgeKind::Feedback,
                subject: Some(i.to_string()),
            });
            return false;
        }
        if f.rounds.iter().any(|r| r.awaiting_spec) {
            return false;
        }
        // Rule F1: every phase finished, so the user reviews the result before the closing stages.
        let blocked = s
            .ext
            .os()
            .phases
            .values()
            .filter_map(|p| match &p.impl_loop.next {
                LoopNext::Blocked { reason } => Some(format!("Phase {}: {reason}", p.info.id)),
                _ => None,
            })
            .collect();
        self.gate(
            "Review the implementation",
            "Every phase is built and reviewed. Try the change, then describe what to change, or accept it. Accepting moves on to formatting, tests, documentation, and the completion report.",
            GatePayload::ImplementationReview {
                round: f.rounds.len() as u32 + 1,
                context_path: s.session_context_path(),
                reports: s
                    .ext.os().phases
                    .values()
                    .filter_map(|p| p.implementer_report.clone())
                    .collect(),
                blocked,
            },
        );
        false
    }

    fn project_phases(&self, project: &str) -> Vec<&'a PhaseRun> {
        self.s
            .ext
            .os()
            .phases
            .values()
            .filter(|p| p.info.project == project)
            .collect()
    }

    fn project_code_done(&self, project: &str, removed: &BTreeSet<u32>) -> bool {
        self.project_phases(project)
            .iter()
            .all(|p| removed.contains(&p.info.id) || p.impl_loop.is_terminal())
    }

    fn closing_stages(&mut self) {
        let s = self.s;
        let removed = removed_phases(s);
        let projects: Vec<String> = s.ext.os().project_tracks.keys().cloned().collect();
        let mut closing_items = vec![];
        for key in &projects {
            if !self.project_code_done(key, &removed) {
                continue;
            }
            let passed: Vec<&PhaseRun> = self
                .project_phases(key)
                .into_iter()
                .filter(|p| p.impl_loop.is_done())
                .collect();
            if passed.is_empty() {
                continue;
            }
            let track = &s.ext.os().project_tracks[key];
            // Rule D8: format runs once per project after its last phase, not gated.
            if track.format.is_none() {
                let command = self.ctx.format_commands.get(key).cloned().flatten();
                self.push(Step::from(OstraStep::Command {
                    purpose: CommandPurpose::Format,
                    project: key.clone(),
                    command,
                    files: vec![],
                }));
                continue;
            }
            let closing = match track.closing {
                Some(c) => c,
                None => {
                    // Rule T3: an explicit request replaces the gate.
                    let (ask_tests, ask_docs) = (!s.tests_requested(), !s.docs_requested());
                    if !ask_tests && !ask_docs {
                        (true, true)
                    } else {
                        if track.closing_gate.is_none() {
                            closing_items.push(ClosingItem {
                                project: key.clone(),
                                phases: passed.len() as u32,
                                ask_tests,
                                ask_docs,
                            });
                        }
                        continue;
                    }
                }
            };
            let tests_done = if closing.0 {
                self.test_stage(key, &passed)
            } else {
                true
            };
            // Rule B10: a workflow with a book stage writes the docs there, after closing.
            if closing.1 && tests_done && !book_node(s) {
                self.docs_stage(key, &passed);
            }
        }
        if !closing_items.is_empty() {
            // Rule T6: projects reaching the gate together are asked in one batch.
            let n: u32 = closing_items.iter().map(|i| i.phases).sum();
            self.gate(
                "Tests and documentation",
                format!(
                    "All {n} phases are implemented and reviewed. Writing tests and writing the documentation book are optional. Neither changes the requirements (Rule T5)."
                ),
                GatePayload::ClosingGate { items: closing_items },
            );
        }
        if !book_node(s) {
            self.book_stage();
        }
    }

    fn test_stage(&mut self, project: &str, passed: &[&'a PhaseRun]) -> bool {
        let s = self.s;
        // Rule T4: Required phases are covered; no plan means the whole change is covered.
        let covered: Vec<&PhaseRun> = passed
            .iter()
            .copied()
            .filter(|p| p.info.file.is_none() || p.info.test_policy == TestPolicy::Required)
            .collect();
        let dir = s.project_session_dir(project);
        let mut all_epa_done = true;
        for p in &covered {
            match &p.epa {
                EpaState::NotStarted => {
                    all_epa_done = false;
                    let inputs = SpawnInputs {
                        phase: Some(p.info.clone()),
                        implementer_report: p.implementer_report.clone(),
                        report_file: Some(dir.join(report::epa(&p.info.id.to_string()))),
                        user_notes: s.notes_for(NoteStage::Tests),
                        ..Default::default()
                    };
                    self.spawn(
                        s.agent_for(BuiltinStage::Closing, Contract::PathAnalysis),
                        ExecPurpose::Epa { phase: p.info.id },
                        project,
                        dir.clone(),
                        inputs,
                    );
                }
                EpaState::Running(_) => all_epa_done = false,
                EpaState::Failed {
                    exec, error, gate, ..
                } => {
                    all_epa_done = false;
                    if gate.is_none() {
                        self.exec_failed_gate(
                            exec,
                            s.agent_for(BuiltinStage::Closing, Contract::PathAnalysis),
                            project,
                            error,
                        );
                    }
                }
                EpaState::Done(_) | EpaState::Abandoned => {}
            }
        }
        if !all_epa_done {
            return false;
        }
        // Analyze in parallel, write serially, in phase order (Rule T4).
        for p in &covered {
            if matches!(p.epa, EpaState::Abandoned) {
                continue;
            }
            let l = &p.test_loop;
            if l.is_blocked() {
                // Rule D9: a blocked test phase is announced, and asked about outside YOLO, before
                // the completion report, which waits for the announcement.
                self.loop_steps(p, true);
                if l.announced_block && (s.yolo || l.block_gate_answered) {
                    continue;
                }
                return false;
            }
            if l.is_terminal() {
                continue;
            }
            if l.is_idle() {
                self.loop_work(p, true, WorkKind::Initial, None);
            } else {
                self.loop_steps(p, true);
            }
            return false;
        }
        true
    }

    fn docs_stage(&mut self, project: &str, passed: &[&PhaseRun]) {
        let s = self.s;
        let track = &s.ext.os().project_tracks[project];
        // Hard rule 21: an open BLOCKER blocks documentation.
        if blocker_open(passed) {
            return;
        }
        let base = SpawnInputs {
            user_notes: s.notes_for(NoteStage::Docs),
            extra: OstraInputs {
                implementer_reports: passed
                    .iter()
                    .filter_map(|p| p.implementer_report.clone())
                    .collect(),
                ..Default::default()
            }
            .into_value(),
            ..Default::default()
        };
        let stage = if book_node(s) {
            BuiltinStage::Book
        } else {
            BuiltinStage::Closing
        };
        let writer = s.agent_for(stage, Contract::Documentation);
        let docs = |page: Option<&str>, round: u32| ExecPurpose::Docs {
            project: project.to_string(),
            page: page.map(str::to_string),
            round,
        };
        // Rule B10: a log from before the pipeline keeps its whole-part writer.
        if !matches!(track.docs, DocsState::NotStarted) {
            self.docs_run(project, &track.docs, writer, docs(None, 0), base);
            return;
        }
        // Rule B10: scan the modules and constants, survey, write a first draft of each page, then
        // synthesis rounds until done. A log whose survey started before the scan keeps going.
        if track.docs_scan.is_none() && matches!(track.survey, DocsState::NotStarted) {
            self.push(Step::from(OstraStep::ScanDocs {
                project: project.to_string(),
            }));
            return;
        }
        let base = SpawnInputs {
            extra: OstraInputs {
                docs_reference: track
                    .docs_scan
                    .as_ref()
                    .map(|_| s.docs_drafts_dir(project).join("reference.md")),
                ..OstraInputs::of(&base)
            }
            .into_value(),
            ..base
        };
        let Some(survey) = track.survey_plan() else {
            let inputs = SpawnInputs {
                extra: OstraInputs {
                    docs_step: Some(DocsStep::Survey),
                    ..OstraInputs::of(&base)
                }
                .into_value(),
                ..base
            };
            let purpose = ExecPurpose::DocsSurvey {
                project: project.to_string(),
            };
            self.docs_run(project, &track.survey, writer, purpose, inputs);
            return;
        };
        let planned = track.planned_pages();
        let inventory = track.current_inventory();
        let page_inputs =
            |page: &ostra_core::book::PlannedPage, instructions: Vec<String>| SpawnInputs {
                extra: OstraInputs {
                    docs_step: Some(DocsStep::Page),
                    docs_page: Some(page.clone()),
                    docs_pages: survey.pages.clone(),
                    docs_inventory: inventory
                        .iter()
                        .filter(|i| i.owner == page.id)
                        .map(|i| {
                            let mut line = format!("{} ({})", i.name, i.sources.join(", "));
                            if !i.settings.is_empty() {
                                line.push_str(&format!(", settings: {}", i.settings.join(", ")));
                            }
                            line
                        })
                        .collect(),
                    docs_instructions: instructions,
                    ..OstraInputs::of(&base.clone())
                }
                .into_value(),
                ..base.clone()
            };
        for page in &planned {
            let Some(d) = track.page_docs.get(&page.id) else {
                continue;
            };
            self.docs_run(
                project,
                &d.run,
                writer,
                docs(Some(&page.id), 0),
                page_inputs(page, vec![]),
            );
        }
        if !track.first_drafts_settled() || track.docs_finished() {
            return;
        }
        let done_rounds = track.docs_rounds.len() as u32;
        let last = track.docs_rounds.last();
        // The round in progress, or the next one when the last round's revisions all settled.
        let round = match last {
            Some(r)
                if !r.synthesis.is_settled()
                    || !r
                        .targets
                        .keys()
                        .all(|p| r.revisions.get(p).is_some_and(|x| x.is_settled())) =>
            {
                done_rounds
            }
            _ => done_rounds + 1,
        };
        let current = track.docs_rounds.get(round as usize - 1);
        // 1. Fact-check each page that changed.
        let checker = s.agent_for(stage, Contract::FactCheck);
        let to_check = track.pages_to_check(round);
        let mut checking = false;
        for page in &to_check {
            let state = current
                .and_then(|r| r.checks.get(page))
                .cloned()
                .unwrap_or(crate::data::StageRun::NotStarted);
            checking |= !state.is_settled();
            let prior = (1..round)
                .rev()
                .find_map(
                    |k| match track.docs_rounds[k as usize - 1].checks.get(page) {
                        Some(crate::data::StageRun::Done(c)) => Some(c.findings_text()),
                        _ => None,
                    },
                )
                .unwrap_or_else(|| "none".into());
            let inputs = SpawnInputs {
                target: Some(s.docs_draft_path(project, page)),
                spec_file: Some(s.docs_inventory_path(project)),
                prior_findings: Some(prior),
                extra: OstraInputs {
                    docs_check: true,
                    source_check: Some("citations".into()),
                    ..Default::default()
                }
                .into_value(),
                ..Default::default()
            };
            let purpose = ExecPurpose::DocsCheck {
                project: project.to_string(),
                page: page.clone(),
                round,
            };
            self.docs_run(project, &state, checker, purpose, inputs);
        }
        if checking {
            return;
        }
        // 2. One synthesis pass over every draft, with the findings and the engine's checks.
        let synthesis = current
            .map(|r| r.synthesis.clone())
            .unwrap_or(DocsState::NotStarted);
        if !matches!(synthesis, DocsState::Done(_)) {
            let mut findings: Vec<String> = current
                .map(|r| {
                    r.checks
                        .iter()
                        .filter_map(|(page, c)| match c {
                            crate::data::StageRun::Done(c) => {
                                Some(format!("{page}: {:?}, {}", c.verdict, c.findings_text()))
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            for (page, issues) in track.engine_issues() {
                let page = if page.is_empty() {
                    "inventory".to_string()
                } else {
                    page
                };
                findings.push(format!("{page} (engine check): {}", issues.join(" ")));
            }
            // Rule B10: the named constants that no page mentions, for the coverage check.
            if let Some(scan) = &track.docs_scan {
                let drafts = track.placed_drafts();
                let missing = crate::book::unmentioned_refs(&drafts, &scan.refs);
                if !missing.is_empty() {
                    let shown: Vec<String> = missing
                        .iter()
                        .take(80)
                        .map(|r| format!("`{}` ({})", r.name, r.file))
                        .collect();
                    findings.push(format!(
                        "reference (engine check): {} of {} named constants appear on no page: {}{}",
                        missing.len(),
                        scan.refs.len(),
                        shown.join(", "),
                        if missing.len() > 80 { ", and more in reference.md" } else { "" }
                    ));
                }
            }
            let inputs = SpawnInputs {
                extra: OstraInputs {
                    docs_step: Some(DocsStep::Synthesis),
                    docs_pages: survey.pages.clone(),
                    docs_instructions: findings,
                    docs_round: round,
                    ..OstraInputs::of(&base)
                }
                .into_value(),
                ..base
            };
            let purpose = ExecPurpose::DocsSynthesis {
                project: project.to_string(),
                round,
            };
            self.docs_run(project, &synthesis, writer, purpose, inputs);
            return;
        }
        let Some(r) = current else {
            return;
        };
        // 3. Rule B10: after each DOCS_ROUNDS rounds without done, the user decides first.
        if round % DOCS_ROUNDS == 0 && track.docs_continued < round {
            if track.docs_gate.is_none() {
                let open: Vec<String> = match &r.synthesis {
                    DocsState::Done(syn) => syn
                        .checks
                        .iter()
                        .filter(|c| !c.passed)
                        .map(|c| format!("{}: {}", c.check, c.note))
                        .collect(),
                    _ => vec![],
                };
                self.gate(
                    format!("Documentation for {project} is not done after {round} rounds"),
                    format!(
                        "{round} synthesis rounds ran, and {} pages still need changes. Run another round, or accept the book as it is.",
                        r.targets.len()
                    ),
                    GatePayload::DocsRounds {
                        project: project.to_string(),
                        rounds: round,
                        open,
                    },
                );
            }
            return;
        }
        // 4. Revise each page that the round names.
        for (page_id, instructions) in &r.targets {
            let Some(page) = planned.iter().find(|p| &p.id == page_id) else {
                continue;
            };
            let state = r
                .revisions
                .get(page_id)
                .cloned()
                .unwrap_or(DocsState::NotStarted);
            let mut inputs = page_inputs(page, instructions.clone());
            inputs.target = Some(s.docs_draft_path(project, page_id));
            self.docs_run(project, &state, writer, docs(Some(page_id), round), inputs);
        }
    }

    fn docs_run<T>(
        &mut self,
        project: &str,
        state: &crate::data::StageRun<T>,
        agent: AgentName,
        purpose: ExecPurpose,
        inputs: SpawnInputs,
    ) {
        let s = self.s;
        match state {
            crate::data::StageRun::NotStarted => {
                // Rule B7: docs runs fan out at once, and the workspace's slot limit bounds them.
                self.spawn(
                    agent,
                    purpose,
                    project,
                    s.project_session_dir(project),
                    inputs,
                );
            }
            crate::data::StageRun::Failed {
                exec,
                error,
                gate: None,
                ..
            } => {
                let (exec, error) = (exec.clone(), error.clone());
                self.exec_failed_gate(&exec, agent, project, &error);
            }
            _ => {}
        }
    }

    fn book_progress(&self) -> Option<BookProgress> {
        let s = self.s;
        let removed = removed_phases(s);
        let mut parts = vec![];
        for key in s.ext.os().project_tracks.keys() {
            if !self.project_code_done(key, &removed) {
                return None;
            }
            let passed: Vec<&PhaseRun> = self
                .project_phases(key)
                .into_iter()
                .filter(|p| p.impl_loop.is_done())
                .collect();
            if passed.is_empty() {
                continue;
            }
            let track = &s.ext.os().project_tracks[key];
            let docs_on = match track.closing {
                Some(c) => c.1,
                None if s.tests_requested() && s.docs_requested() => true,
                None => return None,
            };
            if !docs_on || blocker_open(&passed) {
                continue;
            }
            match track.docs_aggregate() {
                DocsState::Done(_) => parts.push(key.clone()),
                DocsState::Abandoned => {}
                _ => return None,
            }
        }
        Some(BookProgress { parts })
    }

    fn book_stage(&mut self) {
        let s = self.s;
        let Some(progress) = self.book_progress() else {
            return;
        };
        if progress.parts.is_empty() || s.book_written.is_some() {
            return;
        }
        // Rule B6: the book is named after the parts written, so an abandoned part names no book.
        let book = s
            .docs_book
            .clone()
            .filter(|b| ostra_core::book::is_book_id(b))
            .unwrap_or_else(|| ostra_core::book::book_id(&progress.parts));
        self.push(Step::WriteBook { book });
    }

    fn book_flow(&mut self) -> bool {
        let s = self.s;
        // A session without a recorded workflow wrote its docs in the closing stage.
        if !book_node(s) {
            return true;
        }
        let mut settled = true;
        for key in s.ext.os().project_tracks.keys() {
            if docs_chosen(s, key) != Some(true) {
                continue;
            }
            let passed = passed_phases(s, key);
            self.docs_stage(key, &passed);
            settled &= s.ext.os().project_tracks[key].docs_aggregate().is_settled();
        }
        self.book_stage();
        settled
            && match self.book_progress() {
                Some(p) if !p.parts.is_empty() => s.book_written.is_some(),
                Some(_) => true,
                None => false,
            }
    }

    fn all_implement_done(&self) -> bool {
        let s = self.s;
        if !self.nothing_running() {
            return false;
        }
        let removed = removed_phases(s);
        for key in s.ext.os().project_tracks.keys() {
            if !self.project_code_done(key, &removed) {
                return false;
            }
            let passed: Vec<&PhaseRun> = self
                .project_phases(key)
                .into_iter()
                .filter(|p| p.impl_loop.is_done())
                .collect();
            if passed.is_empty() {
                continue;
            }
            let track = &s.ext.os().project_tracks[key];
            if track.format.is_none() {
                return false;
            }
            let Some((tests, docs)) = track
                .closing
                .or_else(|| (s.tests_requested() && s.docs_requested()).then_some((true, true)))
            else {
                return false;
            };
            if tests {
                let covered = passed.iter().filter(|p| {
                    p.info.file.is_none() || p.info.test_policy == TestPolicy::Required
                });
                for p in covered {
                    let epa_ok = matches!(p.epa, EpaState::Done(_) | EpaState::Abandoned);
                    if !epa_ok
                        || (!matches!(p.epa, EpaState::Abandoned) && !p.test_loop.is_terminal())
                    {
                        return false;
                    }
                }
            }
            if docs
                && !book_node(s)
                && !blocker_open(&passed)
                && !track.docs_aggregate().is_settled()
            {
                return false;
            }
        }
        if book_node(s) {
            return true;
        }
        match self.book_progress() {
            Some(p) if !p.parts.is_empty() => s.book_written.is_some(),
            Some(_) => true,
            None => false,
        }
    }

    fn completion(&mut self) {
        let s = self.s;
        // Rule H9: nothing completes while a subagent waits for an answer.
        if !self.nothing_running()
            || s.ext.os().project_inits.values().any(|i| !i.finished)
            || s.coordination_open()
        {
            return;
        }
        match &s.completion_decision {
            None => self.push(Step::Judge {
                judge: JudgeKind::Completion,
                subject: None,
            }),
            Some(id) => {
                let md = s
                    .decisions
                    .get(id)
                    .and_then(|d| {
                        d.output
                            .get("report_markdown")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .map(|md| md + &created_projects_section(s));
                self.push(Step::Complete {
                    report_markdown: md,
                });
            }
        }
    }

    fn quick_answer(&mut self) {
        let s = self.s;
        let q = &s.ext.os().quick;
        if let Some(a) = &q.answer {
            self.push(Step::Complete {
                report_markdown: Some(a.answer.clone()),
            });
            return;
        }
        if q.running {
            return;
        }
        if let Some(err) = &q.failed {
            // Either failure gate counts: a harness that cannot run opens HarnessFailure.
            let failure_gate = |g: &ostra_engine::state::GateRecord, exec: &ExecutionId| {
                matches!(&g.payload,
                    GatePayload::ExecutionFailed { execution, .. }
                    | GatePayload::HarnessFailure { execution, .. } if execution == exec)
            };
            if let Some(exec) = &q.exec
                && !s
                    .gates
                    .values()
                    .any(|g| failure_gate(g, exec) && g.answer.is_none())
            {
                if s.gates.values().any(|g| failure_gate(g, exec)) {
                    self.push(Step::Fail { error: err.clone() });
                } else {
                    self.exec_failed_gate(
                        exec,
                        s.default_agent(Contract::Answer),
                        &s.primary(),
                        err,
                    );
                }
            }
            return;
        }
        if q.exec.is_none() {
            let primary = s.primary();
            let inputs = SpawnInputs {
                extra: OstraInputs {
                    question: Some(s.full_request()),
                    ..Default::default()
                }
                .into_value(),
                ..Default::default()
            };
            self.spawn(
                s.default_agent(Contract::Answer),
                ExecPurpose::QuickAnswer,
                &primary,
                s.session_root.clone(),
                inputs,
            );
        }
    }

    fn init_flow(&mut self) {
        crate::init::plan_init(self.s, &mut |step| self.out.push(step));
    }
}

pub trait OstraFlows<'a> {
    /// Everything the session plans after the YOLO answers: the init flow, Classify, the Route
    /// answer judge, the workflow, research, and the stages.
    fn session_flow(&mut self);
}

impl<'a> OstraFlows<'a> for Planner<'a> {
    fn session_flow(&mut self) {
        let s = self.s;
        if let SessionKind::Init { .. } = s.kind {
            self.init_flow();
            return;
        }
        // Rule C2: queued context waits for the running executions, and nothing starts before it.
        if s.held_amendments().next().is_some() {
            return;
        }
        // Rule WF1: a named workflow is resolved first, because its base is the category.
        if let Some(step @ Step::ResolveWorkflow { name: Some(_), .. }) = s.workflow_due() {
            self.push(step);
            return;
        }
        let Some(category) = s.category else {
            if s.classify.is_none() {
                self.push(Step::Judge {
                    judge: JudgeKind::Classify,
                    subject: None,
                });
            }
            return;
        };
        // Rule J1: a spec or plan answer waits for the judge before any agent sees it.
        for gate in s.ext.os().held_answers.keys() {
            self.push(Step::Judge {
                judge: JudgeKind::RouteAnswer,
                subject: Some(gate.to_string()),
            });
        }
        // Rule C2: added context waits for the judge, and nothing starts or re-runs before it.
        let mut pending = s.pending_amendments().peekable();
        if pending.peek().is_some() {
            for i in pending {
                self.push(Step::Judge {
                    judge: JudgeKind::RouteAnswer,
                    subject: Some(amendment_subject(i)),
                });
            }
            return;
        }
        // Rule WF1: a new session records its workflow before any stage runs.
        if let Some(step) = s.workflow_due() {
            self.push(step);
            return;
        }
        // Explore tasks spawn whatever the stage: rescue explores can arrive mid-build.
        self.explore_tasks();
        match category {
            Category::QuickAnswer => self.quick_answer(),
            _ => self.workflow_flow(),
        }
    }
}
