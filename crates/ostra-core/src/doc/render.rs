//! Markdown for the agents that read a document: the section names match the templates the
//! downstream prompts and fact-check refer to, and derived tables are computed here, not written
//! by the model.

use super::merge::natural_cmp;
use super::{Document, Evidence, Phase, PhaseDoc, PlanDoc, ResearchDoc, SpecDoc};
use crate::pipeline::Question;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

/// The markdown for the document written to `md`.
pub fn render(doc: &Document, md: &Path) -> String {
    let mut out = String::new();
    match doc {
        Document::Research(d) => research(d, &mut out),
        Document::Spec(d) => spec(d, &mut out),
        Document::Plan(d) => plan(d, md, &mut out),
        Document::Phase(d) => phase(d, &mut out),
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// A table cell: one line, pipes escaped.
fn cell(s: &str) -> String {
    s.trim().replace('\n', " ").replace('|', "\\|")
}

fn list(ids: &[String]) -> String {
    if ids.is_empty() {
        "none".into()
    } else {
        ids.join(", ")
    }
}

fn table(out: &mut String, head: &[&str], rows: impl IntoIterator<Item = Vec<String>>) {
    let _ = writeln!(out, "| {} |", head.join(" | "));
    let _ = writeln!(out, "|{}", " --- |".repeat(head.len()));
    for r in rows {
        let _ = writeln!(
            out,
            "| {} |",
            r.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | ")
        );
    }
    out.push('\n');
}

fn bullets(out: &mut String, items: &[String], empty: &str) {
    if items.is_empty() {
        let _ = writeln!(out, "{empty}\n");
        return;
    }
    for i in items {
        let _ = writeln!(out, "- {}", i.trim());
    }
    out.push('\n');
}

fn para(out: &mut String, text: &str) {
    let _ = writeln!(out, "{}\n", text.trim());
}

fn questions(out: &mut String, qs: &[Question], empty: &str) {
    if qs.is_empty() {
        let _ = writeln!(out, "{empty}\n");
        return;
    }
    for q in qs {
        let _ = writeln!(out, "### {} [{}] {}\n", q.id, q.tag, q.question.trim());
        for (i, o) in q.options.iter().enumerate() {
            let rec = if i == q.recommended {
                " (Recommended)"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "- **{}**{rec}: {}",
                o.label.trim(),
                o.description.trim()
            );
        }
        if q.multi_select {
            let _ = writeln!(out, "\nSeveral options may be chosen.");
        }
        out.push('\n');
    }
}

fn evidence_table(out: &mut String, rows: &[Evidence], empty: &str) {
    if rows.is_empty() {
        let _ = writeln!(out, "{empty}\n");
        return;
    }
    table(
        out,
        &[
            "ID",
            "Established fact",
            "Binding rule",
            "Source",
            "Version / date",
        ],
        rows.iter().map(|e| {
            let source = match &e.note {
                Some(n) => format!("`{}` ({n})", e.source),
                None => format!("`{}`", e.source),
            };
            vec![
                e.id.clone(),
                e.fact.clone(),
                e.rule.clone(),
                source,
                e.version.clone(),
            ]
        }),
    );
}

fn research(d: &ResearchDoc, out: &mut String) {
    let _ = writeln!(out, "# Research: {}", d.title.trim());
    let areas = if d.areas.is_empty() {
        "none".into()
    } else {
        d.areas.join(", ")
    };
    let _ = writeln!(
        out,
        "**Date:** {} · **Repo:** {} · **Areas:** {areas} · **Status:** Complete\n",
        d.date, d.repo
    );
    let _ = writeln!(out, "## Scope of this document\n");
    para(out, &d.scope);
    let _ = writeln!(out, "## Problem Statement\n");
    para(out, &d.problem);
    let _ = writeln!(out, "## What the request asks\n");
    bullets(out, &d.asks, "None stated.");
    let _ = writeln!(out, "## Findings\n");
    let _ = writeln!(out, "### Relevant Files\n");
    if d.files.is_empty() {
        let _ = writeln!(out, "None.\n");
    } else {
        table(
            out,
            &["File", "Purpose", "Symbols"],
            d.files.iter().map(|f| {
                let symbols = f
                    .symbols
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                vec![format!("`{}`", f.path), f.purpose.clone(), symbols]
            }),
        );
    }
    let _ = writeln!(out, "### Existing Patterns\n");
    if d.patterns.is_empty() {
        let _ = writeln!(out, "None.\n");
    }
    for p in &d.patterns {
        let _ = writeln!(out, "#### {}\n", p.name.trim());
        para(out, &p.description);
        if !p.files.is_empty() {
            let files = p
                .files
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "Used in: {files}\n");
        }
        if let Some(s) = &p.snippet {
            if let Some(src) = &s.source {
                let _ = writeln!(out, "From `{src}`:\n");
            }
            let _ = writeln!(out, "```{}\n{}\n```\n", s.language, s.code.trim_end());
        }
    }
    let _ = writeln!(out, "### Data Flow\n");
    if d.data_flow.is_empty() {
        let _ = writeln!(out, "None traced.\n");
    }
    for (i, s) in d.data_flow.iter().enumerate() {
        match &s.location {
            Some(l) => {
                let _ = writeln!(out, "{}. {} (`{l}`)", i + 1, s.step.trim());
            }
            None => {
                let _ = writeln!(out, "{}. {}", i + 1, s.step.trim());
            }
        }
    }
    if !d.data_flow.is_empty() {
        out.push('\n');
    }
    let _ = writeln!(out, "### Dependencies\n");
    if d.dependencies.is_empty() {
        let _ = writeln!(out, "None.\n");
    } else {
        table(
            out,
            &["Dependency", "Kind", "Version", "Role"],
            d.dependencies.iter().map(|x| {
                let kind = match x.kind {
                    super::DependencyKind::Internal => "internal",
                    super::DependencyKind::External => "external",
                };
                vec![
                    x.name.clone(),
                    kind.into(),
                    x.version.clone().unwrap_or_default(),
                    x.role.clone(),
                ]
            }),
        );
    }
    let _ = writeln!(out, "### External Technology\n");
    if d.external.is_empty() {
        let _ = writeln!(
            out,
            "None: the request touches nothing the repo does not already do.\n"
        );
    }
    for f in &d.external {
        let _ = writeln!(
            out,
            "- **{}**: {} Consequence: {} Source: `{}` ({})",
            f.technology.trim(),
            f.fact.trim(),
            f.consequence.trim(),
            f.source,
            f.version
        );
    }
    if !d.external.is_empty() {
        out.push('\n');
    }
    if !d.lessons.is_empty() {
        let _ = writeln!(out, "### Lessons Used\n");
        for l in &d.lessons {
            let verified = l
                .verified
                .as_deref()
                .map(|v| format!(" Verified: {v}"))
                .unwrap_or_default();
            let _ = writeln!(out, "- [{}] {}{verified}", l.area, l.lesson.trim());
        }
        out.push('\n');
    }
    let _ = writeln!(out, "## Approaches\n");
    if d.approaches.is_empty() {
        let _ = writeln!(out, "N/A: investigative only.\n");
    }
    for a in &d.approaches {
        let rec = if a.recommended { " (Recommended)" } else { "" };
        let _ = writeln!(out, "### {}{rec}\n", a.name.trim());
        let _ = writeln!(out, "- **Concept:** {}", a.concept.trim());
        let _ = writeln!(
            out,
            "- **Pros:** {}",
            if a.pros.is_empty() {
                "none".into()
            } else {
                a.pros.join("; ")
            }
        );
        let _ = writeln!(
            out,
            "- **Cons:** {}",
            if a.cons.is_empty() {
                "none".into()
            } else {
                a.cons.join("; ")
            }
        );
        let _ = writeln!(out, "- **Precedent:** {}", a.precedent.trim());
        let _ = writeln!(out, "- **Best for:** {}\n", a.best_for.trim());
    }
    let _ = writeln!(out, "### Recommendation\n");
    para(
        out,
        d.recommendation
            .as_deref()
            .unwrap_or("N/A: investigative only."),
    );
    let _ = writeln!(out, "## Open Questions\n");
    questions(out, &d.open_questions, "None");
    let _ = writeln!(out, "## Not Covered\n");
    bullets(out, &d.not_covered, "None");
    let _ = writeln!(out, "## Sources\n");
    if d.sources.is_empty() {
        let _ = writeln!(out, "None\n");
    } else {
        table(
            out,
            &["Source", "Version / date", "What it established"],
            d.sources.iter().map(|s| {
                vec![
                    format!("`{}`", s.url),
                    s.version.clone(),
                    s.established.clone(),
                ]
            }),
        );
    }
    let _ = writeln!(out, "## Next Steps\n");
    bullets(out, &d.next_steps, "None");
}

/// `R1–R4` for a contiguous run, else the list.
fn id_span(ids: &[&str]) -> String {
    match ids {
        [] => "none".into(),
        [one] => one.to_string(),
        [first, .., last] => {
            let nums: Vec<Option<u64>> = ids
                .iter()
                .map(|i| i.trim_start_matches('R').parse().ok())
                .collect();
            let contiguous = nums
                .windows(2)
                .all(|w| matches!(w, [Some(a), Some(b)] if *b == a + 1));
            if contiguous {
                format!("{first}–{last}")
            } else {
                ids.join(", ")
            }
        }
    }
}

fn spec(d: &SpecDoc, out: &mut String) {
    let covered: BTreeSet<&str> = d
        .requirements
        .iter()
        .flat_map(|r| r.covers.iter().map(String::as_str))
        .collect();
    let met = d
        .criteria
        .iter()
        .filter(|c| covered.contains(c.id.as_str()))
        .count();
    let _ = writeln!(out, "# Specification: {}\n", d.title.trim());
    let _ = writeln!(out, "**Date:** {}", d.date);
    let _ = writeln!(
        out,
        "**Research:** {}",
        if d.research.is_empty() {
            "none".into()
        } else {
            d.research.join(", ")
        }
    );
    let repos = d
        .repos
        .iter()
        .map(|r| format!("`{} -> {}`", r.key, r.root))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        out,
        "**Repos in scope:** {}",
        if repos.is_empty() {
            "none".into()
        } else {
            repos
        }
    );
    let _ = writeln!(out, "**Deliverables:** {}", d.deliverables.len());
    let _ = writeln!(out, "**Requirements:** {}", d.requirements.len());
    let _ = writeln!(out, "**Criteria covered:** {met} of {}\n", d.criteria.len());

    let _ = writeln!(out, "## Objective\n");
    para(out, &d.objective);
    let _ = writeln!(out, "## Current Behavior\n");
    para(out, &d.current_behavior);
    let _ = writeln!(out, "## Scope\n\n### In Scope\n");
    let ins: Vec<String> = d
        .in_scope
        .iter()
        .map(|s| {
            if s.criteria.is_empty() {
                s.text.clone()
            } else {
                format!("{} ({})", s.text.trim(), s.criteria.join(", "))
            }
        })
        .collect();
    bullets(out, &ins, "None.");
    let _ = writeln!(out, "### Out of Scope\n");
    let outs: Vec<String> = d
        .out_of_scope
        .iter()
        .map(|s| format!("{} Reason: {}", s.text.trim(), s.reason.trim()))
        .collect();
    bullets(out, &outs, "None.");

    let _ = writeln!(out, "## Criteria\n");
    if d.criteria.is_empty() {
        let _ = writeln!(out, "None.\n");
    } else {
        table(
            out,
            &[
                "Criterion",
                "Statement",
                "Type",
                "Repo",
                "Grounding",
                "Depends on",
                "Status",
            ],
            d.criteria.iter().map(|c| {
                let status = match &c.provisional {
                    Some(q) => format!("Provisional ({q})"),
                    None => "Confirmed".into(),
                };
                vec![
                    c.id.clone(),
                    c.statement.clone(),
                    format!("{:?}", c.kind),
                    c.repo.clone(),
                    c.grounding.clone(),
                    list(&c.depends_on),
                    status,
                ]
            }),
        );
    }

    let _ = writeln!(out, "## Delivery Order\n");
    let _ = writeln!(
        out,
        "Deliverables are built in `D{{n}}` order. `Depends on` names the deliverables whose contracts a deliverable consumes. `none` means no prerequisite.\n"
    );
    table(
        out,
        &[
            "Deliverable",
            "Title",
            "Repo",
            "Area(s)",
            "Depends on",
            "Requirements",
            "Criteria",
        ],
        d.deliverables.iter().map(|x| {
            let reqs: Vec<&str> = d
                .requirements
                .iter()
                .filter(|r| r.deliverable == x.id)
                .map(|r| r.id.as_str())
                .collect();
            let mut crit: Vec<&str> = d
                .requirements
                .iter()
                .filter(|r| r.deliverable == x.id)
                .flat_map(|r| r.covers.iter().map(String::as_str))
                .collect();
            crit.sort_by(|a, b| natural_cmp(a, b));
            crit.dedup();
            vec![
                x.id.clone(),
                x.title.clone(),
                x.repo.clone(),
                x.areas.join(", "),
                list(&x.depends_on),
                id_span(&reqs),
                crit.join(", "),
            ]
        }),
    );

    let _ = writeln!(out, "## Requirements\n");
    for x in &d.deliverables {
        let root = d
            .repos
            .iter()
            .find(|r| r.key == x.repo)
            .map(|r| r.root.as_str())
            .unwrap_or("");
        let _ = writeln!(out, "### {}: {}", x.id, x.title.trim());
        let _ = writeln!(
            out,
            "**Repo:** {} · **Repo root:** {root} · **Depends on:** {}",
            x.repo,
            list(&x.depends_on)
        );
        let _ = writeln!(out, "**Outcome:** {}\n", x.outcome.trim());
        for r in d.requirements.iter().filter(|r| r.deliverable == x.id) {
            let _ = writeln!(out, "#### {} {}", r.id, r.title.trim());
            let _ = writeln!(out, "{}", r.statement.trim());
            let _ = writeln!(out, "**Pattern:** {}", r.pattern.label());
            let _ = writeln!(out, "**Covers:** {}", list(&r.covers));
            let _ = writeln!(out, "**Rests on:** {}\n", list(&r.rests_on));
            let _ = writeln!(out, "**Acceptance Criteria**");
            for a in &r.acceptance {
                let _ = writeln!(
                    out,
                    "- **{}** GIVEN {} WHEN {} THEN {}",
                    a.id,
                    a.given.trim(),
                    a.when.trim(),
                    a.then.trim()
                );
            }
            out.push('\n');
        }
    }
    let orphans: Vec<_> = d
        .requirements
        .iter()
        .filter(|r| !d.deliverables.iter().any(|x| x.id == r.deliverable))
        .collect();
    for r in orphans {
        let _ = writeln!(
            out,
            "#### {} {} (deliverable {} is missing)\n{}\n",
            r.id,
            r.title.trim(),
            r.deliverable,
            r.statement.trim()
        );
    }

    let _ = writeln!(out, "## Contracts Provided\n");
    if d.contracts_provided.is_empty() {
        let _ = writeln!(
            out,
            "None: this spec provides no cross-boundary contract.\n"
        );
    } else {
        table(
            out,
            &["Contract", "Shape", "Provided by", "Consumed by"],
            d.contracts_provided.iter().map(|c| {
                vec![
                    c.name.clone(),
                    c.shape.clone(),
                    c.provided_by.clone(),
                    c.consumed_by.join(", "),
                ]
            }),
        );
    }
    let _ = writeln!(out, "## Contracts Consumed\n");
    if d.contracts_consumed.is_empty() {
        let _ = writeln!(out, "None: this spec consumes no existing contract.\n");
    } else {
        table(
            out,
            &["Contract", "Shape", "Source"],
            d.contracts_consumed
                .iter()
                .map(|c| vec![c.name.clone(), c.shape.clone(), format!("`{}`", c.source)]),
        );
    }
    let _ = writeln!(out, "## External Evidence\n");
    evidence_table(
        out,
        &d.evidence,
        "None: this spec rests on no technology outside the repo.",
    );
    let _ = writeln!(out, "## Data Impact\n");
    let changes: Vec<String> = d
        .data_impact
        .iter()
        .map(|c| match &c.deliverable {
            Some(x) => format!("{x}: {}", c.change.trim()),
            None => c.change.trim().to_string(),
        })
        .collect();
    bullets(out, &changes, "None: this spec changes no persisted data.");
    let _ = writeln!(out, "## Assumptions\n");
    let assumptions: Vec<String> = d
        .assumptions
        .iter()
        .map(|a| format!("{} (source: {})", a.text.trim(), a.source.trim()))
        .collect();
    bullets(out, &assumptions, "None");
    let _ = writeln!(out, "## Open Questions\n");
    questions(
        out,
        &d.open_questions,
        "None: every requirement is resolved from the research documents or the codebase.",
    );
    let _ = writeln!(out, "## Traceability\n");
    table(
        out,
        &[
            "Criterion",
            "Deliverable",
            "Requirements",
            "Acceptance Criteria",
        ],
        d.criteria.iter().map(|c| {
            let reqs: Vec<_> = d
                .requirements
                .iter()
                .filter(|r| r.covers.contains(&c.id))
                .collect();
            let mut dels: Vec<&str> = reqs.iter().map(|r| r.deliverable.as_str()).collect();
            dels.dedup();
            let acs: Vec<&str> = reqs
                .iter()
                .flat_map(|r| r.acceptance.iter().map(|a| a.id.as_str()))
                .collect();
            let names: Vec<&str> = reqs.iter().map(|r| r.id.as_str()).collect();
            let or_none = |v: Vec<&str>| {
                if v.is_empty() {
                    "none".to_string()
                } else {
                    v.join(", ")
                }
            };
            vec![c.id.clone(), or_none(dels), or_none(names), or_none(acs)]
        }),
    );
    let _ = writeln!(out, "## Notes\n");
    bullets(out, &d.notes, "None");
}

fn plan(d: &PlanDoc, md: &Path, out: &mut String) {
    let steps: usize = d.phases.iter().map(|p| p.steps.len()).sum();
    let mut delivered: Vec<&str> = d
        .phases
        .iter()
        .flat_map(|p| {
            p.steps
                .iter()
                .flat_map(|s| s.delivers.iter().map(String::as_str))
        })
        .collect();
    delivered.sort_by(|a, b| natural_cmp(a, b));
    delivered.dedup();
    let _ = writeln!(out, "# Plan: {}\n", d.title.trim());
    let _ = writeln!(out, "**Date:** {}", d.date);
    let _ = writeln!(out, "**Spec:** {}", d.spec);
    let _ = writeln!(out, "**Delivers requirements:** {}", id_span(&delivered));
    let repos = d
        .repos
        .iter()
        .map(|r| format!("`{} -> {}`", r.key, r.root))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        out,
        "**Repos in scope:** {}",
        if repos.is_empty() {
            "none".into()
        } else {
            repos
        }
    );
    let _ = writeln!(out, "**Stakes:** {}", d.stakes.as_str());
    let _ = writeln!(out, "**Stakes Rationale:** {}\n", d.stakes_rationale.trim());
    let _ = writeln!(out, "## Summary\n");
    para(out, &d.summary);
    let _ = writeln!(out, "## Success Criteria\n");
    for c in &d.success_criteria {
        match &c.id {
            Some(id) => {
                let _ = writeln!(out, "- [ ] **{id}**: {}", c.text.trim());
            }
            None => {
                let _ = writeln!(out, "- [ ] {}", c.text.trim());
            }
        }
    }
    out.push('\n');
    let _ = writeln!(out, "## Clarifying Questions\n");
    questions(
        out,
        &d.clarifying_questions,
        "None: the spec resolves every category.",
    );

    let _ = writeln!(out, "## Deliverable Index\n");
    let mut order: Vec<&str> = d.deliverables.iter().map(|x| x.id.as_str()).collect();
    for p in &d.phases {
        if !order.contains(&p.deliverable.as_str()) {
            order.push(p.deliverable.as_str());
        }
    }
    table(
        out,
        &["Deliverable", "Title", "Repo", "Phases", "Requirements"],
        order.iter().map(|id| {
            let phases: Vec<&Phase> = d.phases.iter().filter(|p| p.deliverable == *id).collect();
            let title = d
                .deliverables
                .iter()
                .find(|x| x.id == *id)
                .map(|x| x.title.clone())
                .unwrap_or_default();
            let mut repos: Vec<&str> = phases.iter().map(|p| p.repo.as_str()).collect();
            repos.dedup();
            let mut reqs: Vec<&str> = phases
                .iter()
                .flat_map(|p| {
                    p.steps
                        .iter()
                        .flat_map(|s| s.delivers.iter().map(String::as_str))
                })
                .collect();
            reqs.sort_by(|a, b| natural_cmp(a, b));
            reqs.dedup();
            let ids = phases
                .iter()
                .map(|p| p.id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            vec![id.to_string(), title, repos.join(", "), ids, id_span(&reqs)]
        }),
    );

    let _ = writeln!(out, "## Phase Index\n");
    let _ = writeln!(
        out,
        "The **Repo** and **Depends on** columns are the orchestrator's scheduling graph. **Complexity** is the model-routing tier (P9). **Test policy** decides which phases a requested test run covers (P12).\n"
    );
    table(
        out,
        &[
            "Phase",
            "Name",
            "Deliverable",
            "Repo",
            "Complexity",
            "Test policy",
            "Depends on",
            "File Path",
            "Steps",
            "Description",
        ],
        d.phases.iter().map(|p| {
            let deps = if p.depends_on.is_empty() {
                "none".into()
            } else {
                p.depends_on
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            vec![
                p.id.to_string(),
                p.name.clone(),
                p.deliverable.clone(),
                p.repo.clone(),
                p.complexity.as_str().into(),
                p.test_policy.as_str().into(),
                deps,
                format!("`{}`", super::store::phase_path(md, p.id).display()),
                p.steps.len().to_string(),
                p.description.clone(),
            ]
        }),
    );

    let _ = writeln!(out, "## Test Policy Rationale\n");
    let skipped: Vec<&Phase> = d
        .phases
        .iter()
        .filter(|p| p.test_policy == super::TestPolicy::Skip)
        .collect();
    if skipped.is_empty() {
        let _ = writeln!(out, "None: every phase is Test policy Required.\n");
    } else {
        table(
            out,
            &["Phase", "Rationale"],
            skipped
                .iter()
                .map(|p| vec![p.id.to_string(), p.test_rationale.clone()]),
        );
    }

    let _ = writeln!(out, "## Requirement Traceability\n");
    let mut by_req: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in &d.phases {
        for s in &p.steps {
            for r in &s.delivers {
                by_req
                    .entry(r.clone())
                    .or_default()
                    .push(format!("phase {} step {}", p.id, s.id));
            }
        }
    }
    let mut reqs: Vec<&String> = by_req.keys().collect();
    reqs.sort_by(|a, b| natural_cmp(a, b));
    table(
        out,
        &[
            "Requirement",
            "Deliverable",
            "Delivered by",
            "Acceptance Criteria",
        ],
        reqs.into_iter().map(|r| {
            let n = r.trim_start_matches('R');
            let deliverable = d
                .phases
                .iter()
                .find(|p| p.steps.iter().any(|s| s.delivers.contains(r)))
                .map(|p| p.deliverable.clone())
                .unwrap_or_default();
            let acs: Vec<String> = d
                .success_criteria
                .iter()
                .filter_map(|c| c.id.clone())
                .filter(|id| {
                    id.strip_prefix("AC")
                        .and_then(|x| x.split_once('.'))
                        .is_some_and(|(rn, _)| rn == n)
                })
                .collect();
            vec![
                r.clone(),
                deliverable,
                by_req[r].join(", "),
                if acs.is_empty() {
                    "none".into()
                } else {
                    acs.join(", ")
                },
            ]
        }),
    );

    let _ = writeln!(out, "## Risks and Mitigations\n");
    if d.risks.is_empty() {
        let _ = writeln!(out, "None.\n");
    } else {
        table(
            out,
            &["Risk", "Impact", "Likelihood", "Mitigation"],
            d.risks.iter().map(|r| {
                vec![
                    r.risk.clone(),
                    r.impact.clone(),
                    r.likelihood.as_str().into(),
                    r.mitigation.clone(),
                ]
            }),
        );
    }
    let _ = writeln!(out, "## Verification Strategy\n");
    bullets(
        out,
        &d.verification,
        "- **Per-step / per-phase:** the phase's repo's `build` command.",
    );
    let _ = writeln!(out, "## Step Count Summary\n");
    let _ = writeln!(out, "- Total phases: {}", d.phases.len());
    let _ = writeln!(out, "- Total steps: {steps}");
    let _ = writeln!(out, "- Estimated complexity: {}\n", d.stakes.as_str());
    if !d.pre_checks.is_empty() {
        let _ = writeln!(out, "## Mechanical Pre-Checks\n");
        table(
            out,
            &["Check", "Scope", "Result"],
            d.pre_checks
                .iter()
                .map(|c| vec![c.check.clone(), c.scope.clone(), c.result.clone()]),
        );
    }
}

fn phase(d: &PhaseDoc, out: &mut String) {
    let p = &d.phase;
    let _ = writeln!(out, "# Phase {}: {}\n", p.id, p.name.trim());
    let _ = writeln!(out, "**Phase ID:** {}", p.id);
    let _ = writeln!(out, "**Plan:** {}", d.plan.trim());
    let _ = writeln!(out, "**Date:** {}", d.date);
    let _ = writeln!(out, "**Spec:** {}", d.spec);
    match &d.deliverable_title {
        Some(t) => {
            let _ = writeln!(out, "**Deliverable:** {}: {}", p.deliverable, t.trim());
        }
        None => {
            let _ = writeln!(out, "**Deliverable:** {}", p.deliverable);
        }
    }
    let _ = writeln!(out, "**Repo:** {}", p.repo);
    let _ = writeln!(out, "**Repo root:** {}", p.repo_root);
    let deps = if p.depends_on.is_empty() {
        "none".into()
    } else {
        p.depends_on
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(out, "**Depends on:** {deps}");
    let _ = writeln!(out, "**Complexity:** {}", p.complexity.as_str());
    let _ = writeln!(
        out,
        "**Test policy:** {}: {} Covered only if the user requests tests, after every phase is implemented.",
        p.test_policy.as_str(),
        p.test_rationale.trim()
    );
    let _ = writeln!(
        out,
        "**Area(s):** {}\n",
        if p.areas.is_empty() {
            "none".into()
        } else {
            p.areas.join(", ")
        }
    );
    let _ = writeln!(out, "## Required Skills\n");
    let _ = writeln!(
        out,
        "Load these before starting (the always-on convention skill is auto-loaded and is not listed):\n"
    );
    let skills: Vec<String> = p.skills.iter().map(|s| format!("`{s}`")).collect();
    bullets(out, &skills, "None.");
    let _ = writeln!(out, "## Context\n");
    para(out, &p.context);
    let _ = writeln!(out, "## Requirements Delivered\n");
    table(
        out,
        &["ID", "Statement"],
        p.requirements
            .iter()
            .map(|r| vec![r.id.clone(), r.statement.clone()]),
    );
    let _ = writeln!(out, "## External Constraints\n");
    evidence_table(
        out,
        &p.constraints,
        "None: no step in this phase depends on a technology outside the repo.",
    );
    let _ = writeln!(out, "## Steps\n");
    for s in &p.steps {
        let _ = writeln!(out, "#### Step {}: {}\n", s.id, s.title.trim());
        let _ = writeln!(out, "- **File**: `{}` ({:?})", s.file, s.change);
        let read = if s.read_first.is_empty() {
            "none".into()
        } else {
            s.read_first
                .iter()
                .map(|r| format!("`{r}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(out, "- **Read first**: {read}");
        let _ = writeln!(out, "- **Delivers**: {}", list(&s.delivers));
        let _ = writeln!(out, "- **Action**: {}", s.action.trim());
        let rules = if s.binding_rules.is_empty() {
            "none".into()
        } else {
            s.binding_rules
                .iter()
                .map(|b| format!("{}: {}", b.id, b.rule.trim()))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let _ = writeln!(out, "- **Binding rules**: {rules}");
        let skills = if s.skills.is_empty() {
            "none".into()
        } else {
            s.skills
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(out, "- **Skills**: {skills}");
        let _ = writeln!(out, "- **Verify**: `{}`", s.verify.trim());
        let _ = writeln!(out, "- **Complexity**: {:?}\n", s.size);
    }
    let _ = writeln!(
        out,
        "## Phase Verification\n\n```bash\n{}\n```",
        p.verification.trim()
    );
}
