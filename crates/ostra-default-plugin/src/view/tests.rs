//! Tests of the views.

use super::*;
use ostra_core::ExecutorKind;
use ostra_core::agent::AgentName;
use ostra_core::api::ExecutionView;
use ostra_core::event::{
    ExecPurpose, ProjectRef, SessionEvent, SessionOptions, StoredEvent, WorkKind,
};
use ostra_core::event::{GatePayload, JudgeKind, SessionKind};
use ostra_core::exec::ExecutionStatus;
use ostra_core::exec::Usage;
use ostra_core::executor::ExecStream;
use ostra_core::ids::{DecisionId, GateId, SessionId};
use ostra_core::ids::{ExecutionId, WorkspaceId};
use ostra_core::paths;
use ostra_core::pipeline::StageKind;
use ostra_engine::state::SessionState;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;

fn log(events: Vec<SessionEvent>) -> Vec<StoredEvent> {
    let t0 = chrono::Utc::now();
    events
        .into_iter()
        .enumerate()
        .map(|(i, event)| StoredEvent {
            seq: i as i64 + 1,
            at: t0 + chrono::Duration::seconds(i as i64),
            event,
        })
        .collect()
}

fn created(kind: SessionKind) -> SessionEvent {
    SessionEvent::SessionCreated {
        kind,
        request: "Let customers cancel an order".into(),
        options: SessionOptions::default(),
        projects: vec![ProjectRef {
            key: "backend".into(),
            path: PathBuf::from("/code/backend"),
        }],
        workspace_root: PathBuf::from("/ws"),
        session_root: PathBuf::from("/ws/.ostra/sessions/s1"),
        files: vec![],
        uploads: vec![],
        pinned: vec![],
        docs_book: None,
        workflow: None,
    }
}

fn classify_output(title: &str) -> serde_json::Value {
    json!({"category": "RESEARCH", "projects": ["backend"], "explore_tasks": [], "opts_in": {"tests": false, "docs": false}, "reason": "r", "title": title})
}

fn started(id: &str, agent: AgentName, purpose: ExecPurpose) -> SessionEvent {
    SessionEvent::ExecutionStarted {
        id: ExecutionId::from(id),
        agent,
        purpose,
        stage: StageKind::Implement,
        project: "backend".into(),
        executor: ExecutorKind::Native,
        model: "m".into(),
        params: json!({}),
        spawn_block: String::new(),
        report_path: None,
        resumes: None,
        contract: None,
    }
}

fn view(
    id: &str,
    agent: AgentName,
    status: ExecutionStatus,
    at: i64,
    cost: f64,
    executor: ExecutorKind,
) -> ExecutionView {
    ExecutionView {
        id: ExecutionId::from(id),
        session: Some(SessionId::from("s1")),
        agent,
        purpose: None,
        stage: None,
        project: "backend".into(),
        executor,
        model: "m".into(),
        status,
        started_at: chrono::DateTime::from_timestamp(at, 0).unwrap(),
        ended_at: None,
        usage: Usage {
            cost_usd: cost,
            ..Default::default()
        },
        report_path: None,
        native_session_id: None,
        spawn_block: String::new(),
        error: None,
        can_resume: false,
        can_skip: false,
        can_steer: false,
        queued_steer: None,
        has_terminal: false,
        group: ostra_core::api::execution_group(agent, Some("backend")),
        spans_session: false,
        run_label: agent.to_string(),
        stream: executor.stream(),
        summary: None,
        has_transcript: false,
        pending_gate: None,
        repo_root: None,
    }
}

#[test]
fn titles_come_from_classify_and_init() {
    crate::install();
    let id = SessionId::from("s1");
    let init = crate::fold_session(
        id.clone(),
        &log(vec![created(SessionKind::Init {
            project: "web".into(),
        })]),
    );
    assert_eq!(
        summary(&init, &WorkspaceId::from("w"), 0.0)
            .title
            .as_deref(),
        Some("Initialize web")
    );

    let d = DecisionId::new();
    let mut events = vec![created(SessionKind::Pipeline)];
    assert_eq!(
        crate::fold_session(id.clone(), &log(events.clone())).title,
        None
    );
    events.push(SessionEvent::DecisionMade {
        id: d.clone(),
        judge: JudgeKind::Classify,
        subject: None,
        input_summary: String::new(),
        output: classify_output("  \"Order cancellation.\" "),
        reason: "r".into(),
    });
    assert_eq!(
        crate::fold_session(id.clone(), &log(events.clone()))
            .title
            .as_deref(),
        Some("Order cancellation")
    );
    let over = |title: &str| SessionEvent::DecisionOverridden {
        id: d.clone(),
        output: classify_output(title),
        reason: "user".into(),
    };
    events.push(over("Cancel orders"));
    assert_eq!(
        crate::fold_session(id.clone(), &log(events.clone()))
            .title
            .as_deref(),
        Some("Cancel orders")
    );
    events.push(over(""));
    assert_eq!(
        crate::fold_session(id, &log(events)).title.as_deref(),
        Some("Cancel orders"),
        "an override without a title keeps it"
    );
}

#[test]
fn repeated_runs_are_numbered_per_agent_and_label() {
    crate::install();
    let fix = || ExecPurpose::Implement {
        phase: 1,
        work: WorkKind::Fix,
    };
    let review = |iteration| ExecPurpose::Review {
        phase: 1,
        tests: false,
        iteration,
    };
    let s = crate::fold_session(
        SessionId::from("s1"),
        &log(vec![
            created(SessionKind::Pipeline),
            started(
                "x_1",
                AgentName::GenerateSpec,
                ExecPurpose::Spec { round: 1 },
            ),
            started(
                "x_2",
                AgentName::GenerateSpec,
                ExecPurpose::Spec { round: 2 },
            ),
            started(
                "x_3",
                AgentName::Implementer,
                ExecPurpose::Implement {
                    phase: 1,
                    work: WorkKind::Initial,
                },
            ),
            started("x_4", AgentName::CodeReviewer, review(1)),
            started("x_5", AgentName::Implementer, fix()),
            started("x_6", AgentName::CodeReviewer, review(2)),
            started("x_7", AgentName::Implementer, fix()),
            started(
                "x_8",
                AgentName::Implementer,
                ExecPurpose::Implement {
                    phase: 2,
                    work: WorkKind::Initial,
                },
            ),
        ]),
    );
    let labels = run_labels(&s);
    let got: Vec<&str> = (1..=8)
        .map(|i| labels[&ExecutionId::from(format!("x_{i}"))].as_str())
        .collect();
    assert_eq!(
        got,
        [
            "Spec",
            "Spec · pass 2",
            "Phase 1",
            "Phase 1 · review pass",
            "Phase 1 · fix pass",
            "Phase 1 · review pass 2",
            "Phase 1 · fix pass 2",
            "Phase 2"
        ]
    );
}

#[test]
fn groups_aggregate_status_and_cost_in_start_order() {
    crate::install();
    let native = ExecutorKind::Native;
    let groups = execution_groups(&[
        view(
            "x_3",
            AgentName::Implementer,
            ExecutionStatus::Running,
            30,
            0.5,
            native,
        ),
        view(
            "x_1",
            AgentName::Implementer,
            ExecutionStatus::Ok,
            10,
            1.0,
            native,
        ),
        view(
            "x_2",
            AgentName::CodeReviewer,
            ExecutionStatus::Error,
            20,
            0.25,
            native,
        ),
        view(
            "x_4",
            AgentName::CodeReviewer,
            ExecutionStatus::Ok,
            40,
            0.25,
            native,
        ),
    ]);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].group, "implementer:backend");
    assert_eq!(
        groups[0].executions,
        [ExecutionId::from("x_1"), ExecutionId::from("x_3")]
    );
    assert_eq!(
        groups[0].status,
        ExecutionStatus::Running,
        "a running run makes the group running"
    );
    assert_eq!(groups[0].cost_usd, 1.5);
    assert_eq!(groups[1].agent, AgentName::CodeReviewer);
    assert_eq!(
        groups[1].status,
        ExecutionStatus::Ok,
        "otherwise the latest run decides"
    );
    let groups = execution_groups(&[
        view(
            "x_1",
            AgentName::Implementer,
            ExecutionStatus::Running,
            10,
            0.0,
            native,
        ),
        view(
            "x_2",
            AgentName::Implementer,
            ExecutionStatus::Error,
            20,
            0.0,
            native,
        ),
    ]);
    assert_eq!(groups[0].status, ExecutionStatus::Running);
}

#[test]
fn tree_nodes_group_runs_with_numbered_labels() {
    crate::install();
    let s = crate::fold_session(
        SessionId::from("s1"),
        &log(vec![
            created(SessionKind::Pipeline),
            started(
                "x_1",
                AgentName::GenerateSpec,
                ExecPurpose::Spec { round: 1 },
            ),
            started(
                "x_2",
                AgentName::Implementer,
                ExecPurpose::Implement {
                    phase: 1,
                    work: WorkKind::Initial,
                },
            ),
            started(
                "x_3",
                AgentName::GenerateSpec,
                ExecPurpose::Spec { round: 2 },
            ),
        ]),
    );
    let mut row = summary(&s, &WorkspaceId::from("w"), 0.0);
    row.cost_usd = 0.5;
    row.title = Some("Cancel orders".into());
    let native = ExecutorKind::Native;
    let mut spec2 = view(
        "x_3",
        AgentName::GenerateSpec,
        ExecutionStatus::Running,
        30,
        0.25,
        native,
    );
    spec2.summary = Some("Write spec.md".into());
    let executions = vec![
        view(
            "x_1",
            AgentName::GenerateSpec,
            ExecutionStatus::Ok,
            10,
            1.0,
            native,
        ),
        view(
            "x_2",
            AgentName::Implementer,
            ExecutionStatus::Ok,
            20,
            0.5,
            ExecutorKind::Harness(ostra_core::HarnessKind::Codex),
        ),
        spec2,
    ];
    let node = tree_session(&s, &row, executions);
    assert_eq!(node.title.as_deref(), Some("Cancel orders"));
    assert_eq!(node.request, "Let customers cancel an order");
    assert_eq!(node.cost_usd, 1.75, "the runs' spend wins over a stale row");
    let groups: Vec<(&str, ExecutionStatus, f64)> = node
        .groups
        .iter()
        .map(|g| (g.group.as_str(), g.status, g.cost_usd))
        .collect();
    assert_eq!(
        groups,
        [
            ("generate-spec:backend", ExecutionStatus::Running, 1.25),
            ("implementer:backend", ExecutionStatus::Ok, 0.5)
        ]
    );
    let runs: Vec<(&str, &str, ExecStream, Option<&str>)> = node.groups[0]
        .runs
        .iter()
        .map(|r| {
            (
                r.id.as_str(),
                r.run_label.as_str(),
                r.stream,
                r.summary.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        runs,
        [
            ("x_1", "Spec", ExecStream::Activity, None),
            (
                "x_3",
                "Spec · pass 2",
                ExecStream::Activity,
                Some("Write spec.md")
            )
        ]
    );
    assert_eq!(node.groups[1].runs[0].stream, ExecStream::Terminal);
    assert!(node.artifacts.is_empty());
}

#[test]
fn decorate_numbers_labels_and_finds_transcripts() {
    crate::install();
    let tmp = tempfile::tempdir().unwrap();
    let mut s = SessionState::new(crate::pipeline().into(), SessionId::from("s1"));
    s.session_root = tmp.path().to_path_buf();
    let harness = ExecutorKind::Harness(ostra_core::HarnessKind::Claude);
    let mut h = view(
        "x_h",
        AgentName::Implementer,
        ExecutionStatus::Ok,
        1,
        0.0,
        harness,
    );
    let mut n = view(
        "x_n",
        AgentName::Implementer,
        ExecutionStatus::Ok,
        2,
        0.0,
        ExecutorKind::Native,
    );
    let labels = HashMap::from([(ExecutionId::from("x_h"), "Phase 1 · fix pass 2".to_string())]);
    decorate(&s, &labels, &mut h);
    assert_eq!(h.run_label, "Phase 1 · fix pass 2");
    assert!(!h.has_transcript);
    let path = paths::terminal_transcript(tmp.path(), "x_h");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"\x1b[1mhello").unwrap();
    decorate(&s, &labels, &mut h);
    assert!(h.has_transcript);
    decorate(&s, &labels, &mut n);
    assert_eq!(n.run_label, "implementer");
    assert!(!n.has_transcript);
    assert_eq!((n.repo_root.clone(), n.pending_gate.clone()), (None, None));
}

#[test]
fn decorate_finds_the_project_folder_and_the_open_gate_of_an_execution() {
    crate::install();
    let mut s = SessionState::new(crate::pipeline().into(), SessionId::from("s1"));
    s.projects.push(ProjectRef {
        key: "backend".into(),
        path: PathBuf::from("/code/backend"),
    });
    let t0 = chrono::DateTime::from_timestamp(100, 0).unwrap();
    let gate =
        |id: &str, execution: &str, at: i64, answered: bool| ostra_engine::state::GateRecord {
            id: GateId::from(id),
            title: format!("Gate {id}"),
            explanation: String::new(),
            payload: GatePayload::ExecutionFailed {
                execution: ExecutionId::from(execution),
                agent: AgentName::Implementer,
                project: "backend".into(),
                error: "boom".into(),
            },
            answer: answered.then(|| ostra_core::event::GateAnswer::Choice {
                option: "retry".into(),
                text: None,
            }),
            source: None,
            reason: None,
            opened_at: t0 + chrono::Duration::seconds(at),
            answered_at: None,
        };
    for g in [
        gate("g_old", "x_1", 1, false),
        gate("g_new", "x_1", 2, false),
        gate("g_done", "x_1", 3, true),
        gate("g_other", "x_2", 4, false),
    ] {
        s.gates.insert(g.id.clone(), g);
    }
    let mut e = view(
        "x_1",
        AgentName::Implementer,
        ExecutionStatus::Error,
        1,
        0.0,
        ExecutorKind::Native,
    );
    decorate(&s, &HashMap::new(), &mut e);
    assert_eq!(e.repo_root, Some(PathBuf::from("/code/backend")));
    let pending = e.pending_gate.expect("an open gate names x_1");
    assert_eq!(
        (
            pending.id.as_str(),
            pending.kind.as_str(),
            pending.title.as_str()
        ),
        ("g_new", "execution_failed", "Gate g_new")
    );
    let mut quiet = view(
        "x_3",
        AgentName::Implementer,
        ExecutionStatus::Ok,
        1,
        0.0,
        ExecutorKind::Native,
    );
    decorate(&s, &HashMap::new(), &mut quiet);
    assert_eq!(quiet.pending_gate, None);
}
