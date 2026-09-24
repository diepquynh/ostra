//! Navigation for the redesigned console: the Sessions tree with `tree_patch` pushes, search
//! across the workspace, and the workspace activity summary with `activity` pushes.
//!
//! Engine notices mark sessions and executions dirty; a ticker rebuilds the dirty nodes at most
//! four times a second and pushes the ones that changed. The same nodes serve `GET .../tree`, and
//! building a node indexes its artifacts' headings for search, skipping files whose mtime is
//! unchanged.

pub mod activity;
pub mod search;

use crate::api::ApiErr;
use crate::app::{App, Pushed};
use crate::workspace::WorkspaceRt;
use ostra_core::api::{ServerMsg, TreeSession, WorkspaceActivity, WorkspaceTree};
use ostra_core::exec::ExecutionDelta;
use ostra_core::ids::{ExecutionId, SessionId, WorkspaceId};
use ostra_engine::EngineNotice;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Weak;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// How often dirty tree nodes are rebuilt and pushed: at most four patches a second per session.
const TREE_TICK: Duration = Duration::from_millis(250);
/// At most two `activity` messages a second per workspace.
const ACTIVITY_GAP: Duration = Duration::from_millis(500);
/// Artifacts larger than this are indexed by label only.
const ARTIFACT_INDEX_CAP: u64 = 4 * 1024 * 1024;

#[derive(Default)]
struct WsNav {
    /// The last built node of each session: the `GET .../tree` cache and the base `tree_patch`
    /// compares against.
    nodes: HashMap<SessionId, TreeSession>,
    dirty: HashSet<SessionId>,
    dirty_execs: HashSet<ExecutionId>,
    /// Notices were lost, so every session is rebuilt on the next tick.
    all_dirty: bool,
    activity_dirty: bool,
    activity: Option<WorkspaceActivity>,
    activity_at: Option<Instant>,
}

#[derive(Default)]
pub struct Nav {
    workspaces: Mutex<HashMap<WorkspaceId, WsNav>>,
}

fn workspace_channel(id: &WorkspaceId) -> Vec<String> {
    vec![format!("workspace:{id}")]
}

impl Nav {
    /// Record what one engine notice changed.
    pub fn observe(&self, workspace: &WorkspaceId, notice: &EngineNotice) {
        let mut all = self.workspaces.lock();
        let w = all.entry(workspace.clone()).or_default();
        match notice {
            EngineNotice::Event { session, .. } => {
                w.dirty.insert(session.clone());
                w.activity_dirty = true;
            }
            EngineNotice::SessionUpdated { summary } => {
                w.dirty.insert(summary.id.clone());
                w.activity_dirty = true;
            }
            EngineNotice::ExecutionStatus { execution, .. } => {
                w.dirty_execs.insert(execution.clone());
                w.activity_dirty = true;
            }
            // Tool calls and status messages change the execution's summary line.
            EngineNotice::Delta { execution, item } => {
                if matches!(item.delta, ExecutionDelta::ToolCall { .. } | ExecutionDelta::Status { .. }) {
                    w.dirty_execs.insert(execution.clone());
                    w.activity_dirty = true;
                }
            }
            EngineNotice::ProjectsChanged => {}
        }
    }

    /// Notices were dropped: rebuild everything on the next tick.
    pub fn lagged(&self, workspace: &WorkspaceId) {
        let mut all = self.workspaces.lock();
        let w = all.entry(workspace.clone()).or_default();
        w.all_dirty = true;
        w.activity_dirty = true;
    }

    /// Every session node, most recently updated first.
    pub fn tree(&self, w: &WorkspaceRt) -> Result<WorkspaceTree, ApiErr> {
        let summaries = w.db.list_sessions()?;
        let (mut cached, dirty) = {
            let all = self.workspaces.lock();
            all.get(&w.id).filter(|n| !n.all_dirty).map(|n| (n.nodes.clone(), n.dirty.clone())).unwrap_or_default()
        };
        let mut fresh = vec![];
        let mut sessions = Vec::with_capacity(summaries.len());
        for s in &summaries {
            match cached.remove(&s.id) {
                Some(node) if !dirty.contains(&s.id) => sessions.push(node),
                old => {
                    let Ok(node) = w.engine.tree_session(s) else { continue };
                    index_artifacts(w, &node);
                    // A dirty node stays unstored, so the next tick still pushes it.
                    if old.is_none() && !dirty.contains(&s.id) {
                        fresh.push(node.clone());
                    }
                    sessions.push(node);
                }
            }
        }
        if !fresh.is_empty() {
            let mut all = self.workspaces.lock();
            let n = all.entry(w.id.clone()).or_default();
            for node in fresh {
                n.nodes.entry(node.id.clone()).or_insert(node);
            }
        }
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| b.id.cmp(&a.id)));
        Ok(WorkspaceTree { sessions })
    }

    /// Rebuild dirty nodes and the activity summary of one workspace, and push what changed.
    pub fn flush(&self, app: &App, w: &WorkspaceRt) {
        let (sessions, execs, all_dirty, activity_due) = {
            let mut all = self.workspaces.lock();
            let Some(n) = all.get_mut(&w.id) else { return };
            let due = n.activity_dirty && n.activity_at.is_none_or(|t| t.elapsed() >= ACTIVITY_GAP);
            if due {
                n.activity_dirty = false;
            }
            let nodes = &n.nodes;
            let execs: Vec<(ExecutionId, Option<SessionId>)> = n
                .dirty_execs
                .drain()
                .map(|e| {
                    let s = session_of_cached(nodes, &e);
                    (e, s)
                })
                .collect();
            (std::mem::take(&mut n.dirty), execs, std::mem::take(&mut n.all_dirty), due)
        };
        let mut targets: HashSet<SessionId> = sessions;
        for (exec, cached) in execs {
            let session = cached.or_else(|| w.db.get_execution(&exec).ok().flatten().and_then(|v| v.session));
            targets.extend(session);
        }
        let summaries = if all_dirty {
            w.db.list_sessions().unwrap_or_default()
        } else {
            targets.iter().filter_map(|id| w.db.get_session(id).ok().flatten()).collect()
        };
        for s in summaries {
            let Ok(node) = w.engine.tree_session(&s) else { continue };
            index_artifacts(w, &node);
            let changed = {
                let mut all = self.workspaces.lock();
                let n = all.entry(w.id.clone()).or_default();
                if n.nodes.get(&s.id) == Some(&node) {
                    false
                } else {
                    n.nodes.insert(s.id.clone(), node.clone());
                    true
                }
            };
            if changed {
                let msg = ServerMsg::TreePatch { workspace: w.id.clone(), session: node };
                let _ = app.push.send(Pushed { channels: workspace_channel(&w.id), msg });
            }
        }
        if activity_due {
            let Ok(activity) = activity::build(w, chrono::Local::now()) else { return };
            let changed = {
                let mut all = self.workspaces.lock();
                let n = all.entry(w.id.clone()).or_default();
                n.activity_at = Some(Instant::now());
                if n.activity.as_ref() == Some(&activity) {
                    false
                } else {
                    n.activity = Some(activity.clone());
                    true
                }
            };
            if changed {
                let msg = ServerMsg::Activity { workspace: w.id.clone(), activity };
                let _ = app.push.send(Pushed { channels: workspace_channel(&w.id), msg });
            }
        }
    }
}

fn session_of_cached(nodes: &HashMap<SessionId, TreeSession>, exec: &ExecutionId) -> Option<SessionId> {
    nodes.values().find(|n| n.groups.iter().any(|g| g.runs.iter().any(|r| &r.id == exec))).map(|n| n.id.clone())
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Index each artifact's label, file name, and headings for search, skipping files whose label and
/// mtime match the index. Artifacts are written by agents through their tools, so the index
/// catches up whenever the session's node is rebuilt.
pub fn index_artifacts(w: &WorkspaceRt, node: &TreeSession) {
    if node.artifacts.is_empty() {
        return;
    }
    let indexed = w.db.indexed_artifacts(&node.id).unwrap_or_default();
    for a in &node.artifacts {
        if !a.path.is_absolute() {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&a.path) else { continue };
        let mtime = mtime_ms(&meta);
        let path = a.path.to_string_lossy().to_string();
        if indexed.get(&path).is_some_and(|(label, m)| *label == a.label && *m == mtime) {
            continue;
        }
        let mut body = a.path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        if meta.len() <= ARTIFACT_INDEX_CAP
            && let Ok(text) = std::fs::read_to_string(&a.path)
        {
            for h in ostra_core::outline::headings(&text) {
                body.push('\n');
                body.push_str(&h.title);
            }
        }
        if let Err(e) = w.db.index_artifact(&node.id, &path, &a.label, &body, mtime) {
            tracing::warn!("indexing {path} for search: {e}");
        }
    }
}

/// Rebuild dirty tree nodes and activity summaries until the app is gone.
pub async fn run(app: Weak<App>) {
    let mut tick = tokio::time::interval(TREE_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let Some(app) = app.upgrade() else { break };
        let done = tokio::task::spawn_blocking(move || {
            for w in app.all_workspaces() {
                app.nav.flush(&app, &w);
            }
        })
        .await;
        if let Err(e) = done {
            tracing::error!("navigation flush failed: {e}");
        }
    }
}
