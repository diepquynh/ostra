//! The board's stage cards for each built-in stage.

use super::*;
#[allow(unused_imports)]
use crate::book::DocsTrack;
use crate::data::{DocsState, EpaState, LoopNext, StageRun, WorkLoop};
use crate::planner::removed_phases;
use crate::prelude::*;
use ostra_core::api::{StageCard, StageStatus};
use ostra_core::event::{CommandPurpose, GatePayload, JudgeKind, SessionKind};
use ostra_core::ids::ExecutionId;
use ostra_core::pipeline::{Lane, StageKind};
use ostra_core::submit::Verdict;
use ostra_engine::state::SessionState;

pub(crate) fn exec_ids(
    s: &SessionState,
    f: impl Fn(&ostra_engine::state::ExecRecord) -> bool,
) -> Vec<ExecutionId> {
    s.executions
        .values()
        .filter(|e| f(e))
        .map(|e| e.id.clone())
        .collect()
}

pub(crate) fn gate_for(
    s: &SessionState,
    f: impl Fn(&GatePayload) -> bool,
) -> Option<&ostra_engine::state::GateRecord> {
    s.gates.values().rev().find(|g| f(&g.payload))
}

pub(crate) fn run_status<T>(r: &StageRun<T>) -> Option<StageStatus> {
    match r {
        StageRun::NotStarted => None,
        StageRun::Running(_) => Some(StageStatus::Running),
        StageRun::Done(_) => Some(StageStatus::Done),
        StageRun::Failed { .. } => Some(StageStatus::Failed),
        StageRun::Abandoned => Some(StageStatus::Skipped),
    }
}

pub(crate) fn card(stage: StageKind, label: String, status: StageStatus) -> StageCard {
    StageCard {
        stage,
        lane: stage.lane(),
        label,
        status,
        project: None,
        phase: None,
        executions: vec![],
        gate: None,
        detail: None,
    }
}

/// The work loop whose stuck run the advisor execution `exec` looks at (Rule O7).
pub(crate) fn advised_loop(s: &SessionState, exec: &ExecutionId) -> Option<(u32, bool)> {
    match s.executions.get(exec).map(|e| &e.purpose) {
        Some(
            ostra_core::event::ExecPurpose::Advise { execution, .. }
            | ostra_core::event::ExecPurpose::Unblock { execution, .. },
        ) => s.executions.get(execution).and_then(|r| r.loop_key),
        _ => None,
    }
}

pub(crate) fn advising_detail(l: &WorkLoop) -> Option<String> {
    match l.next {
        LoopNext::RescueAdvise { .. } => Some("The advisor is looking at the stuck run.".into()),
        LoopNext::RescueFix { .. } => {
            Some("An implementer is fixing what stopped the stuck run.".into())
        }
        _ => None,
    }
}

pub(crate) fn loop_status(l: &WorkLoop) -> StageStatus {
    if l.running.is_some() {
        return StageStatus::Running;
    }
    match &l.next {
        LoopNext::Idle => StageStatus::Pending,
        LoopNext::Done => StageStatus::Done,
        LoopNext::Blocked { .. } => StageStatus::Blocked,
        LoopNext::CapReached { .. } | LoopNext::RescueGate { .. } | LoopNext::Failed { .. } => {
            StageStatus::Waiting
        }
        _ => StageStatus::Running,
    }
}

pub fn stages(s: &SessionState) -> Vec<StageCard> {
    let mut out = vec![];
    use ostra_core::event::ExecPurpose as P;
    if let SessionKind::Init { project } = &s.kind {
        if let Some(i) = &s.ext.os().init {
            let st = |exec: &Option<ExecutionId>, done: bool| {
                if done {
                    StageStatus::Done
                } else if exec.is_some() {
                    StageStatus::Running
                } else {
                    StageStatus::Pending
                }
            };
            let mut c = card(
                StageKind::Detect,
                "Detect the stack".into(),
                st(&i.detect, i.detect_result.is_some()),
            );
            c.project = Some(project.clone());
            c.executions = exec_ids(s, |e| {
                matches!(
                    e.purpose,
                    P::Init {
                        mode: ostra_core::InitializerMode::Detect,
                        ..
                    }
                )
            });
            out.push(c);
            for x in &i.scouts {
                let mut c = card(
                    StageKind::Scout,
                    format!("Scout {}", x.key),
                    st(&x.exec, x.result.is_some()),
                );
                c.executions = x.exec.iter().cloned().collect();
                out.push(c);
            }
            let mut c = card(
                StageKind::Propose,
                "Propose skills".into(),
                st(&i.propose, i.propose_result.is_some()),
            );
            c.executions = i.propose.iter().cloned().collect();
            out.push(c);
            let mut c = card(
                StageKind::SkillApproval,
                "Approve skills".into(),
                if i.decisions.is_some() {
                    StageStatus::Done
                } else if i.approval_gate.is_some() {
                    StageStatus::Waiting
                } else {
                    StageStatus::Pending
                },
            );
            c.gate = i.approval_gate.clone();
            out.push(c);
            for x in &i.generates {
                let mut c = card(
                    StageKind::GenerateSkill,
                    format!("Generate {}", x.key),
                    st(&x.exec, x.result.is_some()),
                );
                c.executions = x.exec.iter().cloned().collect();
                out.push(c);
            }
            let mut c = card(
                StageKind::GenerateInventory,
                "Write the inventory".into(),
                st(&i.inventory, i.inventory_result.is_some()),
            );
            c.executions = i.inventory.iter().cloned().collect();
            out.push(c);
        }
        out.push(completion_card(s));
        return out;
    }
    let classify_status = if s.category.is_some() {
        StageStatus::Done
    } else {
        StageStatus::Running
    };
    let mut c = card(
        StageKind::Classify,
        "Classify the request".into(),
        classify_status,
    );
    c.detail = s.category.map(|c| c.to_string());
    out.push(c);
    for t in &s.ext.os().explore {
        let status = if t.result.is_some() {
            StageStatus::Done
        } else if t.abandoned {
            StageStatus::Skipped
        } else if t.failed.is_some() {
            StageStatus::Failed
        } else if t.running {
            StageStatus::Running
        } else {
            StageStatus::Pending
        };
        let mut c = card(
            StageKind::Explore,
            format!("Research: {}", first(&t.task)),
            status,
        );
        c.project = Some(t.project.clone());
        c.executions = t.exec.iter().cloned().collect();
        c.gate = t.gate.clone();
        c.detail = t.result.as_ref().map(|r| {
            format!(
                "{} sources, {} open questions",
                r.sources_retrieved, r.open_questions
            )
        });
        out.push(c);
    }
    let decisions_of = |k: JudgeKind| s.decisions.values().filter(|d| d.judge == k).count();
    if decisions_of(JudgeKind::Sufficiency) > 0 {
        let mut c = card(
            StageKind::Sufficiency,
            "Check research coverage".into(),
            StageStatus::Done,
        );
        c.detail = Some(format!("{} rounds", s.ext.os().sufficiency_rounds));
        out.push(c);
    }
    if let Some(track) = s.ext.os().track
        && s.category == Some(ostra_core::pipeline::Category::Implement)
    {
        let mut c = card(
            StageKind::Track,
            "Choose the track".into(),
            StageStatus::Done,
        );
        c.detail = Some(match track {
            ostra_core::pipeline::Track::Light => "Light: spec and plan skipped".into(),
            ostra_core::pipeline::Track::Full => "Full: spec, plan, and approvals".into(),
        });
        out.push(c);
    }
    let t = &s.ext.os().spec;
    if !t.runs.is_empty() || t.running.is_some() {
        let mut c = card(
            StageKind::Spec,
            "Write the spec".into(),
            if t.running.is_some() {
                StageStatus::Running
            } else if t.current.is_some() {
                StageStatus::Done
            } else {
                StageStatus::Failed
            },
        );
        c.executions = exec_ids(s, |e| matches!(e.purpose, P::Spec { .. }));
        c.detail = Some(format!("version {}", t.version));
        out.push(c);
        if let Some(g) = gate_for(
            s,
            |p| matches!(p, GatePayload::OpenQuestions { artifact, .. } if artifact == "spec"),
        ) {
            let mut c = card(
                StageKind::OpenQuestions,
                "Open questions".into(),
                if g.answer.is_some() {
                    StageStatus::Done
                } else {
                    StageStatus::Waiting
                },
            );
            c.gate = Some(g.id.clone());
            out.push(c);
        }
        if !t.checks.is_empty() {
            let verdict = t.check_for_current().map(|c| c.verdict);
            let mut c = card(
                StageKind::FactCheckSpec,
                "Fact-check the spec".into(),
                if t.check_running.is_some() {
                    StageStatus::Running
                } else if verdict == Some(Verdict::Pass) {
                    StageStatus::Done
                } else {
                    StageStatus::Running
                },
            );
            c.executions = exec_ids(s, |e| {
                matches!(
                    e.purpose,
                    P::FactCheck {
                        target: ostra_core::event::FactTarget::Spec,
                        ..
                    }
                )
            });
            c.detail = t
                .check_for_current()
                .map(|c| format!("{:?}, {} findings", c.verdict, c.findings.len()));
            out.push(c);
        }
        if t.passed_current() && s.category != Some(ostra_core::pipeline::Category::Spec) {
            let mut c = card(
                StageKind::SpecApproval,
                "Approve the spec".into(),
                if t.approved {
                    StageStatus::Done
                } else {
                    StageStatus::Waiting
                },
            );
            c.gate = t.approval_gate.clone();
            out.push(c);
        }
    }
    if let Some((_, stakes)) = s.ext.os().stakes {
        let mut c = card(
            StageKind::Stakes,
            "Judge the stakes".into(),
            StageStatus::Done,
        );
        c.detail = Some(format!("{stakes:?}"));
        out.push(c);
        if stakes == ostra_core::pipeline::Stakes::Low {
            out.push(card(
                StageKind::Plan,
                "Plan skipped: low stakes".into(),
                StageStatus::Skipped,
            ));
        }
    }
    let t = &s.ext.os().plan;
    if !t.runs.is_empty() {
        let mut c = card(
            StageKind::Plan,
            "Write the plan".into(),
            if t.running.is_some() {
                StageStatus::Running
            } else if t.current.is_some() {
                StageStatus::Done
            } else {
                StageStatus::Failed
            },
        );
        c.executions = exec_ids(s, |e| matches!(e.purpose, P::Plan { .. }));
        out.push(c);
        if !t.checks.is_empty() {
            let mut c = card(
                StageKind::FactCheckPlan,
                "Fact-check the plan".into(),
                if t.passed_current() {
                    StageStatus::Done
                } else {
                    StageStatus::Running
                },
            );
            c.executions = exec_ids(s, |e| {
                matches!(
                    e.purpose,
                    P::FactCheck {
                        target: ostra_core::event::FactTarget::Plan,
                        ..
                    }
                )
            });
            c.detail = t
                .check_for_current()
                .map(|c| format!("{:?}, {} findings", c.verdict, c.findings.len()));
            out.push(c);
        }
        if t.passed_current() {
            let mut c = card(
                StageKind::PlanApproval,
                "Approve the plan".into(),
                if t.approved {
                    StageStatus::Done
                } else {
                    StageStatus::Waiting
                },
            );
            c.gate = t.approval_gate.clone();
            out.push(c);
        }
    }
    // Rule O4: a created project's init shows in the build lane, ahead of its phases.
    for (key, i) in &s.ext.os().project_inits {
        let executions: Vec<ExecutionId> = s
            .executions
            .values()
            .filter(|e| {
                e.project == *key
                    && match &e.purpose {
                        P::Init { .. } => true,
                        P::Advise { execution, .. } => s
                            .executions
                            .get(execution)
                            .and_then(|r| r.loop_key)
                            .is_none(),
                        _ => false,
                    }
            })
            .map(|e| e.id.clone())
            .collect();
        let gate = i.approval_gate.clone().or_else(|| i.failed_gate.clone());
        let detail = i.note.clone().or_else(|| {
            i.advising
                .as_ref()
                .map(|_| "The advisor is looking at a failed step.".into())
        });
        let status = if i.finished && i.note.is_some() {
            StageStatus::Skipped
        } else if i.finished {
            StageStatus::Done
        } else if gate.is_some() {
            StageStatus::Waiting
        } else if executions.is_empty() {
            StageStatus::Pending
        } else {
            StageStatus::Running
        };
        let mut c = card(
            StageKind::GenerateInventory,
            format!("Initialize {key}"),
            status,
        );
        c.project = Some(key.clone());
        c.executions = executions;
        c.gate = gate;
        c.detail = detail;
        out.push(c);
    }
    let removed = removed_phases(s);
    for p in s.ext.os().phases.values() {
        let id = p.info.id;
        let status = if removed.contains(&id) {
            StageStatus::Skipped
        } else {
            loop_status(&p.impl_loop)
        };
        let mut c = card(
            StageKind::Implement,
            format!("Phase {id}: {}", p.info.title),
            status,
        );
        c.project = Some(p.info.project.clone());
        c.phase = Some(id);
        c.executions = exec_ids(s, |e| {
            e.loop_key == Some((id, false)) || advised_loop(s, &e.id) == Some((id, false))
        });
        c.gate = p.impl_loop.gate.clone().or(p.blocked_gate.clone());
        c.detail = Some(advising_detail(&p.impl_loop).unwrap_or_else(|| {
            format!(
                "{} review passes{}",
                p.impl_loop.iterations,
                if p.impl_loop.blocker_open {
                    ", BLOCKER open"
                } else {
                    ""
                }
            )
        }));
        out.push(c);
        if !matches!(p.epa, EpaState::NotStarted) || !p.test_loop.is_idle() {
            let mut c = card(
                StageKind::WriteTest,
                format!("Tests for phase {id}"),
                loop_status(&p.test_loop),
            );
            if matches!(p.epa, EpaState::Running(_)) {
                c.status = StageStatus::Running;
                c.stage = StageKind::Epa;
                c.lane = Lane::Test;
            }
            c.project = Some(p.info.project.clone());
            c.phase = Some(id);
            c.executions = exec_ids(s, |e| {
                e.loop_key == Some((id, true))
                    || matches!(e.purpose, P::Epa { phase } if phase == id)
                    || advised_loop(s, &e.id) == Some((id, true))
            });
            c.detail = advising_detail(&p.test_loop);
            out.push(c);
        }
    }
    for (i, r) in s.ext.os().feedback.rounds.iter().enumerate() {
        let mut c = card(
            StageKind::ImplementationReview,
            format!("Feedback round {}", i + 1),
            StageStatus::Done,
        );
        c.detail = Some(first(&r.text));
        out.push(c);
    }
    if s.ext.os().feedback.gate.is_some() || s.ext.os().feedback.accepted {
        let mut c = card(
            StageKind::ImplementationReview,
            "Review the implementation".into(),
            if s.ext.os().feedback.accepted {
                StageStatus::Done
            } else {
                StageStatus::Waiting
            },
        );
        c.gate = s.ext.os().feedback.gate.clone();
        out.push(c);
    }
    for (key, t) in &s.ext.os().project_tracks {
        if t.format.is_none()
            && let Some((CommandPurpose::Format, cmd)) = &t.running
        {
            let mut c = card(
                StageKind::Format,
                format!("Format {key}"),
                StageStatus::Running,
            );
            c.project = Some(key.clone());
            c.detail = Some(cmd.clone());
            out.push(c);
        }
        if let Some(f) = t.format {
            let mut c = card(
                StageKind::Format,
                format!("Format {key}"),
                StageStatus::Done,
            );
            c.project = Some(key.clone());
            c.detail = Some(match f {
                None => "no format command".into(),
                Some(code) => format!("exit {code}"),
            });
            out.push(c);
        }
        if t.closing_gate.is_some() || t.closing.is_some() {
            let mut c = card(
                StageKind::ClosingGate,
                format!("Tests and docs for {key}"),
                if t.closing.is_some() {
                    StageStatus::Done
                } else {
                    StageStatus::Waiting
                },
            );
            c.project = Some(key.clone());
            c.gate = t.closing_gate.clone();
            c.detail = t.closing.map(|(a, b)| {
                format!(
                    "tests {}, docs {}",
                    if a { "yes" } else { "no" },
                    if b { "yes" } else { "no" }
                )
            });
            out.push(c);
        }
        let docs = t.docs_aggregate();
        if let Some(st) = run_status(&docs) {
            let mut c = card(
                StageKind::Documentation,
                format!("Documentation for {key}"),
                st,
            );
            c.project = Some(key.clone());
            c.executions = exec_ids(
                s,
                |e| matches!(&e.purpose, P::Docs { project, .. } | P::DocsSurvey { project } | P::DocsCheck { project, .. } | P::DocsSynthesis { project, .. } if project == key),
            );
            let pages = t.planned_pages().len();
            let rounds = t.docs_rounds.len();
            c.detail = match &docs {
                DocsState::Done(d) => Some(format!(
                    "{} pages after {rounds} synthesis rounds",
                    d.sections.len()
                )),
                _ if rounds > 0 => Some(format!("{pages} pages, synthesis round {rounds}")),
                _ if pages > 0 => Some(format!(
                    "{} of {pages} first drafts written",
                    t.drafts().len()
                )),
                _ => None,
            };
            out.push(c);
        }
    }
    if let Some(w) = &s.book_written {
        let mut c = card(
            StageKind::BookWrite,
            format!("Book {}", w.book),
            if w.error.is_some() {
                StageStatus::Failed
            } else {
                StageStatus::Done
            },
        );
        c.detail = Some(w.error.clone().unwrap_or_else(|| w.projects.join(", ")));
        out.push(c);
    }
    if s.ext.os().quick.exec.is_some() {
        let mut c = card(
            StageKind::QuickAnswer,
            "Answer".into(),
            if s.ext.os().quick.answer.is_some() {
                StageStatus::Done
            } else {
                StageStatus::Running
            },
        );
        c.executions = s.ext.os().quick.exec.iter().cloned().collect();
        out.push(c);
    }
    out.push(completion_card(s));
    out
}
