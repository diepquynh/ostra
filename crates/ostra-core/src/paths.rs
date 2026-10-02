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

/// `std::fs::canonicalize` without the `\\?\` prefix Windows adds, where the plain form names the
/// same file (short enough, no reserved names). Git, shells, and most programs refuse a verbatim
/// path, and guards compare against plain ones. On Unix it is `std::fs::canonicalize`.
pub fn canonical(p: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    dunce::canonicalize(p)
}

/// The user's home folder: `$HOME` on Unix, the profile folder (`C:\Users\<name>`) on Windows,
/// which does not set `HOME`.
pub fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// A path a Git Bash command names, as a Windows path, so guards resolve it to the same file the
/// shell writes: `/c/Users/me` becomes `C:\Users\me`, `/tmp` becomes Git's `%TEMP%`, and a
/// leading `~` becomes the home folder. A path that is already a Windows path, or is not an MSYS
/// path, comes back unchanged. Only on Windows; Unix paths are returned as they are.
pub fn from_msys(raw: &str) -> PathBuf {
    #[cfg(not(windows))]
    return PathBuf::from(raw);
    #[cfg(windows)]
    {
        let s = raw.trim();
        // `/c/rest` or `/c` (a drive by MSYS letter), only for a real drive letter.
        let bytes = s.as_bytes();
        if bytes.len() >= 2
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && (bytes.len() == 2 || bytes[2] == b'/')
        {
            let drive = bytes[1].to_ascii_uppercase() as char;
            let rest = &s[2..];
            return PathBuf::from(format!("{drive}:{}", if rest.is_empty() { "\\" } else { rest }));
        }
        // `/tmp` is Git Bash's `%TEMP%`.
        if s == "/tmp" || s.starts_with("/tmp/") {
            return std::env::temp_dir().join(s.trim_start_matches("/tmp").trim_start_matches('/'));
        }
        PathBuf::from(s)
    }
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
    cred("Library/Keychains"),
    cred("Library/Cookies"),
    cred("Library/HTTPStorages"),
    cred("Library/Application Support/Google/Chrome"),
    cred("Library/Application Support/Chromium"),
    cred("Library/Application Support/BraveSoftware"),
    cred("Library/Application Support/Microsoft Edge"),
    cred("Library/Application Support/Arc"),
    cred("Library/Application Support/Firefox"),
    cred("Library/Safari"),
    cred("Library/Group Containers"),
    cred("Library/Containers"),
    cred("Library/Messages"),
    cred("Library/Mail"),
    cred("Library/Application Support/com.apple.TCC"),
    cred("Library/Application Support/Code/User/globalStorage"),
    // Windows, under the profile folder (WINDOWS_HANDOVER 1.5). `_netrc` is curl's Windows name.
    cred("_netrc"),
    cred("AppData/Roaming/gcloud"),
    cred("AppData/Roaming/GitHub CLI"),
    cred("AppData/Roaming/Microsoft/Protect"),
    cred("AppData/Roaming/Microsoft/Credentials"),
    cred("AppData/Local/Microsoft/Credentials"),
    cred("AppData/Local/Google/Chrome/User Data"),
    cred("AppData/Local/Microsoft/Edge/User Data"),
    cred("AppData/Local/BraveSoftware"),
    cred("AppData/Roaming/Mozilla/Firefox"),
    cred("AppData/Roaming/Code/User/globalStorage"),
    cred("AppData/Roaming/Microsoft/Windows/PowerShell/PSReadLine/ConsoleHost_history.txt"),
    cli_cred(".claude/.credentials.json", crate::HarnessKind::Claude),
    cli_cred(".codex/auth.json", crate::HarnessKind::Codex),
    cli_cred(".grok/auth.json", crate::HarnessKind::Grok),
    cli_cred(".gemini/oauth_creds.json", crate::HarnessKind::Agy),
    cli_cred(".gemini/google_accounts.json", crate::HarnessKind::Agy),
];

/// Ostra's own files, which no setting lets an agent read: the data dir, the config dir (or only
/// the file `OSTRA_CONFIG` names, which may sit in a shared dir), and the master key file.
pub fn ostra_private_paths() -> Vec<PathBuf> {
    let config = match std::env::var_os("OSTRA_CONFIG") {
        Some(_) => global_config_path(),
        None => global_config_path()
            .parent()
            .map_or_else(global_config_path, Path::to_path_buf),
    };
    let mut out = vec![data_dir(), config];
    out.extend(std::env::var_os("OSTRA_MASTER_KEY_FILE").map(PathBuf::from));
    out
}

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

/// Creates `dir` and its missing parents, the new ones owner-only (0700) on Unix. On Windows a new
/// dir inherits its parent's ACL, which under the user's profile grants only that user, SYSTEM,
/// and administrators.
pub fn create_private_dir_all(dir: &Path) -> std::io::Result<()> {
    #[allow(unused_mut)]
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// Creates `dir` owner-only, as [`create_private_dir_all`], succeeding when it already exists.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[allow(unused_mut)]
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    match b.create(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

/// Open options that create a file owner-only (0600) on Unix; on Windows the file inherits its
/// dir's ACL, as in [`create_private_dir_all`].
pub fn private_file_options() -> std::fs::OpenOptions {
    #[allow(unused_mut)]
    let mut o = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o
}

/// Tightens an existing file (0600) or dir (0700) to its owner on Unix. On Windows it keeps the
/// ACL it inherited, as in [`create_private_dir_all`].
pub fn restrict_to_owner(path: &Path, dir: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if dir { 0o700 } else { 0o600 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    let _ = (path, dir);
    Ok(())
}

/// A link at `link` to `target`: a symlink on Unix. On Windows a dir gets a junction, which needs
/// no privilege, and a file a symlink where Developer Mode allows one, else a hard link, else a
/// copy.
pub fn link(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    {
        if target.is_dir() {
            return junction::create(target, link);
        }
        std::os::windows::fs::symlink_file(target, link)
            .or_else(|_| std::fs::hard_link(target, link))
            .or_else(|_| std::fs::copy(target, link).map(|_| ()))
    }
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
    /// Rule O8: the change report of the `round`-th implementer sent to a stuck run of the loop.
    pub fn unblock(phase: &str, round: u32) -> String {
        format!("ostra-implementer-unblock-phase-{phase}-{round}.md")
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
    /// The engine-written stub a docs writer gets when no implementer ran (category DOCS).
    pub fn docs_request() -> String {
        "ostra-docs-request.md".into()
    }
    /// The book's parts from this session, written by the engine for the architecture agent.
    pub fn docs_parts() -> String {
        "ostra-docs-parts.json".into()
    }
    pub fn prompt_gen(n: u32) -> String {
        format!("ostra-prompt-gen-{n}.md")
    }
    pub fn completion() -> String {
        "ostra-completion.md".into()
    }
    pub fn test_request() -> String {
        "ostra-test-request.md".into()
    }
    /// The engine-written session context, in the session root: the request, every artifact
    /// path, and each feedback round, so a revision reads files instead of a conversation.
    pub fn session_context() -> String {
        "ostra-session-context.md".into()
    }
    /// Rule D4a: the engine-written code facts from the research documents, in the session root.
    pub fn code_facts() -> String {
        "ostra-code-facts.md".into()
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

/// True if `inner` is `outer` or sits beneath it. Both should be absolute and normalized. On
/// Windows the comparison ignores letter case, as NTFS does.
pub fn is_inside(outer: &Path, inner: &Path) -> bool {
    let outer = fold(&normalize(outer));
    let inner = fold(&normalize(inner));
    inner.starts_with(&outer)
}

/// The form of a path that guards compare. On Windows two names NTFS treats as one fold to the
/// same text: `\\?\` is dropped, separators become `\`, and each character is uppercased and then
/// lowercased where both map one to one, so every pair NTFS's uppercase table equates stays
/// equal (the fold may equate a few pairs NTFS keeps apart, which only makes a guard stricter).
/// Unix paths are case-sensitive and come back unchanged.
pub fn fold(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = strip_verbatim(p).to_string_lossy().into_owned();
        let folded: String = s
            .chars()
            .map(|c| {
                if c == '/' {
                    return '\\';
                }
                let mut up = c.to_uppercase();
                let u = match (up.next(), up.next()) {
                    (Some(u), None) => u,
                    _ => c,
                };
                let mut low = u.to_lowercase();
                match (low.next(), low.next()) {
                    (Some(l), None) => l,
                    _ => u,
                }
            })
            .collect();
        PathBuf::from(folded)
    }
    #[cfg(not(windows))]
    p.to_path_buf()
}

/// `p` without the `\\?\` prefix `canonicalize` and `read_link` return on Windows: `\\?\C:\x`
/// becomes `C:\x` and `\\?\UNC\srv\share\x` becomes `\\srv\share\x`. Other paths come back
/// unchanged.
pub fn strip_verbatim(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut comps = p.components();
        let Some(Component::Prefix(prefix)) = comps.next() else {
            return p.to_path_buf();
        };
        let mut out = match prefix.kind() {
            Prefix::VerbatimDisk(d) => PathBuf::from(format!("{}:", d as char)),
            Prefix::VerbatimUNC(server, share) => {
                let mut s = std::ffi::OsString::from(r"\\");
                s.push(server);
                s.push(r"\");
                s.push(share);
                PathBuf::from(s)
            }
            _ => return p.to_path_buf(),
        };
        for c in comps {
            match c {
                Component::RootDir => out.push(r"\"),
                other => out.push(other.as_os_str()),
            }
        }
        out
    }
    #[cfg(not(windows))]
    p.to_path_buf()
}

/// Why a path in a tool call names something other than a plain file path, on Windows: another
/// machine or the local one by share name (`\\host\share`, `\\?\UNC\...`), a device or the object
/// namespace (`\\.\`, `\\?\` other than a drive, `\??\`), a path relative to a drive's own
/// working directory (`C:foo`), or an alternate data stream (`file:stream`). Guards compare
/// drive paths only, so each of these could name a protected file under a spelling they miss.
/// `None` for a plain path, and always on Unix.
pub fn windows_path_problem(raw: &str) -> Option<&'static str> {
    if !cfg!(windows) {
        return None;
    }
    let s = raw.trim().replace('/', r"\");
    if s.starts_with(r"\??\") {
        return Some("an NT object path");
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        let disk = rest
            .strip_prefix(r"?\")
            .filter(|r| r.len() >= 3 && r.as_bytes()[0].is_ascii_alphabetic() && &r[1..3] == r":\");
        return match disk {
            Some(r) if !r[2..].contains(':') => None,
            Some(_) => Some("an alternate data stream"),
            None if rest.starts_with(r".\") || rest.starts_with(r"?\") => {
                Some("a device or object namespace path")
            }
            None => Some("a network or administrative share path"),
        };
    }
    let b = s.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if drive && b.get(2) != Some(&b'\\') {
        return Some("a path relative to a drive's working directory");
    }
    let rest = if drive { &s[2..] } else { &s[..] };
    rest.contains(':').then_some("an alternate data stream")
}

/// Windows device names, which open a device under any extension and in any folder.
const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "com¹", "com²", "com³", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8",
    "lpt9", "lpt¹", "lpt²", "lpt³", "conin$", "conout$",
];

/// True when the last component of `p` is a Windows device name other than `NUL` (`CON`,
/// `COM1.txt`, `lpt1 .log`), which opens a device instead of a file. Always false on Unix.
pub fn is_reserved_device(p: &Path) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let Some(name) = p.file_name().map(|n| n.to_string_lossy().to_lowercase()) else {
        return false;
    };
    let stem = name.split('.').next().unwrap_or_default().trim_end_matches([' ', '.']);
    RESERVED_NAMES.contains(&stem)
}

/// A component as Win32 opens it: trailing dots and spaces dropped, and an alternate data stream
/// suffix (`name:stream`) dropped, so the name is the file the stream belongs to.
#[cfg(windows)]
fn win32_name(n: &std::ffi::OsStr) -> std::ffi::OsString {
    let s = n.to_string_lossy();
    let s = s.split(':').next().unwrap_or_default();
    std::ffi::OsString::from(s.trim_end_matches(['.', ' ']))
}

/// `p` made absolute against `cwd` with `.`, `..`, and each component cleaned as Win32 does
/// before the file system sees it: `..` applies lexically, even after a link.
#[cfg(windows)]
fn win32_lexical(cwd: &Path, p: &Path) -> PathBuf {
    use std::path::Component;
    let joined = strip_verbatim(&if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    });
    let mut out = PathBuf::new();
    let mut names = 0usize;
    for c in joined.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if names > 0 && out.pop() {
                    names -= 1;
                }
            }
            Component::Normal(n) => {
                let n = win32_name(n);
                if n.is_empty() {
                    continue;
                }
                if n == ".." {
                    if names > 0 && out.pop() {
                        names -= 1;
                    }
                    continue;
                }
                out.push(n);
                names += 1;
            }
        }
    }
    out
}

/// The path a Win32 program opens for `target` from `cwd`: `..` applied lexically, then the
/// junctions and symlinks in it followed, dangling ones included, then the longest existing
/// ancestor in its real letter case with long names in place of 8.3 short names.
#[cfg(windows)]
fn resolve_windows(cwd: &Path, target: &Path) -> PathBuf {
    let mut path = win32_lexical(cwd, target);
    if !path.is_absolute() {
        return path;
    }
    for _ in 0..40 {
        let mut existing = path.clone();
        let mut tail: Vec<std::ffi::OsString> = vec![];
        while std::fs::symlink_metadata(&existing).is_err() {
            let Some(name) = existing.file_name().map(|n| n.to_os_string()) else {
                return path;
            };
            tail.push(name);
            if !existing.pop() {
                return path;
            }
        }
        // Joining an empty `rest` would append a trailing separator, so keep the path as is.
        let join_rest = |base: PathBuf| {
            tail.iter()
                .rev()
                .fold(base, |acc: PathBuf, n| acc.join(n))
        };
        match canonical(&existing) {
            Ok(real) => return join_rest(strip_verbatim(&real)),
            Err(_) => match std::fs::read_link(&existing) {
                // A dangling link: the file lands at its target.
                Ok(to) => {
                    let base = existing.parent().map(Path::to_path_buf).unwrap_or_default();
                    path = join_rest(win32_lexical(&base, &strip_verbatim(&to)));
                }
                Err(_) => return path,
            },
        }
    }
    path
}

/// True when `raw`, taken from `cwd`, has a `..` right after a component that is a junction or
/// symlink. Win32 programs apply that `..` to the link's own folder, while Git Bash and other
/// POSIX-style programs apply it to the link's target, so the two open different files and a
/// guard cannot judge both. Always false on Unix, where [`resolve`] follows the kernel.
pub fn dot_dot_after_link(cwd: &Path, raw: &Path) -> bool {
    if !cfg!(windows) {
        return false;
    }
    use std::path::Component;
    let mut at = if raw.is_absolute() {
        PathBuf::new()
    } else {
        cwd.to_path_buf()
    };
    let mut prev_link = false;
    for c in raw.components() {
        let dotdot = matches!(c, Component::ParentDir)
            || matches!(c, Component::Normal(n) if n == "..");
        if dotdot && prev_link {
            return true;
        }
        at.push(c.as_os_str());
        prev_link = std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink());
    }
    false
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
/// path look inside a root when the write lands outside it. On Windows it resolves as Win32
/// does (see `resolve_windows`).
pub fn resolve(cwd: &Path, target: &Path) -> PathBuf {
    #[cfg(windows)]
    return resolve_windows(cwd, target);
    #[cfg(not(windows))]
    resolve_posix(cwd, target)
}

#[cfg(not(windows))]
fn resolve_posix(cwd: &Path, target: &Path) -> PathBuf {
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
        let root = canonical(dir.path()).unwrap();
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
        assert_eq!(
            resolve(&repo, Path::new("dangling")),
            root.join("outside/new")
        );
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

    #[cfg(windows)]
    #[test]
    fn is_inside_folds_case_and_separators() {
        assert!(is_inside(
            Path::new(r"C:\Users\Me\repo"),
            Path::new(r"c:\users\me\REPO\src\a.rs")
        ));
        assert!(is_inside(
            Path::new(r"\\?\C:\Users\Me\repo"),
            Path::new(r"C:/Users/Me/repo/src")
        ));
        assert!(!is_inside(
            Path::new(r"C:\Users\Me\repo"),
            Path::new(r"C:\Users\Me\repo-other")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn strip_verbatim_removes_the_prefix() {
        assert_eq!(strip_verbatim(Path::new(r"\\?\C:\x\y")), PathBuf::from(r"C:\x\y"));
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\srv\share\f")),
            PathBuf::from(r"\\srv\share\f")
        );
        assert_eq!(strip_verbatim(Path::new(r"C:\x")), PathBuf::from(r"C:\x"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_problems_are_named() {
        assert_eq!(windows_path_problem(r"C:\ok\file.txt"), None);
        assert_eq!(windows_path_problem("C:/ok/file.txt"), None);
        assert!(windows_path_problem(r"\\host\share\f").is_some());
        assert!(windows_path_problem(r"\\.\PhysicalDrive0").is_some());
        assert!(windows_path_problem(r"\??\C:\x").is_some());
        assert!(windows_path_problem("C:file").is_some());
        assert!(windows_path_problem(r"C:\x\file.txt:hidden").is_some());
        assert!(windows_path_problem(r"\\?\C:\x\y:s").is_some());
        // A verbatim drive path is allowed through, to be normalized by `resolve`.
        assert_eq!(windows_path_problem(r"\\?\C:\x\y"), None);
    }

    #[cfg(windows)]
    #[test]
    fn reserved_device_names_are_caught_except_nul() {
        assert!(is_reserved_device(Path::new(r"C:\x\CON")));
        assert!(is_reserved_device(Path::new(r"C:\x\com1.txt")));
        assert!(is_reserved_device(Path::new(r"C:\x\LPT9")));
        assert!(!is_reserved_device(Path::new(r"C:\x\NUL")));
        assert!(!is_reserved_device(Path::new(r"C:\x\console.txt")));
    }

    #[cfg(windows)]
    #[test]
    fn resolve_strips_verbatim_and_applies_dot_dot_lexically() {
        let dir = tempfile::tempdir().unwrap();
        let root = strip_verbatim(&canonical(dir.path()).unwrap());
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        // `..` after a real dir applies lexically on Windows.
        assert_eq!(resolve(&repo, Path::new("a/../b/c")), repo.join("b\\c"));
        // A forward-slash drive path resolves to the same file as its backslash form.
        assert_eq!(
            resolve(Path::new(r"C:\other"), &PathBuf::from(format!("{}/repo/x", root.display().to_string().replace('\\', "/")))),
            repo.join("x")
        );
    }
}
