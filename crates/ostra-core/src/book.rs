//! Documentation books (HANDOVER 8.5): what the docs stage writes into the workspace. A book
//! covers a set of projects. Each project is one part of sections and sub-sections, and a book of
//! two or more projects also has a system architecture. The engine writes every book file from the
//! agents' submit payloads; no agent writes one (Rule B5).

use crate::paths;
use crate::submit::{StuckInfo, SubmitStatus};
use chrono::{DateTime, Utc};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use ts_rs::TS;

/// Rule B3: participants a sequence diagram may show.
pub const MAX_SEQUENCE_PARTICIPANTS: usize = 8;
/// Rule B3: messages a sequence diagram may show.
pub const MAX_SEQUENCE_MESSAGES: usize = 20;
/// Rule B3: nodes a flowchart may show.
pub const MAX_FLOWCHART_NODES: usize = 15;
/// Longest book ID, so a book folder name stays short on every file system.
pub const MAX_BOOK_ID: usize = 80;
/// Rule B9: tracked source one documentation writer covers. A project at or under it gets one
/// writer, because the one-writer book of a large repository answered 127 of 221 questions and
/// the book written per crate answered 220. A writer given six small crates in 858 KB wrote only
/// 7 sections, so the target stays well under that.
pub const AREA_TARGET_BYTES: u64 = 384 * 1024;
/// Rule B9: writers one project's part may fan out to. On a repository larger than
/// `AREA_TARGET_BYTES * MAX_DOCS_AREAS` the cap, not the target, sets each writer's share.
pub const MAX_DOCS_AREAS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum DiagramKind {
    Sequence,
    Flowchart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct Diagram {
    /// What the diagram shows, in a few words.
    pub title: String,
    pub kind: DiagramKind,
    /// Mermaid source. Starts with `sequenceDiagram` for a sequence and `flowchart` for a flowchart.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema, Default)]
#[ts(export)]
pub struct Boundaries {
    /// What this part of the code is responsible for.
    #[serde(default)]
    pub owns: Vec<String>,
    /// What it leaves to other parts, each naming the part that owns it.
    #[serde(default)]
    pub does_not_own: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct BusinessStep {
    /// Who acts: a user role, a service, or a component.
    pub actor: String,
    pub action: String,
    /// What is true after the step.
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct DocTable {
    pub title: String,
    pub columns: Vec<String>,
    /// Each row has one cell per column.
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct Concern {
    pub component: String,
    pub responsibility: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct CodeRef {
    /// Project-relative path, `/`-separated.
    pub path: String,
    #[serde(default)]
    pub symbol: Option<String>,
    /// A line or range, for example `40` or `40-72`.
    #[serde(default)]
    pub lines: Option<String>,
    /// What the reader finds there.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct GlossaryEntry {
    pub term: String,
    pub definition: String,
    /// Where the term lives in code, when it names a type or module.
    #[serde(default)]
    pub code_ref: Option<String>,
}

/// One unit of work in a section. Same fields as a section, without sub-sections (Rule B1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct DocSubsection {
    /// Lowercase slug, unique in the submit.
    pub id: String,
    pub title: String,
    pub purpose: String,
    #[serde(default)]
    pub boundaries: Boundaries,
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub business_flow: Vec<BusinessStep>,
    #[serde(default)]
    pub diagrams: Vec<Diagram>,
    #[serde(default)]
    pub tables: Vec<DocTable>,
    #[serde(default)]
    pub concerns: Vec<Concern>,
    #[serde(default)]
    pub code_refs: Vec<CodeRef>,
}

/// One work section of a project (Rule B1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct DocSection {
    /// Lowercase slug, unique in the submit.
    pub id: String,
    pub title: String,
    pub purpose: String,
    #[serde(default)]
    pub boundaries: Boundaries,
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub business_flow: Vec<BusinessStep>,
    #[serde(default)]
    pub diagrams: Vec<Diagram>,
    #[serde(default)]
    pub tables: Vec<DocTable>,
    #[serde(default)]
    pub concerns: Vec<Concern>,
    #[serde(default)]
    pub code_refs: Vec<CodeRef>,
    #[serde(default)]
    pub subsections: Vec<DocSubsection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum LinkMode {
    Sync,
    Async,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct Component {
    /// Unique among the components; links name it.
    pub name: String,
    /// The workspace project that holds it, or empty for an external system.
    #[serde(default)]
    pub project: String,
    pub role: String,
    /// Data and decisions only this component changes.
    #[serde(default)]
    pub owns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct Link {
    pub from: String,
    pub to: String,
    /// For example `HTTP/JSON`, `gRPC`, `Kafka topic orders.v1`, `SQL`.
    pub protocol: String,
    pub mode: LinkMode,
    /// What travels over the link.
    pub payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct FailureCase {
    pub failure: String,
    pub detection: String,
    pub recovery: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct ScalingCase {
    pub component: String,
    pub scales_by: String,
    /// The first limit it reaches, and what bounds it.
    pub limit: String,
}

/// Rule B4: how the projects of a book work together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct Architecture {
    pub overview: String,
    /// A flowchart of the components and their links.
    pub diagram: Diagram,
    pub components: Vec<Component>,
    pub links: Vec<Link>,
    pub failure_recovery: Vec<FailureCase>,
    pub scalability: Vec<ScalingCase>,
}

/// `submit_documentation`: one project's part of the book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct DocumentationSubmit {
    pub status: SubmitStatus,
    /// Two or three sentences for the user: what the part covers and what changed in it.
    pub summary: String,
    /// The project's introduction: what it does, for whom, and its main parts.
    #[serde(default)]
    pub overview: String,
    #[serde(default)]
    pub sections: Vec<DocSection>,
    #[serde(default)]
    pub glossary: Vec<GlossaryEntry>,
    #[serde(default)]
    pub stuck: Option<StuckInfo>,
}

/// `submit_system_architecture`: the book's architecture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct ArchitectureSubmit {
    pub status: SubmitStatus,
    pub summary: String,
    #[serde(default)]
    pub architecture: Option<Architecture>,
    #[serde(default)]
    pub glossary: Vec<GlossaryEntry>,
    #[serde(default)]
    pub stuck: Option<StuckInfo>,
}

/// One project's part of a book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BookPart {
    pub project: String,
    pub overview: String,
    pub sections: Vec<DocSection>,
    #[ts(type = "string")]
    pub updated_at: DateTime<Utc>,
    /// Rule B9: the areas that wrote the part, in order, when more than one writer did.
    #[serde(default)]
    pub areas: Vec<BookArea>,
}

/// Rule B9: one writer's slice of a large project, grouped from its module map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocsArea {
    /// Lowercase slug, unique in the project.
    pub id: String,
    /// The module-map areas it groups, joined with commas.
    pub title: String,
    /// Module-map globs the area covers.
    pub globs: Vec<String>,
    /// The area also covers every file no other area's globs match.
    #[serde(default)]
    pub rest: bool,
    /// Bytes of tracked source it held when the stage was planned.
    #[ts(type = "number")]
    pub bytes: u64,
}

/// Rule B9: which sections of a part one area wrote, so a later session that rewrites some areas
/// keeps the sections of the others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BookArea {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub overview: String,
    /// Section IDs, in the part's order.
    pub sections: Vec<String>,
}

/// A documentation book as `book.json` holds it and `GET /api/workspaces/{ws}/docs/{book}` returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Book {
    pub id: String,
    pub title: String,
    /// Sorted project keys.
    pub projects: Vec<String>,
    #[ts(type = "string")]
    pub updated_at: DateTime<Utc>,
    /// Sessions that wrote the book, oldest first.
    #[serde(default)]
    pub sessions: Vec<String>,
    #[serde(default)]
    pub architecture: Option<Architecture>,
    #[serde(default)]
    pub parts: Vec<BookPart>,
    /// Sorted by term.
    #[serde(default)]
    pub glossary: Vec<GlossaryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BookSummary {
    pub id: String,
    pub title: String,
    pub projects: Vec<String>,
    #[ts(type = "string")]
    pub updated_at: DateTime<Utc>,
    pub sections: u32,
    pub has_architecture: bool,
}

impl Book {
    pub fn summary(&self) -> BookSummary {
        BookSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            projects: self.projects.clone(),
            updated_at: self.updated_at,
            sections: self.parts.iter().map(|p| p.sections.len() as u32).sum(),
            has_architecture: self.architecture.is_some(),
        }
    }
}

/// What one session adds to a book, all of it from the event log.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BookUpdate {
    pub session: String,
    /// Each documented project's part as this session wrote it. For a project split into areas,
    /// only the areas it rewrote, combined (see [`combine_areas`]).
    pub parts: Vec<(String, DocumentationSubmit)>,
    /// Rule B9: for each project split into areas, every planned area in order with the submit
    /// this session wrote for it, or `None` to keep the area's sections from the book.
    pub areas: BTreeMap<String, Vec<(DocsArea, Option<DocumentationSubmit>)>>,
    pub architecture: Option<Architecture>,
    pub glossary: Vec<GlossaryEntry>,
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    let cut = out
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= 40)
        .last()
        .unwrap_or(0);
    let out = if out.len() > 40 { &out[..cut] } else { out };
    if out.is_empty() {
        "area".into()
    } else {
        out.trim_end_matches('-').to_string()
    }
}

/// Rule B9: group a project's module-map areas into writer areas of about [`AREA_TARGET_BYTES`]
/// of source each, keeping map order so neighbouring areas share a writer, at most
/// [`MAX_DOCS_AREAS`]. `areas` holds each module-map area with its globs and measured bytes;
/// `rest` is the bytes no glob matched. Empty means one writer for the whole project: the project
/// is small, or only one area holds source.
pub fn plan_areas(areas: &[(String, Vec<String>, u64)], rest: u64) -> Vec<DocsArea> {
    struct Member<'a> {
        name: &'a str,
        globs: &'a [String],
        bytes: u64,
        rest: bool,
    }
    let mut members: Vec<Member> = areas
        .iter()
        .filter(|(_, _, b)| *b > 0)
        .map(|(n, g, b)| Member {
            name: n,
            globs: g,
            bytes: *b,
            rest: false,
        })
        .collect();
    if rest > 0 {
        members.push(Member {
            name: "other files",
            globs: &[],
            bytes: rest,
            rest: true,
        });
    }
    let total: u64 = members.iter().map(|m| m.bytes).sum();
    if members.len() < 2 || total <= AREA_TARGET_BYTES {
        return vec![];
    }
    let pack = |target: u64| -> Vec<Vec<&Member>> {
        let mut groups: Vec<Vec<&Member>> = vec![];
        let mut size = 0;
        for m in &members {
            if groups.is_empty() || (size + m.bytes > target && size > 0) {
                groups.push(vec![]);
                size = 0;
            }
            groups.last_mut().unwrap().push(m);
            size += m.bytes;
        }
        groups
    };
    let mut target = AREA_TARGET_BYTES.max(total.div_ceil(MAX_DOCS_AREAS as u64));
    let mut groups = pack(target);
    while groups.len() > MAX_DOCS_AREAS {
        target += target / 4;
        groups = pack(target);
    }
    if groups.len() < 2 {
        return vec![];
    }
    let mut seen = BTreeSet::new();
    groups
        .into_iter()
        .map(|g| {
            let names: Vec<&str> = g.iter().map(|m| m.name).collect();
            let base = slug(names[0]);
            let mut id = if g.len() > 1 {
                format!("{base}-and-{}-more", g.len() - 1)
            } else {
                base
            };
            let mut n = 2;
            while !seen.insert(id.clone()) {
                id = format!(
                    "{}-{n}",
                    id.trim_end_matches(char::is_numeric).trim_end_matches('-')
                );
                n += 1;
            }
            DocsArea {
                id,
                title: names.join(", "),
                globs: g.iter().flat_map(|m| m.globs.iter().cloned()).collect(),
                rest: g.iter().any(|m| m.rest),
                bytes: g.iter().map(|m| m.bytes).sum(),
            }
        })
        .collect()
}

/// Rule B9: the areas' submits as one part: overviews in area order, each named after its area,
/// then every section, with a section ID another area already used suffixed with the area's ID.
pub fn combine_areas(areas: &[(&DocsArea, &DocumentationSubmit)]) -> DocumentationSubmit {
    let mut seen = BTreeSet::new();
    let mut sections = vec![];
    let mut glossary = vec![];
    let mut overviews = vec![];
    let mut summaries = vec![];
    for (area, sub) in areas {
        if !sub.overview.trim().is_empty() {
            overviews.push(format!("{}: {}", area.title, sub.overview.trim()));
        }
        summaries.push(sub.summary.trim().to_string());
        for s in &sub.sections {
            let mut s = s.clone();
            s.id = unique_id(&mut seen, &s.id, &area.id);
            sections.push(s);
        }
        glossary.extend(sub.glossary.iter().cloned());
    }
    DocumentationSubmit {
        status: SubmitStatus::Ok,
        summary: summaries.join(" "),
        overview: overviews.join("\n\n"),
        sections,
        glossary,
        stuck: None,
    }
}

fn unique_id(seen: &mut BTreeSet<String>, id: &str, area: &str) -> String {
    if seen.insert(id.to_string()) {
        return id.to_string();
    }
    let mut candidate = format!("{id}-{area}");
    let mut n = 2;
    while !seen.insert(candidate.clone()) {
        candidate = format!("{id}-{area}-{n}");
        n += 1;
    }
    candidate
}

/// Rule B9: a project's new part from its planned areas: each rewritten area's sections, and each
/// kept area's sections and overview from the old part.
fn merge_areas(
    project: &str,
    old: Option<&BookPart>,
    areas: &[(DocsArea, Option<DocumentationSubmit>)],
    now: DateTime<Utc>,
) -> BookPart {
    let mut seen = BTreeSet::new();
    let mut sections = vec![];
    let mut records = vec![];
    for (area, sub) in areas {
        let (overview, secs): (String, Vec<DocSection>) = match sub {
            Some(sub) => (sub.overview.trim().to_string(), sub.sections.clone()),
            None => {
                let Some(rec) = old.and_then(|p| p.areas.iter().find(|a| a.id == area.id)) else {
                    continue;
                };
                let old_secs = old.map(|p| p.sections.as_slice()).unwrap_or_default();
                (
                    rec.overview.clone(),
                    rec.sections
                        .iter()
                        .filter_map(|id| old_secs.iter().find(|s| &s.id == id).cloned())
                        .collect(),
                )
            }
        };
        let mut ids = vec![];
        for mut s in secs {
            s.id = unique_id(&mut seen, &s.id, &area.id);
            ids.push(s.id.clone());
            sections.push(s);
        }
        records.push(BookArea {
            id: area.id.clone(),
            title: area.title.clone(),
            overview,
            sections: ids,
        });
    }
    let overview = records
        .iter()
        .filter(|r| !r.overview.is_empty())
        .map(|r| format!("{}: {}", r.title, r.overview))
        .collect::<Vec<_>>()
        .join("\n\n");
    BookPart {
        project: project.to_string(),
        overview,
        sections,
        updated_at: now,
        areas: records,
    }
}

/// Where a workspace keeps its books.
pub fn dir(workspace: &Path) -> PathBuf {
    paths::workspace_runtime(workspace).join("docs")
}

pub fn book_dir(workspace: &Path, id: &str) -> PathBuf {
    dir(workspace).join(id)
}

pub fn book_json(workspace: &Path, id: &str) -> PathBuf {
    book_dir(workspace, id).join("book.json")
}

/// Rule B6: a book is named by its project set, so a later session on the same projects updates it.
pub fn book_id(projects: &[String]) -> String {
    let mut keys: Vec<&str> = projects.iter().map(String::as_str).collect();
    keys.sort_unstable();
    keys.dedup();
    // Project keys hold no `_`, so joining with it keeps different sets apart.
    let id = keys.join("_");
    if id.len() <= MAX_BOOK_ID {
        return id;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let cut = id
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= MAX_BOOK_ID - 17)
        .last()
        .unwrap_or(0);
    format!("{}_{hash:016x}", &id[..cut])
}

pub fn is_book_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.len() <= MAX_BOOK_ID
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn read(workspace: &Path, id: &str) -> Option<Book> {
    let text = std::fs::read_to_string(book_json(workspace, id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Every readable book, newest first.
pub fn list(workspace: &Path) -> Vec<BookSummary> {
    let Ok(read_dir) = std::fs::read_dir(dir(workspace)) else {
        return vec![];
    };
    let mut out: Vec<BookSummary> = read_dir
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| is_book_id(n))
        .filter_map(|n| read(workspace, &n))
        .map(|b| b.summary())
        .collect();
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
    out
}

/// Rule B6: a project's new part replaces its old one, a new architecture replaces the old one, and
/// glossary entries merge by term with the newer definition kept.
pub fn merge(existing: Option<Book>, id: &str, update: &BookUpdate, now: DateTime<Utc>) -> Book {
    let mut book = existing.unwrap_or_else(|| Book {
        id: id.to_string(),
        title: String::new(),
        projects: vec![],
        updated_at: now,
        sessions: vec![],
        architecture: None,
        parts: vec![],
        glossary: vec![],
    });
    for (project, submit) in &update.parts {
        let old = book
            .parts
            .iter()
            .position(|p| &p.project == project)
            .map(|i| book.parts.remove(i));
        let part = match update.areas.get(project) {
            Some(areas) => merge_areas(project, old.as_ref(), areas, now),
            None => BookPart {
                project: project.clone(),
                overview: submit.overview.clone(),
                sections: submit.sections.clone(),
                updated_at: now,
                areas: vec![],
            },
        };
        book.parts.push(part);
    }
    book.parts.sort_by(|a, b| a.project.cmp(&b.project));
    if let Some(a) = &update.architecture {
        book.architecture = Some(a.clone());
    }
    let incoming = update
        .parts
        .iter()
        .flat_map(|(_, s)| s.glossary.iter())
        .chain(update.glossary.iter());
    for entry in incoming {
        let key = entry.term.trim().to_lowercase();
        book.glossary
            .retain(|g| g.term.trim().to_lowercase() != key);
        book.glossary.push(entry.clone());
    }
    book.glossary.sort_by_key(|g| g.term.to_lowercase());
    let projects: BTreeSet<String> = book.parts.iter().map(|p| p.project.clone()).collect();
    book.projects = projects.into_iter().collect();
    book.title = format!("Documentation for {}", book.projects.join(", "));
    if !book.sessions.contains(&update.session) {
        book.sessions.push(update.session.clone());
    }
    book.updated_at = now;
    book
}

/// Serializes every read-merge-write and removal of a book, so two sessions updating one book keep
/// each other's parts (Rule B6).
static BOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Merge `update` into the book on disk and write it, holding the book lock across the read and
/// the write.
pub fn apply(
    workspace: &Path,
    id: &str,
    update: &BookUpdate,
    now: DateTime<Utc>,
) -> std::io::Result<Book> {
    let _guard = BOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let merged = merge(read(workspace, id), id, update, now);
    write(workspace, &merged)?;
    Ok(merged)
}

pub fn remove(workspace: &Path, id: &str) -> std::io::Result<()> {
    let _guard = BOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::fs::remove_dir_all(book_dir(workspace, id))
}

/// Write `book.json` plus the Markdown agents read: `index.md`, `architecture.md`, `glossary.md`,
/// and one file per section under `<project>/`. Files of sections that no longer exist are removed.
pub fn write(workspace: &Path, book: &Book) -> std::io::Result<()> {
    let root = book_dir(workspace, &book.id);
    std::fs::create_dir_all(&root)?;
    let mut keep: BTreeSet<PathBuf> = BTreeSet::new();
    let mut put = |rel: PathBuf, text: String| -> std::io::Result<()> {
        let path = root.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, text)?;
        keep.insert(rel);
        Ok(())
    };
    put(
        "book.json".into(),
        serde_json::to_string_pretty(book).map_err(std::io::Error::other)?,
    )?;
    put("index.md".into(), render_index(book))?;
    put("glossary.md".into(), render_glossary(&book.glossary))?;
    if let Some(a) = &book.architecture {
        put("architecture.md".into(), render_architecture(a))?;
    }
    for part in &book.parts {
        for s in &part.sections {
            put(
                Path::new(&part.project).join(format!("{}.md", s.id)),
                render_section(&part.project, s),
            )?;
        }
    }
    remove_stale(&root, &root, &keep)
}

fn remove_stale(root: &Path, dir: &Path, keep: &BTreeSet<PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)?.flatten() {
        let path = e.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        if e.file_type()?.is_dir() {
            remove_stale(root, &path, keep)?;
            if std::fs::read_dir(&path)?.next().is_none() {
                std::fs::remove_dir(&path)?;
            }
        } else if !keep.contains(rel) {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// Markdown
// -----------------------------------------------------------------------------------------------

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn table(out: &mut String, columns: &[&str], rows: impl IntoIterator<Item = Vec<String>>) {
    out.push_str(&format!("| {} |\n", columns.join(" | ")));
    out.push_str(&format!("|{}\n", " --- |".repeat(columns.len())));
    for r in rows {
        let cells: Vec<String> = r.iter().map(|c| cell(c)).collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out.push('\n');
}

fn list_items(out: &mut String, items: &[String]) {
    for i in items {
        out.push_str(&format!("- {}\n", i.trim()));
    }
    out.push('\n');
}

fn diagram(out: &mut String, h: &str, d: &Diagram) {
    out.push_str(&format!(
        "{h} {}\n\n```mermaid\n{}\n```\n\n",
        d.title.trim(),
        d.source.trim()
    ));
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

impl<'a> From<&'a DocSection> for Body<'a> {
    fn from(s: &'a DocSection) -> Self {
        Body {
            purpose: &s.purpose,
            boundaries: &s.boundaries,
            assumptions: &s.assumptions,
            business_flow: &s.business_flow,
            diagrams: &s.diagrams,
            tables: &s.tables,
            concerns: &s.concerns,
            code_refs: &s.code_refs,
        }
    }
}

impl<'a> From<&'a DocSubsection> for Body<'a> {
    fn from(s: &'a DocSubsection) -> Self {
        Body {
            purpose: &s.purpose,
            boundaries: &s.boundaries,
            assumptions: &s.assumptions,
            business_flow: &s.business_flow,
            diagrams: &s.diagrams,
            tables: &s.tables,
            concerns: &s.concerns,
            code_refs: &s.code_refs,
        }
    }
}

/// Rule B1: the fixed order of a work section, with code references last.
fn render_body(out: &mut String, level: usize, b: &Body) {
    let h = "#".repeat(level);
    out.push_str(&format!("{}\n\n", b.purpose.trim()));
    if !b.boundaries.owns.is_empty() || !b.boundaries.does_not_own.is_empty() {
        out.push_str(&format!("{h} Boundaries\n\n"));
        if !b.boundaries.owns.is_empty() {
            out.push_str("Owns:\n\n");
            list_items(out, &b.boundaries.owns);
        }
        if !b.boundaries.does_not_own.is_empty() {
            out.push_str("Does not own:\n\n");
            list_items(out, &b.boundaries.does_not_own);
        }
    }
    out.push_str(&format!("{h} Assumptions\n\n"));
    list_items(out, b.assumptions);
    if !b.business_flow.is_empty() {
        out.push_str(&format!("{h} Business flow\n\n"));
        table(
            out,
            &["Step", "Actor", "Action", "Outcome"],
            b.business_flow.iter().enumerate().map(|(i, f)| {
                vec![
                    (i + 1).to_string(),
                    f.actor.clone(),
                    f.action.clone(),
                    f.outcome.clone(),
                ]
            }),
        );
    }
    for d in b.diagrams {
        diagram(out, &h, d);
    }
    for t in b.tables {
        out.push_str(&format!("{h} {}\n\n", t.title.trim()));
        let cols: Vec<&str> = t.columns.iter().map(String::as_str).collect();
        table(out, &cols, t.rows.iter().cloned());
    }
    if !b.concerns.is_empty() {
        out.push_str(&format!("{h} Separation of concerns\n\n"));
        table(
            out,
            &["Component", "Responsibility"],
            b.concerns
                .iter()
                .map(|c| vec![c.component.clone(), c.responsibility.clone()]),
        );
    }
    if !b.code_refs.is_empty() {
        out.push_str(&format!("{h} Code references\n\n"));
        table(
            out,
            &["Path", "Symbol", "Lines", "What it holds"],
            b.code_refs.iter().map(|r| {
                vec![
                    format!("`{}`", r.path),
                    r.symbol
                        .clone()
                        .map(|s| format!("`{s}`"))
                        .unwrap_or_default(),
                    r.lines.clone().unwrap_or_default(),
                    r.note.clone(),
                ]
            }),
        );
    }
}

pub fn render_section(project: &str, s: &DocSection) -> String {
    let mut out = format!("# {}\n\nProject: `{project}`\n\n", s.title.trim());
    render_body(&mut out, 2, &Body::from(s));
    for sub in &s.subsections {
        out.push_str(&format!("## {}\n\n", sub.title.trim()));
        render_body(&mut out, 3, &Body::from(sub));
    }
    out
}

pub fn render_architecture(a: &Architecture) -> String {
    let mut out = format!("# System architecture\n\n{}\n\n", a.overview.trim());
    diagram(&mut out, "##", &a.diagram);
    out.push_str("## Components\n\n");
    table(
        &mut out,
        &["Component", "Project", "Role", "Owns"],
        a.components.iter().map(|c| {
            vec![
                c.name.clone(),
                c.project.clone(),
                c.role.clone(),
                c.owns.join("; "),
            ]
        }),
    );
    out.push_str("## Communication\n\n");
    table(
        &mut out,
        &["From", "To", "Protocol", "Mode", "Payload"],
        a.links.iter().map(|l| {
            vec![
                l.from.clone(),
                l.to.clone(),
                l.protocol.clone(),
                match l.mode {
                    LinkMode::Sync => "sync".into(),
                    LinkMode::Async => "async".into(),
                },
                l.payload.clone(),
            ]
        }),
    );
    out.push_str("## Failure and recovery\n\n");
    table(
        &mut out,
        &["Failure", "Detection", "Recovery"],
        a.failure_recovery
            .iter()
            .map(|f| vec![f.failure.clone(), f.detection.clone(), f.recovery.clone()]),
    );
    out.push_str("## Scalability\n\n");
    table(
        &mut out,
        &["Component", "Scales by", "Limit"],
        a.scalability
            .iter()
            .map(|s| vec![s.component.clone(), s.scales_by.clone(), s.limit.clone()]),
    );
    out
}

pub fn render_glossary(glossary: &[GlossaryEntry]) -> String {
    let mut out = "# Glossary\n\n".to_string();
    table(
        &mut out,
        &["Term", "Definition", "Code"],
        glossary.iter().map(|g| {
            vec![
                g.term.clone(),
                g.definition.clone(),
                g.code_ref
                    .clone()
                    .map(|c| format!("`{c}`"))
                    .unwrap_or_default(),
            ]
        }),
    );
    out
}

pub fn render_index(book: &Book) -> String {
    let mut out = format!(
        "# {}\n\nWritten by Ostra's docs stage. Updated {}.\n\n- [Glossary](glossary.md)\n",
        book.title,
        book.updated_at.format("%Y-%m-%d %H:%M UTC")
    );
    if book.architecture.is_some() {
        out.push_str("- [System architecture](architecture.md)\n");
    }
    for part in &book.parts {
        out.push_str(&format!(
            "\n## {}\n\n{}\n\n",
            part.project,
            part.overview.trim()
        ));
        for s in &part.sections {
            out.push_str(&format!(
                "- [{}]({}/{}.md): {}\n",
                s.title.trim(),
                part.project,
                s.id,
                first_sentence(&s.purpose)
            ));
        }
    }
    out
}

fn first_sentence(s: &str) -> &str {
    let s = s.trim();
    match s.find(". ") {
        Some(i) => &s[..=i],
        None => s,
    }
}

// -----------------------------------------------------------------------------------------------
// Validation
// -----------------------------------------------------------------------------------------------

fn is_slug(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_relative(path: &str) -> bool {
    let p = path.trim();
    !p.is_empty()
        && !p.starts_with('/')
        && !p.starts_with('\\')
        && !p.starts_with('~')
        && p.chars().nth(1) != Some(':')
        && !p.split(['/', '\\']).any(|c| c == "..")
}

/// Rule B3: the Mermaid kind matches the declared kind and the diagram stays within its caps.
pub fn check_diagram(where_: &str, d: &Diagram, issues: &mut Vec<String>) {
    let mut lines = d
        .source
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("%%"));
    let head = lines.next().unwrap_or_default();
    let is_sequence = head == "sequenceDiagram";
    let is_flow = head.starts_with("flowchart") || head.starts_with("graph");
    match d.kind {
        DiagramKind::Sequence if !is_sequence => issues.push(format!(
            "Start the Mermaid source of {where_} diagram \"{}\" with `sequenceDiagram`, because its kind is sequence.",
            d.title
        )),
        DiagramKind::Flowchart if !is_flow => issues.push(format!(
            "Start the Mermaid source of {where_} diagram \"{}\" with `flowchart TD` or `flowchart LR`, because its kind is flowchart.",
            d.title
        )),
        DiagramKind::Sequence => {
            let (participants, messages) = sequence_size(lines);
            if participants > MAX_SEQUENCE_PARTICIPANTS || messages > MAX_SEQUENCE_MESSAGES {
                issues.push(format!(
                    "Split {where_} diagram \"{}\" into one diagram per step: it has {participants} participants and {messages} messages, and a sequence diagram may have at most {MAX_SEQUENCE_PARTICIPANTS} and {MAX_SEQUENCE_MESSAGES}, because a reader follows a small diagram and loses a large one.",
                    d.title
                ));
            }
        }
        DiagramKind::Flowchart => {
            let nodes = flowchart_nodes(lines);
            if nodes > MAX_FLOWCHART_NODES {
                issues.push(format!(
                    "Split {where_} diagram \"{}\" into smaller flowcharts: it has {nodes} nodes, and a flowchart may have at most {MAX_FLOWCHART_NODES}, because a reader follows a small diagram and loses a large one.",
                    d.title
                ));
            }
        }
    }
}

const SEQ_ARROWS: [&str; 10] = [
    "-->>", "->>", "<<->>", "<<-->>", "-->", "->", "--x", "-x", "--)", "-)",
];

fn sequence_size<'a>(lines: impl Iterator<Item = &'a str>) -> (usize, usize) {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut messages = 0;
    for l in lines {
        let first = l.split_whitespace().next().unwrap_or_default();
        if first == "participant" || first == "actor" {
            let rest = l[first.len()..].trim();
            let name = rest.split(" as ").next().unwrap_or(rest).trim();
            if !name.is_empty() {
                names.insert(name.to_string());
            }
            continue;
        }
        let Some(colon) = l.find(':') else { continue };
        let head = &l[..colon];
        let Some((at, arrow)) = SEQ_ARROWS
            .iter()
            .filter_map(|a| head.find(a).map(|i| (i, *a)))
            .min_by_key(|(i, a)| (*i, std::cmp::Reverse(a.len())))
        else {
            continue;
        };
        messages += 1;
        let from = head[..at].trim();
        let to = head[at + arrow.len()..]
            .trim()
            .trim_start_matches(['+', '-'])
            .trim();
        for n in [from, to] {
            if !n.is_empty() {
                names.insert(n.to_string());
            }
        }
    }
    (names.len(), messages)
}

fn strip_shapes(line: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut bar = false;
    let mut prev = ' ';
    for c in line.chars() {
        if quoted {
            quoted = c != '"';
        } else if bar {
            bar = c != '|';
        } else if depth > 0 {
            match c {
                '"' => quoted = true,
                '[' | '(' | '{' => depth += 1,
                ']' | ')' | '}' => depth -= 1,
                _ => {}
            }
        } else if matches!(c, '[' | '(' | '{')
            || (c == '>' && (prev.is_ascii_alphanumeric() || prev == '_'))
        {
            depth = 1;
        } else if c == '|' {
            bar = true;
        } else {
            out.push(c);
        }
        prev = c;
    }
    out
}

static INLINE_EDGE_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s(--|==|-\.)\s[^-=.>]+?\s?(-->|==>|\.->|---|===)").expect("valid regex")
});
static EDGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<?(-{2,}|={2,}|-\.+-)[>ox]?|~~~|&").expect("valid regex"));

fn flowchart_nodes<'a>(lines: impl Iterator<Item = &'a str>) -> usize {
    const SKIP: [&str; 8] = [
        "subgraph",
        "end",
        "style",
        "classDef",
        "class",
        "linkStyle",
        "click",
        "direction",
    ];
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    for l in lines {
        let first = l.split_whitespace().next().unwrap_or_default();
        if SKIP.contains(&first) {
            continue;
        }
        let bare = strip_shapes(l);
        let bare = INLINE_EDGE_TEXT.replace_all(&bare, " $2");
        for piece in EDGE.split(&bare) {
            let t = piece.trim().trim_end_matches(';');
            let id: String = t
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !id.is_empty() {
                nodes.insert(id);
            }
        }
    }
    nodes.len()
}

fn check_glossary(g: &[GlossaryEntry], issues: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    for e in g {
        if e.term.trim().is_empty() || e.definition.trim().is_empty() {
            issues.push("Give every glossary entry a term and a definition.".into());
        } else if !seen.insert(e.term.trim().to_lowercase()) {
            issues.push(format!(
                "List the glossary term \"{}\" once: it appears more than once.",
                e.term.trim()
            ));
        }
    }
}

fn check_body(where_: &str, b: &Body, issues: &mut Vec<String>) {
    if b.purpose.trim().is_empty() {
        issues.push(format!("Write the purpose of {where_}."));
    }
    // Rule B2: every unit of work states what it takes as given.
    if b.assumptions.iter().all(|a| a.trim().is_empty()) {
        issues.push(format!(
            "List the assumptions of {where_}: what the code takes as given about its inputs, callers, data, and environment. Write `None found in the code` only after checking, because readers and agents rely on this list."
        ));
    }
    for d in b.diagrams {
        check_diagram(where_, d, issues);
    }
    for t in b.tables {
        if t.columns.is_empty() || t.rows.iter().any(|r| r.len() != t.columns.len()) {
            issues.push(format!(
                "Give every row of table \"{}\" in {where_} one cell per column.",
                t.title
            ));
        }
    }
    for r in b.code_refs {
        if !is_relative(&r.path) {
            issues.push(format!(
                "Write code reference \"{}\" in {where_} as a path relative to the project root, with `/` separators and no `..`.",
                r.path
            ));
        }
    }
}

/// Validate a documentation submit. Each issue states the correction first.
pub fn check_documentation(s: &DocumentationSubmit) -> Vec<String> {
    let mut issues = vec![];
    if s.status != SubmitStatus::Ok {
        return issues;
    }
    if s.sections.is_empty() {
        issues.push("Return at least one section in `sections`, because the book shows only what you submit.".into());
    }
    let mut ids = BTreeSet::new();
    for sec in &s.sections {
        let where_ = format!("section `{}`", sec.id);
        if sec.title.trim().is_empty() {
            issues.push(format!("Give {where_} a title."));
        }
        check_body(&where_, &Body::from(sec), &mut issues);
        let all = std::iter::once(&sec.id).chain(sec.subsections.iter().map(|x| &x.id));
        for id in all {
            if !is_slug(id) {
                issues.push(format!(
                    "Write the ID \"{id}\" as a lowercase slug of letters, digits, and dashes, for example `order-cancellation`."
                ));
            } else if !ids.insert(id.clone()) {
                issues.push(format!(
                    "Give \"{id}\" to one section only: the ID is used twice."
                ));
            }
        }
        for sub in &sec.subsections {
            let w = format!("sub-section `{}`", sub.id);
            if sub.title.trim().is_empty() {
                issues.push(format!("Give {w} a title."));
            }
            check_body(&w, &Body::from(sub), &mut issues);
        }
    }
    check_glossary(&s.glossary, &mut issues);
    issues
}

/// Validate an architecture submit (Rule B4).
pub fn check_architecture(s: &ArchitectureSubmit) -> Vec<String> {
    let mut issues = vec![];
    if s.status != SubmitStatus::Ok {
        return issues;
    }
    let Some(a) = &s.architecture else {
        return vec!["Return the architecture in `architecture`, because the book shows only what you submit.".into()];
    };
    if a.overview.trim().is_empty() {
        issues.push("Write the architecture overview.".into());
    }
    if a.diagram.kind != DiagramKind::Flowchart {
        issues.push(
            "Draw the architecture diagram as a flowchart of the components and their links."
                .into(),
        );
    }
    check_diagram("the architecture", &a.diagram, &mut issues);
    let names: BTreeSet<&str> = a.components.iter().map(|c| c.name.trim()).collect();
    if names.len() < 2 || names.len() != a.components.len() {
        issues.push(
            "List at least two components, each with a unique name, because links name them."
                .into(),
        );
    }
    for l in &a.links {
        for end in [&l.from, &l.to] {
            if !names.contains(end.trim()) {
                issues.push(format!(
                    "Name a listed component in link {} to {}: \"{end}\" is not in `components`.",
                    l.from, l.to
                ));
            }
        }
    }
    if a.links.is_empty() {
        issues.push("List how the components communicate in `links`.".into());
    }
    if a.failure_recovery.is_empty() {
        issues.push(
            "List at least one failure with its detection and recovery in `failure_recovery`."
                .into(),
        );
    }
    if a.scalability.is_empty() {
        issues.push("List how each component scales in `scalability`.".into());
    }
    check_glossary(&s.glossary, &mut issues);
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(n_participants: usize, n_messages: usize) -> Diagram {
        let mut src = "sequenceDiagram\n".to_string();
        for i in 0..n_messages {
            src.push_str(&format!(
                "  P{} ->> P{}: m{i}\n",
                i % n_participants,
                (i + 1) % n_participants
            ));
        }
        Diagram {
            title: "t".into(),
            kind: DiagramKind::Sequence,
            source: src,
        }
    }

    #[test]
    fn sequence_caps() {
        let mut issues = vec![];
        check_diagram("x", &seq(4, 10), &mut issues);
        assert!(issues.is_empty(), "{issues:?}");
        check_diagram("x", &seq(9, 12), &mut issues);
        assert_eq!(issues.len(), 1);
        issues.clear();
        check_diagram("x", &seq(3, 21), &mut issues);
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn sequence_counts_declared_and_aliased() {
        let src = "sequenceDiagram\nactor U as User\nparticipant API\nU->>+API: POST /orders\nAPI-->>-U: 201\nAPI--)Queue: publish";
        let (p, m) = sequence_size(src.lines().skip(1));
        assert_eq!((p, m), (3, 3));
    }

    #[test]
    fn flowchart_counts_nodes() {
        let src = "flowchart TD\n  A[Start] --> B{Valid?}\n  B -- yes --> C(Save)\n  B -->|no| D[Reject]\n  C & D --> E>Flag]\n  E --- G --- H\n  subgraph S\n  F[(\"db [main]\")]\n  end";
        let n = flowchart_nodes(src.lines().skip(1));
        assert_eq!(n, 8);
    }

    #[test]
    fn flowchart_cap() {
        let mut src = "flowchart LR\n".to_string();
        for i in 0..16 {
            src.push_str(&format!("  N{i} --> N{}\n", i + 1));
        }
        let mut issues = vec![];
        check_diagram(
            "x",
            &Diagram {
                title: "t".into(),
                kind: DiagramKind::Flowchart,
                source: src,
            },
            &mut issues,
        );
        assert_eq!(issues.len(), 1, "{issues:?}");
    }

    #[test]
    fn kind_mismatch() {
        let mut issues = vec![];
        let mut d = seq(2, 2);
        d.kind = DiagramKind::Flowchart;
        check_diagram("x", &d, &mut issues);
        assert!(issues[0].starts_with("Start the Mermaid source"));
    }

    #[test]
    fn book_ids() {
        assert_eq!(book_id(&["web".into(), "api".into()]), "api_web");
        assert_ne!(
            book_id(&["a-b".into(), "c".into()]),
            book_id(&["a".into(), "b-c".into()])
        );
        let many: Vec<String> = (0..40).map(|i| format!("project-{i}")).collect();
        let id = book_id(&many);
        assert!(id.len() <= MAX_BOOK_ID && is_book_id(&id), "{id}");
        assert!(!is_book_id("../x"));
    }

    fn area_submit(ids: &[&str], overview: &str) -> DocumentationSubmit {
        serde_json::from_value(serde_json::json!({
            "status": "ok", "summary": "s", "overview": overview,
            "sections": ids.iter().map(|id| serde_json::json!({
                "id": id, "title": id, "purpose": format!("{id} purpose"), "assumptions": ["x"]
            })).collect::<Vec<_>>()
        }))
        .unwrap()
    }

    #[test]
    fn b9_small_projects_and_single_areas_get_one_writer() {
        let kb = 1024;
        let a = |n: &str, b: u64| (n.to_string(), vec![format!("{n}/**")], b);
        assert!(plan_areas(&[a("core", 100 * kb), a("web", 100 * kb)], 0).is_empty());
        assert!(plan_areas(&[a("core", 5000 * kb), a("empty", 0)], 0).is_empty());
    }

    #[test]
    fn b9_areas_pack_in_map_order_under_the_target() {
        let kb = 1024;
        let a = |n: &str, b: u64| (n.to_string(), vec![format!("{n}/**")], b);
        let areas = plan_areas(
            &[
                a("engine", 600 * kb),
                a("store", 100 * kb),
                a("notify", 50 * kb),
                a("server", 300 * kb),
            ],
            20 * kb,
        );
        let ids: Vec<&str> = areas.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["engine", "store-and-1-more", "server-and-1-more"]);
        assert_eq!(areas[1].title, "store, notify");
        assert_eq!(areas[2].title, "server, other files");
        assert!(areas[2].rest && !areas[1].rest);
        assert_eq!(areas[1].globs, ["store/**", "notify/**"]);
    }

    #[test]
    fn b9_the_writer_count_is_capped() {
        let many: Vec<(String, Vec<String>, u64)> = (0..40)
            .map(|i| (format!("m{i}"), vec![format!("m{i}/**")], 600 * 1024))
            .collect();
        let areas = plan_areas(&many, 0);
        assert!(
            areas.len() <= MAX_DOCS_AREAS && areas.len() >= 2,
            "{}",
            areas.len()
        );
        let ids: BTreeSet<&str> = areas.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids.len(), areas.len(), "area IDs are unique");
    }

    #[test]
    fn b9_combined_areas_keep_every_section_with_unique_ids() {
        let a = DocsArea {
            id: "core".into(),
            title: "core".into(),
            globs: vec![],
            rest: false,
            bytes: 1,
        };
        let b = DocsArea {
            id: "web".into(),
            title: "web".into(),
            globs: vec![],
            rest: false,
            bytes: 1,
        };
        let (sa, sb) = (
            area_submit(&["setup", "orders"], "Core."),
            area_submit(&["setup"], "Web."),
        );
        let c = combine_areas(&[(&a, &sa), (&b, &sb)]);
        let ids: Vec<&str> = c.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["setup", "orders", "setup-web"]);
        assert_eq!(c.overview, "core: Core.\n\nweb: Web.");
    }

    #[test]
    fn b9_a_later_session_rewrites_only_its_areas() {
        let now = Utc::now();
        let area = |id: &str| DocsArea {
            id: id.into(),
            title: id.into(),
            globs: vec![],
            rest: false,
            bytes: 1,
        };
        let first = BookUpdate {
            session: "s1".into(),
            parts: vec![("p".into(), area_submit(&[], ""))],
            areas: BTreeMap::from([(
                "p".to_string(),
                vec![
                    (area("core"), Some(area_submit(&["orders"], "Core v1."))),
                    (area("web"), Some(area_submit(&["pages"], "Web v1."))),
                ],
            )]),
            ..Default::default()
        };
        let book = merge(None, "p", &first, now);
        assert_eq!(book.parts[0].areas.len(), 2);
        let second = BookUpdate {
            session: "s2".into(),
            parts: vec![("p".into(), area_submit(&[], ""))],
            areas: BTreeMap::from([(
                "p".to_string(),
                vec![
                    (area("core"), None),
                    (
                        area("web"),
                        Some(area_submit(&["pages", "forms"], "Web v2.")),
                    ),
                ],
            )]),
            ..Default::default()
        };
        let book = merge(Some(book), "p", &second, now);
        let part = &book.parts[0];
        let ids: Vec<&str> = part.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            ["orders", "pages", "forms"],
            "core is kept, web is rewritten"
        );
        assert_eq!(part.overview, "core: Core v1.\n\nweb: Web v2.");
        assert_eq!(part.areas[1].sections, ["pages", "forms"]);
    }

    #[test]
    fn logs_from_before_books_still_read() {
        use crate::agent::AgentName;
        use crate::event::ExecPurpose;
        use crate::pipeline::StageKind;
        let p: ExecPurpose =
            serde_json::from_value(serde_json::json!({"kind": "module_docs", "project": "p"}))
                .unwrap();
        assert_eq!(
            p,
            ExecPurpose::Docs {
                project: "p".into(),
                area: None,
            }
        );
        let a: AgentName = serde_json::from_value("module-documentation".into()).unwrap();
        assert_eq!(a, AgentName::Documentation);
        assert_eq!(
            "module-documentation".parse::<AgentName>(),
            Ok(AgentName::Documentation)
        );
        let k: StageKind = serde_json::from_value("module-docs".into()).unwrap();
        assert_eq!(k, StageKind::Documentation);
    }

    #[test]
    fn documentation_submit_is_checked() {
        let ok: DocumentationSubmit = serde_json::from_value(serde_json::json!({
            "status": "ok", "summary": "s",
            "sections": [{"id": "a", "title": "A", "purpose": "p", "assumptions": ["x"],
                "code_refs": [{"path": "src/a.rs", "note": "n"}],
                "subsections": [{"id": "a-step", "title": "Step", "purpose": "p", "assumptions": ["y"]}]}]
        }))
        .unwrap();
        assert!(check_documentation(&ok).is_empty());
        let mut bad = ok.clone();
        bad.sections[0].subsections[0].id = "a".into();
        bad.sections[0].subsections[0].assumptions = vec![];
        bad.sections[0].code_refs[0].path = "/abs/a.rs".into();
        let issues = check_documentation(&bad);
        assert_eq!(issues.len(), 3, "{issues:?}");
        assert!(
            issues
                .iter()
                .any(|i| i.starts_with("List the assumptions of sub-section"))
        );
    }

    #[test]
    fn relative_paths() {
        assert!(is_relative("src/lib.rs"));
        for bad in ["/etc/passwd", "../x", "a/../../b", "C:\\x", "~/x", ""] {
            assert!(!is_relative(bad), "{bad}");
        }
    }

    #[test]
    fn concurrent_updates_keep_every_part() {
        let ws = tempfile::tempdir().unwrap();
        let part: DocumentationSubmit = serde_json::from_value(serde_json::json!({
            "status": "ok", "summary": "s",
            "sections": [{"id": "a", "title": "A", "purpose": "p", "assumptions": ["x"]}]
        }))
        .unwrap();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let root = ws.path().to_path_buf();
                let part = part.clone();
                std::thread::spawn(move || {
                    let update = BookUpdate {
                        session: format!("s{i}"),
                        parts: vec![(format!("p{i}"), part)],
                        ..Default::default()
                    };
                    apply(&root, "shared", &update, Utc::now()).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let book = read(ws.path(), "shared").unwrap();
        assert_eq!(book.parts.len(), 8);
        assert_eq!(book.sessions.len(), 8);
    }
}
