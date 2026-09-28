//! Judge routing evals over `tests/evals/judges.toml`. Live and paid, so ignored by default:
//!
//! ```text
//! OSTRA_EVAL_MODELS=anthropic:claude-opus-5-5,anthropic:claude-sonnet-5 OSTRA_EVAL_RUNS=3 \
//!   cargo test -p ostra-server --test judge_evals -- --ignored --nocapture
//! ```
//!
//! Each case builds the session state its judge would see, through the same fold and `judge_input` the
//! engine uses, and calls the judge `OSTRA_EVAL_RUNS` times per model. The report lists pass counts, the
//! spread of answers, and cost, and is written to `target/evals/`. `OSTRA_EVAL_CASES` filters case ids by
//! substring. The test fails only when calls fail, because model results vary run to run.

use chrono::Utc;
use futures::StreamExt;
use ostra_core::event::{
    AnswerSource, GateAnswer, GatePayload, JudgeKind, ProjectRef, SessionEvent, SessionKind,
    SessionOptions, StoredEvent, WorkKind,
};
use ostra_core::exec::{ExecutionResult, ExecutionStatus, Usage};
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId};
use ostra_core::model::Effort;
use ostra_core::pipeline::StageKind;
use ostra_core::{AgentName, ExecutorKind};
use ostra_engine::judge::output_schema;
use ostra_engine::judge_input::{ProjectFacts, judge_input};
use ostra_engine::state::SessionState;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct File {
    case: Vec<Case>,
}

#[derive(Deserialize, Clone)]
struct Case {
    id: String,
    judge: String,
    workspace: String,
    request: String,
    #[serde(default)]
    track: Option<String>,
    #[serde(default)]
    feedback: Option<String>,
    #[serde(default)]
    spec: Option<String>,
    #[serde(default)]
    research: Vec<Research>,
    #[serde(default)]
    phase: Vec<Phase>,
    #[serde(default)]
    note: Option<String>,
    expect: Expect,
}

#[derive(Deserialize, Clone)]
struct Research {
    project: String,
    task: String,
    scope: String,
    findings: String,
    #[serde(default)]
    not_covered: Vec<String>,
    doc: String,
}

#[derive(Deserialize, Clone)]
struct Phase {
    project: String,
    report: String,
}

#[derive(Deserialize, Clone, Default)]
struct Expect {
    #[serde(default)]
    category: Vec<String>,
    #[serde(default)]
    projects: Vec<String>,
    #[serde(default)]
    track: Option<String>,
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    targets: Vec<String>,
    /// Stakes levels that pass.
    #[serde(default)]
    stakes: Vec<String>,
    /// Sufficiency: Not covered items (by a unique substring) the judge must mark needed, and not needed.
    #[serde(default)]
    needed: Vec<String>,
    #[serde(default)]
    not_needed: Vec<String>,
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Each project: key, path, stack, and the module map rows an init would write for it.
fn projects(workspace: &str) -> Vec<(String, PathBuf, String, Vec<String>)> {
    let root = repo().canonicalize().unwrap();
    let areas = |rows: &[&str]| rows.iter().map(|r| r.to_string()).collect::<Vec<_>>();
    let server = areas(&[
        "core types, events, API DTOs (crates/ostra-core/**)",
        "engine: fold, planner, judges, runner (crates/ostra-engine/**)",
        "SQLite stores (crates/ostra-store/**)",
        "sandbox and egress proxy (crates/ostra-sandbox/**)",
        "policy guards (crates/ostra-policy/**)",
        "HTTP server and CLI (crates/ostra-server/**)",
        "agent and judge prompts (assets/**)",
    ]);
    let web = areas(&[
        "gate cards (src/features/gates/**)",
        "session board and screens (src/screens/**)",
        "generated API types (src/api/gen/**)",
    ]);
    match workspace {
        "triple" => vec![
            ("server".into(), root.clone(), "rust".into(), server),
            (
                "web".into(),
                root.join("web"),
                "typescript-react".into(),
                web,
            ),
            (
                "site".into(),
                root.join("site"),
                "typescript-react".into(),
                areas(&[
                    "homepage (src/home/**)",
                    "docs site renderer and navigation (src/docs/**)",
                    "docs pages rendered by the site (../docs/**/*.md)",
                ]),
            ),
        ],
        "split" => vec![
            ("server".into(), root.clone(), "rust".into(), server),
            (
                "web".into(),
                root.join("web"),
                "typescript-react".into(),
                web,
            ),
        ],
        _ => {
            let mut all = server;
            all.push("browser console (web/src/**)".into());
            vec![("ostra".into(), root, "rust".into(), all)]
        }
    }
}

fn kind(judge: &str) -> JudgeKind {
    match judge {
        "classify" => JudgeKind::Classify,
        "track" => JudgeKind::Track,
        "feedback" => JudgeKind::Feedback,
        "sufficiency" => JudgeKind::Sufficiency,
        "stakes" => JudgeKind::Stakes,
        other => panic!("no eval support for judge {other}"),
    }
}

struct Log {
    events: Vec<StoredEvent>,
}

impl Log {
    fn ev(&mut self, event: SessionEvent) {
        self.events.push(StoredEvent {
            seq: self.events.len() as i64 + 1,
            at: Utc::now(),
            event,
        });
    }

    fn decide(&mut self, judge: JudgeKind, output: Value) {
        self.ev(SessionEvent::DecisionMade {
            id: DecisionId::new(),
            judge,
            subject: None,
            input_summary: String::new(),
            output,
            reason: "eval".into(),
        });
    }

    fn run(
        &mut self,
        agent: AgentName,
        purpose: ostra_core::event::ExecPurpose,
        project: &str,
        submit: Value,
    ) {
        let id = ExecutionId::new();
        self.ev(SessionEvent::ExecutionStarted {
            id: id.clone(),
            agent,
            purpose,
            stage: StageKind::Explore,
            project: project.into(),
            executor: ExecutorKind::Native,
            model: "eval".into(),
            params: json!({}),
            spawn_block: String::new(),
            report_path: None,
            resumes: None,
        });
        self.ev(SessionEvent::ExecutionFinished {
            id,
            result: ExecutionResult {
                status: ExecutionStatus::Ok,
                submit: Some(submit),
                final_text: String::new(),
                usage: Usage::default(),
                native_session_id: None,
                error: None,
            },
        });
    }
}

/// The session a judge sees at the moment the engine would ask it, with its files in `dir`.
fn session(case: &Case, dir: &Path) -> SessionState {
    let ws = projects(&case.workspace);
    let session_root = dir.join("session");
    std::fs::create_dir_all(&session_root).unwrap();
    let mut log = Log { events: vec![] };
    log.ev(SessionEvent::SessionCreated {
        kind: SessionKind::Pipeline,
        request: case.request.clone(),
        options: SessionOptions::default(),
        projects: ws
            .iter()
            .map(|(k, p, _, _)| ProjectRef {
                key: k.clone(),
                path: p.clone(),
            })
            .collect(),
        workspace_root: dir.to_path_buf(),
        session_root: session_root.clone(),
        files: vec![],
        uploads: vec![],
    });
    if case.judge == "classify" {
        return SessionState::fold(SessionId::from("eval"), &log.events);
    }
    let scope: Vec<String> = ws.iter().map(|(k, _, _, _)| k.clone()).collect();
    let tasks: Vec<Value> = case
        .research
        .iter()
        .map(|r| json!({"project": r.project, "task": r.task}))
        .collect();
    log.decide(
        JudgeKind::Classify,
        json!({"category": "IMPLEMENT", "projects": scope, "explore_tasks": tasks, "opts_in": {"tests": false, "docs": false}, "reason": "eval", "title": "Eval"}),
    );
    for (i, r) in case.research.iter().enumerate() {
        let path = session_root.join(format!("ostra-research-{i}.md"));
        std::fs::write(&path, &r.doc).unwrap();
        log.run(
            AgentName::Explore,
            ostra_core::event::ExecPurpose::Explore { task: i as u32 },
            &r.project,
            json!({"research_path": path, "scope_covered": r.scope, "findings_summary": r.findings, "sources_retrieved": 0, "open_questions": 0, "not_covered": r.not_covered}),
        );
    }
    if matches!(case.judge.as_str(), "track" | "sufficiency") {
        return SessionState::fold(SessionId::from("eval"), &log.events);
    }
    let full = case.track.as_deref() == Some("full");
    log.decide(
        JudgeKind::Track,
        json!({"track": if full { "full" } else { "light" }, "reason": "eval"}),
    );
    if let Some(spec) = &case.spec {
        let path = session_root.join("ostra-spec-1.md");
        std::fs::write(&path, spec).unwrap();
        log.run(
            AgentName::GenerateSpec,
            ostra_core::event::ExecPurpose::Spec { round: 1 },
            &scope[0],
            json!({"spec_path": path, "open_questions": [], "external_evidence_rows": 0, "deliverables": 1, "requirements": 4, "summary": "The approved spec."}),
        );
    }
    if case.judge == "stakes" {
        return SessionState::fold(SessionId::from("eval"), &log.events);
    }
    if full {
        // Low stakes builds one inline phase per project, as the light track does.
        log.decide(
            JudgeKind::Stakes,
            json!({"stakes": "low", "reason": "eval"}),
        );
    }
    let phases: Vec<u32> = SessionState::fold(SessionId::from("eval"), &log.events)
        .phases
        .keys()
        .copied()
        .collect();
    for (id, p) in phases.iter().zip(&case.phase) {
        let path = session_root
            .join(&p.project)
            .join(format!("ostra-implementer-phase-{id}.md"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("# Phase {id} report\n\n{}\n", p.report)).unwrap();
        log.run(
            AgentName::Implementer,
            ostra_core::event::ExecPurpose::Implement {
                phase: *id,
                work: WorkKind::Initial,
            },
            &p.project,
            json!({"status": "ok", "report_path": path, "changed_files": [], "summary": p.report}),
        );
    }
    let gate = GateId::new();
    log.ev(SessionEvent::GateOpened {
        id: gate.clone(),
        title: "Review the implementation".into(),
        explanation: String::new(),
        payload: GatePayload::ImplementationReview {
            round: 1,
            context_path: session_root.join("ostra-session-context.md"),
            reports: vec![],
            blocked: vec![],
        },
    });
    log.ev(SessionEvent::GateAnswered {
        id: gate,
        source: AnswerSource::User,
        answer: GateAnswer::Choice {
            option: "feedback".into(),
            text: case.feedback.clone(),
        },
        reason: None,
    });
    SessionState::fold(SessionId::from("eval"), &log.events)
}

/// The research tasks with Not covered items, as the planner names them for the Sufficiency judge.
fn sufficiency_subject(case: &Case) -> String {
    case.research
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.not_covered.is_empty())
        .map(|(i, _)| i.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn sorted(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    x.as_str()
                        .or_else(|| x["project"].as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out.dedup();
    out
}

/// The answer's route-deciding part, as one comparable label, and whether it matches.
fn score(case: &Case, out: &Value) -> (String, bool) {
    let e = &case.expect;
    let mut want_projects = e.projects.clone();
    want_projects.sort();
    let mut want_targets = e.targets.clone();
    want_targets.sort();
    match case.judge.as_str() {
        "classify" => {
            let cat = out["category"].as_str().unwrap_or("?").to_string();
            let got = sorted(&out["projects"]);
            let ok =
                e.category.contains(&cat) && (want_projects.is_empty() || got == want_projects);
            (format!("{cat} {}", got.join("+")), ok)
        }
        "track" => {
            let t = out["track"].as_str().unwrap_or("?").to_string();
            let ok = e.track.as_deref() == Some(t.as_str());
            (t, ok)
        }
        "stakes" => {
            let t = out["stakes"].as_str().unwrap_or("?").to_string();
            let ok = e.stakes.contains(&t);
            (t, ok)
        }
        "sufficiency" => {
            let items = out["items"].as_array().cloned().unwrap_or_default();
            let marked = |want: &str| {
                items
                    .iter()
                    .find(|i| i["item"].as_str().is_some_and(|t| t.contains(want)))
                    .and_then(|i| i["needed"].as_bool())
            };
            let mut label = vec![];
            let mut ok = true;
            for (list, want) in [(&e.needed, true), (&e.not_needed, false)] {
                for n in list {
                    let got = marked(n);
                    ok &= got == Some(want);
                    let short: String = n.chars().take(18).collect();
                    label.push(match got {
                        Some(true) => format!("{short}:needed"),
                        Some(false) => format!("{short}:no"),
                        None => format!("{short}:missing"),
                    });
                }
            }
            (label.join(" "), ok)
        }
        _ => {
            let route = out["route"].as_str().unwrap_or("?").to_string();
            let got = sorted(&out["targets"]);
            let ok = e.route.as_deref() == Some(route.as_str()) && got == want_targets;
            (format!("{route} {}", got.join("+")), ok)
        }
    }
}

struct Outcome {
    case: String,
    model: String,
    label: Result<(String, bool, String), String>,
    cost: f64,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: calls the judge models and costs money"]
async fn judge_routing_evals() {
    let text = std::fs::read_to_string(repo().join("tests/evals/judges.toml")).unwrap();
    let file: File = toml::from_str(&text).unwrap();
    let filter = std::env::var("OSTRA_EVAL_CASES").unwrap_or_default();
    let cases: Vec<Case> = file
        .case
        .into_iter()
        .filter(|c| c.id.contains(&filter))
        .collect();
    let models: Vec<String> = std::env::var("OSTRA_EVAL_MODELS")
        .unwrap_or_else(|_| "anthropic:claude-opus-5-5".into())
        .split(',')
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    let runs: usize = std::env::var("OSTRA_EVAL_RUNS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(3);
    let providers = ostra_providers::Providers::from_config(
        &ostra_core::config::GlobalConfig::default(),
        &Default::default(),
    );
    // Prices come from the server's cached models.dev catalog, when one exists.
    if let Some(c) = std::fs::read_to_string(ostra_server::prices::cache_path(
        &ostra_core::paths::data_dir(),
    ))
    .ok()
    .and_then(|t| ostra_core::pricing::Catalog::from_models_dev(&t).ok())
    {
        ostra_core::pricing::install(c);
    }
    let dir = tempfile::tempdir().unwrap();

    let mut jobs = vec![];
    for case in &cases {
        let case_dir = dir.path().join(&case.id);
        let st = session(case, &case_dir);
        let k = kind(&case.judge);
        let sufficiency = sufficiency_subject(case);
        let subject = match k {
            JudgeKind::Feedback => Some("0"),
            JudgeKind::Sufficiency => Some(sufficiency.as_str()),
            _ => None,
        };
        let facts: Vec<ProjectFacts> = projects(&case.workspace)
            .into_iter()
            .map(|(key, path, stack, areas)| ProjectFacts {
                key,
                path: path.display().to_string(),
                initialized: true,
                stack: Some(stack),
                areas,
            })
            .collect();
        let (input, _) = judge_input(&st, k, subject, &facts);
        let system = ostra_agents::judge_prompt(k.as_str())
            .unwrap_or_else(|| panic!("no prompt for {}", k.as_str()))
            .to_string();
        for model in &models {
            for _ in 0..runs {
                jobs.push((
                    case.clone(),
                    model.clone(),
                    system.clone(),
                    input.clone(),
                    k,
                ));
            }
        }
    }
    let providers = &providers;
    let outcomes: Vec<Outcome> = futures::stream::iter(jobs)
        .map(|(case, model, system, input, k)| async move {
            let result = match providers.for_model(&model) {
                Ok((p, m)) => ostra_providers::structured(
                    p.as_ref(),
                    &m,
                    &system,
                    &input,
                    output_schema(k, None),
                    Effort::Low,
                )
                .await
                .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let (label, cost) = match result {
                Ok((out, usage)) => {
                    let name = model.split_once(':').map_or(model.as_str(), |(_, m)| m);
                    let cost = ostra_core::pricing::cost(name, &usage, 0);
                    let (l, ok) = score(&case, &out);
                    let reason = out["reason"].as_str().unwrap_or_default().to_string();
                    (Ok((l, ok, reason)), cost)
                }
                Err(e) => (Err(e), 0.0),
            };
            Outcome {
                case: case.id.clone(),
                model,
                label,
                cost,
            }
        })
        .buffer_unordered(6)
        .collect()
        .await;

    // case -> model -> outcomes
    let mut by: BTreeMap<(String, String), Vec<&Outcome>> = BTreeMap::new();
    for o in &outcomes {
        by.entry((o.case.clone(), o.model.clone()))
            .or_default()
            .push(o);
    }
    let mut report = vec![];
    let mut totals: BTreeMap<String, (usize, usize, usize, usize, f64)> = BTreeMap::new();
    println!("\n{:<32} {:<36} {:>6}  answers", "case", "model", "pass");
    for case in &cases {
        for model in &models {
            let Some(os) = by.get(&(case.id.clone(), model.clone())) else {
                continue;
            };
            let mut spread: BTreeMap<String, usize> = BTreeMap::new();
            let mut pass = 0;
            let mut errors = vec![];
            let mut reasons = vec![];
            for o in os {
                match &o.label {
                    Ok((l, ok, reason)) => {
                        *spread
                            .entry(format!("{}{l}", if *ok { "" } else { "✗ " }))
                            .or_default() += 1;
                        pass += usize::from(*ok);
                        reasons.push(reason.clone());
                    }
                    Err(e) => errors.push(e.clone()),
                }
            }
            let t = totals.entry(model.clone()).or_default();
            t.0 += os.len();
            t.1 += pass;
            t.2 += usize::from(spread.len() == 1 && errors.is_empty());
            t.3 += errors.len();
            t.4 += os.iter().map(|o| o.cost).sum::<f64>();
            let answers = spread
                .iter()
                .map(|(l, n)| format!("{l} ×{n}"))
                .chain(
                    errors
                        .iter()
                        .map(|e| format!("ERROR {}", e.chars().take(80).collect::<String>())),
                )
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "{:<32} {:<36} {:>2}/{:<3}  {answers}",
                case.id,
                model,
                pass,
                os.len()
            );
            report.push(json!({
                "case": case.id, "judge": case.judge, "model": model, "note": case.note,
                "pass": pass, "runs": os.len(), "answers": spread, "errors": errors, "reasons": reasons,
            }));
        }
    }
    println!(
        "\n{:<36} {:>9} {:>11} {:>7} {:>9}",
        "model", "accuracy", "consistent", "errors", "cost"
    );
    let groups = cases.len();
    for (model, (n, pass, consistent, errors, cost)) in &totals {
        println!(
            "{model:<36} {:>8.0}% {:>5}/{:<5} {errors:>7} {:>8.4}$",
            100.0 * *pass as f64 / (*n).max(1) as f64,
            consistent,
            groups,
            cost
        );
    }
    let out = repo().join("target/evals");
    std::fs::create_dir_all(&out).unwrap();
    let path = out.join(format!(
        "judges-{}.json",
        Utc::now().format("%Y%m%dT%H%M%S")
    ));
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("\nReport: {}", path.display());
    let failed: usize = totals.values().map(|t| t.3).sum();
    assert!(
        failed < outcomes.len() || outcomes.is_empty(),
        "every judge call failed; check the provider credentials"
    );
}

/// Offline: every case folds into the state its judge expects, so a broken case fails without a model.
#[test]
fn eval_cases_build_their_sessions() {
    let text = std::fs::read_to_string(repo().join("tests/evals/judges.toml")).unwrap();
    let file: File = toml::from_str(&text).unwrap();
    let dir = tempfile::tempdir().unwrap();
    for case in &file.case {
        let st = session(case, &dir.path().join(&case.id));
        let k = kind(&case.judge);
        let sufficiency = sufficiency_subject(case);
        let subject = match k {
            JudgeKind::Feedback => Some("0"),
            JudgeKind::Sufficiency => Some(sufficiency.as_str()),
            _ => None,
        };
        let (input, _) = judge_input(&st, k, subject, &[]);
        assert!(
            input.contains(&case.request),
            "{}: request missing",
            case.id
        );
        match case.judge.as_str() {
            "sufficiency" => {
                for n in case.expect.needed.iter().chain(&case.expect.not_needed) {
                    assert!(
                        input.contains(n.as_str()),
                        "{}: item {n} not in the input",
                        case.id
                    );
                }
            }
            "stakes" => {
                assert!(
                    st.spec.current.is_some() && !case.expect.stakes.is_empty(),
                    "{}",
                    case.id
                );
            }
            "track" => {
                assert!(case.expect.track.is_some(), "{}", case.id);
                assert_eq!(st.research_docs().len(), case.research.len(), "{}", case.id);
                assert!(
                    input.contains(case.research[0].doc.lines().nth(1).unwrap()),
                    "{}",
                    case.id
                );
            }
            "feedback" => {
                let round = &st.feedback.rounds[0];
                assert!(
                    round.route.is_none(),
                    "{}: routed without the judge",
                    case.id
                );
                assert_eq!(st.phases.len(), case.phase.len(), "{}", case.id);
                assert!(
                    st.phases.values().all(|p| p.implementer_report.is_some()),
                    "{}",
                    case.id
                );
                assert_eq!(
                    case.spec.is_some(),
                    st.spec.current.is_some(),
                    "{}",
                    case.id
                );
                assert!(
                    input.contains(case.feedback.as_deref().unwrap()),
                    "{}",
                    case.id
                );
            }
            _ => assert!(!case.expect.category.is_empty(), "{}", case.id),
        }
    }
}
