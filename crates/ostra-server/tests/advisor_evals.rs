//! Advisor evals over `tests/evals/advisor.toml`. Live and paid, so ignored by default:
//!
//! ```text
//! OSTRA_EVAL_MODELS=anthropic:claude-opus-5-5,anthropic:claude-sonnet-5-5 OSTRA_EVAL_RUNS=5 \
//!   cargo test -p ostra-server --test advisor_evals -- --ignored --nocapture
//! ```
//!
//! Each case lays out a failed step of a created project's init on disk, builds the advisor's spawn with the
//! engine's own `advisor_request` and spawn factory, and runs the real advisor in the native loop, with the
//! policy and the sandbox, `OSTRA_EVAL_RUNS` times per model. A run passes when the advisor's action matches,
//! its text names every required fact, and the grader model (`OSTRA_EVAL_GRADER`, default Opus) finds the
//! rubric met. The report lists pass counts, actions, cost, and each run's guidance, and is written to
//! `target/evals/`. `OSTRA_EVAL_CASES` filters case ids by substring; `OSTRA_EVAL_JOBS` sets how many runs go
//! at once (default 4). The test fails only when every run fails to finish, because model results vary.

use chrono::Utc;
use futures::StreamExt;
use ostra_core::ExecutorKind;
use ostra_core::agent::{AgentName, InitializerMode};
use ostra_core::config::{
    GlobalConfig, PermissionMode, PermissionRules, ProjectProfile, ResolvedRoute,
    WorkspaceSettings, load_toml, load_toml_required, save_toml,
};
use ostra_core::event::{
    ExecPurpose, ProjectRef, SessionEvent, SessionKind, SessionOptions, StoredEvent,
};
use ostra_core::exec::{
    CancellationToken, ExecContext, ExecutionDelta, ExecutionHost, ExecutionSpec, Executor,
};
use ostra_core::ids::{ExecutionId, SessionId};
use ostra_core::manage::CreatedProject;
use ostra_core::model::{Effort, Tier};
use ostra_core::paths;
use ostra_core::policy::{PermissionAnswer, PolicyDecision, RuleRef, ToolCall};
use ostra_core::submit::AdvisorSubmit;
use ostra_default_plugin::data::InitTrack;
use ostra_default_plugin::factory::AgentsFactory;
use ostra_default_plugin::init::{AdviceInputs, advisor_request, failed_step_label};
use ostra_default_plugin::inputs::OstraInputs;
use ostra_engine::plan::{SpawnInputs, SpawnRequest};
use ostra_engine::services::{BuiltSpawn, SpawnEnv, SpawnFactory};
use ostra_engine::state::stage_of;
use ostra_exec_native::NativeExecutor;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

#[derive(Deserialize)]
struct File {
    default_project: ProjectSpec,
    case: Vec<Case>,
}

#[derive(Deserialize, Clone)]
struct ProjectSpec {
    key: String,
    stack: String,
    purpose: String,
    requirements: Vec<String>,
}

#[derive(Deserialize, Clone)]
struct Case {
    id: String,
    #[serde(default)]
    note: Option<String>,
    mode: String,
    #[serde(default)]
    item: Option<String>,
    /// The failed step's planner inputs, by spawn label.
    #[serde(default)]
    init: BTreeMap<String, String>,
    #[serde(default)]
    problem: Option<String>,
    /// Compute the problem with the runner's inventory and profile check.
    #[serde(default)]
    problem_check: bool,
    /// The failed run's submit payload, as JSON.
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    earlier: Vec<String>,
    #[serde(default)]
    project: Option<ProjectSpec>,
    #[serde(default)]
    file: Vec<FileSpec>,
    expect: Expect,
}

#[derive(Deserialize, Clone)]
struct FileSpec {
    path: String,
    content: String,
}

#[derive(Deserialize, Clone)]
struct Expect {
    action: String,
    /// Groups of alternatives; the advisor's text must contain one of each group.
    #[serde(default)]
    require: Vec<Vec<String>>,
    cause: String,
    rubric: String,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load_cases() -> File {
    let text = std::fs::read_to_string(repo_root().join("tests/evals/advisor.toml")).unwrap();
    toml::from_str(&text).unwrap()
}

/// Scenarios live under the target dir, never `/tmp`, because the sandbox mounts its own `/tmp`.
fn scratch_root() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("advisor-evals")
}

/// The assets dir is process-wide, so every test in this binary shares one.
fn assets() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = scratch_root().join("assets");
        std::fs::create_dir_all(&dir).unwrap();
        let dir = paths::canonical(&dir).unwrap();
        ostra_agents::set_assets_dir(dir.clone());
        ostra_agents::materialize_assets(&dir).unwrap();
        dir
    })
    .clone()
}

fn mode_of(s: &str) -> InitializerMode {
    match s {
        "detect" => InitializerMode::Detect,
        "scout" => InitializerMode::Scout,
        "propose" => InitializerMode::Propose,
        "generate-skill" => InitializerMode::GenerateSkill,
        "generate-inventory" => InitializerMode::GenerateInventory,
        "adopt" => InitializerMode::Adopt,
        other => panic!("unknown mode {other}"),
    }
}

/// One case laid out on disk, with the advisor's spawn built.
struct Scenario {
    ws: PathBuf,
    repo: PathBuf,
    session_root: PathBuf,
    session: PathBuf,
    project: CreatedProject,
    problem: String,
    step_result: Option<Value>,
    advisor: BuiltSpawn,
}

struct Vars {
    pairs: Vec<(&'static str, String)>,
}

impl Vars {
    fn fill(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (k, v) in &self.pairs {
            out = out.replace(k, v);
        }
        out
    }
}

/// The check after a created project's init (`init_problem` in `stages/init/hooks.rs`).
fn init_check(repo: &Path) -> Option<String> {
    if !paths::project_inventory(repo).exists() {
        return Some("the initializer did not write .ostra/INVENTORY.md".into());
    }
    load_toml_required::<ProjectProfile>(&paths::project_profile(repo))
        .err()
        .map(|e| format!("the generated .ostra/project.toml is not valid: {e}"))
}

fn setup(file: &File, case: &Case, dir: &Path) -> Scenario {
    let spec = case.project.clone().unwrap_or(file.default_project.clone());
    std::fs::create_dir_all(dir).unwrap();
    let dir = paths::canonical(dir).unwrap();
    let ws = dir.join("ws");
    let repo = ws.join(&spec.key);
    let session_root = ws.join(".ostra/sessions/s_eval");
    let session = session_root.join(&spec.key);
    for d in [&repo, &session] {
        std::fs::create_dir_all(d).unwrap();
    }
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(git.success());
    let vars = Vars {
        pairs: vec![
            ("{ws}", ws.display().to_string()),
            ("{repo}", repo.display().to_string()),
            ("{session}", session.display().to_string()),
            ("{assets}", assets().display().to_string()),
            ("{key}", spec.key.clone()),
        ],
    };
    for f in &case.file {
        let path = PathBuf::from(vars.fill(&f.path));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vars.fill(&f.content)).unwrap();
    }

    let project = CreatedProject {
        key: spec.key.clone(),
        path: repo.clone(),
        stack: spec.stack.clone(),
        purpose: spec.purpose.clone(),
        requirements: spec.requirements.clone(),
        execution: ExecutionId::from("x_implementer"),
        agent: AgentName::Implementer,
    };
    let events = [
        SessionEvent::SessionCreated {
            kind: SessionKind::Pipeline,
            request: "Build an MCP server in Rust for the team's notes.".into(),
            options: SessionOptions::default(),
            projects: vec![ProjectRef {
                key: "notes-api".into(),
                path: ws.join("notes-api"),
            }],
            workspace_root: ws.clone(),
            session_root: session_root.clone(),
            files: vec![],
            uploads: vec![],
            pinned: vec![],
            docs_book: None,
            workflow: None,
        },
        SessionEvent::ProjectCreated {
            project: project.clone(),
        },
    ];
    let stored: Vec<StoredEvent> = events
        .into_iter()
        .enumerate()
        .map(|(i, event)| StoredEvent {
            seq: i as i64 + 1,
            at: Utc::now(),
            event,
        })
        .collect();
    ostra_default_plugin::install();
    let state = ostra_default_plugin::fold_session(SessionId::from("s_eval"), &stored);
    let focus = InitTrack::created_focus(&project);

    let settings = WorkspaceSettings::seeded("eval");
    let profile: Option<ProjectProfile> = load_toml(&paths::project_profile(&repo)).ok();
    let inventory = std::fs::read_to_string(paths::project_inventory(&repo)).ok();
    let docs = ostra_agents::brief::project_docs(&repo);
    let env = SpawnEnv {
        state: &state,
        executor: ExecutorKind::Native,
        settings: &settings,
        profile: profile.as_ref(),
        inventory: inventory.as_deref(),
        repo_root: &repo,
        project_docs: &docs,
        work_dirs: &[],
        agents: &ostra_agents::AgentCatalog::builtin(),
    };

    // The failed step's own spawn block, as the engine rendered it for that run.
    let mode = mode_of(&case.mode);
    let mut init: BTreeMap<String, String> = case
        .init
        .iter()
        .map(|(k, v)| (k.clone(), vars.fill(v)))
        .collect();
    if mode == InitializerMode::Detect {
        init.insert("User focus".into(), focus.clone());
    }
    let purpose = ExecPurpose::Init {
        mode,
        item: case.item.clone(),
    };
    let step = SpawnRequest {
        also: Vec::new(),
        agent: AgentName::Initializer,
        stage: stage_of(&purpose),
        purpose: purpose.clone(),
        project: spec.key.clone(),
        session_dir: session.clone(),
        inputs: SpawnInputs {
            extra: OstraInputs {
                init,
                init_item: case.item.clone(),
                ..Default::default()
            }
            .into_value(),
            ..Default::default()
        },
        resumes: None,
        continues: None,
    };
    let step_block = AgentsFactory
        .build(&step, &env)
        .unwrap_or_else(|e| panic!("{}: the failed step does not build: {e}", case.id))
        .spawn_block;

    let step_result: Option<Value> = case.result.as_ref().map(|r| {
        serde_json::from_str(&vars.fill(r))
            .unwrap_or_else(|e| panic!("{}: result is not JSON: {e}", case.id))
    });
    let problem = if case.problem_check {
        let p = init_check(&repo)
            .unwrap_or_else(|| panic!("{}: problem_check found nothing wrong", case.id));
        format!("The init did not finish: {p}.")
    } else if let Some(p) = &case.problem {
        vars.fill(p)
    } else {
        // What the engine writes for a stuck step.
        let r = step_result
            .as_ref()
            .unwrap_or_else(|| panic!("{}: no problem and no result", case.id));
        ostra_default_plugin::fold::stuck_problem(
            r["summary"].as_str().unwrap_or_default(),
            r["stuck"]["need"].as_str().unwrap_or_default(),
        )
    };
    let request = advisor_request(AdviceInputs {
        advisor: AgentName::Advisor,
        project: spec.key.clone(),
        session_dir: session.clone(),
        execution: ExecutionId::from("x_failed"),
        failed_step: failed_step_label(AgentName::Initializer, &purpose),
        problem: problem.clone(),
        step_inputs: step_block,
        step_result: step_result.clone(),
        context: focus,
        earlier: case.earlier.iter().map(|g| vars.fill(g)).collect(),
    });
    let advisor = AgentsFactory
        .build(&request, &env)
        .unwrap_or_else(|e| panic!("{}: the advisor spawn does not build: {e}", case.id));
    Scenario {
        ws,
        repo,
        session_root,
        session,
        project,
        problem,
        step_result,
        advisor,
    }
}

/// Records what the advisor did, and refuses every permission ask, as a user who is away would.
#[derive(Default)]
struct Host {
    calls: Mutex<Vec<String>>,
    asks: Mutex<Vec<String>>,
    denials: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl ExecutionHost for Host {
    fn emit(&self, delta: ExecutionDelta) {
        match delta {
            ExecutionDelta::ToolCall { call, .. } => {
                let input: String = call.input.to_string().chars().take(200).collect();
                self.calls.lock().push(format!("{} {input}", call.tool));
            }
            ExecutionDelta::Policy {
                decision: PolicyDecision::Deny { rule, reason },
                ..
            } => {
                let reason: String = reason.chars().take(160).collect();
                self.denials
                    .lock()
                    .push(format!("{} {}: {reason}", rule.layer, rule.rule));
            }
            _ => {}
        }
    }

    async fn ask_permission(&self, call: &ToolCall, reason: &str, _: &RuleRef) -> PermissionAnswer {
        self.asks.lock().push(format!("{}: {reason}", call.tool));
        PermissionAnswer::Deny
    }
}

struct RunOutcome {
    case: String,
    model: String,
    status: String,
    error: Option<String>,
    submit: Option<AdvisorSubmit>,
    action_ok: bool,
    missing: Vec<String>,
    grade: Option<(bool, String)>,
    grade_error: Option<String>,
    cost: f64,
    grader_cost: f64,
    secs: f64,
    calls: Vec<String>,
    asks: Vec<String>,
    denials: Vec<String>,
    dir: PathBuf,
}

impl RunOutcome {
    fn pass(&self) -> bool {
        self.action_ok && self.missing.is_empty() && self.grade.as_ref().is_some_and(|(ok, _)| *ok)
    }
}

const GRADER: &str = "You grade one decision of Ostra's advisor agent against a rubric the eval author wrote.

The advisor read a pipeline step that failed or got stuck, and chose `retry` with `guidance` (instructions for the step's next run) or `escalate` with a `reason` (for the user). The input gives the case, what actually caused the failure, the rubric, and the advisor's decision as JSON.

Apply the rubric literally. Pass only when every point holds in the advisor's text. A point the text does not state fails, even when the advisor may have meant it, because the next run and the user read only the text. Judge meaning, not wording: a paraphrase that states the same fact passes, and extra correct detail does not hurt. Guidance that also tells the step to do something the rubric forbids fails.

Call `decide` once with `verdict` (`pass` or `fail`) and `reason`: one or two sentences naming the rubric point that decided it.";

async fn grade(
    providers: &ostra_providers::Providers,
    grader: &str,
    case: &Case,
    submit: &AdvisorSubmit,
) -> Result<((bool, String), f64), String> {
    let (p, m) = providers.for_model(grader).map_err(|e| e.to_string())?;
    let decision =
        json!({"action": submit.action, "guidance": submit.guidance, "reason": submit.reason});
    let user = format!(
        "# Case\n\n{}\n\n# What actually caused the failure\n\n{}\n\n# Rubric\n\n{}\n\n# The advisor's decision\n\n```json\n{}\n```",
        case.note.clone().unwrap_or_default(),
        case.expect.cause,
        case.expect.rubric,
        serde_json::to_string_pretty(&decision).unwrap_or_default()
    );
    let schema = json!({"type": "object", "properties": {
        "verdict": {"type": "string", "enum": ["pass", "fail"]},
        "reason": {"type": "string"}
    }, "required": ["verdict", "reason"], "additionalProperties": false});
    let (out, usage) =
        ostra_providers::structured(p.as_ref(), &m, GRADER, &user, schema, Effort::Low)
            .await
            .map_err(|e| e.to_string())?;
    let cost = ostra_core::pricing::cost(&m, &usage, 0);
    Ok((
        (
            out["verdict"].as_str() == Some("pass"),
            out["reason"].as_str().unwrap_or_default().to_string(),
        ),
        cost,
    ))
}

fn missing_facts(case: &Case, submit: &AdvisorSubmit) -> Vec<String> {
    let text = format!("{}\n{}", submit.guidance, submit.reason).to_lowercase();
    case.expect
        .require
        .iter()
        .filter(|group| !group.iter().any(|alt| text.contains(&alt.to_lowercase())))
        .map(|group| group.join(" | "))
        .collect()
}

async fn run_one(
    providers: Arc<ostra_providers::Providers>,
    file: &File,
    case: &Case,
    model: &str,
    grader: &str,
    dir: PathBuf,
) -> RunOutcome {
    let sc = setup(file, case, &dir);
    let def = ostra_agents::agent_def(AgentName::Advisor);
    let ctx = ExecContext {
        work_dirs: Vec::new(),
        execution_id: ExecutionId::new(),
        session_id: Some(SessionId::from("s_eval")),
        agent: AgentName::Advisor,
        initializer_mode: None,
        executor: ExecutorKind::Native,
        workspace_root: sc.ws.clone(),
        repo_root: sc.repo.clone(),
        project_key: sc.project.key.clone(),
        session_dir: sc.session.clone(),
        session_root: sc.session_root.clone(),
        report_file: None,
        phase: None,
        yolo: false,
        permission_mode: PermissionMode::Default,
        permissions: PermissionRules::default(),
        protected_paths: vec![
            paths::global_config_path(),
            paths::data_dir(),
            assets(),
            paths::workspace_toml(&sc.ws),
        ],
        memory_db: paths::project_memory_db(&sc.repo),
        sandbox_mode: None,
        enforce_tool_calls: false,
        sandbox_network: None,
        sandbox_allowed_hosts: vec![],
        sandbox_decoys: vec![],
        sandbox_readable: vec![],
        sandbox_loopback: Default::default(),
        sandbox_blocked_ports: vec![],
        creates_project: false,
        owes_reply: false,
        write_scope: Some(def.write_scope),
        contract: def.returns,
        capabilities: def.capabilities.clone(),
    };
    let spec = ExecutionSpec {
        id: ctx.execution_id.clone(),
        agent: AgentName::Advisor,
        route: ResolvedRoute {
            executor: ExecutorKind::Native,
            model: model.to_string(),
            tier: Some(Tier::Advanced),
        },
        effort: sc.advisor.effort,
        system_prompt: sc.advisor.system_prompt.clone(),
        first_message: sc.advisor.first_message.clone(),
        capabilities: def.capabilities.clone(),
        submit_schema: def.submit_schema(),
        timeout_secs: def.timeout_secs,
        ctx,
        resume: None,
        harness_session_id: None,
    };
    std::fs::write(dir.join("first-message.md"), &spec.first_message).unwrap();
    let host = Arc::new(Host::default());
    let executor =
        NativeExecutor::new(providers.clone(), ostra_server::app::skill_resolver(), None);
    let started = Instant::now();
    let result = executor
        .run(spec, host.clone(), CancellationToken::new())
        .await;
    let secs = started.elapsed().as_secs_f64();
    let submit: Option<AdvisorSubmit> = result
        .submit
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let (action_ok, missing) = match &submit {
        Some(s) => (
            serde_json::to_value(s.action)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                == Some(case.expect.action.clone()),
            missing_facts(case, s),
        ),
        None => (false, vec![]),
    };
    let (grade, grade_error, grader_cost) = match &submit {
        Some(s) => match self::grade(&providers, grader, case, s).await {
            Ok((g, c)) => (Some(g), None, c),
            Err(e) => (None, Some(e), 0.0),
        },
        None => (None, None, 0.0),
    };
    RunOutcome {
        case: case.id.clone(),
        model: model.to_string(),
        status: format!("{:?}", result.status).to_lowercase(),
        error: result.error.clone(),
        submit,
        action_ok,
        missing,
        grade,
        grade_error,
        cost: result.usage.cost_usd,
        grader_cost,
        secs,
        calls: host.calls.lock().clone(),
        asks: host.asks.lock().clone(),
        denials: host.denials.lock().clone(),
        dir,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: runs the advisor agent on real models and costs money"]
async fn advisor_evals() {
    let file = load_cases();
    let filter = std::env::var("OSTRA_EVAL_CASES").unwrap_or_default();
    let cases: Vec<Case> = file
        .case
        .iter()
        .filter(|c| filter.split(',').any(|f| c.id.contains(f.trim())))
        .cloned()
        .collect();
    let models: Vec<String> = std::env::var("OSTRA_EVAL_MODELS")
        .unwrap_or_else(|_| "anthropic:claude-opus-5-5,anthropic:claude-sonnet-5-5".into())
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    let runs: usize = std::env::var("OSTRA_EVAL_RUNS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(5);
    let jobs: usize = std::env::var("OSTRA_EVAL_JOBS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(4);
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
    // The data dir holds the sandbox's egress sockets, whose paths must stay short, and must not be
    // under `/tmp`, which the sandbox replaces with its own.
    let home = paths::home().expect("a home directory");
    let data = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"))
        .join("ostra-evals")
        .join(&stamp);
    std::fs::create_dir_all(&data).unwrap();
    let config = data.join("config.toml");
    save_toml(&config, &GlobalConfig::default()).unwrap();
    // SAFETY: set once, before any run starts, and this binary's other test reads none of them.
    unsafe {
        std::env::set_var("OSTRA_CONFIG", &config);
        std::env::set_var("OSTRA_DATA_DIR", data.join("data"));
        std::env::set_var("OSTRA_SANDBOX_CACHE", data.join("sandbox-cache"));
        std::env::set_var("OSTRA_MODELS_DEV_URL", "");
    }
    paths::ensure_data_dir().unwrap();
    assets();
    let providers = Arc::new(ostra_providers::Providers::from_config(
        &GlobalConfig::default(),
        &Default::default(),
    ));

    // Run by run, alternating models and cases, so the runs in flight at once are spread evenly
    // instead of one model hitting one case several times together.
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
        "\n{} runs: {} cases x {} models x {runs}. Scenarios in {}",
        work.len(),
        cases.len(),
        models.len(),
        root.display()
    );
    let file = &file;
    let grader = grader.as_str();
    let outcomes: Vec<RunOutcome> = futures::stream::iter(work)
        .map(|(case, model, dir)| {
            let providers = providers.clone();
            async move {
                let o = run_one(providers, file, &case, &model, grader, dir).await;
                println!(
                    "  {:<40} {:<28} {:<6} {:>6.1}s ${:.3}{}",
                    o.case,
                    o.model,
                    if o.pass() { "pass" } else { "FAIL" },
                    o.secs,
                    o.cost,
                    o.submit
                        .as_ref()
                        .map(|s| format!(" {:?}", s.action).to_lowercase())
                        .unwrap_or_else(|| format!(" no submit ({})", o.status))
                );
                o
            }
        })
        .buffer_unordered(jobs)
        .collect()
        .await;

    let mut by: BTreeMap<(String, String), Vec<&RunOutcome>> = BTreeMap::new();
    for o in &outcomes {
        by.entry((o.case.clone(), o.model.clone()))
            .or_default()
            .push(o);
    }
    let mut report = vec![];
    let mut totals: BTreeMap<String, (usize, usize, usize, f64, f64, f64)> = BTreeMap::new();
    println!(
        "\n{:<40} {:<28} {:>6}  actions, failures",
        "case", "model", "pass"
    );
    for case in &cases {
        for model in &models {
            let Some(os) = by.get(&(case.id.clone(), model.clone())) else {
                continue;
            };
            let pass = os.iter().filter(|o| o.pass()).count();
            let mut actions: BTreeMap<String, usize> = BTreeMap::new();
            let mut why = vec![];
            for o in os {
                let a = o
                    .submit
                    .as_ref()
                    .map(|s| format!("{:?}", s.action).to_lowercase())
                    .unwrap_or_else(|| "none".into());
                *actions.entry(a).or_default() += 1;
                if !o.pass() {
                    if o.submit.is_none() {
                        why.push(format!(
                            "no submit: {}",
                            o.error.clone().unwrap_or_default()
                        ));
                    } else if !o.action_ok {
                        why.push("wrong action".into());
                    } else if !o.missing.is_empty() {
                        why.push(format!("missing {}", o.missing.join(", ")));
                    } else if let Some((_, r)) = &o.grade {
                        why.push(format!("grader: {r}"));
                    } else if let Some(e) = &o.grade_error {
                        why.push(format!("grader error: {e}"));
                    }
                }
            }
            let t = totals.entry(model.clone()).or_default();
            t.0 += os.len();
            t.1 += pass;
            t.2 += os.iter().filter(|o| o.submit.is_none()).count();
            t.3 += os.iter().map(|o| o.cost).sum::<f64>();
            t.4 += os.iter().map(|o| o.grader_cost).sum::<f64>();
            t.5 += os.iter().map(|o| o.secs).sum::<f64>();
            let actions_text = actions
                .iter()
                .map(|(a, n)| format!("{a} x{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "{:<40} {:<28} {:>2}/{:<3}  {actions_text}{}",
                case.id,
                model,
                pass,
                os.len(),
                if why.is_empty() {
                    String::new()
                } else {
                    format!("; {}", why.join("; ").chars().take(220).collect::<String>())
                }
            );
            for o in os {
                report.push(json!({
                    "case": o.case, "model": o.model, "expected": case.expect.action, "pass": o.pass(),
                    "status": o.status, "error": o.error,
                    "action": o.submit.as_ref().map(|s| s.action),
                    "guidance": o.submit.as_ref().map(|s| s.guidance.clone()),
                    "reason": o.submit.as_ref().map(|s| s.reason.clone()),
                    "action_ok": o.action_ok, "missing": o.missing,
                    "grader": o.grade.as_ref().map(|(ok, r)| json!({"pass": ok, "reason": r})),
                    "grader_error": o.grade_error,
                    "cost_usd": o.cost, "grader_cost_usd": o.grader_cost, "seconds": o.secs,
                    "tool_calls": o.calls, "asks_denied": o.asks, "policy_denials": o.denials,
                    "scenario": o.dir,
                }));
            }
        }
    }
    println!(
        "\n{:<28} {:>9} {:>10} {:>10} {:>10} {:>9}",
        "model", "accuracy", "no submit", "cost", "grading", "avg time"
    );
    for (model, (n, pass, none, cost, gcost, secs)) in &totals {
        println!(
            "{model:<28} {:>8.0}% {none:>10} {:>9.2}$ {:>9.2}$ {:>8.0}s",
            100.0 * *pass as f64 / (*n).max(1) as f64,
            cost,
            gcost,
            secs / (*n).max(1) as f64
        );
    }
    let out = repo_root().join("target/evals");
    std::fs::create_dir_all(&out).unwrap();
    let path = out.join(format!("advisor-{stamp}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("\nReport: {}", path.display());
    let _ = std::fs::remove_dir_all(&data);
    let finished = outcomes.iter().filter(|o| o.submit.is_some()).count();
    assert!(
        finished > 0 || outcomes.is_empty(),
        "no advisor run finished; check the provider credentials and the sandbox"
    );
}

/// Offline: every case lays out, its problem is what the engine would write, and the advisor's spawn builds
/// with every input the engine passes.
#[test]
fn advisor_cases_build_their_spawns() {
    let file = load_cases();
    let root = scratch_root().join("offline");
    let _ = std::fs::remove_dir_all(&root);
    let mut ids = std::collections::BTreeSet::new();
    for case in &file.case {
        assert!(ids.insert(case.id.clone()), "duplicate case id {}", case.id);
        assert!(
            matches!(case.expect.action.as_str(), "retry" | "escalate"),
            "{}: action must be retry or escalate",
            case.id
        );
        let sc = setup(&file, case, &root.join(&case.id));
        let first = &sc.advisor.first_message;
        // Kept for whoever writes a case: what the advisor will read.
        std::fs::write(root.join(&case.id).join("first-message.md"), first).unwrap();
        for label in [
            "Failed step:",
            "Problem:",
            "Step inputs:",
            "Step context:",
            "Repo root:",
        ] {
            assert!(first.contains(label), "{}: no `{label}` line", case.id);
        }
        assert!(
            first.contains(&format!("Mode: {}", case.mode)),
            "{}: the step inputs carry the failed step's block",
            case.id
        );
        assert_eq!(
            first.contains("Step result:"),
            sc.step_result.is_some(),
            "{}: the step result reaches the advisor when the step made one",
            case.id
        );
        assert_eq!(
            first.contains("Earlier guidance:"),
            !case.earlier.is_empty(),
            "{}",
            case.id
        );
        assert!(!sc.problem.trim().is_empty(), "{}: empty problem", case.id);
        for p in ["{ws}", "{repo}", "{session}", "{assets}"] {
            assert!(!first.contains(p), "{}: unfilled {p}", case.id);
        }
        assert!(
            sc.advisor.system_prompt.contains("# Advisor Agent"),
            "{}: the advisor's own prompt",
            case.id
        );
        assert!(
            sc.repo.join(".git").exists(),
            "{}: a created project is a git repo",
            case.id
        );
    }
    let check = file
        .case
        .iter()
        .find(|c| c.id == "inventory-letter-case")
        .unwrap();
    let sc = setup(&file, check, &root.join("again"));
    assert_eq!(
        sc.problem,
        "The init did not finish: the initializer did not write .ostra/INVENTORY.md."
    );
}
