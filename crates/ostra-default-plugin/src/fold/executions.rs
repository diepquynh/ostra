//! How a run that starts or ends changes the built-in stages.

use super::*;
use crate::book::DocsTrack;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::book::{DocsStep, DocumentationSubmit};
use ostra_core::event::{ExecPurpose, FactTarget, WorkKind};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::ExecutionId;
use ostra_core::submit::{
    ExploreSubmit, FactCheckSubmit, GenerateSpecSubmit, PlanSubmit, QuickAnswerSubmit, ReportSubmit,
};
use ostra_engine::state::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// How a run that starts or ends changes the built-in stages.
pub trait FoldExecutions {
    fn on_started(
        &mut self,
        id: &ExecutionId,
        purpose: &ExecPurpose,
        loop_key: Option<(u32, bool)>,
        resumed: bool,
    );

    fn on_finished(&mut self, rec: &ExecRecord, result: &ExecutionResult);
}

impl FoldExecutions for SessionState {
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
}
