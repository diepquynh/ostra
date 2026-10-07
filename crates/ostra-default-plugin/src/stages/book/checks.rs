//! Rule B10: the docs stage's own checks: each submit's step at submit time, and the engine's
//! checks of the drafts against the definition of done.

use super::MAX_DOCS_PAGES;
use super::parts::tagged;
use ostra_core::book::{
    DocSection, DocsModule, DocsStep, DocumentationSubmit, InventoryItem, RefItem, check_glossary,
    check_pages, is_page_id,
};
use ostra_core::submit::SubmitStatus;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

/// The check of the `documentation` contract that `install` hands to the core.
pub fn check_value(v: &serde_json::Value) -> Vec<String> {
    match serde_json::from_value::<DocumentationSubmit>(v.clone()) {
        Ok(s) => check_documentation(&s),
        Err(e) => vec![e.to_string()],
    }
}

/// Validate a documentation submit. Each issue states the correction first.
pub fn check_documentation(s: &DocumentationSubmit) -> Vec<String> {
    let mut issues = vec![];
    if s.status != SubmitStatus::Ok {
        return issues;
    }
    match s.step {
        DocsStep::Survey => {
            check_survey(s, &mut issues);
            return issues;
        }
        DocsStep::Synthesis => {
            check_synthesis(s, &mut issues);
            return issues;
        }
        DocsStep::Page if s.sections.len() != 1 => issues.push(format!(
            "Return the one page of your step in `sections`: it holds {}.",
            s.sections.len()
        )),
        _ => {}
    }
    if s.sections.is_empty() {
        issues.push("Return at least one section in `sections`, because the book shows only what you submit.".into());
    }
    issues.extend(check_pages(&s.sections, &s.glossary));
    issues
}

/// Rule B10: a survey plans the pages and the inventory, and writes no page.
fn check_survey(s: &DocumentationSubmit, issues: &mut Vec<String>) {
    if !s.sections.is_empty() {
        issues.push(
            "Return no `sections` in the survey step: one writer per page writes them.".into(),
        );
    }
    if s.overview.trim().is_empty() {
        issues.push(
            "Write the project `overview` in the survey, because no page writer writes it.".into(),
        );
    }
    if s.pages.is_empty() || s.pages.len() > MAX_DOCS_PAGES {
        issues.push(format!(
            "Plan 1 to {MAX_DOCS_PAGES} pages: the plan has {}. Merge pages that a reader looks for together into one broad page.",
            s.pages.len()
        ));
    }
    let mut ids = BTreeSet::new();
    for p in &s.pages {
        if !is_page_id(&p.id) {
            issues.push(format!(
                "Write the page ID \"{}\" as a lowercase slug of letters, digits, and dashes.",
                p.id
            ));
        } else if !ids.insert(p.id.as_str()) {
            issues.push(format!(
                "Plan page \"{}\" once: the ID is used twice.",
                p.id
            ));
        }
        if p.title.trim().is_empty() || p.group.trim().is_empty() || p.covers.trim().is_empty() {
            issues.push(format!(
                "Give page `{}` a title, a group, and what it covers.",
                p.id
            ));
        }
    }
    if s.inventory.is_empty() {
        issues.push("List what the book must cover in `inventory`, because the synthesis checks every page against it.".into());
    }
    let mut items = BTreeSet::new();
    for item in &s.inventory {
        if !items.insert(item.id.as_str()) {
            issues.push(format!(
                "List inventory item \"{}\" once: the ID is used twice.",
                item.id
            ));
        }
        if item.out_of_scope.is_none() && !ids.contains(item.owner.as_str()) {
            issues.push(format!(
                "Give inventory item `{}` the ID of its owning page, or an `out_of_scope` reason: `{}` is not a planned page.",
                item.id, item.owner
            ));
        }
    }
    check_glossary(&s.glossary, issues);
}

/// Rule B10: a synthesis pass judges every check of the definition of done and lists the edits.
fn check_synthesis(s: &DocumentationSubmit, issues: &mut Vec<String>) {
    if !s.sections.is_empty() || !s.pages.is_empty() {
        issues.push("Return only `checks`, `edits`, and `done` in the synthesis step: the page writers apply the edits.".into());
    }
    if s.checks.is_empty() {
        issues.push("Judge each check of the definition of done in `checks`.".into());
    }
    if s.done && (!s.edits.is_empty() || s.checks.iter().any(|c| !c.passed)) {
        issues.push("Set `done` only when every check passed and no page needs an edit.".into());
    }
    if !s.done && s.edits.is_empty() {
        issues.push("List the `edits` that close each failed check, or set `done`.".into());
    }
    for e in &s.edits {
        if e.instructions.iter().all(|i| i.trim().is_empty()) {
            issues.push(format!(
                "Give the edit of page `{}` at least one instruction.",
                e.page
            ));
        }
    }
}

/// Rule B10: the checks of the definition of done that the engine makes itself, by page, with
/// `kept` the IDs of the pages kept from the book: links to a page that does not exist, words and
/// punctuation the writing standard forbids, a page with code references and no excerpt, settings
/// outside a `For the user` block, a second explanation of an owned item, and, under the empty
/// key, inventory items that no page owns.
pub fn mechanical_issues(
    pages: &[DocSection],
    kept: &[String],
    inventory: &[InventoryItem],
    modules: &[DocsModule],
) -> BTreeMap<String, Vec<String>> {
    static LINK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\]\(([a-z0-9][a-z0-9-]*)\.md(#[^)]*)?\)").expect("valid regex")
    });
    static WORD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(would|should|might|e\.g\.|i\.e\.|etc\.)").expect("valid regex")
    });
    // A page the survey keeps from the book has no draft but stays a page.
    let ids: BTreeSet<&str> = pages
        .iter()
        .map(|p| p.id.as_str())
        .chain(kept.iter().map(String::as_str))
        .collect();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for page in pages {
        let mut issues = vec![];
        for c in LINK.captures_iter(&page.body) {
            if !ids.contains(&c[1]) {
                issues.push(format!(
                    "Link to an existing page: `{}.md` names no page of the book.",
                    &c[1]
                ));
            }
        }
        let text = prose(&page.body);
        let mut words: Vec<String> = WORD
            .find_iter(&text)
            .map(|m| m.as_str().to_string())
            .collect();
        words.sort();
        words.dedup();
        if !words.is_empty() {
            issues.push(format!(
                "Rewrite the sentences with {}: the writing standard does not allow them.",
                words
                    .iter()
                    .map(|w| format!("\"{w}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if text.contains(';') || text.contains('\u{2014}') {
            issues.push("Split the sentences that use a semicolon or an em dash.".into());
        }
        // A revision that copies its rendered draft back repeats what Ostra adds around the body.
        if page.body.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with("Project: `") || l.starts_with("Part: across projects")
        }) || (!page.code_refs.is_empty()
            && page.body.lines().any(|l| l.trim() == "## Code references"))
        {
            issues.push("Remove the `Project:` line and the `## Code references` table from the body: Ostra adds the title, the project, the summary, and the code references around it.".into());
        }
        // Rule B10: a page that shows code holds an excerpt, not only a list of files.
        if !page.code_refs.is_empty() && !has_excerpt(&page.body) {
            issues.push("Add 1 or 2 code excerpts, each at most 15 lines with `...` for the parts you leave out, that show a behavior better than a sentence does, such as a guard, a formula, or a state change.".into());
        }
        if !issues.is_empty() {
            out.insert(page.id.clone(), issues);
        }
    }
    owner_issues(pages, inventory, &ids, &mut out);
    for m in modules {
        if !inventory.iter().any(|i| covers(i, m)) {
            out.entry(String::new()).or_default().push(format!(
                "Add an inventory item for module `{}` ({}) with an owning page, or one with an `out_of_scope` reason: no inventory item covers it.",
                m.name,
                m.globs.join(", ")
            ));
        }
    }
    for item in inventory {
        if item.out_of_scope.is_none() && !ids.contains(item.owner.as_str()) {
            out.entry(String::new()).or_default().push(format!(
                "Give inventory item `{}` ({}) an owning page: `{}` is not a page.",
                item.id, item.name, item.owner
            ));
        }
    }
    out
}

/// Rule B10: the owner page of an item states each of its settings in a `For the user` block,
/// and another page that names the item links to the owner and does not explain it again.
fn owner_issues(
    pages: &[DocSection],
    inventory: &[InventoryItem],
    ids: &BTreeSet<&str>,
    out: &mut BTreeMap<String, Vec<String>>,
) {
    let mut push = |page: &str, issue: String| {
        out.entry(page.to_string()).or_default().push(issue);
    };
    for item in inventory.iter().filter(|i| i.out_of_scope.is_none()) {
        if let Some(owner) = pages.iter().find(|p| p.id == item.owner) {
            let blocks = user_blocks(&owner.body);
            let missing: Vec<String> = item
                .settings
                .iter()
                .filter(|k| !mentions(&blocks, k, true))
                .map(|k| format!("`{k}`"))
                .collect();
            if !missing.is_empty() {
                push(
                    &owner.id,
                    format!(
                        "Add a `### For the user` block to the part that explains {} and list {} in it: for each setting, the default with its unit, where it is stored, when a change takes effect, the screen, command, route, or log that shows it, and how a user stops or changes the behavior.",
                        item.name,
                        missing.join(", ")
                    ),
                );
            }
        }
        if !ids.contains(item.owner.as_str()) {
            continue;
        }
        // A name that most pages use is shared vocabulary, not a sign of a second explanation.
        let names: Vec<&String> = item
            .names
            .iter()
            .chain(&item.settings)
            .filter(|n| {
                let on = pages
                    .iter()
                    .filter(|p| in_code(&prose_with_code(&p.body), n))
                    .count();
                !(pages.len() > 3 && on * 2 > pages.len())
            })
            .collect();
        if names.is_empty() {
            continue;
        }
        let link = format!("]({}.md", item.owner);
        for page in pages.iter().filter(|p| p.id != item.owner) {
            let mut unlinked = vec![];
            let mut paragraphs = 0;
            for (heading, text) in parts(&page.body) {
                let named = names.iter().any(|n| in_code(&text, n));
                if named && !text.contains(&link) {
                    unlinked.push(if heading.is_empty() {
                        "the introduction".to_string()
                    } else {
                        format!("`{heading}`")
                    });
                }
                paragraphs += text
                    .split("\n\n")
                    .filter(|para| names.iter().any(|n| in_code(para, n)))
                    .count();
            }
            let shown = names
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ");
            // A `Reference` page is a catalog: it lists the values, and the link names the owner.
            if paragraphs >= 3 && !page.group.eq_ignore_ascii_case("reference") {
                push(
                    &page.id,
                    format!(
                        "Cut what this page says about {} to one sentence where the page needs it, and link to `{}.md`: that page owns it, and this page names {shown} in {paragraphs} paragraphs.",
                        item.name, item.owner
                    ),
                );
            } else if !unlinked.is_empty() {
                push(
                    &page.id,
                    format!(
                        "Link to `{}.md` in {} where the part names {shown}: that page owns {}.",
                        item.owner,
                        unlinked.join(", "),
                        item.name
                    ),
                );
            }
        }
    }
}

/// The `##` parts of a body, as (heading, text) with code blocks removed; the text before the
/// first `##` heading has an empty heading.
fn parts(body: &str) -> Vec<(String, String)> {
    let mut out = vec![(String::new(), String::new())];
    let mut fence = false;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        if let Some(h) = t.strip_prefix("## ") {
            out.push((h.trim().to_string(), String::new()));
            continue;
        }
        let last = out.last_mut().expect("one part");
        last.1.push_str(line);
        last.1.push('\n');
    }
    out
}

/// A body without its code blocks.
fn prose_with_code(body: &str) -> String {
    parts(body).into_iter().map(|(_, t)| t).collect()
}

/// The text of every block under a `For the user` heading, up to the next heading of the same
/// or a higher level.
fn user_blocks(body: &str) -> String {
    let mut out = String::new();
    let mut level: Option<usize> = None;
    let mut fence = false;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
        }
        let hashes = t.chars().take_while(|c| *c == '#').count();
        if !fence && hashes > 0 && t[hashes..].starts_with(' ') {
            let title = t[hashes..].trim();
            if level.is_some_and(|l| hashes <= l) {
                level = None;
            }
            if title.eq_ignore_ascii_case("for the user") {
                level = Some(hashes);
                continue;
            }
        }
        if level.is_some() {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The text names `name` as a whole word. With `short`, a dotted setting key also matches by its
/// last segment, because a page that shows a `[limits]` table writes the key without the table.
fn mentions(text: &str, name: &str, short: bool) -> bool {
    let name = name.trim().trim_matches('`');
    if name.is_empty() {
        return false;
    }
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let found = |n: &str| {
        text.match_indices(n).any(|(i, _)| {
            !text[..i].chars().next_back().is_some_and(ident)
                && !text[i + n.len()..].chars().next().is_some_and(ident)
        })
    };
    found(name)
        || (short
            && name
                .rsplit_once('.')
                .is_some_and(|(_, last)| last.contains('_') && found(last)))
}

/// An inline code span of the text names `name`, because a page writes a code name in
/// backticks and a setting such as `name` is also an English word.
fn in_code(text: &str, name: &str) -> bool {
    static SPAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").expect("valid regex"));
    SPAN.captures_iter(text)
        .any(|c| mentions(&c[1], name, false))
}

/// The body holds a fenced code block that is not a diagram.
fn has_excerpt(body: &str) -> bool {
    let mut fence = false;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            if !fence
                && !t
                    .trim_start_matches(['`', '~'])
                    .trim()
                    .starts_with("mermaid")
            {
                return true;
            }
            fence = !fence;
        }
    }
    false
}

/// The folder a glob or a source path names, without `./`, wildcards, and the trailing `/`.
fn path_prefix(p: &str) -> &str {
    let p = p.trim().trim_start_matches("./");
    let p = &p[..p.find(['*', '?', '{', '[']).unwrap_or(p.len())];
    p.trim_end_matches('/')
}

/// Rule B10: an inventory item covers a module when one of its sources lies inside the module,
/// or the module lies inside one of its sources. Rule B11: a source that names a project covers
/// only that project's modules.
fn covers(item: &InventoryItem, m: &DocsModule) -> bool {
    m.globs.iter().map(|g| tagged(g)).any(|(mk, mg)| {
        let mp = path_prefix(mg);
        item.sources.iter().map(|s| tagged(s)).any(|(sk, ss)| {
            let sp = path_prefix(ss);
            !(mk.is_some() && sk.is_some() && mk != sk)
                && !sp.is_empty()
                && (mp.is_empty()
                    || sp == mp
                    || sp.starts_with(&format!("{mp}/"))
                    || mp.starts_with(&format!("{sp}/")))
        })
    })
}

/// Rule B10: the named constants that no page mentions by name.
pub fn unmentioned_refs<'a>(pages: &[DocSection], refs: &'a [RefItem]) -> Vec<&'a RefItem> {
    let text: String = pages
        .iter()
        .map(|p| p.body.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    refs.iter().filter(|r| !text.contains(&r.name)).collect()
}

/// The prose of a body: no code blocks, inline code, or link targets.
fn prose(body: &str) -> String {
    static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`]*`").expect("valid regex"));
    static TARGET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\]\([^)]*\)").expect("valid regex"));
    let mut out = String::new();
    let mut fence = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if !fence {
            out.push_str(&TARGET.replace_all(&CODE.replace_all(line, ""), "]"));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn survey(pages: serde_json::Value, inventory: serde_json::Value) -> DocumentationSubmit {
        serde_json::from_value(serde_json::json!({
            "status": "ok", "step": "survey", "summary": "s", "overview": "The project.",
            "pages": pages, "inventory": inventory
        }))
        .unwrap()
    }

    fn planned(id: &str, rewrite: bool) -> serde_json::Value {
        serde_json::json!({"id": id, "title": id, "group": "How it works", "covers": "It.", "rewrite": rewrite})
    }

    fn item(id: &str, owner: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "name": id, "sources": ["src/"], "owner": owner})
    }

    #[test]
    fn b10_a_survey_is_checked() {
        let ok = survey(
            serde_json::json!([planned("executors", true), planned("security", true)]),
            serde_json::json!([item("native", "executors"), {"id": "ui", "name": "UI", "sources": ["web/"], "out_of_scope": "Another book."}]),
        );
        assert!(
            check_documentation(&ok).is_empty(),
            "{:?}",
            check_documentation(&ok)
        );
        let mut bad = survey(
            serde_json::json!([planned("executors", true), planned("executors", true)]),
            serde_json::json!([item("native", "nowhere")]),
        );
        bad.overview.clear();
        let issues = check_documentation(&bad);
        for start in [
            "Write the project `overview`",
            "Plan page \"executors\" once",
            "Give inventory item `native`",
        ] {
            assert!(
                issues.iter().any(|i| i.starts_with(start)),
                "{start}: {issues:?}"
            );
        }
        let many: Vec<_> = (0..31).map(|i| planned(&format!("p{i}"), true)).collect();
        let issues = check_documentation(&survey(
            serde_json::json!(many),
            serde_json::json!([item("a", "p0")]),
        ));
        assert!(
            issues.iter().any(|i| i.starts_with("Plan 1 to 30 pages")),
            "{issues:?}"
        );
    }

    /// Rule B10: the submit tool's check of `documentation` is the docs stage's, through the
    /// pipeline, and the core checks only the shape and the book format.
    #[test]
    fn b10_the_submit_check_is_the_pipelines() {
        use ostra_engine::pipeline::Pipeline;
        let bad = survey(
            serde_json::json!([planned("executors", true), planned("executors", true)]),
            serde_json::json!([item("native", "executors")]),
        );
        let v = serde_json::to_value(&bad).unwrap();
        assert!(
            ostra_core::submit::validate_submit(ostra_core::Contract::Documentation, &v).is_ok()
        );
        let issues = crate::OstraPipeline.check_submit(
            ostra_core::Contract::Documentation,
            &v,
            &serde_json::Value::Null,
        );
        assert!(
            issues
                .iter()
                .any(|i| i.contains("Plan page \"executors\" once")),
            "{issues:?}"
        );
        crate::book::install_checks();
        #[allow(deprecated)]
        let old = ostra_core::book::check_documentation(&bad);
        assert_eq!(
            old,
            check_documentation(&bad),
            "the deprecated path runs the same check"
        );
    }

    /// Rule B11: a source tagged with a project covers only that project's modules.
    #[test]
    fn b1_a_tagged_source_covers_its_own_projects_modules() {
        let module = |glob: &str| DocsModule {
            name: "m".into(),
            globs: vec![glob.into()],
        };
        let item: InventoryItem = serde_json::from_value(serde_json::json!({
            "id": "x", "name": "x", "sources": ["@a/src/orders.rs"], "owner": "p"
        }))
        .unwrap();
        assert!(covers(&item, &module("@a/src/**")));
        assert!(!covers(&item, &module("@b/src/**")));
        assert!(covers(&item, &module("src/**")), "a module without a tag");
        let plain: InventoryItem = serde_json::from_value(serde_json::json!({
            "id": "y", "name": "y", "sources": ["src/orders.rs"], "owner": "p"
        }))
        .unwrap();
        assert!(
            covers(&plain, &module("@b/src/**")),
            "a source without a tag"
        );
    }

    #[test]
    fn b10_a_synthesis_is_checked() {
        let syn = |v: serde_json::Value| -> DocumentationSubmit {
            let mut base = serde_json::json!({"status": "ok", "step": "synthesis", "summary": "s"});
            base.as_object_mut()
                .unwrap()
                .extend(v.as_object().unwrap().clone());
            serde_json::from_value(base).unwrap()
        };
        let pass = serde_json::json!([{"check": "one owner per fact", "passed": true}]);
        let fail = serde_json::json!([{"check": "one owner per fact", "passed": false, "note": "slots twice"}]);
        assert!(
            check_documentation(&syn(serde_json::json!({"done": true, "checks": pass}))).is_empty()
        );
        let issues = check_documentation(&syn(
            serde_json::json!({"done": true, "checks": fail.clone()}),
        ));
        assert!(
            issues.iter().any(|i| i.starts_with("Set `done` only when")),
            "{issues:?}"
        );
        let issues = check_documentation(&syn(
            serde_json::json!({"done": false, "checks": fail.clone()}),
        ));
        assert!(
            issues.iter().any(|i| i.starts_with("List the `edits`")),
            "{issues:?}"
        );
        let ok = syn(
            serde_json::json!({"done": false, "checks": fail, "edits": [{"page": "a", "instructions": ["Move the slot rules to `limits`."]}]}),
        );
        assert!(check_documentation(&ok).is_empty());
    }

    #[test]
    fn b10_the_engine_checks_links_words_and_owners() {
        let page = |id: &str, body: &str| -> DocSection {
            serde_json::from_value(
                serde_json::json!({"id": id, "title": id, "summary": "S.", "body": body}),
            )
            .unwrap()
        };
        let pages = vec![
            page(
                "limits",
                "See [executors](executors.md) and [gone](gone.md#x). It would fail; it stops.\n\n```rust\nlet a = 1; // would\n```\n\nUse `a;b`.",
            ),
            page("executors", "Clean text with a [code link](crates/x.rs)."),
        ];
        let inventory: Vec<InventoryItem> = serde_json::from_value(serde_json::json!([
            item("slots", "limits"),
            item("orphan", "missing")
        ]))
        .unwrap();
        let modules = vec![
            DocsModule {
                name: "core".into(),
                globs: vec!["crates/core/**".into()],
            },
            DocsModule {
                name: "web".into(),
                globs: vec!["web/**".into()],
            },
        ];
        let inventory: Vec<InventoryItem> = inventory
            .into_iter()
            .map(|mut i| {
                if i.id == "slots" {
                    i.sources = vec!["crates/core/src/slots.rs".into()];
                }
                i
            })
            .collect();
        let issues = mechanical_issues(&pages, &[], &inventory, &modules);
        let limits = &issues["limits"];
        assert!(limits.iter().any(|i| i.contains("`gone.md`")), "{limits:?}");
        assert!(limits.iter().any(|i| i.contains("\"would\"")), "{limits:?}");
        assert!(
            limits.iter().any(|i| i.starts_with("Split the sentences")),
            "{limits:?}"
        );
        assert!(
            !issues.contains_key("executors"),
            "code and links are not prose: {issues:?}"
        );
        assert!(
            issues[""].iter().any(|i| i.contains("`orphan`")),
            "{issues:?}"
        );
        assert!(
            issues[""].iter().any(|i| i.contains("module `web`")),
            "{issues:?}"
        );
        assert!(
            !issues[""].iter().any(|i| i.contains("module `core`")),
            "a source inside the module covers it: {issues:?}"
        );
        let refs = vec![
            RefItem {
                name: "MAX_SLOTS".into(),
                file: "a.rs".into(),
            },
            RefItem {
                name: "TERM_QUEUE".into(),
                file: "b.rs".into(),
            },
        ];
        let pages = vec![page("x", "The cap is `MAX_SLOTS`.")];
        let missing: Vec<&str> = unmentioned_refs(&pages, &refs)
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(missing, ["TERM_QUEUE"]);
    }

    #[test]
    fn b10_the_engine_checks_user_blocks_owners_and_excerpts() {
        let page = |id: &str, body: &str, refs: bool| -> DocSection {
            let code_refs = if refs {
                serde_json::json!([{"path": "src/slots.rs", "note": "The limiter."}])
            } else {
                serde_json::json!([])
            };
            serde_json::from_value(serde_json::json!({
                "id": id, "title": id, "summary": "S.", "body": body, "code_refs": code_refs
            }))
            .unwrap()
        };
        let inventory: Vec<InventoryItem> = serde_json::from_value(serde_json::json!([{
            "id": "slots", "name": "the slot limiter", "sources": ["src/slots.rs"], "owner": "limits",
            "settings": ["limits.max_parallel_executions"], "names": ["acquire_slot"]
        }]))
        .unwrap();
        let check = |pages: &[DocSection]| mechanical_issues(pages, &[], &inventory, &[]);
        let plain = page(
            "pipeline",
            "## Spawns\n\nEach spawn names acquire_slot in plain words.",
            false,
        );
        assert!(
            !check(&[
                page(
                    "limits",
                    "### For the user\n\n`max_parallel_executions`\n\n```rust\nx\n```",
                    true
                ),
                plain
            ])
            .contains_key("pipeline"),
            "a name outside backticks is a word"
        );

        let owner_ok = page(
            "limits",
            "## Slots\n\n`acquire_slot` waits.\n\n### For the user\n\n| Setting | Default |\n| --- | --- |\n| `max_parallel_executions` | 4 |\n\n```rust\nlet slot = acquire_slot().await;\n```",
            true,
        );
        let linked = page(
            "pipeline",
            "## Spawns\n\nEach spawn calls `acquire_slot` first. See [Limits](limits.md).",
            false,
        );
        let issues = check(&[owner_ok.clone(), linked]);
        assert!(issues.is_empty(), "{issues:?}");

        let owner_bad = page(
            "limits",
            "## Slots\n\n`limits.max_parallel_executions` caps runs.\n\n### For the user\n\nNothing.\n\n## Next\n\nText.",
            true,
        );
        let unlinked = page(
            "pipeline",
            "## Spawns\n\nEach spawn calls `acquire_slot` first.",
            false,
        );
        let issues = check(&[owner_bad, unlinked]);
        let limits = &issues["limits"];
        assert!(
            limits
                .iter()
                .any(|i| i.starts_with("Add a `### For the user` block")
                    && i.contains("`limits.max_parallel_executions`")),
            "{limits:?}"
        );
        assert!(
            limits
                .iter()
                .any(|i| i.starts_with("Add 1 or 2 code excerpts")),
            "{limits:?}"
        );
        assert!(
            issues["pipeline"]
                .iter()
                .any(|i| i.starts_with("Link to `limits.md` in `Spawns`")),
            "{issues:?}"
        );

        let repeats = page(
            "pipeline",
            "## Spawns\n\n`acquire_slot` waits. See [Limits](limits.md).\n\n`acquire_slot` wakes each 2 seconds.\n\nA panic drops what `acquire_slot` returned.",
            false,
        );
        let mut catalog = repeats.clone();
        catalog.group = "Reference".into();
        assert!(
            !check(&[owner_ok.clone(), catalog]).contains_key("pipeline"),
            "a catalog page lists the values and links to the owner"
        );
        let issues = check(&[owner_ok, repeats]);
        assert!(
            issues["pipeline"]
                .iter()
                .any(|i| i.starts_with("Cut what this page says about the slot limiter")),
            "{issues:?}"
        );
    }
}
