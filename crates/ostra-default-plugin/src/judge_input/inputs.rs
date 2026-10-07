//! Each judge's input: the facts it decides on, as text, and the summary the log records.

use super::*;
#[allow(unused_imports)]
use crate::book::DocsTrack;
use crate::prelude::*;
use ostra_core::event::{GateAnswer, GatePayload, JudgeKind};
use ostra_core::ids::GateId;
pub use ostra_engine::pipeline::{ProjectFacts, YoloPlan};
use ostra_engine::state::{SessionState, amendment_index};
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
        JudgeKind::Classify => crate::stages::research::classify_input(s, projects),
        JudgeKind::Sufficiency => crate::stages::research::sufficiency_input(s, subject),
        JudgeKind::Stakes => crate::stages::stakes::stakes_input(s),
        JudgeKind::Track => crate::stages::track::track_input(s),
        JudgeKind::Feedback => crate::stages::feedback::feedback_input(s, subject),
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
        JudgeKind::Rescue => crate::stages::build::rescue_input(s, subject),
        JudgeKind::ResolveReview => crate::stages::build::resolve_review_input(s, subject),
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
