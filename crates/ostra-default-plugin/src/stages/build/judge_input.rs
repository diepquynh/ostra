//! The Rescue and ResolveReview judges' inputs.

#[allow(unused_imports)]
use crate::judge_input::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_core::event::JudgeKind;
use ostra_core::ids::ExecutionId;
use ostra_engine::state::{SessionState, parse_loop_key};
use std::fmt::Write;

pub fn rescue_input(s: &SessionState, subject: Option<&str>) -> (String, String) {
    let mut m = String::new();
    let request = s.full_request();
    let exec = subject.map(ExecutionId::from);
    let rec = exec.as_ref().and_then(|e| s.executions.get(e));
    let stuck = rec
        .and_then(|r| r.loop_key)
        .and_then(|k| s.loop_ref(k))
        .and_then(|l| match &l.next {
            LoopNext::Rescue { stuck, .. } => Some(stuck.clone()),
            _ => None,
        });
    let _ = writeln!(m, "# Request\n\n{request}\n");
    if let Some(r) = rec {
        let _ = writeln!(m, "# Stuck agent\n\n{} in project `{}`", r.agent, r.project);
        if let Some(k) = r.loop_key
            && let Some(p) = s.ext.os().phases.get(&k.0)
        {
            let _ = writeln!(
                m,
                "Phase {}: {}{}",
                p.info.id,
                p.info.title,
                p.info
                    .file
                    .as_ref()
                    .map(|f| format!(" ({})", f.display()))
                    .unwrap_or_default()
            );
        }
    }
    if let Some(st) = &stuck {
        let _ = writeln!(
            m,
            "\n# Diagnostic, verbatim\n\n{}\n\n# What it needs\n\n{}",
            st.diagnostic, st.need
        );
    }
    let previous: Vec<String> = s
        .decisions
        .values()
        .filter(|d| d.judge == JudgeKind::Rescue)
        .map(|d| {
            format!(
                "- {}: {}",
                d.output
                    .get("action")
                    .and_then(|a| a.as_str())
                    .unwrap_or("?"),
                d.reason
            )
        })
        .collect();
    if !previous.is_empty() {
        let _ = writeln!(
            m,
            "\n# Earlier rescues in this session\n\n{}",
            previous.join("\n")
        );
    }
    // Rule O7: the judge sees how much of the advisor's budget this loop has used.
    if let Some(l) = rec.and_then(|r| r.loop_key).and_then(|k| s.loop_ref(k)) {
        let _ = writeln!(
            m,
            "\n# Advisor rounds for this loop\n\n{} of {}",
            l.advice.len(),
            crate::init::MAX_ADVICE
        );
        for g in &l.advice {
            let _ = writeln!(m, "\nGuidance that did not fix it:\n{g}");
        }
    }
    (
        m,
        format!(
            "STUCK: {}",
            stuck.map(|s| first_line(&s.need)).unwrap_or_default()
        ),
    )
}

pub fn resolve_review_input(s: &SessionState, subject: Option<&str>) -> (String, String) {
    let mut m = String::new();
    let _request = s.full_request();
    let key = subject.and_then(parse_loop_key);
    let l = key.and_then(|k| s.loop_ref(k));
    let project = key
        .and_then(|k| s.ext.os().phases.get(&k.0))
        .map(|p| p.info.project.clone())
        .unwrap_or_default();
    let _ = writeln!(m, "# Review loop at its budget under YOLO\n");
    if let (Some(k), Some(l)) = (key, l) {
        let _ = writeln!(
            m,
            "Phase {}{}; review passes so far: {}; resolution rounds: {}",
            k.0,
            if k.1 { " (tests)" } else { "" },
            l.iterations,
            l.resolve_rounds
        );
        if let LoopNext::Resolve { findings } = &l.next {
            let _ = writeln!(m, "\n# Open findings\n");
            for f in findings {
                let _ = writeln!(m, "- {}", f.line());
            }
        }
        let ledger = s.ledger_path(&project, k.0, k.1);
        let _ = writeln!(
            m,
            "\n# Review ledger ({})\n\n{}",
            ledger.display(),
            excerpt(&ledger)
        );
        if let Some(p) = s
            .ext
            .os()
            .phases
            .get(&k.0)
            .and_then(|p| p.info.file.clone())
        {
            let _ = writeln!(m, "\n# Phase file ({})\n\n{}", p.display(), excerpt(&p));
        }
    }
    (m, "Open findings and the review ledger".into())
}
