//! Watches a project's folders for edits made outside Ostra's write tools (a shell command, the
//! user's editor, a checkout) so the code index and file list catch up without waiting for the
//! periodic disk check. Each folder that holds a listed file is watched on its own, not the tree,
//! so ignored build output adds no watches. Changes arrive in batches after a short quiet spell.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::{EventKind, ModifyKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak, mpsc};
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(250);
const MAX_WAIT: Duration = Duration::from_secs(2);

/// Project-relative paths that changed together.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub changed: BTreeSet<String>,
    /// A file or folder was added, removed, or renamed, so the file list is stale.
    pub structural: bool,
}

pub type OnBatch = Arc<dyn Fn(Batch) + Send + Sync>;

/// Stops watching when dropped.
pub struct ProjectWatch {
    pub root: PathBuf,
    _watcher: Arc<Mutex<RecommendedWatcher>>,
}

fn ignores(root: &Path) -> Gitignore {
    let mut b = GitignoreBuilder::new(root);
    b.add(root.join(".gitignore"));
    b.add(root.join(".git/info/exclude"));
    b.build().unwrap_or_else(|_| Gitignore::empty())
}

fn relative(root: &Path, p: &Path) -> Option<String> {
    let r = p.strip_prefix(root).ok()?;
    let s = r.to_string_lossy().replace('\\', "/");
    (!s.is_empty() && !s.split('/').any(|c| c == ".git")).then_some(s)
}

/// Start watching `root`: each folder of `files` (project-relative) plus the root itself.
/// `None` when the platform refuses even the root.
pub fn start(root: PathBuf, files: &[String], on_batch: OnBatch) -> Option<ProjectWatch> {
    let (tx, rx) = mpsc::channel::<notify::Event>();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(e) = res {
            let _ = tx.send(e);
        }
    })
    .ok()?;
    let watcher = Arc::new(Mutex::new(watcher));
    let mut dirs: HashSet<PathBuf> = HashSet::from([root.clone()]);
    for f in files {
        let mut d = Path::new(f).parent();
        while let Some(dir) = d.filter(|d| !d.as_os_str().is_empty()) {
            if !dirs.insert(root.join(dir)) {
                break;
            }
            d = dir.parent();
        }
    }
    {
        let mut w = watcher.lock();
        w.watch(&root, RecursiveMode::NonRecursive).ok()?;
        let mut refused = 0usize;
        for d in &dirs {
            if d != &root && w.watch(d, RecursiveMode::NonRecursive).is_err() {
                refused += 1;
            }
        }
        if refused > 0 {
            tracing::warn!(
                "watching {}: {refused} folders refused (the OS watch limit?); the periodic check covers them",
                root.display()
            );
        }
    }
    let weak = Arc::downgrade(&watcher);
    let thread_root = root.clone();
    std::thread::Builder::new()
        .name("ostra-code-watch".into())
        .spawn(move || run(thread_root, rx, weak, on_batch))
        .ok()?;
    Some(ProjectWatch {
        root,
        _watcher: watcher,
    })
}

fn run(
    root: PathBuf,
    rx: mpsc::Receiver<notify::Event>,
    watcher: Weak<Mutex<RecommendedWatcher>>,
    on_batch: OnBatch,
) {
    let ignore = ignores(&root);
    // The channel closes when the watcher, which owns the sender, is dropped.
    while let Ok(first) = rx.recv() {
        let mut batch = Batch::default();
        let started = Instant::now();
        let mut next = Some(first);
        while let Some(e) = next.take() {
            absorb(&root, &ignore, &watcher, e, &mut batch);
            let left = MAX_WAIT.saturating_sub(started.elapsed());
            if left.is_zero() {
                break;
            }
            next = rx.recv_timeout(QUIET.min(left)).ok();
        }
        if !batch.changed.is_empty() {
            on_batch(batch);
        }
    }
}

fn absorb(
    root: &Path,
    ignore: &Gitignore,
    watcher: &Weak<Mutex<RecommendedWatcher>>,
    e: notify::Event,
    batch: &mut Batch,
) {
    let structural = match e.kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) => {
            true
        }
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any) => false,
        _ => return,
    };
    for p in e.paths {
        let Some(rel) = relative(root, &p) else {
            continue;
        };
        let is_dir = p.is_dir();
        if ignore.matched_path_or_any_parents(&p, is_dir).is_ignore() {
            continue;
        }
        if is_dir
            && matches!(
                e.kind,
                EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
            )
            && let Some(w) = watcher.upgrade()
        {
            let _ = w.lock().watch(&p, RecursiveMode::NonRecursive);
        }
        batch.structural |= structural;
        batch.changed.insert(rel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(rx: &mpsc::Receiver<Batch>, what: impl Fn(&Batch) -> bool) -> Batch {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let b = rx.recv_timeout(left).expect("a batch arrives");
            if what(&b) {
                return b;
            }
        }
    }

    #[test]
    fn edits_new_files_and_new_folders_arrive_ignored_ones_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let _w = start(
            root.clone(),
            &["src/a.rs".to_string()],
            Arc::new(move |b| {
                let _ = tx.lock().send(b);
            }),
        )
        .unwrap();

        std::fs::write(root.join("src/a.rs"), "fn a() { b() }\n").unwrap();
        wait_for(&rx, |b| b.changed.contains("src/a.rs"));

        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/x.o"), "x").unwrap();
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        // Let the new folder's watch land before writing into it.
        std::thread::sleep(Duration::from_millis(400));
        std::fs::write(root.join("src/nested/c.rs"), "fn c() {}\n").unwrap();
        let b = wait_for(&rx, |b| b.changed.contains("src/nested/c.rs"));
        assert!(b.structural);
        assert!(!b.changed.iter().any(|p| p.starts_with("target")));
    }
}
