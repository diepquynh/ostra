//! The Stakes judge's input.

#[allow(unused_imports)]
use crate::judge_input::*;
#[allow(unused_imports)]
use crate::prelude::*;
use ostra_engine::state::SessionState;
use std::fmt::Write;
use std::path::Path;

pub fn stakes_input(s: &SessionState) -> (String, String) {
    let mut m = String::new();
    let request = s.full_request();
    let _ = writeln!(m, "# Request\n\n{request}\n");
    if let Some(spec) = &s.ext.os().spec.current {
        let _ = writeln!(
            m,
            "# Approved spec\n\n{} deliverables, {} requirements.\nSummary: {}\n\n{}",
            spec.deliverables,
            spec.requirements,
            spec.summary,
            excerpt(Path::new(&spec.spec_path))
        );
    }
    let _ = writeln!(m, "\nProjects in scope: {}", s.scope.join(", "));
    (m, "Approved spec and request".into())
}
