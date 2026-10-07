//! Run labels and the artifacts list, which each stage adds its own artifacts to.

use crate::prelude::*;
use crate::stages::{book, build, plan, research, spec};
use ostra_core::agent::AgentName;
use ostra_core::api::{ArtifactRef, ExecutionView, PendingGate};
use ostra_core::event::numbered_run_label;
use ostra_core::executor::ExecStream;
use ostra_core::ids::ExecutionId;
use ostra_core::paths;
use ostra_engine::state::{ExecRecord, SessionState};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// The session's artifacts, each path once, in the order the stages add them.
#[derive(Default)]
pub(crate) struct Artifacts {
    out: Vec<ArtifactRef>,
    seen: HashSet<PathBuf>,
}

impl Artifacts {
    pub(crate) fn add(
        &mut self,
        path: PathBuf,
        kind: &str,
        label: String,
        project: Option<String>,
    ) {
        if self.seen.insert(path.clone()) {
            self.out.push(ArtifactRef {
                path,
                kind: kind.into(),
                label,
                project,
            });
        }
    }
}

pub fn artifacts(s: &SessionState) -> Vec<ArtifactRef> {
    let mut a = Artifacts::default();
    // Rule C3: uploads are artifacts of the session, so they can be opened and downloaded.
    for u in s
        .uploads
        .iter()
        .chain(s.amendments.iter().flat_map(|a| a.uploads.iter()))
    {
        a.add(
            u.path.clone(),
            "upload",
            format!("Upload: {}", u.name),
            None,
        );
    }
    research::view::artifacts(s, &mut a);
    spec::view::artifacts(s, &mut a);
    plan::view::artifacts(s, &mut a);
    build::view::artifacts(s, &mut a);
    book::view::artifacts(s, &mut a);
    if let Some((path, _)) = &s.completed {
        a.add(path.clone(), "completion", "Completion report".into(), None);
    }
    a.out
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
