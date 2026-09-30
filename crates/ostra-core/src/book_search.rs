//! Rule B8: retrieval over the workspace's documentation books. Each section is cut into small
//! passages (its purpose, boundaries, assumptions, flow, each diagram, each table, concerns, and
//! code references, long lists in windows), each passage is ranked with BM25 over three weighted
//! fields, and a unit (a section, sub-section, glossary term, or architecture aspect) is ranked by its
//! best passages. A hit carries only the passages that matched, never the whole section.

use crate::book::{
    self, Book, Boundaries, BusinessStep, CodeRef, Concern, Diagram, DocTable, LinkMode,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_LIMIT: usize = 5;
pub const MAX_LIMIT: usize = 15;
/// List items or table rows in one passage.
const WINDOW: usize = 5;
/// Passages a hit shows.
const PASSAGES_PER_HIT: usize = 2;
const K1: f32 = 1.2;
const B: f32 = 0.75;
const W_TITLE: f32 = 2.0;
const W_CODE: f32 = 1.5;
const W_BODY: f32 = 1.0;
/// A unit's title, counted once for the unit.
const W_UNIT_TITLE: f32 = 3.0;
/// Share of the unit's whole-text BM25 score added to its score, for terms spread over passages.
const UNIT_SHARE: f32 = 1.0;
/// Share of the second-best passage added to a unit's score.
const SECOND_SHARE: f32 = 0.35;
/// A passage below this share of its unit's best is not shown.
const SHOW_SHARE: f32 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Overview,
    Section,
    Subsection,
    Glossary,
    Architecture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassageKind {
    Overview,
    Purpose,
    Boundaries,
    Assumptions,
    BusinessFlow,
    Diagram,
    Table,
    Concerns,
    CodeRefs,
    Glossary,
    Architecture,
}

/// A part of a book a hit points at.
#[derive(Debug, Clone)]
pub struct Unit {
    pub book: String,
    /// Empty for the glossary and the architecture.
    pub project: String,
    /// `project/section`, `project/section/sub`, `project/overview`, `glossary/term`, or
    /// `architecture/aspect`.
    pub id: String,
    pub kind: UnitKind,
    /// `Section > Sub-section`.
    pub title: String,
    /// The Markdown file, relative to the books folder.
    pub file: PathBuf,
    /// The unit's code reference paths, `/`-separated and project-relative.
    pub code_paths: Vec<String>,
    title_tf: HashMap<String, u32>,
}

#[derive(Debug, Clone)]
pub struct Passage {
    pub unit: usize,
    pub kind: PassageKind,
    /// What the passage is, for example `Assumptions` or `Diagram: Slot hand-off`.
    pub label: String,
    /// The passage as a reader sees it.
    pub text: String,
    title_tf: HashMap<String, u32>,
    code_tf: HashMap<String, u32>,
    body_tf: HashMap<String, u32>,
    len: f32,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub unit: usize,
    pub score: f32,
    /// Passage indexes, best first.
    pub passages: Vec<(usize, f32)>,
}

#[derive(Debug, Default)]
pub struct Index {
    pub units: Vec<Unit>,
    pub passages: Vec<Passage>,
    df: HashMap<String, u32>,
    avg_len: f32,
    unit_tf: Vec<HashMap<String, f32>>,
    unit_len: Vec<f32>,
    unit_avg_len: f32,
}

/// A unit for [`Index::from_parts`].
#[derive(Debug, Clone)]
pub struct UnitSpec {
    pub book: String,
    pub id: String,
    pub title: String,
    pub file: PathBuf,
    pub code_paths: Vec<String>,
}

/// A passage for [`Index::from_parts`]. `code` holds the paths and symbols it names.
#[derive(Debug, Clone)]
pub struct PassageSpec {
    pub unit: usize,
    pub kind: PassageKind,
    pub label: String,
    pub text: String,
    pub code: String,
}

#[derive(Debug, Default, Clone)]
pub struct Filter<'a> {
    pub book: Option<&'a str>,
    pub project: Option<&'a str>,
}

/// Every readable book of the workspace.
pub fn load_books(workspace: &Path) -> Vec<Book> {
    let Ok(rd) = std::fs::read_dir(book::dir(workspace)) else {
        return vec![];
    };
    let mut ids: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| book::is_book_id(n))
        .collect();
    ids.sort();
    ids.iter()
        .filter_map(|id| book::read(workspace, id))
        .collect()
}

impl Index {
    pub fn build(books: &[Book]) -> Index {
        let mut ix = Builder::default();
        for b in books {
            ix.book(b);
        }
        ix.finish()
    }

    /// An index over units and passages cut elsewhere, such as Markdown pages, ranked the same
    /// way as a book. Each passage names its unit by position in `units`.
    pub fn from_parts(units: Vec<UnitSpec>, passages: Vec<PassageSpec>) -> Index {
        let mut ix = Builder::default();
        for u in units {
            ix.unit(Unit {
                book: u.book,
                project: String::new(),
                id: u.id,
                kind: UnitKind::Section,
                title: u.title,
                file: u.file,
                code_paths: u.code_paths,
                title_tf: HashMap::new(),
            });
        }
        for p in passages {
            ix.passage(p.unit, p.kind, p.label, p.text, &p.code);
        }
        ix.finish()
    }

    pub fn is_empty(&self) -> bool {
        self.passages.is_empty()
    }

    pub fn search(&self, query: &str, filter: &Filter, limit: usize) -> Vec<Hit> {
        let mut terms = tokens(query);
        terms.sort();
        terms.dedup();
        if terms.is_empty() || self.passages.is_empty() {
            return vec![];
        }
        let n = (self.passages.len() + self.units.len()) as f32;
        let idf: Vec<(String, f32)> = terms
            .into_iter()
            .filter_map(|t| {
                let df = *self.df.get(&t)? as f32;
                Some((t, (1.0 + (n - df + 0.5) / (df + 0.5)).ln()))
            })
            .collect();
        let keep = |u: &Unit| {
            filter.book.is_none_or(|b| b == u.book)
                && filter.project.is_none_or(|pr| pr == u.project)
        };
        let (k1, b, wt, wc, wb) = (K1, B, W_TITLE, W_CODE, W_BODY);
        let mut by_unit: HashMap<usize, Vec<(usize, f32)>> = HashMap::new();
        for (i, p) in self.passages.iter().enumerate() {
            if !keep(&self.units[p.unit]) {
                continue;
            }
            let norm = k1 * (1.0 - b + b * p.len / self.avg_len);
            let mut score = 0.0;
            for (t, w) in &idf {
                let tf = wt * *p.title_tf.get(t).unwrap_or(&0) as f32
                    + wc * *p.code_tf.get(t).unwrap_or(&0) as f32
                    + wb * *p.body_tf.get(t).unwrap_or(&0) as f32;
                if tf > 0.0 {
                    score += w * tf * (k1 + 1.0) / (tf + norm);
                }
            }
            if score > 0.0 {
                by_unit.entry(p.unit).or_default().push((i, score));
            }
        }
        // A unit's title counts once for the unit, and never picks which passages are shown.
        for (ui, u) in self.units.iter().enumerate() {
            if keep(u) && idf.iter().any(|(t, _)| u.title_tf.contains_key(t)) {
                by_unit.entry(ui).or_default();
            }
        }
        let mut hits: Vec<Hit> = by_unit
            .into_iter()
            .map(|(unit, mut ps)| {
                let u = &self.units[unit];
                let title: f32 = idf
                    .iter()
                    .filter_map(|(t, w)| u.title_tf.get(t).map(|c| (w, *c as f32)))
                    .map(|(w, c)| W_UNIT_TITLE * w * c * (k1 + 1.0) / (c + k1))
                    .sum();
                let unorm = k1 * (1.0 - b + b * self.unit_len[unit] / self.unit_avg_len);
                let doc: f32 = idf
                    .iter()
                    .filter_map(|(t, w)| {
                        self.unit_tf[unit]
                            .get(t)
                            .map(|c| w * c * (k1 + 1.0) / (c + unorm))
                    })
                    .sum();
                ps.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                let best = ps.first().map_or(0.0, |p| p.1);
                let score =
                    title + best + SECOND_SHARE * ps.get(1).map_or(0.0, |p| p.1) + UNIT_SHARE * doc;
                ps.retain(|p| p.1 >= best * SHOW_SHARE);
                ps.truncate(PASSAGES_PER_HIT);
                if ps.is_empty()
                    && let Some(first) = self.passages.iter().position(|p| p.unit == unit)
                {
                    ps.push((first, 0.0));
                }
                Hit {
                    unit,
                    score,
                    passages: ps,
                }
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.unit.cmp(&b.unit)));
        hits.truncate(limit);
        hits
    }

    /// The hits as tool output: each unit's place in the book and its matching passages.
    pub fn render(&self, query: &str, hits: &[Hit], books_dir: &Path) -> String {
        let terms: std::collections::HashSet<String> = tokens(query).into_iter().collect();
        let mut out = String::new();
        for (rank, h) in hits.iter().enumerate() {
            let u = &self.units[h.unit];
            let place = if u.project.is_empty() {
                u.book.clone()
            } else {
                format!("{}, project `{}`", u.book, u.project)
            };
            out.push_str(&format!(
                "{}. {} ({place})\n   File: `{}`\n",
                rank + 1,
                u.title,
                books_dir.join(&u.book).join(&u.file).display()
            ));
            for (pi, _) in &h.passages {
                let p = &self.passages[*pi];
                out.push_str(&format!("   {}:\n", p.label));
                let lines: Vec<&str> = p.text.lines().collect();
                let shown = focus(p.kind, &lines, &terms);
                for line in &shown {
                    out.push_str(&format!("   {line}\n"));
                }
                if shown.len() < lines.len() {
                    let more = lines.len() - shown.len();
                    out.push_str(&format!(
                        "   ({more} more line{} in the file)\n",
                        if more == 1 { "" } else { "s" }
                    ));
                }
            }
            out.push('\n');
        }
        out.trim_end().to_string()
    }
}

/// The lines of a list or table passage that name a query word, so a hit shows the rows that
/// matched and not the whole window. Prose and diagrams stay whole, and so does a passage that
/// matched only through its label.
fn focus<'l>(
    kind: PassageKind,
    lines: &[&'l str],
    terms: &std::collections::HashSet<String>,
) -> Vec<&'l str> {
    let listy = !matches!(
        kind,
        PassageKind::Overview | PassageKind::Purpose | PassageKind::Diagram | PassageKind::Glossary
    );
    if !listy || lines.len() < 2 {
        return lines.to_vec();
    }
    let hit: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| tokens(l).iter().any(|t| terms.contains(t)))
        .collect();
    if hit.is_empty() { lines.to_vec() } else { hit }
}

#[derive(Default)]
struct Builder {
    units: Vec<Unit>,
    passages: Vec<Passage>,
}

impl Builder {
    fn unit(&mut self, mut u: Unit) -> usize {
        u.title_tf = tf(&u.title);
        self.units.push(u);
        self.units.len() - 1
    }

    fn passage(&mut self, unit: usize, kind: PassageKind, label: String, text: String, code: &str) {
        if text.trim().is_empty() {
            return;
        }
        let body_src = if kind == PassageKind::Diagram {
            diagram_words(&text)
        } else {
            text.clone()
        };
        let title_tf = tf(&label);
        let code_tf = tf(code);
        let body_tf = tf(&body_src);
        let len = W_TITLE * total(&title_tf) + W_CODE * total(&code_tf) + W_BODY * total(&body_tf);
        self.passages.push(Passage {
            unit,
            kind,
            label,
            text,
            title_tf,
            code_tf,
            body_tf,
            len,
        });
    }

    fn book(&mut self, b: &Book) {
        for part in &b.parts {
            let u = self.unit(Unit {
                book: b.id.clone(),
                project: part.project.clone(),
                id: format!("{}/overview", part.project),
                kind: UnitKind::Overview,
                title: format!("{} overview", part.project),
                file: "index.md".into(),
                code_paths: vec![],
                title_tf: HashMap::new(),
            });
            for para in part.overview.split("\n\n") {
                self.passage(
                    u,
                    PassageKind::Overview,
                    "Overview".into(),
                    para.trim().into(),
                    "",
                );
            }
            for s in &part.sections {
                let file = PathBuf::from(&part.project).join(format!("{}.md", s.id));
                let u = self.unit(Unit {
                    book: b.id.clone(),
                    project: part.project.clone(),
                    id: format!("{}/{}", part.project, s.id),
                    kind: UnitKind::Section,
                    title: s.title.trim().to_string(),
                    file: file.clone(),
                    code_paths: s.code_refs.iter().map(|r| r.path.clone()).collect(),
                    title_tf: HashMap::new(),
                });
                self.body(
                    u,
                    &Body {
                        purpose: &s.purpose,
                        boundaries: &s.boundaries,
                        assumptions: &s.assumptions,
                        business_flow: &s.business_flow,
                        diagrams: &s.diagrams,
                        tables: &s.tables,
                        concerns: &s.concerns,
                        code_refs: &s.code_refs,
                    },
                );
                for sub in &s.subsections {
                    let u = self.unit(Unit {
                        book: b.id.clone(),
                        project: part.project.clone(),
                        id: format!("{}/{}/{}", part.project, s.id, sub.id),
                        kind: UnitKind::Subsection,
                        title: format!("{} > {}", s.title.trim(), sub.title.trim()),
                        file: file.clone(),
                        code_paths: sub.code_refs.iter().map(|r| r.path.clone()).collect(),
                        title_tf: HashMap::new(),
                    });
                    self.body(
                        u,
                        &Body {
                            purpose: &sub.purpose,
                            boundaries: &sub.boundaries,
                            assumptions: &sub.assumptions,
                            business_flow: &sub.business_flow,
                            diagrams: &sub.diagrams,
                            tables: &sub.tables,
                            concerns: &sub.concerns,
                            code_refs: &sub.code_refs,
                        },
                    );
                }
            }
        }
        for g in &b.glossary {
            let u = self.unit(Unit {
                book: b.id.clone(),
                project: String::new(),
                id: format!("glossary/{}", g.term.trim().to_lowercase()),
                kind: UnitKind::Glossary,
                title: format!("Glossary: {}", g.term.trim()),
                file: "glossary.md".into(),
                code_paths: g.code_ref.iter().cloned().collect(),
                title_tf: HashMap::new(),
            });
            let code = g.code_ref.clone().unwrap_or_default();
            let text = match &g.code_ref {
                Some(c) => format!("{} (`{c}`)", g.definition.trim()),
                None => g.definition.trim().to_string(),
            };
            self.passage(u, PassageKind::Glossary, "Definition".into(), text, &code);
        }
        if let Some(a) = &b.architecture {
            let arch = |aspect: &str, title: &str| Unit {
                book: b.id.clone(),
                project: String::new(),
                id: format!("architecture/{aspect}"),
                kind: UnitKind::Architecture,
                title: format!("System architecture > {title}"),
                file: "architecture.md".into(),
                code_paths: vec![],
                title_tf: HashMap::new(),
            };
            let u = self.unit(arch("overview", "Overview"));
            for para in a.overview.split("\n\n") {
                self.passage(
                    u,
                    PassageKind::Architecture,
                    "Overview".into(),
                    para.trim().into(),
                    "",
                );
            }
            self.diagram(u, &a.diagram);
            let u = self.unit(arch("components", "Components"));
            for c in &a.components {
                let project = if c.project.is_empty() {
                    "external".into()
                } else {
                    format!("project `{}`", c.project)
                };
                let mut text = format!("{} ({project}): {}", c.name, c.role.trim());
                if !c.owns.is_empty() {
                    text.push_str(&format!("\nOwns: {}", c.owns.join("; ")));
                }
                self.passage(
                    u,
                    PassageKind::Architecture,
                    format!("Component: {}", c.name),
                    text,
                    &c.project,
                );
            }
            let u = self.unit(arch("communication", "Communication"));
            for chunk in a.links.chunks(WINDOW) {
                let text = chunk
                    .iter()
                    .map(|l| {
                        let mode = match l.mode {
                            LinkMode::Sync => "sync",
                            LinkMode::Async => "async",
                        };
                        format!(
                            "- {} to {} over {} ({mode}): {}",
                            l.from, l.to, l.protocol, l.payload
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.passage(u, PassageKind::Architecture, "Links".into(), text, "");
            }
            let u = self.unit(arch("failure", "Failure and recovery"));
            for f in &a.failure_recovery {
                let text = format!(
                    "{}\nDetection: {}\nRecovery: {}",
                    f.failure, f.detection, f.recovery
                );
                self.passage(
                    u,
                    PassageKind::Architecture,
                    "Failure case".into(),
                    text,
                    "",
                );
            }
            let u = self.unit(arch("scalability", "Scalability"));
            for chunk in a.scalability.chunks(WINDOW) {
                let text = chunk
                    .iter()
                    .map(|s| {
                        format!(
                            "- {} scales by {}; limit: {}",
                            s.component, s.scales_by, s.limit
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.passage(u, PassageKind::Architecture, "Scaling".into(), text, "");
            }
        }
    }

    fn body(&mut self, u: usize, b: &Body) {
        for para in b.purpose.split("\n\n") {
            self.passage(
                u,
                PassageKind::Purpose,
                "Purpose".into(),
                para.trim().into(),
                "",
            );
        }
        let owns = b
            .boundaries
            .owns
            .iter()
            .map(|o| format!("- Owns: {}", o.trim()));
        let not = b
            .boundaries
            .does_not_own
            .iter()
            .map(|o| format!("- Does not own: {}", o.trim()));
        let lines: Vec<String> = owns.chain(not).collect();
        for chunk in lines.chunks(WINDOW) {
            self.passage(
                u,
                PassageKind::Boundaries,
                "Boundaries".into(),
                chunk.join("\n"),
                "",
            );
        }
        let lines: Vec<String> = b
            .assumptions
            .iter()
            .map(|a| format!("- {}", a.trim()))
            .collect();
        for chunk in lines.chunks(WINDOW) {
            self.passage(
                u,
                PassageKind::Assumptions,
                "Assumptions".into(),
                chunk.join("\n"),
                "",
            );
        }
        let lines: Vec<String> = b
            .business_flow
            .iter()
            .enumerate()
            .map(|(i, s)| {
                format!(
                    "{}. {}: {} Then: {}",
                    i + 1,
                    s.actor.trim(),
                    s.action.trim(),
                    s.outcome.trim()
                )
            })
            .collect();
        for chunk in lines.chunks(WINDOW) {
            self.passage(
                u,
                PassageKind::BusinessFlow,
                "Business flow".into(),
                chunk.join("\n"),
                "",
            );
        }
        for d in b.diagrams {
            self.diagram(u, d);
        }
        for t in b.tables {
            let head = t.columns.join(" | ");
            for chunk in t.rows.chunks(WINDOW) {
                let rows = chunk
                    .iter()
                    .map(|r| format!("- {}", r.join(" | ")))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.passage(
                    u,
                    PassageKind::Table,
                    format!("Table: {} ({head})", t.title.trim()),
                    rows,
                    "",
                );
            }
        }
        let lines: Vec<String> = b
            .concerns
            .iter()
            .map(|c| format!("- {}: {}", c.component.trim(), c.responsibility.trim()))
            .collect();
        for chunk in lines.chunks(WINDOW) {
            self.passage(
                u,
                PassageKind::Concerns,
                "Separation of concerns".into(),
                chunk.join("\n"),
                "",
            );
        }
        for chunk in b.code_refs.chunks(WINDOW) {
            let text = chunk
                .iter()
                .map(code_ref_line)
                .collect::<Vec<_>>()
                .join("\n");
            let code = chunk
                .iter()
                .map(|r| format!("{} {}", r.path, r.symbol.as_deref().unwrap_or("")))
                .collect::<Vec<_>>()
                .join(" ");
            let notes = chunk
                .iter()
                .map(|r| r.note.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            // Notes rank as body text; paths and symbols rank in the code field.
            self.passage_with_body(
                u,
                PassageKind::CodeRefs,
                "Code references".into(),
                text,
                &code,
                &notes,
            );
        }
    }

    fn passage_with_body(
        &mut self,
        unit: usize,
        kind: PassageKind,
        label: String,
        text: String,
        code: &str,
        body: &str,
    ) {
        let title_tf = tf(&label);
        let code_tf = tf(code);
        let body_tf = tf(body);
        let len = W_TITLE * total(&title_tf) + W_CODE * total(&code_tf) + W_BODY * total(&body_tf);
        self.passages.push(Passage {
            unit,
            kind,
            label,
            text,
            title_tf,
            code_tf,
            body_tf,
            len,
        });
    }

    fn diagram(&mut self, u: usize, d: &Diagram) {
        let text = format!("```mermaid\n{}\n```", d.source.trim());
        self.passage(
            u,
            PassageKind::Diagram,
            format!("Diagram: {}", d.title.trim()),
            text,
            "",
        );
    }

    fn finish(self) -> Index {
        let mut df: HashMap<String, u32> = HashMap::new();
        for p in &self.passages {
            let mut seen: Vec<&String> = p
                .title_tf
                .keys()
                .chain(p.code_tf.keys())
                .chain(p.body_tf.keys())
                .collect();
            seen.sort();
            seen.dedup();
            for t in seen {
                *df.entry(t.clone()).or_default() += 1;
            }
        }
        for u in &self.units {
            for t in u.title_tf.keys() {
                *df.entry(t.clone()).or_default() += 1;
            }
        }
        let avg_len = if self.passages.is_empty() {
            1.0
        } else {
            self.passages.iter().map(|p| p.len).sum::<f32>() / self.passages.len() as f32
        };
        let mut unit_tf: Vec<HashMap<String, f32>> = vec![HashMap::new(); self.units.len()];
        for (i, u) in self.units.iter().enumerate() {
            for (t, c) in &u.title_tf {
                *unit_tf[i].entry(t.clone()).or_default() += W_TITLE * *c as f32;
            }
        }
        for p in &self.passages {
            let m = &mut unit_tf[p.unit];
            for (f, w) in [
                (&p.title_tf, W_TITLE),
                (&p.code_tf, W_CODE),
                (&p.body_tf, W_BODY),
            ] {
                for (t, c) in f {
                    *m.entry(t.clone()).or_default() += w * *c as f32;
                }
            }
        }
        let unit_len: Vec<f32> = unit_tf.iter().map(|m| m.values().sum()).collect();
        let unit_avg_len = (unit_len.iter().sum::<f32>() / unit_len.len().max(1) as f32).max(1.0);
        Index {
            units: self.units,
            passages: self.passages,
            df,
            avg_len: avg_len.max(1.0),
            unit_tf,
            unit_len,
            unit_avg_len,
        }
    }
}

struct Body<'a> {
    purpose: &'a str,
    boundaries: &'a Boundaries,
    assumptions: &'a [String],
    business_flow: &'a [BusinessStep],
    diagrams: &'a [Diagram],
    tables: &'a [DocTable],
    concerns: &'a [Concern],
    code_refs: &'a [CodeRef],
}

fn code_ref_line(r: &CodeRef) -> String {
    let mut s = format!("- `{}`", r.path);
    if let Some(sym) = &r.symbol {
        s.push_str(&format!(" `{sym}`"));
    }
    if let Some(l) = &r.lines {
        s.push_str(&format!(" lines {l}"));
    }
    s.push_str(&format!(": {}", r.note.trim()));
    s
}

const MERMAID_WORDS: &[&str] = &[
    "sequencediagram",
    "flowchart",
    "graph",
    "participant",
    "actor",
    "td",
    "lr",
    "tb",
    "bt",
    "rl",
    "alt",
    "else",
    "end",
    "loop",
    "note",
    "over",
    "left",
    "right",
    "activate",
    "deactivate",
    "subgraph",
    "autonumber",
    "opt",
    "par",
    "rect",
    "critical",
    "break",
    "box",
    "mermaid",
];

/// The words a diagram shows: its labels and messages, without Mermaid keywords or bare node IDs.
fn diagram_words(source: &str) -> String {
    source
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| {
            let l = w.to_lowercase();
            w.len() > 2 && !MERMAID_WORDS.contains(&l.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

const STOP: &[&str] = &[
    "a", "an", "the", "and", "or", "of", "to", "in", "on", "for", "by", "with", "at", "from", "as",
    "is", "are", "was", "were", "be", "been", "it", "its", "this", "that", "these", "those",
    "which", "what", "who", "whom", "how", "why", "when", "where", "does", "do", "did", "can",
    "could", "should", "would", "will", "into", "than", "then", "there", "their", "they", "them",
    "not", "no", "if", "so", "but", "any", "all", "each", "every", "has", "have", "had", "one",
    "two", "about", "after", "before", "only", "also", "such", "we", "you", "our", "your", "i",
    "me", "my", "get", "gets", "use", "used", "uses",
];

/// A light suffix stripper: plurals, `-ed`, `-ing`, and a final `e`, so "started", "starts", and "start"
/// meet, and "caching" meets "cache". Words with digits or underscores stay as written.
fn stem(w: &str) -> String {
    if !w.bytes().all(|b| b.is_ascii_lowercase()) {
        return w.to_string();
    }
    let mut s = w.to_string();
    let n = s.len();
    if n > 4 && (s.ends_with("ies") || s.ends_with("ied")) {
        s.truncate(n - 3);
        s.push('y');
        return s;
    }
    if n > 4 && s.ends_with("sses") {
        s.truncate(n - 2);
    } else if n > 3
        && s.ends_with('s')
        && !s.ends_with("ss")
        && !s.ends_with("us")
        && !s.ends_with("is")
    {
        s.truncate(n - 1);
    }
    let has_vowel = |t: &str| t.bytes().any(|b| b"aeiouy".contains(&b));
    for suffix in ["ing", "ed"] {
        if s.len() >= suffix.len() + 3
            && s.ends_with(suffix)
            && has_vowel(&s[..s.len() - suffix.len()])
        {
            s.truncate(s.len() - suffix.len());
            let b = s.as_bytes();
            let l = b.len();
            if l >= 2 && b[l - 1] == b[l - 2] && !b"aeiouylsz".contains(&b[l - 1]) {
                s.truncate(l - 1);
            }
            break;
        }
    }
    if s.len() > 4 && s.ends_with('e') {
        s.truncate(s.len() - 1);
    }
    s
}

/// Lowercased, stemmed words. An identifier also yields its camelCase and snake_case parts, so
/// `SessionState` matches "session state".
pub fn tokens(text: &str) -> Vec<String> {
    let mut out = vec![];
    for run in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if run.is_empty() {
            continue;
        }
        let parts = split_ident(run);
        let whole = run.to_lowercase();
        if parts.len() > 1 && whole.len() > 1 {
            out.push(stem(&whole));
        }
        for p in parts {
            let l = p.to_lowercase();
            if l.len() > 1 && !STOP.contains(&l.as_str()) {
                out.push(stem(&l));
            }
        }
    }
    out
}

fn split_ident(run: &str) -> Vec<&str> {
    let mut parts = vec![];
    for seg in run.split('_').filter(|s| !s.is_empty()) {
        let cs: Vec<(usize, char)> = seg.char_indices().collect();
        let mut start = 0;
        for i in 1..cs.len() {
            let (bi, c) = cs[i];
            let prev = cs[i - 1].1;
            let next_lower = cs.get(i + 1).is_some_and(|(_, n)| n.is_lowercase());
            let boundary = (c.is_uppercase() && (prev.is_lowercase() || prev.is_ascii_digit()))
                || (c.is_uppercase() && prev.is_uppercase() && next_lower);
            if boundary {
                parts.push(&seg[start..bi]);
                start = bi;
            }
        }
        parts.push(&seg[start..]);
    }
    parts
}

fn tf(text: &str) -> HashMap<String, u32> {
    let mut m = HashMap::new();
    for t in tokens(text) {
        *m.entry(t).or_default() += 1;
    }
    m
}

fn total(m: &HashMap<String, u32>) -> f32 {
    m.values().sum::<u32>() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::*;

    fn section(id: &str, title: &str, purpose: &str, refs: &[&str]) -> DocSection {
        DocSection {
            id: id.into(),
            title: title.into(),
            purpose: purpose.into(),
            boundaries: Boundaries::default(),
            assumptions: vec![],
            business_flow: vec![],
            diagrams: vec![],
            tables: vec![],
            concerns: vec![],
            code_refs: refs
                .iter()
                .map(|p| CodeRef {
                    path: (*p).into(),
                    symbol: None,
                    lines: None,
                    note: "Holds it.".into(),
                })
                .collect(),
            subsections: vec![],
        }
    }

    fn book_of(sections: Vec<DocSection>) -> Book {
        Book {
            id: "app".into(),
            title: "Documentation for app".into(),
            projects: vec!["app".into()],
            updated_at: chrono::Utc::now(),
            sessions: vec![],
            architecture: None,
            parts: vec![BookPart {
                project: "app".into(),
                overview: "The app.".into(),
                sections,
                updated_at: chrono::Utc::now(),
                areas: vec![],
            }],
            glossary: vec![],
        }
    }

    #[test]
    fn identifiers_split_into_words() {
        assert_eq!(
            tokens("SessionState::fold"),
            ["sessionstat", "session", "stat", "fold"]
        );
        assert_eq!(
            tokens("max_parallel_executions"),
            ["max_parallel_executions", "max", "parallel", "execution"]
        );
        assert_eq!(
            tokens("started starts running stopped caching cache retried"),
            ["start", "start", "run", "stop", "cach", "cach", "retry"]
        );
        assert_eq!(tokens("HTTPServer"), ["httpserver", "http", "server"]);
    }

    #[test]
    fn ranks_the_section_that_matches_and_shows_only_its_matching_passage() {
        let mut slots = section(
            "slots",
            "Execution slots",
            "Caps concurrent executions per workspace.",
            &["src/runner.rs"],
        );
        slots.assumptions = vec!["The budget is checked before a spawn.".into()];
        let b = book_of(vec![
            slots,
            section(
                "auth",
                "Sign-in",
                "Checks the cookie and the Origin header.",
                &["src/auth.rs"],
            ),
        ]);
        let ix = Index::build(&[b]);
        let hits = ix.search(
            "how many executions run concurrently",
            &Filter::default(),
            5,
        );
        assert_eq!(ix.units[hits[0].unit].id, "app/slots");
        assert_eq!(hits[0].passages.len(), 1);
        assert_eq!(
            ix.passages[hits[0].passages[0].0].kind,
            PassageKind::Purpose
        );
        let hits = ix.search("auth.rs", &Filter::default(), 5);
        assert_eq!(ix.units[hits[0].unit].id, "app/auth");
    }

    #[test]
    fn diagram_syntax_does_not_match() {
        let mut s = section("flow", "Flow", "Moves orders.", &[]);
        s.diagrams = vec![Diagram {
            title: "Order hand-off".into(),
            kind: DiagramKind::Sequence,
            source: "sequenceDiagram\n  participant A as Cart\n  A->>B: submit order".into(),
        }];
        let ix = Index::build(&[book_of(vec![s])]);
        assert!(ix.search("participant", &Filter::default(), 5).is_empty());
        let hits = ix.search("cart submit", &Filter::default(), 5);
        assert_eq!(
            ix.passages[hits[0].passages[0].0].kind,
            PassageKind::Diagram
        );
    }

    #[test]
    fn project_filter_applies() {
        let ix = Index::build(&[book_of(vec![section("x", "Orders", "Orders.", &[])])]);
        let f = Filter {
            book: None,
            project: Some("other"),
        };
        assert!(ix.search("orders", &f, 5).is_empty());
    }
}
