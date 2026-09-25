//! Where documents live and how they are read and written: `<name>.json` is the source, and
//! `<name>.md` beside it is rendered from it for the agents that read markdown.

use super::check::{DocIssue, IssueLevel, check};
use super::render::render;
use super::{Document, DocumentView, PhaseDoc, PlanDoc, ResearchDoc, SpecDoc};
use crate::agent::AgentName;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Research,
    Spec,
    Plan,
}

impl DocKind {
    pub fn for_agent(agent: AgentName) -> Option<DocKind> {
        match agent {
            AgentName::Explore => Some(DocKind::Research),
            AgentName::GenerateSpec => Some(DocKind::Spec),
            AgentName::Plan => Some(DocKind::Plan),
            _ => None,
        }
    }

    pub fn owner(self) -> AgentName {
        match self {
            DocKind::Research => AgentName::Explore,
            DocKind::Spec => AgentName::GenerateSpec,
            DocKind::Plan => AgentName::Plan,
        }
    }

    pub fn prefix(self) -> &'static str {
        match self {
            DocKind::Research => "ostra-research-",
            DocKind::Spec => "ostra-spec-",
            DocKind::Plan => "ostra-plan-",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DocKind::Research => "research document",
            DocKind::Spec => "spec",
            DocKind::Plan => "plan",
        }
    }

    /// JSON schema of the document, for the `Document` tool.
    pub fn schema(self) -> Value {
        let s = match self {
            DocKind::Research => schemars::schema_for!(ResearchDoc),
            DocKind::Spec => schemars::schema_for!(SpecDoc),
            DocKind::Plan => schemars::schema_for!(PlanDoc),
        };
        serde_json::to_value(s).unwrap_or(Value::Null)
    }

    /// Parse a JSON value as this kind's document, naming the field that failed.
    pub fn parse(self, value: &Value) -> Result<Document, String> {
        fn typed<T: DeserializeOwned>(v: &Value) -> Result<T, String> {
            serde_path_to_error::deserialize(v.clone()).map_err(|e| {
                let at = e.path().to_string();
                if at == "." {
                    e.inner().to_string()
                } else {
                    format!("{at}: {}", e.inner())
                }
            })
        }
        Ok(match self {
            DocKind::Research => Document::Research(typed(value)?),
            DocKind::Spec => Document::Spec(typed(value)?),
            DocKind::Plan => {
                let mut plan: super::PlanDoc = typed(value)?;
                // Rule P7: a phase's Required Skills are the union of its steps' skills.
                for p in &mut plan.phases {
                    for s in &p.steps {
                        for k in &s.skills {
                            if !p.skills.contains(k) {
                                p.skills.push(k.clone());
                            }
                        }
                    }
                }
                Document::Plan(plan)
            }
        })
    }
}

/// The document kind a session-dir file name belongs to. Phase files are part of the plan and
/// report `Plan`.
pub fn doc_kind_for(path: &Path) -> Option<DocKind> {
    let name = path.file_name()?.to_string_lossy();
    if !(name.ends_with(".md") || name.ends_with(".json")) {
        return None;
    }
    [DocKind::Research, DocKind::Spec, DocKind::Plan]
        .into_iter()
        .find(|k| name.starts_with(k.prefix()))
}

fn is_phase_file(path: &Path) -> bool {
    path.file_stem()
        .is_some_and(|s| phase_number(&s.to_string_lossy()).is_some())
}

/// `N` in `ostra-plan-...-phase-N`.
fn phase_number(stem: &str) -> Option<u32> {
    let (_, n) = stem.rsplit_once("-phase-")?;
    n.parse().ok()
}

pub fn json_path(md: &Path) -> PathBuf {
    md.with_extension("json")
}

/// The phase file Ostra writes for phase `id` of the master plan at `master`.
pub fn phase_path(master: &Path, id: u32) -> PathBuf {
    let stem = master
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    master.with_file_name(format!("{stem}-phase-{id}.md"))
}

/// The stored JSON of the document rendered to `md`, if one exists.
pub fn load(md: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(json_path(md)).ok()?;
    serde_json::from_str(&text).ok()
}

/// The typed document behind a session-dir markdown file, with its check results.
pub fn load_view(md: &Path) -> Option<DocumentView> {
    let kind = doc_kind_for(md)?;
    let value = load(md)?;
    let document = if kind == DocKind::Plan && is_phase_file(md) {
        Document::Phase(serde_json::from_value::<PhaseDoc>(value).ok()?)
    } else {
        kind.parse(&value).ok()?
    };
    let issues = check(&document);
    Some(DocumentView { document, issues })
}

/// What one write produced.
#[derive(Debug, Clone)]
pub struct Written {
    pub document: Document,
    /// Markdown files written, the main one first.
    pub files: Vec<PathBuf>,
    pub issues: Vec<DocIssue>,
}

impl Written {
    pub fn errors(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.level == IssueLevel::Error)
            .count()
    }
}

fn put(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

fn pretty(v: &impl serde::Serialize) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_default();
    s.push('\n');
    s
}

/// Validate `value` as a `kind` document and write it to `md` (rendered) and its JSON sibling.
/// A plan also writes one phase file pair per phase and removes phase files it no longer has.
pub fn write(kind: DocKind, md: &Path, value: &Value) -> Result<Written, String> {
    let document = kind.parse(value)?;
    let issues = check(&document);
    let dir = md.parent().ok_or("The document path has no directory.")?;
    put(&json_path(md), &pretty(value))?;
    put(md, &render(&document, md))?;
    let mut files = vec![md.to_path_buf()];
    if let Document::Plan(plan) = &document {
        let keep: Vec<PathBuf> = plan.phases.iter().map(|p| phase_path(md, p.id)).collect();
        for p in &plan.phases {
            let path = phase_path(md, p.id);
            let deliverable_title = plan
                .deliverables
                .iter()
                .find(|d| d.id == p.deliverable)
                .map(|d| d.title.clone());
            let doc = PhaseDoc {
                plan: plan.title.clone(),
                date: plan.date.clone(),
                spec: plan.spec.clone(),
                deliverable_title,
                phase: p.clone(),
            };
            put(&json_path(&path), &pretty(&doc))?;
            put(&path, &render(&Document::Phase(doc), &path))?;
            files.push(path);
        }
        let stem = md
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let prefix = format!("{stem}-phase-");
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let stale = name.starts_with(&prefix)
                && (name.ends_with(".md") || name.ends_with(".json"))
                && !keep
                    .iter()
                    .any(|k| k.with_extension("") == path.with_extension(""));
            if stale {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Ok(Written {
        document,
        files,
        issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_paths() {
        let dir = Path::new("/ws/.ostra/sessions/s1");
        assert_eq!(
            doc_kind_for(&dir.join("ostra-spec-1-x.md")),
            Some(DocKind::Spec)
        );
        assert_eq!(
            doc_kind_for(&dir.join("ostra-plan-1-x-phase-2.md")),
            Some(DocKind::Plan)
        );
        assert_eq!(
            doc_kind_for(&dir.join("ostra-research-1.json")),
            Some(DocKind::Research)
        );
        assert_eq!(
            doc_kind_for(&dir.join("ostra-implementer-phase-1.md")),
            None
        );
        assert_eq!(
            phase_path(&dir.join("ostra-plan-1-x.md"), 3),
            dir.join("ostra-plan-1-x-phase-3.md")
        );
        assert!(is_phase_file(&dir.join("ostra-plan-1-x-phase-3.md")));
        assert!(!is_phase_file(&dir.join("ostra-plan-1-x.md")));
    }
}
