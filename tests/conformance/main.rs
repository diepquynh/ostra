//! Engine conformance fixtures (HANDOVER 17): one per rule ID of section 8.2. Each builds an
//! event history, folds it, and checks the planner's next steps.

use ostra_core::event::*;
use ostra_core::exec::{ExecutionResult, ExecutionStatus, Usage};
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId};
use ostra_core::pipeline::{QuestionAnswer, StageKind};
use ostra_core::{AgentName, ExecutorKind};
use ostra_engine::plan::{PlanCtx, SpawnRequest, Step, next_steps};
use ostra_engine::state::SessionState;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::path::PathBuf;

struct H {
    id: SessionId,
    events: Vec<StoredEvent>,
    ctx: PlanCtx,
}

fn root() -> PathBuf {
    PathBuf::from("/ws/.ostra/sessions/s1")
}

impl H {
    fn new(projects: &[&str], options: SessionOptions) -> H {
        let mut h = H {
            id: SessionId::from("s1"),
            events: vec![],
            ctx: PlanCtx::default(),
        };
        for p in projects {
            h.ctx
                .format_commands
                .insert(p.to_string(), Some(format!("fmt-{p}")));
        }
        h.ev(SessionEvent::SessionCreated {
            kind: SessionKind::Pipeline,
            request: "Add order cancellation".into(),
            options,
            projects: projects
                .iter()
                .map(|k| ProjectRef {
                    key: k.to_string(),
                    path: PathBuf::from(format!("/code/{k}")),
                })
                .collect(),
            workspace_root: PathBuf::from("/ws"),
            session_root: root(),
        });
        h
    }

    fn ev(&mut self, event: SessionEvent) {
        let seq = self.events.len() as i64 + 1;
        self.events.push(StoredEvent {
            seq,
            at: chrono::Utc::now(),
            event,
        });
    }

    fn state(&self) -> SessionState {
        SessionState::fold(self.id.clone(), &self.events)
    }

    fn steps(&self) -> Vec<Step> {
        next_steps(&self.state(), &self.ctx)
    }

    fn summaries(&self) -> Vec<String> {
        self.steps().iter().map(|s| s.summary()).collect()
    }

    fn decide(&mut self, judge: JudgeKind, subject: Option<&str>, output: Value) -> DecisionId {
        let id = DecisionId::new();
        self.ev(SessionEvent::DecisionMade {
            id: id.clone(),
            judge,
            subject: subject.map(String::from),
            input_summary: String::new(),
            reason: output
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or_default()
                .into(),
            output,
        });
        id
    }

    fn classify(&mut self, category: &str, projects: &[&str]) {
        let tasks: Vec<Value> = projects
            .iter()
            .map(|p| json!({"project": p, "task": format!("research {p}")}))
            .collect();
        self.decide(
            JudgeKind::Classify,
            None,
            json!({"category": category, "projects": projects, "explore_tasks": tasks, "opts_in": {"tests": false, "docs": false}, "reason": "r"}),
        );
    }

    fn spawn_step(&self, prefix: &str) -> SpawnRequest {
        self.steps()
            .into_iter()
            .find_map(|s| match s {
                Step::Spawn(r) if Step::Spawn(r.clone()).summary().starts_with(prefix) => Some(*r),
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!(
                    "no spawn step starting with `{prefix}` in {:?}",
                    self.summaries()
                )
            })
    }

    fn start(&mut self, prefix: &str) -> (ExecutionId, SpawnRequest) {
        let req = self.spawn_step(prefix);
        let id = ExecutionId::new();
        self.ev(SessionEvent::ExecutionStarted {
            id: id.clone(),
            agent: req.agent,
            purpose: req.purpose.clone(),
            stage: req.stage,
            project: req.project.clone(),
            executor: ExecutorKind::Native,
            model: "mock:m".into(),
            params: json!({"auto_fixable_ids": ["C1"]}),
            spawn_block: String::new(),
            report_path: req.inputs.report_file.clone(),
            resumes: None,
        });
        (id, req)
    }

    fn finish(&mut self, id: &ExecutionId, status: ExecutionStatus, submit: Option<Value>) {
        self.ev(SessionEvent::ExecutionFinished {
            id: id.clone(),
            result: ExecutionResult {
                status,
                submit,
                final_text: String::new(),
                usage: Usage::default(),
                native_session_id: None,
                error: (status != ExecutionStatus::Ok).then(|| "boom".into()),
            },
        });
    }

    /// Start and finish one spawn step with an Ok submit.
    fn run(&mut self, prefix: &str, submit: Value) -> SpawnRequest {
        let (id, req) = self.start(prefix);
        self.finish(&id, ExecutionStatus::Ok, Some(submit));
        req
    }

    fn open_gate(&mut self, kind: &str) -> GateId {
        let step = self
            .steps()
            .into_iter()
            .find(|s| s.summary() == format!("gate {kind}"))
            .unwrap_or_else(|| panic!("no gate {kind} in {:?}", self.summaries()));
        let Step::OpenGate {
            title,
            explanation,
            payload,
        } = step
        else {
            unreachable!()
        };
        let id = GateId::new();
        self.ev(SessionEvent::GateOpened {
            id: id.clone(),
            title,
            explanation,
            payload,
        });
        id
    }

    fn answer(&mut self, gate: &GateId, answer: GateAnswer) {
        self.ev(SessionEvent::GateAnswered {
            id: gate.clone(),
            source: AnswerSource::User,
            answer,
            reason: None,
        });
    }

    fn command(&mut self, purpose: CommandPurpose, project: &str) {
        self.ev(SessionEvent::CommandRan {
            purpose,
            project: project.into(),
            command: String::new(),
            exit_code: Some(0),
            output_tail: String::new(),
        });
    }

    // --- Canned flows ----------------------------------------------------------------------

    fn explored(projects: &[&str], options: SessionOptions) -> H {
        let mut h = H::new(projects, options);
        h.classify("IMPLEMENT", projects);
        for (i, _) in projects.iter().enumerate() {
            h.run(
                &format!("spawn explore explore#{i}"),
                explore_submit(i, &[]),
            );
        }
        h
    }

    fn spec_approved(projects: &[&str], options: SessionOptions) -> H {
        let mut h = H::explored(projects, options);
        h.run("spawn generate-spec", spec_submit(0, 1));
        h.run(
            "spawn fact-check fact-check-spec",
            fact("PASS", "spec", &[]),
        );
        let g = h.open_gate("spec_approval");
        h.answer(
            &g,
            GateAnswer::Approval {
                approved: true,
                feedback: None,
            },
        );
        h
    }

    fn plan_approved(projects: &[&str], phases: Value, options: SessionOptions) -> H {
        let mut h = H::spec_approved(projects, options);
        h.decide(
            JudgeKind::Stakes,
            None,
            json!({"stakes": "high", "reason": "r"}),
        );
        h.run("spawn plan", plan_submit(phases));
        h.run(
            "spawn fact-check fact-check-plan",
            fact("PASS", "plan", &[]),
        );
        let g = h.open_gate("plan_approval");
        h.answer(
            &g,
            GateAnswer::Approval {
                approved: true,
                feedback: None,
            },
        );
        h
    }

    /// Implement and pass review for one phase.
    fn pass_phase(&mut self, phase: u32) {
        self.run(
            &format!("spawn implementer phase {phase} initial"),
            impl_submit(phase, &["src/a.rs"]),
        );
        self.run(
            &format!("spawn code-reviewer review phase {phase} #1"),
            review(&[]),
        );
        let project = self.state().phases[&phase].info.project.clone();
        self.command(CommandPurpose::Stage, &project);
    }
}

fn explore_submit(i: usize, not_covered: &[&str]) -> Value {
    json!({
        "research_path": format!("/ws/.ostra/sessions/s1/p/ostra-research-{i}.md"),
        "scope_covered": "s", "findings_summary": "f", "sources_retrieved": 0, "open_questions": 0,
        "not_covered": not_covered
    })
}

fn spec_submit(questions: usize, evidence: u32) -> Value {
    let qs: Vec<Value> = (0..questions)
        .map(|i| json!({"id": format!("Q{}", i + 1), "question": "Which?", "tag": "Scope", "options": [{"label": "A", "description": "a"}, {"label": "B", "description": "b"}], "recommended": 0}))
        .collect();
    json!({"spec_path": "/ws/.ostra/sessions/s1/ostra-spec-1.md", "open_questions": qs, "external_evidence_rows": evidence, "deliverables": 1, "requirements": 3, "summary": "spec"})
}

fn fact(verdict: &str, target: &str, findings: &[&str]) -> Value {
    let f: Vec<Value> = findings
        .iter()
        .map(|x| json!({"severity": "HIGH", "location": "L", "claim": "C", "issue": x}))
        .collect();
    json!({"verdict": verdict, "target": target, "findings": f})
}

fn plan_submit(phases: Value) -> Value {
    json!({"spec_path": "/ws/.ostra/sessions/s1/ostra-spec-1.md", "master_plan_path": "/ws/.ostra/sessions/s1/ostra-plan-1.md", "phases": phases, "stakes": "High", "summary": "plan", "step_count": 4, "requirement_coverage": "3 of 3"})
}

fn phase(id: u32, project: &str, deps: &[u32], test_policy: &str) -> Value {
    json!({"id": id, "deliverable": "D1", "project": project, "title": format!("phase {id}"), "complexity": "Medium", "test_policy": test_policy, "depends_on": deps, "file": format!("/ws/.ostra/sessions/s1/ostra-plan-1-phase-{id}.md")})
}

fn impl_submit(phase: u32, files: &[&str]) -> Value {
    json!({"status": "ok", "report_path": format!("/ws/.ostra/sessions/s1/p/ostra-implementer-phase-{phase}.md"), "changed_files": files, "summary": "done"})
}

fn finding(sev: &str, rule: &str) -> Value {
    json!({"severity": sev, "file": "src/a.rs", "rule": rule, "description": format!("{rule} problem."), "fix": "Change `a` to `b` on line 1.", "guidance": if sev == "BLOCKER" { json!("research it") } else { Value::Null }})
}

fn review(findings: &[Value]) -> Value {
    let block = findings.iter().any(|f| f["severity"] == "BLOCKER");
    json!({"findings": findings, "security_block": block, "ledger_path": "/l", "summary": "r"})
}

fn report(path: &str) -> Value {
    json!({"status": "ok", "report_path": path, "changed_files": [], "summary": "done"})
}

fn one_phase() -> Value {
    json!([phase(1, "p", &[], "Required")])
}

// ------------------------------------------------------------------------------------------
// D1, Hard 15: IMPLEMENT always passes through Spec; no research document means no Spec.
// ------------------------------------------------------------------------------------------

#[test]
fn d1_explore_leads_to_spec_never_plan() {
    let h = H::explored(&["p"], SessionOptions::default());
    let s = h.summaries();
    assert_eq!(s, vec!["spawn generate-spec spec#1"]);
}

#[test]
fn d1_no_research_document_means_no_spec() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("IMPLEMENT", &["p"]);
    let (id, _) = h.start("spawn explore");
    h.finish(&id, ExecutionStatus::Denied, None);
    let g = h.open_gate("execution_failed");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "abandon".into(),
            text: None,
        },
    );
    assert_eq!(h.summaries(), vec!["fail"]);
}

#[test]
fn d1_implement_with_no_tasks_still_explores() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(JudgeKind::Classify, None, json!({"category": "IMPLEMENT", "projects": ["p"], "explore_tasks": [], "opts_in": {"tests": false, "docs": false}, "reason": "r"}));
    assert_eq!(h.summaries(), vec!["spawn explore explore#0"]);
}

// ------------------------------------------------------------------------------------------
// D2: Spec waits for every explore and for the Sufficiency judge; it gets every document.
// ------------------------------------------------------------------------------------------

#[test]
fn d2_spec_waits_for_running_explore() {
    let mut h = H::new(&["a", "b"], SessionOptions::default());
    h.classify("IMPLEMENT", &["a", "b"]);
    // Rule M1: both explores fan out at once.
    assert_eq!(
        h.summaries(),
        vec!["spawn explore explore#0", "spawn explore explore#1"]
    );
    h.run("spawn explore explore#0", explore_submit(0, &[]));
    h.start("spawn explore explore#1");
    assert_eq!(h.summaries(), Vec::<String>::new());
}

#[test]
fn d2_not_covered_goes_to_sufficiency_and_spec_gets_every_doc() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("IMPLEMENT", &["p"]);
    h.run(
        "spawn explore explore#0",
        explore_submit(0, &["the web client"]),
    );
    assert_eq!(h.summaries(), vec!["judge sufficiency 0"]);
    h.decide(JudgeKind::Sufficiency, Some("0"), json!({"items": [{"item": "the web client", "needed": true, "reason": "r", "task": {"project": "p", "task": "research the web client"}}], "reason": "r"}));
    assert_eq!(h.summaries(), vec!["spawn explore explore#1"]);
    h.run("spawn explore explore#1", explore_submit(1, &[]));
    let spec = h.spawn_step("spawn generate-spec");
    assert_eq!(spec.inputs.research_docs.len(), 2);
    assert!(spec.inputs.research_docs[0].ends_with("ostra-research-0.md"));
}

// ------------------------------------------------------------------------------------------
// D3: open questions before any fact-check; every answer re-runs generate-spec.
// ------------------------------------------------------------------------------------------

#[test]
fn d3_questions_before_fact_check_and_answers_rerun_spec() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(2, 0));
    assert_eq!(h.summaries(), vec!["gate open_questions"]);
    let g = h.open_gate("open_questions");
    assert_eq!(h.summaries(), Vec::<String>::new());
    let answers = vec![
        QuestionAnswer {
            id: "Q1".into(),
            question: "Which?".into(),
            answer: "A".into(),
        },
        QuestionAnswer {
            id: "Q2".into(),
            question: "Which?".into(),
            answer: "B".into(),
        },
    ];
    h.answer(
        &g,
        GateAnswer::Questions {
            answers: answers.clone(),
        },
    );
    let rerun = h.spawn_step("spawn generate-spec");
    assert_eq!(rerun.inputs.answers, answers);
    assert!(
        rerun.inputs.spec_file.is_some(),
        "the rerun rewrites the spec in place"
    );
}

// ------------------------------------------------------------------------------------------
// D3a: Prior findings are `none` once, then the previous pass's findings verbatim.
// ------------------------------------------------------------------------------------------

#[test]
fn d3a_prior_findings() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(0, 0));
    let first = h.spawn_step("spawn fact-check");
    assert_eq!(first.inputs.prior_findings.as_deref(), Some("none"));
    h.run(
        "spawn fact-check",
        fact("FAIL", "spec", &["step 2.3 calls a missing method"]),
    );
    let rerun = h.spawn_step("spawn generate-spec");
    assert!(
        rerun
            .inputs
            .findings
            .as_deref()
            .unwrap()
            .contains("step 2.3 calls a missing method")
    );
    h.run("spawn generate-spec", spec_submit(0, 0));
    let second = h.spawn_step("spawn fact-check");
    assert_eq!(
        second.inputs.prior_findings.as_deref(),
        Some("HIGH, L: C step 2.3 calls a missing method")
    );
    h.run("spawn fact-check", fact("PASS", "spec", &[]));
    let g = h.open_gate("spec_approval");
    h.answer(
        &g,
        GateAnswer::Approval {
            approved: false,
            feedback: Some("Drop the retry logic".into()),
        },
    );
    h.run("spawn generate-spec", spec_submit(0, 0));
    let third = h.spawn_step("spawn fact-check");
    assert_eq!(
        third.inputs.prior_findings.as_deref(),
        Some("no findings on the previous pass"),
        "a clean pass still makes a re-pass"
    );
}

// ------------------------------------------------------------------------------------------
// D3b: refetch only on a spec's first pass with External Evidence rows.
// ------------------------------------------------------------------------------------------

#[test]
fn d3b_source_check() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(0, 2));
    assert_eq!(
        h.spawn_step("spawn fact-check")
            .inputs
            .source_check
            .as_deref(),
        Some("refetch")
    );
    h.run("spawn fact-check", fact("FAIL", "spec", &["x"]));
    h.run("spawn generate-spec", spec_submit(0, 2));
    assert_eq!(
        h.spawn_step("spawn fact-check")
            .inputs
            .source_check
            .as_deref(),
        Some("citations")
    );

    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(0, 0));
    assert_eq!(
        h.spawn_step("spawn fact-check")
            .inputs
            .source_check
            .as_deref(),
        Some("citations")
    );
}

// ------------------------------------------------------------------------------------------
// D4, Hard 16: the plan reads the spec and nothing else.
// ------------------------------------------------------------------------------------------

#[test]
fn d4_plan_gets_only_the_spec() {
    let mut h = H::spec_approved(&["p"], SessionOptions::default());
    assert_eq!(h.summaries(), vec!["judge stakes"]);
    h.decide(
        JudgeKind::Stakes,
        None,
        json!({"stakes": "medium", "reason": "r"}),
    );
    let plan = h.spawn_step("spawn plan");
    assert!(plan.inputs.research_docs.is_empty());
    assert!(plan.inputs.answers.is_empty());
    assert!(plan.inputs.task.is_none());
    assert_eq!(
        plan.inputs.spec_file,
        Some(PathBuf::from("/ws/.ostra/sessions/s1/ostra-spec-1.md"))
    );
    assert_eq!(plan.session_dir, root());
}

#[test]
fn stakes_low_skips_plan() {
    let mut h = H::spec_approved(&["p"], SessionOptions::default());
    h.decide(
        JudgeKind::Stakes,
        None,
        json!({"stakes": "low", "reason": "small"}),
    );
    let s = h.summaries();
    assert_eq!(s, vec!["spawn implementer phase 1 initial"]);
    let req = h.spawn_step("spawn implementer");
    assert!(
        req.inputs.phase.as_ref().unwrap().file.is_none(),
        "Hard rule 13: no plan means No plan:"
    );
}

// ------------------------------------------------------------------------------------------
// D5: plan fact-check uses citations and the approved spec; approval needs PASS.
// ------------------------------------------------------------------------------------------

#[test]
fn d5_plan_fact_check_and_approval() {
    let mut h = H::spec_approved(&["p"], SessionOptions::default());
    h.decide(
        JudgeKind::Stakes,
        None,
        json!({"stakes": "high", "reason": "r"}),
    );
    h.run("spawn plan", plan_submit(one_phase()));
    let fc = h.spawn_step("spawn fact-check fact-check-plan");
    assert_eq!(fc.inputs.source_check.as_deref(), Some("citations"));
    assert_eq!(
        fc.inputs.spec_file,
        Some(PathBuf::from("/ws/.ostra/sessions/s1/ostra-spec-1.md"))
    );
    assert!(fc.inputs.research_docs.is_empty());
    h.run(
        "spawn fact-check fact-check-plan",
        fact("FAIL", "plan", &["phase 5 missing"]),
    );
    let rerun = h.spawn_step("spawn plan");
    assert!(
        rerun
            .inputs
            .findings
            .as_deref()
            .unwrap()
            .contains("phase 5 missing")
    );
    assert!(
        !h.summaries().iter().any(|s| s == "gate plan_approval"),
        "no approval without PASS"
    );
}

#[test]
fn approval_without_pass_is_ignored_by_the_fold() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.run("spawn fact-check", fact("PASS", "spec", &[]));
    let g = h.open_gate("spec_approval");
    // A new spec version lands before the answer: its PASS no longer covers it.
    h.ev(SessionEvent::RequestAmended {
        text: "also refunds".into(),
    });
    h.answer(
        &g,
        GateAnswer::Approval {
            approved: true,
            feedback: None,
        },
    );
    assert!(!h.state().spec.approved);
}

// ------------------------------------------------------------------------------------------
// D6, D7, M2 to M6: the scheduler.
// ------------------------------------------------------------------------------------------

#[test]
fn d6_m2_m3_m5_scheduling() {
    let phases = json!([
        phase(1, "api", &[], "Required"),
        phase(2, "web", &[1], "Required"),
        phase(3, "api", &[], "Required"),
        phase(4, "web", &[99], "Required"),
    ]);
    let mut h = H::plan_approved(&["api", "web"], phases, SessionOptions::default());
    // M2: one pipeline per project; M3: phase 2 waits for phase 1; M5: phase 4's unreadable
    // dependency means it depends on every earlier phase.
    assert_eq!(h.summaries(), vec!["spawn implementer phase 1 initial"]);
    h.pass_phase(1);
    // Ready phases in different projects run in parallel.
    assert_eq!(
        h.summaries(),
        vec![
            "spawn implementer phase 2 initial",
            "spawn implementer phase 3 initial"
        ]
    );
    h.pass_phase(2);
    h.pass_phase(3);
    // Rule D8: api's last phase passed, so api formats while web keeps building.
    assert_eq!(
        h.summaries(),
        vec!["spawn implementer phase 4 initial", "command format api"]
    );
}

// ------------------------------------------------------------------------------------------
// D8, T1 to T7: format once, closing gate once per project, tests and docs only after.
// ------------------------------------------------------------------------------------------

#[test]
fn d8_t1_no_tests_between_phases_and_format_once() {
    let phases = json!([phase(1, "p", &[], "Required"), phase(2, "p", &[1], "Skip")]);
    let mut h = H::plan_approved(
        &["p"],
        phases,
        SessionOptions {
            tests: true,
            ..Default::default()
        },
    );
    h.pass_phase(1);
    assert_eq!(
        h.summaries(),
        vec!["spawn implementer phase 2 initial"],
        "T1: no EPA between phases"
    );
    h.pass_phase(2);
    assert_eq!(h.summaries(), vec!["command format p"]);
    h.command(CommandPurpose::Format, "p");
    // T3: tests were requested, so the gate asks only about docs.
    let g = h.open_gate("closing_gate");
    let st = h.state();
    let GatePayload::ClosingGate { items } = &st.gates[&g].payload else {
        panic!()
    };
    assert!(!items[0].ask_tests && items[0].ask_docs);
    h.answer(
        &g,
        GateAnswer::Closing {
            items: vec![ClosingChoice {
                project: "p".into(),
                tests: false,
                docs: false,
            }],
        },
    );
    // T4: only the Required phase is covered.
    assert_eq!(
        h.summaries(),
        vec!["spawn execution-path-analyzer epa phase 1"]
    );
}

#[test]
fn t4_epa_fans_out_and_write_test_is_serial() {
    let phases = json!([
        phase(1, "p", &[], "Required"),
        phase(2, "p", &[1], "Required")
    ]);
    let mut h = H::plan_approved(
        &["p"],
        phases,
        SessionOptions {
            tests: true,
            docs: true,
            yolo: false,
        },
    );
    h.pass_phase(1);
    h.pass_phase(2);
    h.command(CommandPurpose::Format, "p");
    // T3: both requested, so there is no gate at all.
    assert_eq!(
        h.summaries(),
        vec![
            "spawn execution-path-analyzer epa phase 1",
            "spawn execution-path-analyzer epa phase 2"
        ]
    );
    let (e1, _) = h.start("spawn execution-path-analyzer epa phase 1");
    let (e2, _) = h.start("spawn execution-path-analyzer epa phase 2");
    h.finish(&e1, ExecutionStatus::Ok, Some(report("/e1")));
    assert_eq!(
        h.summaries(),
        vec![] as Vec<String>,
        "write-test waits for every EPA"
    );
    h.finish(&e2, ExecutionStatus::Ok, Some(report("/e2")));
    assert_eq!(
        h.summaries(),
        vec!["spawn write-test write-test phase 1 initial"]
    );
    h.run("spawn write-test write-test phase 1", report("/t1"));
    assert_eq!(
        h.summaries(),
        vec!["spawn code-reviewer review phase 1 tests #1"]
    );
    let r = h.spawn_step("spawn code-reviewer");
    assert_eq!(r.inputs.phase_value.as_deref(), Some("1-tests"));
    assert_eq!(r.inputs.epa_report, Some(PathBuf::from("/e1")));
    h.run("spawn code-reviewer", review(&[]));
    // Test files are staged per phase after that phase's test review passes.
    assert_eq!(h.summaries(), vec!["command stage p"]);
    h.command(CommandPurpose::Stage, "p");
    assert_eq!(
        h.summaries(),
        vec!["spawn write-test write-test phase 2 initial"]
    );
}

#[test]
fn t6_closing_gate_is_batched() {
    let phases = json!([
        phase(1, "a", &[], "Required"),
        phase(2, "b", &[], "Required")
    ]);
    let mut h = H::plan_approved(&["a", "b"], phases, SessionOptions::default());
    h.pass_phase(1);
    h.pass_phase(2);
    h.command(CommandPurpose::Format, "a");
    h.command(CommandPurpose::Format, "b");
    let steps = h.steps();
    let gates: Vec<&Step> = steps
        .iter()
        .filter(|s| s.summary() == "gate closing_gate")
        .collect();
    assert_eq!(gates.len(), 1);
    let Step::OpenGate {
        payload: GatePayload::ClosingGate { items },
        ..
    } = gates[0]
    else {
        panic!()
    };
    assert_eq!(items.len(), 2);
}

#[test]
fn t2_yolo_answers_the_closing_gate() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    h.pass_phase(1);
    h.command(CommandPurpose::Format, "p");
    h.open_gate("closing_gate");
    h.ev(SessionEvent::YoloSet { enabled: true });
    assert!(h.summaries().contains(&"yolo-answer".to_string()));
}

// ------------------------------------------------------------------------------------------
// D9: a failed phase removes its dependents; independent phases continue.
// ------------------------------------------------------------------------------------------

#[test]
fn d9_blocked_phase_removes_dependents() {
    let phases = json!([
        phase(1, "a", &[], "Required"),
        phase(2, "a", &[1], "Required"),
        phase(3, "b", &[], "Required")
    ]);
    let mut h = H::plan_approved(&["a", "b"], phases, SessionOptions::default());
    let (id, _) = h.start("spawn implementer phase 1");
    h.finish(&id, ExecutionStatus::Denied, None);
    let g = h.open_gate("execution_failed");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "abandon".into(),
            text: None,
        },
    );
    let s = h.summaries();
    assert!(s.contains(&"blocked phase 1".to_string()));
    assert!(s.contains(&"spawn implementer phase 3 initial".to_string()));
    assert!(!s.iter().any(|x| x.contains("phase 2")));
    assert!(ostra_engine::plan::removed_phases(&h.state()).contains(&2));
}

// ------------------------------------------------------------------------------------------
// D10: a requirement change after the plan restarts at the spec.
// ------------------------------------------------------------------------------------------

#[test]
fn d10_change_at_plan_approval_goes_to_spec() {
    let mut h = H::spec_approved(&["p"], SessionOptions::default());
    h.decide(
        JudgeKind::Stakes,
        None,
        json!({"stakes": "high", "reason": "r"}),
    );
    h.run("spawn plan", plan_submit(one_phase()));
    h.run(
        "spawn fact-check fact-check-plan",
        fact("PASS", "plan", &[]),
    );
    let g = h.open_gate("plan_approval");
    h.answer(
        &g,
        GateAnswer::Approval {
            approved: false,
            feedback: Some("Drop the retry logic".into()),
        },
    );
    let spec = h.spawn_step("spawn generate-spec");
    assert_eq!(
        spec.inputs.changes,
        vec!["Drop the retry logic".to_string()]
    );
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.run(
        "spawn fact-check fact-check-spec",
        fact("PASS", "spec", &[]),
    );
    let g = h.open_gate("spec_approval");
    h.answer(
        &g,
        GateAnswer::Approval {
            approved: true,
            feedback: None,
        },
    );
    let plan = h.spawn_step("spawn plan");
    assert!(
        plan.inputs.findings.is_none(),
        "a spec-change revision, not a FAIL re-run"
    );
    assert!(
        plan.inputs.target.is_some(),
        "the earlier plan is revised in place"
    );
    h.run("spawn plan", plan_submit(one_phase()));
    let check = h.spawn_step("spawn fact-check fact-check-plan");
    assert_eq!(
        check.inputs.prior_findings.as_deref(),
        Some("no findings on the previous pass")
    );
}

// D10: a spec revision gets only the input its spec does not reflect yet.
#[test]
fn d10_spec_revision_gets_only_new_input() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let first = h.spawn_step("spawn generate-spec");
    assert!(
        first.inputs.new_research_docs.is_empty(),
        "a first run reads every research document"
    );
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.run("spawn fact-check", fact("PASS", "spec", &[]));
    let g = h.open_gate("spec_approval");
    h.answer(
        &g,
        GateAnswer::Approval {
            approved: false,
            feedback: Some("Drop the retry logic".into()),
        },
    );
    let rev = h.spawn_step("spawn generate-spec");
    assert_eq!(rev.inputs.changes, vec!["Drop the retry logic".to_string()]);
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.run("spawn fact-check", fact("FAIL", "spec", &["x"]));
    let fix = h.spawn_step("spawn generate-spec");
    assert!(
        fix.inputs.changes.is_empty(),
        "an applied change is not sent again"
    );
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.ev(SessionEvent::RequestAmended {
        text: "also handle refunds".into(),
    });
    h.run("spawn explore explore#1", explore_submit(1, &[]));
    let amended = h.spawn_step("spawn generate-spec");
    assert_eq!(amended.inputs.research_docs.len(), 2);
    assert_eq!(amended.inputs.new_research_docs.len(), 1);
    assert_eq!(
        amended.inputs.changes,
        vec!["The user extended the request: also handle refunds".to_string()]
    );
}

#[test]
fn amendment_explores_the_new_part_first() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.ev(SessionEvent::RequestAmended {
        text: "also handle refunds".into(),
    });
    assert_eq!(h.summaries(), vec!["spawn explore explore#1"]);
    h.run("spawn explore explore#1", explore_submit(1, &[]));
    let spec = h.spawn_step("spawn generate-spec");
    assert!(spec.inputs.task.unwrap().contains("also handle refunds"));
    assert_eq!(spec.inputs.research_docs.len(), 2);
}

// ------------------------------------------------------------------------------------------
// Hard 4 and Hard 13, staging, the review loop.
// ------------------------------------------------------------------------------------------

#[test]
fn hard4_missing_submit_is_a_failure() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (id, _) = h.start("spawn implementer");
    h.finish(&id, ExecutionStatus::Ok, None);
    assert_eq!(h.summaries(), vec!["gate execution_failed"]);
}

#[test]
fn hard13_phase_spawns_carry_the_phase_file() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let imp = h.spawn_step("spawn implementer");
    assert!(imp.inputs.phase.as_ref().unwrap().file.is_some());
    h.run("spawn implementer", impl_submit(1, &["src/a.rs"]));
    let rev = h.spawn_step("spawn code-reviewer");
    assert!(rev.inputs.phase.as_ref().unwrap().file.is_some());
    assert_eq!(rev.inputs.changed_files, vec!["src/a.rs".to_string()]);
    assert_eq!(rev.inputs.phase_value.as_deref(), Some("1"));
}

#[test]
fn staging_after_review_passes() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    h.run(
        "spawn implementer",
        impl_submit(1, &["src/a.rs", "src/b.rs"]),
    );
    h.run("spawn code-reviewer", review(&[]));
    let steps = h.steps();
    let Some(Step::Command {
        purpose: CommandPurpose::Stage,
        files,
        ..
    }) = steps.first()
    else {
        panic!("{:?}", h.summaries())
    };
    assert_eq!(files, &vec!["src/a.rs".to_string(), "src/b.rs".to_string()]);
}

#[test]
fn review_loop_splits_autofix_fix_and_caps_at_three() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    h.run("spawn implementer", impl_submit(1, &["src/a.rs"]));
    // C1 is auto-fixable in this project; PHASE-REQ never is, whatever its Fix text.
    h.run(
        "spawn code-reviewer",
        review(&[
            finding("LOW", "C1"),
            finding("HIGH", "PHASE-REQ-1"),
            finding("LOW", "C9"),
        ]),
    );
    assert_eq!(h.summaries(), vec!["autofix phase 1"]);
    h.ev(SessionEvent::AutofixApplied {
        project: "p".into(),
        phase: 1,
        tests: false,
        applied: vec!["x".into()],
        failed: vec![],
    });
    let fix = h.spawn_step("spawn implementer phase 1 fix");
    let text = fix.inputs.instructions.unwrap();
    assert!(
        text.contains("PHASE-REQ-1") && !text.contains("C9"),
        "only HIGH and MEDIUM go to the fix agent"
    );
    assert!(
        fix.inputs
            .ledger_file
            .unwrap()
            .ends_with("ostra-review-ledger-phase-1.md")
    );
    h.run(
        "spawn implementer phase 1 fix",
        impl_submit(1, &["src/a.rs"]),
    );
    h.run(
        "spawn code-reviewer review phase 1 #2",
        review(&[finding("HIGH", "R2")]),
    );
    h.run(
        "spawn implementer phase 1 fix",
        impl_submit(1, &["src/a.rs"]),
    );
    h.run(
        "spawn code-reviewer review phase 1 #3",
        review(&[finding("MEDIUM", "R3")]),
    );
    assert_eq!(
        h.summaries(),
        vec!["gate review_cap"],
        "the 4th pass is a gate"
    );
    let g = h.open_gate("review_cap");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "another-pass".into(),
            text: None,
        },
    );
    assert_eq!(h.summaries(), vec!["spawn implementer phase 1 fix"]);
}

#[test]
fn only_low_findings_exit_the_loop() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    h.run("spawn implementer", impl_submit(1, &["src/a.rs"]));
    h.run("spawn code-reviewer", review(&[finding("LOW", "C9")]));
    assert_eq!(h.summaries(), vec!["command stage p"]);
}

// ------------------------------------------------------------------------------------------
// Hard 21: BLOCKER findings go alone to the fix agent, with no cap, and block docs.
// ------------------------------------------------------------------------------------------

#[test]
fn hard21_blocker_has_no_cap() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    h.run("spawn implementer", impl_submit(1, &["src/a.rs"]));
    for i in 1..=5 {
        h.run(
            &format!("spawn code-reviewer review phase 1 #{i}"),
            review(&[finding("BLOCKER", "SEC-BLOCK-EXFIL"), finding("HIGH", "R1")]),
        );
        let fix = h.spawn_step("spawn implementer phase 1 blockerfix");
        let text = fix.inputs.instructions.unwrap();
        assert!(
            text.contains("SEC-BLOCK-EXFIL") && !text.contains("(R1)"),
            "only BLOCKER findings, with a removal instruction"
        );
        assert!(text.contains("Remove"));
        h.run(
            "spawn implementer phase 1 blockerfix",
            impl_submit(1, &["src/a.rs"]),
        );
    }
    assert!(!h.summaries().contains(&"gate review_cap".to_string()));
}

#[test]
fn hard21_open_blocker_blocks_docs() {
    let mut h = H::plan_approved(
        &["p"],
        one_phase(),
        SessionOptions {
            docs: true,
            tests: false,
            yolo: false,
        },
    );
    h.run("spawn implementer", impl_submit(1, &["src/a.rs"]));
    h.run(
        "spawn code-reviewer",
        review(&[finding("BLOCKER", "SEC-BLOCK-EXFIL")]),
    );
    let (id, _) = h.start("spawn implementer phase 1 blockerfix");
    h.finish(&id, ExecutionStatus::Denied, None);
    let g = h.open_gate("execution_failed");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "abandon".into(),
            text: None,
        },
    );
    assert!(
        !h.summaries()
            .iter()
            .any(|s| s.contains("module-documentation"))
    );
}

// ------------------------------------------------------------------------------------------
// HANDOFF and STUCK.
// ------------------------------------------------------------------------------------------

#[test]
fn handoff_runs_prompt_generation_then_resumes() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    h.run(
        "spawn implementer",
        json!({"status": "handoff", "report_path": "/r", "changed_files": [], "summary": "s",
               "handoff": {"specialist": "prompt-generation", "request": "author the agent prompt", "target_files": ["agents/x.md"], "resume_instructions": "wire it into step 3"}}),
    );
    let pg = h.spawn_step("spawn prompt-generation handoff");
    assert_eq!(pg.inputs.task.as_deref(), Some("author the agent prompt"));
    h.run("spawn prompt-generation handoff", report("/pg"));
    let resume = h.spawn_step("spawn implementer phase 1 resume");
    assert!(
        resume
            .inputs
            .instructions
            .unwrap()
            .contains("wire it into step 3")
    );
}

#[test]
fn stuck_goes_to_rescue_never_a_plain_retry() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (id, _) = h.start("spawn implementer");
    h.finish(
        &id,
        ExecutionStatus::Ok,
        Some(json!({"status": "stuck", "report_path": "/r", "changed_files": [], "summary": "s",
                     "stuck": {"diagnostic": "error[E0433]: failed to resolve: use of undeclared crate", "need": "the crate name"}})),
    );
    assert_eq!(h.summaries(), vec![format!("judge rescue {id}")]);
    h.decide(
        JudgeKind::Rescue,
        Some(id.as_str()),
        json!({"action": "rerun", "fact": "The crate is `ostra_core`.", "reason": "r"}),
    );
    let rerun = h.spawn_step("spawn implementer phase 1 rescue");
    let ctx = rerun.inputs.instructions.unwrap();
    assert!(ctx.contains("error[E0433]") && ctx.contains("ostra_core"));
}

#[test]
fn stuck_rescue_by_explore() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (id, _) = h.start("spawn implementer");
    h.finish(&id, ExecutionStatus::Ok, Some(json!({"status": "stuck", "report_path": "/r", "summary": "s", "stuck": {"diagnostic": "d", "need": "n"}})));
    h.decide(JudgeKind::Rescue, Some(id.as_str()), json!({"action": "explore", "explore_task": {"project": "p", "task": "find the working example"}, "reason": "r"}));
    let ex = h.spawn_step("spawn explore");
    assert_eq!(ex.inputs.task.as_deref(), Some("find the working example"));
    h.run("spawn explore", explore_submit(9, &[]));
    let rerun = h.spawn_step("spawn implementer phase 1 rescue");
    assert!(
        rerun
            .inputs
            .instructions
            .unwrap()
            .contains("A targeted explore found")
    );
}

// ------------------------------------------------------------------------------------------
// YOLO: gates are answered by the engine; the review budget rises to 10.
// ------------------------------------------------------------------------------------------

#[test]
fn yolo_answers_gates_and_extends_review_budget() {
    let mut h = H::plan_approved(
        &["p"],
        one_phase(),
        SessionOptions {
            yolo: true,
            ..Default::default()
        },
    );
    h.run("spawn implementer", impl_submit(1, &["src/a.rs"]));
    for i in 1..=9 {
        h.run(
            &format!("spawn code-reviewer review phase 1 #{i}"),
            review(&[finding("HIGH", "R1")]),
        );
        h.run(
            "spawn implementer phase 1 fix",
            impl_submit(1, &["src/a.rs"]),
        );
    }
    h.run(
        "spawn code-reviewer review phase 1 #10",
        review(&[finding("HIGH", "R1"), finding("HIGH", "R2")]),
    );
    assert_eq!(h.summaries(), vec!["judge resolve-review phase:1"]);
    h.decide(JudgeKind::ResolveReview, Some("phase:1"), json!({"action": "fix", "instructions": [{"finding": "R1", "instruction": "do x"}], "reason": "r"}));
    h.run(
        "spawn implementer phase 1 fix",
        impl_submit(1, &["src/a.rs"]),
    );
    // One verification pass; it converged from 2 to 1 open, so another resolution round.
    h.run(
        "spawn code-reviewer review phase 1 #11",
        review(&[finding("HIGH", "R2")]),
    );
    assert_eq!(h.summaries(), vec!["judge resolve-review phase:1"]);
    h.decide(
        JudgeKind::ResolveReview,
        Some("phase:1"),
        json!({"action": "fix", "instructions": [], "reason": "r"}),
    );
    h.run(
        "spawn implementer phase 1 fix",
        impl_submit(1, &["src/a.rs"]),
    );
    // Not converging (1 then 1): the phase is blocked and recorded, with no gate.
    h.run(
        "spawn code-reviewer review phase 1 #12",
        review(&[finding("HIGH", "R2")]),
    );
    assert_eq!(h.summaries(), vec!["blocked phase 1"]);
}

#[test]
fn yolo_gate_gets_a_yolo_answer_step() {
    let mut h = H::explored(
        &["p"],
        SessionOptions {
            yolo: true,
            ..Default::default()
        },
    );
    h.run("spawn generate-spec", spec_submit(1, 0));
    h.open_gate("open_questions");
    assert_eq!(h.summaries(), vec!["yolo-answer"]);
}

// ------------------------------------------------------------------------------------------
// Resume after restart, and the other categories.
// ------------------------------------------------------------------------------------------

#[test]
fn interrupted_execution_reruns() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (id, _) = h.start("spawn implementer");
    h.finish(&id, ExecutionStatus::Interrupted, None);
    assert_eq!(h.summaries(), vec!["spawn implementer phase 1 rerun"]);
}

#[test]
fn research_category_ends_after_explore() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("RESEARCH", &["p"]);
    h.run("spawn explore", explore_submit(0, &[]));
    assert_eq!(h.summaries(), vec!["judge completion"]);
}

#[test]
fn quick_answer_category() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(JudgeKind::Classify, None, json!({"category": "QUICK_ANSWER", "projects": ["p"], "explore_tasks": [], "opts_in": {"tests": false, "docs": false}, "reason": "r"}));
    h.run("spawn quick-answer", json!({"answer": "42", "sources": []}));
    assert_eq!(h.summaries(), vec!["complete"]);
}

// Quick change: one implementer pass on the native executor, staged, then completion.
#[test]
fn quick_change_runs_one_native_pass_without_review() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(JudgeKind::Classify, None, json!({"category": "QUICK_CHANGE", "projects": ["p"], "explore_tasks": [{"project": "p", "task": "ignored"}], "opts_in": {"tests": false, "docs": false}, "reason": "r"}));
    assert_eq!(h.summaries(), vec!["spawn implementer phase 1 initial"]);
    let req = h.spawn_step("spawn implementer");
    assert_eq!(req.inputs.task.as_deref(), Some("Add order cancellation"));
    assert_eq!(
        h.state().forced_executor(AgentName::Implementer),
        Some(ExecutorKind::Native)
    );
    h.run(
        "spawn implementer phase 1 initial",
        impl_submit(1, &["src/a.rs"]),
    );
    assert_eq!(h.summaries(), vec!["command stage p"]);
    h.command(CommandPurpose::Stage, "p");
    assert_eq!(h.summaries(), vec!["judge completion"]);
}

#[test]
fn quick_change_with_nothing_changed_skips_staging() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(JudgeKind::Classify, None, json!({"category": "QUICK_CHANGE", "projects": ["p"], "explore_tasks": [], "opts_in": {"tests": false, "docs": false}, "reason": "r"}));
    h.run("spawn implementer phase 1 initial", impl_submit(1, &[]));
    assert_eq!(h.summaries(), vec!["judge completion"]);
}

#[test]
fn other_categories_follow_the_routing() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("IMPLEMENT", &["p"]);
    assert_eq!(h.state().forced_executor(AgentName::Implementer), None);
}

#[test]
fn unit_test_category_skips_the_closing_gate() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(JudgeKind::Classify, None, json!({"category": "UNIT_TEST", "projects": ["p"], "explore_tasks": [], "opts_in": {"tests": true, "docs": false}, "reason": "r"}));
    assert_eq!(
        h.summaries(),
        vec!["spawn execution-path-analyzer epa phase 1"]
    );
}

#[test]
fn classify_override_before_work_starts() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("RESEARCH", &["p"]);
    let st = h.state();
    let id = st.classify.clone().unwrap();
    assert!(st.can_override(&id));
    h.ev(SessionEvent::DecisionOverridden {
        id,
        output: json!({"category": "IMPLEMENT", "projects": ["p"], "explore_tasks": [{"project": "p", "task": "t"}], "opts_in": {"tests": false, "docs": false}, "reason": "user"}),
        reason: "user".into(),
    });
    assert_eq!(
        h.state().category,
        Some(ostra_core::pipeline::Category::Implement)
    );
    h.start("spawn explore");
    let st = h.state();
    assert!(!st.can_override(st.classify.as_ref().unwrap()));
}

#[test]
fn stage_kinds_map_to_lanes() {
    assert_eq!(
        StageKind::FactCheckSpec.lane(),
        ostra_core::pipeline::Lane::Verification
    );
    let _ = AgentName::Explore;
}

// ------------------------------------------------------------------------------------------
// Cost controls: the session budget pauses spawning; YOLO never raises it.
// ------------------------------------------------------------------------------------------

fn spend(h: &mut H, prefix: &str, submit: Value, cost: f64) {
    let (id, _) = h.start(prefix);
    h.ev(SessionEvent::ExecutionFinished {
        id,
        result: ExecutionResult {
            status: ExecutionStatus::Ok,
            submit: Some(submit),
            final_text: String::new(),
            usage: Usage {
                cost_usd: cost,
                ..Default::default()
            },
            native_session_id: None,
            error: None,
        },
    });
}

#[test]
fn budget_pauses_spawns_until_raised() {
    let mut h = H::new(
        &["p"],
        SessionOptions {
            yolo: true,
            ..Default::default()
        },
    );
    h.ctx.budget_usd = Some(1.0);
    h.classify("IMPLEMENT", &["p"]);
    spend(&mut h, "spawn explore", explore_submit(0, &[]), 1.5);
    // The spec spawn becomes a budget gate, and YOLO leaves it open.
    assert_eq!(h.summaries(), vec!["gate budget_reached"]);
    let g = h.open_gate("budget_reached");
    assert_eq!(
        h.summaries(),
        Vec::<String>::new(),
        "no yolo-answer for a budget gate"
    );
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "raise".into(),
            text: Some("5".into()),
        },
    );
    assert_eq!(h.summaries(), vec!["spawn generate-spec spec#1"]);
}

#[test]
fn budget_stop_ends_the_session() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.ctx.budget_usd = Some(1.0);
    h.classify("IMPLEMENT", &["p"]);
    spend(&mut h, "spawn explore", explore_submit(0, &[]), 2.0);
    let g = h.open_gate("budget_reached");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "stop".into(),
            text: None,
        },
    );
    assert!(h.state().failed.is_some());
    assert_eq!(h.summaries(), Vec::<String>::new());
}

#[test]
fn no_budget_means_no_limit() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("IMPLEMENT", &["p"]);
    spend(&mut h, "spawn explore", explore_submit(0, &[]), 500.0);
    assert_eq!(h.summaries(), vec!["spawn generate-spec spec#1"]);
}

#[test]
fn init_generates_at_most_eight_skills_by_default() {
    let skills: Vec<Value> = (0..12)
        .map(|i| json!({"name": format!("s{i}"), "kind": "creation", "status": "new", "recommend": true}))
        .collect();
    let proposals = ostra_engine::init::proposals(&json!({"skills": skills}));
    assert_eq!(
        proposals
            .iter()
            .filter(|p| p.disposition == "generate")
            .count(),
        ostra_engine::init::MAX_DEFAULT_GENERATE
    );
    assert_eq!(
        proposals.iter().filter(|p| p.disposition == "drop").count(),
        4
    );
}

fn init_session() -> H {
    let mut h = H {
        id: SessionId::from("s1"),
        events: vec![],
        ctx: PlanCtx::default(),
    };
    h.ev(SessionEvent::SessionCreated {
        kind: SessionKind::Init {
            project: "p".into(),
        },
        request: String::new(),
        options: SessionOptions::default(),
        projects: vec![ProjectRef {
            key: "p".into(),
            path: PathBuf::from("/code/p"),
        }],
        workspace_root: PathBuf::from("/ws"),
        session_root: root(),
    });
    h
}

fn detect_submit(slices: Value, existing: Value) -> Value {
    json!({"status": "ok", "summary": "s", "files": [], "result": {
        "scout_plan_path": "/ws/.ostra/sessions/s1/p/ostra-scout-plan.md",
        "stack": "go",
        "reference_name": "go",
        "slices": slices,
        "existing_skills": existing,
        "ultracode_bootstrap": false
    }})
}

#[test]
fn rule_i1_existing_skills_covering_the_project_skip_the_scouts() {
    let mut h = init_session();
    assert_eq!(h.summaries(), vec!["spawn initializer init detect"]);
    h.run(
        "spawn initializer init detect",
        detect_submit(
            json!([]),
            json!([{"name": "convention", "kind": "convention", "path": ".agents/skills/convention/SKILL.md", "description": "d"}]),
        ),
    );
    assert_eq!(h.summaries(), vec!["spawn initializer init propose"]);
}

#[test]
fn rule_i1_no_slices_and_no_existing_skills_fail() {
    let mut h = init_session();
    h.run(
        "spawn initializer init detect",
        detect_submit(json!([]), json!([])),
    );
    assert_eq!(h.summaries(), vec!["fail"]);
}

#[test]
fn rule_i1_slices_still_fan_out_beside_existing_skills() {
    let mut h = init_session();
    h.run(
        "spawn initializer init detect",
        detect_submit(
            json!([{"descriptor": "api", "slug": "api", "paths": ["api"]}]),
            json!([{"name": "entity", "kind": "other", "path": ".ostra/skills/entity/SKILL.md", "description": "d"}]),
        ),
    );
    assert_eq!(h.summaries(), vec!["spawn initializer init scout api"]);
}
