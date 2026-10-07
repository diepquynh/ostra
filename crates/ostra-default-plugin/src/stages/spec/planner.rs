//! Rules D1, D3, D3a, D3b, and D10: the spec, its fact-check, and its approval.

use crate::inputs::OstraInputs;
use crate::planner::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::Contract;
use ostra_core::event::{ExecPurpose, FactTarget, GatePayload};
use ostra_core::submit::Verdict;
use ostra_core::workflow::BuiltinStage;
use ostra_engine::plan::*;
use std::path::PathBuf;

/// Rules D1, D3, D3a, D3b, and D10: the spec, its fact-check, and its approval.
pub trait PlannerSpec<'a> {
    /// Returns true once the spec is done for this category: fact-check PASS, and approval when
    /// `needs_approval`.
    fn spec_flow(&mut self, needs_approval: bool) -> bool;

    fn scope_paths(&self) -> Vec<(String, PathBuf)>;
}

impl<'a> PlannerSpec<'a> for Planner<'a> {
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
}
