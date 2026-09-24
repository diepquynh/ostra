//! The per-project index behind usages, dependencies, and symbol search. It keeps, per file, the
//! set of names it mentions, its definitions, and its imports, never its text. A usages query
//! re-reads only the files that mention the name. `sync` re-analyzes the files whose size or
//! modification time changed, in parallel.

use crate::lang::{self, Lang};
use crate::outline::RawImport;
use crate::resolve::{Resolver, Target, parent};
use crate::{MAX_FILE_BYTES, lex, outline, preview};
use ostra_core::api::FileIndex;
use ostra_core::code::{
    CodeDeps, CodeImport, CodeImporter, CodeLocation, CodeSymbols, CodeUsages, NATIVE_PROVIDER,
    SymbolKind,
};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// Source bytes one project index reads before it stops adding files.
pub const MAX_INDEX_BYTES: u64 = 256 * 1024 * 1024;
/// Files a usages query reads.
pub const MAX_SCAN_FILES: usize = 4_000;
pub const MAX_IMPORTERS: usize = 2_000;
/// Every file is checked on disk again after this long, for edits made outside Ostra.
const FULL_SYNC: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    size: u64,
    mtime: Option<SystemTime>,
}

struct Def {
    name: u32,
    kind: SymbolKind,
    line: u32,
    col: u32,
    len: u32,
    container: Option<Box<str>>,
    preview: Box<str>,
}

struct Entry {
    path: String,
    lang: &'static Lang,
    stamp: Stamp,
    bytes: u64,
    names: Vec<u32>,
    defs: Vec<Def>,
    imports: Vec<RawImport>,
    targets: Vec<Option<Target>>,
    alive: bool,
}

/// What one file's analysis produced, before names are interned.
struct Facts {
    names: Vec<String>,
    defs: Vec<(outline::Symbol, String)>,
    imports: Vec<RawImport>,
}

fn is_minified(src: &str) -> bool {
    let lines = src.bytes().filter(|&b| b == b'\n').count() + 1;
    src.len() > 50_000 && src.len() / lines > 1_000
}

fn analyze(src: &str, lang: &Lang) -> Facts {
    let toks = lex::lex(src, lang);
    let an = outline::analyze(src, lang, &toks);
    let mut names: Vec<String> = toks
        .iter()
        .filter(|t| {
            matches!(
                t.kind,
                lex::Kind::Ident | lex::Kind::Type | lex::Kind::Macro
            )
        })
        .map(|t| t.text(src).trim_end_matches('!').to_string())
        .collect();
    names.sort_unstable();
    names.dedup();
    let defs = an
        .symbols
        .into_iter()
        .map(|s| {
            let p = preview(src, s.byte as usize);
            (s, p)
        })
        .collect();
    Facts {
        names,
        defs,
        imports: an.imports,
    }
}

fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.iter().take(8_000).any(|&b| b == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    m.is_file().then(|| Stamp {
        size: m.len(),
        mtime: m.modified().ok(),
    })
}

/// Run `f` over `items` on a few threads, keeping the input order.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8)
        .min(items.len().max(1));
    if workers <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    *slots[i].lock() = Some(f(item));
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|m| m.into_inner().expect("every item ran"))
        .collect()
}

pub struct ProjectIndex {
    root: PathBuf,
    entries: Vec<Entry>,
    by_path: HashMap<String, u32>,
    names: HashMap<Box<str>, u32>,
    name_list: Vec<Box<str>>,
    /// Name to the files that mention it, sorted.
    postings: HashMap<u32, Vec<u32>>,
    /// Name to the (file, definition) pairs that define it.
    defs_by_name: HashMap<u32, Vec<(u32, u32)>>,
    resolver: Arc<Resolver>,
    importers: HashMap<u32, Vec<(u32, u32)>>,
    /// Folder to the imports that name it (Go packages).
    folder_importers: HashMap<String, Vec<(u32, u32)>>,
    linked: bool,
    bytes: u64,
    /// The byte cap left files out.
    pub truncated: bool,
}

impl ProjectIndex {
    pub fn new(root: PathBuf) -> Self {
        ProjectIndex {
            root,
            entries: vec![],
            by_path: HashMap::new(),
            names: HashMap::new(),
            name_list: vec![],
            postings: HashMap::new(),
            defs_by_name: HashMap::new(),
            resolver: Arc::new(Resolver::default()),
            importers: HashMap::new(),
            folder_importers: HashMap::new(),
            linked: false,
            bytes: 0,
            truncated: false,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn files(&self) -> usize {
        self.by_path.len()
    }

    pub fn set_resolver(&mut self, r: Arc<Resolver>) {
        if !Arc::ptr_eq(&self.resolver, &r) {
            self.resolver = r;
            self.linked = false;
        }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.names.get(s) {
            return id;
        }
        let id = self.name_list.len() as u32;
        self.name_list.push(s.into());
        self.names.insert(s.into(), id);
        id
    }

    fn remove(&mut self, id: u32) {
        let e = &mut self.entries[id as usize];
        if !e.alive {
            return;
        }
        e.alive = false;
        self.bytes -= e.bytes;
        let names = std::mem::take(&mut e.names);
        let defs = std::mem::take(&mut e.defs);
        self.by_path.remove(&e.path);
        for n in names {
            if let Some(list) = self.postings.get_mut(&n)
                && let Ok(i) = list.binary_search(&id)
            {
                list.remove(i);
            }
        }
        for d in defs {
            if let Some(list) = self.defs_by_name.get_mut(&d.name) {
                list.retain(|(f, _)| *f != id);
            }
        }
        self.linked = false;
    }

    fn insert(&mut self, path: String, lang: &'static Lang, stamp: Stamp, facts: Facts) {
        let id = self.entries.len() as u32;
        let names: Vec<u32> = facts.names.iter().map(|n| self.intern(n)).collect();
        for &n in &names {
            let list = self.postings.entry(n).or_default();
            // Ids only grow, so pushing keeps each list sorted.
            list.push(id);
        }
        let mut defs = Vec::with_capacity(facts.defs.len());
        for (i, (s, preview)) in facts.defs.into_iter().enumerate() {
            let name = self.intern(&s.name);
            self.defs_by_name
                .entry(name)
                .or_default()
                .push((id, i as u32));
            defs.push(Def {
                name,
                kind: s.kind,
                line: s.line,
                col: s.col,
                len: s.len,
                container: s.container.map(Into::into),
                preview: preview.into(),
            });
        }
        self.bytes += stamp.size;
        self.by_path.insert(path.clone(), id);
        self.entries.push(Entry {
            path,
            lang,
            stamp,
            bytes: stamp.size,
            names,
            defs,
            imports: facts.imports,
            targets: vec![],
            alive: true,
        });
        self.linked = false;
    }

    /// Bring the index in line with the disk. `only` limits the check to those paths; `None`
    /// checks every path of the file list and drops files that left it.
    pub fn sync(&mut self, paths: &[String], only: Option<&HashSet<String>>) {
        let listed: Vec<(&String, &'static Lang)> = paths
            .iter()
            .filter(|p| only.is_none_or(|o| o.contains(*p)))
            .filter_map(|p| Some((p, lang::for_path(p).filter(|l| l.indexed())?)))
            .collect();
        match only {
            None => {
                let keep: HashSet<&str> = listed.iter().map(|(p, _)| p.as_str()).collect();
                let gone: Vec<u32> = self
                    .by_path
                    .iter()
                    .filter(|(p, _)| !keep.contains(p.as_str()))
                    .map(|(_, &id)| id)
                    .collect();
                for id in gone {
                    self.remove(id);
                }
            }
            Some(o) => {
                // A touched path missing from the list was deleted or is now ignored.
                let listed_set: HashSet<&str> = listed.iter().map(|(p, _)| p.as_str()).collect();
                let gone: Vec<u32> = o
                    .iter()
                    .filter(|p| !listed_set.contains(p.as_str()))
                    .filter_map(|p| self.by_path.get(p).copied())
                    .collect();
                for id in gone {
                    self.remove(id);
                }
            }
        }
        let root = self.root.clone();
        let stamps = parallel(&listed, |(p, _)| stamp(&root.join(p)));
        let mut jobs = vec![];
        for ((p, lang), st) in listed.into_iter().zip(stamps) {
            let existing = self.by_path.get(p.as_str()).copied();
            match st {
                Some(st) if st.size <= MAX_FILE_BYTES => {
                    let same = existing.is_some_and(|id| self.entries[id as usize].stamp == st);
                    if !same {
                        jobs.push((p.clone(), lang, st, existing));
                    }
                }
                _ => {
                    if let Some(id) = existing {
                        self.remove(id);
                    }
                }
            }
        }
        if jobs.is_empty() {
            return;
        }
        let results = parallel(&jobs, |(p, lang, _, _)| {
            let src = read_text(&root.join(p))?;
            (!is_minified(&src)).then(|| analyze(&src, lang))
        });
        for ((path, lang, st, existing), facts) in jobs.into_iter().zip(results) {
            if let Some(id) = existing {
                self.remove(id);
            }
            let Some(facts) = facts else { continue };
            if self.bytes + st.size > MAX_INDEX_BYTES {
                self.truncated = true;
                continue;
            }
            self.insert(path, lang, st, facts);
        }
    }

    fn link(&mut self) {
        if self.linked {
            return;
        }
        self.importers.clear();
        self.folder_importers.clear();
        let resolver = self.resolver.clone();
        for (id, e) in self.entries.iter_mut().enumerate() {
            if !e.alive {
                continue;
            }
            e.targets = e
                .imports
                .iter()
                .map(|i| resolver.resolve(&e.path, i, e.lang))
                .collect();
            for (i, t) in e.targets.iter().enumerate() {
                let Some(t) = t else { continue };
                let edge = (id as u32, i as u32);
                if t.folder {
                    self.folder_importers
                        .entry(t.path.clone())
                        .or_default()
                        .push(edge);
                } else if let Some(&to) = self.by_path.get(&t.path)
                    && to != id as u32
                {
                    self.importers.entry(to).or_default().push(edge);
                }
            }
        }
        self.linked = true;
    }

    fn location(&self, file: u32, def: &Def) -> CodeLocation {
        CodeLocation {
            name: self.name_list[def.name as usize].to_string(),
            path: self.entries[file as usize].path.clone(),
            line: def.line,
            col: def.col,
            len: def.len,
            preview: def.preview.to_string(),
            kind: Some(def.kind),
            container: def.container.as_deref().map(str::to_string),
        }
    }

    /// Where `symbol` is defined and used. Files are read in this order: `from`, its folder,
    /// then the rest by path.
    pub fn usages(&self, symbol: &str, from: Option<&str>, limit: usize) -> CodeUsages {
        let symbol = symbol.trim_end_matches('!');
        let mut out = CodeUsages {
            symbol: symbol.to_string(),
            provider: NATIVE_PROVIDER.to_string(),
            definitions: vec![],
            references: vec![],
            truncated: false,
            warning: None,
        };
        let Some(&name) = self.names.get(symbol) else {
            return out;
        };
        let def_at: HashSet<(u32, u32, u32)> = self
            .defs_by_name
            .get(&name)
            .into_iter()
            .flatten()
            .map(|&(f, d)| {
                let def = &self.entries[f as usize].defs[d as usize];
                (f, def.line, def.col)
            })
            .collect();
        let mut defs: Vec<(u32, u32)> = self.defs_by_name.get(&name).cloned().unwrap_or_default();
        let from_dir = from.map(parent);
        let rank = |f: u32| {
            let p = &self.entries[f as usize].path;
            let tier = if Some(p.as_str()) == from {
                0
            } else if Some(parent(p)) == from_dir {
                1
            } else {
                2
            };
            (tier, p.clone())
        };
        defs.sort_by_key(|&(f, d)| (rank(f), d));
        out.definitions = defs
            .iter()
            .map(|&(f, d)| self.location(f, &self.entries[f as usize].defs[d as usize]))
            .collect();
        let mut files: Vec<u32> = self.postings.get(&name).cloned().unwrap_or_default();
        files.sort_by_cached_key(|&f| rank(f));
        if files.len() > MAX_SCAN_FILES {
            files.truncate(MAX_SCAN_FILES);
            out.truncated = true;
        }
        let found = parallel(&files, |&f| {
            let e = &self.entries[f as usize];
            let Some(src) = read_text(&self.root.join(&e.path)) else {
                return vec![];
            };
            lex::lex(&src, e.lang)
                .into_iter()
                .filter(|t| {
                    matches!(
                        t.kind,
                        lex::Kind::Ident | lex::Kind::Type | lex::Kind::Macro
                    ) && t.text(&src).trim_end_matches('!') == symbol
                        && !def_at.contains(&(f, t.line, t.col))
                })
                .map(|t| CodeLocation {
                    name: symbol.to_string(),
                    path: e.path.clone(),
                    line: t.line,
                    col: t.col,
                    len: t.len,
                    preview: preview(&src, t.start as usize),
                    kind: None,
                    container: None,
                })
                .collect::<Vec<_>>()
        });
        for locs in found {
            for l in locs {
                if out.references.len() >= limit {
                    out.truncated = true;
                    return out;
                }
                out.references.push(l);
            }
        }
        out
    }

    pub fn deps(&mut self, path: &str) -> CodeDeps {
        self.link();
        let mut out = CodeDeps {
            path: path.to_string(),
            provider: NATIVE_PROVIDER.to_string(),
            imports: vec![],
            importers: vec![],
            truncated: false,
            warning: None,
        };
        let Some(&id) = self.by_path.get(path) else {
            return out;
        };
        let e = &self.entries[id as usize];
        out.imports = e
            .imports
            .iter()
            .zip(&e.targets)
            .map(|(i, t)| CodeImport {
                spec: i.spec.clone(),
                line: i.line,
                target: t.as_ref().map(|t| t.path.clone()),
                folder: t.as_ref().is_some_and(|t| t.folder),
            })
            .collect();
        let edges = self.importers.get(&id).into_iter().flatten().chain(
            self.folder_importers
                .get(parent(path))
                .into_iter()
                .flatten(),
        );
        let mut importers: Vec<CodeImporter> = edges
            .filter(|&&(f, _)| f != id)
            .map(|&(f, i)| {
                let from = &self.entries[f as usize];
                let imp = &from.imports[i as usize];
                CodeImporter {
                    path: from.path.clone(),
                    line: imp.line,
                    spec: imp.spec.clone(),
                }
            })
            .collect();
        importers.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        importers.dedup_by(|a, b| a.path == b.path && a.line == b.line);
        if importers.len() > MAX_IMPORTERS {
            importers.truncate(MAX_IMPORTERS);
            out.truncated = true;
        }
        out.importers = importers;
        out
    }

    /// Definitions whose name matches `query`: equal, prefix, substring, then initials
    /// (`pn` finds `parse_name` and `ParseName`), ignoring case.
    pub fn symbols(&self, query: &str, limit: usize) -> CodeSymbols {
        let q = query.trim().to_lowercase();
        let mut out = CodeSymbols {
            provider: NATIVE_PROVIDER.to_string(),
            items: vec![],
            truncated: false,
            warning: None,
        };
        if q.is_empty() {
            return out;
        }
        let mut scored: Vec<(u32, u32)> = self
            .defs_by_name
            .iter()
            .filter(|(_, d)| !d.is_empty())
            .filter_map(|(&n, _)| {
                let name = &self.name_list[n as usize];
                name_score(name, &q).map(|s| (s, n))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| {
                    self.name_list[a.1 as usize]
                        .len()
                        .cmp(&self.name_list[b.1 as usize].len())
                })
                .then_with(|| self.name_list[a.1 as usize].cmp(&self.name_list[b.1 as usize]))
        });
        for (_, n) in scored {
            for &(f, d) in &self.defs_by_name[&n] {
                if out.items.len() >= limit {
                    out.truncated = true;
                    return out;
                }
                out.items
                    .push(self.location(f, &self.entries[f as usize].defs[d as usize]));
            }
        }
        out
    }
}

/// Higher is better; `None` when the name does not match.
fn name_score(name: &str, q: &str) -> Option<u32> {
    if name == q {
        return Some(110);
    }
    let lower = name.to_lowercase();
    if lower == q {
        return Some(100);
    }
    if lower.starts_with(q) {
        return Some(80);
    }
    if lower.contains(q) {
        return Some(60);
    }
    let initials: String = name
        .char_indices()
        .filter(|&(i, c)| i == 0 || c.is_uppercase() || name[..i].ends_with('_'))
        .map(|(_, c)| c.to_ascii_lowercase())
        .filter(|c| *c != '_')
        .collect();
    (initials.len() > 1 && initials.starts_with(q)).then_some(40)
}

struct Slot {
    state: Mutex<SlotState>,
    /// Paths written since the last query, kept apart so a write never waits on a build.
    dirty: Mutex<HashSet<String>>,
}

struct SlotState {
    index: ProjectIndex,
    list: Option<Arc<FileIndex>>,
    last_full: Option<Instant>,
    resolver: Option<(Arc<FileIndex>, Arc<Resolver>)>,
}

/// One index per project, built on first use and kept current from write touches and a
/// periodic check of the disk.
pub struct Indexes<K> {
    slots: Mutex<HashMap<K, Arc<Slot>>>,
}

impl<K> Default for Indexes<K> {
    fn default() -> Self {
        Indexes {
            slots: Mutex::new(HashMap::new()),
        }
    }
}

impl<K: Hash + Eq + Clone> Indexes<K> {
    fn slot(&self, key: &K, root: &Path) -> Arc<Slot> {
        let mut slots = self.slots.lock();
        let slot = slots.entry(key.clone()).or_insert_with(|| {
            Arc::new(Slot {
                state: Mutex::new(SlotState {
                    index: ProjectIndex::new(root.to_path_buf()),
                    list: None,
                    last_full: None,
                    resolver: None,
                }),
                dirty: Mutex::new(HashSet::new()),
            })
        });
        slot.clone()
    }

    /// Mark a project-relative path as changed.
    pub fn touch(&self, key: &K, rel: &str) {
        if let Some(s) = self.slots.lock().get(key) {
            s.dirty.lock().insert(rel.to_string());
        }
    }

    pub fn forget(&self, key: &K) {
        self.slots.lock().remove(key);
    }

    /// The import resolver for a file list. Cheap next to a full index: it reads only manifests.
    /// Blocking.
    pub fn resolver(&self, key: &K, root: &Path, list: &Arc<FileIndex>) -> Arc<Resolver> {
        let slot = self.slot(key, root);
        let mut st = slot.state.lock();
        Self::resolver_for(&mut st, root, list)
    }

    fn resolver_for(st: &mut SlotState, root: &Path, list: &Arc<FileIndex>) -> Arc<Resolver> {
        if let Some((l, r)) = &st.resolver
            && Arc::ptr_eq(l, list)
        {
            return r.clone();
        }
        let r = Arc::new(Resolver::new(&list.paths, |p| {
            std::fs::read_to_string(root.join(p)).ok()
        }));
        st.resolver = Some((list.clone(), r.clone()));
        r
    }

    /// Run `f` on the project's index after bringing it up to date. Blocking: the first call
    /// on a project reads every source file.
    pub fn with<R>(
        &self,
        key: &K,
        root: &Path,
        list: &Arc<FileIndex>,
        f: impl FnOnce(&mut ProjectIndex) -> R,
    ) -> R {
        let slot = self.slot(key, root);
        let mut st = slot.state.lock();
        if st.index.root() != root {
            st.index = ProjectIndex::new(root.to_path_buf());
            st.list = None;
            st.last_full = None;
            st.resolver = None;
        }
        let dirty = std::mem::take(&mut *slot.dirty.lock());
        let resolver = Self::resolver_for(&mut st, root, list);
        st.index.set_resolver(resolver);
        let list_changed = st.list.as_ref().is_none_or(|l| !Arc::ptr_eq(l, list));
        let stale = st.last_full.is_none_or(|t| t.elapsed() > FULL_SYNC);
        if list_changed || stale {
            st.index.sync(&list.paths, None);
            st.list = Some(list.clone());
            st.last_full = Some(Instant::now());
        } else if !dirty.is_empty() {
            st.index.sync(&list.paths, Some(&dirty));
        }
        f(&mut st.index)
    }
}
