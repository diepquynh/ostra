//! Planning evals over `tests/evals/planning.toml`: what the stages after research cost and how much of the code they
//! read again, on Ostra's own source pinned to the file's upstream commit. Live and paid, so the live tests are
//! ignored by default:
//!
//! ```text
//! # Record each case's research once: explore runs live, and its documents go to tests/evals/planning/research/.
//! OSTRA_EVAL_MODE=record cargo test -p ostra-server --test planning_evals planning_evals -- --ignored --nocapture
//! # Run generate-spec, plan, and both fact-checks live on the recorded research, until the plan is approved.
//! cargo test -p ostra-server --test planning_evals planning_evals -- --ignored --nocapture
//! # Compare two reports, for example the same cases on two engines.
//! OSTRA_EVAL_BASELINE=<report.json> OSTRA_EVAL_CANDIDATE=<report.json> \
//!   cargo test -p ostra-server --test planning_evals planning_compare -- --ignored --nocapture
//! ```
//!
//! Each case is a real session of a real `Engine` on a clone of the pinned snapshot, with scripted judges and YOLO.
//! A recorded research document is replayed through the native loop and the real Document tool, so the engine under
//! test treats it as it treats any research document, and two engines compared on one case read the same research.
//! The report counts, per stage, the cost, the tool calls, the code files read, and how many of those the research
//! had already described, and checks the approved plan: requirement and deliverable coverage and step files that exist.
//!
//! A recording skips cases that already have research unless `OSTRA_EVAL_RECORD_AGAIN=1`.
//! `OSTRA_EVAL_MODEL` (default Sonnet 5.5), `OSTRA_EVAL_CASES` and `OSTRA_EVAL_TIERS` filter, `OSTRA_EVAL_JOBS` (default
//! 3), `OSTRA_EVAL_BUDGET` (USD for the whole run, default 50: no session starts past it), `OSTRA_EVAL_SESSION_BUDGET`
//! (default 15), `OSTRA_EVAL_ARM` (the report's label, default the engine's commit), `OSTRA_EVAL_PIN` (overrides the
//! file's pin). The offline test replays every case with stand-ins and needs no model.

use chrono::Utc;
use futures::StreamExt;
use ostra_core::agent::AgentName;
use ostra_core::api::{CreateSession, FileIndex};
use ostra_core::config::{GlobalConfig, ProjectEntry, ResolvedRoute, WorkspaceSettings, save_toml};
use ostra_core::event::{ExecPurpose, GatePayload, SessionOptions};
use ostra_core::exec::{
    CancellationToken, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec,
    ExecutionStatus, Executor, Usage,
};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, SessionId, WorkspaceId};
use ostra_core::model::Effort;
use ostra_core::paths;
use ostra_core::policy::{PermissionAnswer, RuleRef, ToolCall};
use ostra_engine::factory::AgentsFactory;
use ostra_engine::state::{ExploreOrigin, SessionState};
use ostra_engine::{Engine, Notice, Services, SpawnFactory};
use ostra_exec_native::NativeExecutor;
use ostra_providers::{Providers, ScriptedProvider};
use ostra_store::WorkspaceDb;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------
// Cases and recorded research
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct File {
    pin: String,
    project: ProjectSpec,
    case: Vec<Case>,
}

#[derive(Deserialize, Clone)]
struct ProjectSpec {
    key: String,
    profile: String,
    inventory: String,
}

#[derive(Deserialize, Clone)]
struct Case {
    id: String,
    tier: u8,
    request: String,
    explore: Vec<String>,
    #[serde(default)]
    stakes: Option<String>,
}

/// One case's research, recorded once and replayed in every later run.
#[derive(Serialize, Deserialize, Clone)]
struct Fixture {
    pin: String,
    model: String,
    recorded: String,
    docs: Vec<FixtureDoc>,
}

#[derive(Serialize, Deserialize, Clone)]
struct FixtureDoc {
    task: String,
    /// The research document's file name in the explore session dir.
    file: String,
    /// The typed document as the explore agent wrote it, with `{repo}`, `{session}`, `{root}`, and `{ws}` for the
    /// scenario's own paths.
    document: Value,
    /// The explore submit, without `research_path`.
    submit: Value,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_cases() -> File {
    let text = std::fs::read_to_string(repo_root().join("tests/evals/planning.toml")).unwrap();
    toml::from_str(&text).unwrap()
}

fn fixture_path(case: &str) -> PathBuf {
    repo_root()
        .join("tests/evals/planning/research")
        .join(format!("{case}.json"))
}

fn load_fixture(case: &str) -> Option<Fixture> {
    let text = std::fs::read_to_string(fixture_path(case)).ok()?;
    Some(serde_json::from_str(&text).unwrap())
}

/// Scenarios live under the target dir, never `/tmp`, because the sandbox mounts its own `/tmp`.
fn scratch_root() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("planning-evals")
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
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

// ---------------------------------------------------------------------------------------------
// The pinned snapshot
// ---------------------------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
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

fn has_commit(pin: &str) -> bool {
    std::process::Command::new("git")
        .args(["cat-file", "-e", &format!("{pin}^{{commit}}")])
        .current_dir(repo_root())
        .status()
        .is_ok_and(|s| s.success())
}

/// Ostra's tracked files at `pin`, committed into a fresh repo, without this eval, whose cases and recorded research
/// would give other cases' answers away. Built once per pin and reused.
fn snapshot(pin: &str) -> PathBuf {
    let base = scratch_root()
        .join(format!("base-{}", &pin[..pin.len().min(12)]))
        .join("ostra");
    if base.join(".git").exists() {
        return base;
    }
    let partial = base.with_extension("partial");
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).unwrap();
    let archive = std::process::Command::new("git")
        .args(["archive", "--format=tar", pin])
        .current_dir(repo_root())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let status = std::process::Command::new("tar")
        .args(["-x", "-C"])
        .arg(&partial)
        .stdin(archive.stdout.unwrap())
        .status()
        .unwrap();
    assert!(status.success(), "extracting {pin} failed");
    for gone in [
        "tests/evals/planning.toml",
        "tests/evals/planning",
        "crates/ostra-server/tests/planning_evals.rs",
    ] {
        let p = partial.join(gone);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir_all(&p);
    }
    git(&partial, &["init", "-q"]);
    git(&partial, &["add", "-A"]);
    git(
        &partial,
        &[
            "-c",
            "user.name=Ostra eval",
            "-c",
            "user.email=eval@ostra.invalid",
            "commit",
            "-q",
            "-m",
            &format!("Ostra at {pin}"),
        ],
    );
    std::fs::rename(&partial, &base).unwrap();
    base
}

struct Vars(Vec<(&'static str, String)>);

impl Vars {
    fn of(st: &SessionState, repo: &Path, ws: &Path, key: &str) -> Vars {
        // Longest first, so `{session}` is not written as `{root}` plus a tail.
        Vars(vec![
            ("{session}", st.project_session_dir(key).display().to_string()),
            ("{root}", st.session_root.display().to_string()),
            ("{repo}", repo.display().to_string()),
            ("{ws}", ws.display().to_string()),
        ])
    }

    fn fill(&self, v: &Value) -> Value {
        let mut text = v.to_string();
        for (k, path) in &self.0 {
            text = text.replace(k, &json_inner(path));
        }
        serde_json::from_str(&text).unwrap()
    }

    fn hide(&self, v: &Value) -> Value {
        let mut text = v.to_string();
        for (k, path) in &self.0 {
            text = text.replace(&json_inner(path), k);
        }
        serde_json::from_str(&text).unwrap()
    }
}

/// `s` as it appears inside a JSON string.
fn json_inner(s: &str) -> String {
    let quoted = serde_json::to_string(s).unwrap();
    quoted[1..quoted.len() - 1].to_string()
}

// ---------------------------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Explore live; stop before the spec.
    Record,
    /// Replay the research; the stages after it live.
    Run,
    /// Replay the research; stand-ins for the rest.
    Dry,
}

#[derive(Clone)]
struct RunLog {
    agent: AgentName,
    /// `explore`, `explore-helper`, `spec`, `spec-check`, `plan`, or `plan-check`.
    stage: String,
    task: Option<u32>,
    live: bool,
    status: ExecutionStatus,
    submit: Option<Value>,
    error: Option<String>,
    usage: Usage,
    calls: Vec<(String, Value)>,
    /// Document results: the output text and whether it was an error.
    documents: Vec<(String, bool)>,
    turns: usize,
    secs: f64,
}

/// Passes everything to the engine's host and keeps what the run did.
struct Tap {
    inner: Arc<dyn ExecutionHost>,
    calls: Mutex<Vec<(String, Value)>>,
    tools: Mutex<HashMap<String, String>>,
    documents: Mutex<Vec<(String, bool)>>,
    turns: Mutex<usize>,
}

#[async_trait::async_trait]
impl ExecutionHost for Tap {
    fn emit(&self, delta: ExecutionDelta) {
        match &delta {
            ExecutionDelta::ToolCall { call_id, call } => {
                self.calls
                    .lock()
                    .push((call.tool.clone(), call.input.clone()));
                self.tools.lock().insert(call_id.clone(), call.tool.clone());
            }
            ExecutionDelta::ToolResult {
                call_id,
                output,
                is_error,
                ..
            } => {
                if self.tools.lock().get(call_id).map(String::as_str) == Some("Document") {
                    self.documents.lock().push((output.clone(), *is_error));
                }
            }
            ExecutionDelta::Turn { .. } => *self.turns.lock() += 1,
            _ => {}
        }
        self.inner.emit(delta);
    }

    async fn ask_permission(&self, call: &ToolCall, reason: &str, rule: &RuleRef) -> PermissionAnswer {
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

type Slot = Arc<OnceLock<(Engine, SessionId)>>;

/// Replays the case's research, runs the stages after it live (or with stand-ins), and stops at the build.
struct Router {
    case: Case,
    key: String,
    repo: PathBuf,
    ws: PathBuf,
    model: String,
    mode: Mode,
    fixture: Option<Fixture>,
    slot: Slot,
    live: Option<NativeExecutor>,
    runs: Mutex<Vec<RunLog>>,
    stop: Mutex<Option<String>>,
}

/// A minimal research document for a case with no recording, in the offline test only.
fn stand_in_research(case: &Case) -> FixtureDoc {
    FixtureDoc {
        task: case.explore.first().cloned().unwrap_or_default(),
        file: "ostra-research-20260101-000000-stand-in.md".into(),
        document: json!({"title": "Stand-in", "date": "2026-01-01", "repo": "ostra",
            "scope": "A stand-in for the offline test.", "problem": "None.",
            "files": [{"path": "CLAUDE.md", "purpose": "The patterns every change follows."}]}),
        submit: json!({"scope_covered": "The task as asked.", "findings_summary": "A stand-in.",
            "sources_retrieved": 0, "open_questions": 0, "not_covered": []}),
    }
}

impl Router {
    fn stop(&self, why: String) {
        let mut s = self.stop.lock();
        if s.is_none() {
            *s = Some(why);
        }
    }

    fn doc_for(&self, task: u32) -> Option<FixtureDoc> {
        match &self.fixture {
            Some(f) => f.docs.get(task as usize).cloned(),
            None if self.mode == Mode::Dry => Some(stand_in_research(&self.case)),
            None => None,
        }
    }

    /// The recorded research, written again through the native loop and the real Document tool.
    async fn replay(
        &self,
        mut spec: ExecutionSpec,
        host: Arc<Tap>,
        cancel: CancellationToken,
        doc: &FixtureDoc,
        vars: &Vars,
    ) -> ExecutionResult {
        let md = spec.ctx.session_dir.join(&doc.file);
        let mut submit = vars.fill(&doc.submit);
        submit["research_path"] = json!(md);
        let script = Arc::new(ScriptedProvider::named("replay"));
        script.push_tool_use(
            "Document",
            json!({"path": md, "document": vars.fill(&doc.document)}),
        );
        script.push_tool_use("submit_explore", submit);
        let providers = Providers::from_config(&GlobalConfig::default(), &Default::default());
        providers.register("replay", script);
        let native = NativeExecutor::new(
            Arc::new(providers),
            ostra_server::app::skill_resolver(),
            None,
        );
        spec.route.model = "replay:explore".into();
        native.run(spec, host, cancel).await
    }

    /// The offline test's spec, fact-check, and plan: files and submits only, as the coordination evals play them.
    fn stand_in(&self, spec: &ExecutionSpec, st: &SessionState, stage: &str) -> ExecutionResult {
        let root = &st.session_root;
        let spec_path = root.join("ostra-spec-1.md");
        let plan_path = root.join("ostra-plan-1.md");
        let submit = match stage {
            "spec" => {
                std::fs::write(&spec_path, "# Spec\n").unwrap();
                json!({"spec_path": spec_path, "open_questions": [], "external_evidence_rows": 0,
                    "deliverables": 1, "requirements": 1, "summary": "A stand-in spec."})
            }
            "plan" => {
                std::fs::write(&plan_path, "# Plan\n").unwrap();
                let phase = root.join("ostra-plan-1-phase-1.md");
                std::fs::write(&phase, "# Phase 1\n").unwrap();
                json!({"spec_path": spec_path, "master_plan_path": plan_path, "phases": [{"id": 1,
                    "deliverable": "D1", "project": self.key, "title": "Stand-in", "complexity": "Low",
                    "test_policy": "Required", "depends_on": [], "file": phase}], "stakes": "High",
                    "summary": "A stand-in plan.", "step_count": 1, "requirement_coverage": "1 of 1"})
            }
            _ => {
                let target = if stage == "plan-check" { "plan" } else { "spec" };
                json!({"verdict": "PASS", "target": target, "findings": []})
            }
        };
        let _ = spec;
        ExecutionResult {
            status: ExecutionStatus::Ok,
            submit: Some(submit),
            final_text: String::new(),
            usage: Usage::default(),
            native_session_id: None,
            error: None,
        }
    }
}

fn stage_of(agent: AgentName, st: &SessionState, exec: &ExecutionId) -> String {
    let rec = st.executions.get(exec);
    match agent {
        AgentName::Explore => {
            let helper = match rec.map(|x| &x.purpose) {
                Some(ExecPurpose::Explore { task }) => st
                    .explore
                    .get(*task as usize)
                    .is_some_and(|t| matches!(t.origin, ExploreOrigin::Ask { .. })),
                _ => false,
            };
            if helper { "explore-helper" } else { "explore" }.into()
        }
        AgentName::GenerateSpec => "spec".into(),
        AgentName::Plan => "plan".into(),
        AgentName::FactCheck => {
            let plan = rec.is_some_and(|x| x.params["target_type"].as_str() == Some("plan"));
            if plan { "plan-check" } else { "spec-check" }.into()
        }
        other => other.as_str().into(),
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
        let agent = spec.agent;
        let stage = stage_of(agent, &st, &spec.id);
        let task = match st.executions.get(&spec.id).map(|x| &x.purpose) {
            Some(ExecPurpose::Explore { task }) => Some(*task),
            _ => None,
        };
        let tap = Arc::new(Tap {
            inner: host,
            calls: Mutex::new(vec![]),
            tools: Mutex::new(HashMap::new()),
            documents: Mutex::new(vec![]),
            turns: Mutex::new(0),
        });
        let started = Instant::now();
        let planning = matches!(stage.as_str(), "spec" | "spec-check" | "plan" | "plan-check");
        let mut live = false;
        let result = match (stage.as_str(), self.mode) {
            ("explore", Mode::Run | Mode::Dry) => {
                let vars = Vars::of(&st, &self.repo, &self.ws, &self.key);
                match self.doc_for(task.unwrap_or_default()) {
                    Some(doc) => self.replay(spec, tap.clone(), cancel, &doc, &vars).await,
                    None => {
                        self.stop(format!("no recorded research for task {}", task.unwrap_or_default() + 1));
                        ExecutionResult::error("eval: no recorded research")
                    }
                }
            }
            ("explore" | "explore-helper", Mode::Record) | ("explore-helper", Mode::Run) => {
                live = true;
                spec.route.model = self.model.clone();
                match &self.live {
                    Some(native) => native.run(spec, tap.clone(), cancel).await,
                    None => ExecutionResult::error("eval: no live executor"),
                }
            }
            (_, Mode::Record) if planning => {
                self.stop("before generate-spec".into());
                cancelled()
            }
            (_, Mode::Run) if planning => {
                live = true;
                spec.route.model = self.model.clone();
                match &self.live {
                    Some(native) => native.run(spec, tap.clone(), cancel).await,
                    None => ExecutionResult::error("eval: no live executor"),
                }
            }
            (_, Mode::Dry) if planning => self.stand_in(&spec, &st, &stage),
            _ => {
                self.stop(format!("before {agent}"));
                cancelled()
            }
        };
        self.runs.lock().push(RunLog {
            agent,
            stage,
            task,
            live,
            status: result.status,
            submit: result.submit.clone(),
            error: result.error.clone(),
            usage: result.usage,
            calls: tap.calls.lock().clone(),
            documents: tap.documents.lock().clone(),
            turns: *tap.turns.lock(),
            secs: started.elapsed().as_secs_f64(),
        });
        result
    }
}

fn cancelled() -> ExecutionResult {
    let mut r = ExecutionResult::with_status(ExecutionStatus::Cancelled);
    r.error = Some("stopped by the eval".into());
    r
}

/// The code navigation tools over the snapshot, served the way the server's index serves them.
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
            indexes.with(&(), &root, &list, |ix| ostra_code::tools::run(ix, &tool, &input))
        })
        .await
        .map_err(|e| format!("The code index failed: {e}"))?
    }
}

/// Scripted judges: the case's research tasks, the full track, high stakes, every approval, and the recommended
/// option for every open question, so only the agents under test vary.
struct EvalServices {
    settings: WorkspaceSettings,
    router: Arc<Router>,
    model: String,
    case: Case,
    key: String,
    slot: Slot,
}

impl EvalServices {
    fn recommended_answers(&self) -> Result<Value, String> {
        let (engine, session) = self.slot.get().ok_or("eval: no engine")?;
        let st = engine.state(session).map_err(|e| e.to_string())?;
        let questions = st
            .open_gates()
            .find_map(|g| match &g.payload {
                GatePayload::OpenQuestions { questions, .. } => Some(questions.clone()),
                _ => None,
            })
            .ok_or("eval: no open questions gate")?;
        let answers: Vec<Value> = questions
            .iter()
            .map(|q| {
                let label = q
                    .options
                    .get(q.recommended)
                    .or(q.options.first())
                    .map(|o| o.label.clone())
                    .unwrap_or_else(|| "Use your recommendation.".into());
                json!({"id": q.id, "question": q.question, "answer": label})
            })
            .collect();
        Ok(json!({"kind": "questions", "answers": answers}))
    }
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
        let reason = "Scripted by the eval.";
        let out = if props.get("category").is_some() {
            let tasks: Vec<Value> = self
                .case
                .explore
                .iter()
                .map(|t| json!({"project": self.key, "task": t}))
                .collect();
            json!({"category": "IMPLEMENT", "projects": [self.key], "explore_tasks": tasks,
                "opts_in": {"tests": false, "docs": false}, "reason": reason, "title": "Eval"})
        } else if props.get("track").is_some() {
            json!({"track": "full", "reason": reason})
        } else if props.get("stakes").is_some() {
            json!({"stakes": self.case.stakes.clone().unwrap_or_else(|| "high".into()), "reason": reason})
        } else if props.get("route").is_some() && props.get("targets").is_none() {
            json!({"route": "requirement_change", "items": [], "research": [], "forget": [], "skip": [], "reason": reason})
        } else if props.get("items").is_some() && props.get("route").is_none() {
            json!({"items": [], "reason": reason})
        } else if let Some(answer) = props.get("answer") {
            match answer["properties"]["kind"]["const"].as_str() {
                Some("approval") => json!({"answer": {"kind": "approval", "approved": true}, "reason": reason}),
                Some("questions") => json!({"answer": self.recommended_answers()?, "reason": reason}),
                Some("choice") => {
                    let options: Vec<&str> = answer["properties"]["option"]["enum"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    let option = ["another-round", "continue", "retry", "done"]
                        .into_iter()
                        .find(|o| options.contains(o))
                        .or(options.first().copied())
                        .unwrap_or("done");
                    json!({"answer": {"kind": "choice", "option": option}, "reason": reason})
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
    state: SessionState,
    stop: String,
    repo: PathBuf,
    ws: PathBuf,
    secs: f64,
}

async fn run_session(
    providers: Option<Arc<Providers>>,
    file: &File,
    case: &Case,
    model: &str,
    mode: Mode,
    base: &Path,
    dir: &Path,
) -> Session {
    let started = Instant::now();
    std::fs::create_dir_all(dir).unwrap();
    let dir = paths::canonical(dir).unwrap();
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let key = file.project.key.clone();
    let repo = ws.join(&key);
    git(
        &ws,
        &["clone", "-q", "--local", &base.display().to_string(), &key],
    );
    std::fs::create_dir_all(paths::project_runtime(&repo)).unwrap();
    std::fs::write(paths::project_inventory(&repo), &file.project.inventory).unwrap();
    std::fs::write(paths::project_profile(&repo), &file.project.profile).unwrap();

    let slot: Slot = Arc::new(OnceLock::new());
    let live = providers.map(|p| {
        let mut files: Vec<String> = git(&repo, &["ls-files"]).lines().map(String::from).collect();
        files.sort();
        let nav = Nav {
            root: repo.clone(),
            list: Arc::new(FileIndex {
                paths: files,
                truncated: false,
            }),
            indexes: Arc::new(ostra_code::Indexes::default()),
        };
        NativeExecutor::new(p, ostra_server::app::skill_resolver(), Some(Arc::new(nav)))
    });
    let router = Arc::new(Router {
        case: case.clone(),
        key: key.clone(),
        repo: repo.clone(),
        ws: ws.clone(),
        model: model.to_string(),
        mode,
        fixture: load_fixture(&case.id),
        slot: slot.clone(),
        live,
        runs: Mutex::new(vec![]),
        stop: Mutex::new(None),
    });
    let mut settings = WorkspaceSettings::seeded("eval");
    settings.limits.max_parallel_executions = 2;
    settings.limits.session_budget_usd = env_or("OSTRA_EVAL_SESSION_BUDGET", 15.0);
    settings.projects.push(ProjectEntry {
        key: key.clone(),
        path: repo.clone(),
        stack: Some("rust".into()),
        code_provider: None,
        language_servers: vec![],
    });
    let services = Arc::new(EvalServices {
        settings,
        router: router.clone(),
        model: model.to_string(),
        case: case.clone(),
        key: key.clone(),
        slot: slot.clone(),
    });
    let db = WorkspaceDb::open_in_memory().unwrap();
    let engine = Engine::new(ws.clone(), WorkspaceId::new(), db, services);
    let summary = engine
        .create_session(CreateSession {
            request: case.request.clone(),
            options: SessionOptions {
                yolo: true,
                ..Default::default()
            },
            projects: vec![key.clone()],
            files: vec![],
            uploads: vec![],
            docs_book: None,
        })
        .unwrap();
    let session = summary.id.clone();
    let _ = slot.set((engine.clone(), session.clone()));

    let dry = mode == Mode::Dry;
    let timeout = Duration::from_secs(if dry { 120 } else { 3600 });
    let stall = Duration::from_secs(if dry { 15 } else { 300 });
    let mut rx = engine.subscribe();
    let mut last = (0usize, 0i64);
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
        if now != last {
            last = now;
            last_change = Instant::now();
        } else if st.running_executions().next().is_none() && last_change.elapsed() > stall {
            let gate = st
                .open_gates()
                .map(|g| g.payload.kind_str().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            break if gate.is_empty() {
                "stalled: nothing ran".into()
            } else {
                format!("stalled at gate {gate}")
            };
        }
        let _ = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
    };
    if !engine.state(&session).unwrap().is_terminal() {
        let _ = engine.stop_session(&session);
    }
    let settle = Instant::now();
    while engine.state(&session).unwrap().running_executions().next().is_some()
        && settle.elapsed() < Duration::from_secs(30)
    {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Session {
        router,
        state: engine.state(&session).unwrap(),
        stop,
        repo,
        ws,
        secs: started.elapsed().as_secs_f64(),
    }
}

// ---------------------------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------------------------

/// The research a recording session produced, with the scenario's paths turned back into placeholders.
fn recorded(s: &Session, case: &Case, model: &str, pin: &str) -> Result<Fixture, String> {
    let vars = Vars::of(&s.state, &s.repo, &s.ws, &s.router.key);
    let runs = s.router.runs.lock().clone();
    let mut docs = vec![];
    for (i, task) in case.explore.iter().enumerate() {
        let run = runs
            .iter()
            .rev()
            .find(|r| r.stage == "explore" && r.task == Some(i as u32) && r.status == ExecutionStatus::Ok)
            .ok_or(format!("research task {} did not finish", i + 1))?;
        let submit = run.submit.clone().ok_or("an explore run submitted nothing")?;
        let md = PathBuf::from(submit["research_path"].as_str().unwrap_or_default());
        let mut document = ostra_core::doc::load(&md).ok_or(format!("{} has no document", md.display()))?;
        if let Some(m) = document.as_object_mut() {
            m.remove("snapshot");
        }
        let mut submit = submit.clone();
        if let Some(m) = submit.as_object_mut() {
            m.remove("research_path");
        }
        docs.push(FixtureDoc {
            task: task.clone(),
            file: md.file_name().unwrap().to_string_lossy().to_string(),
            document: vars.hide(&document),
            submit: vars.hide(&submit),
        });
    }
    Ok(Fixture {
        pin: pin.to_string(),
        model: model.to_string(),
        recorded: Utc::now().to_rfc3339(),
        docs,
    })
}

// ---------------------------------------------------------------------------------------------
// Measuring
// ---------------------------------------------------------------------------------------------

/// Repo-relative paths of repo files a value names, keeping only files that exist.
fn repo_file(repo: &Path, text: &str) -> Option<String> {
    let t = text
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .trim_end_matches([',', ';', ')']);
    if t.is_empty() || t.contains('*') {
        return None;
    }
    let path = Path::new(t);
    let rel = if path.is_absolute() {
        path.strip_prefix(repo).ok()?.to_path_buf()
    } else {
        PathBuf::from(t.trim_start_matches("./"))
    };
    let s = rel.to_string_lossy().to_string();
    (!s.is_empty() && !s.starts_with(".ostra") && repo.join(&rel).is_file()).then_some(s)
}

/// The repo path at the head of a `path:Symbol` or `path:line` reference.
fn ref_path(repo: &Path, text: &str) -> Option<String> {
    let head = text.split_whitespace().next()?.trim_matches('`');
    let path = head.split(':').next()?;
    repo_file(repo, path)
}

const VIEWERS: [&str; 9] = ["cat", "head", "tail", "sed", "nl", "less", "awk", "bat", "more"];

#[derive(Default, Clone, Serialize, Deserialize)]
struct Reads {
    files: BTreeSet<String>,
    searches: usize,
    index_calls: usize,
    code_facts: bool,
    research_reads: usize,
}

fn reads(run: &RunLog, repo: &Path) -> Reads {
    let mut r = Reads::default();
    for (tool, input) in &run.calls {
        match tool.as_str() {
            "Read" => {
                let p = input["file_path"].as_str().unwrap_or_default();
                if p.ends_with("ostra-code-facts.md") {
                    r.code_facts = true;
                } else if Path::new(p)
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("ostra-research-"))
                {
                    r.research_reads += 1;
                } else if let Some(f) = repo_file(repo, p) {
                    r.files.insert(f);
                }
            }
            "Bash" => {
                let cmd = input["command"].as_str().unwrap_or_default();
                if cmd.contains("ostra-code-facts.md") {
                    r.code_facts = true;
                }
                let words: Vec<&str> = cmd
                    .split(|c: char| c.is_whitespace() || "|;&()<>".contains(c))
                    .filter(|w| !w.is_empty())
                    .collect();
                if words.iter().any(|w| VIEWERS.contains(w)) {
                    for w in &words {
                        if let Some(f) = repo_file(repo, w) {
                            r.files.insert(f);
                        }
                    }
                } else if words
                    .iter()
                    .any(|w| ["grep", "rg", "find", "ls", "git"].contains(w))
                {
                    r.searches += 1;
                }
            }
            "Grep" | "Glob" => r.searches += 1,
            t if t.starts_with("Code") => r.index_calls += 1,
            _ => {}
        }
    }
    r
}

/// Every repo file the recorded research names: what a later stage would not have to read again.
fn cited(fixture: Option<&Fixture>, repo: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for d in fixture.map(|f| f.docs.as_slice()).unwrap_or_default() {
        let doc = &d.document;
        let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
        for f in list(&doc["files"]) {
            out.extend(ref_path(repo, f["path"].as_str().unwrap_or_default()));
        }
        for p in list(&doc["patterns"]) {
            for f in list(&p["files"]) {
                out.extend(ref_path(repo, f.as_str().unwrap_or_default()));
            }
            out.extend(ref_path(repo, p["snippet"]["source"].as_str().unwrap_or_default()));
        }
        for h in list(&doc["data_flow"]) {
            out.extend(ref_path(repo, h["location"].as_str().unwrap_or_default()));
        }
        for a in list(&doc["approaches"]) {
            out.extend(ref_path(repo, a["precedent"].as_str().unwrap_or_default()));
        }
    }
    out
}

/// The Document results that named a missing file or symbol: what the write-time reference checks caught.
fn reference_errors(run: &RunLog) -> usize {
    const MARKS: [&str; 5] = [
        "is not there",
        "is not in `",
        "does not appear in",
        "Use `Create`, or fix the path",
        "Fix `read_first`",
    ];
    run.documents
        .iter()
        .map(|(text, _)| {
            text.lines()
                .filter(|l| l.starts_with("- error") && MARKS.iter().any(|m| l.contains(m)))
                .count()
        })
        .sum()
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Stage {
    runs: usize,
    cost_usd: f64,
    input_tokens: u64,
    cache_read_tokens: u64,
    output_tokens: u64,
    tool_calls: usize,
    turns: usize,
    files_read: usize,
    /// Files read that the research had already described.
    reread_cited: usize,
    searches: usize,
    index_calls: usize,
    code_facts_read: bool,
    document_calls: usize,
    document_chars: usize,
    reference_errors: usize,
    fails: usize,
    secs: f64,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct Quality {
    requirements: usize,
    requirements_delivered: usize,
    deliverables: usize,
    deliverables_planned: usize,
    phases: usize,
    steps: usize,
    /// `Modify` or `Delete` steps whose file is not in the repo and that no earlier step creates.
    broken_step_files: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct CaseReport {
    case: String,
    tier: u8,
    arm: String,
    model: String,
    stop: String,
    approved: bool,
    infra: Option<String>,
    cost_usd: f64,
    cited_files: usize,
    stages: BTreeMap<String, Stage>,
    quality: Option<Quality>,
    secs: f64,
    dir: String,
}

fn stages(runs: &[RunLog], repo: &Path, cited: &BTreeSet<String>) -> BTreeMap<String, Stage> {
    let mut out: BTreeMap<String, Stage> = BTreeMap::new();
    let mut files: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for run in runs.iter().filter(|r| r.live || r.stage != "explore") {
        let s = out.entry(run.stage.clone()).or_default();
        let r = reads(run, repo);
        s.runs += 1;
        s.cost_usd += run.usage.cost_usd;
        s.input_tokens +=
            run.usage.input_tokens + run.usage.cache_read_tokens + run.usage.cache_write_tokens;
        s.cache_read_tokens += run.usage.cache_read_tokens;
        s.output_tokens += run.usage.output_tokens;
        s.tool_calls += run.calls.len();
        s.turns += run.turns;
        s.searches += r.searches;
        s.index_calls += r.index_calls;
        s.code_facts_read |= r.code_facts;
        for (tool, input) in &run.calls {
            if tool == "Document" {
                s.document_calls += 1;
                s.document_chars += input.to_string().len();
            }
        }
        s.reference_errors += reference_errors(run);
        if run.submit.as_ref().and_then(|v| v["verdict"].as_str()) == Some("FAIL") {
            s.fails += 1;
        }
        s.secs += run.secs;
        files.entry(run.stage.clone()).or_default().extend(r.files);
    }
    for (stage, f) in files {
        if let Some(s) = out.get_mut(&stage) {
            s.files_read = f.len();
            s.reread_cited = f.intersection(cited).count();
        }
    }
    out
}

fn newest_json(dir: &Path, prefix: &str, skip: &str) -> Option<Value> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            n.starts_with(prefix) && n.ends_with(".json") && (skip.is_empty() || !n.contains(skip))
        })
        .collect();
    found.sort();
    let text = std::fs::read_to_string(found.last()?).ok()?;
    serde_json::from_str(&text).ok()
}

/// The approved plan against the approved spec and the repo.
fn quality(st: &SessionState, repo: &Path) -> Option<Quality> {
    let spec = newest_json(&st.session_root, "ostra-spec-", "")?;
    let plan = newest_json(&st.session_root, "ostra-plan-", "-phase-")?;
    let ids = |v: &Value, key: &str| -> BTreeSet<String> {
        v[key]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x["id"].as_str().map(String::from)).collect())
            .unwrap_or_default()
    };
    let requirements = ids(&spec, "requirements");
    let deliverables = ids(&spec, "deliverables");
    let phases = plan["phases"].as_array().cloned().unwrap_or_default();
    let mut delivered = BTreeSet::new();
    let mut planned = BTreeSet::new();
    let mut created = HashSet::new();
    let (mut steps, mut broken) = (0, 0);
    for p in &phases {
        if let Some(d) = p["deliverable"].as_str() {
            planned.insert(d.to_string());
        }
        for s in p["steps"].as_array().cloned().unwrap_or_default() {
            steps += 1;
            for r in s["delivers"].as_array().cloned().unwrap_or_default() {
                if let Some(r) = r.as_str() {
                    delivered.insert(r.to_string());
                }
            }
            let file = s["file"].as_str().unwrap_or_default().trim_start_matches("./").to_string();
            match s["change"].as_str() {
                Some("Create") => {
                    created.insert(file);
                }
                Some("Modify" | "Delete") if !repo.join(&file).exists() && !created.contains(&file) => {
                    broken += 1;
                }
                _ => {}
            }
        }
    }
    Some(Quality {
        requirements_delivered: requirements.intersection(&delivered).count(),
        requirements: requirements.len(),
        deliverables_planned: deliverables.intersection(&planned).count(),
        deliverables: deliverables.len(),
        phases: phases.len(),
        steps,
        broken_step_files: broken,
    })
}

fn infra_error(runs: &[RunLog]) -> Option<String> {
    runs.iter()
        .filter(|r| r.live && r.status == ExecutionStatus::Error)
        .filter_map(|r| r.error.clone())
        .find(|e| {
            let e = e.to_lowercase();
            ["model call failed", "provider error", "overloaded", "rate limit", "stream failed"]
                .iter()
                .any(|m| e.contains(m))
        })
}

fn report(s: &Session, case: &Case, arm: &str, model: &str, dir: &Path) -> CaseReport {
    let runs = s.router.runs.lock().clone();
    let cited = cited(s.router.fixture.as_ref(), &s.repo);
    let stages = stages(&runs, &s.repo, &cited);
    CaseReport {
        case: case.id.clone(),
        tier: case.tier,
        arm: arm.to_string(),
        model: model.to_string(),
        stop: s.stop.clone(),
        approved: s.stop == "before implementer",
        infra: infra_error(&runs),
        cost_usd: runs.iter().filter(|r| r.live).map(|r| r.usage.cost_usd).sum(),
        cited_files: cited.len(),
        stages,
        quality: quality(&s.state, &s.repo),
        secs: s.secs,
        dir: dir.display().to_string(),
    }
}

fn run_json(r: &RunLog) -> Value {
    json!({
        "agent": r.agent.as_str(), "stage": r.stage, "task": r.task, "live": r.live, "status": format!("{:?}", r.status).to_lowercase(),
        "error": r.error, "submit": r.submit, "cost_usd": r.usage.cost_usd, "turns": r.turns,
        "input_tokens": r.usage.input_tokens, "cache_read_tokens": r.usage.cache_read_tokens,
        "cache_write_tokens": r.usage.cache_write_tokens, "output_tokens": r.usage.output_tokens,
        "seconds": r.secs, "tool_calls": r.calls.iter().map(|(t, i)| format!("{t} {}", clip(&i.to_string(), 400))).collect::<Vec<_>>(),
        "document_results": r.documents.iter().map(|(t, e)| json!({"error": e, "text": clip(t, 2000)})).collect::<Vec<_>>(),
    })
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

// ---------------------------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------------------------

const STAGES: [&str; 4] = ["spec", "spec-check", "plan", "plan-check"];

fn stage<'a>(c: &'a CaseReport, name: &str) -> Option<&'a Stage> {
    c.stages.get(name)
}

fn markdown(cases: &[CaseReport], title: &str) -> String {
    let mut out = format!("# {title}\n\n");
    let _ = writeln!(
        out,
        "| Case | Tier | Approved | Cost | Spec reads (cited) | Plan reads (cited) | Facts | Plan runs | FAILs spec/plan | Ref errors | Requirements | Deliverables | Broken files |"
    );
    let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for c in cases {
        let s = |n: &str| stage(c, n).cloned().unwrap_or_default();
        let (spec, plan) = (s("spec"), s("plan"));
        let q = c.quality.clone().unwrap_or_default();
        let _ = writeln!(
            out,
            "| {} | {} | {} | ${:.2} | {} ({}) | {} ({}) | {} | {} | {}/{} | {} | {}/{} | {}/{} | {} |",
            c.case,
            c.tier,
            if c.infra.is_some() { "infra" } else if c.approved { "yes" } else { "no" },
            c.cost_usd,
            spec.files_read,
            spec.reread_cited,
            plan.files_read,
            plan.reread_cited,
            if plan.code_facts_read { "read" } else { "-" },
            plan.runs,
            s("spec-check").fails,
            s("plan-check").fails,
            STAGES.iter().map(|n| s(n).reference_errors).sum::<usize>(),
            q.requirements_delivered,
            q.requirements,
            q.deliverables_planned,
            q.deliverables,
            q.broken_step_files,
        );
    }
    let _ = writeln!(out, "\n## Totals by tier\n");
    let _ = writeln!(
        out,
        "| Tier | Cases | Approved | Cost | Spec cost | Spec-check cost | Plan cost | Plan-check cost | Spec reads (cited) | Plan reads (cited) | Plan runs |"
    );
    let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for tier in [Some(1u8), Some(2), Some(3), None] {
        let rows: Vec<&CaseReport> = cases
            .iter()
            .filter(|c| c.infra.is_none() && tier.is_none_or(|t| c.tier == t))
            .collect();
        if rows.is_empty() {
            continue;
        }
        let sum = |n: &str, f: fn(&Stage) -> f64| -> f64 {
            rows.iter().filter_map(|c| stage(c, n)).map(f).sum()
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | ${:.2} | ${:.2} | ${:.2} | ${:.2} | ${:.2} | {} ({}) | {} ({}) | {} |",
            tier.map(|t| t.to_string()).unwrap_or_else(|| "all".into()),
            rows.len(),
            rows.iter().filter(|c| c.approved).count(),
            rows.iter().map(|c| c.cost_usd).sum::<f64>(),
            sum("spec", |s| s.cost_usd),
            sum("spec-check", |s| s.cost_usd),
            sum("plan", |s| s.cost_usd),
            sum("plan-check", |s| s.cost_usd),
            sum("spec", |s| s.files_read as f64),
            sum("spec", |s| s.reread_cited as f64),
            sum("plan", |s| s.files_read as f64),
            sum("plan", |s| s.reread_cited as f64),
            sum("plan", |s| s.runs as f64),
        );
    }
    out
}

fn compare(base: &[CaseReport], cand: &[CaseReport]) -> String {
    let a = &base.first().map(|c| c.arm.clone()).unwrap_or_default();
    let b = &cand.first().map(|c| c.arm.clone()).unwrap_or_default();
    let mut out = format!("# Planning evals: {a} against {b}\n\n");
    let by_id: HashMap<&str, &CaseReport> = base.iter().map(|c| (c.case.as_str(), c)).collect();
    let pairs: Vec<(&CaseReport, &CaseReport)> = cand
        .iter()
        .filter_map(|c| by_id.get(c.case.as_str()).map(|b| (*b, c)))
        .filter(|(x, y)| x.infra.is_none() && y.infra.is_none())
        .collect();
    let _ = writeln!(
        out,
        "Cases on both sides without a provider failure: {}. Each cell is {a} → {b}.\n",
        pairs.len()
    );
    let metric = |c: &CaseReport, f: &dyn Fn(&CaseReport) -> f64| f(c);
    type Row<'a> = (&'a str, Box<dyn Fn(&CaseReport) -> f64>);
    let rows: Vec<Row> = vec![
        ("Approved", Box::new(|c| c.approved as u8 as f64)),
        ("Cost, all stages ($)", Box::new(|c| c.cost_usd)),
        ("Spec cost ($)", Box::new(|c| stage(c, "spec").map_or(0.0, |s| s.cost_usd))),
        ("Spec-check cost ($)", Box::new(|c| stage(c, "spec-check").map_or(0.0, |s| s.cost_usd))),
        ("Plan cost ($)", Box::new(|c| stage(c, "plan").map_or(0.0, |s| s.cost_usd))),
        ("Plan-check cost ($)", Box::new(|c| stage(c, "plan-check").map_or(0.0, |s| s.cost_usd))),
        ("Spec code files read", Box::new(|c| stage(c, "spec").map_or(0.0, |s| s.files_read as f64))),
        ("Spec reads the research described", Box::new(|c| stage(c, "spec").map_or(0.0, |s| s.reread_cited as f64))),
        ("Plan code files read", Box::new(|c| stage(c, "plan").map_or(0.0, |s| s.files_read as f64))),
        ("Plan reads the research described", Box::new(|c| stage(c, "plan").map_or(0.0, |s| s.reread_cited as f64))),
        ("Plan-check code files read", Box::new(|c| stage(c, "plan-check").map_or(0.0, |s| s.files_read as f64))),
        ("Tool calls, all stages", Box::new(|c| c.stages.values().map(|s| s.tool_calls as f64).sum())),
        ("Output tokens, all stages", Box::new(|c| c.stages.values().map(|s| s.output_tokens as f64).sum())),
        ("Spec runs", Box::new(|c| stage(c, "spec").map_or(0.0, |s| s.runs as f64))),
        ("Plan runs", Box::new(|c| stage(c, "plan").map_or(0.0, |s| s.runs as f64))),
        ("Fact-check FAILs", Box::new(|c| c.stages.values().map(|s| s.fails as f64).sum())),
        ("Plan Document chars", Box::new(|c| stage(c, "plan").map_or(0.0, |s| s.document_chars as f64))),
        ("Reference errors caught at write", Box::new(|c| c.stages.values().map(|s| s.reference_errors as f64).sum())),
        ("Requirements delivered (%)", Box::new(|c| c.quality.as_ref().map_or(0.0, |q| 100.0 * q.requirements_delivered as f64 / q.requirements.max(1) as f64))),
        ("Deliverables planned (%)", Box::new(|c| c.quality.as_ref().map_or(0.0, |q| 100.0 * q.deliverables_planned as f64 / q.deliverables.max(1) as f64))),
        ("Broken step files", Box::new(|c| c.quality.as_ref().map_or(0.0, |q| q.broken_step_files as f64))),
        ("Wall time (min)", Box::new(|c| c.secs / 60.0)),
    ];
    for tier in [Some(1u8), Some(2), Some(3), None] {
        let set: Vec<&(&CaseReport, &CaseReport)> = pairs
            .iter()
            .filter(|(x, _)| tier.is_none_or(|t| x.tier == t))
            .collect();
        if set.is_empty() {
            continue;
        }
        let name = tier.map(|t| format!("Tier {t}")).unwrap_or_else(|| "All tiers".into());
        let _ = writeln!(out, "## {name} ({} cases)\n", set.len());
        let _ = writeln!(out, "| Metric | Sum | Mean per case | Median per case | Change |");
        let _ = writeln!(out, "| --- | --- | --- | --- | --- |");
        for (label, f) in &rows {
            let xs: Vec<f64> = set.iter().map(|(x, _)| metric(x, f.as_ref())).collect();
            let ys: Vec<f64> = set.iter().map(|(_, y)| metric(y, f.as_ref())).collect();
            let (sx, sy) = (xs.iter().sum::<f64>(), ys.iter().sum::<f64>());
            let change = if sx.abs() > f64::EPSILON {
                format!("{:+.0}%", 100.0 * (sy - sx) / sx)
            } else {
                "-".into()
            };
            let _ = writeln!(
                out,
                "| {label} | {} → {} | {} → {} | {} → {} | {change} |",
                num(sx),
                num(sy),
                num(sx / set.len() as f64),
                num(sy / set.len() as f64),
                num(median(&xs)),
                num(median(&ys)),
            );
        }
        let _ = writeln!(out);
    }
    let _ = writeln!(out, "## Per case\n");
    let _ = writeln!(out, "| Case | Approved | Cost | Plan reads (cited) | Spec reads (cited) | Plan runs | FAILs |");
    let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- | --- |");
    for (x, y) in &pairs {
        let g = |c: &CaseReport, n: &str| stage(c, n).cloned().unwrap_or_default();
        let fails = |c: &CaseReport| c.stages.values().map(|s| s.fails).sum::<usize>();
        let _ = writeln!(
            out,
            "| {} | {} → {} | ${:.2} → ${:.2} | {} ({}) → {} ({}) | {} ({}) → {} ({}) | {} → {} | {} → {} |",
            x.case,
            x.approved,
            y.approved,
            x.cost_usd,
            y.cost_usd,
            g(x, "plan").files_read,
            g(x, "plan").reread_cited,
            g(y, "plan").files_read,
            g(y, "plan").reread_cited,
            g(x, "spec").files_read,
            g(x, "spec").reread_cited,
            g(y, "spec").files_read,
            g(y, "spec").reread_cited,
            g(x, "plan").runs,
            g(y, "plan").runs,
            fails(x),
            fails(y),
        );
    }
    out
}

fn num(x: f64) -> String {
    if (x - x.round()).abs() < 1e-9 && x.abs() >= 10.0 {
        format!("{x:.0}")
    } else {
        format!("{x:.2}")
    }
}

fn median(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    match v.len() {
        0 => 0.0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}

/// The engine's own commit, and whether its tree has changes on top.
fn engine_arm() -> String {
    if let Ok(arm) = std::env::var("OSTRA_EVAL_ARM") {
        return arm;
    }
    let root = repo_root();
    let head = git(&root, &["rev-parse", "--short=12", "HEAD"]).trim().to_string();
    let dirty = !git(&root, &["status", "--porcelain", "--untracked-files=no"])
        .trim()
        .is_empty();
    if dirty { format!("{head}+changes") } else { head }
}

// ---------------------------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------------------------

/// The live data dir, config, and assets, set once before any run.
fn live_env(stamp: &str) {
    if let Some(c) = std::fs::read_to_string(ostra_server::prices::cache_path(&paths::data_dir()))
        .ok()
        .and_then(|t| ostra_core::pricing::Catalog::from_models_dev(&t).ok())
    {
        ostra_core::pricing::install(c);
    }
    // The data dir holds the sandbox's egress sockets, whose paths must stay short, and must not be under `/tmp`,
    // which the sandbox replaces with its own.
    let home = paths::home().expect("a home directory");
    let data = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"))
        .join("ostra-evals")
        .join(format!("p{stamp}"));
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: runs Ostra's agents on real models and costs money"]
async fn planning_evals() {
    let file = load_cases();
    let mode = match std::env::var("OSTRA_EVAL_MODE").as_deref() {
        Ok("record") => Mode::Record,
        _ => Mode::Run,
    };
    // A recording resumes where it stopped: a case with research is recorded again only on request.
    let again = std::env::var("OSTRA_EVAL_RECORD_AGAIN").is_ok_and(|v| v == "1");
    let cases: Vec<Case> = selected(&file)
        .into_iter()
        .filter(|c| mode != Mode::Record || again || load_fixture(&c.id).is_none())
        .collect();
    let model = std::env::var("OSTRA_EVAL_MODEL")
        .unwrap_or_else(|_| "anthropic:claude-sonnet-5-5".into());
    let pin = std::env::var("OSTRA_EVAL_PIN").unwrap_or_else(|_| file.pin.clone());
    let jobs: usize = env_or("OSTRA_EVAL_JOBS", 3);
    let budget: f64 = env_or("OSTRA_EVAL_BUDGET", 50.0);
    let arm = engine_arm();
    if mode == Mode::Run {
        let missing: Vec<&str> = cases
            .iter()
            .filter(|c| load_fixture(&c.id).is_none())
            .map(|c| c.id.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "Record the research first (OSTRA_EVAL_MODE=record) for: {}",
            missing.join(", ")
        );
    }
    let stamp = Utc::now().format("%Y%m%dT%H%M%S").to_string();
    live_env(&stamp);
    let providers = Arc::new(Providers::from_config(
        &GlobalConfig::default(),
        &Default::default(),
    ));
    let base = snapshot(&pin);
    let root = scratch_root().join(&stamp);
    std::fs::create_dir_all(&root).unwrap();
    println!(
        "\n{} cases, {:?} on {model}, engine {arm}, code at {pin}, budget ${budget:.0}. Scenarios in {}",
        cases.len(),
        mode,
        root.display()
    );
    let spent = Arc::new(Mutex::new(0.0f64));
    let (file, base, model, arm, pin) = (&file, &base, model.as_str(), arm.as_str(), pin.as_str());
    let reports: Vec<Option<CaseReport>> = futures::stream::iter(cases.clone())
        .map(|case| {
            let providers = providers.clone();
            let spent = spent.clone();
            let dir = root.join(&case.id);
            async move {
                if *spent.lock() >= budget {
                    println!("  {:<30} skipped: the run's budget is spent", case.id);
                    return None;
                }
                let mut retries = 0;
                let (s, dir) = loop {
                    let d = if retries == 0 { dir.clone() } else { PathBuf::from(format!("{}-retry{retries}", dir.display())) };
                    let s = run_session(Some(providers.clone()), file, &case, model, mode, base, &d).await;
                    let cost: f64 = s.router.runs.lock().iter().filter(|r| r.live).map(|r| r.usage.cost_usd).sum();
                    *spent.lock() += cost;
                    let infra = infra_error(&s.router.runs.lock());
                    match infra {
                        Some(e) if retries < 2 => {
                            println!("  {:<30} provider error, running again: {}", case.id, clip(&e, 120));
                            retries += 1;
                        }
                        _ => break (s, d),
                    }
                };
                let runs: Vec<Value> = s.router.runs.lock().iter().map(run_json).collect();
                std::fs::write(dir.join("runs.json"), serde_json::to_string_pretty(&runs).unwrap()).unwrap();
                if mode == Mode::Record {
                    match recorded(&s, &case, model, pin) {
                        Ok(f) => {
                            let path = fixture_path(&case.id);
                            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                            std::fs::write(&path, serde_json::to_string_pretty(&f).unwrap() + "\n").unwrap();
                            println!("  {:<30} recorded {} documents  ${:.3}  {:.0}s", case.id, f.docs.len(),
                                s.router.runs.lock().iter().map(|r| r.usage.cost_usd).sum::<f64>(), s.secs);
                        }
                        Err(e) => println!("  {:<30} NOT recorded: {e} ({})", case.id, s.stop),
                    }
                    return None;
                }
                let r = report(&s, &case, arm, model, &dir);
                println!(
                    "  {:<30} {:<8} ${:<6.2} spec reads {:>2} ({:>2} cited)  plan reads {:>2} ({:>2} cited) facts {}  plan runs {}  {:.0}s  {}",
                    r.case,
                    if r.infra.is_some() { "INFRA" } else if r.approved { "approved" } else { "STOPPED" },
                    r.cost_usd,
                    stage(&r, "spec").map_or(0, |s| s.files_read),
                    stage(&r, "spec").map_or(0, |s| s.reread_cited),
                    stage(&r, "plan").map_or(0, |s| s.files_read),
                    stage(&r, "plan").map_or(0, |s| s.reread_cited),
                    if stage(&r, "plan").is_some_and(|s| s.code_facts_read) { "read" } else { "-" },
                    stage(&r, "plan").map_or(0, |s| s.runs),
                    r.secs,
                    r.stop
                );
                Some(r)
            }
        })
        .buffer_unordered(jobs)
        .collect()
        .await;
    println!("\nSpent ${:.2} on live runs.", *spent.lock());
    if mode == Mode::Record {
        return;
    }
    let mut reports: Vec<CaseReport> = reports.into_iter().flatten().collect();
    reports.sort_by(|a, b| (a.tier, &a.case).cmp(&(b.tier, &b.case)));
    let out = repo_root().join("target/evals");
    std::fs::create_dir_all(&out).unwrap();
    let name = format!("planning-{}-{stamp}", arm.replace(['+', '/'], "-"));
    std::fs::write(out.join(format!("{name}.json")), serde_json::to_string_pretty(&reports).unwrap()).unwrap();
    let md = markdown(&reports, &format!("Planning evals: engine {arm}, {model}, code at {pin}"));
    std::fs::write(out.join(format!("{name}.md")), &md).unwrap();
    println!("\n{md}\nReport: {}", out.join(format!("{name}.md")).display());
    assert!(
        reports.iter().any(|r| r.infra.is_none()),
        "no session finished without a provider failure"
    );
}

#[test]
#[ignore = "reads two reports from OSTRA_EVAL_BASELINE and OSTRA_EVAL_CANDIDATE"]
fn planning_compare() {
    let read = |var: &str| -> Vec<CaseReport> {
        let path = std::env::var(var).unwrap_or_else(|_| panic!("set {var} to a planning report"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    };
    let (base, cand) = (read("OSTRA_EVAL_BASELINE"), read("OSTRA_EVAL_CANDIDATE"));
    let md = compare(&base, &cand);
    let out = repo_root().join("target/evals");
    std::fs::create_dir_all(&out).unwrap();
    let path = out.join(format!("planning-compare-{}.md", Utc::now().format("%Y%m%dT%H%M%S")));
    std::fs::write(&path, &md).unwrap();
    println!("{md}\nComparison: {}", path.display());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planning_cases_run_dry() {
    let file = load_cases();
    if !has_commit(&file.pin) {
        println!("planning evals: commit {} is not in this clone, so the offline replay is skipped", file.pin);
        return;
    }
    let ids: HashSet<&str> = file.case.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids.len(), file.case.len(), "case ids are unique");
    let base = snapshot(&file.pin);
    let root = scratch_root().join(format!("dry-{}", Utc::now().format("%Y%m%dT%H%M%S%f")));
    let file = &file;
    let base = &base;
    let reports: Vec<(String, Result<CaseReport, String>)> = futures::stream::iter(file.case.clone())
        .map(|case| {
            let dir = root.join(&case.id);
            async move {
                let s = run_session(None, file, &case, "mock:eval", Mode::Dry, base, &dir).await;
                let runs = s.router.runs.lock().clone();
                let replays = runs.iter().filter(|r| r.stage == "explore").count();
                let failed: Vec<String> = runs
                    .iter()
                    .filter(|r| r.stage == "explore" && r.status != ExecutionStatus::Ok)
                    .map(|r| r.error.clone().unwrap_or_default())
                    .collect();
                let result = if !failed.is_empty() {
                    Err(format!("a research replay failed: {}", failed.join("; ")))
                } else if replays != case.explore.len() {
                    Err(format!("{replays} research replays for {} tasks", case.explore.len()))
                } else if s.stop != "before implementer" {
                    Err(format!("stopped {}", s.stop))
                } else {
                    Ok(report(&s, &case, "dry", "mock:eval", &dir))
                };
                (case.id.clone(), result)
            }
        })
        .buffer_unordered(4)
        .collect()
        .await;
    let fails: Vec<String> = reports
        .iter()
        .filter_map(|(id, r)| r.as_ref().err().map(|e| format!("{id}: {e}")))
        .collect();
    assert!(fails.is_empty(), "{}", fails.join("\n"));
    let ok: Vec<CaseReport> = reports.into_iter().filter_map(|(_, r)| r.ok()).collect();
    let md = markdown(&ok, "dry");
    assert!(md.contains("| all |"), "{md}");
    let _ = std::fs::remove_dir_all(&root);
}
