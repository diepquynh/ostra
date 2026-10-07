//! The build stage on the board: each phase's card, the phase list, the build's artifacts, the
//! files each work pass changed, and the label of a running build run.

use crate::data::{EpaState, LoopNext};
use crate::planner::removed_phases;
use crate::prelude::*;
use crate::stages::closing;
use crate::view::{Artifacts, advised_loop, advising_detail, card, exec_ids, loop_status};
use ostra_core::api::{ChangedBy, PhaseStatus, PhaseView, StageCard, StageStatus};
use ostra_core::event::ExecPurpose as P;
use ostra_core::pipeline::StageKind;
use ostra_engine::state::SessionState;
use std::collections::BTreeMap;
use std::path::{Component, Path};

/// Each phase's implement card, followed by its test card.
pub(crate) fn cards(s: &SessionState, out: &mut Vec<StageCard>) {
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
        closing::view::test_card(s, p, out);
    }
}

/// Each phase's implementer report and review ledgers, followed by its test reports.
pub(crate) fn artifacts(s: &SessionState, a: &mut Artifacts) {
    for p in s.ext.os().phases.values() {
        if let Some(r) = &p.implementer_report {
            a.add(
                r.clone(),
                "report",
                format!("Implementer report, phase {}", p.info.id),
                Some(p.info.project.clone()),
            );
        }
        for tests in [false, true] {
            let ledger = s.ledger_path(&p.info.project, p.info.id, tests);
            if ledger.exists() {
                a.add(
                    ledger,
                    "ledger",
                    format!(
                        "Review ledger, phase {}{}",
                        p.info.id,
                        if tests { " tests" } else { "" }
                    ),
                    Some(p.info.project.clone()),
                );
            }
        }
        closing::view::phase_artifacts(p, a);
    }
}

/// The label of a running implement or review run of a phase.
pub(crate) fn running_label(purpose: &P) -> Option<String> {
    match purpose {
        P::Implement { phase, .. } => Some(format!("Implementing phase {phase}")),
        P::Review {
            phase,
            tests: false,
            iteration,
        } => Some(format!("Reviewing phase {phase}, pass {iteration}")),
        _ => None,
    }
}

pub fn phases(s: &SessionState) -> Vec<PhaseView> {
    let removed = removed_phases(s);
    s.ext
        .os()
        .phases
        .values()
        .map(|p| {
            let l = &p.impl_loop;
            let status = if removed.contains(&p.info.id) {
                PhaseStatus::Removed
            } else if l.is_blocked() {
                PhaseStatus::Blocked
            } else if l.is_done() {
                PhaseStatus::Passed
            } else if l.is_idle() {
                PhaseStatus::Queued
            } else if matches!(
                l.next,
                LoopNext::Review | LoopNext::Autofix { .. } | LoopNext::Stage
            ) || l.running.as_ref().is_some_and(|r| {
                s.executions
                    .get(r)
                    .is_some_and(|e| matches!(e.purpose, P::Review { .. }))
            }) {
                PhaseStatus::Reviewing
            } else {
                PhaseStatus::Implementing
            };
            let tests = match (&p.epa, &p.test_loop.next) {
                (EpaState::NotStarted, _) if p.test_loop.is_idle() => "none",
                (EpaState::Abandoned, _) => "skipped",
                (_, LoopNext::Done) => "passed",
                (_, LoopNext::Blocked { .. }) => "blocked",
                (EpaState::Done(_), LoopNext::Idle) => "queued",
                _ => "running",
            };
            PhaseView {
                info: p.info.clone(),
                status,
                review_iterations: l.iterations,
                tests: tests.into(),
                security_block: l.blocker_open || p.test_loop.blocker_open,
            }
        })
        .collect()
}

/// Files the session's work passes reported changing in `project`, keyed by project-relative path,
/// each attributed to the latest finished execution that reported it.
pub fn file_changes(s: &SessionState, project: &str) -> BTreeMap<String, ChangedBy> {
    let root = s.project_path(project);
    let mut out: BTreeMap<String, ChangedBy> = BTreeMap::new();
    for rec in s.executions.values().filter(|r| r.project == project) {
        let (Some((phase, tests)), Some(result)) = (rec.loop_key, rec.result.as_ref()) else {
            continue;
        };
        let Some(files) = result
            .submit
            .as_ref()
            .and_then(|v| v.get("changed_files"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        let staged = s.ext.os().phases.get(&phase).is_some_and(|p| {
            if tests {
                p.test_loop.staged
            } else {
                p.impl_loop.staged
            }
        });
        let at = rec.ended_at.unwrap_or(rec.started_at);
        for f in files.iter().filter_map(|f| f.as_str()) {
            let Some(path) = project_relative(root.as_deref(), f) else {
                continue;
            };
            if out.get(&path).is_some_and(|c| c.at > at) {
                continue;
            }
            let by = ChangedBy {
                session: s.id.clone(),
                execution: rec.id.clone(),
                agent: rec.agent,
                phase: Some(phase),
                tests,
                staged,
                running: false,
                at,
            };
            out.insert(path, by);
        }
    }
    out
}

/// A reported path as project-relative with `/` separators; `None` when it leaves the project.
pub(crate) fn project_relative(root: Option<&Path>, reported: &str) -> Option<String> {
    let p = Path::new(reported.trim());
    let rel = if p.is_absolute() {
        ostra_core::paths::normalize(p)
            .strip_prefix(ostra_core::paths::normalize(root?))
            .ok()?
            .to_path_buf()
    } else {
        p.to_path_buf()
    };
    let mut parts = vec![];
    for c in rel.components() {
        match c {
            Component::Normal(n) => parts.push(n.to_string_lossy().to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}
