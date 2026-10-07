//! The plan stage on the board: the plan, its fact-check, and its approval.

use crate::prelude::*;
use crate::view::{Artifacts, card, exec_ids, fact_check_views};
use ostra_core::api::{FactCheckView, StageCard, StageStatus};
use ostra_core::event::{ExecPurpose as P, FactTarget};
use ostra_core::pipeline::StageKind;
use ostra_engine::state::SessionState;
use std::path::PathBuf;

pub(crate) fn cards(s: &SessionState, out: &mut Vec<StageCard>) {
    let t = &s.ext.os().plan;
    if t.runs.is_empty() {
        return;
    }
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
                    target: FactTarget::Plan,
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

pub(crate) fn artifacts(s: &SessionState, a: &mut Artifacts) {
    if let Some(plan) = &s.ext.os().plan.current {
        a.add(
            PathBuf::from(&plan.master_plan_path),
            "plan",
            "Master plan".into(),
            None,
        );
        for p in &plan.phases {
            a.add(
                plan.phase_file(p),
                "phase",
                format!("Phase {}: {}", p.id, p.title),
                Some(p.project.clone()),
            );
        }
    }
}

pub(crate) fn fact_checks(s: &SessionState) -> Vec<FactCheckView> {
    fact_check_views(&s.ext.os().plan, "plan")
}
