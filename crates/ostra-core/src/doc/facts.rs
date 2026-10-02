//! Rules D2a and D4a: what the stages after research learn about the code without reading it
//! again. Each research document records its files' fingerprints when it is written; these
//! functions compare them with the files as they are now.

use super::refs::{MISSING, fingerprint, parse_ref};
use super::store::load;
use super::{DocKind, Document, ResearchDoc};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Unchanged,
    Changed,
    Gone,
    /// The document predates fingerprints, or does not name the file in a field Ostra records.
    Unknown,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Unchanged => "unchanged",
            Status::Changed => "changed",
            Status::Gone => "gone",
            Status::Unknown => "not checked",
        }
    }
}

struct Research {
    name: String,
    doc: ResearchDoc,
}

fn load_research(docs: &[PathBuf]) -> Vec<Research> {
    docs.iter()
        .filter_map(|md| {
            let Ok(Document::Research(doc)) = DocKind::Research.parse(&load(md)?) else {
                return None;
            };
            let name = md.file_name()?.to_string_lossy().to_string();
            Some(Research { name, doc })
        })
        .collect()
}

/// Current fingerprints, each file hashed once per call.
#[derive(Default)]
struct Now(HashMap<PathBuf, String>);

impl Now {
    fn status(&mut self, r: &Research, path: &str) -> Status {
        let Some(snap) = &r.doc.snapshot else {
            return Status::Unknown;
        };
        let Some(then) = snap.files.get(path) else {
            return Status::Unknown;
        };
        let abs = Path::new(&snap.root).join(path);
        let now = self
            .0
            .entry(abs.clone())
            .or_insert_with(|| fingerprint(&abs));
        if now == MISSING {
            Status::Gone
        } else if now == then {
            Status::Unchanged
        } else {
            Status::Changed
        }
    }

    fn label(&mut self, r: &Research, text: &str) -> &'static str {
        match parse_ref(text) {
            Some(code) => self.status(r, &code.path).label(),
            None => Status::Unknown.label(),
        }
    }
}

/// Rule D2a: the files the research documents cite that changed or are gone since the newest
/// document naming them was written, one line each. Empty when every cited file is unchanged.
pub fn changed_since_research(docs: &[PathBuf]) -> Vec<String> {
    let research = load_research(docs);
    let mut now = Now::default();
    let mut seen = HashSet::new();
    let mut out = vec![];
    // The newest document that names a file is the one whose description of it stands.
    for r in research.iter().rev() {
        let Some(snap) = &r.doc.snapshot else {
            out.push(format!(
                "{}: written before Ostra recorded file fingerprints, so check a file it cites before you rely on its description",
                r.name
            ));
            continue;
        };
        for path in snap.files.keys() {
            if !seen.insert((snap.root.clone(), path.clone())) {
                continue;
            }
            let what = match now.status(r, path) {
                Status::Changed => "changed",
                Status::Gone => "gone",
                Status::Unchanged | Status::Unknown => continue,
            };
            out.push(format!(
                "`{path}` in {}: {what} since {} was written",
                r.doc.repo, r.name
            ));
        }
    }
    out
}

const PREAMBLE: &str = "# Code facts

Ostra wrote this file from the research documents of this session and checked every file they cite \
against the content the research read. Each file carries one mark:

- `unchanged`: the content is what the research read, so the purpose, symbols, patterns, and flow \
below describe it as it is now.
- `changed`: the file was edited since. Read it before you rely on what is written here.
- `gone`: the file no longer exists.
- `not checked`: the research document predates file fingerprints. Read the file before you rely on it.

These are facts about the code only. Take every requirement from the spec, never from this file.
";

/// Rule D4a: the code facts of every research document, grouped by repo, each file marked by
/// whether it changed since the document that describes it was written. Where several documents
/// describe the same file, pattern, or dependency, the newest one stands.
pub fn render_code_facts(docs: &[PathBuf]) -> String {
    let research = load_research(docs);
    let mut now = Now::default();
    let mut out = String::from(PREAMBLE);
    let mut repos: Vec<&str> = vec![];
    for r in &research {
        if !repos.contains(&r.doc.repo.as_str()) {
            repos.push(r.doc.repo.as_str());
        }
    }
    if repos.is_empty() {
        let _ = writeln!(out, "\nNo research document recorded code facts.");
        return out;
    }
    for repo in repos {
        let newest_first: Vec<&Research> = research
            .iter()
            .rev()
            .filter(|r| r.doc.repo == repo)
            .collect();
        let _ = write!(out, "\n## Repo `{repo}`\n");
        if let Some(root) = newest_first
            .iter()
            .find_map(|r| r.doc.snapshot.as_ref().map(|s| s.root.clone()))
        {
            let _ = writeln!(out, "\nRoot: `{root}`");
        }

        let mut files = String::new();
        let mut seen = HashSet::new();
        for r in &newest_first {
            for f in &r.doc.files {
                if !seen.insert(f.path.clone()) {
                    continue;
                }
                let _ = writeln!(
                    files,
                    "- `{}` ({}): {}",
                    f.path,
                    now.label(r, &f.path),
                    f.purpose.trim()
                );
                for s in &f.symbols {
                    let _ = writeln!(files, "  - `{}`", s.trim());
                }
            }
        }
        if !files.is_empty() {
            let _ = write!(out, "\n### Files\n\n{files}");
        }

        let mut patterns = String::new();
        let mut seen = HashSet::new();
        for r in &newest_first {
            for p in &r.doc.patterns {
                if !seen.insert(p.name.trim().to_lowercase()) {
                    continue;
                }
                let _ = writeln!(
                    patterns,
                    "\n#### {}\n\n{}",
                    p.name.trim(),
                    p.description.trim()
                );
                if !p.files.is_empty() {
                    let used: Vec<String> = p
                        .files
                        .iter()
                        .map(|f| format!("`{f}` ({})", now.label(r, f)))
                        .collect();
                    let _ = writeln!(patterns, "\nUsed in: {}", used.join(", "));
                }
                if let Some(s) = &p.snippet {
                    if let Some(src) = &s.source {
                        let _ = writeln!(patterns, "\nFrom `{src}` ({}):", now.label(r, src));
                    }
                    let _ = writeln!(patterns, "\n```{}\n{}\n```", s.language, s.code.trim_end());
                }
            }
        }
        if !patterns.is_empty() {
            let _ = write!(out, "\n### Patterns\n{patterns}");
        }

        let mut flows: Vec<(String, Vec<String>)> = vec![];
        for r in &newest_first {
            let hops: Vec<String> = r
                .doc
                .data_flow
                .iter()
                .map(|h| match &h.location {
                    Some(l) => format!("{} (`{l}`, {})", h.step.trim(), now.label(r, l)),
                    None => h.step.trim().to_string(),
                })
                .collect();
            if !hops.is_empty() && !flows.iter().any(|(_, f)| *f == hops) {
                flows.push((r.doc.title.trim().to_string(), hops));
            }
        }
        for (title, hops) in flows {
            let _ = writeln!(out, "\n### Data flow: {title}\n");
            for (i, h) in hops.iter().enumerate() {
                let _ = writeln!(out, "{}. {h}", i + 1);
            }
        }

        let mut deps: BTreeMap<String, String> = BTreeMap::new();
        for r in &newest_first {
            for d in &r.doc.dependencies {
                let kind = match d.kind {
                    super::DependencyKind::Internal => "internal",
                    super::DependencyKind::External => "external",
                };
                let version = d
                    .version
                    .as_deref()
                    .map(|v| format!(", {v}"))
                    .unwrap_or_default();
                deps.entry(d.name.clone()).or_insert_with(|| {
                    format!("- `{}` ({kind}{version}): {}", d.name, d.role.trim())
                });
            }
        }
        if !deps.is_empty() {
            let _ = writeln!(out, "\n### Dependencies\n");
            for line in deps.values() {
                let _ = writeln!(out, "{line}");
            }
        }
    }
    out
}
