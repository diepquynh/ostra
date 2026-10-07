//! Run labels, the artifacts list, and the changed files of each run.

use super::*;
#[allow(unused_imports)]
use crate::book::DocsTrack;
use crate::data::EpaState;
use crate::prelude::*;
use ostra_core::agent::AgentName;
use ostra_core::api::{ArtifactRef, ChangedBy, ExecutionView, PendingGate};
use ostra_core::event::numbered_run_label;
use ostra_core::executor::ExecStream;
use ostra_core::ids::ExecutionId;
use ostra_core::paths;
use ostra_engine::state::{ExecRecord, SessionState};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

pub fn artifacts(s: &SessionState) -> Vec<ArtifactRef> {
    let mut out = vec![];
    let mut seen = HashSet::new();
    let mut add = |path: PathBuf, kind: &str, label: String, project: Option<String>| {
        if seen.insert(path.clone()) {
            out.push(ArtifactRef {
                path,
                kind: kind.into(),
                label,
                project,
            });
        }
    };
    // Rule C3: uploads are artifacts of the session, so they can be opened and downloaded.
    for u in s
        .uploads
        .iter()
        .chain(s.amendments.iter().flat_map(|a| a.uploads.iter()))
    {
        add(
            u.path.clone(),
            "upload",
            format!("Upload: {}", u.name),
            None,
        );
    }
    for t in &s.ext.os().explore {
        if let Some(r) = &t.result {
            add(
                PathBuf::from(&r.research_path),
                "research",
                format!("Research: {}", first(&t.task)),
                Some(t.project.clone()),
            );
        }
    }
    if let Some(spec) = &s.ext.os().spec.current {
        add(PathBuf::from(&spec.spec_path), "spec", "Spec".into(), None);
    }
    if let Some(plan) = &s.ext.os().plan.current {
        add(
            PathBuf::from(&plan.master_plan_path),
            "plan",
            "Master plan".into(),
            None,
        );
        for p in &plan.phases {
            add(
                PathBuf::from(&p.file),
                "phase",
                format!("Phase {}: {}", p.id, p.title),
                Some(p.project.clone()),
            );
        }
    }
    for p in s.ext.os().phases.values() {
        if let Some(r) = &p.implementer_report {
            add(
                r.clone(),
                "report",
                format!("Implementer report, phase {}", p.info.id),
                Some(p.info.project.clone()),
            );
        }
        for tests in [false, true] {
            let ledger = s.ledger_path(&p.info.project, p.info.id, tests);
            if ledger.exists() {
                add(
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
        if let EpaState::Done(path) = &p.epa {
            add(
                path.clone(),
                "report",
                format!("EPA report, phase {}", p.info.id),
                Some(p.info.project.clone()),
            );
        }
        if let Some(r) = &p.test_loop.report {
            add(
                r.clone(),
                "report",
                format!("Test report, phase {}", p.info.id),
                Some(p.info.project.clone()),
            );
        }
    }
    if let Some(w) = s.book_written.as_ref().filter(|w| w.error.is_none()) {
        add(
            ostra_core::book::book_dir(&s.workspace_root, &w.book).join("index.md"),
            "book",
            "Documentation book".into(),
            None,
        );
    }
    if let Some((path, _)) = &s.completed {
        add(path.clone(), "completion", "Completion report".into(), None);
    }
    out
}

/// Each execution's run label, numbered when the same agent already ran the same label on the
/// same project in this session.
pub fn run_labels(s: &SessionState) -> HashMap<ExecutionId, String> {
    let mut runs: Vec<&ExecRecord> = s.executions.values().collect();
    runs.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut seen: HashMap<(AgentName, &str, String), u32> = HashMap::new();
    runs.into_iter()
        .map(|r| {
            let base = r.purpose.run_label();
            let n = seen
                .entry((r.agent, r.project.as_str(), base.clone()))
                .or_default();
            *n += 1;
            (r.id.clone(), numbered_run_label(&base, *n))
        })
        .collect()
}

/// Fill the fields of an execution view that come from the fold and the session dir.
pub fn decorate(s: &SessionState, labels: &HashMap<ExecutionId, String>, e: &mut ExecutionView) {
    if let Some(label) = labels.get(&e.id) {
        e.run_label = label.clone();
    }
    e.has_transcript = e.stream == ExecStream::Terminal
        && paths::terminal_transcript(&s.session_root, e.id.as_str()).is_file();
    e.repo_root = s.project_path(&e.project);
    e.can_skip = s.can_skip(&e.id);
    e.can_steer = s.can_steer(&e.id);
    e.queued_steer = s
        .steer_queued(&e.id)
        .then(|| s.steers.get(&e.id).map(|x| x.text.clone()))
        .flatten();
    e.pending_gate = s
        .open_gates()
        .filter(|g| g.payload.execution() == Some(&e.id))
        .max_by(|a, b| a.opened_at.cmp(&b.opened_at).then_with(|| a.id.cmp(&b.id)))
        .map(|g| PendingGate {
            id: g.id.clone(),
            kind: g.payload.kind_str().to_string(),
            title: g.title.clone(),
        });
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
