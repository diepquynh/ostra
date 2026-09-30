//! Rule B9: measure a project's tracked source by module-map area, so the docs stage can split a
//! large project's part of the book among writers. The runner calls this for `Step::PlanDocs` and
//! records the result in `DocsPlanned`, because the fold cannot read the file system.

use globset::{Glob, GlobMatcher};
use ostra_core::book::{DocsArea, plan_areas};
use ostra_core::config::ModuleRow;
use std::path::Path;
use std::process::Command;

/// Files larger than this are data or generated output, not source a writer documents.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    "vendor",
    "__pycache__",
];
const SKIP_NAMES: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "go.sum",
    "poetry.lock",
];
const SKIP_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "ico", "svg", "pdf", "zip", "gz", "tar", "woff", "woff2",
    "ttf", "otf", "mp4", "mp3", "wasm", "so", "dylib", "dll", "exe", "bin", "lock", "map",
];

/// The planned areas of the project at `project` and the IDs of those holding a file in `changed`
/// (paths relative to the repository root, as implementers report them).
pub fn plan(
    project: &Path,
    module_map: &[ModuleRow],
    changed: &[String],
) -> (Vec<DocsArea>, Vec<String>) {
    let files = tracked_files(project);
    let rows = rows(module_map, &files);
    let matchers: Vec<(usize, GlobMatcher)> = rows
        .iter()
        .enumerate()
        .flat_map(|(i, (_, globs))| {
            globs
                .iter()
                .filter_map(move |g| Glob::new(g).ok().map(|g| (i, g.compile_matcher())))
        })
        .collect();
    let area_of = |path: &str| {
        matchers
            .iter()
            .find(|(_, m)| m.is_match(path))
            .map(|(i, _)| *i)
    };
    let mut bytes = vec![0u64; rows.len()];
    let mut rest = 0u64;
    for (path, size) in &files {
        match area_of(path) {
            Some(i) => bytes[i] += size,
            None => rest += size,
        }
    }
    let measured: Vec<(String, Vec<String>, u64)> = rows
        .iter()
        .zip(&bytes)
        .map(|((name, globs), b)| (name.clone(), globs.clone(), *b))
        .collect();
    let areas = plan_areas(&measured, rest);
    if areas.is_empty() {
        return (areas, vec![]);
    }
    let prefix = repo_prefix(project);
    let mut touched = vec![];
    for path in changed {
        let rel = path.strip_prefix(prefix.as_str()).unwrap_or(path);
        let owner = match area_of(rel) {
            Some(i) => areas
                .iter()
                .find(|a| rows[i].1.iter().any(|g| a.globs.contains(g))),
            None => areas.iter().find(|a| a.rest),
        };
        if let Some(a) = owner
            && !touched.contains(&a.id)
        {
            touched.push(a.id.clone());
        }
    }
    (areas, touched)
}

/// Module-map areas in first-seen order with their globs. A project without a module map is
/// split by its top-level folders.
fn rows(module_map: &[ModuleRow], files: &[(String, u64)]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = vec![];
    if module_map.is_empty() {
        for (path, _) in files {
            if let Some((dir, _)) = path.split_once('/')
                && !out.iter().any(|(n, _)| n == dir)
            {
                out.push((dir.to_string(), vec![format!("{dir}/**")]));
            }
        }
        return out;
    }
    for row in module_map {
        match out.iter_mut().find(|(n, _)| *n == row.area) {
            Some((_, globs)) => globs.push(row.glob.clone()),
            None => out.push((row.area.clone(), vec![row.glob.clone()])),
        }
    }
    out
}

fn skipped(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.starts_with(".ostra/")
        || path
            .split('/')
            .any(|c| SKIP_DIRS.contains(&c) || (c.starts_with('.') && c.len() > 1))
        || SKIP_NAMES.contains(&name)
        || name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| SKIP_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Project-relative `/`-separated paths and sizes of the files git tracks or would track, or
/// every file under the project when it is not a git checkout.
fn tracked_files(project: &Path) -> Vec<(String, u64)> {
    let listed = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(project)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split('\0')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
    let paths = listed.unwrap_or_else(|| walk(project));
    paths
        .into_iter()
        .filter(|p| !skipped(p))
        .filter_map(|p| {
            let len = std::fs::metadata(project.join(&p)).ok()?.len();
            (len <= MAX_FILE_BYTES).then_some((p, len))
        })
        .collect()
}

fn walk(root: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(rel) = p.strip_prefix(root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if skipped(&rel) {
                continue;
            }
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => out.push(rel),
                _ => {}
            }
        }
    }
    out
}

/// The project's folder relative to its repository root, `/`-terminated, or empty at the root.
fn repo_prefix(project: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "--show-prefix"])
        .current_dir(project)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, bytes: usize) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x".repeat(bytes)).unwrap();
    }

    #[test]
    fn a_large_project_splits_by_its_module_map_and_marks_touched_areas() {
        let d = tempfile::tempdir().unwrap();
        let kb = 1024;
        write(d.path(), "engine/plan.rs", 600 * kb);
        write(d.path(), "store/db.rs", 300 * kb);
        write(d.path(), "server/api.rs", 300 * kb);
        write(d.path(), "README.md", 10 * kb);
        write(d.path(), "assets/logo.png", 900 * kb);
        write(d.path(), "Cargo.lock", 900 * kb);
        let map = |glob: &str, area: &str| ModuleRow {
            glob: glob.into(),
            area: area.into(),
        };
        let module_map = vec![
            map("engine/**", "engine"),
            map("store/**", "store"),
            map("server/**", "server"),
        ];
        let (areas, touched) = plan(
            d.path(),
            &module_map,
            &["server/api.rs".into(), "README.md".into()],
        );
        let ids: Vec<&str> = areas.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["engine", "store", "server-and-1-more"]);
        assert_eq!(
            areas[0].bytes,
            600 * kb as u64,
            "binaries and lockfiles are not counted"
        );
        assert_eq!(touched, ["server-and-1-more"]);
    }

    #[test]
    fn a_small_project_keeps_one_writer() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "src/a.rs", 1000);
        write(d.path(), "web/b.ts", 1000);
        let (areas, touched) = plan(d.path(), &[], &["src/a.rs".into()]);
        assert!(areas.is_empty() && touched.is_empty());
    }

    #[test]
    fn without_a_module_map_top_level_folders_are_the_areas() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "api/a.rs", 400 * 1024);
        write(d.path(), "web/b.ts", 400 * 1024);
        let (areas, _) = plan(d.path(), &[], &[]);
        let ids: Vec<&str> = areas.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["api", "web"]);
        assert_eq!(areas[1].globs, ["web/**"]);
    }
}
