//! Every path Ostra derives. State is addressed by (session, project key), resolved the same way by
//! every reader and writer, so all derivations live here.

use std::path::{Path, PathBuf};

/// Per-project and per-workspace runtime dir.
pub const RUNTIME_DIR: &str = ".ostra";
/// Per-project skills dir, relative to the project root: the cross-harness `.agents` standard. Ostra
/// writes every new skill here.
pub const SKILLS_DIR: &str = ".agents/skills";
/// Where Ostra kept skills before `.agents/skills`. Still read, never written.
pub const LEGACY_SKILLS_DIR: &str = ".ostra/skills";
/// Every per-project skills dir Ostra loads from, in lookup order.
pub const SKILL_DIRS: &[&str] = &[SKILLS_DIR, LEGACY_SKILLS_DIR];
/// Agent instruction files at a project root, matched case-insensitively.
pub const INSTRUCTION_FILES: &[&str] = &["claude.md", "agents.md", "agent.md"];
/// Artifact prefix, replacing Ultracode's `ultracode-`.
pub const ARTIFACT_PREFIX: &str = "ostra-";

pub fn global_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("OSTRA_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ostra")
        .join("config.toml")
}

pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("OSTRA_DATA_DIR") {
        return PathBuf::from(p);
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ostra")
}

/// A credential file or store under `$HOME`. `harness` names the CLI that signs in with it, which
/// is the one program allowed to see it.
pub struct HomeCredential {
    pub path: &'static str,
    pub harness: Option<crate::HarnessKind>,
}

const fn cred(path: &'static str) -> HomeCredential {
    HomeCredential {
        path,
        harness: None,
    }
}

const fn cli_cred(path: &'static str, harness: crate::HarnessKind) -> HomeCredential {
    HomeCredential {
        path,
        harness: Some(harness),
    }
}

/// Credential stores and personal data under `$HOME` that no agent reads. The policy refuses tool
/// calls that name them, the sandbox hides them, and Grep and Glob skip them.
pub const HOME_CREDENTIALS: &[HomeCredential] = &[
    cred(".ssh"),
    cred(".gnupg"),
    cred(".aws"),
    cred(".azure"),
    cred(".config/gcloud"),
    cred(".kube"),
    cred(".docker"),
    cred(".netrc"),
    cred(".git-credentials"),
    cred(".config/gh/hosts.yml"),
    cred(".config/hub"),
    cred(".cargo/credentials"),
    cred(".cargo/credentials.toml"),
    cred(".pypirc"),
    cred(".vault-token"),
    cred(".terraform.d/credentials.tfrc.json"),
    cred(".local/share/keyrings"),
    cred(".password-store"),
    cred(".Xauthority"),
    cred(".mozilla"),
    cred(".config/google-chrome"),
    cred(".config/chromium"),
    cred(".config/BraveSoftware"),
    cli_cred(".claude/.credentials.json", crate::HarnessKind::Claude),
    cli_cred(".codex/auth.json", crate::HarnessKind::Codex),
    cli_cred(".grok/auth.json", crate::HarnessKind::Grok),
    cli_cred(".gemini/oauth_creds.json", crate::HarnessKind::Agy),
    cli_cred(".gemini/google_accounts.json", crate::HarnessKind::Agy),
];

/// Every path no agent may read: the data dir (registry, master key file, server log), a master
/// key file named by `OSTRA_MASTER_KEY_FILE`, and every [`HOME_CREDENTIALS`] entry.
pub fn secret_paths(home: &Path) -> Vec<PathBuf> {
    let mut out = vec![data_dir()];
    out.extend(std::env::var_os("OSTRA_MASTER_KEY_FILE").map(PathBuf::from));
    out.extend(HOME_CREDENTIALS.iter().map(|c| home.join(c.path)));
    out
}

/// Creates the data dir, or tightens an existing one, so only the owner can enter it: it holds the
/// registry with saved credentials, and SQLite creates its `-wal` and `-shm` files world-readable.
pub fn ensure_data_dir() -> std::io::Result<PathBuf> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
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

/// Uploads waiting for a session or an addition to claim them.
pub fn upload_staging(workspace: &Path) -> PathBuf {
    workspace_runtime(workspace).join("uploads")
}

/// Where a session keeps the files the user uploaded (Rule C3).
pub fn session_uploads(session_root: &Path) -> PathBuf {
    session_root.join("uploads")
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

/// Per-execution harness dir: hook config, MCP registration, and the terminal transcript.
pub fn harness_execution_dir(session_root: &Path, execution: &str) -> PathBuf {
    session_state_dir(session_root)
        .join("harness")
        .join(execution)
}

/// Raw PTY bytes of a harness execution, kept so an ended run can be replayed.
pub fn terminal_transcript(session_root: &Path, execution: &str) -> PathBuf {
    harness_execution_dir(session_root, execution).join("terminal.log")
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

pub fn project_skill_dirs(project: &Path) -> Vec<PathBuf> {
    SKILL_DIRS.iter().map(|d| project.join(d)).collect()
}

/// The `SKILL.md` of the named project skill, from the first skills dir that holds one.
pub fn find_project_skill(project: &Path, name: &str) -> Option<PathBuf> {
    project_skill_dirs(project)
        .into_iter()
        .map(|d| d.join(name).join("SKILL.md"))
        .find(|p| p.is_file())
}

/// `CLAUDE.md`, `AGENTS.md`, and `AGENT.md` at the project root in any letter case, in that order.
pub fn project_instruction_files(project: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(usize, PathBuf)> = std::fs::read_dir(project)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file() || t.is_symlink()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_ascii_lowercase();
            let rank = INSTRUCTION_FILES.iter().position(|f| *f == name)?;
            Some((rank, e.path()))
        })
        .filter(|(_, p)| p.is_file())
        .collect();
    found.sort();
    found.into_iter().map(|(_, p)| p).collect()
}

pub fn project_memory_db(project: &Path) -> PathBuf {
    project_runtime(project)
        .join("memory")
        .join("knowledge.sqlite3")
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
        if is_phase {
            format!("ostra-review-ledger-phase-{v}.md")
        } else {
            "ostra-review-ledger.md".into()
        }
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

/// Resolve `target` against `cwd` the way the kernel would: one component at a time, following
/// each symlink (dangling ones included) before a later `..` applies, so a link cannot make a
/// path look inside a root when the write lands outside it.
pub fn resolve(cwd: &Path, target: &Path) -> PathBuf {
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::path::Component;
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        cwd.join(target)
    };
    if !cfg!(unix) || !joined.is_absolute() {
        return normalize(&joined);
    }
    let parts = |p: &Path| -> Vec<OsString> {
        p.components()
            .filter_map(|c| match c {
                Component::Normal(n) => Some(n.to_os_string()),
                Component::ParentDir => Some(OsString::from("..")),
                _ => None,
            })
            .collect()
    };
    let mut queue: VecDeque<OsString> = parts(&joined).into();
    let mut out = PathBuf::from("/");
    let mut links = 0;
    while let Some(name) = queue.pop_front() {
        if name == ".." {
            out.pop();
            continue;
        }
        out.push(&name);
        let is_link = std::fs::symlink_metadata(&out).is_ok_and(|m| m.file_type().is_symlink());
        if !is_link || links >= 40 {
            continue;
        }
        links += 1;
        let Ok(link) = std::fs::read_link(&out) else {
            continue;
        };
        out.pop();
        if link.is_absolute() {
            out = PathBuf::from("/");
        }
        for p in parts(&link).into_iter().rev() {
            queue.push_front(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn resolve_follows_links_before_dot_dot() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = root.join("repo");
        let outside = root.join("outside/deep");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, repo.join("link")).unwrap();
        assert_eq!(
            resolve(&repo, Path::new("link/../x")),
            root.join("outside/x")
        );
        std::os::unix::fs::symlink(root.join("outside/new"), repo.join("dangling")).unwrap();
        assert_eq!(resolve(&repo, Path::new("dangling")), root.join("outside/new"));
        assert_eq!(resolve(&repo, Path::new("a/../b/c")), repo.join("b/c"));
    }

    #[test]
    fn ledger_names() {
        assert_eq!(report::review_ledger("3"), "ostra-review-ledger-phase-3.md");
        assert_eq!(
            report::review_ledger("3-tests"),
            "ostra-review-ledger-phase-3-tests.md"
        );
        assert_eq!(report::review_ledger("none"), "ostra-review-ledger.md");
        assert_eq!(report::review_ledger("-tests"), "ostra-review-ledger.md");
    }

    #[test]
    fn instruction_files_match_any_case() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["Agents.md", "claude.MD", "README.md", "AGENT.md"] {
            std::fs::write(dir.path().join(f), "x").unwrap();
        }
        std::fs::create_dir(dir.path().join("agents.md.d")).unwrap();
        let names: Vec<String> = project_instruction_files(dir.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["claude.MD", "Agents.md", "AGENT.md"]);
    }

    #[test]
    fn skills_resolve_from_agents_before_ostra() {
        let dir = tempfile::tempdir().unwrap();
        for d in [".ostra/skills/a", ".ostra/skills/b", ".agents/skills/a"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
            std::fs::write(dir.path().join(d).join("SKILL.md"), "x").unwrap();
        }
        let a = find_project_skill(dir.path(), "a").unwrap();
        assert!(a.ends_with(".agents/skills/a/SKILL.md"));
        let b = find_project_skill(dir.path(), "b").unwrap();
        assert!(b.ends_with(".ostra/skills/b/SKILL.md"));
        assert!(find_project_skill(dir.path(), "c").is_none());
    }

    #[test]
    fn inside() {
        assert!(is_inside(Path::new("/a/b"), Path::new("/a/b/c/../d")));
        assert!(!is_inside(Path::new("/a/b"), Path::new("/a/b/../c")));
        assert!(!is_inside(Path::new("/a/b"), Path::new("/a/bc")));
    }
}
