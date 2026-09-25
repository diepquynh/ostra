//! The project's dependency graph, derived from the index without reading files. Import edges come
//! from the resolver. Reference edges link a file to the files that define the names it mentions,
//! searched the way a reader would look: the file itself, the files it imports, its folder, its
//! package, then the packages its package imports. Callers and callees re-read only the files they
//! report on.

use crate::index::{Def, ProjectIndex, parallel, read_text};
use crate::lang::{Family, Lang};
use crate::lex;
use crate::outline::ImportStyle;
use crate::resolve::parent;
use ostra_core::code::{CodeLocation, SymbolKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// A name defined in more files than this links none of them, unless the mentioning file imports
/// the defining file directly.
pub const MAX_DEF_FILES: usize = 3;
/// Names listed per step of a walk or a chain.
pub const MAX_LINK_NAMES: usize = 8;
/// Files a transitive walk visits before it stops.
pub const MAX_REACH: usize = 5_000;
/// References a callers query reads.
pub const MAX_CALLER_REFS: usize = 5_000;
/// A file linked to more files than this is not used as a stepping stone by `related`.
const MAX_BRIDGE_DEGREE: usize = 200;

#[derive(Default)]
pub(crate) struct Graph {
    unit: Vec<u32>,
    units: Vec<String>,
    unit_deps: Vec<HashSet<u32>>,
    test: Vec<bool>,
    hub: Vec<bool>,
    imp_files: Vec<HashSet<u32>>,
    imp_dirs: Vec<HashSet<String>>,
    /// Files whose every name a file re-exports (`export * from`).
    star: Vec<HashSet<u32>>,
    /// Per file, what it depends on.
    out: Vec<Vec<Edge>>,
    /// Per file, what depends on it; `other` is the depending file.
    inn: Vec<Vec<Edge>>,
}

#[derive(Clone)]
struct Edge {
    other: u32,
    /// Lines of the depending file's imports that name the other file, each with whether it is a
    /// Rust `mod` declaration.
    imports: Vec<(u32, bool)>,
    /// Names the depending file uses from the other.
    names: Vec<u32>,
}

impl Edge {
    fn new(other: u32) -> Self {
        Edge {
            other,
            imports: vec![],
            names: vec![],
        }
    }

    fn real_import(&self) -> bool {
        self.imports.iter().any(|&(_, m)| !m)
    }

    /// Worth following in a transitive walk. A `mod` declaration is containment, not use, and an
    /// import-only edge into an index module is a re-export path whose names already link to the
    /// files that define them.
    fn follow(&self, into_hub: bool) -> bool {
        !self.names.is_empty() || (self.real_import() && !into_hub)
    }

    fn weight(&self) -> u32 {
        2 * self.names.len().min(MAX_LINK_NAMES) as u32 + u32::from(!self.imports.is_empty())
    }
}

fn is_test(p: &str) -> bool {
    let (dir, name) = p.rsplit_once('/').unwrap_or(("", p));
    let in_test_dir = dir.split('/').any(|s| {
        matches!(
            s,
            "test" | "tests" | "__tests__" | "spec" | "specs" | "testdata" | "fixtures"
        )
    });
    in_test_dir
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.starts_with("test_")
        || [
            "_test.go",
            "_test.py",
            "_test.rs",
            "_spec.rb",
            "_test.rb",
            "Test.java",
            "Tests.java",
            "Test.kt",
            "Tests.cs",
            "Test.php",
        ]
        .iter()
        .any(|s| name.ends_with(s))
}

/// Index modules gather and re-export a folder: `lib.rs`, `index.ts`, `__init__.py`. A file
/// with an `export * from` counts too.
fn is_hub(p: &str) -> bool {
    let name = p.rsplit('/').next().unwrap_or(p);
    let stem = name.split_once('.').map_or(name, |(s, _)| s);
    matches!(stem, "lib" | "main" | "mod" | "index" | "__init__")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// What the seeds use, and what that uses.
    Dependencies,
    /// What uses the seeds, and what uses that: the files a change can break.
    Dependents,
}

/// One file next to another in the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Link {
    pub path: String,
    /// Lines, in the depending file, of the imports that name the other file.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub import_lines: Vec<u32>,
    /// Every import is a Rust `mod` declaration: the other file is a child module.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub module: bool,
    /// Names the depending file uses from the other.
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Neighbors {
    pub path: String,
    /// The package folder, `""` for the project top.
    pub unit: String,
    pub test: bool,
    pub uses: Vec<Link>,
    pub used_by: Vec<Link>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reached {
    pub path: String,
    /// Hops from the nearest seed.
    pub distance: u32,
    /// The file one hop closer to the seeds.
    pub via: String,
    /// Names on the edge between `via` and this file, the first few.
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reach {
    pub direction: Direction,
    pub seeds: Vec<String>,
    /// Seeds the index does not hold.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Nearest first.
    pub reached: Vec<Reached>,
    pub truncated: bool,
}

/// One step of a dependency chain: this file uses the next one through `names` or an import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hop {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_line: Option<u32>,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Related {
    pub path: String,
    pub score: f32,
    /// 1 for a direct link, 2 through one other file.
    pub distance: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hub {
    pub path: String,
    pub unit: String,
    pub dependents: usize,
    pub dependencies: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitDep {
    pub path: String,
    /// File-to-file links from this package into that one.
    pub links: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitNode {
    pub path: String,
    pub files: usize,
    pub tests: usize,
    pub depends_on: Vec<UnitDep>,
    pub used_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitMap {
    pub units: Vec<UnitNode>,
    /// Packages that depend on each other in a loop.
    pub cycles: Vec<Vec<String>>,
}

/// One definition that mentions a symbol, with the lines where it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Caller {
    pub path: String,
    /// The enclosing definition; `None` at the top level of the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<SymbolKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub lines: Vec<u32>,
    /// Every line is an import statement of the file.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub import: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Callers {
    pub symbol: String,
    pub definitions: Vec<CodeLocation>,
    pub callers: Vec<Caller>,
    pub truncated: bool,
}

/// One definition reached by a symbol walk: it uses `uses`, which is the start symbol or a
/// definition reached one hop earlier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolUse {
    pub path: String,
    /// `None` for a use at the top level of the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<SymbolKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    pub lines: Vec<u32>,
    pub uses: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolImpact {
    pub symbol: String,
    pub definitions: Vec<CodeLocation>,
    /// `hops[0]` uses the symbol itself; `hops[i]` uses something in `hops[i - 1]`.
    pub hops: Vec<Vec<SymbolUse>>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Callees {
    pub path: String,
    pub symbol: String,
    pub line: u32,
    /// `None` when the language gives no body end; only the first line was read.
    pub end_line: Option<u32>,
    /// Definitions the body names, in order of first mention.
    pub uses: Vec<CodeLocation>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Qual {
    None,
    /// `self.x`, `this.x`, `Self::x`: the file's own definitions.
    SelfLike,
    /// `Q::x` or `Q.x`.
    Name(u32),
}

/// One way a file mentions a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Mention {
    pub(crate) name: u32,
    pub(crate) qual: Qual,
    /// After `.` or `->`: a member of a value whose type the tokens do not tell.
    pub(crate) member: bool,
}

pub(crate) enum RawQual {
    None,
    SelfLike,
    Name(String),
}

pub(crate) struct RawMention {
    pub(crate) name: String,
    pub(crate) qual: RawQual,
    pub(crate) member: bool,
}

fn name_like(k: lex::Kind) -> bool {
    matches!(k, lex::Kind::Ident | lex::Kind::Type | lex::Kind::Macro)
}

/// How token `i`, a name, is mentioned, from the tokens just before it.
fn mention_at(src: &str, toks: &[lex::Tok], i: usize, lang: &Lang) -> RawMention {
    let prev = |from: usize| {
        (0..from)
            .rev()
            .find(|&j| toks[j].kind != lex::Kind::Comment)
    };
    let next = |from: usize| (from + 1..toks.len()).find(|&j| toks[j].kind != lex::Kind::Comment);
    let punct = |j: Option<usize>, c: u8| j.is_some_and(|j| toks[j].kind == lex::Kind::Punct(c));
    let name = toks[i].text(src).trim_end_matches('!').to_string();
    let p = prev(i);
    let n = next(i);
    let n2 = n.and_then(next);
    // `label: x` and `label="x"` name a field of some type: struct and object literal fields,
    // typed parameters, keyword arguments, JSX attributes.
    let key = (punct(n, b':')
        && !punct(n2, b':')
        && !punct(p, b':')
        && !p.is_some_and(|j| matches!(toks[j].text(src), "case" | "default")))
        || (punct(n, b'=')
            && !punct(n2, b'=')
            && !punct(n2, b'>')
            && (punct(p, b'(')
                || punct(p, b',')
                || (lang.family == Family::Js
                    && p.is_some_and(|j| {
                        name_like(toks[j].kind)
                            || toks[j].kind == lex::Kind::Str
                            || toks[j].kind == lex::Kind::Punct(b'}')
                    }))));
    let (member, sep_start) = if punct(p, b'.') {
        let j = p.expect("checked");
        let q = prev(j);
        (true, if punct(q, b'?') { q } else { Some(j) })
    } else if punct(p, b'>') && punct(p.and_then(prev), b'-') {
        (true, p.and_then(prev))
    } else if punct(p, b':') && punct(p.and_then(prev), b':') {
        (false, p.and_then(prev))
    } else {
        return RawMention {
            name,
            qual: RawQual::None,
            member: key,
        };
    };
    let qual = match sep_start.and_then(prev) {
        Some(q) => {
            let t = &toks[q];
            let text = t.text(src);
            match text {
                "self" | "this" | "cls" => {
                    if member {
                        RawQual::SelfLike
                    } else {
                        RawQual::None
                    }
                }
                "Self" => RawQual::SelfLike,
                _ if name_like(t.kind) => RawQual::Name(text.to_string()),
                _ => RawQual::None,
            }
        }
        None => RawQual::None,
    };
    RawMention { name, qual, member }
}

/// Every mention of a name in a file's tokens.
pub(crate) fn mentions(src: &str, toks: &[lex::Tok], lang: &Lang) -> Vec<RawMention> {
    (0..toks.len())
        .filter(|&i| name_like(toks[i].kind))
        .map(|i| mention_at(src, toks, i, lang))
        .collect()
}

/// A bare name is a type, function, constant, variable, or macro; fields and methods need a
/// receiver or a qualifier.
fn bare_kind(k: SymbolKind) -> bool {
    !matches!(k, SymbolKind::Field | SymbolKind::Method)
}

fn exported_bare(d: &Def) -> bool {
    match d.kind {
        SymbolKind::Class
        | SymbolKind::Enum
        | SymbolKind::Interface
        | SymbolKind::Type
        | SymbolKind::Macro => true,
        SymbolKind::Constant => d.container.is_none(),
        _ => false,
    }
}

/// The name a file's module goes by: its stem, or its folder for an index module.
fn module_name(p: &str) -> &str {
    let (dir, name) = p.rsplit_once('/').unwrap_or(("", p));
    let stem = name.split_once('.').map_or(name, |(s, _)| s);
    if is_hub(p) {
        dir.rsplit('/').next().unwrap_or(dir)
    } else {
        stem
    }
}

fn member_kind(k: SymbolKind) -> bool {
    matches!(
        k,
        SymbolKind::Method
            | SymbolKind::Field
            | SymbolKind::Function
            | SymbolKind::Constant
            | SymbolKind::Variable
    )
}

fn distinct_files(defs: &[(u32, u32)]) -> usize {
    let mut files: Vec<u32> = defs.iter().map(|&(f, _)| f).collect();
    files.sort_unstable();
    files.dedup();
    files.len()
}

/// Strongly connected components of `adj`, iterative Tarjan.
fn sccs(adj: &[Vec<u32>]) -> Vec<Vec<u32>> {
    const NONE: u32 = u32::MAX;
    let n = adj.len();
    let mut index = vec![NONE; n];
    let mut low = vec![0u32; n];
    let mut on = vec![false; n];
    let mut stack: Vec<usize> = vec![];
    let mut out = vec![];
    let mut next = 0u32;
    for root in 0..n {
        if index[root] != NONE {
            continue;
        }
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on[root] = true;
        let mut work = vec![(root, 0usize)];
        while let Some(&(v, i)) = work.last() {
            if let Some(&w) = adj[v].get(i) {
                work.last_mut().expect("non-empty").1 += 1;
                let w = w as usize;
                if index[w] == NONE {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on[w] = true;
                    work.push((w, 0));
                } else if on[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(p, _)) = work.last() {
                low[p] = low[p].min(low[v]);
            }
            if low[v] == index[v] {
                let mut comp = vec![];
                loop {
                    let w = stack.pop().expect("v is on the stack");
                    on[w] = false;
                    comp.push(w as u32);
                    if w == v {
                        break;
                    }
                }
                out.push(comp);
            }
        }
    }
    out
}

impl ProjectIndex {
    fn graph_ref(&self) -> &Graph {
        self.graph.as_ref().expect("ensure_graph ran")
    }

    pub(crate) fn ensure_graph(&mut self) {
        self.link();
        if self.graph.is_none() {
            self.graph = Some(self.build_graph());
        }
    }

    fn build_graph(&self) -> Graph {
        let n = self.entries.len();
        let mut g = Graph {
            unit: vec![0; n],
            test: vec![false; n],
            hub: vec![false; n],
            imp_files: vec![HashSet::new(); n],
            imp_dirs: vec![HashSet::new(); n],
            star: vec![HashSet::new(); n],
            ..Graph::default()
        };
        let mut unit_ids: HashMap<String, u32> = HashMap::new();
        let mut intern = |u: &str, units: &mut Vec<String>| -> u32 {
            *unit_ids.entry(u.to_string()).or_insert_with(|| {
                units.push(u.to_string());
                units.len() as u32 - 1
            })
        };
        let mut dir_files: HashMap<&str, Vec<u32>> = HashMap::new();
        let mut alive = vec![];
        for (id, e) in self.entries.iter().enumerate() {
            if !e.alive {
                continue;
            }
            alive.push(id as u32);
            g.unit[id] = intern(self.resolver.unit_of_dir(parent(&e.path)), &mut g.units);
            g.test[id] = is_test(&e.path);
            g.hub[id] = is_hub(&e.path);
            dir_files
                .entry(parent(&e.path))
                .or_default()
                .push(id as u32);
        }
        let mut unit_pairs = vec![];
        for &id in &alive {
            let e = &self.entries[id as usize];
            for (imp, t) in e.imports.iter().zip(&e.targets) {
                let Some(t) = t else { continue };
                if imp.star
                    && let Some(&to) = self.by_path.get(&t.path)
                {
                    g.star[id as usize].insert(to);
                }
                if t.folder {
                    g.imp_dirs[id as usize].insert(t.path.clone());
                    let u = intern(self.resolver.unit_of_dir(&t.path), &mut g.units);
                    unit_pairs.push((g.unit[id as usize], u));
                } else if let Some(&to) = self.by_path.get(&t.path)
                    && to != id
                {
                    g.imp_files[id as usize].insert(to);
                    unit_pairs.push((g.unit[id as usize], g.unit[to as usize]));
                }
            }
        }
        for id in &alive {
            let i = *id as usize;
            g.hub[i] |= !g.star[i].is_empty();
        }
        g.unit_deps = vec![HashSet::new(); g.units.len()];
        for (a, b) in unit_pairs {
            if a != b {
                g.unit_deps[a as usize].insert(b);
            }
        }
        let outs = parallel(&alive, |&a| self.out_edges(&g, &dir_files, a));
        g.out = vec![vec![]; n];
        g.inn = vec![vec![]; n];
        for (&a, edges) in alive.iter().zip(outs) {
            for e in &edges {
                g.inn[e.other as usize].push(Edge {
                    other: a,
                    imports: e.imports.clone(),
                    names: e.names.clone(),
                });
            }
            g.out[a as usize] = edges;
        }
        g
    }

    fn out_edges(&self, g: &Graph, dir_files: &HashMap<&str, Vec<u32>>, a: u32) -> Vec<Edge> {
        let e = &self.entries[a as usize];
        let mut map: BTreeMap<u32, Edge> = BTreeMap::new();
        let mut folders = vec![];
        for (imp, t) in e.imports.iter().zip(&e.targets) {
            let Some(t) = t else { continue };
            if t.folder {
                folders.push((t.path.as_str(), imp.line));
            } else if let Some(&to) = self.by_path.get(&t.path)
                && to != a
            {
                map.entry(to)
                    .or_insert_with(|| Edge::new(to))
                    .imports
                    .push((imp.line, imp.style == ImportStyle::Mod));
            }
        }
        for &m in &e.mentions {
            if self.name_list[m.name as usize].chars().nth(1).is_none() {
                continue;
            }
            for (f, _) in self.resolve(g, a, m) {
                if f != a {
                    map.entry(f)
                        .or_insert_with(|| Edge::new(f))
                        .names
                        .push(m.name);
                }
            }
        }
        // A folder import (a Go package) points at the files whose names the importer uses, or at
        // the whole folder when it uses none.
        for (dir, line) in folders {
            let Some(files) = dir_files.get(dir) else {
                continue;
            };
            let used: Vec<u32> = files
                .iter()
                .copied()
                .filter(|f| *f != a && map.get(f).is_some_and(|e| !e.names.is_empty()))
                .collect();
            let to = if used.is_empty() {
                files.iter().copied().filter(|f| *f != a).collect()
            } else {
                used
            };
            for f in to {
                map.entry(f)
                    .or_insert_with(|| Edge::new(f))
                    .imports
                    .push((line, false));
            }
        }
        let mut out: Vec<Edge> = map.into_values().collect();
        for e in &mut out {
            e.names.sort_unstable_by(|x, y| {
                self.name_list[*x as usize].cmp(&self.name_list[*y as usize])
            });
            e.names.dedup();
        }
        out
    }

    /// How close file `f` sits to file `a`, lower first; `None` when `a` cannot see it.
    fn tier(&self, g: &Graph, a: u32, f: u32) -> Option<u8> {
        let (ai, fi) = (a as usize, f as usize);
        let fdir = parent(&self.entries[fi].path);
        if g.imp_files[ai].contains(&f) || g.imp_dirs[ai].contains(fdir) {
            Some(0)
        } else if fdir == parent(&self.entries[ai].path) {
            Some(1)
        } else if g.unit[ai] == g.unit[fi] {
            Some(2)
        } else if g.unit_deps[g.unit[ai] as usize].contains(&g.unit[fi]) {
            Some(3)
        } else {
            None
        }
    }

    /// Of `cands`, those nearest `a`. With `cap`, a name defined in more than
    /// [`MAX_DEF_FILES`] files resolves only through a direct import.
    fn nearest(&self, g: &Graph, a: u32, cands: Vec<(u32, u32)>, cap: bool) -> Vec<(u32, u32)> {
        let mut best = u8::MAX;
        let mut out = vec![];
        for &(f, d) in &cands {
            let Some(t) = self.tier(g, a, f) else {
                continue;
            };
            if t < best {
                best = t;
                out.clear();
            }
            if t == best {
                out.push((f, d));
            }
        }
        if cap && best > 0 && distinct_files(&cands) > MAX_DEF_FILES {
            out.clear();
        }
        out
    }

    /// File `a` imports a file that passes `name` on from file `f`: one that names it and imports
    /// `f`, or one that re-exports all of `f`, up to `depth` files deep.
    fn reexports(&self, g: &Graph, a: u32, f: u32, name: &str, depth: u32) -> bool {
        g.imp_files[a as usize].iter().any(|&r| {
            r != f
                && (g.star[r as usize].contains(&f)
                    || (self.mentions_name(r, name) && g.imp_files[r as usize].contains(&f))
                    || (depth > 1 && self.reexports(g, r, f, name, depth - 1)))
        })
    }

    /// File `a` mentions `name` somewhere.
    fn mentions_name(&self, a: u32, name: &str) -> bool {
        self.entries[a as usize]
            .names
            .binary_search_by(|&n| (*self.name_list[n as usize]).cmp(name))
            .is_ok()
    }

    /// The definitions mention `m` in file `a` most likely means, as (file, def) pairs.
    fn resolve(&self, g: &Graph, a: u32, m: Mention) -> Vec<(u32, u32)> {
        let Some(all) = self.defs_by_name.get(&m.name) else {
            return vec![];
        };
        let a_test = g.test[a as usize];
        let family = self.entries[a as usize].lang.family;
        let def = |f: u32, d: u32| &self.entries[f as usize].defs[d as usize];
        let own = |c: &[(u32, u32)]| -> Option<Vec<(u32, u32)>> {
            c.iter()
                .any(|&(f, _)| f == a)
                .then(|| c.iter().copied().filter(|&(f, _)| f == a).collect())
        };
        // A module name is a path segment; the import that spells it already links the file.
        let cands: Vec<(u32, u32)> = all
            .iter()
            .copied()
            .filter(|&(f, d)| {
                def(f, d).kind != SymbolKind::Module
                    && self.entries[f as usize].lang.family == family
                    && (a_test || !g.test[f as usize])
            })
            .collect();
        match m.qual {
            Qual::SelfLike => {
                return cands.into_iter().filter(|&(f, _)| f == a).collect();
            }
            Qual::Name(q) => {
                let q = &*self.name_list[q as usize];
                // `Thing::go`: the qualifier names the definition's type.
                let by_type: Vec<(u32, u32)> = cands
                    .iter()
                    .copied()
                    .filter(|&(f, d)| def(f, d).container.as_deref() == Some(q))
                    .collect();
                if !by_type.is_empty() {
                    return own(&by_type).unwrap_or_else(|| self.nearest(g, a, by_type, false));
                }
                // `paths::report`, `server.Start`: the qualifier names the defining module.
                let by_module: Vec<(u32, u32)> = cands
                    .iter()
                    .copied()
                    .filter(|&(f, d)| {
                        def(f, d).container.is_none()
                            && module_name(&self.entries[f as usize].path) == q
                    })
                    .collect();
                if !by_module.is_empty() {
                    return self.nearest(g, a, by_module, false);
                }
            }
            Qual::None => {}
        }
        if m.member {
            // A member of an unknown receiver means a definition whose type the file names, or a
            // top-level one of a module the file imports.
            let cands: Vec<(u32, u32)> = cands
                .into_iter()
                .filter(|&(f, d)| {
                    let dd = def(f, d);
                    member_kind(dd.kind)
                        && match dd.container.as_deref() {
                            Some(c) => self.mentions_name(a, c),
                            None => f == a || self.tier(g, a, f) == Some(0),
                        }
                })
                .collect();
            return own(&cands).unwrap_or_else(|| self.nearest(g, a, cands, true));
        }
        // A name the file defines itself, as anything, is most likely that definition or a local.
        if let Some(o) = own(&cands) {
            return o;
        }
        let cands: Vec<(u32, u32)> = cands
            .into_iter()
            .filter(|&(f, d)| bare_kind(def(f, d).kind))
            .collect();
        let name = &*self.name_list[m.name as usize];
        let e = &self.entries[a as usize];
        // `use other_crate::name` through a re-export: an import spelling the name lands in the
        // definition's package.
        let imported_by_name = |f: u32| {
            e.imports.iter().zip(&e.targets).any(|(i, t)| {
                let spelled = i.spec.rsplit([':', '.', '/']).next() == Some(name)
                    || i.alts.iter().any(|x| x == name);
                spelled
                    && t.as_ref().is_some_and(|t| {
                        let dir = if t.folder {
                            t.path.as_str()
                        } else {
                            parent(&t.path)
                        };
                        self.resolver.unit_of_dir(dir)
                            == g.units[g.unit[f as usize] as usize].as_str()
                    })
            })
        };
        // Modules that import explicitly see another file's names only through an import of it
        // or of an index module above it; a Go package sees its own folder.
        let max_tier = match family {
            Family::Js | Family::Python => 0,
            Family::Go => 1,
            _ => 3,
        };
        let reexported = |f: u32| self.reexports(g, a, f, name, 2);
        // Across packages a bare name is a type, a macro, or a top-level constant; a function
        // from another package comes qualified or imported by name.
        let cands: Vec<(u32, u32)> = cands
            .into_iter()
            .filter(|&(f, d)| {
                let Some(t) = self.tier(g, a, f) else {
                    return false;
                };
                if t > max_tier && !reexported(f) && !imported_by_name(f) {
                    return false;
                }
                exported_bare(def(f, d)) || imported_by_name(f) || t < 3
            })
            .collect();
        self.nearest(g, a, cands, true)
    }

    fn names_of(&self, ids: &[u32]) -> Vec<String> {
        ids.iter()
            .take(MAX_LINK_NAMES)
            .map(|&n| self.name_list[n as usize].to_string())
            .collect()
    }

    fn link_of(&self, e: &Edge) -> Link {
        Link {
            path: self.entries[e.other as usize].path.clone(),
            import_lines: {
                let mut v: Vec<u32> = e.imports.iter().map(|&(l, _)| l).collect();
                v.sort_unstable();
                v.dedup();
                v
            },
            module: !e.imports.is_empty() && !e.real_import(),
            names: e
                .names
                .iter()
                .map(|&n| self.name_list[n as usize].to_string())
                .collect(),
        }
    }

    /// The files `node` steps to in `dir`, with the edge between them.
    fn steps<'a>(&'a self, g: &'a Graph, node: u32, dir: Direction) -> Vec<(u32, &'a Edge)> {
        match dir {
            Direction::Dependencies => g.out[node as usize]
                .iter()
                .filter(|e| e.follow(g.hub[e.other as usize]))
                .map(|e| (e.other, e))
                .collect(),
            Direction::Dependents => g.inn[node as usize]
                .iter()
                .filter(|e| e.follow(g.hub[node as usize]))
                .map(|e| (e.other, e))
                .collect(),
        }
    }

    /// What `path` uses and what uses it, strongest link first.
    pub fn neighbors(&mut self, path: &str, limit: usize) -> Option<Neighbors> {
        self.ensure_graph();
        let g = self.graph_ref();
        let &id = self.by_path.get(path)?;
        let mut truncated = false;
        let mut side = |edges: &[Edge]| -> Vec<Link> {
            let mut v: Vec<&Edge> = edges.iter().collect();
            v.sort_by(|a, b| {
                b.weight().cmp(&a.weight()).then_with(|| {
                    self.entries[a.other as usize]
                        .path
                        .cmp(&self.entries[b.other as usize].path)
                })
            });
            if v.len() > limit {
                v.truncate(limit);
                truncated = true;
            }
            v.into_iter().map(|e| self.link_of(e)).collect()
        };
        let uses = side(&g.out[id as usize]);
        let used_by = side(&g.inn[id as usize]);
        Some(Neighbors {
            path: path.to_string(),
            unit: g.units[g.unit[id as usize] as usize].clone(),
            test: g.test[id as usize],
            uses,
            used_by,
            truncated,
        })
    }

    /// Files reachable from `seeds` within `depth` hops, nearest first.
    pub fn reach(&mut self, seeds: &[String], dir: Direction, depth: u32, limit: usize) -> Reach {
        self.ensure_graph();
        let g = self.graph_ref();
        let mut out = Reach {
            direction: dir,
            seeds: seeds.to_vec(),
            missing: vec![],
            reached: vec![],
            truncated: false,
        };
        let mut seen: HashSet<u32> = HashSet::new();
        let mut queue = VecDeque::new();
        for s in seeds {
            match self.by_path.get(s) {
                Some(&id) => {
                    if seen.insert(id) {
                        queue.push_back((id, 0u32));
                    }
                }
                None => out.missing.push(s.clone()),
            }
        }
        let mut found: Vec<(u32, u32, u32, &Edge)> = vec![];
        while let Some((node, d)) = queue.pop_front() {
            if d >= depth {
                continue;
            }
            for (next, e) in self.steps(g, node, dir) {
                if !seen.insert(next) {
                    continue;
                }
                if found.len() >= MAX_REACH {
                    out.truncated = true;
                    break;
                }
                found.push((next, d + 1, node, e));
                queue.push_back((next, d + 1));
            }
        }
        found.sort_by(|a, b| {
            (a.1, &self.entries[a.0 as usize].path).cmp(&(b.1, &self.entries[b.0 as usize].path))
        });
        if found.len() > limit {
            found.truncate(limit);
            out.truncated = true;
        }
        out.reached = found
            .into_iter()
            .map(|(f, d, via, e)| Reached {
                path: self.entries[f as usize].path.clone(),
                distance: d,
                via: self.entries[via as usize].path.clone(),
                names: self.names_of(&e.names),
            })
            .collect();
        out
    }

    /// The shortest chain by which `from` depends on `to`, both ends included.
    pub fn path_between(&mut self, from: &str, to: &str) -> Option<Vec<Hop>> {
        self.ensure_graph();
        let g = self.graph_ref();
        let &a = self.by_path.get(from)?;
        let &b = self.by_path.get(to)?;
        let mut prev: HashMap<u32, (u32, &Edge)> = HashMap::new();
        let mut queue = VecDeque::from([a]);
        let mut seen = HashSet::from([a]);
        while let Some(node) = queue.pop_front() {
            if node == b {
                break;
            }
            for (next, e) in self.steps(g, node, Direction::Dependencies) {
                if seen.insert(next) {
                    prev.insert(next, (node, e));
                    queue.push_back(next);
                }
            }
        }
        if a != b && !prev.contains_key(&b) {
            return None;
        }
        let mut hops = vec![Hop {
            path: to.to_string(),
            import_line: None,
            names: vec![],
        }];
        let mut cur = b;
        while let Some(&(p, e)) = prev.get(&cur) {
            hops.push(Hop {
                path: self.entries[p as usize].path.clone(),
                import_line: e.imports.iter().find(|i| !i.1).map(|i| i.0),
                names: self.names_of(&e.names),
            });
            cur = p;
        }
        hops.reverse();
        Some(hops)
    }

    fn undirected(&self, g: &Graph, id: u32) -> HashMap<u32, u32> {
        let mut m: HashMap<u32, u32> = HashMap::new();
        for e in g.out[id as usize].iter().chain(&g.inn[id as usize]) {
            *m.entry(e.other).or_default() += e.weight();
        }
        m
    }

    /// Files worth reading alongside `path`: its direct links by strength, then files one step
    /// further that share them.
    pub fn related(&mut self, path: &str, limit: usize) -> Option<Vec<Related>> {
        self.ensure_graph();
        let g = self.graph_ref();
        let &id = self.by_path.get(path)?;
        let direct = self.undirected(g, id);
        let mut score: HashMap<u32, (f32, u32, Option<u32>)> = direct
            .iter()
            .map(|(&f, &w)| (f, (w as f32, 1, None)))
            .collect();
        for (&c, &w1) in &direct {
            if g.hub[c as usize] {
                continue;
            }
            let second = self.undirected(g, c);
            if second.len() > MAX_BRIDGE_DEGREE {
                continue;
            }
            for (f, w2) in second {
                if f == id || direct.contains_key(&f) {
                    continue;
                }
                let add = w1.min(w2) as f32 * 0.25;
                let s = score.entry(f).or_insert((0.0, 2, Some(c)));
                if add > 0.0 && s.2.is_none_or(|v| direct[&v] < w1) {
                    s.2 = Some(c);
                }
                s.0 += add;
            }
        }
        let mut v: Vec<_> = score.into_iter().collect();
        v.sort_by(|a, b| {
            b.1.0.total_cmp(&a.1.0).then_with(|| {
                self.entries[a.0 as usize]
                    .path
                    .cmp(&self.entries[b.0 as usize].path)
            })
        });
        v.truncate(limit);
        Some(
            v.into_iter()
                .map(|(f, (s, d, via))| Related {
                    path: self.entries[f as usize].path.clone(),
                    score: (s * 100.0).round() / 100.0,
                    distance: d,
                    via: via.map(|c| self.entries[c as usize].path.clone()),
                })
                .collect(),
        )
    }

    /// The files the most other files depend on.
    pub fn hubs(&mut self, limit: usize) -> Vec<Hub> {
        self.ensure_graph();
        let g = self.graph_ref();
        let mut v: Vec<Hub> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.alive)
            .map(|(id, e)| Hub {
                path: e.path.clone(),
                unit: g.units[g.unit[id] as usize].clone(),
                dependents: self.steps(g, id as u32, Direction::Dependents).len(),
                dependencies: self.steps(g, id as u32, Direction::Dependencies).len(),
            })
            .filter(|h| h.dependents > 0)
            .collect();
        v.sort_by(|a, b| {
            b.dependents
                .cmp(&a.dependents)
                .then_with(|| a.path.cmp(&b.path))
        });
        v.truncate(limit);
        v
    }

    /// Packages, what each depends on, and loops between them.
    pub fn units(&mut self) -> UnitMap {
        self.ensure_graph();
        let g = self.graph_ref();
        let k = g.units.len();
        let mut files = vec![0usize; k];
        let mut tests = vec![0usize; k];
        let mut links: Vec<BTreeMap<u32, usize>> = vec![BTreeMap::new(); k];
        for (id, e) in self.entries.iter().enumerate() {
            if !e.alive {
                continue;
            }
            let u = g.unit[id];
            files[u as usize] += 1;
            tests[u as usize] += usize::from(g.test[id]);
            for (to, _) in self.steps(g, id as u32, Direction::Dependencies) {
                let tu = g.unit[to as usize];
                if tu != u {
                    *links[u as usize].entry(tu).or_default() += 1;
                }
            }
        }
        let adj: Vec<Vec<u32>> = links.iter().map(|m| m.keys().copied().collect()).collect();
        let mut used_by: Vec<Vec<String>> = vec![vec![]; k];
        for (u, m) in links.iter().enumerate() {
            for &t in m.keys() {
                used_by[t as usize].push(g.units[u].clone());
            }
        }
        let mut units: Vec<UnitNode> = (0..k)
            .filter(|&u| files[u] > 0)
            .map(|u| {
                let mut depends_on: Vec<UnitDep> = links[u]
                    .iter()
                    .map(|(&t, &n)| UnitDep {
                        path: g.units[t as usize].clone(),
                        links: n,
                    })
                    .collect();
                depends_on.sort_by(|a, b| b.links.cmp(&a.links).then_with(|| a.path.cmp(&b.path)));
                let mut ub = std::mem::take(&mut used_by[u]);
                ub.sort();
                UnitNode {
                    path: g.units[u].clone(),
                    files: files[u],
                    tests: tests[u],
                    depends_on,
                    used_by: ub,
                }
            })
            .collect();
        units.sort_by(|a, b| a.path.cmp(&b.path));
        let mut cycles: Vec<Vec<String>> = sccs(&adj)
            .into_iter()
            .filter(|c| c.len() > 1)
            .map(|c| {
                let mut v: Vec<String> = c.iter().map(|&u| g.units[u as usize].clone()).collect();
                v.sort();
                v
            })
            .collect();
        cycles.sort();
        UnitMap { units, cycles }
    }

    /// Files that import each other in a loop, largest loop first. Only imports count, not name
    /// references, and not `mod` declarations or imports of index modules.
    pub fn cycles(&mut self, limit: usize) -> Vec<Vec<String>> {
        self.ensure_graph();
        let g = self.graph_ref();
        let adj: Vec<Vec<u32>> = g
            .out
            .iter()
            .map(|edges| {
                edges
                    .iter()
                    .filter(|e| e.real_import() && !g.hub[e.other as usize])
                    .map(|e| e.other)
                    .collect()
            })
            .collect();
        let mut out: Vec<Vec<String>> = sccs(&adj)
            .into_iter()
            .filter(|c| c.len() > 1)
            .map(|c| {
                let mut v: Vec<String> = c
                    .iter()
                    .map(|&f| self.entries[f as usize].path.clone())
                    .collect();
                v.sort();
                v
            })
            .collect();
        out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        out.truncate(limit);
        out
    }

    /// The innermost definition of file `f` whose body holds `line`.
    fn enclosing(&self, f: u32, line: u32) -> Option<usize> {
        self.entries[f as usize]
            .defs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.line <= line && d.end_line.unwrap_or(d.line) >= line)
            .max_by_key(|(_, d)| (d.line, std::cmp::Reverse(d.end_line)))
            .map(|(i, _)| i)
    }

    /// The definitions that mention `symbol`, grouped with their lines. With `defined_in`, only
    /// mentions that can mean the definition in that file count.
    pub fn callers(&mut self, symbol: &str, defined_in: Option<&str>, limit: usize) -> Callers {
        self.ensure_graph();
        let u = self.usages(symbol, defined_in, MAX_CALLER_REFS);
        let g = self.graph_ref();
        let mut out = Callers {
            symbol: u.symbol.clone(),
            definitions: u.definitions,
            callers: vec![],
            truncated: u.truncated,
        };
        let target = defined_in.and_then(|p| self.by_path.get(p).copied());
        if let Some(p) = defined_in {
            out.definitions.retain(|d| d.path == p);
            if target.is_none() {
                return out;
            }
        }
        let mut by_file: BTreeMap<u32, Vec<(u32, u32)>> = BTreeMap::new();
        for r in &u.references {
            if let Some(&f) = self.by_path.get(&r.path) {
                by_file.entry(f).or_default().push((r.line, r.col));
            }
        }
        let mut groups: BTreeMap<(u32, Option<usize>), Vec<u32>> = BTreeMap::new();
        for (f, refs) in by_file {
            let refs = match target {
                Some(t) if f != t => self.refs_meaning(g, f, t, refs),
                _ => refs,
            };
            for (line, _) in refs {
                groups
                    .entry((f, self.enclosing(f, line)))
                    .or_default()
                    .push(line);
            }
        }
        for ((f, d), mut lines) in groups {
            if out.callers.len() >= limit {
                out.truncated = true;
                break;
            }
            lines.dedup();
            let e = &self.entries[f as usize];
            let def = d.map(|i| &e.defs[i]);
            let import =
                def.is_none() && lines.iter().all(|l| e.imports.iter().any(|i| i.line == *l));
            out.callers.push(Caller {
                path: e.path.clone(),
                name: def.map(|d| self.name_list[d.name as usize].to_string()),
                kind: def.map(|d| d.kind),
                container: def.and_then(|d| d.container.as_deref().map(str::to_string)),
                line: def.map(|d| d.line),
                lines,
                import,
            });
        }
        out
    }

    /// The mention at token `i`, with its names interned; `None` for a name the index lacks.
    fn mention_of(&self, src: &str, toks: &[lex::Tok], i: usize, lang: &Lang) -> Option<Mention> {
        let raw = mention_at(src, toks, i, lang);
        let &name = self.names.get(raw.name.as_str())?;
        let qual = match raw.qual {
            RawQual::None => Qual::None,
            RawQual::SelfLike => Qual::SelfLike,
            RawQual::Name(q) => self
                .names
                .get(q.as_str())
                .map_or(Qual::None, |&q| Qual::Name(q)),
        };
        Some(Mention {
            name,
            qual,
            member: raw.member,
        })
    }

    /// Of `refs` in file `f`, the (line, col) positions whose mention resolves into file `t`.
    fn refs_meaning(&self, g: &Graph, f: u32, t: u32, refs: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
        let e = &self.entries[f as usize];
        if !g.out[f as usize].iter().any(|x| x.other == t) {
            return vec![];
        }
        let Some(src) = read_text(&self.root().join(&e.path)) else {
            return vec![];
        };
        let toks = lex::lex(&src, e.lang);
        let at: HashMap<(u32, u32), usize> = toks
            .iter()
            .enumerate()
            .map(|(i, t)| ((t.line, t.col), i))
            .collect();
        refs.into_iter()
            .filter(|p| {
                at.get(p)
                    .and_then(|&i| self.mention_of(&src, &toks, i, e.lang))
                    .is_some_and(|m| self.resolve(g, f, m).iter().any(|&(df, _)| df == t))
            })
            .collect()
    }

    /// What a change to `symbol` (defined in `defined_in`) can break: the definitions that use
    /// it, then the definitions that use those, up to `depth` hops. Unlike [`Self::reach`] this
    /// follows the names, so a file that uses something else from the same file is left out.
    pub fn symbol_impact(
        &mut self,
        symbol: &str,
        defined_in: &str,
        depth: u32,
        limit: usize,
    ) -> SymbolImpact {
        let root = self.callers(symbol, Some(defined_in), 0);
        let mut out = SymbolImpact {
            symbol: root.symbol.clone(),
            definitions: root.definitions,
            hops: vec![],
            truncated: false,
        };
        if out.definitions.is_empty() {
            return out;
        }
        let mut seen: HashSet<(String, Option<String>)> =
            HashSet::from([(defined_in.to_string(), Some(out.symbol.clone()))]);
        let mut frontier = vec![(defined_in.to_string(), out.symbol.clone())];
        let mut total = 0;
        for _ in 0..depth {
            let mut hop = vec![];
            let mut next = vec![];
            for (file, name) in &frontier {
                let c = self.callers(name, Some(file), MAX_CALLER_REFS);
                out.truncated |= c.truncated;
                for x in c.callers {
                    // The import line only brings the name in; the uses are listed on their own.
                    if x.import || !seen.insert((x.path.clone(), x.name.clone())) {
                        continue;
                    }
                    if total >= limit {
                        out.truncated = true;
                        break;
                    }
                    total += 1;
                    if let Some(n) = &x.name {
                        next.push((x.path.clone(), n.clone()));
                    }
                    hop.push(SymbolUse {
                        path: x.path,
                        name: x.name,
                        kind: x.kind,
                        container: x.container,
                        lines: x.lines,
                        uses: name.clone(),
                    });
                }
            }
            if hop.is_empty() {
                break;
            }
            out.hops.push(hop);
            frontier = next;
        }
        out
    }

    /// The definitions that the body of `symbol` in `path` names. `line` picks among several
    /// definitions of the same name.
    pub fn callees(
        &mut self,
        path: &str,
        symbol: &str,
        line: Option<u32>,
        limit: usize,
    ) -> Option<Callees> {
        self.ensure_graph();
        let g = self.graph_ref();
        let &id = self.by_path.get(path)?;
        let &name = self.names.get(symbol)?;
        let e = &self.entries[id as usize];
        let (di, def) = e
            .defs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.name == name)
            .min_by_key(|(_, d)| line.map_or(0, |l| d.line.abs_diff(l)))?;
        let end = def.end_line.unwrap_or(def.line);
        let src = read_text(&self.root().join(path))?;
        let toks = lex::lex(&src, e.lang);
        let in_body = |t: &lex::Tok| t.line >= def.line && t.line <= end && name_like(t.kind);
        let body_names: HashSet<&str> = toks
            .iter()
            .filter(|t| in_body(t))
            .map(|t| t.text(&src))
            .collect();
        let mut seen = HashSet::new();
        let mut uses: Vec<(u32, u32)> = vec![];
        let mut truncated = false;
        for (i, t) in toks.iter().enumerate() {
            if !in_body(t) {
                continue;
            }
            let Some(m) = self.mention_of(&src, &toks, i, e.lang) else {
                continue;
            };
            if !seen.insert(m) {
                continue;
            }
            let mut defs = self.resolve(g, id, m);
            if m.member && m.qual != Qual::SelfLike {
                // Within one body, a member's owner type is named in that body.
                defs.retain(|&(f, d)| {
                    self.entries[f as usize].defs[d as usize]
                        .container
                        .as_deref()
                        .is_none_or(|c| body_names.contains(c))
                });
                if defs.len() > 1 {
                    defs.retain(|&(f, d)| {
                        self.entries[f as usize].defs[d as usize].kind != SymbolKind::Field
                    });
                }
            }
            for (f, d) in defs {
                if (f, d) == (id, di as u32) || uses.contains(&(f, d)) {
                    continue;
                }
                if uses.len() >= limit {
                    truncated = true;
                    break;
                }
                uses.push((f, d));
            }
        }
        let uses = uses
            .into_iter()
            .map(|(f, d)| self.location(f, &self.entries[f as usize].defs[d as usize]))
            .collect();
        Some(Callees {
            path: path.to_string(),
            symbol: symbol.to_string(),
            line: def.line,
            end_line: def.end_line,
            uses,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sccs_find_loops() {
        let adj = vec![vec![1], vec![2], vec![0, 3], vec![], vec![4]];
        let mut c: Vec<Vec<u32>> = sccs(&adj)
            .into_iter()
            .map(|mut c| {
                c.sort();
                c
            })
            .collect();
        c.sort();
        assert_eq!(c, vec![vec![0, 1, 2], vec![3], vec![4]]);
    }

    #[test]
    fn test_and_hub_paths() {
        assert!(is_test("crates/x/tests/runner.rs"));
        assert!(is_test("web/src/a.test.ts"));
        assert!(is_test("pkg/server_test.go"));
        assert!(!is_test("src/testing.rs"));
        assert!(is_hub("src/lib.rs"));
        assert!(is_hub("web/src/index.d.ts"));
        assert!(!is_hub("src/library.rs"));
    }
}
