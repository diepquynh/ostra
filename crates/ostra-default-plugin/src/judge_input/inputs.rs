//! Each judge's input: the facts it decides on, as text, and the summary the log records.

use super::*;
#[allow(unused_imports)]
use crate::book::DocsTrack;
use crate::data::{LoopNext, SUFFICIENCY_ROUNDS};
use crate::prelude::*;
use ostra_core::event::{GateAnswer, GatePayload, JudgeKind};
use ostra_core::ids::{ExecutionId, GateId};
pub use ostra_engine::pipeline::{ProjectFacts, YoloPlan};
use ostra_engine::state::{SessionState, amendment_index, parse_loop_key};
use std::fmt::Write;
use std::path::Path;

pub(crate) const FILE_EXCERPT: usize = 24_000;

/// Per research document for the Track judge, which reads every document of the session.
pub(crate) const TRACK_EXCERPT: usize = 12_000;

pub(crate) fn excerpt(path: &Path) -> String {
    excerpt_n(path, FILE_EXCERPT)
}

pub(crate) fn excerpt_n(path: &Path, max: usize) -> String {
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
            if !s.pinned.is_empty() {
                // Rule O6: the judge sees only the pinned projects, because they are the whole scope.
                let _ = writeln!(
                    m,
                    "# Projects the user pinned\n\nThe scope is exactly these: {}\n",
                    s.pinned
                        .iter()
                        .map(|k| format!("`{k}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            let _ = writeln!(m, "# Projects in this workspace\n");
            let shown = projects
                .iter()
                .filter(|p| s.pinned.is_empty() || s.project_path(&p.key).is_some());
            for p in shown {
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
                    } else if s.created_projects.iter().any(|c| c.key == p.key) {
                        " (created in this session: initialized when the build starts)"
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
            let _ = writeln!(
                m,
                "# Request\n\n{request}\n\nCategory: {}\nTrack: {}\nTests requested: {}\nDocs requested: {}\nResearch rounds already judged: {} of {SUFFICIENCY_ROUNDS}\n\n# Research returned\n",
                s.category.map(|c| c.as_str()).unwrap_or("unknown"),
                s.ext
                    .os()
                    .track
                    .map(|t| t.as_str())
                    .unwrap_or("not decided yet"),
                yes_no(s.tests_requested()),
                yes_no(s.docs_requested()),
                s.ext.os().sufficiency_rounds,
            );
            for t in &s.ext.os().explore {
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
                .ext
                .os()
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
            if let Some(spec) = &s.ext.os().spec.current {
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
            for t in &s.ext.os().explore {
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
                .and_then(|i| s.ext.os().feedback.rounds.get(i));
            let text = round.map(|r| r.text.clone()).unwrap_or_default();
            let _ = writeln!(m, "# Request\n\n{request}\n");
            let _ = writeln!(
                m,
                "Track: {}\nProjects in scope: {}\n",
                s.ext.os().track.map(|t| t.as_str()).unwrap_or("full"),
                s.scope.join(", ")
            );
            match &s.ext.os().spec.current {
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
            for p in s.ext.os().phases.values() {
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
            m.push('\n');
            research_facts(&mut m, s);
            notes_facts(&mut m, s);
            let _ = writeln!(m, "# The user's feedback on the implementation\n\n{text}");
            (m, format!("Feedback: {}", first_line(&text)))
        }
        JudgeKind::RouteAnswer if subject.and_then(amendment_index).is_some() => {
            amendment_facts(&mut m, s, &request, subject.and_then(amendment_index))
        }
        JudgeKind::RouteAnswer => {
            let gate = subject.and_then(|g| s.gates.get(&GateId::from(g)));
            let _ = writeln!(m, "# Request\n\n{request}\n");
            session_facts(&mut m, s);
            let Some(g) = gate else {
                return (m, "Answer: unknown gate".into());
            };
            let _ = writeln!(
                m,
                "# The gate the user answered\n\nKind: {}\nTitle: {}\n{}\n",
                g.payload.kind_str(),
                g.title,
                g.explanation
            );
            let mut first = String::new();
            match (&g.payload, &g.answer) {
                (
                    GatePayload::OpenQuestions {
                        artifact,
                        questions,
                        ..
                    },
                    Some(GateAnswer::Questions { answers }),
                ) => {
                    let _ = writeln!(
                        m,
                        "The {artifact} agent asked these. Give one item per question ID.\n"
                    );
                    for q in questions {
                        let _ = writeln!(
                            m,
                            "## {} {}\n\n{}, numbered as the user saw them:\n",
                            q.id,
                            q.question,
                            if q.multi_select {
                                "Choose one or more"
                            } else {
                                "Single choice, with a field for a typed answer"
                            }
                        );
                        for (n, o) in q.display_options() {
                            let _ = writeln!(
                                m,
                                "{n}. {}{}: {}",
                                o.label,
                                if n == 1 && q.options.get(q.recommended).is_some() {
                                    " (the agent's recommendation)"
                                } else {
                                    ""
                                },
                                o.description
                            );
                        }
                        let a = answers
                            .iter()
                            .find(|a| a.id == q.id)
                            .map(|a| a.answer.as_str())
                            .unwrap_or("(no answer)");
                        if first.is_empty() {
                            first = a.to_string();
                        }
                        let _ = writeln!(m, "\nThe user's answer: {a}\n");
                    }
                }
                (payload, answer) => {
                    gate_facts(&mut m, payload);
                    let (approved, text) = match answer {
                        Some(GateAnswer::Approval { approved, feedback }) => {
                            (Some(*approved), feedback.clone().unwrap_or_default())
                        }
                        Some(GateAnswer::Choice { option, text }) => {
                            let _ = writeln!(m, "The user chose: {option}\n");
                            (None, text.clone().unwrap_or_default())
                        }
                        _ => (None, String::new()),
                    };
                    if let Some(a) = approved {
                        let _ = writeln!(
                            m,
                            "The user {} it.\n",
                            if a { "approved" } else { "did not approve" }
                        );
                    }
                    let _ = writeln!(
                        m,
                        "Give one item with ID `answer`.\n\n## The user's answer\n\n{text}"
                    );
                    first = text;
                }
            }
            (m, format!("Answer: {}", first_line(&first)))
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
                    && let Some(p) = s.ext.os().phases.get(&k.0)
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
            // Rule O7: the judge sees how much of the advisor's budget this loop has used.
            if let Some(l) = rec.and_then(|r| r.loop_key).and_then(|k| s.loop_ref(k)) {
                let _ = writeln!(
                    m,
                    "\n# Advisor rounds for this loop\n\n{} of {}",
                    l.advice.len(),
                    crate::init::MAX_ADVICE
                );
                for g in &l.advice {
                    let _ = writeln!(m, "\nGuidance that did not fix it:\n{g}");
                }
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
                .and_then(|k| s.ext.os().phases.get(&k.0))
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
                if let Some(p) = s
                    .ext
                    .os()
                    .phases
                    .get(&k.0)
                    .and_then(|p| p.info.file.clone())
                {
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
                        for t in &s.ext.os().explore {
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
