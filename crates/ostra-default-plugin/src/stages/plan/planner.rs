//! Rules D4, D5, and D10: the plan, its fact-check, and its approval.

use crate::inputs::OstraInputs;
use crate::planner::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::Contract;
use ostra_core::event::{ExecPurpose, FactTarget, GatePayload};
use ostra_core::pipeline::{PhaseInfo, TestPolicy};
use ostra_core::submit::Verdict;
use ostra_core::workflow::BuiltinStage;
use ostra_engine::plan::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

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

/// Rules D4, D5, and D10: the plan, its fact-check, and its approval.
pub trait PlannerPlan<'a> {
    fn plan_flow(&mut self) -> bool;
}

impl<'a> PlannerPlan<'a> for Planner<'a> {
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
}
