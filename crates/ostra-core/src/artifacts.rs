//! Workspace artifacts (HANDOVER 6.5): files the user keeps for every session of a workspace, such
//! as custom skills, documentation, guidelines, and sample data. Visible artifacts live in
//! `<workspace>/.ostra/artifacts/`. A hidden artifact moves into Ostra's data dir, which no agent,
//! harness, or sandboxed command can read (Rule W2).

use crate::paths;
use chrono::{DateTime, Utc};
use regex::Regex;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

/// The tag root that names an artifact in request text and instructions: `@_artifacts/<path>`.
/// No project key can take it, because project keys start with a letter or digit.
pub const TAG_ROOT: &str = "_artifacts";
/// Largest artifact the user may upload or save.
pub const MAX_ARTIFACT_BYTES: usize = 25 * 1024 * 1024;
/// Artifacts one workspace may hold, visible and hidden together.
pub const MAX_ARTIFACTS: usize = 2000;
/// Artifacts the repo brief names; the rest are found with Glob in the folder.
pub const MAX_BRIEF_ARTIFACTS: usize = 40;

/// Where a workspace keeps its visible artifacts.
pub fn dir(workspace: &Path) -> PathBuf {
    paths::workspace_runtime(workspace).join("artifacts")
}

/// Where a workspace keeps its hidden artifacts: inside the data dir, which agents never read.
pub fn hidden_dir(workspace_id: &str) -> PathBuf {
    paths::data_dir()
        .join("hidden-artifacts")
        .join(workspace_id)
}

/// The hidden units of a workspace: each folder or file the user hid, as a path inside the
/// artifacts folder. Kept next to the hidden folder, in the data dir, so no agent can change it.
pub fn hidden_units_file(workspace_id: &str) -> PathBuf {
    paths::data_dir()
        .join("hidden-artifacts")
        .join(format!("{workspace_id}.units.json"))
}

/// `path` is `unit` or lies inside it.
pub fn is_within(path: &str, unit: &str) -> bool {
    path == unit || path.starts_with(&format!("{unit}/"))
}

/// Rule W2: a path is hidden when it or a folder above it is a hidden unit. Everything under a
/// hidden folder is hidden, including files added later.
pub fn is_hidden(units: &[String], path: &str) -> bool {
    units.iter().any(|u| is_within(path, u))
}

/// Units sorted, each once, without a unit that lies inside another.
pub fn tidy_units(mut units: Vec<String>) -> Vec<String> {
    units.sort();
    units.dedup();
    let all = units.clone();
    units.retain(|u| !all.iter().any(|o| o != u && is_within(u, o)));
    units
}

/// One artifact file: its `/`-separated path inside the artifacts folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
}

/// Every file under `root`, sorted by path. Links are not followed, because an artifact is a file
/// the user put there, and a link could name any file on the machine.
pub fn list(root: &Path) -> Vec<Entry> {
    let mut out = vec![];
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in read.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if kind.is_dir() {
                stack.push((e.path(), rel));
            } else if kind.is_file() {
                let meta = e.metadata().ok();
                out.push(Entry {
                    path: rel,
                    size: meta.as_ref().map_or(0, |m| m.len()),
                    modified: meta
                        .and_then(|m| m.modified().ok())
                        .map(DateTime::<Utc>::from),
                });
            }
            if out.len() >= MAX_ARTIFACTS {
                break;
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// An artifact path made canonical: relative, `/`-separated, with no `.`, `..`, or empty segment,
/// and no segment starting with a dot. The error is the message for the user.
pub fn normalize(raw: &str) -> Result<String, String> {
    let bad = || {
        format!(
            "Name the artifact with a relative path such as `guides/style.md`. `{raw}` is not one."
        )
    };
    let rel = raw.trim().replace('\\', "/");
    if rel.starts_with('/') {
        return Err(bad());
    }
    let rel = rel.trim_start_matches("./").trim_end_matches('/');
    if rel.is_empty() || Path::new(rel).is_absolute() {
        return Err(bad());
    }
    let mut parts = vec![];
    for c in Path::new(rel).components() {
        let Component::Normal(s) = c else {
            return Err(bad());
        };
        let s = s.to_string_lossy();
        if s.starts_with('.') || s.chars().any(|c| c.is_control()) {
            return Err(bad());
        }
        parts.push(s.to_string());
    }
    Ok(parts.join("/"))
}

/// The visible artifact at `rel`, when it is a regular file inside the folder.
pub fn visible_file(workspace: &Path, rel: &str) -> Option<PathBuf> {
    let rel = normalize(rel).ok()?;
    let root = crate::paths::canonical(dir(workspace)).ok()?;
    let full = root.join(&rel);
    let meta = std::fs::symlink_metadata(&full).ok()?;
    (meta.is_file() && crate::paths::canonical(&full).ok()?.starts_with(&root)).then_some(full)
}

/// A custom skill kept as `skills/<name>/SKILL.md` in the workspace's artifacts.
pub fn skill_path(workspace: &Path, name: &str) -> Option<PathBuf> {
    if workspace.as_os_str().is_empty() {
        return None;
    }
    visible_file(workspace, &format!("skills/{name}/SKILL.md"))
}

static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\s)@([A-Za-z0-9._-]+)/(\S+)").expect("static regex"));
const TRAILING: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'', '`'];

/// Every `@key/path` tag in `text`, in order, each once, sentence punctuation after it removed.
pub fn tags(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for c in TAG.captures_iter(text) {
        let path = c[2].trim_end_matches(TRAILING).to_string();
        let tag = (c[1].to_string(), path);
        if !tag.1.is_empty() && !out.contains(&tag) {
            out.push(tag);
        }
    }
    out
}

/// Rule W3: the files that tags in an instruction name, as `(tag, absolute path)`. A tag names a
/// visible artifact or an existing file or folder of a project; any other tag stays plain text.
pub fn tagged_files(
    text: &str,
    workspace: &Path,
    projects: &[(String, PathBuf)],
) -> Vec<(String, PathBuf)> {
    let mut out = vec![];
    for (key, path) in tags(text) {
        let tag = format!("@{key}/{path}");
        if key == TAG_ROOT {
            if let Some(full) = visible_file(workspace, &path) {
                out.push((tag, full));
            }
            continue;
        }
        let Some((_, root)) = projects.iter().find(|(k, _)| *k == key) else {
            continue;
        };
        let Ok(rel) = normalize(path.trim_end_matches('/')) else {
            continue;
        };
        let (Ok(root), Ok(full)) = (crate::paths::canonical(root), crate::paths::canonical(root.join(&rel))) else {
            continue;
        };
        if full.starts_with(&root) {
            out.push((tag, root.join(rel)));
        }
    }
    out
}

/// Why an instruction's artifact tag cannot be saved: it names a hidden or missing artifact.
pub fn tag_issues(text: &str, workspace: &Path, workspace_id: &str) -> Vec<String> {
    tags(text)
        .into_iter()
        .filter(|(key, _)| key == TAG_ROOT)
        .filter_map(|(_, path)| {
            if visible_file(workspace, &path).is_some() {
                return None;
            }
            let hidden = normalize(&path)
                .ok()
                .is_some_and(|rel| hidden_dir(workspace_id).join(rel).is_file());
            Some(if hidden {
                format!(
                    "Unhide the artifact `{path}` or remove its tag, because agents never see a hidden artifact."
                )
            } else {
                format!(
                    "Add the artifact `{path}` on the Artifacts page or remove its tag, because the workspace has no such artifact."
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_normalized_or_refused() {
        assert_eq!(normalize("./guides\\style.md").unwrap(), "guides/style.md");
        assert_eq!(
            normalize("skills/x/SKILL.md/").unwrap(),
            "skills/x/SKILL.md"
        );
        for bad in ["", "/etc/passwd", "../x", "a/../b", ".env", "a/.git/config"] {
            assert!(normalize(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_hidden_folder_hides_everything_inside_it() {
        let units = tidy_units(vec![
            "notes".into(),
            "notes/a.md".into(),
            "data/x.csv".into(),
        ]);
        assert_eq!(units, ["data/x.csv", "notes"]);
        assert!(is_hidden(&units, "notes"));
        assert!(is_hidden(&units, "notes/later/b.md"));
        assert!(!is_hidden(&units, "notesheet.md"));
        assert!(!is_hidden(&units, "data"));
        assert!(is_hidden(&units, "data/x.csv"));
    }

    #[test]
    fn tags_drop_trailing_punctuation_and_repeats() {
        assert_eq!(
            tags("Follow @_artifacts/style.md, and @web/src/. Again @_artifacts/style.md"),
            [
                ("_artifacts".to_string(), "style.md".to_string()),
                ("web".to_string(), "src/".to_string()),
            ]
        );
        assert!(tags("mail me@host/x").is_empty());
    }

    #[test]
    fn tagged_files_resolve_visible_artifacts_and_project_files() {
        let ws = tempfile::tempdir().unwrap();
        let root = crate::paths::canonical(ws.path()).unwrap();
        std::fs::create_dir_all(dir(&root).join("guides")).unwrap();
        std::fs::write(dir(&root).join("guides/style.md"), "x").unwrap();
        let project = root.join("web");
        std::fs::create_dir_all(project.join("src")).unwrap();
        let got = tagged_files(
            "See @_artifacts/guides/style.md and @web/src/ and @_artifacts/gone.md and @api/x",
            &root,
            &[("web".into(), project.clone())],
        );
        assert_eq!(
            got,
            [
                (
                    "@_artifacts/guides/style.md".to_string(),
                    dir(&root).join("guides/style.md")
                ),
                ("@web/src/".to_string(), project.join("src")),
            ]
        );
        assert_eq!(tag_issues("@_artifacts/gone.md", &root, "w1").len(), 1);
        assert!(tag_issues("@_artifacts/guides/style.md", &root, "w1").is_empty());
    }

    #[test]
    fn listing_skips_links_and_sorts() {
        let ws = tempfile::tempdir().unwrap();
        let root = dir(ws.path());
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("b/two.txt"), "22").unwrap();
        std::fs::write(root.join("a.md"), "1").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hostname", root.join("link")).unwrap();
        let got: Vec<(String, u64)> = list(&root).into_iter().map(|e| (e.path, e.size)).collect();
        assert_eq!(got, [("a.md".into(), 1), ("b/two.txt".into(), 2)]);
        assert!(visible_file(ws.path(), "link").is_none());
    }
}
