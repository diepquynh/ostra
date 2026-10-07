//! Rules M2 to M6 and Step 4: the work loops of the build and the tests, and their rescue.

use crate::fold::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::{ExecPurpose, WorkKind};
use ostra_core::exec::{ExecutionResult, ExecutionStatus};
use ostra_core::ids::ExecutionId;
use ostra_core::submit::{
    CodeReviewerSubmit, ImplementerSubmit, ReportSubmit, ReviewFinding, Severity, StuckInfo,
    SubmitStatus,
};
use ostra_engine::state::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Rules M2 to M6 and Step 4: the work loops of the build and the tests, and their rescue.
pub trait FoldLoops {
    fn loop_finished(&mut self, key: (u32, bool), rec: &ExecRecord, result: &ExecutionResult);

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

impl FoldLoops for SessionState {
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
