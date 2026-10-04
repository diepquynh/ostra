//! Test stage evals over `tests/evals/test_stage.toml`: the execution path analyzer and write-test, on real models.
//! Live and paid, so the live test is ignored by default:
//!
//! ```text
//! OSTRA_EVAL_MODELS=anthropic:claude-opus-5-5,anthropic:claude-sonnet-5-5 OSTRA_EVAL_RUNS=3 \
//!   cargo test -p ostra-server --test test_stage_evals -- --ignored --nocapture
//! ```
//!
//! Each case is a real session of a real `Engine` on a small project laid out from `tests/evals/test_stage/`. It
//! reaches the test stage either after a scripted implementation phase, which the engine stages, or as a new TEST
//! session in which no implementer ran, on a repository whose git history holds what an earlier session left. A
//! router executor sends the runs the case lists as live (`epa`, `write-test`, or both) to the native loop on the
//! model under test, with the policy, the sandbox, a code index, and the coordination tools, and plays every other
//! run from the case: the implementer applies the change, and a golden analysis stands in for a scripted analyzer.
//!
//! An analyzer run passes when its report is at the declared path, it changed no project file, the report holds
//! each expected fact, and the grader model (`OSTRA_EVAL_GRADER`, default Opus) finds the rubric met. A write-test
//! run passes when its status is the expected one, it changed only test files and listed all of them, each expected
//! level got a test, the project's test types pass, every planted mutant of the source fails them, the files it
//! must keep hold their text, and the grader agrees. write-test on a golden analysis is reported as `write-test`; after a live
//! analyzer, as `stage`. The report gives pass rates per tier, role, and model, with cost, and is written to
//! `target/evals/`. `OSTRA_EVAL_CASES` filters case ids by substring, `OSTRA_EVAL_TIERS` by tier, `OSTRA_EVAL_JOBS`
//! sets how many sessions run at once (default 3). The test fails only when no live run happened.
//!
//! The offline test replays every case with stand-ins (the golden analysis and the golden tests), checks that each
//! live run is reached and graded clean, that the project's own tests let every mutant survive, and that the golden
//! tests kill every mutant, so a broken case costs nothing to find.

// The fixtures' test commands run through `sh`.
#![cfg(unix)]

use chrono::Utc;
use futures::StreamExt;
use ostra_core::agent::{AgentName, Capability};
use ostra_core::api::{CreateSession, FileIndex};
use ostra_core::config::{
    GlobalConfig, ProjectEntry, ProjectProfile, ResolvedRoute, WorkspaceSettings, load_toml,
    save_toml,
};
use ostra_core::coord::{CoordReply, SEND_MESSAGE};
use ostra_core::event::{ExecPurpose, SessionOptions};
use ostra_core::exec::{
    CancellationToken, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec,
    ExecutionStatus, Executor, Usage,
};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, SessionId, WorkspaceId};
use ostra_core::model::Effort;
use ostra_core::paths;
use ostra_core::policy::{PermissionAnswer, PolicyDecision, RuleRef, ToolCall};
use ostra_core::submit::ReportSubmit;
use ostra_engine::factory::AgentsFactory;
use ostra_engine::state::SessionState;
use ostra_engine::{Engine, Notice, Services, SpawnFactory};
use ostra_exec_native::NativeExecutor;
use ostra_store::WorkspaceDb;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct File {
    case: Vec<Case>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    tier: u8,
    note: String,
    project: String,
    #[serde(default)]
    others: Vec<String>,
    /// `phase`: a scripted implementation phase reaches the test stage. `request`: a TEST session.
    mode: String,
    #[serde(default = "light")]
    track: String,
    request: String,
    /// Overlays in the first commit.
    #[serde(default)]
    setup: Vec<String>,
    /// Paths removed before the first commit.
    #[serde(default)]
    remove: Vec<String>,
    /// Overlays committed after the first commit, in order: what earlier work left.
    #[serde(default)]
    history: Vec<Commit>,
    /// An overlay left staged and uncommitted.
    #[serde(default)]
    staged: Option<String>,
    /// The overlay the scripted implementer applies.
    #[serde(default)]
    change: Option<String>,
    #[serde(default)]
    implementer_report: Option<String>,
    #[serde(default)]
    spec: Option<String>,
    #[serde(default)]
    phase_file: Option<String>,
    live: Vec<String>,
    golden: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    epa: Option<EpaExpect>,
    #[serde(default)]
    write_test: Option<WtExpect>,
    #[serde(default)]
    mutant: Vec<Mutant>,
}

fn light() -> String {
    "light".into()
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Commit {
    apply: String,
    message: String,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct EpaExpect {
    #[serde(default)]
    has: Vec<Vec<String>>,
    #[serde(default)]
    lacks: Vec<String>,
    cause: String,
    rubric: String,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct WtExpect {
    status: String,
    /// Whether the test types in `run` pass before write-test: `passes` or `fails`.
    #[serde(default = "passes")]
    baseline: String,
    #[serde(default)]
    levels: Vec<Vec<String>>,
    /// The test types the grading runs; all of the project's when empty.
    #[serde(default)]
    run: Vec<String>,
    /// Text each file must still hold after the run.
    #[serde(default)]
    files: Vec<FileHas>,
    #[serde(default)]
    has: Vec<Vec<String>>,
    #[serde(default)]
    stuck_has: Vec<Vec<String>>,
    cause: String,
    rubric: String,
}

fn passes() -> String {
    "passes".into()
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct FileHas {
    path: String,
    has: Vec<String>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Mutant {
    id: String,
    file: String,
    find: String,
    replace: String,
    note: String,
}

const EPA: &str = "epa";
const WRITE_TEST: &str = "write-test";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    repo_root().join("tests/evals/test_stage")
}

fn load_cases() -> File {
    let text = std::fs::read_to_string(repo_root().join("tests/evals/test_stage.toml")).unwrap();
    toml::from_str(&text).unwrap()
}

/// Scenarios live under the target dir, never `/tmp`, because the sandbox mounts its own `/tmp`.
fn scratch_root() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("test-stage-evals")
}

/// Missing groups: each group is a set of alternatives, and the text must contain one of each.
fn missing(text: &str, groups: &[Vec<String>]) -> Vec<String> {
    let t = text.to_lowercase();
    groups
        .iter()
        .filter(|g| !g.iter().any(|alt| t.contains(&alt.to_lowercase())))
        .map(|g| g.join(" | "))
        .collect()
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let head: String = s.chars().take(n).collect();
    format!("{head}\n[... clipped]")
}

fn tail(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars[chars.len().saturating_sub(n)..].iter().collect()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(
        dir,
        &[
            "-c",
            "user.name=Ada Lovelace",
            "-c",
            "user.email=ada@example.invalid",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

/// Every file under `dir`, relative, sorted.
fn walk(dir: &Path) -> Vec<String> {
    fn go(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                go(root, &p, out);
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    let mut out = vec![];
    go(dir, dir, &mut out);
    out.sort();
    out
}

/// Copies every file of `src` over `dest`, and returns their relative paths.
fn copy_over(src: &Path, dest: &Path) -> Vec<String> {
    assert!(src.is_dir(), "no fixture folder {}", src.display());
    let files = walk(src);
    for f in &files {
        let to = dest.join(f);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(src.join(f), &to).unwrap();
    }
    files
}

fn overlay(project: &str, name: &str) -> PathBuf {
    fixtures().join("changes").join(project).join(name)
}

fn golden_dir(case: &Case) -> PathBuf {
    fixtures().join("goldens").join(&case.golden)
}

/// The repository's files and contents, without what builds and runs leave behind.
type Tree = BTreeMap<String, Vec<u8>>;

fn tree(repo: &Path) -> Tree {
    walk(repo)
        .into_iter()
        .filter(|f| {
            !(f.starts_with(".git/")
                || f.starts_with(".ostra/memory/")
                || f.starts_with("node_modules/")
                || f.contains("/node_modules/")
                || f.contains("__pycache__/")
                || f.ends_with(".pyc")
                || f.starts_with("test-results/")
                || f.starts_with("playwright-report/"))
        })
        .map(|f| {
            let bytes = std::fs::read(repo.join(&f)).unwrap_or_default();
            (f, bytes)
        })
        .collect()
}

fn diff(before: &Tree, after: &Tree) -> Vec<String> {
    let mut out: Vec<String> = before
        .keys()
        .chain(after.keys())
        .filter(|k| before.get(*k) != after.get(*k))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `**/` matches any number of folders, `**` anything, `*` anything but `/`, `?` one character but `/`.
fn glob_match(pattern: &str, path: &str) -> bool {
    fn m(p: &[u8], s: &[u8]) -> bool {
        if p.is_empty() {
            return s.is_empty();
        }
        if p.starts_with(b"**/") {
            let rest = &p[3..];
            return m(rest, s)
                || s.iter()
                    .enumerate()
                    .any(|(i, c)| *c == b'/' && m(rest, &s[i + 1..]));
        }
        if p.starts_with(b"**") {
            return (0..=s.len()).any(|i| m(&p[2..], &s[i..]));
        }
        match p[0] {
            b'*' => {
                let stop = s.iter().position(|c| *c == b'/').unwrap_or(s.len());
                (0..=stop).any(|i| m(&p[1..], &s[i..]))
            }
            b'?' => !s.is_empty() && s[0] != b'/' && m(&p[1..], &s[1..]),
            c => !s.is_empty() && s[0] == c && m(&p[1..], &s[1..]),
        }
    }
    m(pattern.as_bytes(), path.as_bytes())
}

fn profile_of(repo: &Path) -> ProjectProfile {
    load_toml(&paths::project_profile(repo)).expect("the fixture's project.toml parses")
}

/// The test types a case runs, with their commands.
fn run_types(w: &WtExpect, profile: &ProjectProfile) -> Vec<(String, String)> {
    let names: Vec<String> = if w.run.is_empty() {
        profile.test_types.keys().cloned().collect()
    } else {
        w.run.clone()
    };
    names
        .into_iter()
        .map(|n| {
            let cmd = profile
                .test_types
                .get(&n)
                .and_then(|t| t.command.clone())
                .unwrap_or_else(|| panic!("no command for test type {n}"));
            (n, cmd)
        })
        .collect()
}

/// Runs one command in `dir` with a time limit, and returns whether it passed and its output.
fn sh(dir: &Path, cmd: &str, limit: Duration) -> (bool, String) {
    use std::os::unix::process::CommandExt;
    let out_path = std::env::temp_dir().join(format!("ostra-eval-{}.out", ExecutionId::new()));
    let out = std::fs::File::create(&out_path).unwrap();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(out.try_clone().unwrap()))
        .stderr(Stdio::from(out))
        .process_group(0)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if Instant::now() > deadline {
            let _ = Command::new("kill")
                .args(["-9", "--", &format!("-{}", child.id())])
                .status();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let text = std::fs::read_to_string(&out_path).unwrap_or_default();
    let _ = std::fs::remove_file(&out_path);
    match status {
        Some(s) => (s.success(), text),
        None => (
            false,
            format!("{text}\n[timed out after {}s]", limit.as_secs()),
        ),
    }
}

const COMMAND_LIMIT: Duration = Duration::from_secs(240);

/// Runs every test type; the first failure and its output, or `None`.
fn run_all(dir: &Path, types: &[(String, String)]) -> Option<(String, String)> {
    types.iter().find_map(|(name, cmd)| {
        let (ok, out) = sh(dir, cmd, COMMAND_LIMIT);
        (!ok).then(|| (name.clone(), out))
    })
}

/// Whether the test types pass with one mutant applied; the source is restored either way.
fn survives(dir: &Path, m: &Mutant, types: &[(String, String)]) -> Result<bool, String> {
    let path = dir.join(&m.file);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", m.file))?;
    let n = text.matches(&m.find).count();
    if n != 1 {
        return Err(format!(
            "mutant {}: its text occurs {n} times in {}",
            m.id, m.file
        ));
    }
    std::fs::write(&path, text.replacen(&m.find, &m.replace, 1)).unwrap();
    let alive = run_all(dir, types).is_none();
    std::fs::write(&path, &text).unwrap();
    Ok(alive)
}

/// Whether this machine has the program a project's tests run with.
fn has_runner(project: &str) -> bool {
    let program = if project == "notes" {
        "node"
    } else {
        "python3"
    };
    sh(
        Path::new("."),
        &format!("command -v {program}"),
        Duration::from_secs(10),
    )
    .0
}

// ---------------------------------------------------------------------------------------------
// The scenario: a project, an engine, and the router
// ---------------------------------------------------------------------------------------------

struct Vars(Vec<(&'static str, String)>);

impl Vars {
    fn fill(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (k, v) in &self.0 {
            out = out.replace(k, v);
        }
        out
    }
}

fn vars(st: &SessionState, repo: &Path, ws: &Path, key: &str, report: Option<&Path>) -> Vars {
    Vars(vec![
        ("{repo}", repo.display().to_string()),
        ("{root}", st.session_root.display().to_string()),
        (
            "{session}",
            st.project_session_dir(key).display().to_string(),
        ),
        ("{ws}", ws.display().to_string()),
        (
            "{report}",
            report.map(|p| p.display().to_string()).unwrap_or_default(),
        ),
    ])
}

fn kind_of(p: &ExecPurpose) -> String {
    serde_json::to_value(p)
        .ok()
        .and_then(|v| v["kind"].as_str().map(String::from))
        .unwrap_or_default()
}

/// The role a run plays: the analysis or the test writing, not a consult of either.
fn role_of(agent: AgentName, kind: &str) -> Option<&'static str> {
    match (agent, kind) {
        (AgentName::ExecutionPathAnalyzer, "epa") => Some(EPA),
        (AgentName::WriteTest, "write_test") => Some(WRITE_TEST),
        _ => None,
    }
}

fn role_agent(role: &str) -> AgentName {
    if role == EPA {
        AgentName::ExecutionPathAnalyzer
    } else {
        AgentName::WriteTest
    }
}

/// One run as the router saw it.
#[derive(Clone)]
struct RunLog {
    agent: AgentName,
    kind: String,
    live: bool,
    status: ExecutionStatus,
    submit: Option<Value>,
    error: Option<String>,
    usage: Usage,
    calls: Vec<String>,
    denials: Vec<String>,
    steps: Vec<ToolStep>,
    secs: f64,
    report: Option<PathBuf>,
}

impl RunLog {
    fn role(&self) -> Option<&'static str> {
        role_of(self.agent, &self.kind)
    }
}

/// What the project's own tests do before write-test, in the offline replay.
#[derive(Clone, Debug, Default)]
struct Baseline {
    passes: bool,
    failure: String,
    /// Mutant id, and whether it survived the project's own tests.
    mutants: Vec<(String, Result<bool, String>)>,
}

/// One tool call and what it returned.
#[derive(Clone, Debug)]
struct ToolStep {
    tool: String,
    input: String,
    output: Option<String>,
    is_error: bool,
}

/// Passes everything to the engine's host and keeps what the run did.
struct Tap {
    inner: Arc<dyn ExecutionHost>,
    calls: Mutex<Vec<String>>,
    denials: Mutex<Vec<String>>,
    steps: Mutex<Vec<(String, ToolStep)>>,
}

#[async_trait::async_trait]
impl ExecutionHost for Tap {
    fn emit(&self, delta: ExecutionDelta) {
        match &delta {
            ExecutionDelta::ToolCall { call_id, call } => {
                let input: String = call.input.to_string().chars().take(300).collect();
                self.calls.lock().push(format!("{} {input}", call.tool));
                let input = match call.input.get("command").and_then(Value::as_str) {
                    Some(cmd) => cmd.to_string(),
                    None => call.input.to_string(),
                };
                self.steps.lock().push((
                    call_id.clone(),
                    ToolStep {
                        tool: call.tool.clone(),
                        input,
                        output: None,
                        is_error: false,
                    },
                ));
            }
            ExecutionDelta::ToolResult {
                call_id,
                output,
                is_error,
                ..
            } => {
                if let Some((_, s)) = self
                    .steps
                    .lock()
                    .iter_mut()
                    .rev()
                    .find(|(id, _)| id == call_id)
                {
                    s.output = Some(output.clone());
                    s.is_error = *is_error;
                }
            }
            ExecutionDelta::Policy {
                decision: PolicyDecision::Deny { rule, reason },
                ..
            } => {
                let reason: String = reason.chars().take(200).collect();
                self.denials
                    .lock()
                    .push(format!("{} {}: {reason}", rule.layer, rule.rule));
            }
            _ => {}
        }
        self.inner.emit(delta);
    }

    async fn ask_permission(
        &self,
        call: &ToolCall,
        reason: &str,
        rule: &RuleRef,
    ) -> PermissionAnswer {
        self.inner.ask_permission(call, reason, rule).await
    }

    fn record_message(&self, role: &str, content: &Value) {
        self.inner.record_message(role, content);
    }

    fn transcript(&self, execution: &ExecutionId) -> Vec<(String, Value)> {
        self.inner.transcript(execution)
    }

    fn yolo(&self) -> bool {
        self.inner.yolo()
    }

    async fn wait_for_wake(&self) -> Option<ostra_core::exec::Wake> {
        self.inner.wait_for_wake().await
    }
}

fn text_blocks(text: &str) -> Value {
    json!([{"type": "text", "text": text}])
}

type Slot = Arc<OnceLock<(Engine, SessionId)>>;

/// Plays the case's scripted runs and sends its live runs to the native loop.
struct Router {
    case: Case,
    key: String,
    repo: PathBuf,
    ws: PathBuf,
    /// The scenario folder.
    dir: PathBuf,
    model: String,
    dry: bool,
    slot: Slot,
    live: Option<NativeExecutor>,
    runs: Mutex<Vec<RunLog>>,
    stop: Mutex<Option<String>>,
    /// The repository before and after each role's run.
    trees: Mutex<BTreeMap<&'static str, (Tree, Tree)>>,
    baseline: Mutex<Option<Baseline>>,
    /// The session stops after this role's run.
    last: &'static str,
}

impl Router {
    fn stop(&self, why: String) {
        let mut s = self.stop.lock();
        if s.is_none() {
            *s = Some(why);
        }
    }

    fn is_live(&self, agent: AgentName) -> bool {
        !self.dry && self.case.live.iter().any(|r| role_agent(r) == agent) && self.live.is_some()
    }

    /// The offline replay's check of the project's own tests, before the golden tests go in.
    fn check_baseline(&self) {
        let Some(w) = &self.case.write_test else {
            return;
        };
        let types = run_types(w, &profile_of(&self.repo));
        let failure = run_all(&self.repo, &types);
        let mutants = self
            .case
            .mutant
            .iter()
            .map(|m| (m.id.clone(), survives(&self.repo, m, &types)))
            .collect();
        *self.baseline.lock() = Some(Baseline {
            passes: failure.is_none(),
            failure: failure
                .map(|(t, o)| format!("{t}: {}", tail(&o, 600)))
                .unwrap_or_default(),
            mutants,
        });
    }

    async fn canned(
        &self,
        spec: &ExecutionSpec,
        host: &Tap,
        st: &SessionState,
        kind: &str,
    ) -> ExecutionResult {
        let report = spec.ctx.report_file.clone();
        let v = vars(st, &self.repo, &self.ws, &self.key, report.as_deref());
        // A continued conversation carries the one it continues, as the native loop copies it.
        if let Some(res) = &spec.resume
            && res.from != spec.id
        {
            for (role, content) in host.transcript(&res.from) {
                host.record_message(&role, &content);
            }
        }
        let incoming = spec
            .resume
            .as_ref()
            .and_then(|r| r.note.clone())
            .unwrap_or_else(|| spec.first_message.clone());
        host.record_message("user", &text_blocks(&incoming));
        let ok = |status: ExecutionStatus, submit: Value| ExecutionResult {
            status,
            submit: Some(submit),
            final_text: String::new(),
            usage: Usage::default(),
            native_session_id: None,
            error: None,
        };
        // Rule SM6: a sender that waits gets a plain reply, so it goes on; a run continued only
        // for its messages ends there.
        let owed = st.owed_by(&spec.id);
        if !owed.is_empty() || kind == "message" {
            let text = "Nothing to add beyond my report.";
            host.record_message("assistant", &text_blocks(text));
            let (engine, session) = self.slot.get().unwrap();
            for to in &owed {
                if let Err(e) = engine.coordinate(
                    session,
                    &spec.id,
                    SEND_MESSAGE,
                    &json!({"message": text, "to": to.as_str()}),
                ) {
                    return ExecutionResult::error(format!("eval: the reply was refused: {e}"));
                }
            }
            if kind == "message" {
                return ok(
                    ExecutionStatus::Ok,
                    ostra_core::coord::end_payload(SEND_MESSAGE, text),
                );
            }
        }
        host.record_message("assistant", &text_blocks("Done."));
        let write = |path: &Path, text: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let report_or = |fallback: &str| {
            report
                .clone()
                .unwrap_or_else(|| spec.ctx.session_dir.join(fallback))
        };
        let submit = match spec.agent {
            AgentName::Explore => {
                let path = spec.ctx.session_dir.join("ostra-research-1.md");
                write(
                    &path,
                    &format!(
                        "# Research\n\nThe request: {}\n\nThe code it touches is in `{}`; `README.md` describes each module.\n",
                        self.case.request,
                        self.repo.display()
                    ),
                );
                json!({"research_path": path, "scope_covered": "The task as asked.",
                    "findings_summary": "See the research document.", "sources_retrieved": 1,
                    "open_questions": 0, "not_covered": []})
            }
            AgentName::GenerateSpec => {
                let path = st.session_root.join("ostra-spec-1.md");
                write(
                    &path,
                    &v.fill(self.case.spec.as_deref().unwrap_or("# Spec\n")),
                );
                json!({"spec_path": path, "open_questions": [], "external_evidence_rows": 0,
                    "deliverables": 1, "requirements": 1, "summary": "A spec for the request."})
            }
            AgentName::FactCheck => {
                let target = st
                    .executions
                    .get(&spec.id)
                    .and_then(|x| x.params["target_type"].as_str().map(String::from))
                    .unwrap_or_else(|| "spec".into());
                json!({"verdict": "PASS", "target": target, "findings": []})
            }
            AgentName::Plan => {
                let master = st.session_root.join("ostra-plan-1.md");
                let phase = st.session_root.join("ostra-plan-1-phase-1.md");
                write(&master, "# Plan\n\nOne phase; see its phase file.\n");
                write(
                    &phase,
                    &v.fill(self.case.phase_file.as_deref().unwrap_or("# Phase 1\n")),
                );
                json!({"spec_path": st.session_root.join("ostra-spec-1.md"), "master_plan_path": master,
                    "phases": [{"id": 1, "deliverable": "D1", "project": self.key, "title": "The change",
                        "complexity": "Medium", "test_policy": "Required", "depends_on": [], "file": phase}],
                    "stakes": "High", "summary": "One phase.", "step_count": 3, "requirement_coverage": "1 of 1"})
            }
            AgentName::Implementer => {
                let Some(change) = &self.case.change else {
                    return ExecutionResult::error(
                        "eval: an implementer ran in a case with no change",
                    );
                };
                let files = copy_over(&overlay(&self.case.project, change), &self.repo);
                let path = report_or("ostra-implementer-phase-1.md");
                write(
                    &path,
                    &v.fill(
                        self.case
                            .implementer_report
                            .as_deref()
                            .unwrap_or("# Report\n"),
                    ),
                );
                json!({"status": "ok", "report_path": path, "changed_files": files, "summary": "Implemented the change."})
            }
            AgentName::CodeReviewer => json!({"findings": [], "security_block": false,
                "ledger_path": spec.ctx.session_dir.join("ostra-review-ledger-phase-1.md"), "summary": "No findings."}),
            AgentName::ExecutionPathAnalyzer => {
                let path = report_or("ostra-epa-phase-1.md");
                let text = std::fs::read_to_string(golden_dir(&self.case).join("epa.md")).unwrap();
                write(&path, &v.fill(&text));
                json!({"status": "ok", "report_path": path, "changed_files": [],
                    "summary": "Analyzed the change: see the report."})
            }
            AgentName::WriteTest => {
                let dir = golden_dir(&self.case);
                // The project's own tests and the mutants run before the golden tests go in.
                tokio::task::block_in_place(|| self.check_baseline());
                let files = if dir.join("files").is_dir() {
                    copy_over(&dir.join("files"), &self.repo)
                } else {
                    vec![]
                };
                let path = report_or("ostra-write-test-phase-1.md");
                let text = std::fs::read_to_string(dir.join("write-test.md")).unwrap_or_else(|_| {
                    format!(
                        "# Test Report: {}\n**Status:** Complete\n\n## Verification Results\n| Verification | Level | Command | Result |\n| --- | --- | --- | --- |\n| Final suite | all | the project's test types | Pass |\n",
                        self.case.id
                    )
                });
                write(&path, &v.fill(&text));
                match std::fs::read_to_string(dir.join("write-test.json")) {
                    Ok(s) => match serde_json::from_str(&v.fill(&s)) {
                        Ok(v) => v,
                        Err(e) => {
                            return ExecutionResult::error(format!("eval: write-test.json: {e}"));
                        }
                    },
                    Err(_) => json!({"status": "ok", "report_path": path, "changed_files": files,
                        "summary": "Wrote the tests the analysis asks for. All verifications passed."}),
                }
            }
            other => return ExecutionResult::error(format!("eval: no scripted run for {other}")),
        };
        // The native loop ends a stuck submit as a stuck execution.
        let status = if submit["status"] == "stuck" {
            ExecutionStatus::Stuck
        } else {
            ExecutionStatus::Ok
        };
        ok(status, submit)
    }
}

#[async_trait::async_trait]
impl Executor for Router {
    async fn run(
        &self,
        mut spec: ExecutionSpec,
        host: Arc<dyn ExecutionHost>,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        let Some((engine, session)) = self.slot.get() else {
            return ExecutionResult::error("eval: no engine");
        };
        let st = engine.state(session).unwrap();
        let kind = st
            .executions
            .get(&spec.id)
            .map(|x| kind_of(&x.purpose))
            .unwrap_or_default();
        let scripted = matches!(
            spec.agent,
            AgentName::Explore
                | AgentName::GenerateSpec
                | AgentName::FactCheck
                | AgentName::Plan
                | AgentName::Implementer
                | AgentName::CodeReviewer
                | AgentName::ExecutionPathAnalyzer
        ) || (spec.agent == AgentName::WriteTest
            && self.case.live.iter().any(|l| l == WRITE_TEST));
        if !scripted {
            self.stop(format!("before {} ({kind})", spec.agent));
            let mut r = ExecutionResult::with_status(ExecutionStatus::Cancelled);
            r.error = Some("stopped by the eval".into());
            return r;
        }
        let tap = Arc::new(Tap {
            inner: host,
            calls: Mutex::new(vec![]),
            denials: Mutex::new(vec![]),
            steps: Mutex::new(vec![]),
        });
        let role = role_of(spec.agent, &kind);
        if let Some(r) = role {
            // Kept for whoever reads a run or writes a case: what the agent was told.
            let _ = std::fs::write(
                self.dir.join(format!("first-message-{r}.md")),
                &spec.first_message,
            );
        }
        let before = role.map(|_| tree(&self.repo));
        let live = self.is_live(spec.agent);
        let started = Instant::now();
        let result = if live {
            spec.route.model = self.model.clone();
            self.live
                .as_ref()
                .unwrap()
                .run(spec.clone(), tap.clone(), cancel)
                .await
        } else {
            self.canned(&spec, &tap, &st, &kind).await
        };
        if let (Some(r), Some(b)) = (role, before) {
            self.trees.lock().insert(r, (b, tree(&self.repo)));
        }
        let status = result.status;
        self.runs.lock().push(RunLog {
            agent: spec.agent,
            kind: kind.clone(),
            live,
            status,
            submit: result.submit.clone(),
            error: result.error.clone(),
            usage: result.usage,
            calls: tap.calls.lock().clone(),
            denials: tap.denials.lock().clone(),
            steps: tap.steps.lock().iter().map(|(_, s)| s.clone()).collect(),
            secs: started.elapsed().as_secs_f64(),
            report: spec.ctx.report_file.clone(),
        });
        if role == Some(self.last) && status != ExecutionStatus::Waiting {
            self.stop(format!("after {}", self.last));
        }
        result
    }
}

/// The coordination tools of a live run, wired to its engine as the server wires them.
struct Coord(Slot);

struct ExecCoord {
    engine: Engine,
    session: SessionId,
    execution: ExecutionId,
}

impl ostra_tools::CoordConnector for Coord {
    fn open(&self, spec: &ExecutionSpec) -> Option<Arc<dyn ostra_tools::Coordinate>> {
        if !spec.capabilities.contains(&Capability::Coordinate) {
            return None;
        }
        let (engine, session) = self.0.get()?;
        Some(Arc::new(ExecCoord {
            engine: engine.clone(),
            session: session.clone(),
            execution: spec.id.clone(),
        }))
    }
}

#[async_trait::async_trait]
impl ostra_tools::Coordinate for ExecCoord {
    async fn call(&self, tool: &str, input: &Value) -> Result<CoordReply, String> {
        self.engine
            .coordinate(&self.session, &self.execution, tool, input)
    }
}

/// The code navigation tools over the project, served the way the server's index serves them.
struct Nav {
    root: PathBuf,
    list: Arc<FileIndex>,
    indexes: Arc<ostra_code::Indexes<()>>,
}

#[async_trait::async_trait]
impl ostra_tools::CodeNav for Nav {
    async fn call(&self, _repo: &Path, tool: &str, input: &Value) -> Result<String, String> {
        let (root, list, indexes) = (self.root.clone(), self.list.clone(), self.indexes.clone());
        let (tool, input) = (tool.to_string(), input.clone());
        tokio::task::spawn_blocking(move || {
            indexes.with(&(), &root, &list, |ix| {
                ostra_code::tools::run(ix, &tool, &input)
            })
        })
        .await
        .map_err(|e| format!("The code index failed: {e}"))?
    }
}

struct EvalServices {
    settings: WorkspaceSettings,
    router: Arc<Router>,
    model: String,
    key: String,
    case: Case,
}

#[async_trait::async_trait]
impl Services for EvalServices {
    fn global(&self) -> GlobalConfig {
        let mut g = GlobalConfig::default();
        if let Some(t) = g.tiers.get_mut("native") {
            t.fast = Some(self.model.clone());
            t.balanced = Some(self.model.clone());
            t.advanced = Some(self.model.clone());
            t.frontier = Some(self.model.clone());
        }
        g
    }
    fn workspace(&self) -> WorkspaceSettings {
        self.settings.clone()
    }
    fn executor(&self, kind: ExecutorKind) -> Option<Arc<dyn Executor>> {
        (kind == ExecutorKind::Native).then(|| self.router.clone() as Arc<dyn Executor>)
    }
    fn factory(&self) -> Arc<dyn SpawnFactory> {
        Arc::new(AgentsFactory)
    }
    async fn judge(
        &self,
        _route: &ResolvedRoute,
        _system: &str,
        _user: &str,
        schema: Value,
        _e: Effort,
    ) -> Result<(Value, Usage), String> {
        let props = &schema["properties"];
        let out = if props.get("category").is_some() {
            let request = self.case.mode == "request";
            let tasks: Vec<Value> = if request {
                vec![]
            } else {
                vec![json!({"project": self.key, "task": "Research the code the request touches."})]
            };
            json!({"category": if request { "TEST" } else { "IMPLEMENT" }, "projects": [self.key],
                "explore_tasks": tasks, "opts_in": {"tests": true, "docs": true},
                "reason": "Scripted by the eval.", "title": "Eval"})
        } else if props.get("track").is_some() {
            json!({"track": self.case.track, "reason": "Scripted by the eval."})
        } else if props.get("stakes").is_some() {
            json!({"stakes": "high", "reason": "Scripted by the eval."})
        } else if props.get("report_markdown").is_some() {
            json!({"report_markdown": "# Done", "reason": "Scripted by the eval."})
        } else if let Some(answer) = props.get("answer") {
            match answer["properties"]["kind"]["const"].as_str() {
                Some("approval") => {
                    json!({"answer": {"kind": "approval", "approved": true}, "reason": "Scripted."})
                }
                Some("choice") => {
                    let options: Vec<&str> = answer["properties"]["option"]["enum"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    let option = if options.is_empty() || options.contains(&"done") {
                        "done"
                    } else {
                        options[0]
                    };
                    json!({"answer": {"kind": "choice", "option": option}, "reason": "Scripted."})
                }
                _ => return Err(format!("eval: unscripted YOLO answer {schema}")),
            }
        } else {
            return Err(format!("eval: unscripted judge {schema}"));
        };
        Ok((out, Usage::default()))
    }
    fn notify(&self, _notice: Notice) {}
    fn protected_paths(&self) -> Vec<PathBuf> {
        vec![paths::global_config_path(), paths::data_dir()]
    }
    fn command_approved(&self, _: &Path, _: &str) -> bool {
        true
    }
    fn add_allow_rule(&self, _rule: &str) {}
}

/// Everything one session left behind.
struct Session {
    router: Arc<Router>,
    stop: String,
    repo: PathBuf,
    ws: PathBuf,
    secs: f64,
}

/// Lays out the workspace: the project with its history, the other projects, and the state an earlier session left.
fn lay_out(case: &Case, dir: &Path) -> (PathBuf, PathBuf) {
    let ws = dir.join("ws");
    let repo = ws.join(&case.project);
    std::fs::create_dir_all(&repo).unwrap();
    copy_over(&fixtures().join("projects").join(&case.project), &repo);
    for s in &case.setup {
        copy_over(&overlay(&case.project, s), &repo);
    }
    for r in &case.remove {
        let p = repo.join(r);
        if p.is_dir() {
            std::fs::remove_dir_all(&p).unwrap();
        } else {
            std::fs::remove_file(&p).unwrap_or_else(|e| panic!("{}: remove {r}: {e}", case.id));
        }
    }
    git(&repo, &["init", "-q", "-b", "main"]);
    commit(&repo, "Initial commit");
    for c in &case.history {
        copy_over(&overlay(&case.project, &c.apply), &repo);
        commit(&repo, &c.message);
    }
    if let Some(s) = &case.staged {
        copy_over(&overlay(&case.project, s), &repo);
        git(&repo, &["add", "-A"]);
    }
    for other in &case.others {
        let o = ws.join(other);
        copy_over(&fixtures().join("projects").join(other), &o);
        git(&o, &["init", "-q", "-b", "main"]);
        commit(&o, "Initial commit");
    }
    (ws, repo)
}

async fn run_session(
    providers: Option<Arc<ostra_providers::Providers>>,
    case: &Case,
    model: &str,
    dir: &Path,
) -> Session {
    let started = Instant::now();
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let dir = paths::canonical(dir).unwrap();
    let (ws, repo) = lay_out(case, &dir);
    let key = case.project.clone();

    let slot: Slot = Arc::new(OnceLock::new());
    let live = providers.map(|p| {
        let files: Vec<String> = walk(&repo)
            .into_iter()
            .filter(|f| !f.starts_with(".git/"))
            .collect();
        let nav = Nav {
            root: repo.clone(),
            list: Arc::new(FileIndex {
                paths: files,
                truncated: false,
            }),
            indexes: Arc::new(ostra_code::Indexes::default()),
        };
        NativeExecutor::new(p, ostra_server::app::skill_resolver(), Some(Arc::new(nav)))
            .with_coord(Arc::new(Coord(slot.clone())))
    });
    let last = if case.live.iter().any(|l| l == WRITE_TEST) {
        WRITE_TEST
    } else {
        EPA
    };
    let router = Arc::new(Router {
        case: case.clone(),
        key: key.clone(),
        repo: repo.clone(),
        ws: ws.clone(),
        dir: dir.clone(),
        model: model.to_string(),
        dry: live.is_none(),
        slot: slot.clone(),
        live,
        runs: Mutex::new(vec![]),
        stop: Mutex::new(None),
        trees: Mutex::new(BTreeMap::new()),
        baseline: Mutex::new(None),
        last,
    });
    let mut settings = WorkspaceSettings::seeded("eval");
    settings.limits.max_parallel_executions = 2;
    settings.limits.session_budget_usd = 15.0;
    for (k, path) in std::iter::once((key.clone(), repo.clone()))
        .chain(case.others.iter().map(|o| (o.clone(), ws.join(o))))
    {
        settings.projects.push(ProjectEntry {
            key: k,
            path,
            stack: None,
            code_provider: None,
            language_servers: vec![],
        });
    }
    let services = Arc::new(EvalServices {
        settings,
        router: router.clone(),
        model: model.to_string(),
        key: key.clone(),
        case: case.clone(),
    });
    let db = WorkspaceDb::open_in_memory().unwrap();
    let engine = Engine::new(ws.clone(), WorkspaceId::new(), db, services);
    let summary = engine
        .create_session(CreateSession {
            request: case.request.clone(),
            options: SessionOptions {
                tests: true,
                docs: true,
                yolo: true,
                track: None,
            },
            projects: vec![key.clone()],
            files: vec![],
            uploads: vec![],
            docs_book: None,
            workflow: None,
        })
        .unwrap();
    let session = summary.id.clone();
    let _ = slot.set((engine.clone(), session.clone()));

    let dry = router.dry;
    let timeout = Duration::from_secs(case.timeout_secs.unwrap_or(if dry { 120 } else { 3600 }));
    let stall = Duration::from_secs(if dry { 20 } else { 300 });
    let mut rx = engine.subscribe();
    let mut last_seen = (0usize, 0i64);
    let mut last_change = Instant::now();
    let stop = loop {
        if let Some(why) = router.stop.lock().clone() {
            break why;
        }
        let st = engine.state(&session).unwrap();
        if st.is_terminal() {
            break format!(
                "session ended: {}",
                st.failed.clone().unwrap_or_else(|| "completed".into())
            );
        }
        if started.elapsed() > timeout {
            break "timeout".into();
        }
        let now = (router.runs.lock().len(), st.last_seq);
        if now != last_seen {
            last_seen = now;
            last_change = Instant::now();
        } else if st.running_executions().next().is_none() && last_change.elapsed() > stall {
            break "stalled: nothing ran".into();
        }
        let _ = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
    };
    if !engine.state(&session).unwrap().is_terminal() {
        let _ = engine.stop_session(&session);
    }
    // Let cancelled runs return, so their results are in the log.
    let settle = Instant::now();
    while engine
        .state(&session)
        .unwrap()
        .running_executions()
        .next()
        .is_some()
        && settle.elapsed() < Duration::from_secs(30)
    {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Session {
        router,
        stop,
        repo,
        ws,
        secs: started.elapsed().as_secs_f64(),
    }
}

// ---------------------------------------------------------------------------------------------
// Grading
// ---------------------------------------------------------------------------------------------

fn run_of(s: &Session, role: &str) -> Option<RunLog> {
    s.router
        .runs
        .lock()
        .iter()
        .find(|r| r.role() == Some(role))
        .cloned()
}

fn submit_of(r: &RunLog) -> Result<ReportSubmit, String> {
    let v = r.submit.clone().ok_or("no submit")?;
    serde_json::from_value(v).map_err(|e| format!("the submit does not parse: {e}"))
}

/// The report at the declared path, or why there is none.
fn report_of(r: &RunLog, sub: &ReportSubmit) -> Result<String, String> {
    let declared = r.report.clone().ok_or("no declared report path")?;
    if Path::new(&sub.report_path) != declared {
        return Err(format!(
            "report_path is {}, not the declared {}",
            sub.report_path,
            declared.display()
        ));
    }
    std::fs::read_to_string(&declared).map_err(|_| format!("no report at {}", declared.display()))
}

/// Every tool call in order: a shell command in full (its middle clipped when long) with the end of its output,
/// and other tools by their input, so a claimed test run can be checked against what ran.
fn calls_record(r: &RunLog, ws: &Path) -> String {
    let root = ws.display().to_string();
    let short = |s: &str, head: usize, end: usize| -> String {
        let s = s.replace(&root, "{ws}");
        let n = s.chars().count();
        if n <= head + end {
            return s;
        }
        let a: String = s.chars().take(head).collect();
        format!("{a} [...] {}", tail(&s, end))
    };
    let mut out = String::new();
    for st in r.steps.iter().take(120) {
        if st.tool == "Bash" {
            out.push_str(&format!(
                "- Bash: `{}`\n",
                short(&st.input, 400, 400).replace('`', "'")
            ));
            match &st.output {
                Some(o) => out.push_str(&format!(
                    "  - {}output ends: {}\n",
                    if st.is_error { "ERROR, " } else { "" },
                    short(&tail(o, 500), 0, 500).replace('\n', " / ")
                )),
                None => out.push_str("  - no result recorded\n"),
            }
        } else {
            out.push_str(&format!("- {}: {}\n", st.tool, short(&st.input, 200, 60)));
        }
    }
    if r.steps.len() > 120 {
        out.push_str(&format!("- ... {} more\n", r.steps.len() - 120));
    }
    for d in &r.denials {
        out.push_str(&format!("- DENIED: {d}\n"));
    }
    out
}

/// The code checks of the analyzer's run, and the record the grader reads.
fn check_epa(case: &Case, s: &Session) -> (Vec<String>, String) {
    let e = case.epa.as_ref().expect("an epa expectation");
    let Some(r) = run_of(s, EPA) else {
        return (
            vec![format!("the analyzer never ran ({})", s.stop)],
            String::new(),
        );
    };
    let mut fails = vec![];
    if r.status != ExecutionStatus::Ok {
        fails.push(format!(
            "the analyzer ended {:?}: {}",
            r.status,
            r.error.clone().unwrap_or_default()
        ));
    }
    let sub = match submit_of(&r) {
        Ok(sub) => sub,
        Err(err) => {
            fails.push(err);
            return (fails, String::new());
        }
    };
    if !matches!(sub.status, ostra_core::submit::SubmitStatus::Ok) {
        fails.push(format!("the analyzer submitted {:?}", sub.status));
    }
    let text = match report_of(&r, &sub) {
        Ok(t) => t,
        Err(err) => {
            fails.push(err);
            String::new()
        }
    };
    if let Some((before, after)) = s.router.trees.lock().get(EPA) {
        let changed = diff(before, after);
        if !changed.is_empty() {
            fails.push(format!(
                "the analyzer changed project files: {}",
                changed.join(", ")
            ));
        }
    }
    for m in missing(&text, &e.has) {
        fails.push(format!("the report never mentions {m}"));
    }
    for l in e.lacks.iter().filter(|l| text.contains(l.as_str())) {
        fails.push(format!("the report contains `{l}`"));
    }
    let record = format!(
        "# The analyzer's report\n\n{}\n\n# Its submit\n\n```json\n{}\n```\n\n# Its tool calls, in order\n\n{}",
        clip(&text, 16000),
        serde_json::to_string_pretty(&r.submit).unwrap_or_default(),
        calls_record(&r, &s.ws)
    );
    (fails, record)
}

fn listed(sub: &ReportSubmit, repo: &Path) -> Vec<String> {
    let root = format!("{}/", repo.display());
    sub.changed_files
        .iter()
        .map(|f| {
            f.strip_prefix(&root)
                .unwrap_or(f)
                .trim_start_matches("./")
                .to_string()
        })
        .collect()
}

/// The code checks of write-test's run, and the record the grader reads.
fn check_wt(case: &Case, s: &Session) -> (Vec<String>, String) {
    let w = case.write_test.as_ref().expect("a write-test expectation");
    let Some(r) = run_of(s, WRITE_TEST) else {
        return (
            vec![format!("write-test never ran ({})", s.stop)],
            String::new(),
        );
    };
    let mut fails = vec![];
    // A stuck submit ends the execution as stuck.
    let expected = if w.status == "stuck" {
        ExecutionStatus::Stuck
    } else {
        ExecutionStatus::Ok
    };
    if r.status != expected {
        fails.push(format!(
            "write-test ended {:?}: {}",
            r.status,
            r.error.clone().unwrap_or_default()
        ));
    }
    let sub = match submit_of(&r) {
        Ok(sub) => sub,
        Err(err) => {
            fails.push(err);
            return (fails, String::new());
        }
    };
    let status = serde_json::to_value(sub.status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    if status != w.status {
        fails.push(format!(
            "write-test submitted {status}, expected {}",
            w.status
        ));
    }
    let report = report_of(&r, &sub).unwrap_or_else(|err| {
        fails.push(err);
        String::new()
    });
    let profile = profile_of(&s.repo);
    let (before, after) = s
        .router
        .trees
        .lock()
        .get(WRITE_TEST)
        .cloned()
        .unwrap_or_default();
    let changed = diff(&before, &after);
    let globs: Vec<&String> = profile
        .test_types
        .values()
        .flat_map(|t| &t.matches)
        .collect();
    for f in changed
        .iter()
        .filter(|f| !globs.iter().any(|g| glob_match(g, f)))
    {
        fails.push(format!("changed {f}, which is not a test file"));
    }
    let listed = listed(&sub, &s.repo);
    for f in changed
        .iter()
        .filter(|f| after.contains_key(*f) && !listed.contains(f))
    {
        fails.push(format!(
            "left {f} out of changed_files, so Ostra would not stage it"
        ));
    }
    for f in &w.files {
        let text = after
            .get(&f.path)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        for h in f.has.iter().filter(|h| !text.contains(h.as_str())) {
            fails.push(format!("{} no longer contains `{h}`", f.path));
        }
    }
    for group in &w.levels {
        let hit = changed.iter().filter(|f| after.contains_key(*f)).any(|f| {
            group.iter().any(|level| {
                profile
                    .test_types
                    .get(level)
                    .is_some_and(|t| t.matches.iter().any(|g| glob_match(g, f)))
            })
        });
        if !hit {
            fails.push(format!(
                "no new or changed test at level {}",
                group.join(" or ")
            ));
        }
    }
    for m in missing(&format!("{report}\n{}", sub.summary), &w.has) {
        fails.push(format!("the report and summary never mention {m}"));
    }
    let stuck = sub
        .stuck
        .as_ref()
        .map(|x| format!("{}\n{}", x.diagnostic, x.need))
        .unwrap_or_default();
    for m in missing(&format!("{stuck}\n{}", sub.summary), &w.stuck_has) {
        fails.push(format!("the stuck diagnostic and need never mention {m}"));
    }
    let mut facts = vec![];
    if w.status == "ok" && fails.is_empty() {
        let types = run_types(w, &profile);
        match run_all(&s.repo, &types) {
            Some((t, out)) => fails.push(format!(
                "the {t} tests fail after the run: {}",
                tail(&out, 500)
            )),
            None => {
                facts.push(format!(
                    "The grading ran the {} tests: they pass.",
                    types
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                for m in &case.mutant {
                    match survives(&s.repo, m, &types) {
                        Ok(true) => {
                            fails.push(format!("no test catches mutant {}: {}", m.id, m.note))
                        }
                        Ok(false) => facts.push(format!("Mutant {} ({}) is caught.", m.id, m.note)),
                        Err(e) => fails.push(e),
                    }
                }
            }
        }
    }
    let mut files = String::new();
    for f in changed.iter().filter(|f| after.contains_key(*f)) {
        let text = String::from_utf8_lossy(&after[f]).into_owned();
        let what = if before.contains_key(f) {
            "modified"
        } else {
            "created"
        };
        files.push_str(&format!(
            "## {f} ({what})\n\n```\n{}\n```\n\n",
            clip(&text, 6000)
        ));
    }
    let record = format!(
        "# write-test's report\n\n{}\n\n# Its submit\n\n```json\n{}\n```\n\n# The test files it changed\n\n{}# What the grading found\n\n{}\n\n# Its tool calls, in order\n\n{}",
        clip(&report, 10000),
        serde_json::to_string_pretty(&r.submit).unwrap_or_default(),
        clip(&files, 24000),
        facts.join("\n"),
        calls_record(&r, &s.ws)
    );
    (fails, record)
}

const GRADER: &str = "You grade one run of an agent of Ostra's test stage, the step that verifies an implementation.

The execution path analyzer reads a change (or, in a session that asks for tests directly, the request) and writes an analysis report: the execution paths of the changed functions, the system flows that reach them, the existing suites to re-run as regression, and a test level for each check. write-test reads that report, writes the tests at those levels, and runs them and the regression suites; it hands a regression it cannot fix back as stuck. The input gives the case, what the right behavior rests on, the rubric, and a record of the run: its report, its submit, the files it changed, what the grading's own test runs found, and its tool calls.

Apply the rubric literally. Pass only when every point holds in the record. A point the record does not show fails, even when the agent may have meant it. Judge meaning, not wording: a paraphrase that states the same fact passes, and extra correct detail does not hurt.

Call `decide` once with `verdict` (`pass` or `fail`) and `reason`: one or two sentences naming the rubric point that decided it.";

async fn grade(
    providers: &ostra_providers::Providers,
    grader: &str,
    case: &Case,
    role: &str,
    cause: &str,
    rubric: &str,
    record: &str,
) -> Result<((bool, String), f64), String> {
    let (p, m) = providers.for_model(grader).map_err(|e| e.to_string())?;
    let agent = if role == EPA {
        "the execution path analyzer"
    } else {
        "write-test"
    };
    let user = format!(
        "# Case\n\n{}\n\nThe request: {}\n\nThe run graded: {agent}.\n\n# What the right behavior rests on\n\n{cause}\n\n# Rubric\n\n{rubric}\n\n{record}",
        case.note, case.request
    );
    let schema = json!({"type": "object", "properties": {
        "verdict": {"type": "string", "enum": ["pass", "fail"]},
        "reason": {"type": "string"}
    }, "required": ["verdict", "reason"], "additionalProperties": false});
    let (out, usage) =
        ostra_providers::structured(p.as_ref(), &m, GRADER, &user, schema, Effort::Low)
            .await
            .map_err(|e| e.to_string())?;
    Ok((
        (
            out["verdict"].as_str() == Some("pass"),
            out["reason"].as_str().unwrap_or_default().to_string(),
        ),
        ostra_core::pricing::cost(&m, &usage, 0),
    ))
}

/// A live run that died on the provider, not on anything the model did.
fn infra_error(runs: &[RunLog]) -> Option<String> {
    runs.iter()
        .filter(|r| r.live && r.status == ExecutionStatus::Error)
        .filter_map(|r| r.error.clone())
        .find(|e| {
            let e = e.to_lowercase();
            [
                "model call failed",
                "provider error",
                "overloaded",
                "rate limit",
                "stream failed",
            ]
            .iter()
            .any(|m| e.contains(m))
        })
}

struct Outcome {
    case: String,
    tier: u8,
    model: String,
    /// `epa`, `write-test` on a golden analysis, or `stage`: write-test after a live analyzer.
    role: String,
    stop: String,
    fails: Vec<String>,
    grade: Option<(bool, String)>,
    grade_error: Option<String>,
    cost: f64,
    grader_cost: f64,
    secs: f64,
    dir: PathBuf,
    infra: Option<String>,
}

impl Outcome {
    fn pass(&self) -> bool {
        self.infra.is_none()
            && self.fails.is_empty()
            && self.grade.as_ref().is_some_and(|(ok, _)| *ok)
    }
}

fn run_json(r: &RunLog) -> Value {
    json!({
        "agent": r.agent.as_str(), "kind": r.kind, "live": r.live,
        "status": format!("{:?}", r.status).to_lowercase(), "error": r.error, "submit": r.submit,
        "cost_usd": r.usage.cost_usd, "input_tokens": r.usage.input_tokens,
        "cache_read_tokens": r.usage.cache_read_tokens, "output_tokens": r.usage.output_tokens,
        "seconds": r.secs, "tool_calls": r.calls, "policy_denials": r.denials,
    })
}

async fn run_one(
    providers: Arc<ostra_providers::Providers>,
    case: &Case,
    model: &str,
    grader: &str,
    dir: PathBuf,
) -> Vec<Outcome> {
    // A session whose live run died on the provider runs again, up to twice.
    let mut retries = 0;
    let (s, dir) = loop {
        let d = if retries == 0 {
            dir.clone()
        } else {
            PathBuf::from(format!("{}-retry{retries}", dir.display()))
        };
        let s = run_session(Some(providers.clone()), case, model, &d).await;
        let infra = infra_error(&s.router.runs.lock());
        match infra {
            Some(e) if retries < 2 => {
                println!(
                    "  {:<34} {:<40} provider error, running again: {}",
                    case.id,
                    model,
                    e.chars().take(120).collect::<String>()
                );
                retries += 1;
            }
            _ => break (s, d),
        }
    };
    let infra = infra_error(&s.router.runs.lock());
    let runs = s.router.runs.lock().clone();
    std::fs::write(
        dir.join("runs.json"),
        serde_json::to_string_pretty(&runs.iter().map(run_json).collect::<Vec<_>>()).unwrap(),
    )
    .unwrap();
    let mut out = vec![];
    for role in [EPA, WRITE_TEST] {
        if !case.live.iter().any(|l| l == role) {
            continue;
        }
        let (fails, record, cause, rubric) = if role == EPA {
            let (f, r) = tokio::task::block_in_place(|| check_epa(case, &s));
            let e = case.epa.as_ref().unwrap();
            (f, r, e.cause.clone(), e.rubric.clone())
        } else {
            let (f, r) = tokio::task::block_in_place(|| check_wt(case, &s));
            let w = case.write_test.as_ref().unwrap();
            (f, r, w.cause.clone(), w.rubric.clone())
        };
        std::fs::write(dir.join(format!("record-{role}.md")), &record).unwrap();
        let (grade, grade_error, grader_cost) = if fails.is_empty() && infra.is_none() {
            match self::grade(&providers, grader, case, role, &cause, &rubric, &record).await {
                Ok((g, c)) => (Some(g), None, c),
                Err(e) => (None, Some(e), 0.0),
            }
        } else {
            (None, None, 0.0)
        };
        let agent = role_agent(role);
        let mine: Vec<&RunLog> = runs.iter().filter(|r| r.live && r.agent == agent).collect();
        out.push(Outcome {
            case: case.id.clone(),
            tier: case.tier,
            model: model.to_string(),
            role: if role == WRITE_TEST && case.live.iter().any(|l| l == EPA) {
                "stage".into()
            } else {
                role.into()
            },
            stop: s.stop.clone(),
            fails,
            grade,
            grade_error,
            cost: mine.iter().map(|r| r.usage.cost_usd).sum(),
            grader_cost,
            secs: mine.iter().map(|r| r.secs).sum(),
            dir: dir.clone(),
            infra: infra.clone(),
        });
    }
    out
}

fn selected(file: &File) -> Vec<Case> {
    let filter = std::env::var("OSTRA_EVAL_CASES").unwrap_or_default();
    let tiers = std::env::var("OSTRA_EVAL_TIERS").unwrap_or_default();
    file.case
        .iter()
        .filter(|c| filter.split(',').any(|f| c.id.contains(f.trim())))
        .filter(|c| {
            tiers.trim().is_empty()
                || tiers
                    .split(',')
                    .any(|t| t.trim().parse::<u8>().ok() == Some(c.tier))
        })
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: runs Ostra's test stage agents on real models and costs money"]
async fn test_stage_evals() {
    let file = load_cases();
    let cases = selected(&file);
    let models: Vec<String> = std::env::var("OSTRA_EVAL_MODELS")
        .unwrap_or_else(|_| {
            "anthropic:claude-opus-5-5,anthropic:claude-sonnet-5-5,anthropic:claude-haiku-4-5-20251001"
                .into()
        })
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    let runs: usize = std::env::var("OSTRA_EVAL_RUNS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(3);
    let jobs: usize = std::env::var("OSTRA_EVAL_JOBS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(3);
    let grader =
        std::env::var("OSTRA_EVAL_GRADER").unwrap_or_else(|_| "anthropic:claude-opus-5-5".into());

    // Prices come from the server's cached models.dev catalog, read before the data dir moves.
    if let Some(c) = std::fs::read_to_string(ostra_server::prices::cache_path(&paths::data_dir()))
        .ok()
        .and_then(|t| ostra_core::pricing::Catalog::from_models_dev(&t).ok())
    {
        ostra_core::pricing::install(c);
    }
    let stamp = Utc::now().format("%Y%m%dT%H%M%S").to_string();
    let root = scratch_root().join(&stamp);
    std::fs::create_dir_all(&root).unwrap();
    // The data dir holds the sandbox's egress sockets, whose paths must stay short, and must not be under
    // `/tmp`, which the sandbox replaces with its own.
    let home = paths::home().expect("a home directory");
    let data = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"))
        .join("ostra-evals")
        .join(format!("t{stamp}"));
    std::fs::create_dir_all(&data).unwrap();
    let config = data.join("config.toml");
    save_toml(&config, &GlobalConfig::default()).unwrap();
    // SAFETY: set once, before any run starts; the offline test in this binary is not run with `--ignored`.
    unsafe {
        std::env::set_var("OSTRA_CONFIG", &config);
        std::env::set_var("OSTRA_DATA_DIR", data.join("data"));
        std::env::set_var("OSTRA_SANDBOX_CACHE", data.join("sandbox-cache"));
        std::env::set_var("OSTRA_MODELS_DEV_URL", "");
    }
    paths::ensure_data_dir().unwrap();
    let assets = data.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    ostra_agents::set_assets_dir(paths::canonical(&assets).unwrap());
    ostra_agents::materialize_assets(&assets).unwrap();
    let providers = Arc::new(ostra_providers::Providers::from_config(
        &GlobalConfig::default(),
        &Default::default(),
    ));

    let mut work = vec![];
    for run in 0..runs {
        for case in &cases {
            for model in &models {
                let short = model.split(':').next_back().unwrap_or(model);
                work.push((
                    case.clone(),
                    model.clone(),
                    root.join(format!("{}-{short}-{run}", case.id)),
                ));
            }
        }
    }
    println!(
        "\n{} sessions: {} cases x {} models x {runs}. Scenarios in {}",
        work.len(),
        cases.len(),
        models.len(),
        root.display()
    );
    let grader = grader.as_str();
    let outcomes: Vec<Outcome> = futures::stream::iter(work)
        .map(|(case, model, dir)| {
            let providers = providers.clone();
            async move {
                let os = run_one(providers, &case, &model, grader, dir).await;
                for o in &os {
                    println!(
                        "  {:<34} {:<40} {:<10} {:<5} {:>6.0}s ${:<6.3} {}",
                        o.case,
                        o.model,
                        o.role,
                        if o.infra.is_some() {
                            "INFRA"
                        } else if o.pass() {
                            "pass"
                        } else {
                            "FAIL"
                        },
                        o.secs,
                        o.cost,
                        o.stop
                    );
                }
                os
            }
        })
        .buffer_unordered(jobs)
        .flat_map(futures::stream::iter)
        .collect()
        .await;

    let mut by: BTreeMap<(String, String, String), Vec<&Outcome>> = BTreeMap::new();
    for o in &outcomes {
        by.entry((o.case.clone(), o.role.clone(), o.model.clone()))
            .or_default()
            .push(o);
    }
    let mut report = vec![];
    println!(
        "\n{:<34} {:<10} {:<40} {:>6}  failures",
        "case", "role", "model", "pass"
    );
    for case in &cases {
        for role in ["epa", "write-test", "stage"] {
            for model in &models {
                let Some(os) = by.get(&(case.id.clone(), role.to_string(), model.clone())) else {
                    continue;
                };
                let counted: Vec<&&Outcome> = os.iter().filter(|o| o.infra.is_none()).collect();
                let pass = counted.iter().filter(|o| o.pass()).count();
                let why: Vec<String> = os
                    .iter()
                    .filter(|o| !o.pass())
                    .map(|o| {
                        if let Some(e) = &o.infra {
                            format!(
                                "provider error on every attempt: {}",
                                e.chars().take(80).collect::<String>()
                            )
                        } else if !o.fails.is_empty() {
                            o.fails.join("; ")
                        } else if let Some((_, r)) = &o.grade {
                            format!("grader: {r}")
                        } else {
                            format!(
                                "grader error: {}",
                                o.grade_error.clone().unwrap_or_default()
                            )
                        }
                    })
                    .collect();
                println!(
                    "{:<34} {:<10} {:<40} {:>2}/{:<3}  {}",
                    case.id,
                    role,
                    model,
                    pass,
                    counted.len(),
                    why.join(" || ").chars().take(260).collect::<String>()
                );
                for o in os {
                    report.push(json!({
                        "case": o.case, "tier": o.tier, "role": o.role, "model": o.model, "pass": o.pass(),
                        "stop": o.stop, "failures": o.fails,
                        "grader": o.grade.as_ref().map(|(ok, r)| json!({"pass": ok, "reason": r})),
                        "grader_error": o.grade_error, "cost_usd": o.cost, "grader_cost_usd": o.grader_cost,
                        "seconds": o.secs, "scenario": o.dir, "infra_error": o.infra,
                    }));
                }
            }
        }
    }
    println!(
        "\n{:<40} {:>7} {:>7} {:>7} {:>7} {:>11} {:>7} {:>7} {:>9} {:>9} {:>9}",
        "model",
        "tier 1",
        "tier 2",
        "tier 3",
        "epa",
        "write-test",
        "stage",
        "all",
        "cost",
        "grading",
        "avg time"
    );
    for model in &models {
        let os: Vec<&Outcome> = outcomes
            .iter()
            .filter(|o| o.model == *model && o.infra.is_none())
            .collect();
        let rate = |keep: &dyn Fn(&Outcome) -> bool| {
            let t: Vec<&&Outcome> = os.iter().filter(|o| keep(o)).collect();
            if t.is_empty() {
                "-".to_string()
            } else {
                format!(
                    "{:.0}%",
                    100.0 * t.iter().filter(|o| o.pass()).count() as f64 / t.len() as f64
                )
            }
        };
        println!(
            "{model:<40} {:>7} {:>7} {:>7} {:>7} {:>11} {:>7} {:>7} {:>8.2}$ {:>8.2}$ {:>8.0}s",
            rate(&|o| o.tier == 1),
            rate(&|o| o.tier == 2),
            rate(&|o| o.tier == 3),
            rate(&|o| o.role == "epa"),
            rate(&|o| o.role == "write-test"),
            rate(&|o| o.role == "stage"),
            rate(&|_| true),
            os.iter().map(|o| o.cost).sum::<f64>(),
            os.iter().map(|o| o.grader_cost).sum::<f64>(),
            os.iter().map(|o| o.secs).sum::<f64>() / os.len().max(1) as f64
        );
    }
    let out = repo_root().join("target/evals");
    std::fs::create_dir_all(&out).unwrap();
    let path = out.join(format!("test-stage-{stamp}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("\nReport: {}", path.display());
    let _ = std::fs::remove_dir_all(&data);
    assert!(
        outcomes.is_empty() || outcomes.iter().any(|o| o.cost > 0.0 || o.secs > 0.0),
        "no live run happened; check the provider credentials and the sandbox"
    );
}

/// Offline: every case parses and lays out, the project's own tests let every mutant survive, and replayed with
/// the golden analysis and tests through the real engine, each live run is reached and every code check passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_stage_cases_run_dry() {
    let file = load_cases();
    let root = scratch_root().join("offline");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut ids = std::collections::BTreeSet::new();
    for case in &file.case {
        let id = &case.id;
        assert!(ids.insert(id.clone()), "duplicate case id {id}");
        assert!((1..=3).contains(&case.tier), "{id}: tier 1 to 3");
        assert!(
            matches!(case.mode.as_str(), "phase" | "request"),
            "{id}: mode"
        );
        assert!(
            matches!(case.track.as_str(), "light" | "full"),
            "{id}: track"
        );
        assert!(
            !case.live.is_empty() && case.live.iter().all(|l| l == EPA || l == WRITE_TEST),
            "{id}: live lists epa, write-test, or both"
        );
        assert_eq!(
            case.live.iter().any(|l| l == EPA),
            case.epa.is_some(),
            "{id}: [case.epa] when and only when epa is live"
        );
        assert_eq!(
            case.live.iter().any(|l| l == WRITE_TEST),
            case.write_test.is_some(),
            "{id}: [case.write_test] when and only when write-test is live"
        );
        assert!(
            golden_dir(case).join("epa.md").is_file(),
            "{id}: the golden has an epa.md"
        );
        if case.mode == "phase" {
            assert!(
                case.change.is_some() && case.implementer_report.is_some(),
                "{id}: a phase case has a change and a report"
            );
            assert!(
                case.history.is_empty() && case.staged.is_none(),
                "{id}: a phase case's implementer makes the change"
            );
        } else {
            assert!(
                case.change.is_none() && case.implementer_report.is_none(),
                "{id}: no implementer runs in a request case"
            );
        }
        assert_eq!(
            case.track == "full" && case.mode == "phase",
            case.phase_file.is_some(),
            "{id}: a phase file only on the full track"
        );
        if let Some(w) = &case.write_test {
            assert!(matches!(w.status.as_str(), "ok" | "stuck"), "{id}: status");
            assert!(
                matches!(w.baseline.as_str(), "passes" | "fails"),
                "{id}: baseline"
            );
            assert_eq!(
                w.status == "ok",
                !case.mutant.is_empty(),
                "{id}: mutants when and only when write-test should pass"
            );
            assert!(
                !w.cause.trim().is_empty() && !w.rubric.trim().is_empty(),
                "{id}"
            );
        }
        if let Some(e) = &case.epa {
            assert!(
                !e.cause.trim().is_empty() && !e.rubric.trim().is_empty(),
                "{id}"
            );
        }
        for m in &case.mutant {
            assert!(
                !m.note.trim().is_empty(),
                "{id}: mutant {} needs a note",
                m.id
            );
        }
    }

    let cases: Vec<Case> = file
        .case
        .iter()
        .filter(|c| {
            let ok = has_runner(&c.project);
            if !ok {
                println!(
                    "{}: skipped, because this machine lacks the runner its tests need",
                    c.id
                );
            }
            ok
        })
        .cloned()
        .collect();
    let root = &root;
    let results: Vec<(Case, Session)> = futures::stream::iter(cases)
        .map(|case| async move {
            let s = run_session(None, &case, "mock:eval", &root.join(&case.id)).await;
            (case, s)
        })
        .buffer_unordered(4)
        .collect()
        .await;
    let mut problems = vec![];
    for (case, s) in &results {
        let id = &case.id;
        let seen: Vec<String> = s
            .router
            .runs
            .lock()
            .iter()
            .map(|r| format!("{}:{}", r.agent, r.kind))
            .collect();
        println!("{id}: {} | {} ({:.1}s)", seen.join(" > "), s.stop, s.secs);
        let last = if case.live.iter().any(|l| l == WRITE_TEST) {
            WRITE_TEST
        } else {
            EPA
        };
        if s.stop != format!("after {last}") {
            let errors: Vec<String> = s
                .router
                .runs
                .lock()
                .iter()
                .filter_map(|r| r.error.clone())
                .collect();
            problems.push(format!("{id}: the replay stopped with `{}`, not after {last}; runs: {seen:?}; errors: {errors:?}", s.stop));
            continue;
        }
        // What each agent is told comes from the engine, so the replay checks the parts the cases rest on.
        let first = |role: &str| {
            std::fs::read_to_string(s.router.dir.join(format!("first-message-{role}.md")))
                .unwrap_or_default()
        };
        let label = |text: &str, name: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{name}: ")))
                .map(|v| v.trim().to_string())
        };
        let epa_first = first(EPA);
        let wt_first = first(WRITE_TEST);
        match label(&epa_first, "Implementer report").map(|p| std::fs::read_to_string(p).unwrap_or_default()) {
            Some(r) if case.mode == "request" && !(r.starts_with("# Test request") && r.contains(&case.request)) => {
                problems.push(format!("{id}: the analyzer's implementer report is not the engine's test request with the request in it"))
            }
            Some(r) if case.mode == "phase" && !r.contains("## Changed Files") => {
                problems.push(format!("{id}: the analyzer's implementer report is not the scripted implementer's"))
            }
            None => problems.push(format!("{id}: the analyzer got no `Implementer report:` line")),
            _ => {}
        }
        if (label(&epa_first, "Phase file").is_some())
            != (case.track == "full" && case.mode == "phase")
        {
            problems.push(format!(
                "{id}: the analyzer's `Phase file:` line does not match the track"
            ));
        }
        if case.write_test.is_some() {
            let plan = if case.track == "full" && case.mode == "phase" {
                "Phase file"
            } else {
                "No plan"
            };
            if label(&wt_first, plan).is_none() {
                problems.push(format!("{id}: write-test got no `{plan}:` line"));
            }
        }
        if case.epa.is_some() {
            let (fails, _) = check_epa(case, s);
            problems.extend(
                fails
                    .into_iter()
                    .map(|f| format!("{id}: golden analysis: {f}")),
            );
        }
        if let Some(w) = &case.write_test {
            let b = s.router.baseline.lock().clone().unwrap_or_default();
            match (w.baseline.as_str(), b.passes) {
                ("passes", false) => problems.push(format!("{id}: the project's own tests fail before write-test: {}", b.failure)),
                ("fails", true) => problems.push(format!("{id}: the project's own tests pass before write-test, but the case says they fail")),
                _ => {}
            }
            for (m, alive) in &b.mutants {
                match alive {
                    Err(e) => problems.push(format!("{id}: {e}")),
                    Ok(false) if w.baseline == "passes" => {
                        problems.push(format!("{id}: the project's own tests already catch mutant {m}, so it measures nothing"))
                    }
                    _ => {}
                }
            }
            let (fails, _) = check_wt(case, s);
            problems.extend(
                fails
                    .into_iter()
                    .map(|f| format!("{id}: golden tests: {f}")),
            );
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn glob_matches_folders_and_files() {
    assert!(glob_match("tests/unit/**/*.py", "tests/unit/test_a.py"));
    assert!(glob_match("tests/unit/**/*.py", "tests/unit/sub/test_a.py"));
    assert!(!glob_match(
        "tests/unit/**/*.py",
        "tests/integration/test_a.py"
    ));
    assert!(!glob_match("tests/unit/**/*.py", "shop/unit.py"));
    assert!(glob_match("e2e/**/*.spec.js", "e2e/archive.spec.js"));
    assert!(!glob_match("e2e/**/*.spec.js", "e2e/archive.js"));
    assert!(glob_match("test/*.js", "test/a.js"));
    assert!(!glob_match("test/*.js", "test/sub/a.js"));
}
