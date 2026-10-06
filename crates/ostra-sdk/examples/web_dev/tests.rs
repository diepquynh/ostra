use super::*;
use ostra_core::exec::ExecutionStatus;
use ostra_core::ids::{ExecutionId, SessionId};
use parking_lot::Mutex;
use std::collections::BTreeMap;

fn view(decisions: Vec<StageDecision>, runs: Vec<StageRunView>, answers: &[&str]) -> StageView {
    StageView {
        session: SessionId::from("s_1"),
        node: "e2e".into(),
        stage: "e2e".into(),
        scope: Some("project:web".into()),
        request: "Add a checkout page".into(),
        workspace_root: "/ws".into(),
        projects: vec![("web".into(), "/ws/web".into())],
        instructions: None,
        inputs: Default::default(),
        decisions,
        runs,
        answers: answers.iter().map(|a| a.to_string()).collect(),
        earlier_stages: vec![],
        spec_file: None,
        master_plan: None,
    }
}

fn scenario(name: &str, status: Status, cause: Option<Cause>) -> Scenario {
    Scenario {
        name: name.into(),
        file: "e2e/checkout.spec.ts:12".into(),
        status,
        error: (status == Status::Fail).then(|| "expected Total to be visible".into()),
        cause,
    }
}

fn report(scenarios: Vec<Scenario>) -> E2eReport {
    E2eReport {
        web_app: true,
        dir: "/ws/web".into(),
        command: "npx playwright test e2e/checkout.spec.ts".into(),
        scenarios,
        blocker: None,
        summary: "Tested checkout.".into(),
    }
}

/// A finished run whose result the plugin handled.
fn handled(agent: &str, r: E2eReport) -> StageRunView {
    StageRunView {
        execution: ExecutionId::from("e_1"),
        agent: agent.into(),
        status: ExecutionStatus::Ok,
        submit: serde_json::to_value(&r).ok(),
        error: None,
        handled: serde_json::to_value(verdict(r)).ok(),
    }
}

fn ran(agent: &str) -> StageDecision {
    run(agent, None)
}

fn agent_of(d: &StageDecision) -> &str {
    match d {
        StageDecision::Run { agent, .. } => agent,
        other => panic!("expected a run, got {other:?}"),
    }
}

#[test]
fn the_first_decision_runs_the_tester() {
    assert_eq!(agent_of(&decide(&view(vec![], vec![], &[]))), TESTER);
}

#[test]
fn passing_scenarios_pass_the_stage() {
    let r = report(vec![scenario("pays", Status::Pass, None)]);
    let d = decide(&view(vec![ran(TESTER)], vec![handled(TESTER, r)], &[]));
    assert!(
        matches!(d, StageDecision::Pass { summary } if summary.starts_with("1 end-to-end scenarios pass"))
    );
}

#[test]
fn a_project_without_a_web_interface_passes() {
    let r = E2eReport {
        web_app: false,
        summary: "A library with no pages.".into(),
        ..Default::default()
    };
    assert_eq!(verdict(r).verdict, StageVerdict::Pass);
}

#[test]
fn no_scenario_is_a_failure() {
    assert_eq!(verdict(report(vec![])).verdict, StageVerdict::Fail);
}

#[test]
fn app_failures_go_to_the_runner_with_a_plan_it_reads() {
    let r = report(vec![
        scenario("pays", Status::Fail, Some(Cause::App)),
        scenario("browses", Status::Pass, None),
    ]);
    let d = decide(&view(vec![ran(TESTER)], vec![handled(TESTER, r)], &[]));
    let StageDecision::Run {
        agent,
        instructions: Some(text),
    } = d
    else {
        panic!("expected a run with instructions, got {d:?}");
    };
    assert_eq!(agent, RUNNER);
    let plan = rerun_plan(&format!("Stage block\n\n{text}")).expect("the plan parses");
    assert_eq!(plan.project.as_deref(), Some("web"));
    assert_eq!(plan.command, "npx playwright test e2e/checkout.spec.ts");
    assert_eq!(plan.failures.len(), 1);
    assert_eq!(plan.failures[0].name, "pays");
}

#[test]
fn test_and_unknown_failures_go_back_to_the_tester() {
    for cause in [Some(Cause::Test), Some(Cause::Unknown), None] {
        let r = report(vec![
            scenario("pays", Status::Fail, Some(Cause::App)),
            scenario("refunds", Status::Fail, cause),
        ]);
        let d = decide(&view(vec![ran(TESTER)], vec![handled(TESTER, r)], &[]));
        let StageDecision::Run {
            agent,
            instructions: Some(text),
        } = d
        else {
            panic!("expected a run, got {d:?}");
        };
        assert_eq!(agent, TESTER);
        assert!(text.contains("refunds"), "{text}");
    }
}

#[test]
fn five_failing_runs_ask_the_user_and_an_answer_resets_the_count() {
    let failing = || {
        handled(
            TESTER,
            report(vec![scenario("pays", Status::Fail, Some(Cause::Test))]),
        )
    };
    let decisions: Vec<StageDecision> = (0..RUNS_BEFORE_ASK).map(|_| ran(TESTER)).collect();
    let runs: Vec<StageRunView> = (0..RUNS_BEFORE_ASK).map(|_| failing()).collect();
    let d = decide(&view(decisions.clone(), runs.clone(), &[]));
    assert!(
        matches!(&d, StageDecision::Ask { options, .. } if options == &["retry", "continue", "stop"]),
        "{d:?}"
    );

    let mut asked = decisions.clone();
    asked.push(d.clone());
    let retry = decide(&view(asked.clone(), runs.clone(), &["retry"]));
    assert_eq!(agent_of(&retry), TESTER);
    let go_on = decide(&view(asked.clone(), runs.clone(), &["continue"]));
    assert!(matches!(go_on, StageDecision::Fail { .. }), "{go_on:?}");

    // One failing run after the answer is below the threshold again.
    asked.push(retry);
    let mut more = runs;
    more.push(failing());
    assert_eq!(agent_of(&decide(&view(asked, more, &["retry"]))), TESTER);
}

#[test]
fn a_blocker_asks_the_user_with_its_reason() {
    let r = E2eReport {
        blocker: Some("npm install could not reach the registry.".into()),
        ..report(vec![])
    };
    let out = verdict(r.clone());
    assert_eq!(out.verdict, StageVerdict::NeedsUser);
    let d = decide(&view(vec![ran(TESTER)], vec![handled(TESTER, r)], &[]));
    assert!(
        matches!(&d, StageDecision::Ask { question, .. } if question.contains("could not reach the registry")),
        "{d:?}"
    );
}

#[test]
fn a_run_without_a_result_runs_again_then_asks() {
    let broken = |agent: &str| StageRunView {
        execution: ExecutionId::from("e_2"),
        agent: agent.into(),
        status: ExecutionStatus::Error,
        submit: None,
        error: Some("the model stopped".into()),
        handled: None,
    };
    let d = decide(&view(vec![ran(RUNNER)], vec![broken(RUNNER)], &[]));
    assert_eq!(agent_of(&d), RUNNER);
    let d = decide(&view(
        vec![ran(RUNNER), ran(RUNNER)],
        vec![broken(RUNNER), broken(RUNNER)],
        &[],
    ));
    assert!(matches!(d, StageDecision::Ask { .. }), "{d:?}");

    // One broken run after three failing rounds runs again instead of asking.
    let failing = handled(
        TESTER,
        report(vec![scenario("pays", Status::Fail, Some(Cause::Test))]),
    );
    let d = decide(&view(
        vec![ran(TESTER), ran(TESTER), ran(TESTER), ran(RUNNER)],
        vec![failing.clone(), failing.clone(), failing, broken(RUNNER)],
        &[],
    ));
    assert_eq!(agent_of(&d), RUNNER);
}

const AGENTS: &str = "Your subagent ID: e_9\n\nSubagents of this session, oldest first:\n\
- e_1: explore, research, project api, ok\n\
- e_2: implementer, phase 1, project api, ok (a message continues it)\n\
- e_3: implementer, phase 2, project web, ok (a message continues it)\n\
- e_4: implementer, phase 3, project api, ok (a message continues it)\n\
- e_9: e2e-runner, stage e2e, project web, running (you)\n";

#[test]
fn the_runner_finds_the_projects_implementer() {
    assert_eq!(implementer(AGENTS, Some("web")).as_deref(), Some("e_3"));
    assert_eq!(implementer(AGENTS, Some("docs")).as_deref(), Some("e_4"));
    assert_eq!(implementer(AGENTS, None).as_deref(), Some("e_4"));
    assert_eq!(implementer("Your subagent ID: e_9\n", None), None);
}

#[test]
fn shell_quoting_survives_a_quote() {
    assert_eq!(sh_quote("/a b/it's"), r"'/a b/it'\''s'");
}

#[test]
fn the_contract_schema_is_in_the_supported_subset() {
    ostra_core::schema_check::validate_schema(&contract_schema()).unwrap();
    let ok = serde_json::to_value(report(vec![scenario(
        "pays",
        Status::Fail,
        Some(Cause::App),
    )]))
    .unwrap();
    assert!(ostra_core::schema_check::check(&contract_schema(), &ok, "").is_empty());
    let manifest = WebDev.manifest();
    let mut issues = vec![];
    ostra_core::plugin::validate(
        &[ostra_core::plugin::PluginConfig {
            name: manifest.name.clone(),
            command: vec!["web_dev".into()],
            env: Default::default(),
            enabled: true,
            timeout_secs: 120,
        }],
        &mut issues,
    );
    assert!(issues.is_empty(), "{issues:?}");
    let mut set = ostra_standard::workflow_set();
    set.add_plugin(PLUGIN, &manifest);
    let flow = set.resolve("web-dev:web-app").unwrap();
    assert!(flow.stage("e2e").is_some());
}

/// Tool calls answered from a script, recorded in order.
struct Calls {
    seen: Mutex<Vec<(String, Value)>>,
    summary: String,
}

#[async_trait::async_trait]
impl AgentCalls for Calls {
    async fn tool(&self, name: &str, input: Value) -> ToolReply {
        self.seen.lock().push((name.into(), input.clone()));
        let output = match name {
            "ListAgents" => AGENTS.to_string(),
            "SendMessage" | "WaitForMessage" => "From e_3: fixed the total.".into(),
            "Bash" if input["command"].as_str().unwrap_or("").starts_with("node ") => {
                self.summary.clone()
            }
            _ => "ok".into(),
        };
        ToolReply {
            output,
            is_error: false,
        }
    }
    async fn complete(&self, _: CompleteRequest) -> Result<String, String> {
        Err("the runner makes no model call".into())
    }
    fn status(&self, _: &str) {}
}

fn task() -> AgentTask {
    let plan = Rerun {
        project: Some("web".into()),
        dir: "/ws/web".into(),
        command: "npx playwright test e2e/checkout.spec.ts".into(),
        failures: vec![scenario("pays", Status::Fail, Some(Cause::App))],
    };
    AgentTask {
        execution: ExecutionId::from("e_9"),
        session: Some(SessionId::from("s_1")),
        agent: RUNNER.into(),
        first_message: format!(
            "Stage e2e\n\n```json\n{}\n```\n",
            serde_json::to_string(&plan).unwrap()
        ),
        repo_root: "/ws/web".into(),
        session_dir: "/data/s_1/web".into(),
        workspace_root: "/ws".into(),
        model: "mock:m".into(),
        tools: vec![],
        submit_schema: contract_schema(),
        resumed: false,
    }
}

fn calls(summary: Value) -> Arc<Calls> {
    Arc::new(Calls {
        seen: Mutex::new(vec![]),
        summary: summary.to_string(),
    })
}

#[tokio::test]
async fn the_runner_asks_the_implementer_once_and_reports_the_rerun() {
    let c = calls(json!({
        "failed": [{"name": "refunds", "file": "e2e/checkout.spec.ts:30", "status": "fail", "error": "timeout"}],
        "failed_count": 1,
        "passed": [{"name": "pays", "file": "e2e/checkout.spec.ts:12", "status": "pass"}],
        "passed_count": 1,
        "skipped_count": 0,
        "errors": []
    }));
    let checkpoints = MemoryCheckpoints::default();
    let out = rerun(&task(), &AgentContext::new(c.clone()), &checkpoints)
        .await
        .unwrap();
    assert_eq!(out.scenarios.len(), 2);
    assert_eq!(out.scenarios[0].cause, Some(Cause::Unknown));
    assert!(out.summary.contains("1 pass, 1 fail"), "{}", out.summary);

    let seen = c.seen.lock().clone();
    let names: Vec<&str> = seen.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["ListAgents", "SendMessage", "Write", "Bash", "Bash"]
    );
    assert_eq!(seen[1].1["to"], "e_3");
    assert_eq!(seen[1].1["wait"], true);
    let command = seen[3].1["command"].as_str().unwrap();
    assert!(
        command.starts_with(
            "cd '/ws/web' && PLAYWRIGHT_JSON_OUTPUT_FILE='/data/s_1/web/web-dev-e2e-e_9.json'"
        ),
        "{command}"
    );
    assert!(command.ends_with("npx playwright test e2e/checkout.spec.ts --reporter=json"));
    assert_eq!(
        checkpoints.get("implementer:e_9").unwrap()["state"],
        "replied"
    );
}

#[tokio::test]
async fn a_resumed_runner_waits_instead_of_asking_again() {
    let c = calls(json!({
        "failed": [], "failed_count": 0,
        "passed": [{"name": "pays", "file": "e2e/checkout.spec.ts:12", "status": "pass"}],
        "passed_count": 1, "skipped_count": 0, "errors": []
    }));
    let checkpoints = MemoryCheckpoints::new(BTreeMap::from([(
        "implementer:e_9".to_string(),
        json!({"state": "asked", "to": "e_3"}),
    )]));
    let out = rerun(&task(), &AgentContext::new(c.clone()), &checkpoints)
        .await
        .unwrap();
    let names: Vec<String> = c.seen.lock().iter().map(|(n, _)| n.clone()).collect();
    assert_eq!(names, ["WaitForMessage", "Write", "Bash", "Bash"]);
    assert_eq!(verdict(out).verdict, StageVerdict::Pass);
}

#[tokio::test]
async fn a_run_without_a_report_is_a_blocker() {
    let c = Arc::new(Calls {
        seen: Mutex::new(vec![]),
        summary: "Error: ENOENT: no such file".into(),
    });
    let checkpoints = MemoryCheckpoints::new(BTreeMap::from([(
        "implementer:e_9".to_string(),
        json!({"state": "replied"}),
    )]));
    let out = rerun(&task(), &AgentContext::new(c), &checkpoints)
        .await
        .unwrap();
    assert!(out.blocker.is_some());
    assert_eq!(verdict(out).verdict, StageVerdict::NeedsUser);
}

/// The summarizer against a report in Playwright's JSON shape, when Node is installed.
#[test]
fn the_summarizer_reads_a_playwright_report() {
    if std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("node is not installed; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let report = json!({
        "suites": [{
            "title": "checkout.spec.ts",
            "file": "checkout.spec.ts",
            "specs": [],
            "suites": [{
                "title": "checkout",
                "file": "checkout.spec.ts",
                "specs": [
                    {"title": "pays", "ok": true, "file": "checkout.spec.ts", "line": 4,
                     "tests": [{"status": "expected", "results": [{"status": "passed"}]}]},
                    {"title": "refunds", "ok": false, "file": "checkout.spec.ts", "line": 9,
                     "tests": [{"status": "unexpected", "results": [{"status": "failed",
                        "error": {"message": "\u{1b}[31mexpect(locator).toBeVisible()\u{1b}[39m failed"}}]}]},
                    {"title": "later", "ok": true, "file": "checkout.spec.ts", "line": 14,
                     "tests": [{"status": "skipped", "results": []}]}
                ]
            }]
        }],
        "errors": []
    });
    let report_path = dir.path().join("report.json");
    let script = dir.path().join("summarize.cjs");
    std::fs::write(&report_path, report.to_string()).unwrap();
    std::fs::write(&script, SUMMARIZER).unwrap();
    let out = std::process::Command::new("node")
        .arg(&script)
        .arg(&report_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: Summary = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (
            summary.passed_count,
            summary.failed_count,
            summary.skipped_count
        ),
        (1, 1, 1)
    );
    assert_eq!(summary.failed[0].name, "checkout > refunds");
    assert_eq!(summary.failed[0].file, "checkout.spec.ts:9");
    assert_eq!(
        summary.failed[0].error.as_deref(),
        Some("expect(locator).toBeVisible() failed")
    );
}
