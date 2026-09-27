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
use ostra_core::code::{
    CodeGraph, CodeGraphEdge, CodeGraphEdgeKind, CodeGraphMember, CodeGraphNode, CodeGraphNodeKind,
    CodeGraphSymbol, CodeGraphView, CodeLocation, SymbolKind,
};
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

/// One definition's implementation links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImplLinks {
    pub definition: CodeLocation,
    /// What it implements or extends; for a method, the supertype methods it implements.
    pub implements: Vec<CodeLocation>,
    /// What implements or extends it; for a method, the methods that implement it.
    pub implemented_by: Vec<CodeLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Implementations {
    pub symbol: String,
    /// How many definitions the name has in the scope asked about.
    pub definitions: usize,
    /// Definitions with at least one link, most linked first. When nothing links, the first
    /// definition with empty lists.
    pub links: Vec<ImplLinks>,
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
pub(crate) fn mention_at(src: &str, toks: &[lex::Tok], i: usize, lang: &Lang) -> RawMention {
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

pub(crate) fn member_kind(k: SymbolKind) -> bool {
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
    pub(crate) fn graph_ref(&self) -> &Graph {
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
    pub(crate) fn mentions_name(&self, a: u32, name: &str) -> bool {
        self.entries[a as usize]
            .names
            .binary_search_by(|&n| (*self.name_list[n as usize]).cmp(name))
            .is_ok()
    }

    /// The definitions mention `m` in file `a` most likely means, as (file, def) pairs.
    pub(crate) fn resolve(&self, g: &Graph, a: u32, m: Mention) -> Vec<(u32, u32)> {
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
        // `use other_crate::name` or `import { name } from "@scope/pkg"` through a re-export: an
        // import spelling the name lands in the definition's package.
        let imported_by_name = |f: u32| {
            e.imports.iter().zip(&e.targets).any(|(i, t)| {
                let spelled = i.spec.rsplit([':', '.', '/']).next() == Some(name)
                    || i.alts.iter().any(|x| x == name)
                    || i.names.iter().any(|x| x == name);
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
    pub(crate) fn mention_of(
        &self,
        src: &str,
        toks: &[lex::Tok],
        i: usize,
        lang: &Lang,
    ) -> Option<Mention> {
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
            // The definition's own name is not a use; on a bodiless interface method it would
            // resolve to the methods that implement it.
            if !in_body(t) || (t.line == def.line && t.col == def.col) {
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

/// Files a package view shows.
pub const MAX_VIEW_FILES: usize = 150;
/// Files a file view shows.
pub const MAX_VIEW_NEIGHBORS: usize = 80;

/// A column per node so that each node sits left of what it uses; nodes that use each other in a
/// loop share a column. Sinks are column 0 and users take negative columns.
fn columns(adj: &[Vec<u32>]) -> Vec<i32> {
    let comps = sccs(adj);
    let mut comp_of = vec![0usize; adj.len()];
    for (ci, c) in comps.iter().enumerate() {
        for &v in c {
            comp_of[v as usize] = ci;
        }
    }
    // Tarjan emits a component after every component it reaches, so those levels are known.
    let mut level = vec![0i32; comps.len()];
    for (ci, c) in comps.iter().enumerate() {
        let mut l = 0;
        for &v in c {
            for &w in &adj[v as usize] {
                let cw = comp_of[w as usize];
                if cw != ci {
                    l = l.max(level[cw] + 1);
                }
            }
        }
        level[ci] = l;
    }
    (0..adj.len()).map(|v| -level[comp_of[v]]).collect()
}

fn package_id(unit: &str) -> String {
    format!("package:{unit}")
}

fn package_label(unit: &str) -> String {
    if unit.is_empty() {
        "(project top)".into()
    } else {
        unit.to_string()
    }
}

impl ProjectIndex {
    fn file_node(&self, g: &Graph, f: u32, column: i32) -> CodeGraphNode {
        let path = &self.entries[f as usize].path;
        CodeGraphNode {
            id: path.clone(),
            kind: CodeGraphNodeKind::File,
            label: path.rsplit('/').next().unwrap_or(path).to_string(),
            package: g.units[g.unit[f as usize] as usize].clone(),
            column,
            files: 1,
            test: g.test[f as usize],
            dependents: self.steps(g, f, Direction::Dependents).len() as u32,
            dependencies: self.steps(g, f, Direction::Dependencies).len() as u32,
            symbol: None,
            file_defs: self.file_defs(f, None),
            more_file_defs: self.more_file_defs(f, None),
        }
    }

    fn file_edge(&self, from: u32, e: &Edge) -> CodeGraphEdge {
        CodeGraphEdge {
            from: self.entries[from as usize].path.clone(),
            to: self.entries[e.other as usize].path.clone(),
            weight: e.names.len().max(1) as u32,
            names: self.names_of(&e.names),
            import: e.real_import(),
            kind: CodeGraphEdgeKind::Uses,
        }
    }

    /// Every package and the links between them.
    pub fn packages_view(&mut self) -> CodeGraph {
        let m = self.units();
        let files = self.files() as u32;
        let index: HashMap<&str, u32> = m
            .units
            .iter()
            .enumerate()
            .map(|(i, u)| (u.path.as_str(), i as u32))
            .collect();
        let adj: Vec<Vec<u32>> = m
            .units
            .iter()
            .map(|u| {
                u.depends_on
                    .iter()
                    .filter_map(|d| index.get(d.path.as_str()).copied())
                    .collect()
            })
            .collect();
        let cols = columns(&adj);
        let nodes = m
            .units
            .iter()
            .zip(&cols)
            .map(|(u, &column)| CodeGraphNode {
                id: package_id(&u.path),
                kind: CodeGraphNodeKind::Package,
                label: package_label(&u.path),
                package: u.path.clone(),
                column,
                files: u.files as u32,
                test: false,
                dependents: u.used_by.len() as u32,
                dependencies: u.depends_on.len() as u32,
                symbol: None,
                file_defs: vec![],
                more_file_defs: 0,
            })
            .collect();
        let edges = m
            .units
            .iter()
            .flat_map(|u| {
                u.depends_on.iter().map(|d| CodeGraphEdge {
                    from: package_id(&u.path),
                    to: package_id(&d.path),
                    weight: d.links as u32,
                    names: vec![],
                    import: true,
                    kind: CodeGraphEdgeKind::Uses,
                })
            })
            .collect();
        CodeGraph {
            view: CodeGraphView::Packages,
            focus: String::new(),
            nodes,
            edges,
            indexed_files: files,
            truncated: false,
        }
    }

    /// One package's files with the links between them, and the other packages they use.
    /// `None` when no indexed file belongs to `unit`.
    pub fn package_view(&mut self, unit: &str) -> Option<CodeGraph> {
        self.ensure_graph();
        let g = self.graph_ref();
        let u = g.units.iter().position(|x| x == unit)? as u32;
        let mut members: Vec<u32> = (0..self.entries.len() as u32)
            .filter(|&f| self.entries[f as usize].alive && g.unit[f as usize] == u)
            .collect();
        if members.is_empty() {
            return None;
        }
        let truncated = members.len() > MAX_VIEW_FILES;
        if truncated {
            // Keep the files the rest of the package leans on most.
            members.sort_by_cached_key(|&f| {
                std::cmp::Reverse(self.steps(g, f, Direction::Dependents).len())
            });
            members.truncate(MAX_VIEW_FILES);
        }
        members.sort_by(|a, b| {
            self.entries[*a as usize]
                .path
                .cmp(&self.entries[*b as usize].path)
        });
        let slot: HashMap<u32, u32> = members
            .iter()
            .enumerate()
            .map(|(i, &f)| (f, i as u32))
            .collect();
        let mut outside: BTreeMap<u32, u32> = BTreeMap::new();
        let mut adj: Vec<Vec<u32>> = vec![vec![]; members.len()];
        let mut edges = vec![];
        let mut out_links: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        for (i, &f) in members.iter().enumerate() {
            for (to, e) in self.steps(g, f, Direction::Dependencies) {
                if let Some(&j) = slot.get(&to) {
                    adj[i].push(j);
                    edges.push(self.file_edge(f, e));
                } else if g.unit[to as usize] != u {
                    let tu = g.unit[to as usize];
                    let next = members.len() as u32 + outside.len() as u32;
                    outside.entry(tu).or_insert(next);
                    *out_links.entry((f, tu)).or_default() += 1;
                }
            }
        }
        adj.resize(members.len() + outside.len(), vec![]);
        for &(f, tu) in out_links.keys() {
            adj[slot[&f] as usize].push(outside[&tu]);
        }
        let cols = columns(&adj);
        let mut nodes: Vec<CodeGraphNode> = members
            .iter()
            .enumerate()
            .map(|(i, &f)| self.file_node(g, f, cols[i]))
            .collect();
        let m = self.units_of(g);
        for (&tu, &i) in &outside {
            let name = &g.units[tu as usize];
            let (files, dependents, dependencies) =
                m.get(name.as_str()).copied().unwrap_or_default();
            nodes.push(CodeGraphNode {
                id: package_id(name),
                kind: CodeGraphNodeKind::Package,
                label: package_label(name),
                package: name.clone(),
                column: cols[i as usize],
                files,
                test: false,
                dependents,
                dependencies,
                symbol: None,
                file_defs: vec![],
                more_file_defs: 0,
            });
        }
        for (&(f, tu), &n) in &out_links {
            edges.push(CodeGraphEdge {
                from: self.entries[f as usize].path.clone(),
                to: package_id(&g.units[tu as usize]),
                weight: n,
                names: vec![],
                import: true,
                kind: CodeGraphEdgeKind::Uses,
            });
        }
        Some(CodeGraph {
            view: CodeGraphView::Package,
            focus: unit.to_string(),
            nodes,
            edges,
            indexed_files: self.files() as u32,
            truncated,
        })
    }

    /// Files, dependents, and dependencies per package, for package nodes in a package view.
    fn units_of(&self, g: &Graph) -> HashMap<String, (u32, u32, u32)> {
        let mut files: HashMap<u32, u32> = HashMap::new();
        let mut uses: HashMap<u32, HashSet<u32>> = HashMap::new();
        let mut used: HashMap<u32, HashSet<u32>> = HashMap::new();
        for (id, e) in self.entries.iter().enumerate() {
            if !e.alive {
                continue;
            }
            let u = g.unit[id];
            *files.entry(u).or_default() += 1;
            for (to, _) in self.steps(g, id as u32, Direction::Dependencies) {
                let tu = g.unit[to as usize];
                if tu != u {
                    uses.entry(u).or_default().insert(tu);
                    used.entry(tu).or_default().insert(u);
                }
            }
        }
        files
            .into_iter()
            .map(|(u, n)| {
                (
                    g.units[u as usize].clone(),
                    (
                        n,
                        used.get(&u).map_or(0, |s| s.len() as u32),
                        uses.get(&u).map_or(0, |s| s.len() as u32),
                    ),
                )
            })
            .collect()
    }

    /// `path` and the files within `depth` hops: what uses it to the left, what it uses to the
    /// right, one column per hop. `None` when the index does not hold `path`.
    pub fn file_view(&mut self, path: &str, depth: u32) -> Option<CodeGraph> {
        self.ensure_graph();
        let g = self.graph_ref();
        let &focus = self.by_path.get(path)?;
        let mut column: HashMap<u32, i32> = HashMap::from([(focus, 0)]);
        let mut order = vec![focus];
        let mut truncated = false;
        for (dir, sign) in [(Direction::Dependencies, 1), (Direction::Dependents, -1)] {
            let mut frontier = vec![focus];
            for d in 1..=depth as i32 {
                let mut next = vec![];
                for &n in &frontier {
                    for (m, _) in self.steps(g, n, dir) {
                        if column.contains_key(&m) {
                            continue;
                        }
                        if order.len() >= MAX_VIEW_NEIGHBORS {
                            truncated = true;
                            break;
                        }
                        column.insert(m, sign * d);
                        order.push(m);
                        next.push(m);
                    }
                }
                frontier = next;
            }
        }
        let nodes = order
            .iter()
            .map(|&f| self.file_node(g, f, column[&f]))
            .collect();
        let mut edges = vec![];
        for &f in &order {
            for (to, e) in self.steps(g, f, Direction::Dependencies) {
                if column.contains_key(&to) {
                    edges.push(self.file_edge(f, e));
                }
            }
        }
        Some(CodeGraph {
            view: CodeGraphView::File,
            focus: path.to_string(),
            nodes,
            edges,
            indexed_files: self.files() as u32,
            truncated,
        })
    }
}

/// Callees a symbol view reads per definition.
const MAX_VIEW_CALLEES: usize = 200;
/// Members a type node lists.
const MAX_MEMBERS: usize = 40;
/// Top-level definitions a node lists from its file.
const MAX_FILE_DEFS: usize = 30;
/// Links a symbol view draws; implementations are kept first, then the heaviest references.
pub const MAX_VIEW_EDGES: usize = 300;
/// Stands for the top level of a file in a symbol view key.
const TOP: u32 = u32::MAX;

/// A definition in a symbol view: (file, index into its defs), or (file, [`TOP`]).
pub(crate) type DefKey = (u32, u32);

/// One link out of (or into) a definition in a symbol view.
struct CallStep {
    to: DefKey,
    /// Lines of the reference, 1 for an implementation.
    weight: u32,
    kind: CodeGraphEdgeKind,
}

/// Definitions a symbol view follows out of a body: what can be called or named as a type.
/// Fields, variables, and constants would crowd out the calls.
fn view_kind(k: SymbolKind) -> bool {
    matches!(
        k,
        SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Macro
            | SymbolKind::Class
            | SymbolKind::Enum
            | SymbolKind::Interface
            | SymbolKind::Type
    )
}

pub(crate) fn type_kind(k: SymbolKind) -> bool {
    matches!(
        k,
        SymbolKind::Class | SymbolKind::Enum | SymbolKind::Interface | SymbolKind::Type
    )
}

pub(crate) fn callable(k: SymbolKind) -> bool {
    matches!(
        k,
        SymbolKind::Function | SymbolKind::Method | SymbolKind::Macro
    )
}

/// A Go method's parameter and result types, with names and spaces dropped.
#[derive(Debug, PartialEq, Eq)]
struct GoSig {
    params: Vec<String>,
    results: Vec<String>,
}

/// Top-level comma-separated parts of `s`.
fn split_top(s: &str) -> Vec<&str> {
    let mut out = vec![];
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        out.push(last);
    }
    out
}

/// The types of a Go parameter or result list. Either every entry is named or none is, and
/// `a, b string` gives both names the type after the last.
fn go_types(list: &str) -> Vec<String> {
    // Without spaces or package qualifiers, since an import alias may rename the package.
    let squash = |s: &str| {
        let mut out = String::new();
        for c in s.chars().filter(|c| !c.is_whitespace()) {
            if c == '.' {
                while out.ends_with(|p: char| p.is_alphanumeric() || p == '_') {
                    out.pop();
                }
            } else {
                out.push(c);
            }
        }
        out
    };
    let parts: Vec<(Option<&str>, &str)> = split_top(list)
        .into_iter()
        .map(|p| match p.split_once(char::is_whitespace) {
            Some((first, tail))
                if first.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && !matches!(first, "chan" | "func" | "map" | "struct" | "interface") =>
            {
                (Some(first), tail)
            }
            _ => (None, p),
        })
        .collect();
    if !parts.iter().any(|(n, _)| n.is_some()) {
        return parts.into_iter().map(|(_, t)| squash(t)).collect();
    }
    let mut out = vec![String::new(); parts.len()];
    let mut ty = String::new();
    for (i, (name, t)) in parts.iter().enumerate().rev() {
        if name.is_some() {
            ty = squash(t);
        }
        out[i] = ty.clone();
    }
    out
}

/// The signature after `name` on a Go definition: `Save(tx *gorm.DB, id string) (_ *X, err
/// error)` gives params `["*gorm.DB", "string"]` and results `["*X", "error"]`. `None` when
/// the signature is cut off.
fn go_sig(line: &str, name: &str) -> Option<GoSig> {
    let at = line.find(&format!("{name}("))? + name.len();
    let rest = &line[at..];
    let close = |s: &str| {
        let mut depth = 0;
        s.char_indices().find_map(|(i, c)| {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
            None
        })
    };
    let end = close(rest)?;
    let tail = rest[end + 1..].trim();
    let results = match tail.strip_prefix('(') {
        Some(_) => go_types(&tail[1..close(tail)?]),
        None if tail.is_empty() => vec![],
        None => go_types(tail),
    };
    Some(GoSig {
        params: go_types(&rest[1..end]),
        results,
    })
}

fn bare(name: u32) -> Mention {
    Mention {
        name,
        qual: Qual::None,
        member: false,
    }
}

impl ProjectIndex {
    fn def_at(&self, f: u32, line: u32, name: &str) -> Option<u32> {
        let &n = self.names.get(name)?;
        self.entries[f as usize]
            .defs
            .iter()
            .position(|d| d.name == n && d.line == line)
            .map(|i| i as u32)
    }

    pub(crate) fn def_of(&self, (f, d): DefKey) -> &Def {
        &self.entries[f as usize].defs[d as usize]
    }

    /// The type definitions `name` can mean in file `a`.
    pub(crate) fn types_named(&self, g: &Graph, a: u32, name: u32) -> Vec<DefKey> {
        self.resolve(g, a, bare(name))
            .into_iter()
            .filter(|&k| type_kind(self.def_of(k).kind))
            .collect()
    }

    pub(crate) fn is_go(&self, f: u32) -> bool {
        self.entries[f as usize].lang.family == Family::Go
    }

    pub(crate) fn dir_of(&self, f: u32) -> &str {
        parent(&self.entries[f as usize].path)
    }

    /// The methods Go interface `t` declares: name and signature.
    fn go_method_set(&self, t: DefKey) -> Vec<(u32, Option<GoSig>)> {
        let name = &*self.name_list[self.def_of(t).name as usize];
        self.entries[t.0 as usize]
            .defs
            .iter()
            .filter(|x| x.kind == SymbolKind::Method && x.container.as_deref() == Some(name))
            .map(|x| (x.name, go_sig(&x.preview, &self.name_list[x.name as usize])))
            .collect()
    }

    /// Go type `s` has every method in `set` with a matching signature, declared in its package.
    fn go_satisfies(&self, s: DefKey, set: &[(u32, Option<GoSig>)]) -> bool {
        let owner = &*self.name_list[self.def_of(s).name as usize];
        let dir = self.dir_of(s.0);
        set.iter().all(|(m, want)| {
            self.defs_by_name.get(m).is_some_and(|v| {
                v.iter().any(|&(f, d)| {
                    let x = self.def_of((f, d));
                    callable(x.kind)
                        && x.container.as_deref() == Some(owner)
                        && self.dir_of(f) == dir
                        // A signature the definition line cuts off matches by name alone.
                        && match (want, go_sig(&x.preview, &self.name_list[*m as usize])) {
                            (Some(a), Some(b)) => *a == b,
                            _ => true,
                        }
                })
            })
        })
    }

    /// Go interfaces are satisfied implicitly, so a Go type implements every project
    /// interface whose method set its package defines. Empty interfaces are left out.
    fn go_interfaces_of(&self, s: DefKey) -> Vec<DefKey> {
        let mut out = vec![];
        for (fi, e) in self.entries.iter().enumerate() {
            if !e.alive || e.lang.family != Family::Go {
                continue;
            }
            for (di, x) in e.defs.iter().enumerate() {
                let i = (fi as u32, di as u32);
                if x.kind == SymbolKind::Interface && i != s {
                    let set = self.go_method_set(i);
                    if !set.is_empty() && self.go_satisfies(s, &set) {
                        out.push(i);
                    }
                }
            }
        }
        out
    }

    fn go_implementors_of(&self, t: DefKey) -> Vec<(DefKey, u32)> {
        let set = self.go_method_set(t);
        let Some(&(rare, _)) = set
            .iter()
            .min_by_key(|(m, _)| self.defs_by_name.get(m).map_or(0, Vec::len))
        else {
            return vec![];
        };
        let mut out: Vec<(DefKey, u32)> = vec![];
        for &(f, d) in self.defs_by_name.get(&rare).into_iter().flatten() {
            let x = self.def_of((f, d));
            let Some(owner) = x.container.as_deref().and_then(|c| self.names.get(c)) else {
                continue;
            };
            if !callable(x.kind) || (f, d) == t {
                continue;
            }
            let dir = self.dir_of(f);
            for &(sf, sd) in self.defs_by_name.get(owner).into_iter().flatten() {
                let s = (sf, sd);
                if s != t
                    && self.is_go(sf)
                    && self.dir_of(sf) == dir
                    && matches!(self.def_of(s).kind, SymbolKind::Class | SymbolKind::Type)
                    && !out.iter().any(|&(x, _)| x == s)
                    && self.go_satisfies(s, &set)
                {
                    out.push((s, sf));
                }
            }
        }
        out
    }

    /// What type `t` extends or implements, from every declaration that names it as the subtype
    /// (a Rust `impl Trait for T` may sit in any file).
    pub(crate) fn supertypes_of(&self, g: &Graph, t: DefKey) -> Vec<DefKey> {
        if self.is_go(t.0) {
            return self.go_interfaces_of(t);
        }
        let name = self.def_of(t).name;
        let mut out = vec![];
        for (rf, e) in self.entries.iter().enumerate() {
            let rf = rf as u32;
            for r in e.supers.iter().filter(|r| r.sub == name) {
                if !self.types_named(g, rf, name).contains(&t) {
                    continue;
                }
                for s in self.types_named(g, rf, r.sup) {
                    if s != t && !out.contains(&s) {
                        out.push(s);
                    }
                }
            }
        }
        out
    }

    /// The types that extend or implement `t`, each with the file whose declaration says so.
    fn subtypes_of(&self, g: &Graph, t: DefKey) -> Vec<(DefKey, u32)> {
        if self.is_go(t.0) {
            return if self.def_of(t).kind == SymbolKind::Interface {
                self.go_implementors_of(t)
            } else {
                vec![]
            };
        }
        let name = self.def_of(t).name;
        let mut out: Vec<(DefKey, u32)> = vec![];
        for (rf, e) in self.entries.iter().enumerate() {
            let rf = rf as u32;
            for r in e.supers.iter().filter(|r| r.sup == name) {
                if !self.types_named(g, rf, name).contains(&t) {
                    continue;
                }
                for s in self.types_named(g, rf, r.sub) {
                    if s != t && !out.iter().any(|&(x, f)| x == s && f == rf) {
                        out.push((s, rf));
                    }
                }
            }
        }
        out
    }

    /// The methods named `name` that type `owner` defines, in its own file or in `also`.
    fn methods_of(&self, owner: DefKey, name: u32, also: u32, via: Option<u32>) -> Vec<DefKey> {
        let owner_name = &*self.name_list[self.def_of(owner).name as usize];
        let via_name = via.map(|v| &*self.name_list[v as usize]);
        self.defs_by_name
            .get(&name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&(f, d)| {
                let x = self.def_of((f, d));
                // Go methods may sit in any file of the type's package.
                (f == owner.0
                    || f == also
                    || (self.is_go(f) && self.dir_of(f) == self.dir_of(owner.0)))
                    && callable(x.kind)
                    && x.container.as_deref() == Some(owner_name)
                    && via_name.is_none_or(|v| x.via.as_deref().is_none_or(|xv| xv == v))
            })
            .collect()
    }

    /// Implementation links of `m`: for a type, what it extends and what extends it; for a
    /// method, the supertype method it implements and the methods that implement it.
    pub(crate) fn impl_steps(&self, g: &Graph, m: DefKey, dir: Direction) -> Vec<DefKey> {
        let md = self.def_of(m);
        if type_kind(md.kind) {
            return match dir {
                Direction::Dependencies => self.supertypes_of(g, m),
                Direction::Dependents => {
                    self.subtypes_of(g, m).into_iter().map(|(s, _)| s).collect()
                }
            };
        }
        if !callable(md.kind) {
            return vec![];
        }
        let Some(&c) = md.container.as_deref().and_then(|c| self.names.get(c)) else {
            return vec![];
        };
        let via = md.via.as_deref().and_then(|v| self.names.get(v)).copied();
        let owners = self.types_named(g, m.0, c);
        let mut out = vec![];
        match dir {
            Direction::Dependencies => {
                let sups: Vec<DefKey> = match via {
                    Some(v) => self.types_named(g, m.0, v),
                    None => owners
                        .iter()
                        .flat_map(|&o| self.supertypes_of(g, o))
                        .collect(),
                };
                for s in sups {
                    out.extend(self.methods_of(s, md.name, s.0, None));
                }
            }
            Direction::Dependents => {
                for &o in &owners {
                    for (sub, rf) in self.subtypes_of(g, o) {
                        out.extend(self.methods_of(sub, md.name, rf, Some(self.def_of(o).name)));
                    }
                }
            }
        }
        out.retain(|&k| k != m);
        out.sort_unstable();
        out.dedup();
        out
    }

    /// One step of a symbol view from `(file, def)`: the definitions it names, or the definitions
    /// (or file top levels) that name it, plus its implementation links.
    fn call_steps(&mut self, (f, d): DefKey, dir: Direction) -> (Vec<CallStep>, bool) {
        if d == TOP {
            return (vec![], false);
        }
        let e = &self.entries[f as usize];
        let def = &e.defs[d as usize];
        let (path, name_id, line) = (e.path.clone(), def.name, def.line);
        let name = self.name_list[name_id as usize].to_string();
        let mut out = vec![];
        let mut truncated = false;
        match dir {
            Direction::Dependencies => {
                let is_type = type_kind(def.kind);
                if let Some(c) = self.callees(&path, &name, Some(line), MAX_VIEW_CALLEES) {
                    truncated = c.truncated;
                    for l in c.uses {
                        // A type's own methods are listed inside its node, not as its callees.
                        if !l.kind.is_some_and(view_kind)
                            || (is_type
                                && l.path == path
                                && l.container.as_deref() == Some(name.as_str()))
                        {
                            continue;
                        }
                        let Some(&tf) = self.by_path.get(&l.path) else {
                            continue;
                        };
                        if let Some(td) = self.def_at(tf, l.line, &l.name) {
                            out.push(CallStep {
                                to: (tf, td),
                                weight: 1,
                                kind: CodeGraphEdgeKind::Uses,
                            });
                        }
                    }
                }
            }
            Direction::Dependents => {
                let c = self.callers(&name, Some(&path), MAX_CALLER_REFS);
                truncated = c.truncated;
                for x in c.callers {
                    if x.import {
                        continue;
                    }
                    let Some(&cf) = self.by_path.get(&x.path) else {
                        continue;
                    };
                    // The `implements` clause shows as an implementation link, and an import
                    // only brings the name in.
                    let ce = &self.entries[cf as usize];
                    let decl: HashSet<u32> = ce
                        .supers
                        .iter()
                        .filter(|r| r.sup == name_id)
                        .map(|r| r.line)
                        .chain(ce.imports.iter().map(|i| i.line))
                        .collect();
                    let lines = x.lines.iter().filter(|l| !decl.contains(l)).count();
                    if lines == 0 {
                        continue;
                    }
                    let key = match (&x.name, x.line) {
                        (Some(n), Some(l)) => match self.def_at(cf, l, n) {
                            Some(cd) => (cf, cd),
                            None => continue,
                        },
                        _ => (cf, TOP),
                    };
                    out.push(CallStep {
                        to: key,
                        weight: lines as u32,
                        kind: CodeGraphEdgeKind::Uses,
                    });
                }
            }
        }
        self.ensure_graph();
        let g = self.graph_ref();
        for to in self.impl_steps(g, (f, d), dir) {
            out.push(CallStep {
                to,
                weight: 1,
                kind: CodeGraphEdgeKind::Implements,
            });
        }
        (out, truncated)
    }

    /// The methods and functions type `t` defines: in its own file, and in impl blocks of other
    /// files whose name for it means `t`.
    fn members_of(
        &self,
        g: &Graph,
        t: DefKey,
        by_container: &HashMap<u32, Vec<DefKey>>,
    ) -> Vec<DefKey> {
        let mut out: Vec<DefKey> = by_container
            .get(&self.def_of(t).name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&k| callable(self.def_of(k).kind))
            .filter(|&(f, _)| f == t.0 || self.types_named(g, f, self.def_of(t).name).contains(&t))
            .collect();
        out.sort_by_key(|&(f, d)| (f != t.0, f, self.def_of((f, d)).line));
        out
    }

    fn member(&self, k: DefKey) -> CodeGraphMember {
        let x = self.def_of(k);
        CodeGraphMember {
            path: self.entries[k.0 as usize].path.clone(),
            name: self.name_list[x.name as usize].to_string(),
            kind: x.kind,
            line: x.line,
            end_line: x.end_line,
            via: x.via.as_deref().map(str::to_string),
            signature: x.preview.to_string(),
        }
    }

    /// The top-level types and functions file `f` defines, except def `except`, in source order.
    fn top_level(&self, f: u32, except: Option<u32>) -> impl Iterator<Item = u32> + '_ {
        self.entries[f as usize]
            .defs
            .iter()
            .enumerate()
            .filter(move |&(i, x)| {
                Some(i as u32) != except
                    && x.container.is_none()
                    && (type_kind(x.kind) || callable(x.kind))
            })
            .map(|(i, _)| i as u32)
    }

    fn file_defs(&self, f: u32, except: Option<u32>) -> Vec<CodeGraphMember> {
        self.top_level(f, except)
            .take(MAX_FILE_DEFS)
            .map(|d| self.member((f, d)))
            .collect()
    }

    fn more_file_defs(&self, f: u32, except: Option<u32>) -> u32 {
        self.top_level(f, except)
            .count()
            .saturating_sub(MAX_FILE_DEFS) as u32
    }

    fn view_symbol(
        &self,
        g: &Graph,
        k: DefKey,
        by_container: Option<&HashMap<u32, Vec<DefKey>>>,
    ) -> CodeGraphSymbol {
        let e = &self.entries[k.0 as usize];
        let def = self.def_of(k);
        let mut members = vec![];
        let mut more_members = 0;
        if let Some(bc) = by_container.filter(|_| type_kind(def.kind)) {
            let all = self.members_of(g, k, bc);
            more_members = all.len().saturating_sub(MAX_MEMBERS) as u32;
            members = all
                .into_iter()
                .take(MAX_MEMBERS)
                .map(|m| self.member(m))
                .collect();
        }
        CodeGraphSymbol {
            path: e.path.clone(),
            name: self.name_list[def.name as usize].to_string(),
            kind: def.kind,
            line: def.line,
            end_line: def.end_line,
            container: def.container.as_deref().map(str::to_string),
            via: def.via.as_deref().map(str::to_string),
            signature: def.preview.to_string(),
            members,
            more_members,
        }
    }

    /// What the types or methods named `symbol` implement, and what implements them. `path` and
    /// `line` pick one definition; without them every type or method of that name is asked, and
    /// the ones with links come first, up to `groups`.
    pub fn implementations(
        &mut self,
        symbol: &str,
        path: Option<&str>,
        line: Option<u32>,
        groups: usize,
        limit: usize,
    ) -> Implementations {
        self.ensure_graph();
        let mut out = Implementations {
            symbol: symbol.to_string(),
            definitions: 0,
            links: vec![],
            truncated: false,
        };
        let Some(&n) = self.names.get(symbol) else {
            return out;
        };
        let mut defs: Vec<DefKey> = self
            .defs_by_name
            .get(&n)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&k| {
                let x = self.def_of(k);
                (type_kind(x.kind) || callable(x.kind))
                    && path.is_none_or(|p| self.entries[k.0 as usize].path == p)
            })
            .collect();
        if let Some(l) = line
            && let Some(&best) = defs
                .iter()
                .min_by_key(|&&k| self.def_of(k).line.abs_diff(l))
        {
            defs = vec![best];
        }
        out.definitions = defs.len();
        let g = self.graph_ref();
        let mut all: Vec<ImplLinks> = defs
            .iter()
            .map(|&k| {
                let locs = |v: Vec<DefKey>| -> Vec<CodeLocation> {
                    v.into_iter()
                        .map(|x| self.location(x.0, self.def_of(x)))
                        .collect()
                };
                ImplLinks {
                    definition: self.location(k.0, self.def_of(k)),
                    implements: locs(self.impl_steps(g, k, Direction::Dependencies)),
                    implemented_by: locs(self.impl_steps(g, k, Direction::Dependents)),
                }
            })
            .collect();
        let linked = |x: &ImplLinks| x.implements.len() + x.implemented_by.len();
        all.sort_by_key(|x| std::cmp::Reverse(linked(x)));
        if all.iter().any(|x| linked(x) > 0) {
            all.retain(|x| linked(x) > 0);
        } else {
            all.truncate(1);
        }
        if all.len() > groups {
            out.truncated = true;
            all.truncate(groups);
        }
        for x in &mut all {
            for v in [&mut x.implements, &mut x.implemented_by] {
                if v.len() > limit {
                    out.truncated = true;
                    v.truncate(limit);
                }
            }
        }
        out.links = all;
        out
    }

    /// `symbol` in `path` (the definition nearest `line`), what references or implements it to
    /// the left and what it calls, names, or implements to the right, one column per hop. A
    /// reference from the top level of a file shows as that file. `None` when the index does
    /// not hold the definition.
    pub fn symbol_view(
        &mut self,
        path: &str,
        symbol: &str,
        line: Option<u32>,
        depth: u32,
    ) -> Option<CodeGraph> {
        self.ensure_graph();
        let &f = self.by_path.get(path)?;
        let &n = self.names.get(symbol)?;
        let (d, _) = self.entries[f as usize]
            .defs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.name == n)
            .min_by_key(|(_, d)| line.map_or(0, |l| d.line.abs_diff(l)))?;
        let focus = (f, d as u32);
        let mut column: HashMap<DefKey, i32> = HashMap::from([(focus, 0)]);
        let mut order = vec![focus];
        let mut links: BTreeMap<(DefKey, DefKey), (u32, CodeGraphEdgeKind)> = BTreeMap::new();
        let mut truncated = false;
        // Callees stop at half the nodes so the callers keep room.
        for (dir, sign, cap) in [
            (Direction::Dependencies, 1, MAX_VIEW_NEIGHBORS / 2),
            (Direction::Dependents, -1, MAX_VIEW_NEIGHBORS),
        ] {
            let mut frontier = vec![focus];
            for hop in 1..=depth as i32 {
                let mut next = vec![];
                for &a in &frontier {
                    let (steps, cut) = self.call_steps(a, dir);
                    truncated |= cut;
                    // Implementations first, so a full view still shows them.
                    let mut steps = steps;
                    steps.sort_by_key(|s| s.kind != CodeGraphEdgeKind::Implements);
                    for s in steps {
                        let b = s.to;
                        if a == b {
                            continue;
                        }
                        if let std::collections::hash_map::Entry::Vacant(slot) = column.entry(b) {
                            if order.len() >= cap {
                                truncated = true;
                                continue;
                            }
                            slot.insert(sign * hop);
                            order.push(b);
                            next.push(b);
                        }
                        let k = if sign > 0 { (a, b) } else { (b, a) };
                        let slot = links.entry(k).or_insert((0, s.kind));
                        slot.0 = slot.0.max(s.weight);
                        if s.kind == CodeGraphEdgeKind::Implements {
                            slot.1 = s.kind;
                        }
                    }
                }
                frontier = next;
            }
        }
        if links.len() > MAX_VIEW_EDGES {
            truncated = true;
            let mut all: Vec<_> = std::mem::take(&mut links).into_iter().collect();
            all.sort_by_key(|&(_, (w, kind))| {
                (kind != CodeGraphEdgeKind::Implements, std::cmp::Reverse(w))
            });
            all.truncate(MAX_VIEW_EDGES);
            links = all.into_iter().collect();
        }
        self.ensure_graph();
        let g = self.graph_ref();
        let mut by_container: HashMap<u32, Vec<DefKey>> = HashMap::new();
        if order
            .iter()
            .any(|&(f, d)| d != TOP && type_kind(self.def_of((f, d)).kind))
        {
            for (fi, e) in self.entries.iter().enumerate() {
                if !e.alive {
                    continue;
                }
                for (di, x) in e.defs.iter().enumerate() {
                    if let Some(&c) = x.container.as_deref().and_then(|c| self.names.get(c)) {
                        by_container
                            .entry(c)
                            .or_default()
                            .push((fi as u32, di as u32));
                    }
                }
            }
        }
        let id = |(f, d): DefKey| {
            let e = &self.entries[f as usize];
            if d == TOP {
                e.path.clone()
            } else {
                let def = &e.defs[d as usize];
                format!(
                    "symbol:{}:{}:{}",
                    e.path, def.line, self.name_list[def.name as usize]
                )
            }
        };
        let mut ins: HashMap<DefKey, u32> = HashMap::new();
        let mut outs: HashMap<DefKey, u32> = HashMap::new();
        for &(a, b) in links.keys() {
            *outs.entry(a).or_default() += 1;
            *ins.entry(b).or_default() += 1;
        }
        let nodes = order
            .iter()
            .map(|&k| {
                let (f, d) = k;
                let e = &self.entries[f as usize];
                let file = e.path.rsplit('/').next().unwrap_or(&e.path).to_string();
                let symbol = (d != TOP).then(|| self.view_symbol(g, k, Some(&by_container)));
                let label = match &symbol {
                    None => file,
                    Some(s) => match (&s.container, e.lang.family) {
                        (Some(c), Family::Rust | Family::C) => format!("{c}::{}", s.name),
                        (Some(c), _) => format!("{c}.{}", s.name),
                        (None, _) => s.name.clone(),
                    },
                };
                CodeGraphNode {
                    id: id(k),
                    kind: if d == TOP {
                        CodeGraphNodeKind::File
                    } else {
                        CodeGraphNodeKind::Symbol
                    },
                    label,
                    package: g.units[g.unit[f as usize] as usize].clone(),
                    column: column[&k],
                    files: 1,
                    test: g.test[f as usize],
                    dependents: ins.get(&k).copied().unwrap_or(0),
                    dependencies: outs.get(&k).copied().unwrap_or(0),
                    symbol,
                    file_defs: self.file_defs(f, (d != TOP).then_some(d)),
                    more_file_defs: self.more_file_defs(f, (d != TOP).then_some(d)),
                }
            })
            .collect();
        let edges = links
            .iter()
            .map(|(&(a, b), &(w, kind))| CodeGraphEdge {
                from: id(a),
                to: id(b),
                weight: w,
                names: vec![],
                import: false,
                kind,
            })
            .collect();
        Some(CodeGraph {
            view: CodeGraphView::Symbol,
            focus: id(focus),
            nodes,
            edges,
            indexed_files: self.files() as u32,
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
