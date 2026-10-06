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
use std::collections::BTreeSet;
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

/// One page of a project's part (Rule B1). The writer picks the page's topic and structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct DocSection {
    /// Lowercase slug, unique in the submit.
    pub id: String,
    pub title: String,
    /// One or two sentences: what a reader learns on the page. The index shows it.
    pub summary: String,
    /// The page as Markdown. Headings start at `##`, and diagrams are `mermaid` code blocks.
    pub body: String,
    /// The files a reader opens after the page. The page shows them last.
    #[serde(default)]
    pub code_refs: Vec<CodeRef>,
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
    /// Rule B9: a book written before free pages holds typed sections, which load as pages.
    #[serde(deserialize_with = "legacy::sections")]
    pub sections: Vec<DocSection>,
    #[ts(type = "string")]
    pub updated_at: DateTime<Utc>,
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
    /// Each documented project's part as this session wrote it.
    pub parts: Vec<(String, DocumentationSubmit)>,
    pub architecture: Option<Architecture>,
    pub glossary: Vec<GlossaryEntry>,
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
        book.parts.retain(|p| &p.project != project);
        book.parts.push(BookPart {
            project: project.clone(),
            overview: submit.overview.clone(),
            sections: submit.sections.clone(),
            updated_at: now,
        });
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

fn code_refs_table(out: &mut String, h: &str, refs: &[CodeRef]) {
    if refs.is_empty() {
        return;
    }
    out.push_str(&format!("{h} Code references\n\n"));
    table(
        out,
        &["Path", "Symbol", "Lines", "What it holds"],
        refs.iter().map(|r| {
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

/// Rule B1: the title, the summary, the writer's own Markdown, and the code references last.
pub fn render_section(project: &str, s: &DocSection) -> String {
    let mut out = format!(
        "# {}\n\nProject: `{project}`\n\n{}\n\n{}\n\n",
        s.title.trim(),
        s.summary.trim(),
        s.body.trim()
    );
    code_refs_table(&mut out, "##", &s.code_refs);
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
                first_sentence(&s.summary)
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
    let head = mermaid_head(&d.source);
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
        _ => check_mermaid(where_, &d.title, &d.source, issues),
    }
}

fn mermaid_lines(source: &str) -> impl Iterator<Item = &str> {
    source
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("%%"))
}

fn mermaid_head(source: &str) -> &str {
    mermaid_lines(source).next().unwrap_or_default()
}

/// Rule B3: a sequence diagram or a flowchart stays within its caps. Other Mermaid kinds have none.
fn check_mermaid(where_: &str, title: &str, source: &str, issues: &mut Vec<String>) {
    let mut lines = mermaid_lines(source);
    let head = lines.next().unwrap_or_default();
    if head == "sequenceDiagram" {
        let (participants, messages) = sequence_size(lines);
        if participants > MAX_SEQUENCE_PARTICIPANTS || messages > MAX_SEQUENCE_MESSAGES {
            issues.push(format!(
                "Split {where_} diagram \"{title}\" into one diagram per step: it has {participants} participants and {messages} messages, and a sequence diagram can have at most {MAX_SEQUENCE_PARTICIPANTS} and {MAX_SEQUENCE_MESSAGES}, because a reader follows a small diagram and loses a large one."
            ));
        }
    } else if head.starts_with("flowchart") || head.starts_with("graph") {
        let nodes = flowchart_nodes(lines);
        if nodes > MAX_FLOWCHART_NODES {
            issues.push(format!(
                "Split {where_} diagram \"{title}\" into smaller flowcharts: it has {nodes} nodes, and a flowchart can have at most {MAX_FLOWCHART_NODES}, because a reader follows a small diagram and loses a large one."
            ));
        }
    }
}

/// The `mermaid` code blocks of a Markdown body, each with the heading above it as its title.
fn mermaid_blocks(body: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut heading = String::new();
    let mut fence: Option<(String, bool, Vec<&str>)> = None;
    for line in body.lines() {
        let t = line.trim_start();
        if let Some((marker, is_mermaid, lines)) = &mut fence {
            if t.starts_with(marker.as_str())
                && t.trim_start_matches(marker.as_str()).trim().is_empty()
            {
                if *is_mermaid {
                    let title = if heading.is_empty() {
                        format!("{} in the page", ordinal(out.len() + 1))
                    } else {
                        heading.clone()
                    };
                    out.push((title, lines.join("\n")));
                }
                fence = None;
            } else {
                lines.push(line);
            }
            continue;
        }
        let marker: String = t.chars().take_while(|c| *c == '`' || *c == '~').collect();
        if marker.len() >= 3 && marker.chars().all(|c| c == marker.chars().next().unwrap()) {
            let lang = t[marker.len()..].trim();
            fence = Some((marker, lang.starts_with("mermaid"), vec![]));
        } else if t.starts_with('#') {
            heading = t.trim_start_matches('#').trim().to_string();
        }
    }
    out
}

fn ordinal(n: usize) -> String {
    match n {
        1 => "the first".into(),
        2 => "the second".into(),
        3 => "the third".into(),
        n => format!("number {n}"),
    }
}

/// Lines of a Markdown body that start a level-1 heading outside a code block.
fn top_headings(body: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut fence: Option<String> = None;
    for line in body.lines() {
        let t = line.trim_start();
        let marker: String = t.chars().take_while(|c| *c == '`' || *c == '~').collect();
        if let Some(m) = &fence {
            if t.starts_with(m.as_str()) {
                fence = None;
            }
        } else if marker.len() >= 3 {
            fence = Some(marker);
        } else if t.starts_with("# ") {
            out.push(t);
        }
    }
    out
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
        if !is_slug(&sec.id) {
            issues.push(format!(
                "Write the ID \"{}\" as a lowercase slug of letters, digits, and dashes, for example `order-cancellation`.",
                sec.id
            ));
        } else if !ids.insert(sec.id.clone()) {
            issues.push(format!(
                "Give \"{}\" to one section only: the ID is used twice.",
                sec.id
            ));
        }
        if sec.title.trim().is_empty() {
            issues.push(format!("Give {where_} a title."));
        }
        if sec.summary.trim().is_empty() {
            issues.push(format!(
                "Write the summary of {where_}: one or two sentences that say what a reader learns on the page."
            ));
        }
        if sec.body.trim().is_empty() {
            issues.push(format!("Write the Markdown body of {where_}."));
        }
        if let Some(h) = top_headings(&sec.body).first() {
            issues.push(format!(
                "Start the headings in the body of {where_} at `##`: \"{h}\" is a level-1 heading, and Ostra writes the title as the only one."
            ));
        }
        for (title, source) in mermaid_blocks(&sec.body) {
            check_mermaid(&where_, &title, &source, &mut issues);
        }
        for r in &sec.code_refs {
            if !is_relative(&r.path) {
                issues.push(format!(
                    "Write code reference \"{}\" in {where_} as a path relative to the project root, with `/` separators and no `..`.",
                    r.path
                ));
            }
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

/// Rule B9: books written before free pages held typed sections: purpose, boundaries, assumptions,
/// flow, diagrams, tables, concerns, and sub-sections. They load as pages, so an old book stays
/// readable and searchable until the next docs run rewrites it.
mod legacy {
    use super::{CodeRef, Diagram, DocSection, code_refs_table, diagram, list_items, table};
    use serde::{Deserialize, Deserializer};

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Boundaries {
        owns: Vec<String>,
        does_not_own: Vec<String>,
    }

    #[derive(Deserialize)]
    struct Step {
        actor: String,
        action: String,
        outcome: String,
    }

    #[derive(Deserialize)]
    struct Table {
        title: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    }

    #[derive(Deserialize)]
    struct Concern {
        component: String,
        responsibility: String,
    }

    #[derive(Deserialize)]
    struct Typed {
        id: String,
        title: String,
        #[serde(default)]
        purpose: String,
        #[serde(default)]
        boundaries: Boundaries,
        #[serde(default)]
        assumptions: Vec<String>,
        #[serde(default)]
        business_flow: Vec<Step>,
        #[serde(default)]
        diagrams: Vec<Diagram>,
        #[serde(default)]
        tables: Vec<Table>,
        #[serde(default)]
        concerns: Vec<Concern>,
        #[serde(default)]
        code_refs: Vec<CodeRef>,
        #[serde(default)]
        subsections: Vec<Typed>,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Page(DocSection),
        Typed(Typed),
    }

    pub fn sections<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<DocSection>, D::Error> {
        Ok(Vec::<Stored>::deserialize(d)?
            .into_iter()
            .map(|s| match s {
                Stored::Page(p) => p,
                Stored::Typed(t) => page(t),
            })
            .collect())
    }

    /// The section's own fields under `###` headings, then each sub-section as a `##` heading
    /// with its fields under `####`, so a sub-section stays its own search unit.
    fn page(t: Typed) -> DocSection {
        let mut body = String::new();
        fields(&mut body, "###", &t);
        for sub in &t.subsections {
            body.push_str(&format!(
                "## {}\n\n{}\n\n",
                sub.title.trim(),
                sub.purpose.trim()
            ));
            fields(&mut body, "####", sub);
            code_refs_table(&mut body, "####", &sub.code_refs);
        }
        DocSection {
            id: t.id,
            title: t.title,
            summary: t.purpose.trim().to_string(),
            body: body.trim_end().to_string(),
            code_refs: t.code_refs,
        }
    }

    fn fields(out: &mut String, h: &str, t: &Typed) {
        let b = &t.boundaries;
        if !b.owns.is_empty() || !b.does_not_own.is_empty() {
            out.push_str(&format!("{h} Boundaries\n\n"));
            if !b.owns.is_empty() {
                out.push_str("Owns:\n\n");
                list_items(out, &b.owns);
            }
            if !b.does_not_own.is_empty() {
                out.push_str("Does not own:\n\n");
                list_items(out, &b.does_not_own);
            }
        }
        if !t.assumptions.is_empty() {
            out.push_str(&format!("{h} Assumptions\n\n"));
            list_items(out, &t.assumptions);
        }
        if !t.business_flow.is_empty() {
            out.push_str(&format!("{h} Business flow\n\n"));
            table(
                out,
                &["Step", "Actor", "Action", "Outcome"],
                t.business_flow.iter().enumerate().map(|(i, f)| {
                    vec![
                        (i + 1).to_string(),
                        f.actor.clone(),
                        f.action.clone(),
                        f.outcome.clone(),
                    ]
                }),
            );
        }
        for d in &t.diagrams {
            diagram(out, h, d);
        }
        for tb in &t.tables {
            out.push_str(&format!("{h} {}\n\n", tb.title.trim()));
            let cols: Vec<&str> = tb.columns.iter().map(String::as_str).collect();
            table(out, &cols, tb.rows.iter().cloned());
        }
        if !t.concerns.is_empty() {
            out.push_str(&format!("{h} Separation of concerns\n\n"));
            table(
                out,
                &["Component", "Responsibility"],
                t.concerns
                    .iter()
                    .map(|c| vec![c.component.clone(), c.responsibility.clone()]),
            );
        }
    }
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

    fn page_submit(body: &str) -> DocumentationSubmit {
        serde_json::from_value(serde_json::json!({
            "status": "ok", "summary": "s",
            "sections": [{"id": "a", "title": "A", "summary": "What A does.", "body": body,
                "code_refs": [{"path": "src/a.rs", "note": "n"}]}]
        }))
        .unwrap()
    }

    #[test]
    fn documentation_submit_is_checked() {
        let ok = page_submit(
            "Text.\n\n## Step\n\n```mermaid\nsequenceDiagram\nA->>B: go\n```\n\n```rust\n# not a heading\n```",
        );
        assert!(
            check_documentation(&ok).is_empty(),
            "{:?}",
            check_documentation(&ok)
        );
        let mut big = "sequenceDiagram\n".to_string();
        for i in 0..21 {
            big.push_str(&format!("A->>B: m{i}\n"));
        }
        let mut bad = page_submit(&format!(
            "# Title again\n\n## Calls\n\n```mermaid\n{big}```"
        ));
        bad.sections[0].summary = " ".into();
        bad.sections[0].code_refs[0].path = "/abs/a.rs".into();
        bad.sections.push(bad.sections[0].clone());
        let issues = check_documentation(&bad);
        assert!(
            issues
                .iter()
                .any(|i| i.starts_with("Give \"a\" to one section only")),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|i| i.starts_with("Write the summary")),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|i| i.starts_with("Start the headings")),
            "{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.starts_with("Split section `a` diagram \"Calls\"")),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|i| i.starts_with("Write code reference")),
            "{issues:?}"
        );
    }

    #[test]
    fn other_mermaid_kinds_have_no_caps() {
        let mut src = "stateDiagram-v2\n".to_string();
        for i in 0..40 {
            src.push_str(&format!("S{i} --> S{}\n", i + 1));
        }
        let s = page_submit(&format!("```mermaid\n{src}```"));
        assert!(check_documentation(&s).is_empty());
    }

    #[test]
    fn typed_sections_from_old_books_load_as_pages() {
        let part: BookPart = serde_json::from_value(serde_json::json!({
            "project": "p", "overview": "o", "updated_at": "2026-09-30T00:00:00Z",
            "areas": [{"id": "core", "title": "core", "sections": ["orders"]}],
            "sections": [
                {"id": "orders", "title": "Orders", "purpose": "Creates orders.",
                 "assumptions": ["Auth is done upstream."],
                 "code_refs": [{"path": "src/orders.rs", "note": "n"}],
                 "subsections": [{"id": "orders-cancel", "title": "Cancel", "purpose": "Cancels.",
                    "assumptions": ["x"], "code_refs": [{"path": "src/cancel.rs", "note": "c"}]}]},
                {"id": "new", "title": "New", "summary": "S.", "body": "B."}
            ]
        }))
        .unwrap();
        let old = &part.sections[0];
        assert_eq!(old.summary, "Creates orders.");
        assert!(
            old.body
                .starts_with("### Assumptions\n\n- Auth is done upstream."),
            "{}",
            old.body
        );
        assert!(
            old.body
                .contains("## Cancel\n\nCancels.\n\n#### Assumptions"),
            "{}",
            old.body
        );
        assert!(old.body.contains("`src/cancel.rs`"), "{}", old.body);
        assert_eq!(old.code_refs[0].path, "src/orders.rs");
        assert_eq!(part.sections[1].body, "B.");
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
        let part = page_submit("B.");
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
