//! A workspace's projects: validating an import, and the facts the project list shows.

use crate::settings::field_issue;
use ostra_core::api::ImportProject;
use ostra_core::config::{ProjectEntry, ValidationIssue, WorkspaceSettings};
use ostra_core::paths;
use ostra_core::slug::{is_project_key, is_stack_name, stack_issue};
use std::path::{Path, PathBuf};

/// The trimmed key and stack of an import or clone request, with an issue on `key` or `stack` for
/// each problem.
pub fn key_and_stack(
    settings: &WorkspaceSettings,
    key: &str,
    stack: Option<&str>,
    issues: &mut Vec<ValidationIssue>,
) -> (String, Option<String>) {
    let key = key.trim();
    if !is_project_key(key) {
        issues.push(field_issue("key", format!("`{key}` is not a project key. Use lowercase letters, digits, and dashes, starting with a letter or digit.")));
    } else if settings.project(key).is_some() {
        issues.push(field_issue(
            "key",
            format!(
                "A project named `{key}` already exists in this workspace. Choose another key."
            ),
        ));
    }
    let stack = stack.map(str::trim).filter(|s| !s.is_empty());
    if let Some(s) = stack.filter(|s| !is_stack_name(s)) {
        issues.push(field_issue("stack", stack_issue(s)));
    }
    (key.to_string(), stack.map(str::to_string))
}

/// The settings entry an import request adds, or every problem with the request, each on its
/// field: `key`, `path`, or `stack`.
pub fn import_entry(
    settings: &WorkspaceSettings,
    root: &Path,
    req: &ImportProject,
) -> Result<ProjectEntry, Vec<ValidationIssue>> {
    let mut issues = vec![];
    let (key, stack) = key_and_stack(settings, &req.key, req.stack.as_deref(), &mut issues);
    let path = if !req.path.is_absolute() {
        issues.push(field_issue("path", "Use an absolute path.".into()));
        None
    } else {
        match ostra_core::paths::canonical(&req.path) {
            Err(_) => {
                issues.push(field_issue(
                    "path",
                    format!("{} does not exist.", req.path.display()),
                ));
                None
            }
            Ok(p) if !p.is_dir() => {
                issues.push(field_issue(
                    "path",
                    format!("{} is not a folder.", p.display()),
                ));
                None
            }
            Ok(p) => {
                if let Some(other) = settings.projects.iter().find(|o| o.path == p) {
                    issues.push(field_issue(
                        "path",
                        format!("That folder is already imported as `{}`.", other.key),
                    ));
                } else if paths::is_inside(&root.join(paths::RUNTIME_DIR), &p) {
                    issues.push(field_issue(
                        "path",
                        "A project cannot live inside the workspace's .ostra directory.".into(),
                    ));
                }
                Some(p)
            }
        }
    };
    match path {
        Some(path) if issues.is_empty() => Ok(ProjectEntry {
            key,
            path,
            stack,
            code_provider: None,
            language_servers: vec![],
        }),
        _ => Err(issues),
    }
}

/// The branch `.git/HEAD` names, or the short commit id of a detached HEAD. Reads the file
/// directly, because a git process per project per workspace read is too slow. A `.git` file
/// (a worktree or submodule) points at the real git dir.
pub fn git_branch(project: &Path) -> Option<String> {
    let dot = project.join(".git");
    let dir = if dot.is_file() {
        let text = std::fs::read_to_string(&dot).ok()?;
        let target = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if target.is_absolute() {
            target
        } else {
            project.join(target)
        }
    } else {
        dot
    };
    let head = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        return Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string());
    }
    (head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit())).then(|| head[..7].to_string())
}

pub(crate) fn ultracode_bootstrap(path: &Path) -> bool {
    let uc = path.join(".ultracode");
    uc.join("INVENTORY.md").exists() && uc.join("repo-profile.json").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: &Path, key: &str, stack: Option<&str>) -> ImportProject {
        ImportProject {
            path: path.to_path_buf(),
            key: key.into(),
            stack: stack.map(str::to_string),
        }
    }

    fn fields(r: Result<ProjectEntry, Vec<ValidationIssue>>) -> Vec<String> {
        r.err()
            .unwrap_or_default()
            .into_iter()
            .map(|i| i.path)
            .collect()
    }

    #[test]
    fn import_problems_name_their_field() {
        let dir = tempfile::tempdir().unwrap();
        let base = ostra_core::paths::canonical(dir.path()).unwrap();
        let (root, app) = (base.join("ws"), base.join("app"));
        std::fs::create_dir_all(root.join(".ostra/inner")).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(base.join("file.txt"), "x").unwrap();
        let mut settings = WorkspaceSettings::seeded("ws");
        settings.projects.push(ProjectEntry {
            key: "api".into(),
            path: base.join("api"),
            stack: None,
            code_provider: None,
            language_servers: vec![],
        });

        let ok = import_entry(&settings, &root, &request(&app, " app ", Some(" "))).unwrap();
        assert_eq!(
            (ok.key.as_str(), ok.path.clone(), ok.stack.clone()),
            ("app", app.clone(), None)
        );
        assert_eq!(
            import_entry(&settings, &root, &request(&app, "app", Some("rust-axum")))
                .unwrap()
                .stack
                .as_deref(),
            Some("rust-axum")
        );

        assert_eq!(
            fields(import_entry(
                &settings,
                &root,
                &request(&app, "App", Some("Go\u{7}Lang"))
            )),
            ["key", "stack"]
        );
        assert_eq!(
            fields(import_entry(&settings, &root, &request(&app, "api", None))),
            ["key"]
        );
        assert_eq!(
            fields(import_entry(
                &settings,
                &root,
                &request(Path::new("rel"), "b", None)
            )),
            ["path"]
        );
        assert_eq!(
            fields(import_entry(
                &settings,
                &root,
                &request(&base.join("nope"), "b", None)
            )),
            ["path"]
        );
        assert_eq!(
            fields(import_entry(
                &settings,
                &root,
                &request(&base.join("file.txt"), "b", None)
            )),
            ["path"]
        );
        assert_eq!(
            fields(import_entry(
                &settings,
                &root,
                &request(&root.join(".ostra/inner"), "b", None)
            )),
            ["path"]
        );
        settings.projects.push(ok);
        let again = import_entry(&settings, &root, &request(&app, "other", None)).unwrap_err();
        assert_eq!(
            (again[0].path.as_str(), again[0].message.as_str()),
            ("path", "That folder is already imported as `app`.")
        );
    }

    #[test]
    fn branch_comes_from_head() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        assert_eq!(git_branch(p), None, "not a repository");
        std::fs::create_dir_all(p.join(".git")).unwrap();
        std::fs::write(p.join(".git/HEAD"), "ref: refs/heads/feature/login\n").unwrap();
        assert_eq!(git_branch(p).as_deref(), Some("feature/login"));
        std::fs::write(
            p.join(".git/HEAD"),
            "3f2a9c1d0e4b5a6978877665544332211aabbccd\n",
        )
        .unwrap();
        assert_eq!(
            git_branch(p).as_deref(),
            Some("3f2a9c1"),
            "a detached HEAD shows its short id"
        );
        std::fs::write(p.join(".git/HEAD"), "garbage").unwrap();
        assert_eq!(git_branch(p), None);

        let wt = p.join("wt");
        std::fs::create_dir_all(p.join("gitdirs/wt")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(p.join("gitdirs/wt/HEAD"), "ref: refs/heads/topic\n").unwrap();
        std::fs::write(wt.join(".git"), "gitdir: ../gitdirs/wt\n").unwrap();
        assert_eq!(
            git_branch(&wt).as_deref(),
            Some("topic"),
            "a .git file points at the real git dir"
        );
    }
}
