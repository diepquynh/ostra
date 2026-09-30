//! The repo brief appended to every execution's first message, ported from Ultracode's
//! `hooks/lib/context-brief.js`. It carries the project facts an agent needs so it does not spend
//! its first tool calls fetching them, then the project's own agent instruction files (`CLAUDE.md`,
//! `AGENTS.md`, `AGENT.md`), then the workspace's custom instructions.
//!
//! Selection is a containment test against the inventory's own text, not a static field list: the
//! same profile field repeats the inventory in one repo and is the only statement of a rule in
//! another, and the brief must avoid stating a fact twice without withholding it. Routing settings
//! are never included: they mean nothing to an agent and would leak tier names into its context.

use ostra_core::AgentName;
use ostra_core::config::{ModuleRow, ProjectProfile, SkillEntry};
use std::path::{Path, PathBuf};

pub const MAX_BRIEF_CHARS: usize = 3600;
/// Per instruction file. Longer files are cut and the agent reads the rest from disk.
pub const MAX_PROJECT_DOC_CHARS: usize = 12000;
const MAX_SKILL_ROWS: usize = 16;
const MAX_MODULE_ROWS: usize = 10;

/// Heading that marks a message already carrying a brief, so a re-render never stacks two.
pub const BRIEF_HEADING: &str = "## Repo brief for ";
pub const INSTRUCTIONS_HEADING: &str = "## Workspace instructions";
pub const PROJECT_DOCS_HEADING: &str = "## Project instructions";
pub const ARTIFACTS_HEADING: &str = "## Workspace artifacts";
const NEW_PROJECTS_HEADING: &str = "## Projects created in this session";

/// An agent instruction file at the project root and its text.
#[derive(Debug, Clone)]
pub struct ProjectDoc {
    pub path: PathBuf,
    pub content: String,
}

/// The project's instruction files, read from disk. A file that is a link to, or a copy of, one
/// already read is left out, because repos often ship `AGENTS.md` as a link to `CLAUDE.md`.
pub fn project_docs(repo_root: &Path) -> Vec<ProjectDoc> {
    let mut seen: Vec<(PathBuf, String)> = vec![];
    let mut out = vec![];
    for path in ostra_core::paths::project_instruction_files(repo_root) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let real = ostra_core::paths::canonical(&path).unwrap_or_else(|_| path.clone());
        if content.trim().is_empty() || seen.iter().any(|(p, c)| *p == real || *c == content) {
            continue;
        }
        seen.push((real, content.clone()));
        out.push(ProjectDoc { path, content });
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Stack,
    Commands,
    Testing,
    Skills,
    Conventions,
    Review,
    Modules,
}

fn sections(agent: AgentName) -> &'static [Section] {
    use Section::*;
    match agent {
        AgentName::Implementer => &[Commands, Skills, Conventions, Modules],
        AgentName::WriteTest => &[Commands, Testing, Skills, Conventions, Modules],
        AgentName::CodeReviewer => &[Commands, Review, Conventions, Skills],
        AgentName::ExecutionPathAnalyzer => &[Commands, Testing, Modules],
        AgentName::Explore => &[Stack, Skills, Modules],
        AgentName::Plan => &[Stack, Commands, Skills, Modules],
        AgentName::GenerateSpec => &[Stack, Modules],
        AgentName::ModuleDocumentation => &[Commands, Skills, Modules],
        AgentName::FactCheck => &[Stack, Modules],
        AgentName::PromptGeneration => &[Skills],
        AgentName::QuickAnswer => &[Stack, Commands, Skills, Modules],
        AgentName::Initializer => &[],
        AgentName::Advisor => &[Stack, Commands, Skills, Modules],
    }
}

/// The workspace's visible artifacts (HANDOVER 6.5), read when the spawn is built.
#[derive(Debug, Clone, Default)]
pub struct ArtifactsBrief {
    pub dir: PathBuf,
    /// Path inside the folder and size in bytes, at most [`ostra_core::artifacts::MAX_BRIEF_ARTIFACTS`].
    pub entries: Vec<(String, u64)>,
    /// Every visible artifact, listed or not.
    pub total: usize,
}

impl ArtifactsBrief {
    /// The visible artifacts of the workspace at `workspace`, or `None` when it holds none.
    pub fn read(workspace: &Path) -> Option<ArtifactsBrief> {
        let dir = ostra_core::artifacts::dir(workspace);
        let all = ostra_core::artifacts::list(&dir);
        (!all.is_empty()).then(|| ArtifactsBrief {
            total: all.len(),
            entries: all
                .into_iter()
                .take(ostra_core::artifacts::MAX_BRIEF_ARTIFACTS)
                .map(|e| (e.path, e.size))
                .collect(),
            dir,
        })
    }
}

/// What the brief is built from.
pub struct BriefInput<'a> {
    pub agent: AgentName,
    /// The spawn block and task text, scanned for paths that narrow the module map.
    pub prompt: &'a str,
    pub repo_root: &'a Path,
    pub profile: Option<&'a ProjectProfile>,
    /// `INVENTORY.md` text, when it exists.
    pub inventory: Option<&'a str>,
    /// `instructions.all`, then the agent's own entry.
    pub instructions: &'a [String],
    /// The project's `CLAUDE.md`, `AGENTS.md`, and `AGENT.md`.
    pub project_docs: &'a [ProjectDoc],
    pub artifacts: Option<&'a ArtifactsBrief>,
    /// Rule O3: projects agents created in this session, with the facts from their `ProjectCreate`.
    pub new_projects: &'a [ostra_core::manage::CreatedProject],
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whitespace-insensitive containment, so a value wrapped across lines or padded inside a table cell
/// still counts as already stated. Values under three characters are too short to be distinctive.
pub fn stated_in(haystack: &str, value: &str) -> bool {
    let text = value.trim();
    if text.chars().count() < 3 {
        return true;
    }
    squash(haystack).contains(&squash(text))
}

fn truncate(value: &str, limit: usize) -> String {
    let text = squash(value);
    if text.chars().count() > limit {
        let cut: String = text.chars().take(limit.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        text
    }
}

/// Paths and module-like tokens named in the prompt, used to narrow the module map.
pub fn scope_hints(prompt: &str) -> Vec<String> {
    let re = regex::Regex::new(r"[\w.-]+/[\w./-]+").expect("static regex");
    let mut hints: Vec<String> = vec![];
    for m in re.find_iter(prompt) {
        let cleaned = m.as_str().trim_start_matches("./").to_string();
        if cleaned.len() > 3 && !cleaned.contains("ostra-") && !hints.contains(&cleaned) {
            hints.push(cleaned);
        }
        if hints.len() >= 40 {
            break;
        }
    }
    hints
}

/// Module-map rows whose glob plausibly covers a path the prompt names. Without a hint the map is
/// left out: the full map of a large repo is bigger than the rest of the brief combined.
fn relevant_modules<'p>(map: &'p [ModuleRow], hints: &[String]) -> Vec<&'p ModuleRow> {
    if hints.is_empty() {
        return vec![];
    }
    map.iter()
        .filter(|row| {
            let stem = row.glob.replace('*', "");
            let stem = stem.trim_end_matches('/');
            stem.len() >= 3
                && hints
                    .iter()
                    .any(|h| h.contains(stem) || stem.contains(h.as_str()))
        })
        .take(MAX_MODULE_ROWS)
        .collect()
}

fn skill_rows(
    agent: AgentName,
    skills: &[SkillEntry],
    inventory: &str,
    repo_root: &Path,
) -> Vec<String> {
    skills
        .iter()
        .filter(|s| !s.path.is_empty())
        .filter(|s| {
            if s.kind == "convention" || s.kind == "module-hub" {
                return true;
            }
            match agent {
                AgentName::WriteTest => {
                    let hay = format!("{} {}", s.name, s.kind).to_lowercase();
                    hay.contains("test") || hay.contains("spec")
                }
                AgentName::CodeReviewer => s.kind == "convention",
                AgentName::ModuleDocumentation => s.kind == "module-hub",
                _ => true,
            }
        })
        .take(MAX_SKILL_ROWS)
        .map(|s| {
            let path = repo_root.join(&s.path);
            let use_for = s
                .component_type
                .as_deref()
                .filter(|c| !stated_in(inventory, c))
                .map(|c| format!(": use for {}", truncate(c, 48)))
                .unwrap_or_default();
            if s.name.is_empty() {
                format!("- `{}`{use_for}", path.display())
            } else {
                format!("- `{}` at `{}`{use_for}", s.name, path.display())
            }
        })
        .collect()
}

/// The brief as markdown, or `None` when there is nothing to say.
pub fn build_brief(input: &BriefInput<'_>) -> Option<String> {
    let mut out: Vec<String> = vec![];
    let wanted = sections(input.agent);
    if let Some(profile) = input.profile.filter(|_| !wanted.is_empty()) {
        let inventory = squash(input.inventory.unwrap_or(""));
        let mut body: Vec<String> = vec![];

        if wanted.contains(&Section::Stack) {
            let mut bits = vec![];
            if let Some(st) = &profile.stack {
                bits.push(format!("- stack {st}"));
            }
            if let Some(b) = &profile.build_tool {
                bits.push(format!("- build tool {b}"));
            }
            if let Some(t) = &profile.test_framework {
                bits.push(format!("- test framework {t}"));
            }
            if !bits.is_empty() {
                body.push(format!("### Stack\n{}", bits.join("\n")));
            }
        }

        if wanted.contains(&Section::Commands) {
            let rows: Vec<String> = profile
                .commands
                .entries()
                .iter()
                .map(|(k, v)| format!("- **{k}**: `{v}`"))
                .collect();
            if !rows.is_empty() {
                body.push(format!(
                    "### Commands: use these exact strings\nSubstitute `{{MODULE}}`, `{{TEST}}`, and `{{PATH}}` \
                     placeholders. Never invent a different invocation.\n{}",
                    rows.join("\n")
                ));
            }
        }

        if wanted.contains(&Section::Testing) && !profile.test_types.is_empty() {
            let mut rows = vec![];
            for (name, t) in &profile.test_types {
                // The test stage runs a whole level for regression and single tests while it writes.
                let cmd = match (t.command.as_deref(), t.command_one.as_deref()) {
                    (Some(all), Some(one)) if all != one => format!("`{all}`; one test: `{one}`"),
                    (Some(c), _) | (None, Some(c)) => format!("`{c}`"),
                    (None, None) => "none".into(),
                };
                rows.push(format!("- **{name}**: {cmd}"));
                if !t.matches.is_empty() && t.matches.iter().any(|m| !stated_in(&inventory, m)) {
                    rows.push(format!(
                        "  - applies to {}",
                        truncate(&t.matches.join("; "), 150)
                    ));
                }
                if let Some(n) = t.note.as_deref().filter(|n| !stated_in(&inventory, n)) {
                    rows.push(format!("  - {}", truncate(n, 190)));
                }
                if let Some(r) = t.reports.as_deref().filter(|r| !stated_in(&inventory, r)) {
                    rows.push(format!("  - reports at {}", truncate(r, 90)));
                }
            }
            body.push(format!(
                "### Test types: pick the runner by what you are testing\n{}",
                rows.join("\n")
            ));
        }

        if wanted.contains(&Section::Skills) {
            let rows = skill_rows(input.agent, &profile.skills, &inventory, input.repo_root);
            if !rows.is_empty() {
                body.push(format!(
                    "### Skills\nLoad a skill by its name with the skill tool, or read the file at its path. \
                     Never search for skills or guess paths.\n{}",
                    rows.join("\n")
                ));
            }
        }

        if wanted.contains(&Section::Conventions) {
            let c = &profile.conventions;
            let mut rows = vec![];
            if let Some(n) = c.naming.as_deref().filter(|n| !stated_in(&inventory, n)) {
                rows.push(format!("- Naming: {}", truncate(n, 190)));
            }
            if let Some(k) = c
                .immutability_keyword
                .as_deref()
                .filter(|k| !stated_in(&inventory, k))
            {
                rows.push(format!("- Immutability: {}", truncate(k, 90)));
            }
            for note in c.notes.iter().filter(|n| !stated_in(&inventory, n)) {
                rows.push(format!("- {}", truncate(note, 190)));
            }
            if !rows.is_empty() {
                body.push(format!(
                    "### Conventions not stated in the inventory tables\n{}",
                    rows.join("\n")
                ));
            }
        }

        // Exempt from the containment test: for the reviewer this catalog is its whole basis for
        // judgment, and a partial one silently narrows what gets reviewed.
        if wanted.contains(&Section::Review) {
            let rows: Vec<String> = profile
                .review_rules
                .iter()
                .filter(|r| !r.id.is_empty() && !r.rule.is_empty())
                .map(|r| {
                    let fix = if r.auto_fixable { ", auto-fixable" } else { "" };
                    format!(
                        "- **{}** ({}{fix}): {}",
                        r.id,
                        r.severity,
                        truncate(&r.rule, 170)
                    )
                })
                .collect();
            if !rows.is_empty() {
                body.push(format!(
                    "### Review Rule Set: your complete rule catalog\nApply these IDs and severities. This is \
                     the whole set, not a summary.\n{}",
                    rows.join("\n")
                ));
            }
        }

        if wanted.contains(&Section::Modules) {
            let rows: Vec<String> =
                relevant_modules(&profile.module_map, &scope_hints(input.prompt))
                    .into_iter()
                    .map(|r| match &r.reference {
                        Some(reference) => {
                            format!("- `{}`: {} (reference: {reference})", r.glob, r.area)
                        }
                        None => format!("- `{}`: {}", r.glob, r.area),
                    })
                    .collect();
            if !rows.is_empty() {
                body.push(format!(
                    "### Module map rows covering the paths in your task\n{}",
                    rows.join("\n")
                ));
            }
        }

        if !body.is_empty() {
            let inventory_path = ostra_core::paths::project_inventory(input.repo_root);
            let mut brief = format!(
                "{BRIEF_HEADING}{}\n\nThese facts are resolved from this project's inventory and profile. Follow \
                 them directly and do not open `INVENTORY.md` or `project.toml` to re-read them.\n\n{}\n\nThe full \
                 inventory is at `{}` if you need a table this brief does not carry. Reading it is a fallback, \
                 not the first step.",
                input.agent,
                body.join("\n\n"),
                inventory_path.display()
            );
            if brief.chars().count() > MAX_BRIEF_CHARS {
                brief = brief.chars().take(MAX_BRIEF_CHARS).collect();
                brief.push_str("\n(brief truncated; see the inventory for the rest)");
            }
            out.push(brief);
        }
    }

    if !input.project_docs.is_empty() {
        let docs: Vec<String> = input
            .project_docs
            .iter()
            .map(|d| {
                let text = d.content.trim();
                let body = if text.chars().count() > MAX_PROJECT_DOC_CHARS {
                    let cut: String = text.chars().take(MAX_PROJECT_DOC_CHARS).collect();
                    format!("{cut}\n\n(truncated; read the file for the rest)")
                } else {
                    text.to_string()
                };
                format!("### `{}`\n\n{body}", d.path.display())
            })
            .collect();
        out.push(format!(
            "{PROJECT_DOCS_HEADING}\n\nThe project's own agent instruction files. Follow them for work in this \
             project unless they conflict with your own rules above, which win.\n\n{}",
            docs.join("\n\n")
        ));
    }

    // Rule O3: a created project has no inventory until its init runs, so its facts come from
    // the call that created it.
    if !input.new_projects.is_empty() {
        let rows: Vec<String> = input
            .new_projects
            .iter()
            .map(|p| {
                let reqs: Vec<String> = p.requirements.iter().map(|r| format!("  - {r}")).collect();
                format!(
                    "- `{}` at `{}`, stack {}: {}\n  Base requirements:\n{}",
                    p.key,
                    p.path.display(),
                    p.stack,
                    p.purpose,
                    reqs.join("\n")
                )
            })
            .collect();
        out.push(format!(
            "{NEW_PROJECTS_HEADING}\n\nAn agent created these projects in this session, and the user approved each one. Ostra \
             initializes each one right after it is created, before any other work runs in it. Until then its \
             folder may be empty and it has no inventory or profile to read, so take its facts from here.\n\n{}",
            rows.join("\n")
        ));
    }

    // Rule W1: every agent learns where the workspace artifacts are, so it can read them.
    if let Some(a) = input.artifacts.filter(|a| !a.entries.is_empty()) {
        let dir = a.dir.display();
        let mut rows: Vec<String> = a
            .entries
            .iter()
            .map(|(path, size)| format!("- `{dir}/{path}` ({size} bytes)"))
            .collect();
        if a.total > a.entries.len() {
            rows.push(format!(
                "- and {} more; find them with Glob in `{dir}`",
                a.total - a.entries.len()
            ));
        }
        out.push(format!(
            "{ARTIFACTS_HEADING}\n\nThe user keeps these files for every task in this workspace: documentation, \
             guidelines, skills, and sample data. Read the ones that bear on your task with Read. Do not write \
             in `{dir}`, because the folder belongs to the user. An artifact named `skills/<name>/SKILL.md` is a \
             skill: load it with the Skill tool by its path.\n\n{}",
            rows.join("\n")
        ));
    }

    let instructions: Vec<&String> = input
        .instructions
        .iter()
        .filter(|s| !s.trim().is_empty())
        .collect();
    if !instructions.is_empty() {
        let lines: Vec<String> = instructions.iter().map(|s| s.trim().to_string()).collect();
        out.push(format!(
            "{INSTRUCTIONS_HEADING}\n\nThe user set these for this workspace. Follow them unless they \
             conflict with your own rules above, which win.\n\n{}",
            lines.join("\n\n")
        ));
    }

    if out.is_empty() {
        None
    } else {
        Some(out.join("\n\n"))
    }
}

/// The first message with the brief appended. Idempotent: a message that already carries a brief is
/// returned unchanged.
pub fn augment(first_message: &str, input: &BriefInput<'_>) -> String {
    if [
        BRIEF_HEADING,
        PROJECT_DOCS_HEADING,
        ARTIFACTS_HEADING,
        INSTRUCTIONS_HEADING,
    ]
    .iter()
    .any(|h| first_message.contains(h))
    {
        return first_message.to_string();
    }
    match build_brief(input) {
        Some(brief) => format!("{}\n\n---\n\n{brief}\n", first_message.trim_end()),
        None => first_message.to_string(),
    }
}
