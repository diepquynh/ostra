//! Project paths on disk: containment, folder listings with ignore flags, the file name walk, and
//! capped file reads. Blocking; callers run these on the blocking pool.

use ostra_core::paths;
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// A project-relative path that passed containment: `rel` is the normalized request (`/`-joined,
/// empty for the root), `lexical` is `root/rel`, and `real` is it with symlinks resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Contained {
    pub rel: String,
    pub lexical: PathBuf,
    pub real: PathBuf,
}

/// Resolve a project-relative path under the canonical `root`, refusing anything that leaves it.
pub fn contain(root: &Path, raw: &str) -> Result<Contained, String> {
    let raw = raw.trim();
    let p = Path::new(raw);
    if p.is_absolute() {
        return Err("Use a path relative to the project root.".into());
    }
    let mut parts: Vec<String> = vec![];
    for c in p.components() {
        match c {
            Component::Normal(n) => parts.push(n.to_string_lossy().to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(outside());
                }
            }
            _ => return Err(outside()),
        }
    }
    let rel = parts.join("/");
    let lexical = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(&rel)
    };
    // Same rule as the write-scope guard: follow symlinks of the longest existing prefix, then
    // require the result inside the root, so a link cannot lead out of the project.
    let real = paths::resolve(root, Path::new(&rel));
    if !paths::is_inside(root, &real) {
        return Err(outside());
    }
    Ok(Contained { rel, lexical, real })
}

fn outside() -> String {
    "That path is outside the project. Use a path inside the project root.".into()
}

fn walker(dir: &Path) -> ignore::WalkBuilder {
    let mut b = ignore::WalkBuilder::new(dir);
    // The same ignore sources as the Grep and Glob tools, so the dock and the agents agree.
    b.hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .parents(true)
        .require_git(false);
    b
}

/// Names directly inside `dir` that no ignore rule excludes.
fn kept_children(dir: &Path) -> HashSet<OsString> {
    let mut b = walker(dir);
    b.max_depth(Some(1));
    b.build()
        .flatten()
        .filter(|e| e.depth() == 1)
        .map(|e| e.file_name().to_os_string())
        .collect()
}

/// Whether an ignore rule excludes `rel` itself or one of its parent folders.
pub fn is_ignored(root: &Path, rel: &str) -> bool {
    if rel.is_empty() {
        return false;
    }
    let target = root.join(rel);
    let depth = rel.split('/').count();
    let chain = target.clone();
    let mut b = walker(root);
    b.max_depth(Some(depth))
        .follow_links(true)
        .filter_entry(move |e| chain.starts_with(e.path()));
    !b.build().flatten().any(|e| e.path() == target)
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawEntry {
    pub name: String,
    pub rel: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<chrono::DateTime<chrono::Utc>>,
    pub ignored: bool,
}

pub struct Listing {
    pub entries: Vec<RawEntry>,
    pub truncated: bool,
}

/// List `dir` (already contained) depth-first down to `depth` levels. Folders sort before files;
/// `.git` is never listed, dotfiles only with `hidden`. Symlinked and ignored folders are listed
/// but not entered.
pub fn list(
    root: &Path,
    dir: &Contained,
    depth: u32,
    hidden: bool,
    cap: usize,
) -> Result<Listing, String> {
    let meta = std::fs::metadata(&dir.real)
        .map_err(|_| format!("{} does not exist.", display(&dir.rel)))?;
    if !meta.is_dir() {
        return Err(format!(
            "{} is a file. Open it with the file endpoint.",
            display(&dir.rel)
        ));
    }
    let mut out = Listing {
        entries: vec![],
        truncated: false,
    };
    walk_dir(
        &dir.lexical,
        &dir.rel,
        depth.max(1),
        hidden,
        is_ignored(root, &dir.rel),
        cap,
        &mut out,
    );
    Ok(out)
}

fn display(rel: &str) -> String {
    if rel.is_empty() {
        "The project root".into()
    } else {
        rel.to_string()
    }
}

fn walk_dir(
    dir: &Path,
    rel: &str,
    depth: u32,
    hidden: bool,
    parent_ignored: bool,
    cap: usize,
    out: &mut Listing,
) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let kept = if parent_ignored {
        HashSet::new()
    } else {
        kept_children(dir)
    };
    let mut rows: Vec<(RawEntry, PathBuf)> = vec![];
    for e in read.flatten() {
        let name = e.file_name();
        let name_s = name.to_string_lossy().to_string();
        if name_s == ".git" || (!hidden && name_s.starts_with('.')) {
            continue;
        }
        let is_symlink = e.file_type().is_ok_and(|t| t.is_symlink());
        let meta = e
            .metadata()
            .ok()
            .filter(|_| !is_symlink)
            .or_else(|| std::fs::metadata(e.path()).ok());
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
        let entry = RawEntry {
            rel: if rel.is_empty() {
                name_s.clone()
            } else {
                format!("{rel}/{name_s}")
            },
            name: name_s,
            is_dir,
            is_symlink,
            size: meta
                .as_ref()
                .filter(|m| !m.is_dir())
                .map(|m| m.len())
                .unwrap_or(0),
            modified: meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .map(chrono::DateTime::<chrono::Utc>::from),
            ignored: parent_ignored || !kept.contains(&name),
        };
        rows.push((entry, e.path()));
    }
    rows.sort_by(|(a, _), (b, _)| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    for (entry, path) in rows {
        if out.entries.len() >= cap {
            out.truncated = true;
            return;
        }
        let descend = depth > 1 && entry.is_dir && !entry.is_symlink && !entry.ignored;
        let (child_rel, ignored) = (entry.rel.clone(), entry.ignored);
        out.entries.push(entry);
        if descend {
            walk_dir(&path, &child_rel, depth - 1, hidden, ignored, cap, out);
        }
    }
}

/// Every non-ignored file under `root` (dotfiles included, `.git` excluded), sorted, at most `cap`.
pub fn index(root: &Path, cap: usize) -> (Vec<String>, bool) {
    let mut b = walker(root);
    b.filter_entry(|e| e.file_name() != ".git");
    let mut out = vec![];
    let mut truncated = false;
    for e in b.build().flatten() {
        let is_file = e
            .file_type()
            .is_some_and(|t| t.is_file() || (t.is_symlink() && e.path().is_file()));
        if !is_file {
            continue;
        }
        let Ok(rel) = e.path().strip_prefix(root) else {
            continue;
        };
        if out.len() >= cap {
            truncated = true;
            break;
        }
        out.push(
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        );
    }
    out.sort();
    (out, truncated)
}

pub struct FileRead {
    pub content: Option<String>,
    pub binary: bool,
    pub size: u64,
    pub truncated: bool,
    pub modified: Option<chrono::DateTime<chrono::Utc>>,
    /// SHA-256 of the whole file, set only when the text is the complete, valid UTF-8 file.
    pub hash: Option<String>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// NUL in the first 8 KiB means binary, the same test git uses.
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

/// Read a contained file as text up to `cap` bytes, or report it binary.
pub fn read(file: &Contained, cap: usize) -> Result<FileRead, String> {
    let meta = std::fs::metadata(&file.real)
        .map_err(|_| format!("{} does not exist.", display(&file.rel)))?;
    if meta.is_dir() {
        return Err(format!(
            "{} is a folder. List it with the tree endpoint.",
            display(&file.rel)
        ));
    }
    let mut f =
        std::fs::File::open(&file.real).map_err(|e| format!("Cannot read {}: {e}", file.rel))?;
    let mut buf = Vec::with_capacity((meta.len() as usize).min(cap + 1));
    (&mut f)
        .take(cap as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("Cannot read {}: {e}", file.rel))?;
    let modified = meta
        .modified()
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from);
    if looks_binary(&buf) {
        return Ok(FileRead {
            content: None,
            binary: true,
            size: meta.len(),
            truncated: false,
            modified,
            hash: None,
        });
    }
    let truncated = buf.len() > cap;
    let hash = (!truncated && std::str::from_utf8(&buf).is_ok()).then(|| sha256_hex(&buf));
    buf.truncate(cap);
    if truncated {
        // Cut at the last complete UTF-8 sequence so the cap does not leave a broken character.
        if let Err(e) = std::str::from_utf8(&buf)
            && e.error_len().is_none()
        {
            buf.truncate(e.valid_up_to());
        }
    }
    Ok(FileRead {
        content: Some(String::from_utf8_lossy(&buf).to_string()),
        binary: false,
        size: meta.len(),
        truncated,
        modified,
        hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = ostra_core::paths::canonical(dir.path()).unwrap().join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").unwrap();
        (dir, root)
    }

    #[test]
    fn containment_refuses_parent_escapes() {
        let (_d, root) = root();
        assert_eq!(contain(&root, "src/lib.rs").unwrap().rel, "src/lib.rs");
        assert_eq!(
            contain(&root, "./src/../src/lib.rs").unwrap().rel,
            "src/lib.rs"
        );
        assert_eq!(contain(&root, "").unwrap().rel, "");
        assert_eq!(
            paths::fold(&contain(&root, "src/new/not-yet.rs").unwrap().real),
            paths::fold(&root.join("src/new/not-yet.rs"))
        );
        assert!(contain(&root, "..").is_err());
        assert!(contain(&root, "src/../../proj2/x").is_err());
        assert!(contain(&root, "/etc/passwd").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn containment_refuses_symlink_escapes() {
        let (d, root) = root();
        let outside = ostra_core::paths::canonical(d.path()).unwrap().join("secret");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("key"), "k").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        std::os::unix::fs::symlink(outside.join("key"), root.join("key-link")).unwrap();
        std::os::unix::fs::symlink(root.join("src"), root.join("inner")).unwrap();
        assert!(contain(&root, "link").is_err());
        assert!(contain(&root, "link/key").is_err());
        assert!(contain(&root, "key-link").is_err());
        assert!(contain(&root, "link/missing/deeper").is_err());
        assert_eq!(
            contain(&root, "inner/lib.rs").unwrap().real,
            root.join("src/lib.rs"),
            "a link that stays inside is allowed"
        );
    }

    #[test]
    fn listing_flags_ignored_entries_and_hides_dotfiles() {
        let (_d, root) = root();
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/app"), "bin").unwrap();
        std::fs::write(root.join("run.log"), "x").unwrap();
        std::fs::write(root.join("README.md"), "# r").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".ostra")).unwrap();

        let top = contain(&root, "").unwrap();
        let l = list(&root, &top, 1, false, 100).unwrap();
        let names: Vec<(&str, bool, bool)> = l
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.is_dir, e.ignored))
            .collect();
        assert_eq!(
            names,
            vec![
                ("src", true, false),
                ("target", true, true),
                ("README.md", false, false),
                ("run.log", false, true)
            ]
        );

        let l = list(&root, &top, 1, true, 100).unwrap();
        let names: Vec<&str> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                ".ostra",
                "src",
                "target",
                ".gitignore",
                "README.md",
                "run.log"
            ],
            ".git is never listed"
        );

        let l = list(&root, &top, 2, false, 100).unwrap();
        let rels: Vec<&str> = l.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(
            rels,
            vec!["src", "src/lib.rs", "target", "README.md", "run.log"],
            "ignored folders are not entered"
        );

        let inside = list(&root, &contain(&root, "target").unwrap(), 1, false, 100).unwrap();
        assert!(
            inside.entries.iter().all(|e| e.ignored),
            "entries of an ignored folder are ignored"
        );
        assert!(is_ignored(&root, "target/debug/app"));
        assert!(!is_ignored(&root, "src/lib.rs"));

        let capped = list(&root, &top, 2, false, 2).unwrap();
        assert!(capped.truncated);
        assert_eq!(capped.entries.len(), 2);
        assert!(list(&root, &contain(&root, "src/lib.rs").unwrap(), 1, false, 100).is_err());
    }

    #[test]
    fn index_skips_ignored_files_and_git() {
        let (_d, root) = root();
        std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
        std::fs::write(root.join("node_modules/x/i.js"), "").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref").unwrap();
        std::fs::write(root.join("b.txt"), "").unwrap();
        let (paths, truncated) = index(&root, 100);
        assert_eq!(paths, vec![".gitignore", "b.txt", "src/lib.rs"]);
        assert!(!truncated);
        let (paths, truncated) = index(&root, 2);
        assert_eq!(paths.len(), 2);
        assert!(truncated);
    }

    #[test]
    fn reads_text_with_a_cap_and_detects_binary() {
        let (_d, root) = root();
        std::fs::write(root.join("big.txt"), "é".repeat(10)).unwrap();
        std::fs::write(root.join("img.bin"), [0x89, 0x50, 0x00, 0x01]).unwrap();
        let r = read(&contain(&root, "big.txt").unwrap(), 5).unwrap();
        assert_eq!(
            r.content.as_deref(),
            Some("éé"),
            "cut on a character boundary"
        );
        assert!(r.truncated);
        assert_eq!(r.size, 20);
        let r = read(&contain(&root, "img.bin").unwrap(), 1024).unwrap();
        assert!(r.binary);
        assert_eq!(r.content, None);
        assert!(read(&contain(&root, "src").unwrap(), 1024).is_err());
        assert!(read(&contain(&root, "missing").unwrap(), 1024).is_err());
    }
}
