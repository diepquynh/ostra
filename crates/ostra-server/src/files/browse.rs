//! `GET /api/fs`: folder names for the type-to-browse picker. One directory read per request (or
//! none, from a two-second cache), no recursion, no git process, never file contents.
//! `POST /api/fs/mkdir`: create a folder with its missing parents.

use ostra_core::api::{FsBrowse, FsEntry};
use ostra_core::paths;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(2);
pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 2000;
/// Stop reading a folder after this many entries, so `/usr/lib`-sized folders stay fast.
const MAX_SCAN: usize = 50_000;

/// Expand `~` and `~/...` against `home`; a relative path is taken from `home` too.
pub fn expand(raw: &str, home: &Path) -> PathBuf {
    let raw = raw.trim();
    let p = if raw.is_empty() || raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        home.join(raw)
    };
    paths::normalize(&p)
}

/// The deepest ancestor of `path` (itself included) that is a folder.
pub fn nearest_dir(path: &Path) -> PathBuf {
    let mut cur = path.to_path_buf();
    loop {
        if cur.is_dir() {
            return cur;
        }
        if !cur.pop() {
            return PathBuf::from("/");
        }
    }
}

/// Filter names case-insensitively: names starting with `prefix` first, then names containing it,
/// each group sorted. Returns at most `limit` names and whether more matched.
pub fn filter_names<'a>(
    names: impl Iterator<Item = &'a str>,
    prefix: &str,
    limit: usize,
) -> (Vec<String>, bool) {
    let needle = prefix.to_lowercase();
    let mut starts = vec![];
    let mut contains = vec![];
    for n in names {
        let lower = n.to_lowercase();
        if lower.starts_with(&needle) {
            starts.push((lower, n));
        } else if lower.contains(&needle) {
            contains.push((lower, n));
        }
    }
    starts.sort();
    contains.sort();
    let total = starts.len() + contains.len();
    let out: Vec<String> = starts
        .into_iter()
        .chain(contains)
        .take(limit)
        .map(|(_, n)| n.to_string())
        .collect();
    (out, total > limit)
}

/// Folder names read at an instant.
type Folders = (Instant, Arc<Vec<String>>);

#[derive(Default)]
pub struct BrowseCache {
    dirs: Mutex<HashMap<PathBuf, Folders>>,
}

impl BrowseCache {
    /// Folder names directly inside `dir`, or `None` when it cannot be read.
    fn folders(&self, dir: &Path) -> Option<Arc<Vec<String>>> {
        let now = Instant::now();
        {
            let mut cache = self.dirs.lock();
            cache.retain(|_, (at, _)| now.duration_since(*at) < CACHE_TTL);
            if let Some((_, names)) = cache.get(dir) {
                return Some(names.clone());
            }
        }
        let read = std::fs::read_dir(dir).ok()?;
        let mut names = vec![];
        for e in read.flatten().take(MAX_SCAN) {
            let is_dir = match e.file_type() {
                Ok(t) if t.is_symlink() => e.path().is_dir(),
                Ok(t) => t.is_dir(),
                Err(_) => false,
            };
            if is_dir {
                names.push(e.file_name().to_string_lossy().to_string());
            }
        }
        let names = Arc::new(names);
        self.dirs
            .lock()
            .insert(dir.to_path_buf(), (now, names.clone()));
        Some(names)
    }

    /// Create `raw` and its missing parents, like `mkdir -p`, then describe it.
    pub fn mkdir(&self, raw: &str, home: &Path) -> Result<FsBrowse, String> {
        let raw = raw.trim();
        if !(raw.starts_with('/') || raw == "~" || raw.starts_with("~/")) {
            return Err("Type an absolute path, starting with / or ~/.".into());
        }
        let path = expand(raw, home);
        if path.exists() && !path.is_dir() {
            return Err(format!(
                "Choose another name, because {} is a file.",
                path.display()
            ));
        }
        std::fs::create_dir_all(&path)
            .map_err(|e| format!("Could not create {}: {e}", path.display()))?;
        // Every cached ancestor listing may now miss a new folder.
        self.dirs.lock().clear();
        Ok(self.browse(Some(raw), None, None, home))
    }

    pub fn browse(
        &self,
        raw: Option<&str>,
        prefix: Option<&str>,
        limit: Option<usize>,
        home: &Path,
    ) -> FsBrowse {
        let expanded = expand(raw.unwrap_or(""), home);
        let prefix = prefix.unwrap_or("").trim();
        let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let exists = expanded.exists();
        let path = if exists {
            std::fs::canonicalize(&expanded).unwrap_or(expanded)
        } else {
            expanded
        };
        let listed = path.is_dir().then(|| self.folders(&path)).flatten();
        let readable = listed.is_some();
        let nearest = if readable {
            path.clone()
        } else {
            let n = nearest_dir(path.parent().unwrap_or(Path::new("/")));
            std::fs::canonicalize(&n).unwrap_or(n)
        };
        let (entries, truncated) = match &listed {
            Some(names) => {
                // Dot folders appear only when the typed prefix asks for them.
                let show_hidden = prefix.starts_with('.');
                let visible = names
                    .iter()
                    .map(String::as_str)
                    .filter(|n| show_hidden || !n.starts_with('.'));
                let (names, truncated) = filter_names(visible, prefix, limit);
                let entries = names
                    .into_iter()
                    .map(|name| {
                        let p = path.join(&name);
                        FsEntry {
                            is_git: p.join(".git").exists(),
                            is_ostra_project: paths::project_inventory(&p).exists(),
                            name,
                            is_dir: true,
                        }
                    })
                    .collect();
                (entries, truncated)
            }
            None => (vec![], false),
        };
        let is_dir = path.is_dir();
        FsBrowse {
            parent: path.parent().map(Path::to_path_buf),
            home: home.to_path_buf(),
            exists,
            readable,
            nearest,
            entries,
            truncated,
            is_git: is_dir && path.join(".git").exists(),
            is_ostra_project: is_dir && paths::project_inventory(&path).exists(),
            path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expansion() {
        let home = Path::new("/home/me");
        assert_eq!(expand("~", home), PathBuf::from("/home/me"));
        assert_eq!(expand("", home), PathBuf::from("/home/me"));
        assert_eq!(expand("~/code/../src", home), PathBuf::from("/home/me/src"));
        assert_eq!(expand("/srv/repos", home), PathBuf::from("/srv/repos"));
        assert_eq!(expand("code", home), PathBuf::from("/home/me/code"));
        assert_eq!(expand("~other", home), PathBuf::from("/home/me/~other"));
    }

    #[test]
    fn prefix_ordering_starts_with_then_contains() {
        let names = [
            "Shop-admin",
            "billing-service",
            "shop",
            "workshop",
            "dotfiles",
            "shop-web",
            "eshop",
        ];
        let (out, truncated) = filter_names(names.iter().copied(), "SHOP", 10);
        assert_eq!(
            out,
            vec!["shop", "Shop-admin", "shop-web", "eshop", "workshop"]
        );
        assert!(!truncated);
        let (out, truncated) = filter_names(names.iter().copied(), "shop", 2);
        assert_eq!(out, vec!["shop", "Shop-admin"]);
        assert!(truncated);
        let (out, _) = filter_names(names.iter().copied(), "", 3);
        assert_eq!(out, vec!["billing-service", "dotfiles", "eshop"]);
    }

    #[test]
    fn mkdir_creates_parents_and_refreshes_listings() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(base.join("file.txt"), "x").unwrap();
        let cache = BrowseCache::default();
        assert!(
            cache
                .browse(Some("~"), None, None, &base)
                .entries
                .is_empty()
        );

        let b = cache.mkdir("~/code/shop/web", &base).unwrap();
        assert!(b.exists && b.readable);
        assert_eq!(b.path, base.join("code/shop/web"));
        let listed = cache.browse(Some("~"), None, None, &base);
        assert_eq!(
            listed.entries.len(),
            1,
            "the cached home listing was dropped"
        );
        assert!(
            cache.mkdir("~/code/shop/web", &base).is_ok(),
            "an existing folder is fine"
        );

        assert!(
            cache
                .mkdir("~/file.txt", &base)
                .unwrap_err()
                .contains("is a file")
        );
        assert!(cache.mkdir("relative/path", &base).is_err());
    }

    #[test]
    fn missing_path_reports_nearest_existing_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(base.join("code/shop/.git")).unwrap();
        std::fs::create_dir_all(base.join("code/.hidden")).unwrap();
        std::fs::create_dir_all(base.join("code/web/.ostra")).unwrap();
        std::fs::write(base.join("code/web/.ostra/INVENTORY.md"), "#").unwrap();
        std::fs::write(base.join("code/file.txt"), "x").unwrap();
        let cache = BrowseCache::default();

        let b = cache.browse(Some("~/code/nope/deeper"), None, None, &base);
        assert!(!b.exists);
        assert!(!b.readable);
        assert_eq!(b.nearest, base.join("code"));
        assert!(b.entries.is_empty());
        assert_eq!(b.home, base);

        let b = cache.browse(Some("~/code"), None, None, &base);
        assert!(b.exists && b.readable);
        assert_eq!(b.nearest, base.join("code"));
        let names: Vec<(&str, bool, bool)> = b
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.is_git, e.is_ostra_project))
            .collect();
        assert_eq!(
            names,
            vec![("shop", true, false), ("web", false, true)],
            "folders only, no dot folders"
        );

        assert!(!b.is_git && !b.is_ostra_project);
        let shop = cache.browse(Some("~/code/shop"), None, None, &base);
        assert!(
            shop.is_git && !shop.is_ostra_project,
            "the browsed folder itself is described"
        );
        let web = cache.browse(Some("~/code/web"), None, None, &base);
        assert!(!web.is_git && web.is_ostra_project);

        let b = cache.browse(Some("~/code"), Some(".h"), None, &base);
        assert_eq!(
            b.entries
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec![".hidden"]
        );

        let b = cache.browse(Some("~/code/file.txt"), None, None, &base);
        assert!(b.exists && !b.readable);
        assert_eq!(b.nearest, base.join("code"));
    }
}
