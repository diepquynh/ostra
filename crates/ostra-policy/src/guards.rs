//! Layer 1: guards no permission, user answer, or YOLO setting overrides. Each is a port of an
//! Ultracode hook library, named in its function's comment.

use crate::bash::{self, Parsed, SimpleCommand, TargetSpec};
use ostra_core::exec::ExecContext;
use ostra_core::ignore_files::{IgnoreFiles, Under};
use ostra_core::paths;
use ostra_core::policy::RuleRef;
use ostra_core::{Capability, WriteScope};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub const WRITE_SCOPE: &str = "write-scope";
pub const NO_TESTS: &str = "no-tests-from-implementer";
pub const STATE_OWNERSHIP: &str = "state-ownership";
pub const ARTIFACT_OWNERSHIP: &str = "artifact-ownership";
pub const REPORT_PATH: &str = "report-path";
pub const DOCUMENT_TOOL: &str = "document-tool";
pub const LESSON_GATE: &str = "lesson-gate";
pub const BUILD_STREAK: &str = "build-streak";
pub const SELF_PROTECTION: &str = ostra_core::containment::SELF_PROTECTION;
pub const GIT_METADATA: &str = ostra_core::containment::GIT_METADATA;
pub const SECRET_READ: &str = ostra_core::containment::SECRET_READ;
pub const WORKSPACE_ARTIFACTS: &str = "workspace-artifacts";
pub const WORKSPACE_DOCS: &str = "workspace-docs";
pub const MANAGE_TOOLS: &str = "manage-tools";

/// A refusal: which guard, and the message for the model (correction first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    pub rule: RuleRef,
    pub reason: String,
}

fn deny(guard: &str, reason: String) -> Denial {
    Denial {
        rule: RuleRef::guard(guard),
        reason,
    }
}

fn canon(p: &Path) -> PathBuf {
    if p.as_os_str().is_empty() {
        PathBuf::new()
    } else {
        paths::resolve(p, p)
    }
}

/// Canonical roots of one execution, computed once.
#[derive(Debug, Clone)]
pub struct Roots {
    pub repo: PathBuf,
    pub session_dir: PathBuf,
    pub session_root: PathBuf,
    pub state_dir: PathBuf,
    pub sessions_root: PathBuf,
    pub memory_db: PathBuf,
    pub protected: Vec<PathBuf>,
    /// Files protected together with their `-wal`, `-shm`, and `-journal` siblings.
    pub protected_db_files: Vec<PathBuf>,
    pub temps: Vec<PathBuf>,
    /// Never read by an agent: the data dir (registry, master key, server log) and the key file.
    pub secret: Vec<PathBuf>,
    /// Credential stores, which an agent reads only where `readable` covers them.
    pub credentials: Vec<PathBuf>,
    /// The user's `extra_readable` and `sandbox_readable` entries.
    pub readable: Vec<PathBuf>,
    /// Agent assets inside the data dir, which agents do read.
    pub assets: PathBuf,
    pub home: PathBuf,
    pub report_file: Option<PathBuf>,
    /// The workspace's visible artifacts, which agents read and never write (Rule W1).
    pub artifacts: PathBuf,
    /// The workspace's documentation books, which only the engine writes (Rule B5).
    pub books: PathBuf,
    /// Rule G1: tool enforcement is on, so write scope, the report path, and self-protection
    /// apply.
    pub strict: bool,
}

impl Roots {
    pub fn new(ctx: &ExecContext) -> Self {
        let home = ostra_core::paths::home().unwrap_or_else(|| PathBuf::from("/"));
        let session_root = canon(&ctx.session_root);
        let mut temps = vec![canon(&std::env::temp_dir()), canon(Path::new("/tmp"))];
        temps.dedup();
        let ws = &ctx.workspace_root;
        let mut protected: Vec<PathBuf> = ctx.protected_paths.iter().map(|p| canon(p)).collect();
        if !ws.as_os_str().is_empty() {
            protected.push(canon(&paths::workspace_toml(ws)));
            // Rules CA1, WF1, and WB7: an agent that wrote these would define the agents, stages,
            // and transforms that run after it.
            protected.push(canon(&paths::workspace_agents_dir(ws)));
            protected.push(canon(&paths::workspace_workflows_dir(ws)));
            protected.push(canon(&paths::workspace_transforms_dir(ws)));
        }
        let mut protected_db_files = vec![];
        if !ws.as_os_str().is_empty() {
            protected_db_files.push(canon(&paths::workspace_db(ws)));
        }
        Roots {
            repo: canon(&ctx.repo_root),
            session_dir: canon(&ctx.session_dir),
            state_dir: paths::session_state_dir(&session_root),
            session_root,
            sessions_root: if ws.as_os_str().is_empty() {
                PathBuf::new()
            } else {
                canon(&paths::sessions_root(ws))
            },
            memory_db: canon(&ctx.memory_db),
            protected,
            protected_db_files,
            temps,
            secret: [paths::data_dir()]
                .into_iter()
                .chain(std::env::var_os("OSTRA_MASTER_KEY_FILE").map(PathBuf::from))
                .map(|p| canon(&p))
                .collect(),
            credentials: paths::HOME_CREDENTIALS
                .iter()
                .map(|c| canon(&home.join(c.path)))
                .collect(),
            readable: ctx
                .sandbox_readable
                .iter()
                .filter_map(|r| {
                    let r = r.trim();
                    let p = match r.strip_prefix("~/") {
                        Some(rest) => home.join(rest),
                        None => PathBuf::from(r),
                    };
                    // Each entry opens only what it names, never the home folder as a whole.
                    (p.is_absolute() && !home.starts_with(&p)).then(|| canon(&p))
                })
                .collect(),
            assets: canon(&paths::data_dir().join("assets")),
            home,
            report_file: ctx.report_file.as_ref().map(|p| canon(p)),
            artifacts: if ws.as_os_str().is_empty() {
                PathBuf::new()
            } else {
                canon(&ostra_core::artifacts::dir(ws))
            },
            books: if ws.as_os_str().is_empty() {
                PathBuf::new()
            } else {
                canon(&ostra_core::book::dir(ws))
            },
            strict: ctx.enforce_tool_calls,
        }
    }

    pub fn resolve(&self, base: &Path, raw: &str) -> PathBuf {
        let raw = raw.trim();
        if raw == "~" {
            return self.home.clone();
        }
        if let Some(rest) = raw.strip_prefix("~/") {
            return paths::resolve(&self.home, Path::new(rest));
        }
        // Git Bash writes MSYS paths (`/c/Users/me`, `/tmp`), which name a different file than the
        // same text would on Unix, so they are translated before the guard compares.
        let translated = paths::from_msys(raw);
        paths::resolve(base, &translated)
    }

    pub fn in_session(&self, p: &Path) -> bool {
        inside(&self.session_dir, p) || inside(&self.session_root, p)
    }

    /// OS temp scratch. A project that itself lives under temp is project source, not scratch.
    pub fn in_temp(&self, p: &Path) -> bool {
        self.temps.iter().any(|t| inside(t, p)) && !inside(&self.repo, p)
    }

    /// Rule G2: the ignore files as this execution's searches see them, Ostra's own state and the
    /// temp dirs exempt.
    pub fn ignore_files(&self) -> IgnoreFiles {
        IgnoreFiles::new().exempt(
            self.sessions_root
                .parent()
                .map(Path::to_path_buf)
                .into_iter()
                .chain(self.temps.iter().cloned()),
        )
    }

    pub fn in_repo(&self, p: &Path) -> bool {
        inside(&self.repo, p)
    }

    fn db_file_match(file: &Path, p: &Path) -> bool {
        if file.as_os_str().is_empty() {
            return false;
        }
        if p == file {
            return true;
        }
        let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else {
            return false;
        };
        p.parent() == Some(dir)
            && p.file_name().is_some_and(|n| {
                let n = n.to_string_lossy();
                let base = name.to_string_lossy();
                ["-wal", "-shm", "-journal"]
                    .iter()
                    .any(|s| n == format!("{base}{s}"))
            })
    }

    pub fn is_protected(&self, p: &Path) -> bool {
        self.protected.iter().any(|r| inside(r, p))
            || self
                .protected_db_files
                .iter()
                .any(|f| Self::db_file_match(f, p))
    }

    /// Holds credentials or another process's memory: Ostra's data dir, a workspace database
    /// (every tool output of every session), or `/proc/<pid>/...`, whose `environ` and `fd`
    /// entries expose the server's environment and open files to in-process tools.
    pub fn is_secret(&self, p: &Path) -> bool {
        (self.secret.iter().any(|r| inside(r, p)) && !inside(&self.assets, p))
            || (self.credentials.iter().any(|r| inside(r, p))
                && !self.readable.iter().any(|r| inside(r, p)))
            || self
                .protected_db_files
                .iter()
                .any(|f| Self::db_file_match(f, p))
            || proc_of_process(p)
            || self.is_harness_state(p)
    }

    /// Any execution's harness dir, which holds that execution's bridge token.
    fn is_harness_state(&self, p: &Path) -> bool {
        !self.sessions_root.as_os_str().is_empty()
            && p.strip_prefix(&self.sessions_root).is_ok_and(|rel| {
                let mut c = rel.components().skip(1);
                c.next().is_some_and(|c| c.as_os_str() == ".state")
                    && c.next().is_some_and(|c| c.as_os_str() == "harness")
            })
    }

    pub fn is_memory_db(&self, p: &Path) -> bool {
        Self::db_file_match(&self.memory_db, p)
    }

    /// Engine state: this session's state dir, any session's `.state`, and the memory store.
    pub fn is_engine_state(&self, p: &Path) -> bool {
        (!self.state_dir.as_os_str().is_empty() && inside(&self.state_dir, p))
            || (!self.sessions_root.as_os_str().is_empty()
                && p.strip_prefix(&self.sessions_root)
                    .is_ok_and(|rel| rel.components().any(|c| c.as_os_str() == ".state")))
            || self.is_memory_db(p)
    }
}

fn inside(root: &Path, p: &Path) -> bool {
    !root.as_os_str().is_empty() && paths::is_inside(root, p)
}

fn disp(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

// ---------------------------------------------------------------------------------------------
// Ledgers and artifacts (ledger-policy.js, artifact-guard.js)
// ---------------------------------------------------------------------------------------------

struct Owned {
    pattern: Regex,
    /// Rule CA6: the capability that grants writing it.
    grant: Capability,
    stakes: &'static str,
}

static TOOL_OWNED: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            Regex::new(r"^knowledge\.sqlite3(-wal|-shm|-journal)?$").unwrap(),
            "the lesson store, written only by the Memory tool because later sessions recall it as fact",
        ),
        (
            Regex::new(r"^(workspace|registry)\.db(-wal|-shm|-journal)?$").unwrap(),
            "Ostra's own database, the record of every event, gate, and decision",
        ),
    ]
});

static AGENT_OWNED: LazyLock<Vec<Owned>> = LazyLock::new(|| {
    vec![
        Owned {
            pattern: Regex::new(r"^ostra-review-ledger(-[\w.-]+)?\.md$").unwrap(),
            grant: Capability::ReviewLedger,
            stakes: "the engine counts its iterations to cap the review loop",
        },
        Owned {
            pattern: Regex::new(r"^ostra-security-block\.json$").unwrap(),
            grant: Capability::SecurityBlock,
            stakes: "it records unwaivable BLOCKER findings",
        },
        Owned {
            pattern: Regex::new(r"^ostra-implementer-progress(-[\w.-]+)?\.md$").unwrap(),
            grant: Capability::ProgressLog,
            stakes: "re-runs read it to learn which steps already succeeded",
        },
    ]
});

/// Rule CA6: each typed document, the capability that grants writing it, and why it is guarded.
static ARTIFACTS: LazyLock<Vec<(Regex, Capability, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            Regex::new(r"^ostra-spec-.*\.(md|json)$").unwrap(),
            Capability::DocumentSpec,
            "the spec is the requirements contract, written only by the agent that writes the spec (Rules D3 and D10)",
        ),
        (
            Regex::new(r"^ostra-plan-.*\.(md|json)$").unwrap(),
            Capability::DocumentPlan,
            "the plan and its phase files are written only by the agent that writes the plan (Rule D10)",
        ),
        (
            Regex::new(r"^ostra-research-.*\.(md|json)$").unwrap(),
            Capability::DocumentResearch,
            "research documents are written only by the agents that research",
        ),
    ]
});

fn grant_name(c: Capability) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// One pattern matching every protected state name, for scanning inline interpreter code.
pub static STATE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(ostra-review-ledger(-[\w.-]+)?\.md|ostra-security-block\.json|ostra-implementer-progress(-[\w.-]+)?\.md|knowledge\.sqlite3|workspace\.db|registry\.db|workspace\.toml|/\.state(/|\b))",
    )
    .unwrap()
});

// ---------------------------------------------------------------------------------------------
// Test paths (scope-policy.js isTestPath)
// ---------------------------------------------------------------------------------------------

static TEST_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(^|/)(__tests__|__mocks__|tests?)/").unwrap());
static TEST_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(\.(test|spec)\.[cm]?[jt]sx?$)|(^test_.+\.py$)|(_test\.py$)|(_test\.go$)|(_spec\.rb$)|(^spec_.+\.rb$)|([_.]?[Tt]ests?\.(java|kt|kts|cs)$)",
    )
    .unwrap()
});

/// A path that looks like a test file, judged relative to the repo root.
pub fn is_test_path(rel: &str) -> bool {
    let normalized = rel.replace('\\', "/");
    let base = normalized.rsplit('/').next().unwrap_or(&normalized);
    TEST_DIR.is_match(&normalized) || TEST_FILE.is_match(base)
}

// ---------------------------------------------------------------------------------------------
// Write checks
// ---------------------------------------------------------------------------------------------

/// On Windows, a word that looks like a path even without a forward slash: a backslash, a drive
/// (`C:` or `C:\`), or a verbatim or UNC prefix. Always false on Unix, where only `/` and `~`
/// mark a path.
fn is_windows_pathlike(text: &str) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let b = text.as_bytes();
    text.contains('\\') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

fn proc_of_process(p: &Path) -> bool {
    use std::path::Component;
    let mut parts = p
        .components()
        .skip_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir));
    parts.next().is_some_and(|c| c.as_os_str() == "proc")
        && parts.next().is_some_and(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == "self" || s == "thread-self" || s.chars().all(|c| c.is_ascii_digit())
        })
}

pub const WINDOWS_PATH: &str = "windows-path";

/// Hardening (WINDOWS_HANDOVER 1.4): a Windows path form the guards do not compare against, such
/// as an alternate data stream (`file:stream`), a reserved device name (`CON`, `COM1`), a UNC or
/// device path (`\\host\share`, `\\?\`), or a drive-relative path (`C:foo`). Refused for any tool
/// call, because such a spelling could reach a protected file under a name the guards miss.
/// Always `None` on Unix.
pub fn check_windows_path(raw: &str, target: &Path) -> Option<Denial> {
    if let Some(what) = paths::windows_path_problem(raw) {
        return Some(deny(
            WINDOWS_PATH,
            format!(
                "Do not use \"{raw}\": it is {what}, which Ostra refuses in a tool call because it can name a protected \
                 file under a spelling the guards do not check. Use a plain drive path such as C:\\path\\to\\file."
            ),
        ));
    }
    if paths::is_reserved_device(target) {
        return Some(deny(
            WINDOWS_PATH,
            format!(
                "Do not use \"{raw}\": it names a Windows device (such as CON, NUL, or COM1), which opens the device \
                 instead of a file. Choose another name."
            ),
        ));
    }
    None
}

/// Hardening: reads of credentials and process memory are refused for every tool and mode.
pub fn check_read(roots: &Roots, target: &Path, raw: &str) -> Option<Denial> {
    roots.is_secret(target).then(|| {
        deny(
            SECRET_READ,
            format!(
                "Do not read \"{raw}\": it holds Ostra's credentials, session records, or another process's memory, \
                 which agents never see. If the task needs a credential, ask for it in your report."
            ),
        )
    })
}

/// Hardening: every path-like word of a shell command, checked with [`check_read`].
pub fn check_shell_reads(roots: &Roots, parsed: &Parsed, start: &Path) -> Option<Denial> {
    let cwds = bash::command_cwds(parsed);
    for (i, cmd) in parsed.commands.iter().enumerate() {
        let base = cwd_for(roots, start, cwds[i].as_deref());
        // Printing a path names it without opening it.
        if matches!(cmd.effective_name().as_deref(), Some("echo" | "printf")) {
            continue;
        }
        let words = cmd
            .words
            .iter()
            .chain(cmd.redirects.iter().filter_map(|r| r.target.as_ref()));
        for w in words {
            let text = w.text();
            let pathlike = text.contains('/') || text.starts_with('~') || is_windows_pathlike(text);
            let names_proc = text.contains("/proc/");
            if !(pathlike || names_proc) {
                continue;
            }
            // `/proc/$pid/environ` is dynamic, so its source text is matched as well.
            if names_proc && PROC_SECRET.is_match(text) {
                return check_read(roots, Path::new("/proc/self"), text);
            }
            let value = text
                .split_once('=')
                .map_or(text, |(k, v)| if k.starts_with('-') { v } else { text });
            if let Some(d) = check_read(roots, &roots.resolve(&base, value), value) {
                return Some(d);
            }
        }
    }
    None
}

static PROC_SECRET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/proc/[^/\s]+/(environ|mem|fd)\b").unwrap());

/// Checks every guard that applies to one write target. `pending_lesson` is the first
/// unrecorded recovery, if any.
pub fn check_write(
    ctx: &ExecContext,
    roots: &Roots,
    target: &Path,
    raw: &str,
    pending_lesson: Option<&crate::build::Recovery>,
) -> Option<Denial> {
    if let Some(d) = check_creates_project(ctx, roots, target) {
        return Some(d);
    }
    let agent = ctx.agent;
    let base = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Hardening (Windows): refuse a stream, device, UNC, or drive-relative spelling before the
    // scope checks, since it could name a protected file the guards would not recognize.
    if let Some(d) = check_windows_path(raw, target) {
        return Some(d);
    }

    // Tool self-protection (plugin-policy.js checkPluginWrite).
    if roots.strict && roots.is_protected(target) {
        return Some(deny(
            SELF_PROTECTION,
            format!(
                "Leave \"{raw}\" alone: it is part of Ostra itself (its binary, configuration, or databases). Agents may read \
                 these files but never write, move, or delete them, because they are what enforces the pipeline. If Ostra \
                 is behaving wrongly, say so in your report."
            ),
        ));
    }

    // Rule W1: agents read workspace artifacts; only the user changes them.
    if inside(&roots.artifacts, target) {
        return Some(deny(
            WORKSPACE_ARTIFACTS,
            format!(
                "Write your result in your session dir instead of \"{raw}\": workspace artifacts belong to the user, \
                 so agents read them but never write, move, or delete them. Propose a change in your report."
            ),
        ));
    }

    // Rule B5: the engine writes the books from the docs stage's submit calls.
    if inside(&roots.books, target) {
        return Some(deny(
            WORKSPACE_DOCS,
            format!(
                "Put the documentation in your submit call instead of writing \"{raw}\": Ostra writes the \
                 workspace books from the docs stage's submit calls, so agents read them but never write, move, or \
                 delete them."
            ),
        ));
    }

    // Hardening: git metadata names programs git runs later (hooks, fsmonitor, a gitfile
    // pointing at another git dir), so it changes only through git commands.
    if target.components().any(|c| c.as_os_str() == ".git") {
        return Some(deny(
            GIT_METADATA,
            format!(
                "Use git commands instead of writing \"{raw}\" directly: files under .git decide which programs git \
                 runs, so Ostra refuses direct writes there in every mode."
            ),
        ));
    }

    // State ownership (ledger-policy.js).
    if roots.is_engine_state(target) {
        let reason = if roots.is_memory_db(target) {
            format!(
                "Record lessons with the Memory tool instead of writing \"{raw}\": the lesson store is written only by that \
                 tool, because later sessions recall it as fact."
            )
        } else {
            format!(
                "Do not write \"{raw}\": it is engine-owned session state, a record of something that already happened. \
                 A hand-written value would forge a pipeline decision rather than record one, so do the underlying work \
                 and the engine records it."
            )
        };
        return Some(deny(STATE_OWNERSHIP, reason));
    }
    for (pattern, stakes) in TOOL_OWNED.iter() {
        if pattern.is_match(&base) {
            return Some(deny(
                STATE_OWNERSHIP,
                format!(
                    "Do not write \"{base}\": it is {stakes}. A hand-authored value would forge a record rather than make \
                     one."
                ),
            ));
        }
    }
    for owned in AGENT_OWNED.iter() {
        if owned.pattern.is_match(&base) && !ctx.has(owned.grant) {
            return Some(deny(
                STATE_OWNERSHIP,
                format!(
                    "Leave \"{base}\" to the agents that hold `{}`, which {agent} does not. {}, so only the agent that did \
                     the work may write it. Put what you need to say in your own report instead.",
                    grant_name(owned.grant),
                    capitalize(owned.stakes)
                ),
            ));
        }
    }

    // Artifact ownership (artifact-guard.js). Fact-check's snapshot copies are not the artifact.
    let in_snapshot = target.ancestors().skip(1).any(|a| {
        a.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("factcheck-snapshot-"))
    });
    if !in_snapshot {
        for (pattern, grant, why) in ARTIFACTS.iter() {
            if pattern.is_match(&base) && !ctx.has(*grant) {
                return Some(deny(
                    ARTIFACT_OWNERSHIP,
                    format!(
                        "Leave \"{base}\" to an agent that holds `{}`: {why}. Say what should change in your report instead.",
                        grant_name(*grant)
                    ),
                ));
            }
        }
        // Document tool (HANDOVER 10.4): the markdown is rendered from the typed document.
        if roots.in_session(target) && ostra_core::doc::doc_kind_for(target).is_some() {
            return Some(deny(
                DOCUMENT_TOOL,
                format!(
                    "Change \"{base}\" with the Document tool instead, sending `update` for a revision: Ostra renders \
                     this file from its typed document, so a direct write would be overwritten and the browser would not \
                     show it."
                ),
            ));
        }
    }

    // Write scope (scope-policy.js checkScope).
    if let Some(d) = check_scope(ctx, roots, target, raw) {
        return Some(d);
    }

    // Report path (report-policy.js checkReportWrite).
    if let Some(declared) = &roots.report_file
        && inside(&roots.session_root, target)
        && base.starts_with("ostra-")
        && !AGENT_OWNED
            .iter()
            .any(|o| o.pattern.is_match(&base) && ctx.has(o.grant))
    {
        if target != declared.as_path() {
            // Rule G1: without tool enforcement any `ostra-*` name is allowed.
            if !roots.strict {
                return None;
            }
            return Some(deny(
                REPORT_PATH,
                format!(
                    "Write your report to \"{}\" instead: \"{base}\" is not this execution's declared report path, and the \
                     next stage reads that exact path. Any mechanism may write it, including a shell heredoc or chunked \
                     appends if one large write stalls.",
                    disp(declared)
                ),
            ));
        }
        if let Some(rec) = pending_lesson {
            return Some(lesson_denial(rec, true));
        }
    }
    None
}

/// Rule O2: only the implementer of a phase the approved plan puts in a project that does not
/// exist yet creates a project, only that one, and only with a well-formed call, so the user is
/// never asked about one that cannot run. (Ostra; no Ultracode source.)
pub fn check_manage(ctx: &ExecContext, tool: &str, input: &serde_json::Value) -> Option<Denial> {
    use ostra_core::manage::{PROJECT_CREATE, ProjectCreateInput};
    if tool != PROJECT_CREATE {
        return None;
    }
    // Rule CA6: the capability grants the tools; the phase decides whether it may create.
    if !ctx.has(Capability::ManageProjects) || !ctx.creates_project || ctx.session_id.is_none() {
        return Some(deny(
            MANAGE_TOOLS,
            format!(
                "Work in the projects already in scope: {} does not create a project here, because only a run that holds `manage_projects` in a phase the approved plan puts in a new project creates it.",
                ctx.agent
            ),
        ));
    }
    let req = match ProjectCreateInput::parse(input) {
        Ok(r) => r,
        Err(e) => return Some(deny(MANAGE_TOOLS, e)),
    };
    (req.key != ctx.project_key).then(|| {
        deny(
            MANAGE_TOOLS,
            format!(
                "Call ProjectCreate with key `{}`, the project your phase is in: `{}` is not it.",
                ctx.project_key, req.key
            ),
        )
    })
}

/// Rule O2: a run whose project does not exist yet writes nothing outside its session dir and
/// temp until it creates that project, because no project root exists to hold its work.
fn check_creates_project(ctx: &ExecContext, roots: &Roots, path: &Path) -> Option<Denial> {
    if !ctx.creates_project || roots.in_session(path) || roots.in_temp(path) {
        return None;
    }
    Some(deny(
        MANAGE_TOOLS,
        format!(
            "Call ProjectCreate for `{}` first, then end your run: the project does not exist yet, so nothing is written for this phase until it is created and initialized.",
            ctx.project_key
        ),
    ))
}

/// The `Document` tool's target: its owner writes it, inside this execution's session dir.
pub fn check_document(
    ctx: &ExecContext,
    roots: &Roots,
    target: &Path,
    raw: &str,
) -> Option<Denial> {
    if roots.is_protected(target) || roots.is_engine_state(target) {
        return check_write(ctx, roots, target, raw, None);
    }
    let base = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    for (pattern, grant, why) in ARTIFACTS.iter() {
        if pattern.is_match(&base) && !ctx.has(*grant) {
            return Some(deny(
                ARTIFACT_OWNERSHIP,
                format!(
                    "Leave \"{base}\" to an agent that holds `{}`: {why}. Say what should change in your report instead.",
                    grant_name(*grant)
                ),
            ));
        }
    }
    if roots.strict && !inside(&roots.session_dir, target) {
        return Some(deny(
            WRITE_SCOPE,
            format!(
                "Write the document inside \"{}\", this execution's session dir: \"{raw}\" is outside it, and the next \
                 stage reads the session dir.",
                disp(&roots.session_dir)
            ),
        ));
    }
    None
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn check_scope(ctx: &ExecContext, roots: &Roots, target: &Path, raw: &str) -> Option<Denial> {
    let agent = ctx.agent;
    let in_session = roots.in_session(target);
    // Rule CA6: test files need the `test_files` grant.
    if !ctx.has(Capability::TestFiles)
        && !in_session
        && let Ok(rel) = target.strip_prefix(&roots.repo)
        && is_test_path(&rel.to_string_lossy())
    {
        return Some(deny(
            NO_TESTS,
            format!(
                "Leave test files to the test stage: \"{raw}\" is a test file or directory path, and {agent} does not hold \
                 `test_files` (implementer Constraint 6). Tests run only after the user asks for them at the closing gate."
            ),
        ));
    }
    // Rule G1: the rest is write scope, which applies only with tool enforcement.
    if !roots.strict {
        return None;
    }
    // Rule CA2: the scope comes from the agent's definition, built-in or custom.
    let scope = ctx.scope();
    if scope == WriteScope::ReadOnly {
        return Some(deny(
            WRITE_SCOPE,
            format!(
                "Put your answer in your submit call instead of writing \"{raw}\": {agent} is read-only."
            ),
        ));
    }
    if in_session {
        return None;
    }
    if !roots.in_repo(target) {
        if scope == WriteScope::Session && roots.in_temp(target) {
            return None;
        }
        return Some(deny(
            WRITE_SCOPE,
            format!(
                "Write only inside the repo root \"{}\" or your session dir \"{}\": \"{raw}\" is outside both.",
                disp(&roots.repo),
                disp(&roots.session_dir)
            ),
        ));
    }
    // The target is resolved through symlinks, so the dirs it is compared with are too: a skills
    // dir may be a link to `skills/` or `.claude/skills/` elsewhere in the project.
    let skills = canon(&paths::project_skills_dir(&roots.repo));
    let legacy_skills = canon(&roots.repo.join(paths::LEGACY_SKILLS_DIR));
    // HANDOVER rule I2: the initializer writes skills only to `.agents/skills/`.
    let setup = scope == WriteScope::Setup;
    if setup && inside(&legacy_skills, target) && !inside(&skills, target) {
        return Some(deny(
            WRITE_SCOPE,
            format!(
                "Write the skill under \"{}\" instead: \"{raw}\" is in the older skills dir, which Ostra reads but \
                 never writes new skills to.",
                disp(&paths::project_skills_dir(&roots.repo))
            ),
        ));
    }
    let extra: Option<Vec<PathBuf>> =
        setup.then(|| vec![canon(&paths::project_runtime(&roots.repo)), skills]);
    if let Some(roots_allowed) = extra {
        if roots_allowed.iter().any(|r| inside(r, target)) {
            return None;
        }
        let list = roots_allowed
            .iter()
            .map(|r| format!("\"{}\"", disp(r)))
            .collect::<Vec<_>>()
            .join(" or ");
        return Some(deny(
            WRITE_SCOPE,
            format!(
                "Write only inside {list} or your session dir \"{}\": \"{raw}\" is outside the scope of {agent}.",
                disp(&roots.session_dir)
            ),
        ));
    }
    if scope == WriteScope::Session {
        return Some(deny(
            WRITE_SCOPE,
            format!(
                "Write only inside your session dir \"{}\": \"{raw}\" is project source, and {agent} never modifies \
                 project source.",
                disp(&roots.session_dir)
            ),
        ));
    }
    None
}

pub fn lesson_denial(rec: &crate::build::Recovery, report: bool) -> Denial {
    let what = if report { "your report" } else { "your result" };
    deny(
        LESSON_GATE,
        format!(
            "Record the lesson with the Memory tool before submitting {what}: you recovered from {} consecutive build \
             failures on \"{}\" and have not recorded what fixed it. Use area = the affected module and lesson = the \
             diagnostic and the correct pattern. If the fix was situational and teaches nothing reusable, call the \
             Report tool with `reason` saying so. This diagnostic recurs across sessions, and recording it stops the next \
             run re-deriving it.",
            rec.streak, rec.signature
        ),
    )
}

// ---------------------------------------------------------------------------------------------
// Shell analysis
// ---------------------------------------------------------------------------------------------

/// A resolved write target of a shell command.
#[derive(Debug, Clone)]
pub struct ShellTarget {
    pub command: usize,
    pub path: PathBuf,
    pub raw: String,
    /// On Windows, the raw path has a `..` right after a junction or symlink, so Win32 and Git
    /// Bash may reach different files ([`paths::dot_dot_after_link`]).
    pub ambiguous: bool,
}

/// Hardening (Windows): refuses a shell write target whose `..` follows a link, because Git Bash
/// may apply it to the link's target while the guard resolves it as Win32 does.
pub fn check_ambiguous_target(t: &ShellTarget) -> Option<Denial> {
    t.ambiguous.then(|| {
        deny(
            WINDOWS_PATH,
            format!(
                "Name \"{}\" without a `..` after a junction or symlink: Git Bash and Windows programs resolve that \
                 `..` differently, so Ostra cannot tell which file the command writes.",
                t.raw
            ),
        )
    })
}

/// Resolve every write target of a parsed command against the starting cwd.
pub fn shell_targets(roots: &Roots, parsed: &Parsed, start: &Path) -> Vec<ShellTarget> {
    bash::write_targets(parsed)
        .into_iter()
        .map(|t| {
            let base = cwd_for(roots, start, t.cwd.as_deref());
            let (path, raw) = match t.spec {
                TargetSpec::Path(p) => (roots.resolve(&base, &p), p),
                TargetSpec::CopyInto {
                    dest,
                    src_name,
                    force_dir,
                } => {
                    let d = roots.resolve(&base, &dest);
                    if force_dir || d.is_dir() {
                        (
                            d.join(&src_name),
                            format!("{}/{src_name}", dest.trim_end_matches('/')),
                        )
                    } else {
                        (d, dest)
                    }
                }
                TargetSpec::GitTree(dir) => {
                    let raw = dir.unwrap_or_else(|| ".".into());
                    (roots.resolve(&base, &raw), raw)
                }
            };
            let ambiguous = paths::dot_dot_after_link(&base, &paths::from_msys(&raw));
            ShellTarget {
                command: t.command,
                path,
                raw,
                ambiguous,
            }
        })
        .collect()
}

pub fn cwd_for(roots: &Roots, start: &Path, cd: Option<&str>) -> PathBuf {
    match cd {
        None => start.to_path_buf(),
        Some(c) => roots.resolve(start, c),
    }
}

// Commands that only read what they are pointed at (plugin-policy.js READ_ONLY_COMMANDS).
const PLAIN_READERS: &[&str] = &[
    "cat",
    "bat",
    "head",
    "tail",
    "less",
    "more",
    "nl",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ag",
    "ack",
    "ls",
    "find",
    "fd",
    "tree",
    "stat",
    "wc",
    "file",
    "du",
    "cksum",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "diff",
    "cmp",
    "jq",
    "yq",
    "cut",
    "tr",
    "sort",
    "uniq",
    "strings",
    "xxd",
    "od",
    "readlink",
    "realpath",
    "dirname",
    "basename",
    "test",
    "true",
    "echo",
    "printf",
    "sed",
];

fn is_plain_reader(cmd: &SimpleCommand) -> bool {
    let Some(name) = cmd.effective_name() else {
        return false;
    };
    if !PLAIN_READERS.contains(&name.as_str()) {
        return false;
    }
    if name == "sed" {
        return !cmd.args().iter().any(|w| {
            let t = w.text();
            t.starts_with('-') && !t.starts_with("--") && t.contains('i')
                || t.starts_with("--in-place")
        });
    }
    true
}

static PATH_CANDIDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[~\w./@-]*/[~\w./@-]+").unwrap());

/// A Windows path in shell text: a drive path (`C:\x`, `C:/x`) or a backslash path (`.git\x`,
/// `dir\file`). Matched only on Windows, in addition to [`PATH_CANDIDATE`].
static WIN_PATH_CANDIDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z]:[\\/][\w.\\/@ -]*|[\w.@-]*\\[\w.\\/@-]+").unwrap());

fn path_candidates(text: &str) -> Vec<String> {
    let mut out: Vec<String> = PATH_CANDIDATE
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    if cfg!(windows) {
        out.extend(
            WIN_PATH_CANDIDATE
                .find_iter(text)
                .map(|m| m.as_str().to_string()),
        );
    }
    out = out
        .into_iter()
        .map(|s| {
            s.trim_end_matches([')', ',', ';', ':', '\'', '"', '`', ' '])
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();
    out.dedup();
    out
}

static WRITE_API: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:writeFileSync|writeFile|appendFileSync|appendFile|createWriteStream|copyFileSync|copyFile|renameSync|rename|unlinkSync|unlink|rmSync|rmdirSync|mkdirSync|mkdtempSync|truncateSync|ftruncate|chmodSync|utimesSync|write_text|write_bytes|writelines|os\.remove|os\.unlink|os\.rename|os\.replace|os\.mkdir|os\.makedirs|os\.rmdir|os\.chmod|Deno\.writeTextFile|Deno\.writeFile|Deno\.remove|File\.write|IO\.write|file_put_contents|fopen)\b|\bshutil\.|\bFileUtils\.",
    )
    .unwrap()
});
static PY_OPEN_WRITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bopen\s*\([^)]*['"][rbt]*[wax][rbt+]*['"]"#).unwrap());
static SPAWN_API: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:child_process|execSync|execFileSync|spawnSync|execFile|subprocess|os\.system|os\.popen|popen|Deno\.Command|Deno\.run|Process\.spawn|shell_exec|passthru|proc_open)\b",
    )
    .unwrap()
});

/// Layer 1 for an opaque shell (PowerShell, cmd), which Ostra cannot parse into commands. The
/// write-scope and read guards need a parsed command, so instead the raw text is scanned for the
/// paths that must never be touched however they are reached: Ostra's own state and databases, a
/// credential store, and the tool's own binary or config. A hit is refused; anything else falls
/// to the permission layer, which never auto-allows these tools. `start` is the shell's cwd.
pub fn check_opaque_shell(roots: &Roots, raw: &str, start: &Path) -> Option<Denial> {
    if let Some(m) = STATE_NAME.find(raw) {
        return Some(deny(
            STATE_OWNERSHIP,
            format!(
                "Do not name \"{}\" in a PowerShell or cmd command: it is pipeline state, and such a command is invisible \
                 to Ostra's write guards, so it cannot be allowed to touch one. Do the work that makes the record update \
                 itself, or use the Bash tool for a command Ostra can check.",
                m.as_str().trim_start_matches(['/', '\\'])
            ),
        ));
    }
    for candidate in path_candidates(raw) {
        let value = candidate
            .split_once('=')
            .map_or(candidate.as_str(), |(k, v)| {
                if k.starts_with('-') { v } else { &candidate }
            });
        let p = roots.resolve(start, value);
        if roots.is_secret(&p) {
            return check_read(roots, &p, value);
        }
        if roots.is_engine_state(&p) || (roots.strict && roots.is_protected(&p)) {
            return Some(deny(
                SELF_PROTECTION,
                format!(
                    "Do not name \"{value}\" in a PowerShell or cmd command: it is part of Ostra itself or its engine \
                     state, which such a command cannot be allowed to touch because Ostra cannot read what it does. Use \
                     the Bash tool, or say what you need in your report."
                ),
            ));
        }
    }
    None
}

/// Guards that read a whole shell command: running Ostra itself, touching its files with
/// anything but a reader, and opaque interpreter write channels (plugin-policy.js).
pub fn check_shell(roots: &Roots, parsed: &Parsed, start: &Path) -> Option<Denial> {
    let cwds = bash::command_cwds(parsed);
    for (i, cmd) in parsed.commands.iter().enumerate() {
        let base = cwd_for(roots, start, cwds[i].as_deref());
        if let Some(idx) = cmd.effective_index() {
            let word = cmd.words[idx].text();
            let runs_ostra = bash::basename(word) == "ostra"
                || (word.contains('/') && roots.is_protected(&roots.resolve(&base, word)));
            if runs_ostra && roots.strict {
                return Some(deny(
                    SELF_PROTECTION,
                    "Do not run Ostra's own binary from a tool call: it is what enforces the pipeline, so running it \
                     would let a caller grant itself what the guards withhold. If Ostra is behaving wrongly, say so in \
                     your report."
                        .into(),
                ));
            }
        }

        if let Some((channel, code)) = bash::inline_code(cmd) {
            if let Some(m) = STATE_NAME.find(&code) {
                return Some(deny(
                    STATE_OWNERSHIP,
                    format!(
                        "Do not name \"{}\" in {channel} handed to an interpreter: it is pipeline state whose only author \
                         is the engine or its owning agent, and code passed this way is invisible to the write guards, so a \
                         value written from here would forge a pipeline decision. Do the work that makes the record update \
                         itself.",
                        m.as_str().trim_start_matches('/')
                    ),
                ));
            }
            // Rule G1: without tool enforcement a script may write and spawn; the sandbox bounds it.
            if roots.strict && (WRITE_API.is_match(&code) || PY_OPEN_WRITE.is_match(&code)) {
                return Some(deny(
                    SELF_PROTECTION,
                    format!(
                        "Use the Write or Edit tool, or a plain shell redirect naming the path, instead of {channel} that \
                         writes to the filesystem: that channel is invisible to Ostra's write guards, because they read the \
                         paths a tool call names and there are none here."
                    ),
                ));
            }
            if roots.strict && SPAWN_API.is_match(&code) {
                return Some(deny(
                    SELF_PROTECTION,
                    format!(
                        "Run the command you want directly, as its own tool call, instead of {channel} that spawns another \
                         process: a spawned child is invisible to Ostra's guards."
                    ),
                ));
            }
        }

        if is_plain_reader(cmd) {
            continue;
        }
        let mut text = cmd.text.clone();
        if let Some(body) = &cmd.heredoc_body
            && cmd
                .effective_name()
                .is_some_and(|n| bash::is_interpreter(&n))
        {
            text.push('\n');
            text.push_str(body);
        }
        for candidate in path_candidates(&text) {
            let p = roots.resolve(&base, &candidate);
            if roots.is_engine_state(&p) {
                let name = cmd.effective_name().unwrap_or_default();
                let hint = if roots.is_memory_db(&p) {
                    "Use MemoryRecall to read lessons and Memory to record one"
                } else {
                    "Read it only with a plain reader such as cat, grep, or head"
                };
                return Some(deny(
                    STATE_OWNERSHIP,
                    format!(
                        "{hint}: this command touches \"{candidate}\" with `{name}`, and engine state is a record of what \
                         already happened, written only by the engine."
                    ),
                ));
            }
            if roots.strict && roots.is_protected(&p) {
                let name = cmd.effective_name().unwrap_or_default();
                return Some(deny(
                    SELF_PROTECTION,
                    format!(
                        "Read Ostra's own files only with a plain reader such as cat, grep, or head: this command touches \
                         \"{candidate}\" with `{name}`, and Ostra's binary, configuration, databases, and engine state are \
                         read-only to agents."
                    ),
                ));
            }
        }
    }
    None
}

pub const IGNORED_SEARCH: &str = "ignored-search";

/// Entries a walking command's check looks at before it stops and refuses.
const IGNORED_WALK_BUDGET: usize = 20_000;

/// Rule G2: a sandboxed Grep or Glob never searches a path a `.*ignore` file hides.
pub fn check_ignored_search_tool(roots: &Roots, target: &Path, raw: &str) -> Option<Denial> {
    let ignores = roots.ignore_files();
    let file = ignores.hidden_by(target)?;
    Some(ignored_denial(
        raw,
        &file,
        "Search a path no ignore file hides",
    ))
}

/// Rule G2: a sandboxed shell search never lists a path a `.*ignore` file hides. Searchers that
/// read ignore files (`rg`, `fd`, `ag`) are refused when told to skip them or pointed at a hidden
/// path; walkers that read none (`grep -r`, `find`, `tree`, `ls -R`, `ack`) are refused when their
/// tree holds a hidden path.
pub fn check_ignored_search_shell(roots: &Roots, parsed: &Parsed, start: &Path) -> Option<Denial> {
    let cwds = bash::command_cwds(parsed);
    let ignores = roots.ignore_files();
    for (i, cmd) in parsed.commands.iter().enumerate() {
        let Some(name) = cmd.effective_name() else {
            continue;
        };
        let args = cmd.args();
        let base = cwd_for(roots, start, cwds[i].as_deref());
        let kind = match name.as_str() {
            "rg" | "fd" | "fdfind" | "ag" => SearchKind::ReadsIgnores,
            "grep" | "egrep" | "fgrep" if grep_recurses(args) => SearchKind::Walks,
            "rgrep" | "find" | "tree" | "ack" | "ack-grep" => SearchKind::Walks,
            "ls" if has_short(args, 'R') || has_long(args, "--recursive") => SearchKind::Walks,
            "git" => {
                if let Some(flag) = git_lists_ignored(args) {
                    return Some(deny(
                        IGNORED_SEARCH,
                        format!(
                            "Drop `{flag}` from the git command: it lists files the repository's ignore files hide, \
                             and a sandboxed agent never searches those."
                        ),
                    ));
                }
                continue;
            }
            _ => continue,
        };
        if kind == SearchKind::ReadsIgnores
            && let Some(flag) = skips_ignores(&name, args)
        {
            return Some(deny(
                IGNORED_SEARCH,
                format!(
                    "Run `{name}` without `{flag}`: that flag makes it search files the `.*ignore` files hide, and a \
                     sandboxed agent never searches those."
                ),
            ));
        }
        let mut operands: Vec<(PathBuf, String)> = search_operands(&name, args)
            .into_iter()
            .map(|w| (roots.resolve(&base, w.text()), w.text().to_string()))
            .filter(|(p, _)| p.exists())
            .collect();
        if operands.is_empty() {
            operands.push((base.clone(), ".".into()));
        }
        for (path, raw) in operands {
            if let Some(file) = ignores.hidden_by(&path) {
                return Some(ignored_denial(
                    &raw,
                    &file,
                    "Search a path no ignore file hides",
                ));
            }
            if kind == SearchKind::Walks && path.is_dir() {
                let correction = format!(
                    "Use Grep, Glob, `rg`, or `rg --files` instead of `{name}` on \"{raw}\""
                );
                match ignores.first_hidden_under(&path, IGNORED_WALK_BUDGET) {
                    Under::Clean => {}
                    Under::Hidden(p) => {
                        let shown = p.strip_prefix(&path).unwrap_or(&p).display().to_string();
                        return Some(deny(
                            IGNORED_SEARCH,
                            format!(
                                "{correction}: `{name}` reads no ignore files, so it would search \"{shown}\", which \
                                 an ignore file hides, and a sandboxed agent never searches those."
                            ),
                        ));
                    }
                    Under::TooLarge => {
                        return Some(deny(
                            IGNORED_SEARCH,
                            format!(
                                "{correction}: `{name}` reads no ignore files, and the tree is too large to confirm \
                                 that no ignore file hides part of it."
                            ),
                        ));
                    }
                }
            }
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchKind {
    ReadsIgnores,
    Walks,
}

fn ignored_denial(raw: &str, file: &Path, correction: &str) -> Denial {
    deny(
        IGNORED_SEARCH,
        format!(
            "{correction}: \"{raw}\" is hidden by {}, and a sandboxed agent never searches what an ignore file \
             hides. Read a file you already know the path of with Read instead.",
            file.display()
        ),
    )
}

/// The words a search command walks: every positional word for `tree` and `ls`, the words before
/// the first expression for `find`, and the positional words after the pattern for the rest.
fn search_operands<'a>(name: &str, args: &'a [bash::Word]) -> Vec<&'a bash::Word> {
    let plain = |w: &&bash::Word| w.is_static() && !w.glob;
    match name {
        "find" => args
            .iter()
            .take_while(|w| !w.text().starts_with('-') && !matches!(w.text(), "(" | "!"))
            .filter(plain)
            .collect(),
        "tree" | "ls" => args
            .iter()
            .filter(plain)
            .filter(|w| !w.text().starts_with('-'))
            .collect(),
        _ => {
            let words: Vec<&bash::Word> =
                args.iter().filter(|w| !w.text().starts_with('-')).collect();
            let given = ["-e", "-f", "--regexp", "--file", "--files"];
            let pattern_given = args.iter().any(|w| {
                given
                    .iter()
                    .any(|g| w.text() == *g || w.text().starts_with(&format!("{g}=")))
            });
            let skip = usize::from(!pattern_given);
            words.into_iter().skip(skip).filter(plain).collect()
        }
    }
}

fn has_short(args: &[bash::Word], flag: char) -> bool {
    args.iter().any(|w| {
        let t = w.text();
        t.len() > 1 && t.starts_with('-') && !t.starts_with("--") && t[1..].contains(flag)
    })
}

fn has_long(args: &[bash::Word], prefix: &str) -> bool {
    args.iter().any(|w| w.text().starts_with(prefix))
}

fn grep_recurses(args: &[bash::Word]) -> bool {
    has_short(args, 'r')
        || has_short(args, 'R')
        || has_long(args, "--recursive")
        || has_long(args, "--dereference-recursive")
        || has_long(args, "--directories=recurse")
        || args
            .windows(2)
            .any(|w| w[0].text() == "-d" && w[1].text() == "recurse")
}

/// The flag that turns off a searcher's own ignore files, if any.
fn skips_ignores(name: &str, args: &[bash::Word]) -> Option<String> {
    let shorts: &[char] = match name {
        "rg" => &['u'],
        "fd" | "fdfind" => &['u', 'I'],
        _ => &['u', 'U'],
    };
    let longs: &[&str] = match name {
        "ag" => &["--unrestricted", "--skip-vcs-ignores"],
        _ => &["--unrestricted", "--no-ignore"],
    };
    args.iter().map(|w| w.text()).find_map(|t| {
        let short = t.len() > 1
            && t.starts_with('-')
            && !t.starts_with("--")
            && t[1..].chars().any(|c| shorts.contains(&c));
        let long = longs.iter().any(|l| t.starts_with(l));
        (short || long).then(|| t.to_string())
    })
}

/// The flag of a git command that lists or searches ignored files.
fn git_lists_ignored(args: &[bash::Word]) -> Option<String> {
    let (sub, _) = bash::git_subcommand(args);
    let words: Vec<&str> = args.iter().map(|w| w.text()).collect();
    let find = |fs: &[&str]| {
        words
            .iter()
            .find(|w| fs.iter().any(|f| w.starts_with(f)))
            .map(|w| w.to_string())
    };
    match sub.as_deref() {
        Some("status") => find(&["--ignored"]),
        Some("grep") => find(&["--no-exclude-standard"]),
        Some("ls-files") => find(&["--ignored"])
            .or_else(|| words.contains(&"-i").then(|| "-i".to_string()))
            .or_else(|| {
                let others = words.iter().any(|w| *w == "-o" || *w == "--others");
                let excluded = words
                    .iter()
                    .any(|w| *w == "--exclude-standard" || w.starts_with("--exclude"));
                (others && !excluded).then(|| "--others".to_string())
            }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths() {
        assert!(is_test_path("src/App.test.ts"));
        assert!(is_test_path(
            "core/src/test/java/com/x/OrderServiceTest.java"
        ));
        assert!(is_test_path("tests/test_api.py"));
        assert!(is_test_path("pkg/foo_test.go"));
        assert!(is_test_path("src/__tests__/x.js"));
        assert!(!is_test_path("src/App.ts"));
        assert!(!is_test_path("src/testing/helpers.ts"));
        assert!(!is_test_path("src/contest.ts"));
    }

    #[test]
    fn state_name_pattern() {
        for name in [
            "knowledge.sqlite3",
            "ostra-review-ledger.md",
            "ostra-review-ledger-phase-2.md",
            "ostra-security-block.json",
            "ostra-implementer-progress-phase-1.md",
            "workspace.db",
            "/s/.state/x",
        ] {
            assert!(STATE_NAME.is_match(name), "{name}");
        }
        assert!(!STATE_NAME.is_match("ostra-spec-2026-01-01-topic.md"));
    }
}
