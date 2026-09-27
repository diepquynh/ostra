//! Git repositories inside a sandbox's writable roots: finding them, the paths in each git dir
//! that choose programs git runs, and undoing what an agent plants there.

use crate::profile::real;
use std::path::{Path, PathBuf};

/// Paths inside a `.git` dir that choose programs git runs. A trailing `/` marks a dir.
/// `config.worktree` is read only with `extensions.worktreeConfig`, which `config` sets, so the
/// empty placeholder a missing one gets is harmless. `commondir` cannot have one: git refuses to
/// open a repository whose `commondir` is empty; [`repair_git_dirs`] covers it instead.
pub(crate) const GIT_PROTECTED: &[&str] = &["config", "config.worktree", "hooks/", "info/"];

/// A `.git` dir and each submodule git dir under its `modules/`.
pub(crate) fn git_dirs(git: &Path) -> Vec<PathBuf> {
    let mut out = vec![git.to_path_buf()];
    let mut i = 0;
    while i < out.len() && out.len() < 256 {
        if let Ok(rd) = std::fs::read_dir(out[i].join("modules")) {
            for e in rd.flatten() {
                let p = e.path();
                if p.join("config").is_file() {
                    out.push(p);
                } else if p.is_dir() {
                    // Nested module names keep their slashes as dirs.
                    if let Ok(inner) = std::fs::read_dir(&p) {
                        out.extend(
                            inner
                                .flatten()
                                .map(|e| e.path())
                                .filter(|q| q.join("config").is_file()),
                        );
                    }
                }
            }
        }
        i += 1;
    }
    out
}

/// Undoes what an agent can write inside a repository's `.git` that makes git load another
/// repository's config, and so run its `core.fsmonitor`, `credential.helper`, or hooks: a
/// `commondir` in a repository's own git dir (which git never writes there) is removed, and a
/// linked worktree's `commondir` that no longer names `../..` is restored. Takes the repos
/// [`git_repos`] found, the ones a profile protects. Returns the paths it changed.
pub fn repair_git_dirs(repos: &[PathBuf]) -> Vec<PathBuf> {
    let mut changed = vec![];
    for repo in repos {
        let Some(git) = GitDir::of(repo) else {
            continue;
        };
        if git.linked {
            continue;
        }
        for dir in git_dirs(&git.dir) {
            let own = dir.join("commondir");
            if let Ok(meta) = std::fs::symlink_metadata(&own) {
                let removed = if meta.is_dir() {
                    std::fs::remove_dir_all(&own)
                } else {
                    std::fs::remove_file(&own)
                };
                if removed.is_ok() {
                    changed.push(own);
                }
            }
            for w in read_dirs(&dir.join("worktrees")) {
                let common = w.join("commondir");
                let Ok(meta) = std::fs::symlink_metadata(&common) else {
                    continue;
                };
                let intact = meta.is_file()
                    && std::fs::read_to_string(&common).is_ok_and(|t| t.trim() == "../..");
                if intact {
                    continue;
                }
                if meta.is_dir() {
                    let _ = std::fs::remove_dir_all(&common);
                } else {
                    let _ = std::fs::remove_file(&common);
                }
                if std::fs::write(&common, "../..\n").is_ok() {
                    changed.push(common);
                }
            }
        }
    }
    changed
}

/// The correction an agent gets for each path [`repair_git_dirs`] changed.
pub fn git_repair_note(path: &Path) -> String {
    format!(
        "Leave `{}` to git: Ostra undid it, because a `commondir` makes git load another repository's config and run the programs it names.",
        path.display()
    )
}

/// Dirs [`git_repos`] reads before it stops, which bounds the walk in a large tree.
const GIT_WALK_DIRS: usize = 20_000;
/// Repos [`git_repos`] returns at most, which bounds the mounts they add.
const GIT_WALK_REPOS: usize = 128;

/// Every git work tree under `roots` at any depth, shallowest first: a dir holding a `.git` dir
/// or a `.git` file. Symlinks, `node_modules`, and dirs tagged as caches (`CACHEDIR.TAG`, as
/// Cargo tags `target/`) are not entered, and the walk stops after [`GIT_WALK_DIRS`] dirs or
/// [`GIT_WALK_REPOS`] repos.
pub fn git_repos(roots: &[&Path]) -> Vec<PathBuf> {
    let mut queue: std::collections::VecDeque<PathBuf> = roots
        .iter()
        .filter(|r| !r.as_os_str().is_empty())
        .filter_map(|r| real(r))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    while let Some(dir) = queue.pop_front() {
        if !seen.insert(dir.clone()) {
            continue;
        }
        if seen.len() > GIT_WALK_DIRS || out.len() >= GIT_WALK_REPOS {
            tracing::warn!(
                "stopped looking for git repositories at {}: repositories below it keep a writable .git/config",
                dir.display()
            );
            break;
        }
        if dir.join("CACHEDIR.TAG").is_file() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut subdirs = vec![];
        for e in rd.flatten() {
            let name = e.file_name();
            if name == ".git" {
                let git = e.path();
                if (git.is_dir() || git.is_file()) && !out.contains(&dir) {
                    out.push(dir.clone());
                }
            } else if name != "node_modules" && e.file_type().is_ok_and(|t| t.is_dir()) {
                subdirs.push(e.path());
            }
        }
        subdirs.sort();
        queue.extend(subdirs);
    }
    out
}

/// A work tree's git dir: its `.git` dir, or the dir its `.git` file names.
pub(crate) struct GitDir {
    pub(crate) dir: PathBuf,
    /// Named by a `.git` file.
    pub(crate) file: bool,
    /// A linked worktree's dir, `<common dir>/worktrees/<name>`, whose config is the main repo's.
    pub(crate) linked: bool,
}

impl GitDir {
    pub(crate) fn of(repo: &Path) -> Option<GitDir> {
        let git = repo.join(".git");
        if git.is_dir() {
            return Some(GitDir {
                dir: git,
                file: false,
                linked: false,
            });
        }
        let mut head = String::new();
        std::io::Read::read_to_string(
            &mut std::io::Read::take(std::fs::File::open(&git).ok()?, 4096),
            &mut head,
        )
        .ok()?;
        let named = head.lines().next()?.strip_prefix("gitdir:")?.trim();
        let dir = real(&repo.join(named))?;
        let linked = dir
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "worktrees");
        dir.is_dir().then_some(GitDir {
            dir,
            file: true,
            linked,
        })
    }
}

/// The dirs directly inside `dir`, none when it is missing.
pub(crate) fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}
