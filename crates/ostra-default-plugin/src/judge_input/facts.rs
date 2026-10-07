//! The session facts several judge inputs share: the request, research, notes, gates, and added context.

#[allow(unused_imports)]
use crate::book::DocsTrack;
use crate::prelude::*;
use ostra_core::event::{ContextDelivery, ExecPurpose, GatePayload};
use ostra_engine::state::{Interrupt, SessionState};
use std::fmt::Write;

/// Rule J1: what the Route answer and Feedback judges need to see about the session.
/// Rule C2: what the Route answer judge needs to route context the user added mid-session.
pub(crate) fn amendment_facts(
    m: &mut String,
    s: &SessionState,
    request: &str,
    i: Option<usize>,
) -> (String, String) {
    let _ = writeln!(m, "# Request\n\n{request}\n");
    session_facts(m, s);
    let Some(a) = i.and_then(|i| s.amendments.get(i)) else {
        return (std::mem::take(m), "Added context: unknown".into());
    };
    let interrupted: Vec<String> = s
        .executions
        .values()
        .filter(|r| {
            r.ended_at.is_some_and(|t| t >= a.at)
                && r.result.as_ref().and_then(|x| x.error.as_deref())
                    == Some(Interrupt::Context.message())
        })
        .map(|r| match &r.purpose {
            ExecPurpose::Explore { task } => format!(
                "- Research task {} in `{}`: {}",
                task + 1,
                r.project,
                s.ext
                    .os()
                    .explore
                    .get(*task as usize)
                    .map(|t| first_line(&t.task))
                    .unwrap_or_default()
            ),
            _ => format!("- {} in `{}`", r.agent, r.project),
        })
        .collect();
    let _ = writeln!(
        m,
        "# Context the user added\n\nThere is no gate: the user added this to the running session. Give one item with ID `answer`.\n"
    );
    if a.delivery == ContextDelivery::Now {
        let _ = writeln!(
            m,
            "The user sent it now, so Ostra stopped this running work. Each re-runs with the context once you decide, unless you discard the context or skip the task:\n{}\n",
            if interrupted.is_empty() {
                "- nothing was running".to_string()
            } else {
                interrupted.join("\n")
            }
        );
    } else {
        let _ = writeln!(
            m,
            "The user queued it, so running work finishes on the old request and the next step sees it.\n"
        );
    }
    let text = s.added_part(&a.text, &a.files, &a.uploads);
    let _ = writeln!(m, "## The added context\n\n{text}");
    (
        std::mem::take(m),
        format!("Added context: {}", first_line(&a.text)),
    )
}

pub(crate) fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

pub(crate) fn session_facts(m: &mut String, s: &SessionState) {
    let _ = writeln!(
        m,
        "Category: {}\nTrack: {}\nProjects you may target: {}\n",
        s.category.map(|c| c.as_str()).unwrap_or("unknown"),
        s.ext.os().track.map(|t| t.as_str()).unwrap_or("none"),
        s.scope.join(", ")
    );
    if !s.created_projects.is_empty() {
        let _ = writeln!(
            m,
            "Projects this session created, where its new code lives: {}\n",
            s.created_projects
                .iter()
                .map(|p| p.key.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    research_facts(m, s);
    match &s.ext.os().spec.current {
        Some(spec) => {
            let _ = writeln!(
                m,
                "# Current spec\n\nSummary: {}\nSpec file: {}\n",
                spec.summary, spec.spec_path
            );
        }
        None => {
            let _ = writeln!(
                m,
                "# No spec\n\nThis session has no spec, so a delivered answer goes to the agent that asked, whatever the route.\n"
            );
        }
    }
    if s.ext.os().plan.current.is_some() {
        let _ = writeln!(m, "# Plan phases\n");
        for p in s.ext.os().phases.values() {
            let _ = writeln!(
                m,
                "- Phase {} ({}): {}",
                p.info.id, p.info.project, p.info.title
            );
        }
        m.push('\n');
    }
    notes_facts(m, s);
}

pub(crate) fn research_facts(m: &mut String, s: &SessionState) {
    let _ = writeln!(m, "# Research documents so far\n");
    let mut any = false;
    for t in &s.ext.os().explore {
        let Some(r) = &t.result else { continue };
        any = true;
        let _ = writeln!(
            m,
            "- Task {} ({}): {}\n  Findings: {}\n  Not covered: {}",
            t.idx,
            t.project,
            first_line(&t.task),
            r.findings_summary,
            if r.not_covered.is_empty() {
                "none".to_string()
            } else {
                r.not_covered.join("; ")
            }
        );
    }
    // Rule U1: numbered as the judge's `skip` names them.
    for t in s.skippable_research() {
        any = true;
        let _ = writeln!(
            m,
            "- Research task {} ({}), not finished: {}",
            t.idx + 1,
            t.project,
            first_line(&t.task)
        );
    }
    if !any {
        let _ = writeln!(m, "none");
    }
    m.push('\n');
}

pub(crate) fn notes_facts(m: &mut String, s: &SessionState) {
    if s.ext.os().user_notes.iter().any(|n| !n.forgotten) {
        let _ = writeln!(m, "# Notes already kept for later stages\n");
        for n in s.ext.os().user_notes.iter().filter(|n| !n.forgotten) {
            let stages: Vec<&str> = n
                .stages
                .iter()
                .map(|x| match x {
                    crate::judge::NoteStage::Implement => "implement",
                    crate::judge::NoteStage::Tests => "tests",
                    crate::judge::NoteStage::Docs => "docs",
                })
                .collect();
            let _ = writeln!(m, "- {} ({}): {}", n.id, stages.join(", "), n.text);
        }
        m.push('\n');
    }
}

/// The part of a gate's payload a Route answer judge needs, for gates without questions.
pub(crate) fn gate_facts(m: &mut String, payload: &GatePayload) {
    match payload {
        GatePayload::SpecApproval { summary, .. } => {
            let _ = writeln!(m, "Spec summary: {summary}\n");
        }
        GatePayload::PlanApproval {
            summary, phases, ..
        } => {
            let _ = writeln!(m, "Plan summary: {summary}\nPhases:");
            for p in phases {
                let _ = writeln!(m, "- Phase {}: {}", p.id, p.title);
            }
            m.push('\n');
        }
        GatePayload::FactCheckRecurring {
            target,
            passes,
            findings,
        } => {
            let _ = writeln!(
                m,
                "The {} fact-check failed {passes} times in a row with {} findings.\n",
                target.as_str(),
                findings.len()
            );
        }
        GatePayload::ReviewCap {
            phase,
            iterations,
            findings,
            ..
        } => {
            let _ = writeln!(
                m,
                "Phase {phase}: {iterations} review passes ran and {} findings are open:",
                findings.len()
            );
            for f in findings.iter().take(20) {
                let _ = writeln!(m, "- {:?}: {}", f.severity, first_line(&f.description));
            }
            m.push('\n');
        }
        GatePayload::Stuck {
            agent,
            phase,
            diagnostic,
            need,
            ..
        } => {
            let _ = writeln!(
                m,
                "The {agent} agent{} is stuck.\nIt needs: {need}\nDiagnostic:\n{diagnostic}\n",
                phase.map(|p| format!(" of phase {p}")).unwrap_or_default()
            );
        }
        GatePayload::PhaseBlocked { phase, reason, .. } => {
            let _ = writeln!(m, "Phase {phase} is blocked: {reason}\n");
        }
        _ => {}
    }
}

pub(crate) fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or_default();
    if l.len() > 160 {
        format!(
            "{}...",
            &l[..l.char_indices().nth(157).map(|(i, _)| i).unwrap_or(l.len())]
        )
    } else {
        l.to_string()
    }
}

pub(crate) fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}
