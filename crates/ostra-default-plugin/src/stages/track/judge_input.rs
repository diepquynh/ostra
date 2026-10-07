//! The Track judge's input.

#[allow(unused_imports)]
use crate::judge_input::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_engine::state::SessionState;
use std::fmt::Write;
use std::path::Path;

pub fn track_input(s: &SessionState) -> (String, String) {
    let mut m = String::new();
    let request = s.full_request();
    let _ = writeln!(m, "# Request\n\n{request}\n");
    let _ = writeln!(m, "Projects in scope: {}\n", s.scope.join(", "));
    let _ = writeln!(m, "# Research returned\n");
    for t in &s.ext.os().explore {
        let Some(r) = &t.result else { continue };
        let _ = writeln!(
            m,
            "## Task {} ({})\n\nTask: {}\nScope covered: {}\nFindings: {}\nNot covered: {}\n\n{}\n",
            t.idx,
            t.project,
            t.task,
            r.scope_covered,
            r.findings_summary,
            if r.not_covered.is_empty() {
                "none".to_string()
            } else {
                r.not_covered.join("; ")
            },
            excerpt_n(Path::new(&r.research_path), TRACK_EXCERPT)
        );
    }
    (m, format!("Research for: {}", first_line(&request)))
}
