//! Subagent coordination evals over `tests/evals/coordination.toml` (HANDOVER 10.8). Live and paid, so the live test
//! is ignored by default:
//!
//! ```text
//! OSTRA_EVAL_MODELS=anthropic:claude-opus-5-5,anthropic:claude-sonnet-5-5 OSTRA_EVAL_RUNS=3 \
//!   cargo test -p ostra-server --test coordination_evals -- --ignored --nocapture
//! ```
//!
//! Each case is a real session of a real `Engine` on a snapshot of Ostra's own source, with scripted judges. A router
//! executor sends the runs the case lists as `live` to the native loop on the model under test, with the policy, the
//! sandbox, the code index, and the coordination tools wired to that engine, and plays every other run from the
//! case. A run passes when every code check holds (questions, answers, statuses, submits, files, continuations, cache
//! reads) and the grader model (`OSTRA_EVAL_GRADER`, default Opus) finds the rubric met. The report lists pass counts
//! per case, tier, and model, with cost and the share of live input tokens read from the prompt cache, and is written
//! to `target/evals/`. `OSTRA_EVAL_CASES` filters case ids by substring, `OSTRA_EVAL_TIERS` by tier (`1,2`),
//! `OSTRA_EVAL_JOBS` sets how many sessions run at once (default 3). The test fails only when no live run finished.
//!
//! The offline test replays every case with stand-ins for the live runs (the case's `dry` actors, else the defaults)
//! and checks that each live run is reached and the session stops where the case says, so a broken scenario costs
//! nothing to find.

use chrono::Utc;
use futures::StreamExt;
use ostra_core::agent::{AgentName, Capability};
use ostra_core::api::{CreateSession, FileIndex};
use ostra_core::config::{GlobalConfig, ProjectEntry, ResolvedRoute, WorkspaceSettings, save_toml};
use ostra_core::coord::{CoordReply, MessageTarget, SEND_MESSAGE};
use ostra_core::event::{ExecPurpose, SessionEvent, SessionOptions, StoredEvent};
use ostra_core::exec::{
    CancellationToken, ExecutionDelta, ExecutionHost, ExecutionResult, ExecutionSpec,
    ExecutionStatus, Executor, Usage,
};
use ostra_core::executor::ExecutorKind;
use ostra_core::ids::{ExecutionId, SessionId, WorkspaceId};
use ostra_core::model::Effort;
use ostra_core::paths;
use ostra_core::policy::{PermissionAnswer, PolicyDecision, RuleRef, ToolCall};
use ostra_default_plugin::factory::AgentsFactory;
use ostra_engine::state::{ExploreOrigin, SessionState};
use ostra_engine::{Engine, Notice, Services, SpawnFactory};
use ostra_exec_native::NativeExecutor;
use ostra_store::WorkspaceDb;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct File {
    project: ProjectSpec,
    case: Vec<Case>,
}

#[derive(Deserialize, Clone)]
struct ProjectSpec {
    key: String,
    test_command: String,
    research: String,
}

#[derive(Deserialize, Clone)]
struct Case {
    id: String,
    tier: u8,
    #[serde(default)]
    note: Option<String>,
    request: String,
    track: String,
    #[serde(default)]
    research: Option<String>,
    #[serde(default)]
    spec: Option<String>,
    #[serde(default)]
    spec_say: Option<String>,
    /// Tool calls of the first spec run, before `spec_say`.
    #[serde(default)]
    spec_tools: Vec<ToolSpec>,
    #[serde(default = "one")]
    explore_tasks: usize,
    live: Vec<String>,
    #[serde(default)]
    stop_after: Vec<String>,
    #[serde(default)]
    stop_before: Vec<String>,
    #[serde(default)]
    actor: Vec<Actor>,
    /// Stand-ins for live runs in the offline replay, so it takes the path a good run takes.
    #[serde(default)]
    dry: Vec<Actor>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    /// Workspace custom instructions per agent, the channel a user steers an agent through.
    #[serde(default)]
    instructions: BTreeMap<String, String>,
    expect: Expect,
}

fn one() -> usize {
    1
}

#[derive(Deserialize, Clone, Default)]
struct Actor {
    run: String,
    #[serde(default)]
    say: Option<String>,
    #[serde(default)]
    write: Vec<FileSpec>,
    #[serde(default)]
    edit: Vec<EditSpec>,
    #[serde(default)]
    research: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    ask_agent: Option<String>,
    #[serde(default)]
    ask_subagent: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    reply: Option<String>,
    #[serde(default)]
    submit: Option<String>,
    #[serde(default)]
    changed: Vec<String>,
    /// Tool calls the scripted run made before `say`, kept in its conversation with their results.
    #[serde(default)]
    tools: Vec<ToolSpec>,
}

#[derive(Deserialize, Clone)]
struct ToolSpec {
    name: String,
    /// JSON.
    input: String,
    output: String,
}

#[derive(Deserialize, Clone)]
struct FileSpec {
    path: String,
    content: String,
}

#[derive(Deserialize, Clone)]
struct EditSpec {
    path: String,
    old: String,
    new: String,
}

#[derive(Deserialize, Clone, Default)]
struct Expect {
    #[serde(default)]
    asks: Vec<AskExp>,
    #[serde(default)]
    no_asks_from: Vec<String>,
    #[serde(default)]
    replies: Vec<ReplyExp>,
    #[serde(default)]
    statuses: Vec<StatusExp>,
    #[serde(default)]
    submits: Vec<SubmitExp>,
    #[serde(default)]
    files: Vec<FileExp>,
    #[serde(default)]
    continued: Vec<String>,
    #[serde(default)]
    cached: Vec<String>,
    #[serde(default)]
    no_writes: Vec<String>,
    #[serde(default)]
    research: Option<Contains>,
    cause: String,
    rubric: String,
}

#[derive(Deserialize, Clone)]
struct AskExp {
    from: String,
    to: String,
    #[serde(default)]
    has: Vec<Vec<String>>,
}

#[derive(Deserialize, Clone)]
struct ReplyExp {
    from: String,
    #[serde(default)]
    has: Vec<Vec<String>>,
    #[serde(default)]
    lacks: Vec<String>,
}

#[derive(Deserialize, Clone)]
struct StatusExp {
    run: String,
    status: String,
}

#[derive(Deserialize, Clone)]
struct SubmitExp {
    run: String,
    #[serde(default)]
    pointer: Option<String>,
    #[serde(default)]
    equals: Option<String>,
    #[serde(default)]
    has: Vec<Vec<String>>,
    #[serde(default)]
    lacks: Vec<String>,
}

#[derive(Deserialize, Clone)]
struct FileExp {
    path: String,
    #[serde(default)]
    has: Vec<Vec<String>>,
    #[serde(default)]
    lacks: Vec<String>,
}

#[derive(Deserialize, Clone, Default)]
struct Contains {
    #[serde(default)]
    has: Vec<Vec<String>>,
    #[serde(default)]
    lacks: Vec<String>,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_cases() -> File {
    let text = std::fs::read_to_string(repo_root().join("tests/evals/coordination.toml")).unwrap();
    toml::from_str(&text).unwrap()
}

/// Scenarios live under the target dir, never `/tmp`, because the sandbox mounts its own `/tmp`.
fn scratch_root() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("coordination-evals")
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

fn present<'a>(text: &str, lacks: &'a [String]) -> Vec<&'a str> {
    lacks
        .iter()
        .filter(|l| text.contains(l.as_str()))
        .map(String::as_str)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Run addresses
// ---------------------------------------------------------------------------------------------

/// `agent[:kind][#n][!]`: which runs a line of a case means.
#[derive(Debug, Clone)]
struct Addr {
    agent: AgentName,
    kind: Option<String>,
    nth: Option<usize>,
    /// Only a run that ends other than `waiting`.
    final_only: bool,
}

fn addr(s: &str) -> Result<Addr, String> {
    let (s, final_only) = match s.strip_suffix('!') {
        Some(rest) => (rest, true),
        None => (s, false),
    };
    let (s, nth) = match s.split_once('#') {
        Some((a, n)) => (
            a,
            Some(n.parse::<usize>().map_err(|e| format!("`{s}`: {e}"))?),
        ),
        None => (s, None),
    };
    let (agent, kind) = match s.split_once(':') {
        Some((a, k)) => (a, Some(k.to_string())),
        None => (s, None),
    };
    Ok(Addr {
        agent: agent.parse().map_err(|e| format!("`{s}`: {e}"))?,
        kind,
        nth,
        final_only,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunRef {
    agent: AgentName,
    kind: String,
    nth: usize,
}

impl std::fmt::Display for RunRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}#{}", self.agent, self.kind, self.nth)
    }
}

impl Addr {
    fn matches(&self, r: &RunRef) -> bool {
        self.agent == r.agent
            && self.kind.as_ref().is_none_or(|k| *k == r.kind)
            && self.nth.is_none_or(|n| n == r.nth)
    }

    /// Agent and kind only, for facts about an execution rather than one of its runs.
    fn matches_exec(&self, runs: &[&RunLog]) -> bool {
        runs.iter()
            .any(|r| self.agent == r.at.agent && self.kind.as_ref().is_none_or(|k| *k == r.at.kind))
    }
}

fn kind_of(p: &ExecPurpose) -> String {
    serde_json::to_value(p)
        .ok()
        .and_then(|v| v["kind"].as_str().map(String::from))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// The scenario: a snapshot of Ostra, an engine, and the router
// ---------------------------------------------------------------------------------------------

/// Ostra's working tree as it is now: tracked files plus new ones, without the private handovers and without
/// this eval, whose expectations would give the answers away.
fn snapshot(root: &Path) -> PathBuf {
    let base = root.join("base").join("ostra");
    if base.join(".git").exists() {
        return base;
    }
    std::fs::create_dir_all(&base).unwrap();
    let src = repo_root();
    let out = std::process::Command::new("git")
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .current_dir(&src)
        .output()
        .unwrap();
    assert!(out.status.success());
    for f in String::from_utf8_lossy(&out.stdout).split('\0') {
        let secret = f.ends_with("_HANDOVER.md")
            || f == "tests/evals/coordination.toml"
            || f == "crates/ostra-server/tests/coordination_evals.rs";
        if f.is_empty() || secret || !src.join(f).is_file() {
            continue;
        }
        let dest = base.join(f);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(src.join(f), &dest).unwrap();
    }
    git(&base, &["init", "-q"]);
    git(&base, &["add", "-A"]);
    git(
        &base,
        &[
            "-c",
            "user.name=Ostra eval",
            "-c",
            "user.email=eval@ostra.invalid",
            "commit",
            "-q",
            "-m",
            "Ostra snapshot",
        ],
    );
    base
}

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

/// One run as the router saw it.
#[derive(Clone)]
struct RunLog {
    at: RunRef,
    exec: ExecutionId,
    live: bool,
    resumed_from: Option<ExecutionId>,
    status: ExecutionStatus,
    submit: Option<Value>,
    error: Option<String>,
    usage: Usage,
    calls: Vec<String>,
    denials: Vec<String>,
    secs: f64,
}

impl RunLog {
    fn continues(&self) -> bool {
        self.resumed_from.as_ref().is_some_and(|f| *f != self.exec)
    }

    fn wrote(&self) -> bool {
        self.calls.iter().any(|c| {
            [
                "Write ",
                "Edit ",
                "Document ",
                "MultiEdit ",
                "NotebookEdit ",
            ]
            .iter()
            .any(|t| c.starts_with(t))
        }) || self.denials.iter().any(|d| d.contains("answer-only"))
    }
}

/// Passes everything to the engine's host and keeps what the run did.
struct Tap {
    inner: Arc<dyn ExecutionHost>,
    calls: Mutex<Vec<String>>,
    denials: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl ExecutionHost for Tap {
    fn emit(&self, delta: ExecutionDelta) {
        match &delta {
            ExecutionDelta::ToolCall { call, .. } => {
                let input: String = call.input.to_string().chars().take(300).collect();
                self.calls.lock().push(format!("{} {input}", call.tool));
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
    project: ProjectSpec,
    repo: PathBuf,
    ws: PathBuf,
    model: String,
    dry: bool,
    slot: Slot,
    live: Option<NativeExecutor>,
    runs: Mutex<Vec<RunLog>>,
    counts: Mutex<HashMap<(AgentName, String), usize>>,
    explores: Mutex<usize>,
    stop: Mutex<Option<String>>,
    live_at: Vec<Addr>,
    stop_after: Vec<Addr>,
    stop_before: Vec<Addr>,
}

/// Placeholders, from the session's own dirs.
fn vars(st: &SessionState, repo: &Path, ws: &Path, key: &str) -> Vars {
    Vars(vec![
        ("{repo}", repo.display().to_string()),
        ("{root}", st.session_root.display().to_string()),
        (
            "{session}",
            st.project_session_dir(key).display().to_string(),
        ),
        ("{ws}", ws.display().to_string()),
    ])
}

impl Router {
    fn stop(&self, why: String) {
        let mut s = self.stop.lock();
        if s.is_none() {
            *s = Some(why);
        }
    }

    fn actor(&self, r: &RunRef) -> Option<Actor> {
        let dry = if self.dry {
            self.case.dry.iter()
        } else {
            [].iter()
        };
        dry.chain(self.case.actor.iter())
            .find(|a| addr(&a.run).is_ok_and(|x| x.matches(r)))
            .cloned()
    }

    fn coordinate(
        &self,
        exec: &ExecutionId,
        tool: &str,
        input: Value,
    ) -> Result<CoordReply, String> {
        let (engine, session) = self.slot.get().ok_or("no engine")?;
        engine.coordinate(session, exec, tool, &input)
    }

    async fn canned(
        &self,
        spec: &ExecutionSpec,
        host: &Tap,
        st: &SessionState,
        r: &RunRef,
    ) -> ExecutionResult {
        let actor = self.actor(r).unwrap_or_default();
        let vars = vars(st, &self.repo, &self.ws, &self.project.key);
        let fill = |s: &str| vars.fill(s);
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
        let say = actor.say.clone().or_else(|| {
            (r.agent == AgentName::GenerateSpec && r.kind == "spec" && r.nth == 1)
                .then(|| self.case.spec_say.clone())
                .flatten()
        });
        let first_spec = r.agent == AgentName::GenerateSpec && r.kind == "spec" && r.nth == 1;
        let tools = if actor.tools.is_empty() && first_spec {
            self.case.spec_tools.clone()
        } else {
            actor.tools.clone()
        };
        for (i, t) in tools.iter().enumerate() {
            let id = format!("toolu_eval_{}_{i}", r.nth);
            let input: Value = serde_json::from_str(&fill(&t.input)).unwrap_or(json!({}));
            host.record_message(
                "assistant",
                &json!([{"type": "tool_use", "id": id, "name": t.name, "input": input}]),
            );
            host.record_message(
                "user",
                &json!([{"type": "tool_result", "tool_use_id": id, "content": fill(&t.output), "is_error": false}]),
            );
        }
        if let Some(say) = &say {
            host.record_message("assistant", &text_blocks(&fill(say)));
        }
        for w in &actor.write {
            let path = PathBuf::from(fill(&w.path));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, fill(&w.content)).unwrap();
        }
        for e in &actor.edit {
            let path = PathBuf::from(fill(&e.path));
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if !text.contains(&e.old) {
                return ExecutionResult::error(format!(
                    "eval: the edit's old text is not in {}",
                    path.display()
                ));
            }
            std::fs::write(&path, text.replacen(&e.old, &e.new, 1)).unwrap();
        }
        let ok = |status: ExecutionStatus, submit: Value| ExecutionResult {
            status,
            submit: Some(submit),
            final_text: String::new(),
            usage: Usage::default(),
            native_session_id: None,
            error: None,
        };

        if let Some(q) = &actor.ask {
            let input = match (&actor.ask_agent, &actor.ask_subagent) {
                (Some(agent), _) => json!({"message": fill(q), "agent": agent, "wait": true}),
                (None, Some(agent)) => {
                    let Some(first) = st
                        .executions
                        .values()
                        .find(|x| x.agent.as_str() == agent.as_str())
                    else {
                        return ExecutionResult::error(format!("eval: no {agent} run to ask"));
                    };
                    json!({"message": fill(q), "to": st.subagent_of(&first.id).as_str(), "wait": true})
                }
                (None, None) => return ExecutionResult::error("eval: an ask names no target"),
            };
            return match self.coordinate(&spec.id, SEND_MESSAGE, input) {
                Ok(_) => ok(
                    ExecutionStatus::Waiting,
                    ostra_core::coord::end_payload(SEND_MESSAGE, q),
                ),
                Err(e) => ExecutionResult::error(format!("eval: the ask was refused: {e}")),
            };
        }
        // Rule SM6: a run replies to every sender that waits for it.
        let owed = st.owed_by(&spec.id);
        let continued = matches!(
            st.executions.get(&spec.id).map(|x| &x.purpose),
            Some(ExecPurpose::Message { .. })
        );
        let reply = actor.reply.clone().or_else(|| {
            (!owed.is_empty() || continued)
                .then(|| "Nothing to add beyond my earlier work.".to_string())
        });
        if let Some(text) = reply {
            for to in &owed {
                if let Err(e) = self.coordinate(
                    &spec.id,
                    SEND_MESSAGE,
                    json!({"message": fill(&text), "to": to.as_str()}),
                ) {
                    return ExecutionResult::error(format!("eval: the reply was refused: {e}"));
                }
            }
            if continued || actor.submit.is_none() {
                return ok(
                    ExecutionStatus::Ok,
                    ostra_core::coord::end_payload(SEND_MESSAGE, &text),
                );
            }
        }
        if let Some(s) = &actor.submit {
            return match serde_json::from_str(&fill(s)) {
                Ok(v) => ok(ExecutionStatus::Ok, v),
                Err(e) => ExecutionResult::error(format!("eval: submit is not JSON: {e}")),
            };
        }
        let rec = st.executions.get(&spec.id);
        let submit = match r.agent {
            AgentName::Explore => {
                let helper = match rec.map(|x| &x.purpose) {
                    Some(ExecPurpose::Explore { task }) => st
                        .explore
                        .get(*task as usize)
                        .is_some_and(|t| matches!(t.origin, ExploreOrigin::Ask { .. })),
                    _ => false,
                };
                let content = actor.research.clone().unwrap_or_else(|| {
                    if helper {
                        "# Research\n\nNo further findings.\n".into()
                    } else {
                        self.case
                            .research
                            .clone()
                            .unwrap_or_else(|| self.project.research.clone())
                    }
                });
                let n = {
                    let mut e = self.explores.lock();
                    *e += 1;
                    *e
                };
                let path = spec.ctx.session_dir.join(format!("ostra-research-{n}.md"));
                std::fs::create_dir_all(&spec.ctx.session_dir).unwrap();
                std::fs::write(&path, fill(&content)).unwrap();
                json!({"research_path": path, "scope_covered": "The task as asked.",
                    "findings_summary": actor.summary.clone().unwrap_or_else(|| "See the research document.".into()),
                    "sources_retrieved": 1, "open_questions": 0, "not_covered": []})
            }
            AgentName::GenerateSpec => {
                let path = st.session_root.join("ostra-spec-1.md");
                if !path.exists() || (r.nth == 1 && self.case.spec.is_some()) {
                    let text = self.case.spec.clone().unwrap_or_else(|| {
                        "# Spec\n\n- R1: THE native loop SHALL keep its limits.\n".into()
                    });
                    std::fs::write(&path, fill(&text)).unwrap();
                }
                json!({"spec_path": path, "open_questions": [], "external_evidence_rows": 0,
                    "deliverables": 1, "requirements": 3, "summary": "A spec for the request."})
            }
            AgentName::FactCheck => {
                let target = rec
                    .and_then(|x| x.params["target_type"].as_str().map(String::from))
                    .unwrap_or_else(|| "spec".into());
                json!({"verdict": "PASS", "target": target, "findings": []})
            }
            AgentName::Implementer => {
                let report = spec
                    .ctx
                    .report_file
                    .clone()
                    .unwrap_or_else(|| spec.ctx.session_dir.join("report.md"));
                std::fs::create_dir_all(report.parent().unwrap()).unwrap();
                std::fs::write(
                    &report,
                    format!(
                        "# Report\n\n{}\n",
                        say.map(|s| fill(&s)).unwrap_or_else(|| "Done.".into())
                    ),
                )
                .unwrap();
                json!({"status": "ok", "report_path": report, "changed_files": actor.changed, "summary": "Done."})
            }
            AgentName::CodeReviewer => json!({"findings": [], "security_block": false,
                "ledger_path": "/l", "summary": "No findings."}),
            other => return ExecutionResult::error(format!("eval: no scripted run for {other}")),
        };
        ok(ExecutionStatus::Ok, submit)
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
        let nth = {
            let mut c = self.counts.lock();
            let n = c.entry((spec.agent, kind.clone())).or_default();
            *n += 1;
            *n
        };
        let at = RunRef {
            agent: spec.agent,
            kind,
            nth,
        };
        let tap = Arc::new(Tap {
            inner: host,
            calls: Mutex::new(vec![]),
            denials: Mutex::new(vec![]),
        });
        let exec = spec.id.clone();
        let resumed_from = spec.resume.as_ref().map(|r| r.from.clone());
        let started = Instant::now();
        let live_listed = self.live_at.iter().any(|a| a.matches(&at));
        // An agent the eval has no script for ends the session too, because the case is over by then.
        let scripted = matches!(
            at.agent,
            AgentName::Explore
                | AgentName::GenerateSpec
                | AgentName::FactCheck
                | AgentName::Implementer
                | AgentName::CodeReviewer
        );
        let stopped =
            self.stop_before.iter().any(|a| a.matches(&at)) || (!scripted && !live_listed);
        let live = !stopped && !self.dry && live_listed;
        let result = if stopped {
            self.stop(format!("before {at}"));
            let mut r = ExecutionResult::with_status(ExecutionStatus::Cancelled);
            r.error = Some("stopped by the eval".into());
            r
        } else if live {
            spec.route.model = self.model.clone();
            match &self.live {
                Some(native) => native.run(spec, tap.clone(), cancel).await,
                None => ExecutionResult::error("eval: no live executor"),
            }
        } else {
            self.canned(&spec, &tap, &st, &at).await
        };
        let status = result.status;
        self.runs.lock().push(RunLog {
            at: at.clone(),
            exec,
            live,
            resumed_from,
            status,
            submit: result.submit.clone(),
            error: result.error.clone(),
            usage: result.usage,
            calls: tap.calls.lock().clone(),
            denials: tap.denials.lock().clone(),
            secs: started.elapsed().as_secs_f64(),
        });
        if self
            .stop_after
            .iter()
            .any(|a| a.matches(&at) && (!a.final_only || status != ExecutionStatus::Waiting))
        {
            self.stop(format!("after {at}"));
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
            let tasks: Vec<Value> = (0..self.case.explore_tasks)
                .map(|i| json!({"project": self.key, "task": format!("Research the code the request touches, part {}.", i + 1)}))
                .collect();
            json!({"category": "IMPLEMENT", "projects": [self.key], "explore_tasks": tasks,
                "opts_in": {"tests": false, "docs": false}, "reason": "The request changes code.", "title": "Eval"})
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
    state: SessionState,
    events: Vec<StoredEvent>,
    stop: String,
    repo: PathBuf,
    ws: PathBuf,
    secs: f64,
}

async fn run_session(
    providers: Option<Arc<ostra_providers::Providers>>,
    file: &File,
    case: &Case,
    model: &str,
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
    std::fs::write(
        paths::project_inventory(&repo),
        "# Inventory\n\nOstra: a Rust workspace (`crates/*`, one crate per concern) with a React console in `web/`. `CLAUDE.md` describes the crates and patterns.\n",
    )
    .unwrap();
    std::fs::write(
        paths::project_profile(&repo),
        format!(
            "[commands]\ntest = \"{}\"\nformat = \"true\"\n",
            file.project.test_command
        ),
    )
    .unwrap();

    let slot: Slot = Arc::new(OnceLock::new());
    let live = providers.map(|p| {
        let mut files: Vec<String> = git(&repo, &["ls-files"])
            .lines()
            .map(String::from)
            .collect();
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
            .with_coord(Arc::new(Coord(slot.clone())))
    });
    let parse = |v: &[String]| -> Vec<Addr> { v.iter().map(|s| addr(s).unwrap()).collect() };
    let router = Arc::new(Router {
        case: case.clone(),
        project: file.project.clone(),
        repo: repo.clone(),
        ws: ws.clone(),
        model: model.to_string(),
        dry: live.is_none(),
        slot: slot.clone(),
        live,
        runs: Mutex::new(vec![]),
        counts: Mutex::new(HashMap::new()),
        explores: Mutex::new(0),
        stop: Mutex::new(None),
        live_at: parse(&case.live),
        stop_after: parse(&case.stop_after),
        stop_before: parse(&case.stop_before),
    });
    let mut settings = WorkspaceSettings::seeded("eval");
    settings.limits.max_parallel_executions = 2;
    settings.limits.session_budget_usd = 25.0;
    settings.instructions.agents = case.instructions.clone();
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
        key: key.clone(),
        case: case.clone(),
    });
    let db = WorkspaceDb::open_in_memory().unwrap();
    ostra_default_plugin::install();
    let engine = Engine::new(ws.clone(), WorkspaceId::new(), db, services);
    // The router needs the session before its first run, and the first run waits for the classify judge, so
    // the slot is filled right after the session is created.
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
            workflow: None,
        })
        .unwrap();
    let session = summary.id.clone();
    let _ = slot.set((engine.clone(), session.clone()));

    let dry = router.dry;
    let timeout = Duration::from_secs(case.timeout_secs.unwrap_or(if dry { 60 } else { 1800 }));
    let stall = Duration::from_secs(if dry { 10 } else { 180 });
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
        state: engine.state(&session).unwrap(),
        events: engine.db().events(&session).unwrap(),
        stop,
        repo,
        ws,
        secs: started.elapsed().as_secs_f64(),
    }
}

// ---------------------------------------------------------------------------------------------
// Grading
// ---------------------------------------------------------------------------------------------

fn runs_of<'a>(runs: &'a [RunLog], exec: &ExecutionId) -> Vec<&'a RunLog> {
    runs.iter().filter(|r| r.exec == *exec).collect()
}

fn target_name(st: &SessionState, t: &MessageTarget) -> String {
    match t {
        MessageTarget::Agent { agent, .. } => agent.as_str().to_string(),
        MessageTarget::Subagent { id } => st
            .executions
            .get(id)
            .map(|r| r.agent.as_str().to_string())
            .unwrap_or_default(),
    }
}

/// A message that asks: one that pauses its sender or starts a helper. Any other is an answer.
fn is_question(e: &SessionEvent) -> bool {
    matches!(
        e,
        SessionEvent::MessageSent { wait: true, .. }
            | SessionEvent::MessageSent {
                to: MessageTarget::Agent { .. },
                ..
            }
    )
}

/// The code checks. Each failure is one line for the report.
fn check(case: &Case, s: &Session, key: &str) -> Vec<String> {
    let runs = s.router.runs.lock().clone();
    let v = vars(&s.state, &s.repo, &s.ws, key);
    let mut fails = vec![];
    if !runs.iter().any(|r| r.live) {
        fails.push(format!("no live run happened (stop: {})", s.stop));
        return fails;
    }
    if s.stop == "timeout" || s.stop.starts_with("stalled") {
        fails.push(format!("the session did not reach its end: {}", s.stop));
    }
    let asks: Vec<(ExecutionId, MessageTarget, String)> = s
        .events
        .iter()
        .filter_map(|e| match &e.event {
            SessionEvent::MessageSent { from, to, text, .. } if is_question(&e.event) => {
                Some((from.clone(), to.clone(), text.clone()))
            }
            _ => None,
        })
        .collect();
    let replies: Vec<(ExecutionId, String)> = s
        .events
        .iter()
        .filter_map(|e| match &e.event {
            SessionEvent::MessageSent { from, text, .. } if !is_question(&e.event) => {
                Some((from.clone(), text.clone()))
            }
            _ => None,
        })
        .collect();
    for exp in &case.expect.asks {
        let from = addr(&exp.from).unwrap();
        let found: Vec<&(ExecutionId, MessageTarget, String)> = asks
            .iter()
            .filter(|(f, _, _)| from.matches_exec(&runs_of(&runs, f)))
            .collect();
        match found
            .iter()
            .find(|(_, t, _)| target_name(&s.state, t) == exp.to)
        {
            None if found.is_empty() => fails.push(format!("{} asked no one", exp.from)),
            None => fails.push(format!(
                "{} asked {} instead of {}",
                exp.from,
                target_name(&s.state, &found[0].1),
                exp.to
            )),
            Some((_, _, m)) => {
                let miss = missing(m, &exp.has);
                if !miss.is_empty() {
                    fails.push(format!(
                        "the question to {} lacks {}",
                        exp.to,
                        miss.join(", ")
                    ));
                }
            }
        }
    }
    for who in &case.expect.no_asks_from {
        let a = addr(who).unwrap();
        let n = asks
            .iter()
            .filter(|(f, _, _)| a.matches_exec(&runs_of(&runs, f)))
            .count();
        if n > 0 {
            fails.push(format!("{who} asked {n} question(s) where none was needed"));
        }
    }
    for exp in &case.expect.replies {
        let a = addr(&exp.from).unwrap();
        match replies
            .iter()
            .find(|(f, _)| a.matches_exec(&runs_of(&runs, f)))
        {
            None => fails.push(format!("{} did not reply", exp.from)),
            Some((_, m)) => {
                let miss = missing(m, &exp.has);
                if !miss.is_empty() {
                    fails.push(format!("the reply lacks {}", miss.join(", ")));
                }
                let bad = present(m, &exp.lacks);
                if !bad.is_empty() {
                    fails.push(format!("the reply says {}", bad.join(", ")));
                }
            }
        }
    }
    let run_at = |spec: &str| -> Option<RunLog> {
        let a = addr(spec).unwrap();
        runs.iter().rev().find(|r| a.matches(&r.at)).cloned()
    };
    for exp in &case.expect.statuses {
        match run_at(&exp.run) {
            None => fails.push(format!("{} never ran", exp.run)),
            Some(r) => {
                let got = format!("{:?}", r.status).to_lowercase();
                if got != exp.status {
                    fails.push(format!("{} ended {got}, not {}", exp.run, exp.status));
                }
            }
        }
    }
    for exp in &case.expect.submits {
        let a = addr(&exp.run).unwrap();
        let Some(r) = runs.iter().rev().find(|r| {
            a.matches(&r.at) && r.submit.is_some() && r.status != ExecutionStatus::Waiting
        }) else {
            fails.push(format!("{} submitted nothing", exp.run));
            continue;
        };
        let sub = r.submit.clone().unwrap_or_default();
        if let (Some(p), Some(want)) = (&exp.pointer, &exp.equals) {
            let got = sub.pointer(p).and_then(Value::as_str).unwrap_or_default();
            if got != want {
                fails.push(format!("{} {p} is {got:?}, not {want:?}", exp.run));
            }
        }
        let text = sub.to_string();
        let miss = missing(&text, &exp.has);
        if !miss.is_empty() {
            fails.push(format!("{} submit lacks {}", exp.run, miss.join(", ")));
        }
        let bad = present(&text, &exp.lacks);
        if !bad.is_empty() {
            fails.push(format!("{} submit has {}", exp.run, bad.join(", ")));
        }
    }
    for exp in &case.expect.files {
        let path = PathBuf::from(v.fill(&exp.path));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let miss = missing(&text, &exp.has);
        if !miss.is_empty() {
            fails.push(format!("{} lacks {}", exp.path, miss.join(", ")));
        }
        let bad = present(&text, &exp.lacks);
        if !bad.is_empty() {
            fails.push(format!("{} still has {}", exp.path, bad.join(", ")));
        }
    }
    for spec in &case.expect.continued {
        match run_at(spec) {
            None => fails.push(format!("{spec} never ran")),
            Some(r) if !r.continues() => {
                fails.push(format!("{spec} started fresh instead of continuing"))
            }
            _ => {}
        }
    }
    for spec in &case.expect.cached {
        if let Some(r) = run_at(spec)
            && r.usage.cache_read_tokens == 0
        {
            fails.push(format!("{spec} read nothing from the prompt cache"));
        }
    }
    for spec in &case.expect.no_writes {
        let a = addr(spec).unwrap();
        if runs.iter().any(|r| a.matches(&r.at) && r.wrote()) {
            fails.push(format!("{spec} tried to change files"));
        }
    }
    if let Some(c) = &case.expect.research {
        match research_of(&runs) {
            None => fails.push("no live explore run wrote a research document".into()),
            Some((_, text)) => {
                let miss = missing(&text, &c.has);
                if !miss.is_empty() {
                    fails.push(format!("the research document lacks {}", miss.join(", ")));
                }
                let bad = present(&text, &c.lacks);
                if !bad.is_empty() {
                    fails.push(format!("the research document has {}", bad.join(", ")));
                }
            }
        }
    }
    fails
}

/// The research document of the last live explore run that submitted one.
fn research_of(runs: &[RunLog]) -> Option<(String, String)> {
    let r = runs.iter().rev().find(|r| {
        r.live
            && r.at.agent == AgentName::Explore
            && r.submit.is_some()
            && r.status == ExecutionStatus::Ok
    })?;
    let path = r.submit.as_ref()?["research_path"].as_str()?.to_string();
    let text = std::fs::read_to_string(&path).ok()?;
    Some((path, text))
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    format!("{}\n... (clipped)", s.chars().take(n).collect::<String>())
}

/// What the grader reads: every run, question, answer, and live submit, and what changed on disk.
fn record(s: &Session) -> String {
    let runs = s.router.runs.lock().clone();
    let mut out = String::from("# Runs, in order\n\n");
    for r in &runs {
        out.push_str(&format!(
            "- {} ({}, ended {:?}{}, {} tool calls)\n",
            r.at,
            if r.live { "live" } else { "scripted" },
            r.status,
            if r.continues() {
                ", continues an earlier run's conversation"
            } else if r.resumed_from.is_some() {
                ", woken in place"
            } else {
                ""
            },
            r.calls.len()
        ));
    }
    let who = |exec: &ExecutionId| -> String {
        runs.iter()
            .find(|r| r.exec == *exec)
            .map(|r| format!("{} ({})", r.at.agent, r.at.kind))
            .unwrap_or_else(|| exec.to_string())
    };
    out.push_str("\n# Questions and answers, in order\n\n");
    for e in &s.events {
        match &e.event {
            SessionEvent::MessageSent {
                from,
                to: target,
                text: message,
                ..
            } if is_question(&e.event) => out.push_str(&format!(
                "- QUESTION from {} to {}:\n{}\n\n",
                who(from),
                match target {
                    MessageTarget::Agent { agent, .. } => format!("a new {agent} helper"),
                    MessageTarget::Subagent { .. } =>
                        format!("the {} subagent", target_name(&s.state, target)),
                },
                message
            )),
            SessionEvent::MessageSent { from, text, .. } => {
                out.push_str(&format!("- ANSWER from {}:\n{}\n\n", who(from), text))
            }
            _ => {}
        }
    }
    out.push_str("# What each live run did (tool calls, in order)\n\n");
    let root = s.ws.display().to_string();
    for r in runs.iter().filter(|r| r.live) {
        out.push_str(&format!("- {}:\n", r.at));
        for c in r.calls.iter().take(60) {
            let short: String = c.replace(&root, "{ws}").chars().take(220).collect();
            out.push_str(&format!("  - {short}\n"));
        }
        if r.calls.len() > 60 {
            out.push_str(&format!("  - ... {} more\n", r.calls.len() - 60));
        }
        for d in &r.denials {
            out.push_str(&format!("  - DENIED: {d}\n"));
        }
    }
    out.push_str("\n# Submits of live runs\n\n");
    for r in runs.iter().filter(|r| r.live) {
        if let Some(sub) = &r.submit {
            out.push_str(&format!(
                "- {}:\n```json\n{}\n```\n",
                r.at,
                clip(&serde_json::to_string_pretty(sub).unwrap_or_default(), 4000)
            ));
        }
    }
    let diff = std::process::Command::new("git")
        .args(["diff", "HEAD", "--", ".", ":(exclude).ostra"])
        .current_dir(&s.repo)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    if !diff.trim().is_empty() {
        out.push_str(&format!(
            "\n# Changes to the repository\n\n```diff\n{}\n```\n",
            clip(&diff, 8000)
        ));
    }
    if let Some((path, text)) = research_of(&runs) {
        out.push_str(&format!(
            "\n# The live helper's research document ({path})\n\n{}\n",
            clip(&text, 6000)
        ));
    }
    out
}

const GRADER: &str = "You grade how the agents of Ostra, an engineering pipeline, coordinated in one eval case.

Ostra's agents can ask each other questions: `SubagentAsk` starts an explore helper or asks an existing subagent, the asking run waits, and the answer wakes it with its conversation intact. A subagent answers with `SubagentReply`. The input gives the case, what the right behavior rests on, the rubric, and a record of the session: every run, every question and answer, the live runs' submits, and what changed on disk. Runs marked scripted were played by the eval; judge only the live runs.

Apply the rubric literally. Pass only when every point holds in the record. A point the record does not show fails, even when an agent may have meant it. Judge meaning, not wording: a paraphrase that states the same fact passes, and extra correct detail does not hurt.

Call `decide` once with `verdict` (`pass` or `fail`) and `reason`: one or two sentences naming the rubric point that decided it.";

async fn grade(
    providers: &ostra_providers::Providers,
    grader: &str,
    case: &Case,
    record: &str,
) -> Result<((bool, String), f64), String> {
    let (p, m) = providers.for_model(grader).map_err(|e| e.to_string())?;
    let user = format!(
        "# Case\n\n{}\n\nThe request: {}\n\n# What the right behavior rests on\n\n{}\n\n# Rubric\n\n{}\n\n{}",
        case.note.clone().unwrap_or_default(),
        case.request,
        case.expect.cause,
        case.expect.rubric,
        record
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
    stop: String,
    fails: Vec<String>,
    grade: Option<(bool, String)>,
    grade_error: Option<String>,
    cost: f64,
    grader_cost: f64,
    live_runs: usize,
    asks: usize,
    /// Cache reads over all input tokens of the live runs.
    cache_ratio: Option<f64>,
    secs: f64,
    runs: Vec<Value>,
    dir: PathBuf,
    /// The provider failed every attempt, so the case says nothing about the model.
    infra: Option<String>,
    retries: usize,
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
        "run": r.at.to_string(), "live": r.live, "status": format!("{:?}", r.status).to_lowercase(),
        "continues": r.continues(), "woken_in_place": r.resumed_from.is_some() && !r.continues(),
        "error": r.error, "submit": r.submit, "cost_usd": r.usage.cost_usd,
        "input_tokens": r.usage.input_tokens, "cache_read_tokens": r.usage.cache_read_tokens,
        "cache_write_tokens": r.usage.cache_write_tokens, "output_tokens": r.usage.output_tokens,
        "seconds": r.secs, "tool_calls": r.calls, "policy_denials": r.denials,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_one(
    providers: Arc<ostra_providers::Providers>,
    file: &File,
    case: &Case,
    model: &str,
    grader: &str,
    base: &Path,
    dir: PathBuf,
) -> Outcome {
    // A session whose live run died on the provider runs again, up to twice.
    let mut retries = 0;
    let (s, dir) = loop {
        let d = if retries == 0 {
            dir.clone()
        } else {
            PathBuf::from(format!("{}-retry{retries}", dir.display()))
        };
        let s = run_session(Some(providers.clone()), file, case, model, base, &d).await;
        let infra = infra_error(&s.router.runs.lock());
        match infra {
            Some(e) if retries < 2 => {
                println!(
                    "  {:<52} {:<34} provider error, running again: {}",
                    case.id,
                    model,
                    e.chars().take(120).collect::<String>()
                );
                let _ = std::fs::remove_dir_all(s.repo.join("target"));
                retries += 1;
            }
            _ => break (s, d),
        }
    };
    let infra = infra_error(&s.router.runs.lock());
    let fails = check(case, &s, &file.project.key);
    let rec = record(&s);
    std::fs::write(dir.join("record.md"), &rec).unwrap();
    let (grade, grade_error, grader_cost) = if fails.is_empty() && infra.is_none() {
        match self::grade(&providers, grader, case, &rec).await {
            Ok((g, c)) => (Some(g), None, c),
            Err(e) => (None, Some(e), 0.0),
        }
    } else {
        (None, None, 0.0)
    };
    let runs = s.router.runs.lock().clone();
    let live: Vec<&RunLog> = runs.iter().filter(|r| r.live).collect();
    let input: u64 = live
        .iter()
        .map(|r| r.usage.input_tokens + r.usage.cache_read_tokens + r.usage.cache_write_tokens)
        .sum();
    let cache_ratio = (input > 0)
        .then(|| live.iter().map(|r| r.usage.cache_read_tokens).sum::<u64>() as f64 / input as f64);
    let run_values: Vec<Value> = runs.iter().map(run_json).collect();
    std::fs::write(
        dir.join("runs.json"),
        serde_json::to_string_pretty(&run_values).unwrap(),
    )
    .unwrap();
    // A build of the snapshot is gigabytes; the record and the runs stay.
    let _ = std::fs::remove_dir_all(s.repo.join("target"));
    Outcome {
        case: case.id.clone(),
        tier: case.tier,
        model: model.to_string(),
        stop: s.stop.clone(),
        fails,
        grade,
        grade_error,
        cost: live.iter().map(|r| r.usage.cost_usd).sum(),
        grader_cost,
        live_runs: live.len(),
        asks: s.events.iter().filter(|e| is_question(&e.event)).count(),
        cache_ratio,
        secs: s.secs,
        runs: run_values,
        dir,
        infra,
        retries,
    }
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
#[ignore = "live: runs Ostra's agents on real models and costs money"]
async fn coordination_evals() {
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
        .join(format!("c{stamp}"));
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
    let base = snapshot(&root);

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
    let (file, grader, base) = (&file, grader.as_str(), &base);
    let outcomes: Vec<Outcome> = futures::stream::iter(work)
        .map(|(case, model, dir)| {
            let providers = providers.clone();
            async move {
                let o = run_one(providers, file, &case, &model, grader, base, dir).await;
                println!(
                    "  {:<52} {:<34} {:<5} {:>6.0}s ${:<6.3} live {} asks {} cache {}  {}",
                    o.case,
                    o.model,
                    if o.infra.is_some() {
                        "INFRA"
                    } else if o.pass() {
                        "pass"
                    } else {
                        "FAIL"
                    },
                    o.secs,
                    o.cost,
                    o.live_runs,
                    o.asks,
                    o.cache_ratio
                        .map(|c| format!("{:.0}%", c * 100.0))
                        .unwrap_or_else(|| "-".into()),
                    o.stop
                );
                o
            }
        })
        .buffer_unordered(jobs)
        .collect()
        .await;

    let mut by: BTreeMap<(String, String), Vec<&Outcome>> = BTreeMap::new();
    for o in &outcomes {
        by.entry((o.case.clone(), o.model.clone()))
            .or_default()
            .push(o);
    }
    let mut report = vec![];
    println!("\n{:<52} {:<34} {:>6}  failures", "case", "model", "pass");
    for case in &cases {
        for model in &models {
            let Some(os) = by.get(&(case.id.clone(), model.clone())) else {
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
                "{:<52} {:<34} {:>2}/{:<3}  {}",
                case.id,
                model,
                pass,
                counted.len(),
                why.join(" || ").chars().take(260).collect::<String>()
            );
            for o in os {
                report.push(json!({
                    "case": o.case, "tier": o.tier, "model": o.model, "pass": o.pass(), "stop": o.stop,
                    "failures": o.fails, "grader": o.grade.as_ref().map(|(ok, r)| json!({"pass": ok, "reason": r})),
                    "grader_error": o.grade_error, "cost_usd": o.cost, "grader_cost_usd": o.grader_cost,
                    "live_runs": o.live_runs, "asks": o.asks, "cache_read_share": o.cache_ratio,
                    "seconds": o.secs, "runs": o.runs, "scenario": o.dir,
                    "infra_error": o.infra, "retries": o.retries,
                }));
            }
        }
    }
    println!(
        "\n{:<34} {:>7} {:>7} {:>7} {:>7} {:>9} {:>9} {:>7} {:>9}",
        "model", "tier 1", "tier 2", "tier 3", "all", "cost", "grading", "cache", "avg time"
    );
    for model in &models {
        let os: Vec<&Outcome> = outcomes
            .iter()
            .filter(|o| o.model == *model && o.infra.is_none())
            .collect();
        let rate = |tier: Option<u8>| {
            let t: Vec<&&Outcome> = os
                .iter()
                .filter(|o| tier.is_none_or(|x| o.tier == x))
                .collect();
            if t.is_empty() {
                "-".to_string()
            } else {
                format!(
                    "{:.0}%",
                    100.0 * t.iter().filter(|o| o.pass()).count() as f64 / t.len() as f64
                )
            }
        };
        let cached: Vec<f64> = os.iter().filter_map(|o| o.cache_ratio).collect();
        println!(
            "{model:<34} {:>7} {:>7} {:>7} {:>7} {:>8.2}$ {:>8.2}$ {:>7} {:>8.0}s",
            rate(Some(1)),
            rate(Some(2)),
            rate(Some(3)),
            rate(None),
            os.iter().map(|o| o.cost).sum::<f64>(),
            os.iter().map(|o| o.grader_cost).sum::<f64>(),
            if cached.is_empty() {
                "-".into()
            } else {
                format!(
                    "{:.0}%",
                    100.0 * cached.iter().sum::<f64>() / cached.len() as f64
                )
            },
            os.iter().map(|o| o.secs).sum::<f64>() / os.len().max(1) as f64
        );
    }
    let out = repo_root().join("target/evals");
    std::fs::create_dir_all(&out).unwrap();
    let path = out.join(format!("coordination-{stamp}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("\nReport: {}", path.display());
    let _ = std::fs::remove_dir_all(&data);
    assert!(
        outcomes.is_empty() || outcomes.iter().any(|o| o.live_runs > 0),
        "no live run happened; check the provider credentials and the sandbox"
    );
}

/// Offline: every case parses, and replayed with stand-ins for its live runs, reaches each of them and stops
/// where it says, through the real engine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coordination_cases_run_dry() {
    let file = load_cases();
    let root = scratch_root().join("offline");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let base = snapshot(&root);
    let secret = git(
        &base,
        &["ls-files", "tests/evals", "crates/ostra-server/tests"],
    );
    assert!(
        !secret.contains("coordination"),
        "the snapshot must not hold the eval's expectations"
    );
    let mut ids = std::collections::BTreeSet::new();
    for case in &file.case {
        assert!(ids.insert(case.id.clone()), "duplicate case id {}", case.id);
        assert!((1..=3).contains(&case.tier), "{}: tier 1 to 3", case.id);
        assert!(
            matches!(case.track.as_str(), "light" | "full"),
            "{}: track",
            case.id
        );
        for a in case
            .live
            .iter()
            .chain(&case.stop_after)
            .chain(&case.stop_before)
            .chain(case.actor.iter().chain(&case.dry).map(|a| &a.run))
        {
            addr(a).unwrap_or_else(|e| panic!("{}: {e}", case.id));
        }
        let e = &case.expect;
        for a in e
            .asks
            .iter()
            .map(|x| &x.from)
            .chain(&e.no_asks_from)
            .chain(e.replies.iter().map(|x| &x.from))
            .chain(e.statuses.iter().map(|x| &x.run))
            .chain(e.submits.iter().map(|x| &x.run))
            .chain(&e.continued)
            .chain(&e.cached)
            .chain(&e.no_writes)
        {
            addr(a).unwrap_or_else(|err| panic!("{}: {err}", case.id));
        }
        assert!(
            !e.rubric.trim().is_empty() && !e.cause.trim().is_empty(),
            "{}",
            case.id
        );
        for t in case
            .spec_tools
            .iter()
            .chain(case.actor.iter().chain(&case.dry).flat_map(|a| &a.tools))
        {
            serde_json::from_str::<Value>(&t.input)
                .unwrap_or_else(|err| panic!("{}: tool input is not JSON: {err}", case.id));
        }

        let s = run_session(None, &file, case, "mock:eval", &base, &root.join(&case.id)).await;
        let runs = s.router.runs.lock().clone();
        let seen: Vec<String> = runs.iter().map(|r| r.at.to_string()).collect();
        println!("{}: {} | {}", case.id, seen.join(" > "), s.stop);
        assert!(
            s.stop.starts_with("after ") || s.stop.starts_with("before "),
            "{}: the replay stopped with `{}`, not a stop condition; runs: {seen:?}; failures: {:?}",
            case.id,
            s.stop,
            runs.iter()
                .filter_map(|r| r.error.clone())
                .collect::<Vec<_>>()
        );
        for l in &case.live {
            let a = addr(l).unwrap();
            assert!(
                runs.iter().any(|r| a.matches(&r.at)),
                "{}: live run `{l}` was never reached; runs: {seen:?}",
                case.id
            );
        }
        // Continuing a conversation is the engine's decision, so the replay checks it too.
        for c in &case.expect.continued {
            let a = addr(c).unwrap();
            let r = runs
                .iter()
                .find(|r| a.matches(&r.at))
                .unwrap_or_else(|| panic!("{}: `{c}` was never reached; runs: {seen:?}", case.id));
            assert!(r.continues(), "{}: `{c}` started fresh", case.id);
        }
        assert!(
            runs.iter()
                .all(|r| r.error.is_none() || r.error.as_deref() == Some("stopped by the eval")),
            "{}: a scripted run failed: {:?}",
            case.id,
            runs.iter()
                .filter_map(|r| r.error.clone())
                .collect::<Vec<_>>()
        );
    }
}
