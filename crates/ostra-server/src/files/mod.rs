//! Project files for the Files dock: folder trees with ignore and git marks, capped file reads,
//! the file name index, per-file diffs, change attribution to session executions, and live
//! `project_fs_changed` messages. The only write is `save`, one file from the browser's editor,
//! checked against the hash the edit started from.

pub mod browse;
pub mod git;
pub mod tree;
pub mod watch;

use crate::api::ApiErr;
use crate::app::{App, Pushed};
use crate::workspace::WorkspaceRt;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use git::GitStatus;
use ostra_core::api::{
    ChangedBy, FileDiff, FileIndex, GitMark, ProjectChange, ProjectFile, ProjectTree,
    ProjectTreeEntry, SaveProjectFile, ServerMsg,
};
use ostra_core::artifacts;
use ostra_core::event::ExecPurpose;
use ostra_core::exec::ExecutionStatus;
use ostra_core::ids::{ExecutionId, WorkspaceId};
use ostra_core::paths;
use ostra_engine::EngineNotice;
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use watch::Touch;

/// Text files are served up to this many bytes.
pub const TEXT_CAP: usize = 1024 * 1024;
/// The file name index holds at most this many paths.
pub const INDEX_CAP: usize = 50_000;
const TREE_CAP: usize = 5_000;
const MAX_DEPTH: u32 = 4;
const DIFF_CAP: usize = 4 * 1024 * 1024;
const CHANGES_CAP: usize = 500;
/// Sessions read for attribution, most recently updated first.
const ATTRIBUTION_SESSIONS: usize = 20;
const MAX_LIVE: usize = 5_000;
const MAX_PUSHED_PATHS: usize = 500;
/// Changes outside Ostra's tools (an editor, a shell) show up within these windows.
const INDEX_TTL: Duration = Duration::from_secs(30);
const GIT_TTL: Duration = Duration::from_secs(5);
const ATTRIBUTION_TTL: Duration = Duration::from_secs(3);
const DEBOUNCE: Duration = Duration::from_millis(150);

pub type ProjectId = (WorkspaceId, String);
/// Project-relative path to the execution that last wrote it, and when.
type LiveWrites = HashMap<String, (ExecutionId, DateTime<Utc>)>;

struct Cached<T> {
    at: Instant,
    root: PathBuf,
    ttl: Duration,
    value: Arc<T>,
}

fn cached<T>(
    map: &Mutex<HashMap<ProjectId, Cached<T>>>,
    id: &ProjectId,
    root: &Path,
) -> Option<Arc<T>> {
    map.lock()
        .get(id)
        .filter(|c| c.root == root && c.at.elapsed() < c.ttl)
        .map(|c| c.value.clone())
}

fn store<T>(
    map: &Mutex<HashMap<ProjectId, Cached<T>>>,
    id: ProjectId,
    root: PathBuf,
    ttl: Duration,
    value: T,
) -> Arc<T> {
    let value = Arc::new(value);
    map.lock().insert(
        id,
        Cached {
            at: Instant::now(),
            root,
            ttl,
            value: value.clone(),
        },
    );
    value
}

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    pub path: Option<String>,
    pub depth: Option<u32>,
    pub hidden: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct DiffQuery {
    pub path: String,
    pub base: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
    pub path: Option<String>,
    pub prefix: Option<String>,
    pub limit: Option<usize>,
}

fn not_found(m: impl Into<String>) -> ApiErr {
    ApiErr::new(StatusCode::NOT_FOUND, m)
}

fn bad(m: impl Into<String>) -> ApiErr {
    ApiErr::new(StatusCode::BAD_REQUEST, m)
}

fn internal(e: impl std::fmt::Display) -> ApiErr {
    ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// The canonical root of a workspace project.
pub(crate) fn project_root(w: &WorkspaceRt, key: &str) -> Result<PathBuf, ApiErr> {
    let path = w
        .project_path(key)
        .ok_or_else(|| not_found(format!("No project `{key}` in this workspace.")))?;
    std::fs::canonicalize(&path).map_err(|_| {
        not_found(format!(
            "The folder of project `{key}` is missing: {}.",
            path.display()
        ))
    })
}

/// The folders the Files view lists under `key`: a project's root, or for the workspace artifacts
/// the visible folder and then the hidden one, created when missing (HANDOVER 6.5).
fn view_roots(w: &WorkspaceRt, key: &str) -> Result<(PathBuf, Option<PathBuf>), ApiErr> {
    if key != artifacts::TAG_ROOT {
        return Ok((project_root(w, key)?, None));
    }
    let make = |p: PathBuf| -> Result<PathBuf, ApiErr> {
        std::fs::create_dir_all(&p).map_err(internal)?;
        p.canonicalize().map_err(internal)
    };
    Ok((
        make(artifacts::dir(&w.root))?,
        Some(make(artifacts::hidden_dir(w.id.as_str()))?),
    ))
}

/// A file under the view's roots: the visible one, else a hidden artifact at the same path.
fn locate_view_file(
    root: &Path,
    hidden: Option<&PathBuf>,
    raw: &str,
) -> Result<tree::Contained, ApiErr> {
    let file = contain(root, raw)?;
    if !file.real.exists()
        && let Some(h) = hidden
    {
        let other = contain(h, raw)?;
        if other.real.exists() {
            return Ok(other);
        }
    }
    Ok(file)
}

/// Rule W2: a new artifact path inside a hidden folder goes into the hidden folder, so what the user
/// adds to a hidden folder is hidden too.
fn within_hidden_unit(
    w: &WorkspaceRt,
    hidden: Option<&PathBuf>,
    target: tree::Contained,
) -> Result<tree::Contained, ApiErr> {
    match hidden {
        Some(h)
            if !target.real.exists()
                && artifacts::is_hidden(&crate::artifacts::hidden_units(w), &target.rel) =>
        {
            contain(h, &target.rel)
        }
        _ => Ok(target),
    }
}

pub(crate) fn contain(root: &Path, raw: &str) -> Result<tree::Contained, ApiErr> {
    tree::contain(root, raw).map_err(|m| ApiErr::new(StatusCode::FORBIDDEN, m))
}

fn phase_of(purpose: Option<&ExecPurpose>) -> (Option<u32>, bool) {
    match purpose {
        Some(
            ExecPurpose::Implement { phase, .. }
            | ExecPurpose::Verify { phase }
            | ExecPurpose::Review {
                phase,
                tests: false,
                ..
            },
        ) => (Some(*phase), false),
        Some(
            ExecPurpose::WriteTest { phase, .. }
            | ExecPurpose::Review {
                phase, tests: true, ..
            },
        ) => (Some(*phase), true),
        _ => (None, false),
    }
}

fn mark_fields(git: &GitStatus, rel: &str) -> (Option<GitMark>, bool) {
    match git.file(rel) {
        Some((m, staged)) => (Some(m), staged),
        None => (None, false),
    }
}

pub struct Files {
    index: Mutex<HashMap<ProjectId, Cached<FileIndex>>>,
    git: Mutex<HashMap<ProjectId, Cached<GitStatus>>>,
    attribution: Mutex<HashMap<ProjectId, Cached<BTreeMap<String, ChangedBy>>>>,
    /// Files running executions wrote, before their submit reports them.
    live: Mutex<HashMap<ProjectId, LiveWrites>>,
    watch: watch::ToolWatch,
    pub browse: browse::BrowseCache,
    touches: mpsc::UnboundedSender<Touch>,
    /// When the user last saved each file from the browser. A save supersedes older attribution.
    user_edits: Mutex<HashMap<ProjectId, HashMap<String, DateTime<Utc>>>>,
    /// Serializes saves, so a hash check and its write are not split by another save.
    saving: tokio::sync::Mutex<()>,
}

impl Files {
    pub fn new(touches: mpsc::UnboundedSender<Touch>) -> Self {
        Files {
            index: Default::default(),
            git: Default::default(),
            attribution: Default::default(),
            live: Default::default(),
            watch: Default::default(),
            browse: Default::default(),
            touches,
            user_edits: Default::default(),
            saving: Default::default(),
        }
    }

    /// Watch one engine notice for native write tool results.
    pub fn observe(&self, workspace: &WorkspaceId, notice: &EngineNotice) {
        if let Some(t) = self.watch.observe(workspace, notice) {
            let _ = self.touches.send(t);
        }
    }

    /// Drop every cached fact about a project, so the next request reads the disk and git again.
    pub fn invalidate(&self, workspace: &WorkspaceId, key: &str) {
        let id = (workspace.clone(), key.to_string());
        self.index.lock().remove(&id);
        self.git.lock().remove(&id);
        self.attribution.lock().remove(&id);
    }

    fn record_live(&self, id: &ProjectId, rel: &str, execution: &ExecutionId) {
        let mut live = self.live.lock();
        let files = live.entry(id.clone()).or_default();
        if files.len() >= MAX_LIVE {
            files.clear();
        }
        files.insert(rel.to_string(), (execution.clone(), Utc::now()));
    }

    /// Every non-ignored file of a project, cached until a write lands or 30 seconds pass. This is
    /// the index "Find a file" and search read.
    pub async fn index(&self, w: &WorkspaceRt, key: &str) -> Result<Arc<FileIndex>, ApiErr> {
        let (root, hidden) = view_roots(w, key)?;
        let id = (w.id.clone(), key.to_string());
        if let Some(v) = cached(&self.index, &id, &root) {
            return Ok(v);
        }
        let walk_root = root.clone();
        let (paths, truncated) = tokio::task::spawn_blocking(move || {
            let (mut paths, truncated) = tree::index(&walk_root, INDEX_CAP);
            if let Some(h) = hidden {
                paths.extend(tree::index(&h, INDEX_CAP).0);
                paths.sort();
                paths.dedup();
            }
            (paths, truncated)
        })
        .await
        .map_err(internal)?;
        Ok(store(
            &self.index,
            id,
            root,
            INDEX_TTL,
            FileIndex { paths, truncated },
        ))
    }

    async fn git_status(&self, w: &WorkspaceRt, key: &str, root: &Path) -> Arc<GitStatus> {
        let id = (w.id.clone(), key.to_string());
        if let Some(v) = cached(&self.git, &id, root) {
            return v;
        }
        let st = git::status(root).await;
        store(&self.git, id, root.to_path_buf(), GIT_TTL, st)
    }

    /// Which session execution last changed each file of a project: finished work passes from the
    /// session fold, then newer writes of running executions.
    pub fn attribution(
        &self,
        w: &WorkspaceRt,
        key: &str,
        root: &Path,
    ) -> Arc<BTreeMap<String, ChangedBy>> {
        let id = (w.id.clone(), key.to_string());
        if let Some(v) = cached(&self.attribution, &id, root) {
            return v;
        }
        let mut map: BTreeMap<String, ChangedBy> = BTreeMap::new();
        let mut sessions: Vec<_> =
            w.db.list_sessions()
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s.projects.iter().any(|p| p == key))
                .collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
        for s in sessions.into_iter().take(ATTRIBUTION_SESSIONS) {
            let Ok(st) = w.engine.state(&s.id) else {
                continue;
            };
            for (path, by) in ostra_engine::view::file_changes(&st, key) {
                if map.get(&path).is_none_or(|c| c.at < by.at) {
                    map.insert(path, by);
                }
            }
        }
        if let Some(edits) = self.user_edits.lock().get(&id) {
            map.retain(|path, by| edits.get(path).is_none_or(|at| *at < by.at));
        }
        let live = self.live.lock().get(&id).cloned().unwrap_or_default();
        let mut views = HashMap::new();
        for (path, (exec, at)) in live {
            if map.get(&path).is_some_and(|c| c.at >= at)
                || self
                    .user_edits
                    .lock()
                    .get(&id)
                    .and_then(|e| e.get(&path))
                    .is_some_and(|u| *u >= at)
            {
                continue;
            }
            let view = views
                .entry(exec.clone())
                .or_insert_with(|| w.db.get_execution(&exec).ok().flatten());
            let Some(v) = view.as_ref() else { continue };
            let Some(session) = v.session.clone() else {
                continue;
            };
            let (phase, tests) = phase_of(v.purpose.as_ref());
            let by = ChangedBy {
                session,
                execution: exec,
                agent: v.agent,
                phase,
                tests,
                staged: false,
                running: v.status == ExecutionStatus::Running,
                at,
            };
            map.insert(path, by);
        }
        store(
            &self.attribution,
            id,
            root.to_path_buf(),
            ATTRIBUTION_TTL,
            map,
        )
    }

    pub async fn tree(
        &self,
        w: &WorkspaceRt,
        key: &str,
        q: TreeQuery,
    ) -> Result<ProjectTree, ApiErr> {
        let (root, hidden_root) = view_roots(w, key)?;
        let raw = q.path.as_deref().unwrap_or("");
        let dir = contain(&root, raw)?;
        let hidden_dir = hidden_root
            .as_ref()
            .map(|h| contain(h, raw))
            .transpose()?
            .filter(|d| d.real.is_dir());
        if !dir.real.exists() && hidden_dir.is_none() {
            return Err(not_found(format!(
                "{} does not exist in project `{key}`.",
                dir.rel
            )));
        }
        let depth = q.depth.unwrap_or(1).clamp(1, MAX_DEPTH);
        let hidden = q.hidden.unwrap_or(false);
        let (list_root, list_dir) = (root.clone(), dir.clone());
        let (listing, concealed) = tokio::task::spawn_blocking(move || {
            let listing = if list_dir.real.exists() {
                tree::list(&list_root, &list_dir, depth, hidden, TREE_CAP)?
            } else {
                tree::Listing {
                    entries: vec![],
                    truncated: false,
                }
            };
            // Rule W2: hidden artifacts show in the tree, marked, so the user can find and unhide them.
            let concealed = match (hidden_root, hidden_dir) {
                (Some(h), Some(d)) => tree::list(&h, &d, depth, hidden, TREE_CAP)?.entries,
                _ => vec![],
            };
            Ok::<_, String>((listing, concealed))
        })
        .await
        .map_err(internal)?
        .map_err(bad)?;
        let git = self.git_status(w, key, &root).await;
        let by = self.attribution(w, key, &root);
        let visible: BTreeSet<String> = listing.entries.iter().map(|e| e.rel.clone()).collect();
        let units = if key == artifacts::TAG_ROOT {
            crate::artifacts::hidden_units(w)
        } else {
            vec![]
        };
        let entries = listing
            .entries
            .into_iter()
            .chain(concealed.into_iter().filter(|e| !visible.contains(&e.rel)))
            .map(|e| (artifacts::is_hidden(&units, &e.rel), e))
            .map(|(hidden_from_agents, e)| {
                let (git_mark, staged, has_changes) = if hidden_from_agents {
                    (None, false, false)
                } else if e.is_dir {
                    let (m, has) = git.dir(&e.rel);
                    (m, false, has)
                } else {
                    let (m, staged) = mark_fields(&git, &e.rel);
                    (m, staged, m.is_some())
                };
                ProjectTreeEntry {
                    changed_by: if e.is_dir {
                        None
                    } else {
                        by.get(&e.rel).cloned()
                    },
                    name: e.name,
                    path: e.rel,
                    is_dir: e.is_dir,
                    is_symlink: e.is_symlink,
                    size: e.size,
                    modified: e.modified,
                    ignored: e.ignored,
                    git: git_mark,
                    staged,
                    has_changes,
                    hidden_from_agents,
                }
            })
            .collect();
        Ok(ProjectTree {
            project: key.to_string(),
            path: dir.rel,
            entries,
            is_git: git.repo,
            truncated: listing.truncated,
        })
    }

    pub async fn file(&self, w: &WorkspaceRt, key: &str, raw: &str) -> Result<ProjectFile, ApiErr> {
        let (root, hidden) = view_roots(w, key)?;
        let file = locate_view_file(&root, hidden.as_ref(), raw)?;
        if !file.real.exists() {
            return Err(not_found(format!(
                "{} does not exist in project `{key}`.",
                file.rel
            )));
        }
        let target = file.clone();
        let read = tokio::task::spawn_blocking(move || tree::read(&target, TEXT_CAP))
            .await
            .map_err(internal)?
            .map_err(bad)?;
        let git = self.git_status(w, key, &root).await;
        let (mark, staged) = mark_fields(&git, &file.rel);
        let changed_by = self.attribution(w, key, &root).get(&file.rel).cloned();
        let read_only = read_only_reason(w, &root, &file, changed_by.as_ref());
        Ok(ProjectFile {
            path: file.rel,
            content: read.content,
            binary: read.binary,
            size: read.size,
            truncated: read.truncated,
            modified: read.modified,
            git: mark,
            staged,
            changed_by,
            hash: read.hash,
            read_only,
        })
    }

    /// Create a folder and its missing parents. Answers its project-relative path and its
    /// parent's listing.
    pub async fn mkdir(
        &self,
        w: &WorkspaceRt,
        key: &str,
        raw: &str,
    ) -> Result<(String, ProjectTree), ApiErr> {
        let (visible, hidden) = view_roots(w, key)?;
        let dir = within_hidden_unit(w, hidden.as_ref(), contain(&visible, raw)?)?;
        let root = if dir.real.starts_with(&visible) {
            visible
        } else {
            hidden.unwrap_or(visible)
        };
        if dir.rel.is_empty() {
            return Err(bad("Name the folder to create with path."));
        }
        if let Some(why) = read_only_reason(w, &root, &dir, None) {
            return Err(ApiErr::new(StatusCode::FORBIDDEN, why));
        }
        if dir.real.exists() {
            return Err(ApiErr::conflict_on(
                "path",
                format!("{} already exists. Choose another name.", dir.rel),
            ));
        }
        std::fs::create_dir_all(&dir.real)
            .map_err(|e| bad(format!("Cannot create {}: {e}", dir.rel)))?;
        self.invalidate(&w.id, key);
        let parent = dir.rel.rsplit_once('/').map_or("", |(p, _)| p).to_string();
        let listing = self
            .tree(
                w,
                key,
                TreeQuery {
                    path: Some(parent),
                    depth: Some(1),
                    hidden: Some(true),
                },
            )
            .await?;
        Ok((dir.rel, listing))
    }

    /// Write one file from the browser when the disk still holds the version the edit started
    /// from. Returns the file as saved. The caller announces the change.
    pub async fn save(
        &self,
        w: &WorkspaceRt,
        key: &str,
        body: SaveProjectFile,
    ) -> Result<ProjectFile, ApiErr> {
        let (root, hidden) = view_roots(w, key)?;
        let file = locate_view_file(&root, hidden.as_ref(), &body.path)?;
        let file = within_hidden_unit(w, hidden.as_ref(), file)?;
        if file.rel.is_empty() || file.real.is_dir() {
            return Err(bad("Name a file with path. Folders cannot be saved."));
        }
        if key == artifacts::TAG_ROOT && !file.real.exists() {
            artifacts::normalize(&file.rel).map_err(bad)?;
            crate::artifacts::check_room(w)?;
        }
        if body.content.len() > TEXT_CAP {
            return Err(bad(format!(
                "The text is larger than {} MB, the size Ostra edits in the browser.",
                TEXT_CAP / (1024 * 1024)
            )));
        }
        let changed_by = self.attribution(w, key, &root).get(&file.rel).cloned();
        if let Some(why) = read_only_reason(w, &root, &file, changed_by.as_ref()) {
            let status = if changed_by.is_some_and(|c| c.running) {
                StatusCode::CONFLICT
            } else {
                StatusCode::FORBIDDEN
            };
            return Err(ApiErr::new(status, why));
        }
        let _guard = self.saving.lock().await;
        let target = file.clone();
        let content = body.content.into_bytes();
        let base = body.base_hash;
        tokio::task::spawn_blocking(move || save_file(&target, &content, base.as_deref()))
            .await
            .map_err(internal)??;
        let id = (w.id.clone(), key.to_string());
        self.user_edits
            .lock()
            .entry(id.clone())
            .or_default()
            .insert(file.rel.clone(), Utc::now());
        self.invalidate(&id.0, &id.1);
        drop(_guard);
        self.file(w, key, &file.rel).await
    }

    pub async fn diff(&self, w: &WorkspaceRt, key: &str, q: DiffQuery) -> Result<FileDiff, ApiErr> {
        let base = q
            .base
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .unwrap_or("HEAD")
            .to_string();
        if key == artifacts::TAG_ROOT {
            // Workspace artifacts have no history to compare with.
            return Ok(FileDiff {
                path: q.path,
                base,
                hunks: vec![],
                added: 0,
                removed: 0,
                binary: false,
                truncated: false,
                git: None,
                changed_by: None,
            });
        }
        if !git::valid_base(&base) {
            return Err(bad(
                "Use a commit, branch, or tag name for base, such as HEAD or main.",
            ));
        }
        let root = project_root(w, key)?;
        let file = contain(&root, &q.path)?;
        if file.rel.is_empty() || file.real.is_dir() {
            return Err(bad("Name a file with path. Folders have no diff."));
        }
        let git = self.git_status(w, key, &root).await;
        let (mark, _) = mark_fields(&git, &file.rel);
        let changed_by = self.attribution(w, key, &root).get(&file.rel).cloned();
        let mut out = FileDiff {
            path: file.rel.clone(),
            base: base.clone(),
            hunks: vec![],
            added: 0,
            removed: 0,
            binary: false,
            truncated: false,
            git: mark,
            changed_by,
        };
        if !git.repo {
            return Ok(out);
        }
        let untracked = mark == Some(GitMark::Untracked);
        let commit = if untracked {
            String::new()
        } else {
            git::resolve_base(&root, &base).await.map_err(bad)?
        };
        if let Some((d, truncated)) =
            git::diff_file(&root, &file.rel, &commit, untracked, DIFF_CAP).await
        {
            out.hunks = d.hunks;
            out.added = d.added;
            out.removed = d.removed;
            out.binary = d.binary;
            out.truncated = truncated;
        }
        Ok(out)
    }

    /// Files sessions changed that still differ from HEAD, with line counts.
    pub async fn changes(&self, w: &WorkspaceRt, key: &str) -> Result<Vec<ProjectChange>, ApiErr> {
        if key == artifacts::TAG_ROOT {
            return Ok(vec![]);
        }
        let root = project_root(w, key)?;
        let git = self.git_status(w, key, &root).await;
        let by = self.attribution(w, key, &root);
        let rows: Vec<(String, ChangedBy, Option<GitMark>, bool)> = by
            .iter()
            .filter_map(|(path, c)| {
                let (mark, staged) = mark_fields(&git, path);
                (!git.repo || mark.is_some()).then(|| (path.clone(), c.clone(), mark, staged))
            })
            .take(CHANGES_CAP)
            .collect();
        let mut counts = BTreeMap::new();
        if git.repo {
            let tracked: Vec<String> = rows
                .iter()
                .filter(|r| r.2 != Some(GitMark::Untracked))
                .map(|r| r.0.clone())
                .collect();
            if !tracked.is_empty()
                && let Ok(commit) = git::resolve_base(&root, "HEAD").await
            {
                counts = git::numstat(&root, &commit, &tracked, &git.prefix).await;
            }
            for (path, _, mark, _) in &rows {
                if *mark == Some(GitMark::Untracked)
                    && let Ok(f) = tree::contain(&root, path)
                {
                    let lines = tokio::task::spawn_blocking(move || {
                        tree::read(&f, TEXT_CAP)
                            .ok()
                            .and_then(|r| r.content)
                            .map(|c| c.lines().count() as u32)
                    })
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(0);
                    counts.insert(path.clone(), (lines, 0));
                }
            }
        }
        Ok(rows
            .into_iter()
            .map(|(path, changed_by, git, staged)| {
                let (added, removed) = counts.get(&path).copied().unwrap_or((0, 0));
                ProjectChange {
                    path,
                    git,
                    staged,
                    added,
                    removed,
                    changed_by,
                }
            })
            .collect())
    }
}

/// Why the browser may not write this file, or `None`.
fn read_only_reason(
    w: &WorkspaceRt,
    root: &Path,
    file: &tree::Contained,
    changed_by: Option<&ChangedBy>,
) -> Option<String> {
    if file.rel.split('/').any(|s| s == ".git") {
        return Some("Files under .git are read-only here. Use git to change them.".into());
    }
    // `file.real` is canonical, and `is_inside` compares lexically.
    let runtime = paths::workspace_runtime(&w.root);
    let runtime = std::fs::canonicalize(&runtime).unwrap_or(runtime);
    let memory = paths::project_runtime(root).join("memory");
    let artifacts = artifacts::dir(&w.root);
    let artifacts = std::fs::canonicalize(&artifacts).unwrap_or(artifacts);
    let is_artifact = paths::is_inside(&artifacts, &file.real);
    if (paths::is_inside(&runtime, &file.real) && !is_artifact)
        || paths::is_inside(&memory, &file.real)
    {
        return Some(
            "Ostra's own state is read-only here. Change workspace settings on the Settings screen, because a direct edit skips their validation.".into(),
        );
    }
    changed_by.filter(|c| c.running).map(|_| {
        "A running execution is writing this file. Edit it after the execution ends, so neither change overwrites the other.".into()
    })
}

/// Check the base hash against the disk and write the new text atomically.
pub(crate) fn save_file(
    file: &tree::Contained,
    content: &[u8],
    base: Option<&str>,
) -> Result<(), ApiErr> {
    use std::io::Write;
    let changed = "The file changed on disk after you opened it. Reload it and apply your change again, or save over it.";
    let current = match std::fs::read(&file.real) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(bad(format!("Cannot read {}: {e}", file.rel))),
    };
    let perms = match (&current, base) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(ApiErr::conflict_on(
                "base_hash",
                "The file was deleted after you opened it. Save it again as a new file to create it.",
            ));
        }
        (Some(_), None) => {
            return Err(ApiErr::conflict_on(
                "base_hash",
                format!(
                    "{} already exists. Open it and edit that version.",
                    file.rel
                ),
            ));
        }
        (Some(bytes), Some(b)) => {
            if tree::sha256_hex(bytes) != b {
                return Err(ApiErr::conflict_on("base_hash", changed));
            }
            std::fs::metadata(&file.real).ok().map(|m| m.permissions())
        }
    };
    let dir = file
        .real
        .parent()
        .ok_or_else(|| bad("The file has no folder."))?;
    std::fs::create_dir_all(dir).map_err(|e| bad(format!("Cannot create the folder: {e}")))?;
    let write = || -> std::io::Result<()> {
        let mut tmp = tempfile::Builder::new()
            .prefix(".ostra-save-")
            .tempfile_in(dir)?;
        tmp.write_all(content)?;
        tmp.as_file().sync_all()?;
        match perms {
            Some(p) => tmp.as_file().set_permissions(p)?,
            // A temp file starts at 0600; a new project file gets the usual 0644.
            #[cfg(unix)]
            None => {
                use std::os::unix::fs::PermissionsExt;
                tmp.as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o644))?
            }
            #[cfg(not(unix))]
            None => {}
        }
        tmp.persist(&file.real).map_err(|e| e.error)?;
        Ok(())
    };
    write().map_err(|e| internal(format!("Cannot write {}: {e}", file.rel)))
}

/// Map an absolute path to the workspace project holding it: the deepest project root wins.
fn locate(workspaces: &[Arc<WorkspaceRt>], real: &Path) -> Option<(ProjectId, String)> {
    let mut best: Option<(usize, ProjectId, String)> = None;
    for w in workspaces {
        for p in w.settings().projects {
            let Ok(root) = std::fs::canonicalize(&p.path) else {
                continue;
            };
            if !paths::is_inside(&root, real) {
                continue;
            }
            let Ok(rel) = real.strip_prefix(&root) else {
                continue;
            };
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let len = root.as_os_str().len();
            if !rel.is_empty() && best.as_ref().is_none_or(|b| b.0 < len) {
                best = Some((len, (w.id.clone(), p.key.clone()), rel));
            }
        }
    }
    best.map(|(_, id, rel)| (id, rel))
}

fn execution_root(workspaces: &[Arc<WorkspaceRt>], execution: &ExecutionId) -> Option<PathBuf> {
    workspaces.iter().find_map(|w| {
        w.db.get_execution(execution)
            .ok()
            .flatten()
            .and_then(|v| w.project_path(&v.project))
    })
}

fn absorb(app: &App, t: Touch, batch: &mut BTreeMap<ProjectId, BTreeSet<String>>) {
    let workspaces = match &t.workspace {
        Some(id) => app.workspace(id).into_iter().collect(),
        None => app.all_workspaces(),
    };
    let base = t
        .base
        .clone()
        .or_else(|| execution_root(&workspaces, &t.execution));
    for raw in &t.paths {
        let abs = if raw.is_absolute() {
            raw.clone()
        } else {
            match &base {
                Some(b) => b.join(raw),
                None => continue,
            }
        };
        let real = paths::resolve(Path::new("/"), &abs);
        let Some((id, rel)) = locate(&workspaces, &real) else {
            continue;
        };
        app.files.invalidate(&id.0, &id.1);
        app.code.touch(&id, &rel);
        app.files.record_live(&id, &rel, &t.execution);
        batch.entry(id).or_default().insert(rel);
    }
}

/// Tell the browsers that git changed the project: marks, and files too after a pull or checkout.
/// An empty path list reloads every listing a browser holds.
pub fn announce_git(app: &App, workspace: &WorkspaceId, key: &str) {
    app.files.invalidate(workspace, key);
    let _ = app.push.send(Pushed {
        channels: vec![format!("workspace:{workspace}")],
        msg: ServerMsg::ProjectFsChanged {
            workspace: workspace.clone(),
            key: key.to_string(),
            paths: vec![],
        },
    });
}

/// Tell the browsers and the code index that the user saved a file.
pub fn announce_save(app: &App, workspace: &WorkspaceId, key: &str, rel: &str) {
    app.code.touch(&(workspace.clone(), key.to_string()), rel);
    let _ = app.push.send(Pushed {
        channels: vec![format!("workspace:{workspace}")],
        msg: ServerMsg::ProjectFsChanged {
            workspace: workspace.clone(),
            key: key.to_string(),
            paths: vec![rel.to_string()],
        },
    });
}

/// Turn write touches into cache invalidation and `project_fs_changed` messages, coalescing the
/// touches that arrive within a short window into one message per project.
pub async fn run_touches(app: Weak<App>, mut rx: mpsc::UnboundedReceiver<Touch>) {
    while let Some(first) = rx.recv().await {
        let Some(app) = app.upgrade() else { break };
        let mut batch = BTreeMap::new();
        absorb(&app, first, &mut batch);
        let deadline = tokio::time::Instant::now() + DEBOUNCE;
        while let Ok(Some(t)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            absorb(&app, t, &mut batch);
        }
        for ((workspace, key), paths) in batch {
            let msg = ServerMsg::ProjectFsChanged {
                workspace: workspace.clone(),
                key,
                paths: paths.into_iter().take(MAX_PUSHED_PATHS).collect(),
            };
            let _ = app.push.send(Pushed {
                channels: vec![format!("workspace:{workspace}")],
                msg,
            });
        }
    }
}
