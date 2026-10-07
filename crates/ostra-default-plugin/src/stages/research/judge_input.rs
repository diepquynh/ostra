//! The Classify and Sufficiency judges' inputs.

#[allow(unused_imports)]
use crate::judge_input::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_engine::pipeline::ProjectFacts;
use ostra_engine::state::SessionState;
use std::fmt::Write;

pub fn classify_input(s: &SessionState, projects: &[ProjectFacts]) -> (String, String) {
    let mut m = String::new();
    let request = s.full_request();
    let _ = writeln!(m, "# Request\n\n{request}\n");
    let _ = writeln!(
        m,
        "# Toggles from the New task form\n\n- tests: {}\n- docs: {}\n- yolo: {}\n",
        s.options.tests, s.options.docs, s.options.yolo
    );
    if !s.pinned.is_empty() {
        // Rule O6: the judge sees only the pinned projects, because they are the whole scope.
        let _ = writeln!(
            m,
            "# Projects the user pinned\n\nThe scope is exactly these: {}\n",
            s.pinned
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let _ = writeln!(m, "# Projects in this workspace\n");
    let shown = projects
        .iter()
        .filter(|p| s.pinned.is_empty() || s.project_path(&p.key).is_some());
    for p in shown {
        let _ = writeln!(
            m,
            "- `{}` at {}{}{}",
            p.key,
            p.path,
            p.stack
                .as_ref()
                .map(|s| format!(", stack {s}"))
                .unwrap_or_default(),
            if p.initialized {
                ""
            } else if s.created_projects.iter().any(|c| c.key == p.key) {
                " (created in this session: initialized when the build starts)"
            } else {
                " (not initialized: no pipeline task may target it)"
            }
        );
        if !p.areas.is_empty() {
            let _ = writeln!(m, "  - Areas: {}", p.areas.join("; "));
        }
    }
    let summary = format!("Request: {}", first_line(&request));
    (m, summary)
}

pub fn sufficiency_input(s: &SessionState, subject: Option<&str>) -> (String, String) {
    let mut m = String::new();
    let request = s.full_request();
    let covered: Vec<u32> = subject
        .unwrap_or_default()
        .split(',')
        .filter_map(|x| x.parse().ok())
        .collect();
    let _ = writeln!(
        m,
        "# Request\n\n{request}\n\nCategory: {}\nTrack: {}\nTests requested: {}\nDocs requested: {}\nResearch rounds already judged: {} of {SUFFICIENCY_ROUNDS}\n\n# Research returned\n",
        s.category.map(|c| c.as_str()).unwrap_or("unknown"),
        s.ext
            .os()
            .track
            .map(|t| t.as_str())
            .unwrap_or("not decided yet"),
        yes_no(s.tests_requested()),
        yes_no(s.docs_requested()),
        s.ext.os().sufficiency_rounds,
    );
    for t in &s.ext.os().explore {
        let Some(r) = &t.result else { continue };
        let _ = writeln!(
            m,
            "## Task {} ({})\n\nTask: {}\nScope covered: {}\nFindings: {}\nResearch document: {}\n",
            t.idx, t.project, t.task, r.scope_covered, r.findings_summary, r.research_path
        );
        if covered.contains(&t.idx) && !r.not_covered.is_empty() {
            let _ = writeln!(m, "Not covered:");
            for n in &r.not_covered {
                let _ = writeln!(m, "- {n}");
            }
            m.push('\n');
        }
    }
    let _ = writeln!(
        m,
        "Project keys you may target: {}",
        s.projects
            .iter()
            .map(|p| p.key.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let n: usize = s
        .ext
        .os()
        .explore
        .iter()
        .filter(|t| covered.contains(&t.idx))
        .filter_map(|t| t.result.as_ref())
        .map(|r| r.not_covered.len())
        .sum();
    (
        m,
        format!(
            "{n} Not covered items across {} research documents",
            covered.len()
        ),
    )
}
