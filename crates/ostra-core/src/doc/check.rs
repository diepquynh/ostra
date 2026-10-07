//! Checks Ostra runs on every document write. Errors are broken references and rules code can
//! decide; they block the submit call. Warnings are shown to the agent and the reader.

use super::refs;
use super::store::{DocKind, load, phase_path};
use super::{Document, PhaseDoc, PlanDoc, ResearchDoc, SpecDoc};
use crate::contract::Contract;
use crate::pipeline::Question;
use crate::submit::{GenerateSpecSubmit, PlanSubmit};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum IssueLevel {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocIssue {
    pub level: IssueLevel,
    /// The element the issue is about, for example `R3` or `step 2.1`.
    pub element: Option<String>,
    pub message: String,
}

impl DocIssue {
    pub fn line(&self) -> String {
        let level = match self.level {
            IssueLevel::Error => "error",
            IssueLevel::Warning => "warning",
        };
        match &self.element {
            Some(e) => format!("- {level} ({e}): {}", self.message),
            None => format!("- {level}: {}", self.message),
        }
    }
}

#[derive(Default)]
pub(super) struct Issues(Vec<DocIssue>);

impl Issues {
    pub(super) fn error(&mut self, element: impl Into<Option<String>>, message: String) {
        self.0.push(DocIssue {
            level: IssueLevel::Error,
            element: element.into(),
            message,
        });
    }

    pub(super) fn warn(&mut self, element: impl Into<Option<String>>, message: String) {
        self.0.push(DocIssue {
            level: IssueLevel::Warning,
            element: element.into(),
            message,
        });
    }

    fn unique<'a>(&mut self, what: &str, ids: impl IntoIterator<Item = &'a str>) {
        let mut seen = HashSet::new();
        for id in ids {
            if !seen.insert(id) {
                self.error(
                    Some(id.to_string()),
                    format!("Give each {what} its own id: `{id}` appears more than once."),
                );
            }
        }
    }

    fn questions(&mut self, qs: &[Question]) {
        self.unique("question", qs.iter().map(|q| q.id.as_str()));
        for q in qs {
            let at = Some(q.id.clone());
            if !(2..=4).contains(&q.options.len()) {
                self.error(
                    at.clone(),
                    format!(
                        "Give the question 2 to 4 options; it has {}.",
                        q.options.len()
                    ),
                );
            }
            if q.recommended >= q.options.len().max(1) {
                self.error(
                    at.clone(),
                    "Point `recommended` at one of the options by its index.".into(),
                );
            }
            if q.tag.chars().count() > 12 {
                self.warn(at, format!("Shorten the tag `{}` to 12 characters or fewer, because the question card clips it.", q.tag));
            }
        }
    }

    fn done(mut self) -> Vec<DocIssue> {
        self.0.sort_by_key(|i| i.level);
        self.0
    }
}

pub fn check(doc: &Document) -> Vec<DocIssue> {
    let mut out = Issues::default();
    checks(doc, &mut out);
    out.done()
}

fn checks(doc: &Document, out: &mut Issues) {
    match doc {
        Document::Research(d) => research(d, out),
        Document::Spec(d) => spec(d, out),
        Document::Plan(d) => plan(d, out),
        Document::Phase(d) => phase_file(d, out),
    }
}

/// [`check`] plus the checks of the code the document names, which hold only until a build
/// changes the repo. Every write and every submit runs these (Rule Hard 4).
pub fn check_written(doc: &Document) -> Vec<DocIssue> {
    let mut out = Issues::default();
    checks(doc, &mut out);
    match doc {
        Document::Research(d) => refs::research(d, &mut out),
        Document::Spec(d) => refs::spec(d, &mut out),
        Document::Plan(d) => refs::plan(d, &mut out),
        Document::Phase(_) => {}
    }
    out.done()
}

fn research(d: &ResearchDoc, out: &mut Issues) {
    out.questions(&d.open_questions);
    let urls: HashSet<&str> = d.sources.iter().map(|s| s.url.as_str()).collect();
    for f in &d.external {
        if !urls.contains(f.source.as_str()) {
            out.error(
                Some(f.technology.clone()),
                format!("Add `{}` to `sources`, or drop the fact: every external fact cites a page retrieved in this run.", f.source),
            );
        }
    }
    for s in &d.sources {
        if !(s.url.starts_with("http://") || s.url.starts_with("https://")) {
            out.warn(
                Some(s.url.clone()),
                "Cite the page by its full http or https URL.".into(),
            );
        }
    }
    let picked = d.approaches.iter().filter(|a| a.recommended).count();
    if picked > 1 {
        out.error(None, "Mark at most one approach as recommended.".into());
    }
    if !d.approaches.is_empty() && d.recommendation.is_none() {
        out.warn(
            None,
            "Write the recommendation, because approaches were given.".into(),
        );
    }
}

/// `n` in `R{n}`.
fn req_number(id: &str) -> Option<&str> {
    id.strip_prefix('R')
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

fn spec(d: &SpecDoc, out: &mut Issues) {
    out.unique("criterion", d.criteria.iter().map(|c| c.id.as_str()));
    out.unique("deliverable", d.deliverables.iter().map(|x| x.id.as_str()));
    out.unique("requirement", d.requirements.iter().map(|r| r.id.as_str()));
    out.unique(
        "acceptance criterion",
        d.requirements
            .iter()
            .flat_map(|r| r.acceptance.iter().map(|a| a.id.as_str())),
    );
    out.unique("evidence row", d.evidence.iter().map(|e| e.id.as_str()));
    out.questions(&d.open_questions);

    let criteria: HashSet<&str> = d.criteria.iter().map(|c| c.id.as_str()).collect();
    let deliverables: HashSet<&str> = d.deliverables.iter().map(|x| x.id.as_str()).collect();
    let evidence: HashSet<&str> = d.evidence.iter().map(|e| e.id.as_str()).collect();
    let questions: HashSet<&str> = d.open_questions.iter().map(|q| q.id.as_str()).collect();

    for c in &d.criteria {
        for dep in &c.depends_on {
            if !criteria.contains(dep.as_str()) {
                out.error(
                    Some(c.id.clone()),
                    format!("`depends_on` names `{dep}`, which is not a criterion."),
                );
            }
        }
        if let Some(q) = &c.provisional
            && !questions.contains(q.as_str())
        {
            out.warn(
                Some(c.id.clone()),
                format!("`provisional` names `{q}`, which is not an open question."),
            );
        }
    }

    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for x in &d.deliverables {
        for dep in &x.depends_on {
            if !deliverables.contains(dep.as_str()) {
                out.error(
                    Some(x.id.clone()),
                    format!("`depends_on` names `{dep}`, which is not a deliverable."),
                );
            }
        }
        edges.insert(
            x.id.as_str(),
            x.depends_on.iter().map(String::as_str).collect(),
        );
    }
    if let Some(cycle) = find_cycle(&edges) {
        out.error(None, format!("Merge the deliverables in the cycle {cycle} into one, because delivery order must be acyclic (Step 5)."));
    }

    // Rule S1, R-e: every criterion is delivered by a requirement.
    let covered: HashSet<&str> = d
        .requirements
        .iter()
        .flat_map(|r| r.covers.iter().map(String::as_str))
        .collect();
    for c in &d.criteria {
        if !covered.contains(c.id.as_str()) {
            out.error(
                Some(c.id.clone()),
                "Cover this criterion with a requirement, or no plan will build it (S1).".into(),
            );
        }
    }

    let order: Vec<&str> = d.deliverables.iter().map(|x| x.id.as_str()).collect();
    let mut last_deliverable = 0usize;
    for (expected, r) in (1u64..).zip(&d.requirements) {
        let at = Some(r.id.clone());
        if !deliverables.contains(r.deliverable.as_str()) {
            out.error(
                at.clone(),
                format!(
                    "`deliverable` names `{}`, which is not a deliverable.",
                    r.deliverable
                ),
            );
        }
        if let Some(pos) = order.iter().position(|x| *x == r.deliverable) {
            if pos < last_deliverable {
                out.warn(
                    at.clone(),
                    "Number requirements in deliverable order: D1's first, then D2's.".into(),
                );
            }
            last_deliverable = pos;
        }
        match req_number(&r.id).and_then(|n| n.parse::<u64>().ok()) {
            Some(n) if n == expected => {}
            Some(_) => out.warn(
                at.clone(),
                format!("Number requirements in one flat sequence; expected `R{expected}` here."),
            ),
            None => out.error(at.clone(), "Use the id form `R{n}`.".into()),
        }
        if r.covers.is_empty() {
            out.error(
                at.clone(),
                "Name the criteria this requirement delivers in `covers` (R-e).".into(),
            );
        }
        for c in &r.covers {
            if !criteria.contains(c.as_str()) {
                out.error(
                    at.clone(),
                    format!("`covers` names `{c}`, which is not a criterion (S8)."),
                );
            }
        }
        for e in &r.rests_on {
            if !evidence.contains(e.as_str()) {
                out.error(
                    at.clone(),
                    format!("`rests_on` names `{e}`, which is not an evidence row."),
                );
            }
        }
        let shalls = r.statement.matches("SHALL").count();
        if shalls != 1 {
            out.warn(
                at.clone(),
                format!("Write exactly one `SHALL` in the statement; it has {shalls} (R-a)."),
            );
        }
        if r.acceptance.is_empty() {
            out.error(
                at.clone(),
                "Give the requirement at least one acceptance criterion (AC-a).".into(),
            );
        }
        let n = req_number(&r.id).unwrap_or_default();
        for a in &r.acceptance {
            let ok =
                a.id.strip_prefix("AC")
                    .and_then(|rest| rest.split_once('.'))
                    .is_some_and(|(rn, m)| {
                        rn == n && !m.is_empty() && m.chars().all(|c| c.is_ascii_digit())
                    });
            if !ok {
                out.error(Some(a.id.clone()), format!("Number this acceptance criterion `AC{n}.{{m}}`, after its requirement `{}`.", r.id));
            }
            if a.given.trim().is_empty() || a.when.trim().is_empty() || a.then.trim().is_empty() {
                out.error(
                    Some(a.id.clone()),
                    "Fill GIVEN, WHEN, and THEN (AC-a).".into(),
                );
            }
        }
    }

    // Rule S4: at most 6 criteria per deliverable, unless the notes explain the overrun.
    for x in &d.deliverables {
        let count: BTreeSet<&str> = d
            .requirements
            .iter()
            .filter(|r| r.deliverable == x.id)
            .flat_map(|r| r.covers.iter().map(String::as_str))
            .collect();
        if count.len() > 6 && !d.notes.iter().any(|n| n.contains(&x.id)) {
            out.warn(
                Some(x.id.clone()),
                format!(
                    "Split the deliverable or explain it in `notes`: it covers {} criteria (S4).",
                    count.len()
                ),
            );
        }
        if !d.requirements.iter().any(|r| r.deliverable == x.id) {
            out.error(
                Some(x.id.clone()),
                "Give the deliverable at least one requirement, or remove it.".into(),
            );
        }
    }

    for c in &d.contracts_provided {
        if !deliverables.contains(c.provided_by.as_str()) {
            out.error(
                Some(c.name.clone()),
                format!(
                    "`provided_by` names `{}`, which is not a deliverable.",
                    c.provided_by
                ),
            );
        }
        for by in &c.consumed_by {
            if by.starts_with('D')
                && by[1..].chars().all(|ch| ch.is_ascii_digit())
                && !deliverables.contains(by.as_str())
            {
                out.error(
                    Some(c.name.clone()),
                    format!("`consumed_by` names `{by}`, which is not a deliverable."),
                );
            }
        }
    }
    let used: HashSet<&str> = d
        .requirements
        .iter()
        .flat_map(|r| r.rests_on.iter().map(String::as_str))
        .collect();
    for e in &d.evidence {
        if !used.contains(e.id.as_str()) {
            out.warn(
                Some(e.id.clone()),
                "No requirement rests on this row. Name it in a `rests_on`, or drop the row."
                    .into(),
            );
        }
    }
}

fn find_cycle<K: Ord + Copy + std::fmt::Display>(edges: &BTreeMap<K, Vec<K>>) -> Option<String> {
    fn visit<K: Ord + Copy + std::fmt::Display>(
        n: K,
        edges: &BTreeMap<K, Vec<K>>,
        stack: &mut Vec<K>,
        done: &mut BTreeSet<K>,
    ) -> Option<String> {
        if let Some(pos) = stack.iter().position(|s| *s == n) {
            let mut path: Vec<String> = stack[pos..].iter().map(|s| s.to_string()).collect();
            path.push(n.to_string());
            return Some(path.join(" -> "));
        }
        if done.contains(&n) {
            return None;
        }
        stack.push(n);
        for m in edges.get(&n).into_iter().flatten() {
            if edges.contains_key(m)
                && let Some(c) = visit(*m, edges, stack, done)
            {
                return Some(c);
            }
        }
        stack.pop();
        done.insert(n);
        None
    }
    let mut done = BTreeSet::new();
    edges
        .keys()
        .find_map(|k| visit(*k, edges, &mut vec![], &mut done))
}

/// The skills installed in a project: each skills dir entry holding a `SKILL.md`.
fn installed_skills(root: &Path) -> BTreeSet<String> {
    crate::paths::project_skill_dirs(root)
        .into_iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter(|e| e.path().join("SKILL.md").is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect()
}

/// Rules P6 and P7: in a repo with skills installed, each phase with code steps names the skills
/// they need, and every named skill exists there. The implementer loads only what the phase file
/// lists, so a plan without skills builds without the project's patterns.
fn plan_skills(d: &PlanDoc, out: &mut Issues, installed: &dyn Fn(&str) -> BTreeSet<String>) {
    let mut by_root: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for p in &d.phases {
        let have = by_root
            .entry(p.repo_root.as_str())
            .or_insert_with(|| installed(&p.repo_root));
        if have.is_empty() {
            continue;
        }
        let at = Some(format!("phase {}", p.id));
        let code_steps = p
            .steps
            .iter()
            .filter(|s| !matches!(s.change, super::FileChange::Delete))
            .count();
        if code_steps > 0 && p.steps.iter().all(|s| s.skills.is_empty()) {
            out.error(
                at.clone(),
                format!(
                    "Name each step's skills from {}'s INVENTORY Skill Application Mapping (P6): no step in phase {} names one, so the implementer would build without the project's patterns. Installed: {}.",
                    p.repo,
                    p.id,
                    have.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            );
        }
        for s in &p.steps {
            for k in &s.skills {
                if !have.contains(k) {
                    out.error(
                        Some(format!("step {}", s.id)),
                        format!(
                            "Use a skill installed in {}: `{k}` is not one. Take the exact name from its INVENTORY Skill Application Mapping (P6).",
                            p.repo
                        ),
                    );
                }
            }
        }
    }
}

fn plan(d: &PlanDoc, out: &mut Issues) {
    out.questions(&d.clarifying_questions);
    plan_skills(d, out, &|root| installed_skills(Path::new(root)));
    out.unique(
        "phase",
        d.phases
            .iter()
            .map(|p| p.id.to_string())
            .collect::<Vec<_>>()
            .iter()
            .map(String::as_str),
    );
    // Rule P10: one unbroken sequence.
    for (i, p) in d.phases.iter().enumerate() {
        if p.id as usize != i + 1 {
            out.error(
                Some(format!("phase {}", p.id)),
                format!(
                    "Number phases 1 to {} in order; expected phase {} here (P10).",
                    d.phases.len(),
                    i + 1
                ),
            );
            break;
        }
    }
    let ids: HashSet<u32> = d.phases.iter().map(|p| p.id).collect();
    let mut edges: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let deliverables: HashSet<&str> = d.deliverables.iter().map(|x| x.id.as_str()).collect();
    for p in &d.phases {
        let at = Some(format!("phase {}", p.id));
        for dep in &p.depends_on {
            if !ids.contains(dep) {
                out.error(
                    at.clone(),
                    format!("`depends_on` names phase {dep}, which does not exist."),
                );
            } else if *dep >= p.id {
                out.warn(
                    at.clone(),
                    format!("Depend only on earlier phases; phase {dep} comes after this one."),
                );
            }
        }
        edges.insert(p.id, p.depends_on.clone());
        if !d.deliverables.is_empty() && !deliverables.contains(p.deliverable.as_str()) {
            out.warn(
                at.clone(),
                format!(
                    "Add `{}` to `deliverables`, so the Deliverable Index shows its title.",
                    p.deliverable
                ),
            );
        }
        phase_body(p, out);
    }
    if let Some(cycle) = find_cycle(&edges) {
        out.error(
            None,
            format!("Break the phase cycle {cycle}, because Ostra schedules phases as a graph."),
        );
    }
    working_features(d, out);
}

/// Rule P14: each phase leaves at least one requirement working, so a phase is a feature and never a
/// layer. A phase in another repo does not count against it, because P8a splits one feature by repo.
fn working_features(d: &PlanDoc, out: &mut Issues) {
    let mut by_req: BTreeMap<&str, Vec<&super::Phase>> = BTreeMap::new();
    for p in &d.phases {
        let reqs: BTreeSet<&str> = p
            .steps
            .iter()
            .flat_map(|s| s.delivers.iter().map(String::as_str))
            .collect();
        for r in reqs {
            by_req.entry(r).or_default().push(p);
        }
    }
    for p in &d.phases {
        let mine: Vec<&str> = by_req
            .iter()
            .filter(|(_, ps)| ps.iter().any(|q| q.id == p.id))
            .map(|(r, _)| *r)
            .collect();
        if mine.is_empty() {
            continue;
        }
        let shared_with = |r: &str| -> Vec<u32> {
            by_req[r]
                .iter()
                .filter(|q| q.id != p.id && q.repo == p.repo)
                .map(|q| q.id)
                .collect()
        };
        if mine.iter().all(|r| !shared_with(r).is_empty()) {
            let others: BTreeSet<u32> = mine.iter().flat_map(|r| shared_with(r)).collect();
            out.error(
                Some(format!("phase {}", p.id)),
                format!(
                    "Merge phase {} with phase {}: it completes none of {}, so its code works only after another phase (P14). A phase delivers at least one working feature, never one layer.",
                    p.id,
                    others.iter().map(u32::to_string).collect::<Vec<_>>().join(", "),
                    mine.join(", ")
                ),
            );
        }
    }
    for (r, ps) in &by_req {
        let repo = ps[0].repo.as_str();
        if ps.len() > 1 && ps.iter().all(|q| q.repo == repo) {
            out.warn(
                Some((*r).to_string()),
                format!(
                    "Deliver `{r}` in one phase: phases {} each build part of it (P14).",
                    ps.iter().map(|q| q.id.to_string()).collect::<Vec<_>>().join(", ")
                ),
            );
        }
    }
}

fn phase_file(d: &PhaseDoc, out: &mut Issues) {
    phase_body(&d.phase, out);
}

fn phase_body(p: &super::Phase, out: &mut Issues) {
    let at = || Some(format!("phase {}", p.id));
    // Rule P12: `Skip` carries a rationale naming what each step contains.
    if p.test_rationale.trim().is_empty() {
        out.error(
            at(),
            "Write the one-sentence test policy rationale (P12).".into(),
        );
    }
    if p.steps.is_empty() {
        out.error(at(), "Give the phase at least one step.".into());
    }
    out.unique("step", p.steps.iter().map(|s| s.id.as_str()));
    let quoted: HashSet<&str> = p.requirements.iter().map(|r| r.id.as_str()).collect();
    let constraints: BTreeMap<&str, &str> = p
        .constraints
        .iter()
        .map(|e| (e.id.as_str(), e.rule.as_str()))
        .collect();
    let prefix = format!("{}.", p.id);
    for s in &p.steps {
        let at = Some(format!("step {}", s.id));
        if !s
            .id
            .strip_prefix(&prefix)
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        {
            out.error(
                at.clone(),
                format!("Number the step `{}{{n}}`, after its phase.", prefix),
            );
        }
        // Rule P11: every step cites what it delivers.
        if s.delivers.is_empty() {
            out.warn(
                at.clone(),
                "Name the requirement IDs this step delivers (P11).".into(),
            );
        }
        for r in &s.delivers {
            if !quoted.contains(r.as_str()) {
                out.warn(at.clone(), format!("Quote `{r}` in the phase's `requirements`, because the implementer never reads the spec."));
            }
        }
        // Rule P13: the rule text travels with the step.
        for b in &s.binding_rules {
            match constraints.get(b.id.as_str()) {
                None => out.error(at.clone(), format!("Copy `{}` into the phase's `constraints`, because the implementer never reads the spec (P13).", b.id)),
                Some(rule) if rule.trim() != b.rule.trim() => {
                    out.error(at.clone(), format!("Copy `{}`'s rule verbatim from `constraints`; the step's wording differs (P13).", b.id))
                }
                Some(_) => {}
            }
            if b.rule.trim().is_empty() {
                out.error(
                    at.clone(),
                    format!("Write `{}`'s rule sentence, not only its id (P13).", b.id),
                );
            }
        }
        if s.verify.trim().is_empty() {
            out.error(
                at.clone(),
                "Give the step a verification command (P5).".into(),
            );
        }
    }
}

/// The generate-inventory submit: the inventory exists and the profile parses, checked while the
/// agent can still fix them, because the init fails at its last step otherwise. It is the only
/// initializer mode whose `result` names both files.
fn check_inventory_submit(input: &serde_json::Value) -> Result<(), String> {
    let result = input.get("result");
    let path = |k: &str| result.and_then(|r| r.get(k)).and_then(|v| v.as_str());
    let (Some(inventory), Some(profile)) = (path("inventory_path"), path("profile_path")) else {
        return Ok(());
    };
    if !Path::new(inventory).is_file() {
        return Err(format!(
            "Write {inventory} before you submit: the file does not exist."
        ));
    }
    crate::config::check_profile(Path::new(profile)).map(|_| ())
}

fn read_doc(kind: DocKind, path: &str) -> Result<Document, String> {
    let md = Path::new(path);
    let value = load(md).ok_or_else(|| {
        format!(
            "Write the {} with the Document tool before you submit: {} has no document behind it.",
            kind.label(),
            md.display()
        )
    })?;
    kind.parse(&value)
}

fn blocking(doc: &Document) -> Result<(), String> {
    let errors: Vec<String> = check_written(doc)
        .into_iter()
        .filter(|i| i.level == IssueLevel::Error)
        .map(|i| i.line())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Fix these in the document with the Document tool, then submit again:\n{}",
            errors.join("\n")
        ))
    }
}

/// For agents that write a document: the submitted path has a document with no errors, and the
/// submit's counts agree with it. Reads files, so it runs at the tool boundary, never in the fold.
pub fn check_submit(contract: Contract, input: &serde_json::Value) -> Result<(), String> {
    if contract == Contract::Setup {
        return check_inventory_submit(input);
    }
    let Some(kind) = DocKind::for_contract(contract) else {
        return Ok(());
    };
    let field = match kind {
        DocKind::Research => "research_path",
        DocKind::Spec => "spec_path",
        DocKind::Plan => "master_plan_path",
    };
    let path = input
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("`{field}` is missing."))?;
    let doc = read_doc(kind, path)?;
    blocking(&doc)?;
    let mut wrong = vec![];
    match &doc {
        Document::Spec(d) => {
            if let Ok(s) = serde_json::from_value::<GenerateSpecSubmit>(input.clone()) {
                let pairs = [
                    (
                        "external_evidence_rows",
                        s.external_evidence_rows as usize,
                        d.evidence.len(),
                    ),
                    (
                        "deliverables",
                        s.deliverables as usize,
                        d.deliverables.len(),
                    ),
                    (
                        "requirements",
                        s.requirements as usize,
                        d.requirements.len(),
                    ),
                ];
                for (name, sent, has) in pairs {
                    if sent != has {
                        wrong.push(format!("`{name}` is {sent}, but the spec has {has}."));
                    }
                }
            }
        }
        Document::Plan(d) => {
            if let Ok(s) = serde_json::from_value::<PlanSubmit>(input.clone()) {
                let steps: usize = d.phases.iter().map(|p| p.steps.len()).sum();
                if s.step_count as usize != steps {
                    wrong.push(format!(
                        "`step_count` is {}, but the plan has {steps} steps.",
                        s.step_count
                    ));
                }
                if s.phases.len() != d.phases.len() {
                    wrong.push(format!(
                        "`phases` lists {}, but the plan has {}.",
                        s.phases.len(),
                        d.phases.len()
                    ));
                }
                for (sp, p) in s.phases.iter().zip(&d.phases) {
                    let file = phase_path(Path::new(path), p.id);
                    let mut deps = sp.depends_on.clone();
                    deps.sort();
                    let mut have = p.depends_on.clone();
                    have.sort();
                    let same = sp.id == p.id
                        && sp.deliverable == p.deliverable
                        && sp.project == p.repo
                        && sp.complexity == p.complexity.as_str()
                        && sp.test_policy == p.test_policy.as_str()
                        && deps == have
                        && (sp.file.trim().is_empty() || Path::new(&sp.file) == file);
                    if !same {
                        wrong.push(format!(
                            "Phase {} in `phases` disagrees with the plan. Send id {}, deliverable {}, project {}, complexity {}, test_policy {}, depends_on {:?}, file {}.",
                            sp.id,
                            p.id,
                            p.deliverable,
                            p.repo,
                            p.complexity.as_str(),
                            p.test_policy.as_str(),
                            p.depends_on,
                            file.display()
                        ));
                    }
                }
            }
        }
        _ => {}
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(wrong.join(" "))
    }
}
