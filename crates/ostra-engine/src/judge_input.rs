//! Inputs for judge calls, built from session state, and the YOLO answer logic.

use crate::state::{DocsState, EpaState, LoopNext, SessionState, parse_loop_key};
use ostra_core::event::{AnswerSource, GateAnswer, GatePayload, JudgeKind};
use ostra_core::ids::{ExecutionId, GateId};
use ostra_core::pipeline::{Category, QuestionAnswer, TestPolicy};
use serde_json::{Value, json};
use std::fmt::Write;
use std::path::Path;

const FILE_EXCERPT: usize = 24_000;
/// Per research document for the Track judge, which reads every document of the session.
const TRACK_EXCERPT: usize = 12_000;

pub struct ProjectFacts {
    pub key: String,
    pub path: String,
    pub initialized: bool,
    pub stack: Option<String>,
    /// Module map rows from `project.toml`, as `area (glob)`.
    pub areas: Vec<String>,
}

fn excerpt(path: &Path) -> String {
    excerpt_n(path, FILE_EXCERPT)
}

fn excerpt_n(path: &Path, max: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(t) if t.len() > max => {
            let mut end = max;
            while !t.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}\n\n(truncated at {max} characters)", &t[..end])
        }
        Ok(t) => t,
        Err(_) => format!("(could not read {})", path.display()),
    }
}

/// The user message for a judge call, and a short summary stored with the decision.
pub fn judge_input(
    s: &SessionState,
    kind: JudgeKind,
    subject: Option<&str>,
    projects: &[ProjectFacts],
) -> (String, String) {
    let mut m = String::new();
    let request = s.full_request();
    match kind {
        JudgeKind::Classify => {
            let _ = writeln!(m, "# Request\n\n{request}\n");
            let _ = writeln!(
                m,
                "# Toggles from the New task form\n\n- tests: {}\n- docs: {}\n- yolo: {}\n",
                s.options.tests, s.options.docs, s.options.yolo
            );
            let _ = writeln!(m, "# Projects in this workspace\n");
            for p in projects {
                let _ = writeln!(
                    m,
                    "- `{}` at {}{}{}",
                    p.key,
                    p.path,
                    p.stack
                        .as_ref()
                        .map(|s| format!(", stack {s}"))
                        .unwrap_or_default(),
                    if p.initialized {
                        ""
                    } else {
                        " (not initialized: no pipeline task may target it)"
                    }
                );
                if !p.areas.is_empty() {
                    let _ = writeln!(m, "  - Areas: {}", p.areas.join("; "));
                }
            }
            let summary = format!("Request: {}", first_line(&request));
            (m, summary)
        }
        JudgeKind::Sufficiency => {
            let covered: Vec<u32> = subject
                .unwrap_or_default()
                .split(',')
                .filter_map(|x| x.parse().ok())
                .collect();
            let _ = writeln!(m, "# Request\n\n{request}\n\n# Research returned\n");
            for t in &s.explore {
                let Some(r) = &t.result else { continue };
                let _ = writeln!(
                    m,
                    "## Task {} ({})\n\nTask: {}\nScope covered: {}\nFindings: {}\nResearch document: {}\n",
                    t.idx, t.project, t.task, r.scope_covered, r.findings_summary, r.research_path
                );
                if covered.contains(&t.idx) && !r.not_covered.is_empty() {
                    let _ = writeln!(m, "Not covered:");
                    for n in &r.not_covered {
                        let _ = writeln!(m, "- {n}");
                    }
                    m.push('\n');
                }
            }
            let _ = writeln!(
                m,
                "Project keys you may target: {}",
                s.projects
                    .iter()
                    .map(|p| p.key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let n: usize = s
                .explore
                .iter()
                .filter(|t| covered.contains(&t.idx))
                .filter_map(|t| t.result.as_ref())
                .map(|r| r.not_covered.len())
                .sum();
            (
                m,
                format!(
                    "{n} Not covered items across {} research documents",
                    covered.len()
                ),
            )
        }
        JudgeKind::Stakes => {
            let _ = writeln!(m, "# Request\n\n{request}\n");
            if let Some(spec) = &s.spec.current {
                let _ = writeln!(
                    m,
                    "# Approved spec\n\n{} deliverables, {} requirements.\nSummary: {}\n\n{}",
                    spec.deliverables,
                    spec.requirements,
                    spec.summary,
                    excerpt(Path::new(&spec.spec_path))
                );
            }
            let _ = writeln!(m, "\nProjects in scope: {}", s.scope.join(", "));
            (m, "Approved spec and request".into())
        }
        JudgeKind::Track => {
            let _ = writeln!(m, "# Request\n\n{request}\n");
            let _ = writeln!(m, "Projects in scope: {}\n", s.scope.join(", "));
            let _ = writeln!(m, "# Research returned\n");
            for t in &s.explore {
                let Some(r) = &t.result else { continue };
                let _ = writeln!(
                    m,
                    "## Task {} ({})\n\nTask: {}\nScope covered: {}\nFindings: {}\nNot covered: {}\n\n{}\n",
                    t.idx,
                    t.project,
                    t.task,
                    r.scope_covered,
                    r.findings_summary,
                    if r.not_covered.is_empty() {
                        "none".to_string()
                    } else {
                        r.not_covered.join("; ")
                    },
                    excerpt_n(Path::new(&r.research_path), TRACK_EXCERPT)
                );
            }
            (m, format!("Research for: {}", first_line(&request)))
        }
        JudgeKind::Feedback => {
            let round = subject
                .and_then(|x| x.parse::<usize>().ok())
                .and_then(|i| s.feedback.rounds.get(i));
            let text = round.map(|r| r.text.clone()).unwrap_or_default();
            let _ = writeln!(m, "# Request\n\n{request}\n");
            let _ = writeln!(
                m,
                "Track: {}\nProjects in scope: {}\n",
                s.track.map(|t| t.as_str()).unwrap_or("full"),
                s.scope.join(", ")
            );
            match &s.spec.current {
                Some(spec) => {
                    let _ = writeln!(
                        m,
                        "# Current spec\n\nSummary: {}\nSpec file: {}\n\n{}\n",
                        spec.summary,
                        spec.spec_path,
                        excerpt(Path::new(&spec.spec_path))
                    );
                }
                None => {
                    let _ = writeln!(
                        m,
                        "# No spec\n\nThis session has no spec, so every route builds a revision directly.\n"
                    );
                }
            }
            let _ = writeln!(m, "# Phases built so far\n");
            for p in s.phases.values() {
                let _ = writeln!(
                    m,
                    "- Phase {} ({}): {}. Report: {}",
                    p.info.id,
                    p.info.project,
                    p.info.title,
                    p.implementer_report
                        .as_ref()
                        .map(|r| r.display().to_string())
                        .unwrap_or_else(|| "none".into())
                );
            }
            let _ = writeln!(m, "\n# The user's feedback on the implementation\n\n{text}");
            (m, format!("Feedback: {}", first_line(&text)))
        }
        JudgeKind::RouteAnswer => {
            let gate = subject.and_then(|g| s.gates.get(&GateId::from(g)));
            let text = s
                .phases
                .values()
                .flat_map(|p| [&p.impl_loop, &p.test_loop])
                .find_map(|l| match &l.next {
                    LoopNext::AwaitRoute { gate: g, text, .. } if Some(g.as_str()) == subject => {
                        Some(text.clone())
                    }
                    _ => None,
                })
                .unwrap_or_default();
            let _ = writeln!(m, "# Request\n\n{request}\n");
            if let Some(spec) = &s.spec.current {
                let _ = writeln!(
                    m,
                    "# Current spec summary\n\n{}\nSpec file: {}\n",
                    spec.summary, spec.spec_path
                );
            }
            if let Some(g) = gate {
                let _ = writeln!(
                    m,
                    "# The question that was asked\n\n{}\n{}\n",
                    g.title, g.explanation
                );
            }
            let _ = writeln!(m, "# The user's answer\n\n{text}");
            (m, format!("Answer: {}", first_line(&text)))
        }
        JudgeKind::Rescue => {
            let exec = subject.map(ExecutionId::from);
            let rec = exec.as_ref().and_then(|e| s.executions.get(e));
            let stuck = rec
                .and_then(|r| r.loop_key)
                .and_then(|k| s.loop_ref(k))
                .and_then(|l| match &l.next {
                    LoopNext::Rescue { stuck, .. } => Some(stuck.clone()),
                    _ => None,
                });
            let _ = writeln!(m, "# Request\n\n{request}\n");
            if let Some(r) = rec {
                let _ = writeln!(m, "# Stuck agent\n\n{} in project `{}`", r.agent, r.project);
                if let Some(k) = r.loop_key
                    && let Some(p) = s.phases.get(&k.0)
                {
                    let _ = writeln!(
                        m,
                        "Phase {}: {}{}",
                        p.info.id,
                        p.info.title,
                        p.info
                            .file
                            .as_ref()
                            .map(|f| format!(" ({})", f.display()))
                            .unwrap_or_default()
                    );
                }
            }
            if let Some(st) = &stuck {
                let _ = writeln!(
                    m,
                    "\n# Diagnostic, verbatim\n\n{}\n\n# What it needs\n\n{}",
                    st.diagnostic, st.need
                );
            }
            let previous: Vec<String> = s
                .decisions
                .values()
                .filter(|d| d.judge == JudgeKind::Rescue)
                .map(|d| {
                    format!(
                        "- {}: {}",
                        d.output
                            .get("action")
                            .and_then(|a| a.as_str())
                            .unwrap_or("?"),
                        d.reason
                    )
                })
                .collect();
            if !previous.is_empty() {
                let _ = writeln!(
                    m,
                    "\n# Earlier rescues in this session\n\n{}",
                    previous.join("\n")
                );
            }
            (
                m,
                format!(
                    "STUCK: {}",
                    stuck.map(|s| first_line(&s.need)).unwrap_or_default()
                ),
            )
        }
        JudgeKind::ResolveReview => {
            let key = subject.and_then(parse_loop_key);
            let l = key.and_then(|k| s.loop_ref(k));
            let project = key
                .and_then(|k| s.phases.get(&k.0))
                .map(|p| p.info.project.clone())
                .unwrap_or_default();
            let _ = writeln!(m, "# Review loop at its budget under YOLO\n");
            if let (Some(k), Some(l)) = (key, l) {
                let _ = writeln!(
                    m,
                    "Phase {}{}; review passes so far: {}; resolution rounds: {}",
                    k.0,
                    if k.1 { " (tests)" } else { "" },
                    l.iterations,
                    l.resolve_rounds
                );
                if let LoopNext::Resolve { findings } = &l.next {
                    let _ = writeln!(m, "\n# Open findings\n");
                    for f in findings {
                        let _ = writeln!(m, "- {}", f.line());
                    }
                }
                let ledger = s.ledger_path(&project, k.0, k.1);
                let _ = writeln!(
                    m,
                    "\n# Review ledger ({})\n\n{}",
                    ledger.display(),
                    excerpt(&ledger)
                );
                if let Some(p) = s.phases.get(&k.0).and_then(|p| p.info.file.clone()) {
                    let _ = writeln!(m, "\n# Phase file ({})\n\n{}", p.display(), excerpt(&p));
                }
            }
            (m, "Open findings and the review ledger".into())
        }
        JudgeKind::YoloAnswer => {
            let gate = subject.and_then(|g| s.gates.get(&GateId::from(g)));
            let _ = writeln!(m, "# Request\n\n{request}\n");
            if let Some(g) = gate {
                let _ = writeln!(m, "# Gate: {}\n\n{}\n", g.title, g.explanation);
                let _ = writeln!(
                    m,
                    "```json\n{}\n```\n",
                    serde_json::to_string_pretty(&g.payload).unwrap_or_default()
                );
                match &g.payload {
                    GatePayload::OpenQuestions { .. } => {
                        let _ = writeln!(m, "# Research findings\n");
                        for t in &s.explore {
                            if let Some(r) = &t.result {
                                let _ = writeln!(
                                    m,
                                    "- {} ({}): {}",
                                    r.research_path, t.project, r.findings_summary
                                );
                            }
                        }
                    }
                    GatePayload::SpecApproval { spec_path, .. } => {
                        let _ = writeln!(m, "# Spec\n\n{}", excerpt(spec_path));
                    }
                    GatePayload::PlanApproval { plan_path, .. } => {
                        let _ = writeln!(m, "# Master plan\n\n{}", excerpt(plan_path));
                    }
                    _ => {}
                }
            }
            (m, gate.map(|g| g.title.clone()).unwrap_or_default())
        }
        JudgeKind::Completion => completion_input(s),
    }
}

fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or_default();
    if l.len() > 160 {
        format!(
            "{}...",
            &l[..l.char_indices().nth(157).map(|(i, _)| i).unwrap_or(l.len())]
        )
    } else {
        l.to_string()
    }
}

fn completion_input(s: &SessionState) -> (String, String) {
    let mut m = String::new();
    let _ = writeln!(m, "# Request\n\n{}\n", s.full_request());
    let _ = writeln!(
        m,
        "Category: {}\nYOLO: {}\n",
        s.category.map(|c| c.to_string()).unwrap_or_default(),
        s.yolo
    );
    if s.category == Some(Category::QuickAnswer) {
        return (m, "Quick answer".into());
    }
    if let (Some(Category::Implement), Some(track)) = (s.category, s.track) {
        let _ = writeln!(
            m,
            "Track: {}{}\n",
            track.as_str(),
            if track == ostra_core::pipeline::Track::Light {
                " (the spec, fact-check, and plan stages did not run)"
            } else {
                ""
            }
        );
    }
    if !s.feedback.rounds.is_empty() {
        let _ = writeln!(m, "# Feedback rounds after the implementation\n");
        for (i, r) in s.feedback.rounds.iter().enumerate() {
            let _ = writeln!(
                m,
                "- Round {}: {} (built as phases {:?})",
                i + 1,
                first_line(&r.text),
                r.phases
            );
        }
        m.push('\n');
    }
    let _ = writeln!(m, "# Research\n");
    for t in &s.explore {
        match &t.result {
            Some(r) => {
                let _ = writeln!(m, "- {}: {}", r.research_path, r.findings_summary);
            }
            None if t.abandoned => {
                let _ = writeln!(m, "- Task {} abandoned: {}", t.idx, first_line(&t.task));
            }
            None => {}
        }
    }
    if let Some(spec) = &s.spec.current {
        let _ = writeln!(
            m,
            "\n# Spec\n\n{} ({}). Approved: {}.",
            spec.spec_path, spec.summary, s.spec.approved
        );
    }
    match (&s.stakes, &s.plan.current) {
        (Some((_, st)), None) => {
            let _ = writeln!(
                m,
                "\n# Plan\n\nStakes {st:?}: the plan stage was skipped and the change ran inline."
            );
        }
        (_, Some(plan)) => {
            let _ = writeln!(
                m,
                "\n# Plan\n\n{} ({})",
                plan.master_plan_path, plan.summary
            );
        }
        _ => {}
    }
    if !s.phases.is_empty() {
        let removed = crate::plan::removed_phases(s);
        let _ = writeln!(m, "\n# Phases\n");
        for p in s.phases.values() {
            let status = if removed.contains(&p.info.id) {
                "removed because a phase it depends on failed".to_string()
            } else {
                match &p.impl_loop.next {
                    LoopNext::Done => {
                        format!("passed review after {} passes", p.impl_loop.iterations)
                    }
                    LoopNext::Blocked { reason } => format!("BLOCKED: {reason}"),
                    other => format!("{other:?}"),
                }
            };
            let _ = writeln!(
                m,
                "- Phase {} ({}, {}): {}. {status}",
                p.info.id,
                p.info.project,
                p.info.title,
                p.info.test_policy_label()
            );
            if !p.impl_loop.leftover_low.is_empty() {
                for f in &p.impl_loop.leftover_low {
                    let _ = writeln!(m, "  - LOW finding left open: {}", f.line());
                }
            }
            if p.impl_loop.blocker_open || p.test_loop.blocker_open {
                let _ = writeln!(m, "  - A BLOCKER security finding is still open.");
            }
            let tests = match (&p.epa, &p.test_loop.next) {
                (EpaState::NotStarted, _) => None,
                (EpaState::Abandoned, _) => Some("test stage abandoned".to_string()),
                (_, LoopNext::Done) => Some(format!(
                    "tests written and reviewed ({} passes)",
                    p.test_loop.iterations
                )),
                (_, LoopNext::Blocked { reason }) => Some(format!("tests BLOCKED: {reason}")),
                _ => Some("tests incomplete".into()),
            };
            if let Some(t) = tests {
                let _ = writeln!(m, "  - {t}");
            }
        }
        let _ = writeln!(m, "\n# Closing stages per project\n");
        for (k, t) in &s.project_tracks {
            let closing = t
                .closing
                .map(|(a, b)| format!("tests {}, docs {}", yes(a), yes(b)))
                .unwrap_or_else(|| "not reached".into());
            let docs = match &t.docs {
                DocsState::Done(p) => format!(
                    "docs written ({})",
                    p.as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                ),
                DocsState::Abandoned => "docs abandoned".into(),
                DocsState::NotStarted => "docs not run".into(),
                _ => "docs incomplete".into(),
            };
            let format = match t.format {
                Some(None) => {
                    "no format command in project.toml, so format was skipped".to_string()
                }
                Some(Some(0)) => "the format command ran and exited 0".to_string(),
                Some(Some(code)) => format!("format exited {code}"),
                None => "format not run".into(),
            };
            let _ = writeln!(m, "- {k}: {format}; closing gate: {closing}; {docs}");
        }
    }
    let yolo: Vec<String> = s
        .gates
        .values()
        .filter(|g| g.source == Some(AnswerSource::Yolo))
        .map(|g| {
            format!(
                "- {} (gate {}): {}",
                g.title,
                g.id,
                g.reason.clone().unwrap_or_default()
            )
        })
        .collect();
    if !yolo.is_empty() {
        let _ = writeln!(m, "\n# Decided for you under YOLO\n\n{}", yolo.join("\n"));
    }
    if !s.notes.is_empty() {
        let _ = writeln!(
            m,
            "\n# Notes\n\n{}",
            s.notes
                .iter()
                .map(|n| format!("- {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    (m, "Session state at completion".into())
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

trait PolicyLabel {
    fn test_policy_label(&self) -> String;
}

impl PolicyLabel for ostra_core::pipeline::PhaseInfo {
    fn test_policy_label(&self) -> String {
        match self.test_policy {
            TestPolicy::Required => "tests Required".into(),
            TestPolicy::Skip => format!(
                "tests Skip ({})",
                self.test_rationale.clone().unwrap_or_default()
            ),
        }
    }
}

/// How the engine answers a gate under YOLO.
pub enum YoloPlan {
    /// No judgment needed.
    Fixed { answer: GateAnswer, reason: String },
    /// The YOLO judge answers, against this schema.
    Judge { schema: Value },
}

pub fn yolo_plan(s: &SessionState, gate: &GateId) -> Option<YoloPlan> {
    let g = s.gates.get(gate)?;
    let choice = |option: &str, reason: &str| YoloPlan::Fixed {
        answer: GateAnswer::Choice {
            option: option.into(),
            text: None,
        },
        reason: reason.into(),
    };
    Some(match &g.payload {
        GatePayload::OpenQuestions { questions, .. } => YoloPlan::Judge {
            schema: json!({
                "type": "object",
                "properties": {
                    "kind": {"const": "questions"},
                    "answers": {"type": "array", "items": {
                        "type": "object",
                        "properties": {"id": {"type": "string"}, "question": {"type": "string"}, "answer": {"type": "string", "description": "The chosen option's label, or free text."}},
                        "required": ["id", "question", "answer"]
                    }, "minItems": questions.len(), "maxItems": questions.len()}
                },
                "required": ["kind", "answers"]
            }),
        },
        GatePayload::SpecApproval { .. } | GatePayload::PlanApproval { .. } => YoloPlan::Judge {
            schema: json!({
                "type": "object",
                "properties": {
                    "kind": {"const": "approval"},
                    "approved": {"type": "boolean"},
                    "feedback": {"type": ["string", "null"], "description": "What to change, when not approved."}
                },
                "required": ["kind", "approved"]
            }),
        },
        GatePayload::Stuck { .. } => YoloPlan::Judge {
            schema: json!({
                "type": "object",
                "properties": {
                    "kind": {"const": "choice"},
                    "option": {"type": "string", "enum": ["fact", "block"]},
                    "text": {"type": ["string", "null"], "description": "The missing fact, when option is fact. Never guess a fact only the user has."}
                },
                "required": ["kind", "option"]
            }),
        },
        GatePayload::ImplementationReview { .. } => choice(
            "done",
            "Under YOLO the implementation is accepted as built, because only the user can say what they want changed.",
        ),
        GatePayload::ClosingGate { items } => YoloPlan::Fixed {
            answer: GateAnswer::Closing {
                items: items
                    .iter()
                    .map(|i| ostra_core::event::ClosingChoice {
                        project: i.project.clone(),
                        tests: !i.ask_tests && s.tests_requested(),
                        docs: !i.ask_docs && s.docs_requested(),
                    })
                    .collect(),
            },
            reason: "Under YOLO the closing gate takes its recommended defaults, no tests and no docs, unless the request already asked for them (Rules T2, T3).".into(),
        },
        GatePayload::FactCheckRecurring { passes, .. } => {
            if *passes < 6 {
                choice("another-round", "Under YOLO another fact-check round runs while the budget allows.")
            } else {
                choice("stop", "The fact-check failed six times in a row, so the engine stopped rather than guess.")
            }
        }
        GatePayload::ReviewCap { .. } => choice("another-pass", "Under YOLO the review loop keeps its larger budget."),
        GatePayload::PhaseBlocked { .. } => choice("leave", "Under YOLO a blocked phase is recorded and independent work continues (Rule D9)."),
        GatePayload::ExecutionFailed { execution, .. } => {
            let agent = s.executions.get(execution).map(|r| r.agent);
            let prior = s
                .gates
                .values()
                .filter(|x| x.answer.is_some())
                .filter(|x| matches!(&x.payload, GatePayload::ExecutionFailed { execution: e, .. } if s.executions.get(e).map(|r| r.agent) == agent))
                .count();
            if prior < 2 {
                choice("retry", "Under YOLO a failed execution is retried.")
            } else {
                choice("abandon", "This agent failed three times, so the engine abandoned the step instead of retrying again.")
            }
        }
        GatePayload::HarnessFailure { .. } => choice("native", "Under YOLO a harness failure re-routes the execution to the native executor."),
        GatePayload::SkillApproval { skills, .. } => YoloPlan::Fixed {
            answer: GateAnswer::Skills {
                decisions: skills
                    .iter()
                    .map(|k| ostra_core::event::SkillDecision { name: k.name.clone(), disposition: k.disposition.clone() })
                    .collect(),
            },
            reason: "Under YOLO the proposal's default dispositions are used.".into(),
        },
        GatePayload::Permission { .. } => YoloPlan::Fixed {
            answer: GateAnswer::Permission { answer: ostra_core::policy::PermissionAnswer::AllowOnce },
            reason: "Under YOLO every permission ask is allowed.".into(),
        },
        // Spending more is the user's decision, so YOLO leaves a budget gate open.
        GatePayload::BudgetReached { .. } => return None,
    })
}

/// Turn the YOLO judge's `answer` into a gate answer, enforcing what must stay true: approval
/// only with a fact-check PASS, and every question answered.
pub fn yolo_answer_from_judge(
    s: &SessionState,
    gate: &GateId,
    answer: &Value,
) -> Result<GateAnswer, String> {
    let g = s.gates.get(gate).ok_or("unknown gate")?;
    match &g.payload {
        GatePayload::OpenQuestions { questions, .. } => {
            let answers: Vec<QuestionAnswer> =
                serde_json::from_value(answer.get("answers").cloned().unwrap_or(Value::Null))
                    .map_err(|e| format!("answers: {e}"))?;
            let complete: Vec<QuestionAnswer> = questions
                .iter()
                .map(|q| {
                    answers
                        .iter()
                        .find(|a| a.id == q.id)
                        .cloned()
                        .unwrap_or_else(|| QuestionAnswer {
                            id: q.id.clone(),
                            question: q.question.clone(),
                            answer: q
                                .options
                                .get(q.recommended)
                                .map(|o| o.label.clone())
                                .unwrap_or_default(),
                        })
                })
                .collect();
            Ok(GateAnswer::Questions { answers: complete })
        }
        GatePayload::SpecApproval { .. } | GatePayload::PlanApproval { .. } => {
            let approved = answer
                .get("approved")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let feedback = answer
                .get("feedback")
                .and_then(|v| v.as_str())
                .map(String::from)
                .filter(|f| !f.trim().is_empty());
            let passed = match &g.payload {
                GatePayload::SpecApproval { .. } => s.spec.passed_current(),
                _ => s.plan.passed_current(),
            };
            if approved && !passed {
                return Err("approval requires a fact-check PASS".into());
            }
            if !approved && feedback.is_none() {
                return Err("a rejection needs feedback saying what to change".into());
            }
            Ok(GateAnswer::Approval { approved, feedback })
        }
        GatePayload::Stuck { .. } => {
            let option = answer
                .get("option")
                .and_then(|v| v.as_str())
                .unwrap_or("block")
                .to_string();
            let text = answer
                .get("text")
                .and_then(|v| v.as_str())
                .map(String::from)
                .filter(|t| !t.trim().is_empty());
            let option = if option == "fact" && text.is_none() {
                "block".into()
            } else {
                option
            };
            Ok(GateAnswer::Choice { option, text })
        }
        _ => Err("this gate is answered without the judge".into()),
    }
}
