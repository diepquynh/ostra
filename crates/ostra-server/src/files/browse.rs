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

/// Windows has no single root, so `/` there lists the drives instead of a folder.
pub const DRIVE_LIST: &str = "/";

fn is_drive_list(p: &Path) -> bool {
    cfg!(windows) && p.as_os_str() == DRIVE_LIST
}

/// The drive roots, from the bitmask of mapped letters, so no drive is touched and an empty card
/// reader cannot stall the listing.
#[cfg(windows)]
fn drives() -> Vec<String> {
    // SAFETY: a plain call with no arguments.
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:", (b'A' + i) as char))
        .collect()
}

#[cfg(not(windows))]
fn drives() -> Vec<String> {
    vec![]
}

fn tilde_rest(raw: &str) -> Option<&str> {
    raw.strip_prefix("~/")
        .or_else(|| raw.strip_prefix("~\\").filter(|_| cfg!(windows)))
}

/// Expand `~` and `~/...` against `home`; a relative path is taken from `home` too. On Windows,
/// `/` is [`DRIVE_LIST`], `C:` is the root of that drive, and a Git Bash `/c/...` path is
/// translated.
pub fn expand(raw: &str, home: &Path) -> PathBuf {
    let raw = raw.trim();
    if cfg!(windows) {
        if !raw.is_empty() && raw.chars().all(|c| c == '/' || c == '\\') {
            return PathBuf::from(DRIVE_LIST);
        }
        let b = raw.as_bytes();
        // Windows reads `C:` as that drive's current folder; the picker means its root.
        if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return PathBuf::from(format!("{raw}\\"));
        }
    }
    let p = if raw.is_empty() || raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = tilde_rest(raw) {
        home.join(rest)
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else if cfg!(windows) && raw.starts_with('/') {
        paths::from_msys(raw)
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

struct Folder {
    name: String,
    /// Marked hidden or system on Windows, such as `$Recycle.Bin` or `Documents and Settings`.
    hidden: bool,
}

/// Folders read at an instant.
type Folders = (Instant, Arc<Vec<Folder>>);

#[cfg(windows)]
fn hidden(e: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM};
    e.metadata()
        .is_ok_and(|m| m.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0)
}

#[cfg(not(windows))]
fn hidden(_: &std::fs::DirEntry) -> bool {
    false
}

#[derive(Default)]
pub struct BrowseCache {
    dirs: Mutex<HashMap<PathBuf, Folders>>,
}

impl BrowseCache {
    /// Folders directly inside `dir`, or `None` when it cannot be read.
    fn folders(&self, dir: &Path) -> Option<Arc<Vec<Folder>>> {
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
                names.push(Folder {
                    name: e.file_name().to_string_lossy().to_string(),
                    hidden: hidden(&e),
                });
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
        let absolute = Path::new(raw).is_absolute() || raw.starts_with('/');
        if !(absolute || raw == "~" || tilde_rest(raw).is_some()) {
            return Err(if cfg!(windows) {
                "Type an absolute path, such as C:\\Users\\you\\code or ~/code.".into()
            } else {
                "Type an absolute path, starting with / or ~/.".into()
            });
        }
        let path = expand(raw, home);
        if is_drive_list(&path) {
            return Err("Type a folder on a drive, such as C:\\code.".into());
        }
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
        if is_drive_list(&expanded) {
            let all = drives();
            let (names, truncated) = filter_names(all.iter().map(String::as_str), prefix, limit);
            return FsBrowse {
                path: expanded.clone(),
                parent: None,
                home: home.to_path_buf(),
                exists: true,
                readable: true,
                nearest: expanded,
                entries: names
                    .into_iter()
                    .map(|name| FsEntry {
                        name,
                        is_dir: true,
                        is_git: false,
                        is_ostra_project: false,
                    })
                    .collect(),
                truncated,
                is_git: false,
                is_ostra_project: false,
            };
        }
        let exists = expanded.exists();
        let path = if exists {
            ostra_core::paths::canonical(&expanded).unwrap_or(expanded)
        } else {
            expanded
        };
        let listed = path.is_dir().then(|| self.folders(&path)).flatten();
        let readable = listed.is_some();
        let nearest = if readable {
            path.clone()
        } else {
            let n = nearest_dir(path.parent().unwrap_or(Path::new("/")));
            ostra_core::paths::canonical(&n).unwrap_or(n)
        };
        let (entries, truncated) = match &listed {
            Some(names) => {
                // Dot folders appear only when the typed prefix asks for them, and hidden or system
                // ones only when their name starts with it.
                let show_dot = prefix.starts_with('.');
                let lower = prefix.to_lowercase();
                let visible = names
                    .iter()
                    .filter(|f| {
                        (show_dot || !f.name.starts_with('.'))
                            && (!f.hidden
                                || (!lower.is_empty() && f.name.to_lowercase().starts_with(&lower)))
                    })
                    .map(|f| f.name.as_str());
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
            // A drive root's parent is the drive list.
            parent: path
                .parent()
                .map(Path::to_path_buf)
                .or_else(|| cfg!(windows).then(|| PathBuf::from(DRIVE_LIST))),
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

    #[cfg(windows)]
    #[test]
    fn windows_lists_drives_at_the_root_and_reads_drive_paths() {
        let home = Path::new(r"C:\Users\me");
        assert_eq!(expand("/", home), PathBuf::from(DRIVE_LIST));
        assert_eq!(expand("\\", home), PathBuf::from(DRIVE_LIST));
        assert_eq!(expand("c:", home), PathBuf::from(r"c:\"));
        assert_eq!(expand(r"~\code", home), PathBuf::from(r"C:\Users\me\code"));
        assert_eq!(expand("/c/Users", home), PathBuf::from(r"C:\Users"));

        let cache = BrowseCache::default();
        let root = cache.browse(Some("/"), None, None, home);
        assert!(root.exists && root.readable && root.parent.is_none());
        assert!(
            root.entries.iter().any(|e| e.name == "C:"),
            "{:?}",
            root.entries
        );
        let c = cache.browse(Some("C:"), None, None, home);
        assert_eq!(c.parent, Some(PathBuf::from(DRIVE_LIST)));
        assert!(c.readable && !c.entries.is_empty(), "{c:?}");
        assert!(cache.mkdir("/", home).is_err());

        let names = |b: &FsBrowse| b.entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        let all = cache.browse(Some(r"C:\"), None, Some(MAX_LIMIT), home);
        assert!(
            !names(&all).iter().any(|n| n == "$Recycle.Bin"),
            "{:?}",
            names(&all)
        );
        let asked = cache.browse(Some(r"C:\"), Some("$rec"), None, home);
        assert_eq!(names(&asked), vec!["$Recycle.Bin"]);
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
        let base = ostra_core::paths::canonical(dir.path()).unwrap();
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
        let base = ostra_core::paths::canonical(dir.path()).unwrap();
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
