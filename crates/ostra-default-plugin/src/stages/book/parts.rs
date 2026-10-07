//! Rule B11: the parts of a session-wide book: the check of a documentation submit against the
//! parts the run names, and the `@<project>/<path>` form of a source.

use super::checks::check_documentation;
use ostra_core::book::{CROSS_PART, DocsStep, DocumentationSubmit};
use ostra_core::submit::SubmitStatus;
use std::collections::BTreeSet;

/// Rule B11: the checks of a documentation submit with the run's params: the shape of each step,
/// and in a session-wide pipeline (`Book parts:`), the part of each page, the overview of each
/// part, and the project of each code reference.
pub fn check_with_params(v: &serde_json::Value, params: &serde_json::Value) -> Vec<String> {
    let s = match serde_json::from_value::<DocumentationSubmit>(v.clone()) {
        Ok(s) => s,
        Err(e) => return vec![e.to_string()],
    };
    let mut issues = check_documentation(&s);
    let mut parts: Vec<String> = params
        .get("book_parts")
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();
    if parts.is_empty() || s.status != SubmitStatus::Ok {
        return issues;
    }
    // Every session-wide book has the part across projects. Runs from before this listed it.
    if !parts.iter().any(|p| p == CROSS_PART) {
        parts.push(CROSS_PART.into());
    }
    let projects: Vec<&str> = parts
        .iter()
        .map(String::as_str)
        .filter(|p| *p != CROSS_PART)
        .collect();
    let list = parts.join(", ");
    match s.step {
        DocsStep::Survey => {
            let mut used: BTreeSet<String> = BTreeSet::new();
            for p in &s.pages {
                match &p.part {
                    Some(part) if parts.contains(part) => {
                        used.insert(part.clone());
                    }
                    Some(part) => issues.push(format!(
                        "Give page `{}` one of the book parts as its `part` ({list}): `{part}` is not a part.",
                        p.id
                    )),
                    None if projects.len() > 1 => issues.push(format!(
                        "Give page `{}` a `part`: one of {list}.",
                        p.id
                    )),
                    None => {
                        used.insert(projects.first().copied().unwrap_or_default().to_string());
                    }
                }
            }
            for o in &s.part_overviews {
                if !parts.contains(&o.part) {
                    issues.push(format!(
                        "Write `part_overviews` only for the book parts ({list}): `{}` is not a part.",
                        o.part
                    ));
                }
            }
            if projects.len() > 1 {
                for part in &used {
                    if !s
                        .part_overviews
                        .iter()
                        .any(|o| &o.part == part && !o.overview.trim().is_empty())
                    {
                        issues.push(format!(
                            "Write the overview of part `{part}` in `part_overviews`, because the book shows it above the part's pages."
                        ));
                    }
                }
            }
        }
        DocsStep::Page => {
            let part = params
                .pointer("/mode/Page/part")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            for sec in &s.sections {
                for r in &sec.code_refs {
                    match r.project.as_deref() {
                        Some(p) if !projects.contains(&p) => issues.push(format!(
                            "Set the `project` of code reference `{}` to a documented project ({}): `{p}` is not one.",
                            r.path,
                            projects.join(", ")
                        )),
                        None if part == CROSS_PART => issues.push(format!(
                            "Set the `project` of code reference `{}`, because a page across projects has no project of its own.",
                            r.path
                        )),
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    issues
}

/// Rule B11: a path written `@<project>/<path>` as its project and its path. A path without the
/// tag has no project.
pub(crate) fn tagged(p: &str) -> (Option<&str>, &str) {
    match p.trim().strip_prefix('@').and_then(|r| r.split_once('/')) {
        Some((key, rest)) => (Some(key), rest),
        None => (None, p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(id: &str, rewrite: bool) -> serde_json::Value {
        serde_json::json!({"id": id, "title": id, "group": "How it works", "covers": "It.", "rewrite": rewrite})
    }

    /// Rule B11: a session-wide survey gives each page a part and each part an overview, and a page
    /// across projects names the project of each code reference.
    #[test]
    fn b1_a_session_wide_submit_is_checked_against_the_book_parts() {
        let params = serde_json::json!({"book_parts": ["a", "b", "_cross"]});
        let wide = |pages: serde_json::Value, overviews: serde_json::Value| {
            serde_json::json!({
                "status": "ok", "step": "survey", "summary": "s", "overview": "The book.",
                "part_overviews": overviews, "pages": pages,
                "inventory": [{"id": "x", "name": "x", "sources": ["@a/src/"], "owner": "flow"}]
            })
        };
        let page = |id: &str, part: Option<&str>| {
            let mut p = planned(id, true);
            if let Some(part) = part {
                p["part"] = serde_json::json!(part);
            }
            p
        };
        let ok = wide(
            serde_json::json!([page("flow", Some("_cross")), page("orders", Some("a"))]),
            serde_json::json!([{"part": "_cross", "overview": "How b calls a."}, {"part": "a", "overview": "a."}]),
        );
        assert!(
            check_with_params(&ok, &params).is_empty(),
            "{:?}",
            check_with_params(&ok, &params)
        );
        // The spawn lists only the projects, and the part across projects is always there.
        let projects_only = serde_json::json!({"book_parts": ["a", "b"]});
        assert!(check_with_params(&ok, &projects_only).is_empty());
        let bad = wide(
            serde_json::json!([
                page("flow", Some("c")),
                page("orders", None),
                page("screens", Some("b"))
            ]),
            serde_json::json!([{"part": "z", "overview": "?"}]),
        );
        let issues = check_with_params(&bad, &params);
        for start in [
            "Give page `flow` one of the book parts",
            "Give page `orders` a `part`",
            "Write `part_overviews` only for the book parts",
            "Write the overview of part `b`",
        ] {
            assert!(
                issues.iter().any(|i| i.starts_with(start)),
                "{start}: {issues:?}"
            );
        }
        assert!(
            check_with_params(&bad, &serde_json::Value::Null)
                .iter()
                .all(|i| !i.contains("part")),
            "one project's own pipeline has no parts"
        );
        let page_step = |refs: serde_json::Value| {
            serde_json::json!({
                "status": "ok", "step": "page", "summary": "s",
                "sections": [{"id": "flow", "title": "Flow", "summary": "S.", "body": "Text.", "code_refs": refs}]
            })
        };
        let cross = serde_json::json!({"book_parts": ["a", "b", "_cross"], "mode": {"Page": {"part": "_cross"}}});
        let issues = check_with_params(
            &page_step(serde_json::json!([
                {"path": "src/api.rs", "note": "n"},
                {"project": "c", "path": "src/x.rs", "note": "n"},
                {"project": "b", "path": "ui/client.ts", "note": "n"}
            ])),
            &cross,
        );
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues[0].starts_with("Set the `project` of code reference `src/api.rs`"));
        assert!(issues[1].contains("`c` is not one"));
    }
}
