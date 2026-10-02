//! Every `.*ignore` file (`.gitignore`, `.ignore`, `.rgignore`, `.dockerignore`, `.npmignore`, ...)
//! read with gitignore syntax. Sandboxed agents never see a path one of them hides in a search.
//!
//! Each file name is its own family, evaluated the way git evaluates `.gitignore`: the deepest
//! folder's file decides first and a `!` line re-includes only within its own family. A path is
//! hidden when any family hides it or one of its folders. The files apply from the nearest
//! enclosing git repository root downward, or from the filesystem root outside a repository.
//! `.git/info/exclude` and the global excludes file join the `.gitignore` family below its files.

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const GIT_FAMILY: &str = ".gitignore";

/// True for a file name that counts as an ignore file: a dot, any text, then `ignore`.
pub fn is_ignore_file_name(name: &str) -> bool {
    name.starts_with('.') && name.ends_with("ignore")
}

struct Rules {
    family: String,
    file: PathBuf,
    matcher: Gitignore,
}

/// What a bounded walk found under a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Under {
    Clean,
    Hidden(PathBuf),
    /// The walk stopped at its budget before finding a hidden path.
    TooLarge,
}

/// Ignore files read once per instance. Make a new one per search so an edit to an ignore file
/// applies to the next call.
pub struct IgnoreFiles {
    dirs: Mutex<HashMap<PathBuf, Arc<Vec<Rules>>>>,
    git_roots: Mutex<HashMap<PathBuf, Option<PathBuf>>>,
    excludes: Mutex<HashMap<PathBuf, Arc<Option<Rules>>>>,
    global: Option<Rules>,
    exempt: Vec<PathBuf>,
}

impl Default for IgnoreFiles {
    fn default() -> Self {
        Self::new()
    }
}

impl IgnoreFiles {
    pub fn new() -> Self {
        let (global, _) = Gitignore::global();
        IgnoreFiles {
            dirs: Mutex::default(),
            git_roots: Mutex::default(),
            excludes: Mutex::default(),
            global: (!global.is_empty()).then(|| Rules {
                family: GIT_FAMILY.into(),
                file: global.path().to_path_buf(),
                matcher: global,
            }),
            exempt: vec![],
        }
    }

    /// Folders whose contents are never hidden: Ostra's own state and the temp dir, whose
    /// `.gitignore` files keep them out of git, not out of an agent's view.
    pub fn exempt(mut self, dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        self.exempt
            .extend(dirs.into_iter().filter(|d| !d.as_os_str().is_empty()));
        self
    }

    fn is_exempt(&self, p: &Path) -> bool {
        self.exempt.iter().any(|d| p.starts_with(d))
    }

    /// The ignore file that hides `path` or one of its folders, or `None` when it is visible.
    pub fn hidden_by(&self, path: &Path) -> Option<PathBuf> {
        if in_git_dir(path) || self.is_exempt(path) || path.join(".git").exists() {
            return None;
        }
        let boundary = self.boundary(path.parent().unwrap_or(path));
        let mut prefixes: Vec<&Path> = path
            .ancestors()
            .take_while(|a| *a != boundary && a.starts_with(&boundary))
            .collect();
        prefixes.reverse();
        let last = prefixes.len().saturating_sub(1);
        prefixes.iter().enumerate().find_map(|(i, p)| {
            let is_dir = i < last || p.is_dir();
            self.decide(p, is_dir, &boundary)
        })
    }

    /// Whether this entry itself is hidden, its folders already known to be visible. For a walk
    /// that prunes hidden folders.
    pub fn entry_hidden(&self, path: &Path, is_dir: bool) -> bool {
        if in_git_dir(path) || self.is_exempt(path) {
            return false;
        }
        let boundary = self.boundary(path.parent().unwrap_or(path));
        self.decide(path, is_dir, &boundary).is_some()
    }

    /// The first hidden path under `dir`, breadth first, looking at no more than `budget` entries.
    pub fn first_hidden_under(&self, dir: &Path, budget: usize) -> Under {
        let mut queue = VecDeque::from([dir.to_path_buf()]);
        let mut seen = 0usize;
        while let Some(d) = queue.pop_front() {
            let Ok(read) = std::fs::read_dir(&d) else {
                continue;
            };
            for entry in read.flatten() {
                if entry.file_name() == ".git" {
                    continue;
                }
                seen += 1;
                if seen > budget {
                    return Under::TooLarge;
                }
                let path = entry.path();
                let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
                if self.entry_hidden(&path, is_dir) {
                    return Under::Hidden(path);
                }
                if is_dir {
                    queue.push_back(path);
                }
            }
        }
        Under::Clean
    }

    fn decide(&self, path: &Path, is_dir: bool, boundary: &Path) -> Option<PathBuf> {
        let mut decided: HashSet<String> = HashSet::new();
        let dirs = path
            .ancestors()
            .skip(1)
            .take_while(|a| a.starts_with(boundary));
        for dir in dirs {
            for r in self.rules(dir).iter() {
                if decided.contains(&r.family) {
                    continue;
                }
                match r.matcher.matched(path, is_dir) {
                    Match::Ignore(_) => return Some(r.file.clone()),
                    Match::Whitelist(_) => {
                        decided.insert(r.family.clone());
                    }
                    Match::None => {}
                }
            }
        }
        if decided.contains(GIT_FAMILY) || !boundary.join(".git").exists() {
            return None;
        }
        let rel = path.strip_prefix(boundary).unwrap_or(path);
        let exclude = self.exclude(boundary);
        [exclude.as_ref().as_ref(), self.global.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|r| match r.matcher.matched(rel, is_dir) {
                Match::Ignore(_) => Some(Some(r.file.clone())),
                Match::Whitelist(_) => Some(None),
                Match::None => None,
            })
            .flatten()
    }

    fn rules(&self, dir: &Path) -> Arc<Vec<Rules>> {
        if let Some(r) = lock(&self.dirs).get(dir) {
            return r.clone();
        }
        let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (is_ignore_file_name(&name) && e.path().is_file()).then(|| (name, e.path()))
            })
            .collect();
        found.sort();
        let rules: Vec<Rules> = found
            .into_iter()
            .filter_map(|(family, file)| {
                let matcher = build(dir, &file)?;
                Some(Rules {
                    family,
                    file,
                    matcher,
                })
            })
            .collect();
        let rules = Arc::new(rules);
        lock(&self.dirs).insert(dir.to_path_buf(), rules.clone());
        rules
    }

    fn exclude(&self, root: &Path) -> Arc<Option<Rules>> {
        if let Some(r) = lock(&self.excludes).get(root) {
            return r.clone();
        }
        let file = root.join(".git").join("info").join("exclude");
        let rules = Arc::new(build(root, &file).map(|matcher| Rules {
            family: GIT_FAMILY.into(),
            file,
            matcher,
        }));
        lock(&self.excludes).insert(root.to_path_buf(), rules.clone());
        rules
    }

    /// The nearest folder at or above `dir` that holds `.git`, or the filesystem root.
    fn boundary(&self, dir: &Path) -> PathBuf {
        if let Some(b) = lock(&self.git_roots).get(dir) {
            return b.clone().unwrap_or_else(|| fs_root(dir));
        }
        let found = dir
            .ancestors()
            .find(|a| a.join(".git").exists())
            .map(Path::to_path_buf);
        lock(&self.git_roots).insert(dir.to_path_buf(), found.clone());
        found.unwrap_or_else(|| fs_root(dir))
    }
}

fn build(root: &Path, file: &Path) -> Option<Gitignore> {
    if !file.is_file() {
        return None;
    }
    let mut b = GitignoreBuilder::new(root);
    b.add(file);
    b.build().ok().filter(|g| !g.is_empty())
}

fn fs_root(p: &Path) -> PathBuf {
    p.ancestors().last().unwrap_or(p).to_path_buf()
}

fn in_git_dir(p: &Path) -> bool {
    p.components().any(|c| c.as_os_str() == ".git")
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git/info")).unwrap();
        dir
    }

    #[test]
    fn names() {
        for n in [".gitignore", ".ignore", ".dockerignore", ".prettierignore"] {
            assert!(is_ignore_file_name(n), "{n}");
        }
        for n in ["gitignore", ".gitignored", "ignore", ".git"] {
            assert!(!is_ignore_file_name(n), "{n}");
        }
    }

    #[test]
    fn every_dot_ignore_file_hides() {
        let r = repo();
        let root = r.path();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join(".dockerignore"), "secrets.txt\n").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        for f in ["target/debug/out", "secrets.txt", "src/main.rs"] {
            fs::write(root.join(f), "x").unwrap();
        }
        let i = IgnoreFiles::new();
        assert_eq!(
            i.hidden_by(&root.join("target/debug/out")),
            Some(root.join(".gitignore"))
        );
        assert_eq!(
            i.hidden_by(&root.join("secrets.txt")),
            Some(root.join(".dockerignore"))
        );
        assert_eq!(i.hidden_by(&root.join("src/main.rs")), None);
        assert_eq!(i.hidden_by(root), None);
    }

    #[test]
    fn negation_stays_in_its_family_and_deeper_files_win() {
        let r = repo();
        let root = r.path();
        fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        fs::write(root.join(".npmignore"), "!keep.log\n").unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("a/.gitignore"), "!keep.log\n").unwrap();
        fs::write(root.join("keep.log"), "x").unwrap();
        fs::write(root.join("a/keep.log"), "x").unwrap();
        let i = IgnoreFiles::new();
        assert!(i.hidden_by(&root.join("keep.log")).is_some());
        assert_eq!(i.hidden_by(&root.join("a/keep.log")), None);
    }

    #[test]
    fn git_exclude_and_nested_repo_boundary() {
        let r = repo();
        let root = r.path();
        fs::write(root.join(".git/info/exclude"), "local/\n").unwrap();
        fs::write(root.join(".ignore"), "*.bin\n").unwrap();
        fs::create_dir_all(root.join("local")).unwrap();
        fs::create_dir_all(root.join("sub/.git")).unwrap();
        fs::write(root.join("sub/a.bin"), "x").unwrap();
        let i = IgnoreFiles::new();
        assert!(i.hidden_by(&root.join("local/x")).is_some());
        assert_eq!(i.hidden_by(&root.join("sub/a.bin")), None);
    }

    #[test]
    fn bounded_walk_finds_the_first_hidden_entry() {
        let r = repo();
        let root = r.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "x").unwrap();
        let i = IgnoreFiles::new();
        assert_eq!(i.first_hidden_under(root, 100), Under::Clean);
        fs::write(root.join(".prettierignore"), "src/a.rs\n").unwrap();
        let i = IgnoreFiles::new();
        assert_eq!(
            i.first_hidden_under(root, 100),
            Under::Hidden(root.join("src/a.rs"))
        );
        assert_eq!(i.first_hidden_under(root, 1), Under::TooLarge);
        let i = IgnoreFiles::new().exempt([root.join("src")]);
        assert_eq!(i.first_hidden_under(root, 100), Under::Clean);
        assert_eq!(i.hidden_by(&root.join("src/a.rs")), None);
    }
}
