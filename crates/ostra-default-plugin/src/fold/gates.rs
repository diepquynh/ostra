//! How the built-in stages' gates open and how their answers apply.

use super::*;
use crate::book::DocsTrack;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::{ExecPurpose, FactTarget, GateAnswer, GatePayload, WorkKind};
use ostra_core::ids::{ExecutionId, GateId};
use ostra_core::submit::{ReviewFinding, Severity};
use ostra_engine::state::*;

/// How the built-in stages' gates open and how their answers apply.
pub trait FoldGates {
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
}

impl FoldGates for SessionState {
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
}
