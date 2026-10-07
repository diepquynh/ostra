//! The spec stage on the board: the spec, its open questions, its fact-check, and its approval.

use crate::prelude::*;
use crate::view::{Artifacts, card, exec_ids, fact_check_views, gate_for};
use ostra_core::api::{FactCheckView, StageCard, StageStatus};
use ostra_core::event::{ExecPurpose as P, FactTarget, GatePayload};
use ostra_core::pipeline::{Category, StageKind};
use ostra_core::submit::Verdict;
use ostra_engine::state::SessionState;
use std::path::PathBuf;

pub(crate) fn cards(s: &SessionState, out: &mut Vec<StageCard>) {
    let t = &s.ext.os().spec;
    if t.runs.is_empty() && t.running.is_none() {
        return;
    }
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
                    target: FactTarget::Spec,
                    ..
                }
            )
        });
        c.detail = t
            .check_for_current()
            .map(|c| format!("{:?}, {} findings", c.verdict, c.findings.len()));
        out.push(c);
    }
    if t.passed_current() && s.category != Some(Category::Spec) {
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

pub(crate) fn artifacts(s: &SessionState, a: &mut Artifacts) {
    if let Some(spec) = &s.ext.os().spec.current {
        a.add(PathBuf::from(&spec.spec_path), "spec", "Spec".into(), None);
    }
}

pub(crate) fn fact_checks(s: &SessionState) -> Vec<FactCheckView> {
    fact_check_views(&s.ext.os().spec, "spec")
}
