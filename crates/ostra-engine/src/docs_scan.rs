//! Rule B10: the project's modules and named constants, read from disk before the docs survey. The
//! runner calls this for `Step::ScanDocs` and records the result in `DocsScanned`, because the fold
//! cannot read the file system.

use ostra_core::book::{DocsModule, RefItem};
use ostra_core::config::ModuleRow;
use regex::Regex;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

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

/// Constants the scan records at most, so a large project does not flood the reference sheet.
const MAX_REFS: usize = 1500;

/// The modules of the project at `project` and its named constants, outside test files.
pub fn scan(project: &Path, module_map: &[ModuleRow]) -> (Vec<DocsModule>, Vec<RefItem>) {
    let files = tracked_files(project);
    let modules = modules(module_map, &files);
    let mut refs: Vec<RefItem> = vec![];
    for (path, _) in &files {
        if is_test(path) || !is_source(path) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(project.join(path)) else {
            continue;
        };
        for name in constants(&text) {
            if refs.len() >= MAX_REFS {
                return (modules, refs);
            }
            if !refs.iter().any(|r| r.name == name) {
                refs.push(RefItem {
                    name,
                    file: path.clone(),
                });
            }
        }
    }
    (modules, refs)
}

/// Module-map areas with their globs, else the top-level source folders, with a folder that holds
/// 3 or more source subfolders (such as `crates/`) split into one module per subfolder.
fn modules(module_map: &[ModuleRow], files: &[(String, u64)]) -> Vec<DocsModule> {
    let mut out: Vec<DocsModule> = vec![];
    if !module_map.is_empty() {
        for row in module_map {
            match out.iter_mut().find(|m| m.name == row.area) {
                Some(m) => m.globs.push(row.glob.clone()),
                None => out.push(DocsModule {
                    name: row.area.clone(),
                    globs: vec![row.glob.clone()],
                }),
            }
        }
        return out;
    }
    let mut tops: Vec<(String, Vec<String>)> = vec![];
    for (path, _) in files {
        if !is_source(path) {
            continue;
        }
        let mut parts = path.split('/');
        let (Some(top), Some(second)) = (parts.next(), parts.next()) else {
            continue;
        };
        let sub = if parts.next().is_some() {
            Some(second)
        } else {
            None
        };
        match tops.iter_mut().find(|(t, _)| t == top) {
            Some((_, subs)) => {
                if let Some(s) = sub
                    && !subs.iter().any(|x| x == s)
                {
                    subs.push(s.to_string());
                }
            }
            None => tops.push((
                top.to_string(),
                sub.map(str::to_string).into_iter().collect(),
            )),
        }
    }
    for (top, subs) in tops {
        if subs.len() >= 3 {
            for s in subs {
                out.push(DocsModule {
                    name: format!("{top}/{s}"),
                    globs: vec![format!("{top}/{s}/**")],
                });
            }
        } else {
            out.push(DocsModule {
                name: top.clone(),
                globs: vec![format!("{top}/**")],
            });
        }
    }
    out
}

const SOURCE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "py", "go", "java", "kt", "cs", "rb", "php", "swift",
    "c", "h", "cc", "cpp", "hpp", "scala", "toml", "yaml", "yml", "sql", "sh",
];

fn is_source(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| SOURCE_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

fn is_test(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.split('/')
        .any(|c| c == "tests" || c == "test" || c == "__tests__" || c == "testdata")
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// The upper-case constants a file defines: Rust `const`/`static`, JavaScript and TypeScript
/// `const`, Go `const`, Java and Kotlin `static final`, and Python module-level assignments.
fn constants(text: &str) -> Vec<String> {
    static DEF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^\s*(?:(?:pub(?:\([^)]*\))?|export|private|public|protected|internal)\s+)*(?:(?:const|static|final|let)\s+)+(?:[A-Za-z_][\w<>\[\], ]*\s+)?([A-Z][A-Z0-9_]{2,})\s*[:=]|^([A-Z][A-Z0-9_]{2,})\s*(?::[^=]+)?=",
        )
        .expect("valid regex")
    });
    DEF.captures_iter(text)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str().to_string())
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b10_constants_in_several_languages() {
        let rust = "pub const MAX_SLOTS: usize = 4;\nconst REVIEW_CAP: u32 = 3;\npub(crate) static NAMES: &[&str] = &[];\nlet x = 1;";
        assert_eq!(constants(rust), ["MAX_SLOTS", "REVIEW_CAP", "NAMES"]);
        let ts = "export const MAX_HITS = 30;\nconst lower = 1;";
        assert_eq!(constants(ts), ["MAX_HITS"]);
        let py = "TIMEOUT_SECONDS = 600\n    INNER = 1\nRETRY: int = 3";
        assert_eq!(constants(py), ["TIMEOUT_SECONDS", "RETRY"]);
        let java = "    private static final int MAX_SIZE = 10;";
        assert_eq!(constants(java), ["MAX_SIZE"]);
    }

    #[test]
    fn b10_modules_split_a_folder_of_packages() {
        let f = |p: &str| (p.to_string(), 1u64);
        let files = [
            f("crates/a/src/lib.rs"),
            f("crates/b/src/lib.rs"),
            f("crates/c/src/lib.rs"),
            f("web/src/main.ts"),
            f("README.md"),
        ];
        let names: Vec<String> = modules(&[], &files).into_iter().map(|m| m.name).collect();
        assert_eq!(names, ["crates/a", "crates/b", "crates/c", "web"]);
    }
}
