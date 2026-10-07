//! How the build stage's gates open and how their answers apply: the review cap, a stuck agent,
//! and a blocked phase.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::WorkKind;
use ostra_core::ids::{ExecutionId, GateId};
use ostra_core::submit::{ReviewFinding, Severity};
use ostra_engine::state::*;

/// How the build stage's gates open and how their answers apply.
pub trait BuildGates {
    fn phase_blocked_opened(&mut self, id: &GateId, phase: u32);

    fn review_cap_answered(
        &mut self,
        id: &GateId,
        phase: u32,
        tests: bool,
        findings: &[ReviewFinding],
        choice: Choice<'_>,
        routed: bool,
    );

    fn stuck_answered(&mut self, id: &GateId, execution: &ExecutionId, choice: Choice<'_>);

    fn phase_blocked_answered(&mut self, id: &GateId, phase: u32, choice: Choice<'_>);
}

impl BuildGates for SessionState {
    fn phase_blocked_opened(&mut self, id: &GateId, phase: u32) {
        if let Some(p) = self.ext.os_mut().phases.get_mut(&phase) {
            p.blocked_gate = Some(id.clone());
        }
    }

    fn review_cap_answered(
        &mut self,
        id: &GateId,
        phase: u32,
        tests: bool,
        findings: &[ReviewFinding],
        choice: Choice<'_>,
        routed: bool,
    ) {
        let project = self
            .ext
            .os()
            .phases
            .get(&phase)
            .map(|p| p.info.project.clone())
            .unwrap_or_default();
        let ledger = self
            .ledger_path(&project, phase, tests)
            .display()
            .to_string();
        if let Some(l) = self.loop_mut((phase, tests)) {
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

    fn stuck_answered(&mut self, id: &GateId, execution: &ExecutionId, choice: Choice<'_>) {
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

    fn phase_blocked_answered(&mut self, id: &GateId, phase: u32, choice: Choice<'_>) {
        let tests = self
            .ext
            .os()
            .phases
            .get(&phase)
            .is_some_and(|p| p.test_loop.is_blocked() && !p.impl_loop.is_blocked());
        if let Some(p) = self.ext.os_mut().phases.get_mut(&phase) {
            p.blocked_gate = None;
        }
        if let Some(l) = self.loop_mut((phase, tests)) {
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
}
