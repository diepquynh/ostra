//! The Completion judge's input: what every stage did, for the completion report.

use super::*;
#[allow(unused_imports)]
use crate::book::DocsTrack;
use crate::data::{DocsState, EpaState, LoopNext};
use crate::prelude::*;
use ostra_core::event::AnswerSource;
use ostra_core::pipeline::{Category, TestPolicy};
use ostra_engine::state::SessionState;
use std::fmt::Write;

pub(crate) fn completion_input(s: &SessionState) -> (String, String) {
    let mut m = String::new();
    let _ = writeln!(m, "# Request\n\n{}\n", s.full_request());
    let _ = writeln!(
        m,
        "Category: {}\nYOLO: {}\n",
        s.category.map(|c| c.to_string()).unwrap_or_default(),
        s.yolo
    );
    if s.category == Some(Category::QuickAnswer) {
        return (m, "Quick answer".into());
    }
    if let (Some(Category::Implement), Some(track)) = (s.category, s.ext.os().track) {
        let _ = writeln!(
            m,
            "Track: {}{}\n",
            track.as_str(),
            if track == ostra_core::pipeline::Track::Light {
                " (the spec, fact-check, and plan stages did not run)"
            } else {
                ""
            }
        );
    }
    if !s.ext.os().feedback.rounds.is_empty() {
        let _ = writeln!(m, "# Feedback rounds after the implementation\n");
        for (i, r) in s.ext.os().feedback.rounds.iter().enumerate() {
            let _ = writeln!(
                m,
                "- Round {}: {} (built as phases {:?})",
                i + 1,
                first_line(&r.text),
                r.phases
            );
        }
        m.push('\n');
    }
    let _ = writeln!(m, "# Research\n");
    for t in &s.ext.os().explore {
        match &t.result {
            Some(r) => {
                let _ = writeln!(m, "- {}: {}", r.research_path, r.findings_summary);
            }
            None if t.abandoned => {
                let _ = writeln!(m, "- Task {} abandoned: {}", t.idx, first_line(&t.task));
            }
            None => {}
        }
    }
    if let Some(spec) = &s.ext.os().spec.current {
        let _ = writeln!(
            m,
            "\n# Spec\n\n{} ({}). Approved: {}.",
            spec.spec_path,
            spec.summary,
            s.ext.os().spec.approved
        );
    }
    match (&s.ext.os().stakes, &s.ext.os().plan.current) {
        (Some((_, st)), None) => {
            let _ = writeln!(
                m,
                "\n# Plan\n\nStakes {st:?}: the plan stage was skipped and the change ran inline."
            );
        }
        (_, Some(plan)) => {
            let _ = writeln!(
                m,
                "\n# Plan\n\n{} ({})",
                plan.master_plan_path, plan.summary
            );
        }
        _ => {}
    }
    if !s.ext.os().phases.is_empty() {
        let removed = crate::planner::removed_phases(s);
        let _ = writeln!(m, "\n# Phases\n");
        for p in s.ext.os().phases.values() {
            let status = if removed.contains(&p.info.id) {
                "removed because a phase it depends on failed".to_string()
            } else {
                match &p.impl_loop.next {
                    LoopNext::Done => {
                        format!("passed review after {} passes", p.impl_loop.iterations)
                    }
                    LoopNext::Blocked { reason } => format!("BLOCKED: {reason}"),
                    other => format!("{other:?}"),
                }
            };
            let _ = writeln!(
                m,
                "- Phase {} ({}, {}): {}. {status}",
                p.info.id,
                p.info.projects().join(", "),
                p.info.title,
                p.info.test_policy_label()
            );
            if !p.impl_loop.leftover_low.is_empty() {
                for f in &p.impl_loop.leftover_low {
                    let _ = writeln!(m, "  - LOW finding left open: {}", f.line());
                }
            }
            if p.impl_loop.blocker_open || p.test_loop.blocker_open {
                let _ = writeln!(m, "  - A BLOCKER security finding is still open.");
            }
            let tests = match (&p.epa, &p.test_loop.next) {
                (EpaState::NotStarted, _) => None,
                (EpaState::Abandoned, _) => Some("test stage abandoned".to_string()),
                (_, LoopNext::Done) => Some(format!(
                    "tests written and reviewed ({} passes)",
                    p.test_loop.iterations
                )),
                (_, LoopNext::Blocked { reason }) => Some(format!("tests BLOCKED: {reason}")),
                _ => Some("tests incomplete".into()),
            };
            if let Some(t) = tests {
                let _ = writeln!(m, "  - {t}");
            }
        }
        let _ = writeln!(m, "\n# Closing stages per project\n");
        for (k, t) in &s.ext.os().project_tracks {
            let closing = t
                .closing
                .map(|(a, b)| format!("tests {}, docs {}", yes(a), yes(b)))
                .unwrap_or_else(|| "not reached".into());
            let docs = match &t.docs_aggregate() {
                DocsState::Done(d) => format!(
                    "docs written ({} sections): {}",
                    d.sections.len(),
                    d.summary.trim()
                ),
                DocsState::Abandoned => "docs abandoned".into(),
                DocsState::NotStarted => "docs not run".into(),
                _ => "docs incomplete".into(),
            };
            let format = match t.format {
                Some(None) => {
                    "no format command in project.toml, so format was skipped".to_string()
                }
                Some(Some(0)) => "the format command ran and exited 0".to_string(),
                Some(Some(code)) => format!("format exited {code}"),
                None => "format not run".into(),
            };
            let _ = writeln!(m, "- {k}: {format}; closing gate: {closing}; {docs}");
        }
    }
    let yolo: Vec<String> = s
        .gates
        .values()
        .filter(|g| g.source == Some(AnswerSource::Yolo))
        .map(|g| {
            format!(
                "- {} (gate {}): {}",
                g.title,
                g.id,
                g.reason.clone().unwrap_or_default()
            )
        })
        .collect();
    if !yolo.is_empty() {
        let _ = writeln!(m, "\n# Decided for you under YOLO\n\n{}", yolo.join("\n"));
    }
    if !s.notes.is_empty() {
        let _ = writeln!(
            m,
            "\n# Notes\n\n{}",
            s.notes
                .iter()
                .map(|n| format!("- {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    (m, "Session state at completion".into())
}

pub(crate) trait PolicyLabel {
    fn test_policy_label(&self) -> String;
}

impl PolicyLabel for ostra_core::pipeline::PhaseInfo {
    fn test_policy_label(&self) -> String {
        match self.test_policy {
            TestPolicy::Required => "tests Required".into(),
            TestPolicy::Skip => format!(
                "tests Skip ({})",
                self.test_rationale.clone().unwrap_or_default()
            ),
        }
    }
}
