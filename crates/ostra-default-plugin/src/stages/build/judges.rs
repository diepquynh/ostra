//! How the Rescue and ResolveReview judges' decisions change a phase's loop.

use crate::fold::*;
use crate::judge::{RescueAction, RescueOut, ResolveAction, ResolveReviewOut};
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::WorkKind;
use ostra_core::ids::ExecutionId;
use ostra_engine::state::*;
use serde_json::Value;

/// How the Rescue and ResolveReview judges' decisions change a phase's loop.
pub trait BuildJudges {
    fn rescue_decided(&mut self, subject: Option<&str>, output: &Value);

    fn resolve_review_decided(&mut self, subject: Option<&str>, output: &Value);
}

impl BuildJudges for SessionState {
    fn rescue_decided(&mut self, subject: Option<&str>, output: &Value) {
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
            RescueAction::Advise | RescueAction::Gate => LoopNext::RescueGate { exec, stuck },
        };
        if let Some(l) = self.loop_mut(key) {
            l.next = new_next;
        }
    }

    fn resolve_review_decided(&mut self, subject: Option<&str>, output: &Value) {
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
                    text.push_str(&format!(
                        "\nRecord a FIXED or WONTFIX line for each in the review ledger at {ledger}."
                    ));
                }
                l.next = LoopNext::Work {
                    kind: WorkKind::Fix,
                    instructions: Some(text),
                };
            }
            ResolveAction::Block => {
                l.next = LoopNext::Blocked {
                    reason: format!(
                        "The engine could not resolve the open review findings: {}. Ledger: {}",
                        out.reason, ledger
                    ),
                };
            }
        }
    }
}
