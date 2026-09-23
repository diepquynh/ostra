//! Every path Ostra derives. State is addressed by (session, project key), resolved the same way by
//! every reader and writer, so all derivations live here.

use std::path::{Path, PathBuf};

/// Per-project and per-workspace runtime dir.
pub const RUNTIME_DIR: &str = ".ostra";
/// Per-project skills dir, relative to the project root.
pub const SKILLS_DIR: &str = ".ostra/skills";
/// Artifact prefix, replacing Ultracode's `ultracode-`.
pub const ARTIFACT_PREFIX: &str = "ostra-";

pub fn global_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("OSTRA_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("ostra").join("config.toml")
}

pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("OSTRA_DATA_DIR") {
        return PathBuf::from(p);
    }
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("ostra")
}

pub fn registry_db_path() -> PathBuf {
    data_dir().join("registry.db")
}

pub fn workspace_runtime(workspace: &Path) -> PathBuf {
    workspace.join(RUNTIME_DIR)
}

pub fn workspace_toml(workspace: &Path) -> PathBuf {
    workspace_runtime(workspace).join("workspace.toml")
}

pub fn workspace_db(workspace: &Path) -> PathBuf {
    workspace_runtime(workspace).join("workspace.db")
}

pub fn sessions_root(workspace: &Path) -> PathBuf {
    workspace_runtime(workspace).join("sessions")
}

/// The session's root dir. Cross-project artifacts (spec, plan) live here.
pub fn session_root(workspace: &Path, session_id: &str) -> PathBuf {
    sessions_root(workspace).join(session_id)
}

/// Per-project subdir of a session. Per-project reports live here.
pub fn session_project_dir(workspace: &Path, session_id: &str, project_key: &str) -> PathBuf {
    session_root(workspace, session_id).join(project_key)
}

/// Engine-owned state dir inside a session. No agent may write here.
pub fn session_state_dir(session_root: &Path) -> PathBuf {
    session_root.join(".state")
}

pub fn project_runtime(project: &Path) -> PathBuf {
    project.join(RUNTIME_DIR)
}

pub fn project_inventory(project: &Path) -> PathBuf {
    project_runtime(project).join("INVENTORY.md")
}

pub fn project_profile(project: &Path) -> PathBuf {
    project_runtime(project).join("project.toml")
}

pub fn project_skills_dir(project: &Path) -> PathBuf {
    project.join(SKILLS_DIR)
}

pub fn project_memory_db(project: &Path) -> PathBuf {
    project_runtime(project).join("memory").join("knowledge.sqlite3")
}

/// Declared report names. The engine names every report so later stages can predict them.
pub mod report {
    pub fn implementer(phase: &str) -> String {
        format!("ostra-implementer-phase-{phase}.md")
    }
    pub fn implementer_progress(phase: &str) -> String {
        format!("ostra-implementer-progress-phase-{phase}.md")
    }
    pub fn epa(phase: &str) -> String {
        format!("ostra-epa-phase-{phase}.md")
    }
    pub fn write_test(phase: &str) -> String {
        format!("ostra-write-test-phase-{phase}.md")
    }
    pub fn module_docs() -> String {
        "ostra-module-docs.md".into()
    }
    pub fn prompt_gen(n: u32) -> String {
        format!("ostra-prompt-gen-{n}.md")
    }
    pub fn completion() -> String {
        "ostra-completion.md".into()
    }
    pub fn unit_test_request() -> String {
        "ostra-unit-test-request.md".into()
    }
    /// The review ledger a loop belongs to, from its `Phase:` value (`N`, `N-tests`, or `none`).
    pub fn review_ledger(phase: &str) -> String {
        let v = phase.trim().to_ascii_lowercase();
        let is_phase = {
            let base = v.strip_suffix("-tests").unwrap_or(&v);
            !base.is_empty() && base.chars().all(|c| c.is_ascii_digit())
        };
        if is_phase { format!("ostra-review-ledger-phase-{v}.md") } else { "ostra-review-ledger.md".into() }
    }
    pub fn security_sentinel() -> String {
        "ostra-security-block.json".into()
    }
}

/// True if `inner` is `outer` or sits beneath it. Both should be absolute and normalized.
pub fn is_inside(outer: &Path, inner: &Path) -> bool {
    let outer = normalize(outer);
    let inner = normalize(inner);
    inner.starts_with(&outer)
}

/// Lexical normalization: resolves `.` and `..` without touching the filesystem.
pub fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve `target` against `cwd` and normalize. Follows symlinks of the longest existing prefix
/// so a link cannot escape a root.
pub fn resolve(cwd: &Path, target: &Path) -> PathBuf {
    let joined = if target.is_absolute() { target.to_path_buf() } else { cwd.join(target) };
    let joined = normalize(&joined);
    let mut existing = joined.clone();
    let mut rest = vec![];
    while !existing.exists() {
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return joined,
        }
    }
    let mut base = std::fs::canonicalize(&existing).unwrap_or(existing);
    for name in rest.into_iter().rev() {
        base.push(name);
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_names() {
        assert_eq!(report::review_ledger("3"), "ostra-review-ledger-phase-3.md");
        assert_eq!(report::review_ledger("3-tests"), "ostra-review-ledger-phase-3-tests.md");
        assert_eq!(report::review_ledger("none"), "ostra-review-ledger.md");
        assert_eq!(report::review_ledger("-tests"), "ostra-review-ledger.md");
    }

    #[test]
    fn inside() {
        assert!(is_inside(Path::new("/a/b"), Path::new("/a/b/c/../d")));
        assert!(!is_inside(Path::new("/a/b"), Path::new("/a/b/../c")));
        assert!(!is_inside(Path::new("/a/b"), Path::new("/a/bc")));
    }
}
