//! The Feedback judge's input.

#[allow(unused_imports)]
use crate::judge_input::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_engine::state::SessionState;
use std::fmt::Write;
use std::path::Path;

pub fn feedback_input(s: &SessionState, subject: Option<&str>) -> (String, String) {
    let mut m = String::new();
    let request = s.full_request();
    let round = subject
        .and_then(|x| x.parse::<usize>().ok())
        .and_then(|i| s.ext.os().feedback.rounds.get(i));
    let text = round.map(|r| r.text.clone()).unwrap_or_default();
    let _ = writeln!(m, "# Request\n\n{request}\n");
    let _ = writeln!(
        m,
        "Track: {}\nProjects in scope: {}\n",
        s.ext.os().track.map(|t| t.as_str()).unwrap_or("full"),
        s.scope.join(", ")
    );
    match &s.ext.os().spec.current {
        Some(spec) => {
            let _ = writeln!(
                m,
                "# Current spec\n\nSummary: {}\nSpec file: {}\n\n{}\n",
                spec.summary,
                spec.spec_path,
                excerpt(Path::new(&spec.spec_path))
            );
        }
        None => {
            let _ = writeln!(
                m,
                "# No spec\n\nThis session has no spec, so every route builds a revision directly.\n"
            );
        }
    }
    let _ = writeln!(m, "# Phases built so far\n");
    for p in s.ext.os().phases.values() {
        let _ = writeln!(
            m,
            "- Phase {} ({}): {}. Report: {}",
            p.info.id,
            p.info.projects().join(", "),
            p.info.title,
            p.implementer_report
                .as_ref()
                .map(|r| r.display().to_string())
                .unwrap_or_else(|| "none".into())
        );
    }
    m.push('\n');
    research_facts(&mut m, s);
    notes_facts(&mut m, s);
    let _ = writeln!(m, "# The user's feedback on the implementation\n\n{text}");
    (m, format!("Feedback: {}", first_line(&text)))
}
