//! The research stage on the board: Classify, the explore tasks, and the sufficiency check.

use crate::prelude::*;
use crate::view::{Artifacts, card, first};
use ostra_core::api::{StageCard, StageStatus};
use ostra_core::event::JudgeKind;
use ostra_core::pipeline::StageKind;
use ostra_engine::state::SessionState;
use std::path::PathBuf;

pub(crate) fn cards(s: &SessionState, out: &mut Vec<StageCard>) {
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
}

pub(crate) fn artifacts(s: &SessionState, a: &mut Artifacts) {
    for t in &s.ext.os().explore {
        if let Some(r) = &t.result {
            a.add(
                PathBuf::from(&r.research_path),
                "research",
                format!("Research: {}", first(&t.task)),
                Some(t.project.clone()),
            );
        }
    }
}
