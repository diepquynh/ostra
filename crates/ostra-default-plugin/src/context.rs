//! The session context file (Rule F2): a Markdown index of the session, rendered from the fold and
//! written by the runner. A revision reads it instead of a conversation, so a session can take any
//! number of feedback rounds without its context growing.

#[allow(unused_imports)]
use crate::prelude::*;

use crate::data::LoopNext;
use ostra_engine::state::SessionState;
use std::fmt::Write;

pub fn render(s: &SessionState) -> String {
    let mut m = String::new();
    let _ = writeln!(m, "# Session context\n");
    let _ = writeln!(
        m,
        "Ostra rewrites this file from the session's event log before each revision. Read the files it lists when you need their detail, because this file holds paths and short summaries only.\n"
    );
    let _ = writeln!(m, "## Request\n\n{}\n", s.full_request());
    let _ = writeln!(
        m,
        "## Route\n\n- Category: {}\n- Track: {}\n- Projects in scope: {}\n",
        s.category.map(|c| c.to_string()).unwrap_or_default(),
        s.ext.os().track.map(|t| t.as_str()).unwrap_or("none"),
        s.scope.join(", ")
    );
    let docs = s.research_docs();
    if !docs.is_empty() {
        let _ = writeln!(m, "## Research documents\n");
        for d in docs {
            let _ = writeln!(m, "- `{}`", d.display());
        }
        m.push('\n');
    }
    if let Some(spec) = &s.ext.os().spec.current {
        let _ = writeln!(
            m,
            "## Spec\n\n- `{}`: {} (approved: {})\n",
            spec.spec_path,
            spec.summary,
            s.ext.os().spec.approved
        );
    }
    if let Some(plan) = &s.ext.os().plan.current {
        let _ = writeln!(
            m,
            "## Plan\n\n- `{}`: {}\n",
            plan.master_plan_path, plan.summary
        );
    }
    if !s.ext.os().phases.is_empty() {
        let _ = writeln!(m, "## Phases\n");
        for p in s.ext.os().phases.values() {
            let status = match &p.impl_loop.next {
                LoopNext::Done => "passed review".to_string(),
                LoopNext::Blocked { reason } => format!("blocked: {reason}"),
                LoopNext::Idle => "not started".to_string(),
                _ => "in progress".to_string(),
            };
            let _ = writeln!(
                m,
                "- Phase {} ({}, {}): {status}.",
                p.info.id, p.info.project, p.info.title
            );
            if let Some(r) = &p.implementer_report {
                let _ = writeln!(m, "  - Implementer report: `{}`", r.display());
            }
            let ledger = s.ledger_path(&p.info.project, p.info.id, false);
            if p.impl_loop.iterations > 0 {
                let _ = writeln!(m, "  - Review ledger: `{}`", ledger.display());
            }
        }
        m.push('\n');
    }
    if !s.ext.os().feedback.rounds.is_empty() {
        let _ = writeln!(m, "## Feedback rounds\n");
        for (i, r) in s.ext.os().feedback.rounds.iter().enumerate() {
            let _ = writeln!(m, "### Round {}\n\n{}\n", i + 1, r.text.trim());
            if r.awaiting_spec {
                let _ = writeln!(
                    m,
                    "A requirement change: the spec is being updated before it is built.\n"
                );
            }
            for id in &r.phases {
                let _ = writeln!(m, "- Built as phase {id}.");
            }
            if !r.phases.is_empty() {
                m.push('\n');
            }
        }
    }
    m
}
