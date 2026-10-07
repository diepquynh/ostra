//! Rules D6, D7, D9, M2 to M6, and Step 4: the phases and their work and review loops.

use crate::inputs::{OstraInputs, OstraInputsExt};
use crate::judge::NoteStage;
use crate::planner::*;
#[allow(unused_imports)]
use crate::prelude::*;
use crate::steps::OstraStep;
use ostra_core::Contract;
use ostra_core::event::{CommandPurpose, ExecPurpose, GatePayload, JudgeKind, WorkKind};
use ostra_core::paths::report;
use ostra_core::pipeline::{Category, Track};
use ostra_core::workflow::{BuiltinStage, StageScope, WorkflowDef};
use ostra_engine::plan::*;
use ostra_engine::state::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

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

/// Rules D6, D7, D9, M2 to M6, and Step 4: the phases and their work and review loops.
pub trait PlannerBuild<'a> {
    /// Every phase ended and every phase stage of a passed phase is done (Rule WF4).
    fn build_done(&self, wf: &WorkflowDef) -> bool;

    fn phases(&mut self);

    fn loop_work(
        &mut self,
        p: &PhaseRun,
        tests: bool,
        kind: WorkKind,
        instructions: Option<String>,
    );

    fn loop_steps(&mut self, p: &PhaseRun, tests: bool);

    fn project_phases(&self, project: &str) -> Vec<&'a PhaseRun>;

    fn project_code_done(&self, project: &str, removed: &BTreeSet<u32>) -> bool;
}

impl<'a> PlannerBuild<'a> for Planner<'a> {
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
}
