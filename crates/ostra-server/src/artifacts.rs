//! Workspace artifacts (HANDOVER 6.5): the files the user keeps for every session of a workspace.
//! Visible ones sit in `<workspace>/.ostra/artifacts/`, which every agent reads. The user hides a
//! folder or a file as one unit; everything within a hidden unit lives in Ostra's data dir, where no
//! agent, harness, or sandboxed command can read it (Rule W2). Every change keeps that invariant:
//! a path is stored in the hidden folder exactly when it is within a hidden unit.

use crate::api::ApiErr;
use crate::files::tree::{self, Contained};
use crate::workspace::WorkspaceRt;
use axum::http::StatusCode;
use ostra_core::api::{SessionStatus, WorkspaceArtifact, WorkspaceArtifacts};
use ostra_core::artifacts::{self, MAX_ARTIFACT_BYTES, MAX_ARTIFACTS};
use std::path::{Path, PathBuf};

fn bad(m: impl Into<String>) -> ApiErr {
    ApiErr::new(StatusCode::BAD_REQUEST, m)
}

fn not_found(rel: &str) -> ApiErr {
    ApiErr::new(
        StatusCode::NOT_FOUND,
        format!("The workspace has no artifact `{rel}`."),
    )
}

fn io(e: std::io::Error) -> ApiErr {
    ApiErr::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("The artifact could not be changed: {e}"),
    )
}

fn visible_root(w: &WorkspaceRt) -> PathBuf {
    artifacts::dir(&w.root)
}

fn hidden_root(w: &WorkspaceRt) -> PathBuf {
    artifacts::hidden_dir(w.id.as_str())
}

/// The folder, created when missing, in its canonical form.
fn ensure(root: &Path) -> Result<PathBuf, ApiErr> {
    std::fs::create_dir_all(root).map_err(io)?;
    ostra_core::paths::canonical(root).map_err(io)
}

fn contained(root: &Path, rel: &str) -> Result<Contained, ApiErr> {
    tree::contain(&ensure(root)?, rel).map_err(|m| ApiErr::new(StatusCode::FORBIDDEN, m))
}

/// Where the artifact at `rel` is now, and whether it is hidden.
fn locate(w: &WorkspaceRt, rel: &str) -> Result<(Contained, bool), ApiErr> {
    for (root, hidden) in [(visible_root(w), false), (hidden_root(w), true)] {
        let file = contained(&root, rel)?;
        if std::fs::symlink_metadata(&file.lexical).is_ok_and(|m| m.is_file()) {
            return Ok((file, hidden));
        }
    }
    Err(not_found(rel))
}

fn exists(w: &WorkspaceRt, rel: &str) -> bool {
    [visible_root(w), hidden_root(w)]
        .iter()
        .any(|r| r.join(rel).exists())
}

/// The hidden units of the workspace. A workspace that hid files before units were recorded has
/// each of its hidden files as a unit.
pub fn hidden_units(w: &WorkspaceRt) -> Vec<String> {
    match std::fs::read(artifacts::hidden_units_file(w.id.as_str())) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => artifacts::list(&hidden_root(w))
            .into_iter()
            .map(|e| e.path)
            .collect(),
    }
}

fn save_units(w: &WorkspaceRt, units: Vec<String>) -> Result<(), ApiErr> {
    let file = artifacts::hidden_units_file(w.id.as_str());
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let json = serde_json::to_vec(&artifacts::tidy_units(units)).map_err(|e| io(e.into()))?;
    std::fs::write(file, json).map_err(io)
}

/// The folder a new file at `rel` goes into: the hidden one when a hidden unit holds it.
pub fn root_for(w: &WorkspaceRt, rel: &str) -> PathBuf {
    if artifacts::is_hidden(&hidden_units(w), rel) {
        hidden_root(w)
    } else {
        visible_root(w)
    }
}

/// Remove empty folders from `file`'s parent up to `root`, so a moved or deleted artifact leaves
/// no empty folder behind.
fn prune(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

fn normalize(raw: &str) -> Result<String, ApiErr> {
    artifacts::normalize(raw).map_err(bad)
}

/// Every file and folder at or under `rel` in `root`, folders before what they hold. Links are not
/// followed.
fn subtree(root: &Path, rel: &str, out: &mut Vec<(String, bool)>) {
    let Ok(meta) = std::fs::symlink_metadata(root.join(rel)) else {
        return;
    };
    if meta.is_file() {
        out.push((rel.to_string(), false));
    } else if meta.is_dir() {
        out.push((rel.to_string(), true));
        let Ok(read) = std::fs::read_dir(root.join(rel)) else {
            return;
        };
        for e in read.flatten() {
            subtree(
                root,
                &format!("{rel}/{}", e.file_name().to_string_lossy()),
                out,
            );
        }
    }
}

/// Move everything at or under `from` to `to` and record `units`, putting each file in the folder
/// its hidden state under `units` asks for. Every target is checked before anything moves.
fn relocate(w: &WorkspaceRt, from: &str, to: &str, units: Vec<String>) -> Result<(), ApiErr> {
    let units = artifacts::tidy_units(units);
    let (visible, hidden) = (ensure(&visible_root(w))?, ensure(&hidden_root(w))?);
    let dest_root = |p: &str| {
        if artifacts::is_hidden(&units, p) {
            &hidden
        } else {
            &visible
        }
    };
    let mut plan = vec![];
    for root in [&visible, &hidden] {
        let mut entries = vec![];
        subtree(root, from, &mut entries);
        for (path, is_dir) in entries {
            let new = format!("{to}{}", &path[from.len()..]);
            let (src, dst) = (root.join(&path), dest_root(&new).join(&new));
            if src == dst {
                continue;
            }
            if !is_dir && std::fs::symlink_metadata(&dst).is_ok() {
                return Err(ApiErr::conflict_on(
                    "path",
                    format!("`{new}` exists already. Move or delete it first."),
                ));
            }
            plan.push((root.clone(), src, dst, is_dir));
        }
    }
    for (_, _, dst, is_dir) in &plan {
        let dir = if *is_dir {
            Some(dst.as_path())
        } else {
            dst.parent()
        };
        if let Some(d) = dir {
            std::fs::create_dir_all(d).map_err(io)?;
        }
    }
    for (_, src, dst, is_dir) in &plan {
        if !*is_dir && std::fs::rename(src, dst).is_err() {
            // The data dir can be on another file system than the workspace.
            std::fs::copy(src, dst).map_err(io)?;
            std::fs::remove_file(src).map_err(io)?;
        }
    }
    // Emptied folders go, deepest first, then the parents `from` leaves empty.
    for (root, src, _, is_dir) in plan.iter().rev() {
        if !*is_dir || std::fs::remove_dir(src).is_ok() {
            prune(root, src);
        }
    }
    save_units(w, units)
}

/// Every artifact, visible and hidden, sorted by path.
pub fn list(w: &WorkspaceRt) -> WorkspaceArtifacts {
    let units = hidden_units(w);
    let mut out: Vec<WorkspaceArtifact> = [visible_root(w), hidden_root(w)]
        .iter()
        .flat_map(|root| artifacts::list(root))
        .map(|e| WorkspaceArtifact {
            hidden: artifacts::is_hidden(&units, &e.path),
            path: e.path,
            size: e.size,
            modified: e.modified,
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    WorkspaceArtifacts {
        dir: visible_root(w),
        artifacts: out,
        delete_blocked: blocked(w, "delete"),
    }
}

/// Rule W4: artifacts are deleted or moved only while no session is running, waiting, stalled, or
/// paused and no execution runs, because an agent may be reading the file. `verb` names the action.
pub fn blocked(w: &WorkspaceRt, verb: &str) -> Option<String> {
    let sessions = w.db.list_sessions().unwrap_or_default();
    if let Some(s) = sessions.iter().find(|s| {
        matches!(
            s.status,
            SessionStatus::Running
                | SessionStatus::Waiting
                | SessionStatus::Stalled
                | SessionStatus::Paused
        )
    }) {
        return Some(format!(
            "Stop session {} or wait for it to end, then {verb}, because its agents may be reading the artifact.",
            s.id
        ));
    }
    w.db.running_executions()
        .unwrap_or_default()
        .first()
        .map(|e| {
            format!(
                "Wait for execution {} to finish, then {verb}, because it may be reading the artifact.",
                e.id
            )
        })
}

/// The bytes and file name of one artifact, for download.
pub fn download(w: &WorkspaceRt, raw: &str) -> Result<(String, Vec<u8>), ApiErr> {
    let rel = normalize(raw)?;
    let (file, _) = locate(w, &rel)?;
    let bytes = std::fs::read(&file.real).map_err(io)?;
    let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
    Ok((name, bytes))
}

pub(crate) fn check_room(w: &WorkspaceRt) -> Result<(), ApiErr> {
    let count = artifacts::list(&visible_root(w)).len() + artifacts::list(&hidden_root(w)).len();
    if count >= MAX_ARTIFACTS {
        return Err(ApiErr::conflict_on(
            "path",
            format!(
                "Delete an artifact first: a workspace holds at most {MAX_ARTIFACTS}, because every agent's brief lists them."
            ),
        ));
    }
    Ok(())
}

/// Add an uploaded file as a new artifact at `raw`: hidden when a hidden folder holds it.
pub fn upload(w: &WorkspaceRt, raw: &str, bytes: &[u8]) -> Result<WorkspaceArtifact, ApiErr> {
    let rel = normalize(raw)?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(bad(format!(
            "Upload artifacts of at most {} MB, because agents read them whole.",
            MAX_ARTIFACT_BYTES / (1024 * 1024)
        )));
    }
    if exists(w, &rel) {
        return Err(ApiErr::conflict_on(
            "path",
            format!("`{rel}` already exists. Delete it first or choose another name."),
        ));
    }
    check_room(w)?;
    let root = root_for(w, &rel);
    let file = contained(&root, &rel)?;
    crate::files::save_file(&file, bytes, None)?;
    Ok(WorkspaceArtifact {
        hidden: root == hidden_root(w),
        path: rel,
        size: bytes.len() as u64,
        modified: Some(chrono::Utc::now()),
    })
}

/// Rule W2: hide or show a folder or file as one unit. Showing something inside a hidden folder is
/// refused, because the folder hides all it holds.
pub fn set_hidden(w: &WorkspaceRt, raw: &str, hidden: bool) -> Result<(), ApiErr> {
    let rel = normalize(raw)?;
    if !exists(w, &rel) {
        return Err(not_found(&rel));
    }
    let units = hidden_units(w);
    let mut next: Vec<String> = units
        .iter()
        .filter(|u| !artifacts::is_within(u, &rel))
        .cloned()
        .collect();
    if hidden {
        if artifacts::is_hidden(&units, &rel) {
            return Ok(());
        }
        next.push(rel.clone());
    } else if !units.contains(&rel) {
        if let Some(folder) = units.iter().find(|u| artifacts::is_within(&rel, u)) {
            return Err(ApiErr::conflict_on(
                "path",
                format!(
                    "Show the folder `{folder}` instead: it is hidden, and a hidden folder hides everything in it."
                ),
            ));
        }
        return Ok(());
    }
    relocate(w, &rel, &rel, next)
}

/// Delete one artifact file, visible or hidden, when Rule W4 allows it.
pub fn delete(w: &WorkspaceRt, raw: &str) -> Result<(), ApiErr> {
    let rel = normalize(raw)?;
    if let Some(why) = blocked(w, "delete") {
        return Err(ApiErr::new(StatusCode::CONFLICT, why));
    }
    let (file, hidden) = locate(w, &rel)?;
    std::fs::remove_file(&file.lexical).map_err(io)?;
    let root = if hidden {
        hidden_root(w)
    } else {
        visible_root(w)
    };
    prune(&ensure(&root)?, &file.lexical);
    let units = hidden_units(w);
    if units.contains(&rel) {
        save_units(w, units.into_iter().filter(|u| *u != rel).collect())?;
    }
    Ok(())
}

/// Move an artifact file or folder. What was hidden stays hidden, and what lands inside a hidden
/// folder becomes hidden. Rule W4 applies, because the old path stops naming a file an agent may be
/// reading.
pub fn move_to(w: &WorkspaceRt, from: &str, to: &str) -> Result<String, ApiErr> {
    let (from, to) = (normalize(from)?, normalize(to)?);
    if from == to {
        return Ok(to);
    }
    if artifacts::is_within(&to, &from) {
        return Err(bad(format!(
            "Move `{from}` somewhere outside itself: `{to}` is inside it."
        )));
    }
    if let Some(why) = blocked(w, "move") {
        return Err(ApiErr::new(StatusCode::CONFLICT, why));
    }
    if !exists(w, &from) {
        return Err(not_found(&from));
    }
    if exists(w, &to) {
        return Err(ApiErr::conflict_on(
            "to",
            format!("`{to}` already exists. Move it elsewhere or rename one of them first."),
        ));
    }
    let units = hidden_units(w);
    let hidden_by_folder = artifacts::is_hidden(&units, &from) && !units.contains(&from);
    let mut next: Vec<String> = units
        .into_iter()
        .map(|u| {
            if artifacts::is_within(&u, &from) {
                format!("{to}{}", &u[from.len()..])
            } else {
                u
            }
        })
        .collect();
    if hidden_by_folder {
        next.push(to.clone());
    }
    relocate(w, &from, &to, next)?;
    Ok(to)
}
