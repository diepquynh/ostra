//! Per-project skills: the `skills` list in `project.toml` plus the files under `.agents/skills/` and
//! the older `.ostra/skills/`, and skills that sit in a harness's own directory until they are adopted.

use crate::api::ApiErr;
use crate::workspace::WorkspaceRt;
use axum::http::StatusCode;
use ostra_core::api::{
    InitStatus, ProjectSkills, SkillAdopt, SkillDoc, SkillOrigin, SkillSave, SkillView,
};
use ostra_core::config::{ProjectProfile, SkillEntry, load_toml, save_toml};
use ostra_core::paths;
use std::path::{Path, PathBuf};

pub const KINDS: &[&str] = &["convention", "module-hub", "creation", "test", "other"];

/// Harness skill directories, relative to the project root.
pub const HARNESS_DIRS: &[&str] = &[
    ".claude/skills",
    ".codex/skills",
    ".grok/skills",
    ".gemini/skills",
    ".ultracode/skills",
];

const MAX_SKILL_BYTES: usize = 512 * 1024;

fn bad(m: impl Into<String>) -> ApiErr {
    ApiErr::new(StatusCode::BAD_REQUEST, m)
}

fn not_found(m: impl Into<String>) -> ApiErr {
    ApiErr::new(StatusCode::NOT_FOUND, m)
}

fn conflict(m: impl Into<String>) -> ApiErr {
    ApiErr::new(StatusCode::CONFLICT, m)
}

fn io(e: impl std::fmt::Display) -> ApiErr {
    ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub fn list(w: &WorkspaceRt) -> Vec<ProjectSkills> {
    w.project_views()
        .into_iter()
        .map(|p| {
            let blocked = blocked_reason(&p.init_status);
            let skills = if p.init_status == InitStatus::Missing {
                vec![]
            } else {
                scan(&p.path)
            };
            ProjectSkills {
                project: p.key,
                path: p.path,
                blocked,
                skills,
            }
        })
        .collect()
}

fn blocked_reason(status: &InitStatus) -> Option<String> {
    match status {
        InitStatus::Missing => Some("The project folder is missing, so its skills cannot be read.".into()),
        InitStatus::Initializing => {
            Some("Wait for the init session to finish before you change skills, because the initializer writes them.".into())
        }
        _ => None,
    }
}

/// The project's root, refusing changes while its folder is missing or an init session runs.
fn editable_root(w: &WorkspaceRt, key: &str) -> Result<PathBuf, ApiErr> {
    let view = w
        .project_views()
        .into_iter()
        .find(|p| p.key == key)
        .ok_or_else(|| not_found(format!("No project {key}.")))?;
    match blocked_reason(&view.init_status) {
        Some(reason) => Err(conflict(reason)),
        None => Ok(view.path),
    }
}

fn read_root(w: &WorkspaceRt, key: &str) -> Result<PathBuf, ApiErr> {
    w.project_path(key)
        .filter(|p| p.is_dir())
        .ok_or_else(|| not_found(format!("No project {key}.")))
}

pub fn check_name(name: &str) -> Result<(), ApiErr> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(bad(format!(
            "Name the skill with letters, digits, `-`, or `_` (at most 64, starting with a letter or digit), because it becomes a directory name: `{name}` is not allowed."
        )))
    }
}

fn check_kind(kind: &str) -> Result<(), ApiErr> {
    if KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(bad(format!(
            "Set the kind to one of {}: `{kind}` is not a skill kind.",
            KINDS.join(", ")
        )))
    }
}

fn load_profile(root: &Path) -> Result<ProjectProfile, ApiErr> {
    load_toml(&paths::project_profile(root))
        .map_err(|e| conflict(format!("Fix `.ostra/project.toml` first: {e}")))
}

fn ostra_rel(name: &str) -> String {
    format!("{}/{name}/SKILL.md", paths::SKILLS_DIR)
}

/// Where the named skill's SKILL.md is, relative to the root: the dir that already holds it, else
/// `.agents/skills/`.
fn located_rel(root: &Path, name: &str) -> String {
    paths::SKILL_DIRS
        .iter()
        .map(|d| format!("{d}/{name}/SKILL.md"))
        .find(|rel| root.join(rel).is_file())
        .unwrap_or_else(|| ostra_rel(name))
}

/// The `description` from YAML frontmatter, including a folded (`>`) or literal (`|`) block.
pub fn frontmatter_description(text: &str) -> Option<String> {
    let body = text
        .strip_prefix("---")?
        .trim_start_matches(['\r', ' '])
        .strip_prefix('\n')?;
    let end = body.find("\n---").unwrap_or(body.len());
    let lines: Vec<&str> = body[..end].lines().collect();
    let i = lines.iter().position(|l| l.starts_with("description:"))?;
    let value = lines[i]["description:".len()..].trim();
    let text = if value.is_empty() || value.starts_with('>') || value.starts_with('|') {
        lines[i + 1..]
            .iter()
            .take_while(|l| l.starts_with(' ') || l.starts_with('\t') || l.is_empty())
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        value.trim_matches(|c| c == '"' || c == '\'').to_string()
    };
    (!text.is_empty()).then_some(text)
}

/// `rel` under the project, with symlinks resolved, or `None` when it leaves the project: skill
/// paths come from `project.toml`, which the repository itself can supply.
fn in_project(root: &Path, rel: &str) -> Option<PathBuf> {
    let root = std::fs::canonicalize(root).ok()?;
    crate::files::tree::contain(&root, rel).ok().map(|c| c.real)
}

fn describe(path: &Path) -> Option<String> {
    frontmatter_description(&std::fs::read_to_string(path).ok()?)
}

/// Skill directories (holding a SKILL.md) one level under `dir`, sorted by name.
fn skill_dirs(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && e.path().join("SKILL.md").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

pub fn scan(root: &Path) -> Vec<SkillView> {
    let profile: ProjectProfile = load_toml(&paths::project_profile(root)).unwrap_or_default();
    let mut out: Vec<SkillView> = profile
        .skills
        .iter()
        .map(|e| {
            let file = in_project(root, &e.path);
            SkillView {
                name: e.name.clone(),
                description: file.as_deref().and_then(describe),
                path: e.path.clone(),
                origin: SkillOrigin::Ostra,
                entry: Some(e.clone()),
                exists: file.is_some_and(|f| f.is_file()),
            }
        })
        .collect();
    for dir in paths::SKILL_DIRS {
        for name in skill_dirs(&root.join(dir)) {
            let rel = format!("{dir}/{name}/SKILL.md");
            if out.iter().any(|s| s.path == rel || s.name == name) {
                continue;
            }
            out.push(SkillView {
                description: describe(&root.join(&rel)),
                name,
                path: rel,
                origin: SkillOrigin::Ostra,
                entry: None,
                exists: true,
            });
        }
    }
    for dir in HARNESS_DIRS {
        for name in skill_dirs(&root.join(dir)) {
            let rel = format!("{dir}/{name}/SKILL.md");
            out.push(SkillView {
                description: describe(&root.join(&rel)),
                name,
                path: rel,
                origin: SkillOrigin::Harness,
                entry: None,
                exists: true,
            });
        }
    }
    out
}

fn view(root: &Path, name: &str) -> Option<SkillView> {
    scan(root)
        .into_iter()
        .find(|s| s.origin == SkillOrigin::Ostra && s.name == name)
}

pub fn get(w: &WorkspaceRt, key: &str, name: &str) -> Result<SkillDoc, ApiErr> {
    let root = read_root(w, key)?;
    let skill =
        view(&root, name).ok_or_else(|| not_found(format!("No skill `{name}` in {key}.")))?;
    let content = if skill.exists {
        let file = in_project(&root, &skill.path)
            .ok_or_else(|| conflict(format!("Move skill `{name}` inside the project: its path leaves it.")))?;
        std::fs::read_to_string(file).map_err(io)?
    } else {
        String::new()
    };
    Ok(SkillDoc { skill, content })
}

/// Read a harness skill that has not been adopted yet, by its path.
pub fn get_harness(w: &WorkspaceRt, key: &str, rel: &str) -> Result<SkillDoc, ApiErr> {
    let root = read_root(w, key)?;
    let skill = scan(&root)
        .into_iter()
        .find(|s| s.origin == SkillOrigin::Harness && s.path == rel)
        .ok_or_else(|| not_found(format!("No harness skill at `{rel}` in {key}.")))?;
    let file = in_project(&root, &skill.path)
        .ok_or_else(|| not_found(format!("No harness skill at `{rel}` in {key}.")))?;
    let content = std::fs::read_to_string(file).map_err(io)?;
    Ok(SkillDoc { skill, content })
}

fn upsert_entry(
    root: &Path,
    name: &str,
    kind: &str,
    component_type: Option<String>,
    source: &str,
) -> Result<(), ApiErr> {
    let mut profile = load_profile(root)?;
    let component_type = component_type
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    match profile.skills.iter_mut().find(|e| e.name == name) {
        Some(e) => {
            e.kind = kind.into();
            e.component_type = component_type;
            if e.path.is_empty() {
                e.path = located_rel(root, name);
            }
        }
        None => profile.skills.push(SkillEntry {
            name: name.into(),
            kind: kind.into(),
            path: located_rel(root, name),
            component_type,
            source: Some(source.into()),
        }),
    }
    save_toml(&paths::project_profile(root), &profile).map_err(io)
}

pub fn save(w: &WorkspaceRt, key: &str, name: &str, body: &SkillSave) -> Result<SkillDoc, ApiErr> {
    check_name(name)?;
    check_kind(&body.kind)?;
    if body.content.trim().is_empty() {
        return Err(bad(
            "Write the skill's instructions: SKILL.md cannot be empty.",
        ));
    }
    if body.content.len() > MAX_SKILL_BYTES {
        return Err(bad(
            "Keep SKILL.md under 512 KB and move long material into files next to it, because agents read it whole.",
        ));
    }
    let root = editable_root(w, key)?;
    let rel = load_profile(&root)?
        .skills
        .into_iter()
        .find(|e| e.name == name && !e.path.is_empty())
        .map(|e| e.path)
        .unwrap_or_else(|| located_rel(&root, name));
    // Resolved, so a symlinked `.agents` or `.ostra` cannot carry the write out of the project.
    let file = in_project(&root, &rel);
    let writable = [
        paths::project_runtime(&root),
        paths::project_skills_dir(&root),
    ]
    .map(|d| paths::resolve(&d, &d));
    let Some(file) = file.filter(|f| writable.iter().any(|d| f.starts_with(d))) else {
        return Err(conflict(format!(
            "Edit `{rel}` in the Files tab: Ostra writes skills only under `{}/` and `.ostra/`.",
            paths::SKILLS_DIR
        )));
    };
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    std::fs::write(&file, &body.content).map_err(io)?;
    upsert_entry(&root, name, &body.kind, body.component_type.clone(), "user")?;
    get(w, key, name)
}

pub fn delete(w: &WorkspaceRt, key: &str, name: &str) -> Result<(), ApiErr> {
    check_name(name)?;
    let root = editable_root(w, key)?;
    let mut profile = load_profile(&root)?;
    let before = profile.skills.len();
    profile.skills.retain(|e| e.name != name);
    let dirs: Vec<PathBuf> = paths::project_skill_dirs(&root)
        .into_iter()
        .map(|d| d.join(name))
        .filter(|d| d.exists())
        .collect();
    if profile.skills.len() == before && dirs.is_empty() {
        return Err(not_found(format!("No skill `{name}` in {key}.")));
    }
    if profile.skills.len() != before {
        save_toml(&paths::project_profile(&root), &profile).map_err(io)?;
    }
    for dir in dirs {
        std::fs::remove_dir_all(&dir).map_err(io)?;
    }
    Ok(())
}

pub fn adopt(
    w: &WorkspaceRt,
    key: &str,
    name: &str,
    body: &SkillAdopt,
) -> Result<SkillDoc, ApiErr> {
    check_name(name)?;
    check_kind(&body.kind)?;
    let root = editable_root(w, key)?;
    let source = scan(&root)
        .into_iter()
        .find(|s| s.origin == SkillOrigin::Harness && s.path == body.from)
        .ok_or_else(|| not_found(format!("No harness skill at `{}` in {key}.", body.from)))?;
    let target = paths::project_skills_dir(&root).join(name);
    let taken = paths::project_skill_dirs(&root)
        .iter()
        .any(|d| d.join(name).exists());
    if taken || load_profile(&root)?.skills.iter().any(|e| e.name == name) {
        return Err(conflict(format!(
            "Pick another name: `{name}` is already a skill in {key}."
        )));
    }
    let from_dir = root
        .join(&source.path)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| bad("The skill has no directory."))?;
    copy_dir(&from_dir, &target).map_err(io)?;
    upsert_entry(
        &root,
        name,
        &body.kind,
        body.component_type.clone(),
        "adopted",
    )?;
    get(w, key, name)
}

/// Copy regular files and directories; symlinks are skipped so a copy cannot reach outside the skill.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let t = e.file_type()?;
        if t.is_dir() {
            copy_dir(&e.path(), &to.join(e.file_name()))?;
        } else if t.is_file() {
            std::fs::copy(e.path(), to.join(e.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions() {
        assert_eq!(
            frontmatter_description("---\nname: a\ndescription: Use it.\n---\n# A").as_deref(),
            Some("Use it.")
        );
        assert_eq!(
            frontmatter_description("---\ndescription: \"Quoted\"\n---\n").as_deref(),
            Some("Quoted")
        );
        assert_eq!(
            frontmatter_description("---\ndescription: >\n  Two\n  lines\nname: x\n---\n")
                .as_deref(),
            Some("Two lines")
        );
        assert_eq!(
            frontmatter_description("# No frontmatter\ndescription: x"),
            None
        );
    }

    #[test]
    fn names() {
        assert!(check_name("tool-handler").is_ok());
        assert!(check_name("a_b2").is_ok());
        for bad in ["", "-x", "../x", "a/b", "a.b", &"x".repeat(65)] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn scan_merges_registered_unregistered_and_harness_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let write = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write(
            ".agents/skills/entity/SKILL.md",
            "---\ndescription: Entities.\n---\n",
        );
        write(".agents/skills/fresh/SKILL.md", "# Fresh\n");
        write(".ostra/skills/loose/SKILL.md", "# Loose\n");
        write(".ostra/skills/fresh/SKILL.md", "# Shadowed\n");
        write(
            ".claude/skills/deploy/SKILL.md",
            "---\ndescription: Deploys.\n---\n",
        );
        let profile = ProjectProfile {
            skills: vec![
                SkillEntry {
                    name: "entity".into(),
                    kind: "creation".into(),
                    path: ostra_rel("entity"),
                    ..Default::default()
                },
                SkillEntry {
                    name: "gone".into(),
                    kind: "other".into(),
                    path: ostra_rel("gone"),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        save_toml(&paths::project_profile(root), &profile).unwrap();
        let got: Vec<(String, SkillOrigin, bool, bool, Option<String>)> = scan(root)
            .into_iter()
            .map(|s| (s.name, s.origin, s.entry.is_some(), s.exists, s.description))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "entity".into(),
                    SkillOrigin::Ostra,
                    true,
                    true,
                    Some("Entities.".into())
                ),
                ("gone".into(), SkillOrigin::Ostra, true, false, None),
                ("fresh".into(), SkillOrigin::Ostra, false, true, None),
                ("loose".into(), SkillOrigin::Ostra, false, true, None),
                (
                    "deploy".into(),
                    SkillOrigin::Harness,
                    false,
                    true,
                    Some("Deploys.".into())
                ),
            ]
        );
    }
}
