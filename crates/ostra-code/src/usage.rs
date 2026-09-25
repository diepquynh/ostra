//! Usages of one definition. The name at the asked position picks the definition, and a mention
//! elsewhere counts only when it can mean that definition. A member after `.`, `->`, or `::`
//! resolves through the type its receiver is declared with (`Repo repo`, `repo: Repo`,
//! `repo = new Repo()`) and then that type's supertypes, so `a.name` and `b.name` of two
//! unrelated classes stay apart. Receivers whose type the tokens do not tell fall back to the
//! graph's guess.

use crate::graph::{DefKey, Direction, Graph, RawQual, callable, mention_at, type_kind};
use crate::index::{ProjectIndex, empty_usages, read_text};
use crate::lang::Family;
use crate::lex::{self, Kind, Tok};
use ostra_core::code::{CodeUsages, SymbolKind};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Supertype hops a member lookup follows.
const MAX_SUPER_DEPTH: u32 = 8;

/// Types whose members are the members of their first type argument.
const WRAPPERS: &[&str] = &[
    "Box", "Rc", "Arc", "Option", "Result", "Optional", "Readonly", "Promise",
];

/// Library calls that hand back the value a wrapper holds: `repo.find(id).orElseThrow().name`.
const PASSTHROUGH: &[&str] = &[
    "unwrap",
    "expect",
    "unwrap_or",
    "unwrap_or_default",
    "unwrap_or_else",
    "get",
    "orElse",
    "orElseThrow",
    "orElseGet",
    "clone",
    "as_ref",
    "as_mut",
    "borrow",
    "borrow_mut",
];

/// Library calls that return their first argument: Mockito's `verify(repo).save(x)`.
const FIRST_ARG: &[&str] = &["verify", "spy", "requireNonNull"];

/// Calls a receiver chain follows back: `a.b().c().d()`.
const MAX_CHAIN: u32 = 6;

/// A name the file declares as a variable, parameter, or field.
struct Decl {
    line: u32,
    col: u32,
    /// The declared type's last path segment, when the tokens tell it. `[]` for an array.
    ty: Option<String>,
    /// The last call of the value it is set to, as a token of the file: `find` in
    /// `var user = repo.find(id)`. Its return type is the name's type.
    call: Option<usize>,
    /// A declaration rather than an assignment, so it hides a member of the same name.
    local: bool,
}

type Decls = HashMap<String, Vec<Decl>>;

/// Languages where a bare name inside a class body can mean a member of that class.
fn implicit_this(f: Family) -> bool {
    matches!(f, Family::Jvm | Family::C)
}

fn name_like(t: &Tok) -> bool {
    matches!(t.kind, Kind::Ident | Kind::Type)
}

fn upper(s: &str) -> bool {
    s.starts_with(|c: char| c.is_uppercase())
}

/// Scans the tokens of one file, comments left out.
struct Scan<'a> {
    src: &'a str,
    t: Vec<Tok>,
    /// Each token's index among all the file's tokens.
    idx: Vec<usize>,
    family: Family,
}

impl Scan<'_> {
    fn p(&self, i: usize, c: u8) -> bool {
        self.t.get(i).is_some_and(|t| t.kind == Kind::Punct(c))
    }

    fn text(&self, i: usize) -> &str {
        self.t.get(i).map_or("", |t| t.text(self.src))
    }

    fn named(&self, i: usize) -> bool {
        self.t.get(i).is_some_and(name_like)
    }

    fn kw(&self, i: usize, words: &[&str]) -> bool {
        self.t
            .get(i)
            .is_some_and(|t| t.kind == Kind::Keyword && words.contains(&t.text(self.src)))
    }

    /// A type name the tokens can mean: a builtin, a capitalized name, or any name in Go and C.
    fn typed(&self, i: usize) -> bool {
        self.t[i].kind == Kind::Type
            || upper(self.text(i))
            || matches!(self.family, Family::Go | Family::C)
    }

    /// The type written from token `i`: its last path segment, looking through references,
    /// pointers, and the wrappers in [`WRAPPERS`].
    fn type_at(&self, mut i: usize) -> Option<String> {
        while self.p(i, b'&')
            || self.p(i, b'*')
            || self.t.get(i).is_some_and(|t| t.kind == Kind::Lifetime)
            || self.kw(i, &["mut", "dyn", "impl", "const", "readonly", "final"])
        {
            i += 1;
        }
        if self.p(i, b'[') && self.p(i + 1, b']') {
            return Some("[]".into());
        }
        let mut last = None;
        while self.named(i) {
            last = Some(i);
            if self.p(i + 1, b':') && self.p(i + 2, b':') && self.named(i + 3) {
                i += 3;
            } else if self.p(i + 1, b'.') && self.named(i + 2) {
                i += 2;
            } else {
                break;
            }
        }
        let n = last?;
        if !self.typed(n) {
            return None;
        }
        let s = self.text(n);
        if WRAPPERS.contains(&s)
            && (self.p(n + 1, b'<') || self.p(n + 1, b'['))
            && let Some(inner) = self.type_at(n + 2)
        {
            return Some(inner);
        }
        Some(s.to_string())
    }

    /// The type a value starting at token `i` is constructed as: `new T(`, `T{`, `&T{`,
    /// `T::new(`, `T(` where a capitalized call constructs.
    fn init_type(&self, mut i: usize) -> Option<String> {
        let mut new = false;
        while self.kw(i, &["new", "await"]) || self.p(i, b'&') {
            new |= self.kw(i, &["new"]);
            i += 1;
        }
        let mut segs = vec![];
        let mut path = false;
        while self.named(i) {
            segs.push(i);
            if self.p(i + 1, b':') && self.p(i + 2, b':') && self.named(i + 3) {
                path = true;
                i += 3;
            } else if self.p(i + 1, b'.') && self.named(i + 2) {
                i += 2;
            } else {
                break;
            }
        }
        let &last = segs.last()?;
        let (call, brace) = (self.p(last + 1, b'('), self.p(last + 1, b'{'));
        let generic = self.p(last + 1, b'<');
        let s = self.text(last);
        let ok = match self.family {
            _ if new => call || generic || brace,
            Family::Go => brace,
            Family::Rust if path && matches!(s, "new" | "default") && call => {
                let prev = segs[segs.len() - 2];
                return upper(self.text(prev)).then(|| self.text(prev).to_string());
            }
            Family::Rust => (call || brace) && upper(s),
            Family::Python | Family::Jvm => call && upper(s),
            _ => false,
        };
        ok.then(|| s.to_string())
    }

    /// The `)` that closes the `(` at `open`.
    fn close(&self, open: usize) -> Option<usize> {
        let mut depth = 0u32;
        for j in open..self.t.len() {
            if self.p(j, b'(') {
                depth += 1;
            } else if self.p(j, b')') {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
        }
        None
    }

    /// The last call of a chain from token `i` that ends in a call: `find` in
    /// `repo.find(id)` and in `Repo.get().find(id)`, as an index among all tokens.
    fn last_call(&self, mut i: usize) -> Option<usize> {
        while self.kw(i, &["await"]) {
            i += 1;
        }
        loop {
            if !self.named(i) {
                return None;
            }
            let mut j = i + 1;
            let call = self.p(j, b'(');
            if call {
                j = self.close(j)? + 1;
            }
            while self.p(j, b'?') || self.p(j, b'!') {
                j += 1;
            }
            if self.p(j, b'.') {
                i = j + 1;
            } else if self.p(j, b':') && self.p(j + 1, b':') {
                i = j + 2;
            } else {
                return call.then(|| self.idx[i]);
            }
        }
    }

    /// The type and last call of the value that starts at token `v`.
    fn value(&self, v: usize) -> (Option<String>, Option<usize>) {
        match self.init_type(v) {
            Some(ty) => (Some(ty), None),
            None => (None, self.last_call(v)),
        }
    }

    fn assign_at(&self, i: usize) -> Option<usize> {
        if self.p(i, b'=') && !self.p(i + 1, b'=') && !self.p(i + 1, b'>') {
            Some(i + 1)
        } else if self.p(i, b':') && self.p(i + 1, b'=') {
            Some(i + 2)
        } else {
            None
        }
    }

    fn decl(&self, k: usize) -> Option<Decl> {
        let t = &self.t[k];
        if t.kind != Kind::Ident || matches!(self.text(k), "self" | "this" | "cls" | "super") {
            return None;
        }
        let prev = k.checked_sub(1);
        let after_dot = prev.is_some_and(|p| {
            self.p(p, b'.')
                || (self.p(p, b'>') && p.checked_sub(1).is_some_and(|q| self.p(q, b'-')))
        });
        let mk = |ty: Option<String>, local: bool| Decl {
            line: t.line,
            col: t.col,
            ty,
            call: None,
            local,
        };
        let init = |local: bool| {
            let (ty, call) = self.value(self.assign_at(k + 1)?);
            (ty.is_some() || call.is_some()).then(|| Decl {
                call,
                ..mk(ty, local)
            })
        };
        if after_dot {
            // `self.repo = Repo()`: an attribute the file assigns.
            return init(false);
        }
        match self.family {
            Family::Go => {
                // `repo *Repo` in parameters, receivers, fields, and `var`.
                let starts = prev.is_none_or(|p| {
                    self.p(p, b'(')
                        || self.p(p, b',')
                        || self.p(p, b'{')
                        || self.p(p, b';')
                        || self.kw(p, &["var"])
                        || self.t[p].line < t.line
                });
                if starts
                    && !self.p(k + 1, b'.')
                    && !self.p(k + 1, b'(')
                    && let Some(ty) = self.type_at(k + 1)
                {
                    return Some(mk(Some(ty), true));
                }
                init(true)
            }
            f => {
                // `repo: Repo`, but not `a ? b : c`, `case x:`, or a path.
                let colon = self.p(k + 1, b':')
                    && !self.p(k + 2, b':')
                    && prev.is_none_or(|p| {
                        !self.p(p, b'?') && !self.p(p, b':') && !self.kw(p, &["case", "default"])
                    });
                if colon
                    && f != Family::C
                    && let Some(ty) = self.type_at(k + 2)
                {
                    return Some(mk(Some(ty), true));
                }
                if matches!(f, Family::Jvm | Family::C)
                    && let Some(p) = prev
                {
                    let ends = self.assign_at(k + 1).is_some()
                        || b";,):[".iter().any(|&c| self.p(k + 1, c));
                    if ends {
                        if self.kw(p, &["var", "val", "auto", "let", "const", "dynamic"]) {
                            return Some(init(true).unwrap_or_else(|| mk(None, true)));
                        }
                        if let Some(ty) = self.type_before(p) {
                            return Some(mk(Some(ty), true));
                        }
                    }
                }
                init(false)
            }
        }
    }

    /// The type that ends at token `p` in a `Type name` declaration.
    fn type_before(&self, p: usize) -> Option<String> {
        if self.named(p) && self.typed(p) {
            return Some(self.text(p).to_string());
        }
        if self.p(p, b']') && p.checked_sub(1).is_some_and(|b| self.p(b, b'[')) {
            return Some("[]".into());
        }
        if self.family == Family::Jvm && self.p(p, b'>') {
            let mut depth = 0i32;
            for j in (0..=p).rev() {
                if self.p(j, b'>') {
                    depth += 1;
                } else if self.p(j, b'<') {
                    depth -= 1;
                    if depth == 0 {
                        return j
                            .checked_sub(1)
                            .filter(|&n| self.named(n) && self.typed(n))
                            .map(|n| self.text(n).to_string());
                    }
                }
            }
        }
        if self.family == Family::C && (self.p(p, b'*') || self.p(p, b'&')) {
            return p
                .checked_sub(1)
                .filter(|&n| self.named(n))
                .map(|n| self.text(n).to_string());
        }
        None
    }
}

fn scan<'a>(src: &'a str, toks: &[Tok], family: Family) -> Scan<'a> {
    let idx: Vec<usize> = (0..toks.len())
        .filter(|&i| toks[i].kind != Kind::Comment)
        .collect();
    Scan {
        src,
        t: idx.iter().map(|&i| toks[i]).collect(),
        idx,
        family,
    }
}

/// The name called just before the member at token `i`: `find` in `repo.find(id).name`.
fn call_before(toks: &[Tok], i: usize) -> Option<usize> {
    let prev = |from: usize| (0..from).rev().find(|&j| toks[j].kind != Kind::Comment);
    let punct = |j: usize, c: u8| toks[j].kind == Kind::Punct(c);
    let p = prev(i)?;
    let mut q = if punct(p, b'.') {
        prev(p)?
    } else if punct(p, b'>') && punct(prev(p)?, b'-') {
        prev(prev(p)?)?
    } else {
        return None;
    };
    while punct(q, b'?') || punct(q, b'!') {
        q = prev(q)?;
    }
    if !punct(q, b')') {
        return None;
    }
    let mut depth = 0u32;
    let mut j = q;
    loop {
        if punct(j, b')') {
            depth += 1;
        } else if punct(j, b'(') {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
        j = prev(j)?;
    }
    let c = prev(j)?;
    name_like(&toks[c]).then_some(c)
}

fn declarations(src: &str, toks: &[Tok], family: Family) -> Decls {
    let s = scan(src, toks, family);
    let mut out: Decls = HashMap::new();
    for k in 0..s.t.len() {
        if let Some(d) = s.decl(k) {
            out.entry(s.text(k).to_string()).or_default().push(d);
        }
    }
    out
}

/// What a mention can mean: the definitions, and whether the tokens settled it or the graph
/// guessed.
struct Meaning {
    defs: Vec<DefKey>,
    sure: bool,
}

/// One file a query reads.
#[derive(Clone, Copy)]
struct File<'a> {
    f: u32,
    src: &'a str,
    toks: &'a [Tok],
    decls: &'a Decls,
}

struct Ctx<'a> {
    g: &'a Graph,
    members: Mutex<HashMap<(DefKey, u32), Vec<DefKey>>>,
}

impl ProjectIndex {
    /// The innermost definition of file `f` whose lines hold `line`, among those `keep` accepts.
    fn innermost(&self, f: u32, line: u32, keep: impl Fn(SymbolKind) -> bool) -> Option<u32> {
        self.entries[f as usize]
            .defs
            .iter()
            .enumerate()
            .filter(|(_, d)| keep(d.kind) && d.line <= line && d.end_line.unwrap_or(d.line) >= line)
            .max_by_key(|(_, d)| (d.line, std::cmp::Reverse(d.end_line)))
            .map(|(i, _)| i as u32)
    }

    /// The types whose body holds `line` of file `f`: the enclosing type, or the type an
    /// enclosing method belongs to (a Rust `impl` or a Go receiver).
    fn owners_at(&self, g: &Graph, f: u32, line: u32) -> Vec<DefKey> {
        let Some(d) = self.innermost(f, line, |k| type_kind(k) || callable(k)) else {
            return vec![];
        };
        let def = self.def_of((f, d));
        if type_kind(def.kind) {
            return vec![(f, d)];
        }
        match def.container.as_deref().and_then(|c| self.names.get(c)) {
            Some(&c) => self.types_named(g, f, c),
            None => vec![],
        }
    }

    /// The members named `name` of type `owner`, looked up the way a compiler would: the type's
    /// own, else the nearest supertypes'.
    fn members(&self, cx: &Ctx<'_>, owner: DefKey, name: u32, depth: u32) -> Vec<DefKey> {
        if let Some(v) = cx.members.lock().get(&(owner, name)) {
            return v.clone();
        }
        let o = self.def_of(owner);
        let owner_name = &*self.name_list[o.name as usize];
        let family = self.entries[owner.0 as usize].lang.family;
        let mut out: Vec<DefKey> = self
            .defs_by_name
            .get(&name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&(f, d)| {
                let x = self.def_of((f, d));
                x.kind != SymbolKind::Module
                    && x.container.as_deref() == Some(owner_name)
                    && self.entries[f as usize].lang.family == family
                    && (f == owner.0
                        || (self.is_go(f) && self.dir_of(f) == self.dir_of(owner.0))
                        || self.types_named(cx.g, f, o.name).contains(&owner))
            })
            .collect();
        if out.is_empty() && depth < MAX_SUPER_DEPTH {
            for s in self.supertypes_of(cx.g, owner) {
                out.extend(self.members(cx, s, name, depth + 1));
            }
            out.sort_unstable();
            out.dedup();
        }
        cx.members.lock().insert((owner, name), out.clone());
        out
    }

    fn members_in(&self, cx: &Ctx<'_>, owners: &[DefKey], name: u32) -> Vec<DefKey> {
        let mut out: Vec<DefKey> = owners
            .iter()
            .flat_map(|&o| self.members(cx, o, name, 0))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The project types a file means by type name `ty`.
    fn types_for(&self, g: &Graph, f: u32, ty: &str) -> Vec<DefKey> {
        self.names
            .get(ty)
            .map_or(vec![], |&n| self.types_named(g, f, n))
    }

    /// What the mention at token `i` of file `f` means.
    fn meaning(&self, cx: &Ctx<'_>, file: &File<'_>, i: usize, depth: u32) -> Meaning {
        let File {
            f,
            src,
            toks,
            decls,
        } = *file;
        let sure = |defs| Meaning { defs, sure: true };
        let lang = self.entries[f as usize].lang;
        let raw = mention_at(src, toks, i, lang);
        let Some(&name) = self.names.get(raw.name.as_str()) else {
            return sure(vec![]);
        };
        let t = &toks[i];
        match &raw.qual {
            RawQual::SelfLike => {
                let o = self.owners_at(cx.g, f, t.line);
                if !o.is_empty() {
                    return sure(self.members_in(cx, &o, name));
                }
            }
            RawQual::Name(q) if q == "super" && raw.member => {
                let sups: Vec<DefKey> = self
                    .owners_at(cx.g, f, t.line)
                    .into_iter()
                    .flat_map(|o| self.supertypes_of(cx.g, o))
                    .collect();
                if !sups.is_empty() {
                    return sure(self.members_in(cx, &sups, name));
                }
            }
            RawQual::Name(q) => {
                if let Some(owners) = self.receiver(cx, file, i, Some(q), depth) {
                    return sure(self.members_in(cx, &owners, name));
                }
                if !decls.contains_key(q.as_str()) {
                    let owners = self.types_for(cx.g, f, q);
                    let found = self.members_in(cx, &owners, name);
                    if !found.is_empty() {
                        return sure(found);
                    }
                }
            }
            RawQual::None if !raw.member && implicit_this(lang.family) => {
                let local = decls.get(raw.name.as_str()).is_some_and(|ds| {
                    self.innermost(f, t.line, callable).is_some_and(|m| {
                        let m = self.def_of((f, m));
                        ds.iter().any(|d| {
                            d.local && d.line >= m.line && (d.line, d.col) <= (t.line, t.col)
                        })
                    })
                });
                if local {
                    return sure(vec![]);
                }
                let found = self.members_in(cx, &self.owners_at(cx.g, f, t.line), name);
                if !found.is_empty() {
                    return sure(found);
                }
            }
            RawQual::None if raw.member => {
                if let Some(owners) = self.receiver(cx, file, i, None, depth) {
                    return sure(self.members_in(cx, &owners, name));
                }
            }
            RawQual::None => {}
        }
        Meaning {
            defs: self
                .mention_of(src, toks, i, lang)
                .map_or(vec![], |m| self.resolve(cx.g, f, m)),
            sure: false,
        }
    }

    /// The project types the receiver of member token `i` can have, from the call before it or
    /// from how the file declares receiver name `q`. `None` when the tokens do not tell. A
    /// receiver of an array or of a declared type the project does not define gives an empty
    /// list, because it has none of the project's members.
    fn receiver(
        &self,
        cx: &Ctx<'_>,
        file: &File<'_>,
        i: usize,
        q: Option<&str>,
        depth: u32,
    ) -> Option<Vec<DefKey>> {
        let File { f, toks, decls, .. } = *file;
        if let Some(c) = call_before(toks, i) {
            return self.value_types(cx, file, c, depth);
        }
        let ds = decls.get(q?)?;
        let tys: Vec<&str> = ds.iter().filter_map(|d| d.ty.as_deref()).collect();
        if !tys.is_empty() {
            return Some(
                tys.iter()
                    .flat_map(|ty| self.types_for(cx.g, f, ty))
                    .collect(),
            );
        }
        let calls: Vec<usize> = ds.iter().filter_map(|d| d.call).collect();
        if calls.is_empty() {
            return None;
        }
        let owners: Option<Vec<Vec<DefKey>>> = calls
            .iter()
            .map(|&c| self.value_types(cx, file, c, depth))
            .collect();
        owners.map(|o| o.concat())
    }

    /// The project types the call at token `c` returns. `None` when the call or its return type
    /// is unknown; empty when it returns a type the project does not define.
    fn value_types(
        &self,
        cx: &Ctx<'_>,
        file: &File<'_>,
        c: usize,
        depth: u32,
    ) -> Option<Vec<DefKey>> {
        let File { f, src, toks, .. } = *file;
        if depth >= MAX_CHAIN {
            return None;
        }
        let before = (0..c).rev().find(|&j| toks[j].kind != Kind::Comment);
        if before.is_some_and(|j| toks[j].kind == Kind::Keyword && toks[j].text(src) == "new") {
            return Some(self.types_for(cx.g, f, toks[c].text(src)));
        }
        let m = self.meaning(cx, file, c, depth + 1);
        // An unqualified function the graph resolves to one definition is trusted too.
        let single_fn = m.defs.len() == 1 && {
            let d = self.def_of(m.defs[0]);
            d.kind == SymbolKind::Function && d.container.is_none()
        };
        if m.defs.is_empty() && FIRST_ARG.contains(&toks[c].text(src)) {
            let next =
                |from: usize| (from + 1..toks.len()).find(|&j| toks[j].kind != Kind::Comment);
            let punct =
                |j: Option<usize>, ch: u8| j.is_some_and(|j| toks[j].kind == Kind::Punct(ch));
            let open = next(c).filter(|&j| punct(Some(j), b'('))?;
            let mut a = next(open)?;
            if matches!(toks[a].text(src), "this" | "self") && punct(next(a), b'.') {
                a = next(next(a)?)?;
            }
            if !name_like(&toks[a]) || !(punct(next(a), b',') || punct(next(a), b')')) {
                return None;
            }
            let q = toks[a].text(src);
            return self.receiver(cx, file, a, Some(q), depth + 1);
        }
        if m.defs.is_empty() && m.sure && PASSTHROUGH.contains(&toks[c].text(src)) {
            let q = match mention_at(src, toks, c, self.entries[f as usize].lang).qual {
                RawQual::Name(q) => Some(q),
                _ => None,
            };
            return self.receiver(cx, file, c, q.as_deref(), depth + 1);
        }
        // A method the project does not define may still return a project type, as Spring's
        // `findById` does.
        if m.defs.is_empty() || !(m.sure || single_fn) {
            return None;
        }
        let mut owners = vec![];
        for d in m.defs {
            let def = self.def_of(d);
            if type_kind(def.kind) {
                owners.push(d);
                continue;
            }
            let ty = self.return_type(d)?;
            if matches!(ty.as_str(), "Self" | "this") {
                let c = def.container.as_deref()?;
                owners.extend(self.types_for(cx.g, d.0, c));
            } else {
                owners.extend(self.types_for(cx.g, d.0, &ty));
            }
        }
        Some(owners)
    }

    /// The return type callable `d` declares, from its signature line.
    fn return_type(&self, d: DefKey) -> Option<String> {
        let def = self.def_of(d);
        let lang = self.entries[d.0 as usize].lang;
        let toks = lex::lex(&def.preview, lang);
        let s = scan(&def.preview, &toks, lang.family);
        let name = &*self.name_list[def.name as usize];
        let n = (0..s.t.len()).find(|&i| s.t[i].kind == Kind::Ident && s.text(i) == name)?;
        let after = matches!(
            lang.family,
            Family::Rust | Family::Js | Family::Python | Family::Go
        ) || matches!(lang.id, "kotlin" | "scala" | "swift");
        let ty = if after {
            let mut o = n + 1;
            if s.p(o, b'<') {
                let mut depth = 0i32;
                while o < s.t.len() {
                    depth += i32::from(s.p(o, b'<')) - i32::from(s.p(o, b'>'));
                    o += 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            if !s.p(o, b'(') {
                return None;
            }
            let c = s.close(o)?;
            if lang.family == Family::Go {
                s.type_at(if s.p(c + 1, b'(') { c + 2 } else { c + 1 })
            } else if s.p(c + 1, b'-') && s.p(c + 2, b'>') {
                s.type_at(c + 3)
            } else if s.p(c + 1, b':') {
                s.type_at(c + 2)
            } else {
                None
            }
        } else {
            s.type_before(n.checked_sub(1)?)
        }?;
        // `<T> T get()`: a type parameter says nothing about the value.
        let open = (n..s.t.len()).find(|&i| s.p(i, b'(')).unwrap_or(s.t.len());
        let param = (1..open).any(|i| s.text(i) == ty && (s.p(i - 1, b'<') || s.p(i - 1, b',')));
        (!param).then_some(ty)
    }

    /// Where the definition that `symbol` at `line` and `col` of `path` means is defined and
    /// used. A position on a definition means that definition; on a use, what the use resolves
    /// to. When the index cannot tell, every definition and use of the name is listed.
    pub fn usages_at(
        &mut self,
        symbol: &str,
        path: &str,
        line: u32,
        col: Option<u32>,
        limit: usize,
    ) -> CodeUsages {
        let symbol = symbol.trim_end_matches('!');
        self.ensure_graph();
        let by_name = |ix: &Self| ix.usages(symbol, Some(path), limit);
        let (Some(&a), Some(&name)) = (self.by_path.get(path), self.names.get(symbol)) else {
            return by_name(self);
        };
        let e = &self.entries[a as usize];
        let Some(src) = read_text(&self.root().join(&e.path)) else {
            return by_name(self);
        };
        let toks = lex::lex(&src, e.lang);
        let at = toks
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.line == line
                    && matches!(t.kind, Kind::Ident | Kind::Type | Kind::Macro)
                    && t.text(&src).trim_end_matches('!') == symbol
            })
            .min_by_key(|(_, t)| {
                col.map_or(0, |c| {
                    if (t.col..=t.col + t.len).contains(&c) {
                        0
                    } else {
                        c.abs_diff(t.col) + 1
                    }
                })
            })
            .map(|(i, _)| i);
        let Some(i) = at else {
            return by_name(self);
        };
        let tok = toks[i];
        let cx = Ctx {
            g: self.graph_ref(),
            members: Mutex::new(HashMap::new()),
        };
        let targets: Vec<DefKey> = match e
            .defs
            .iter()
            .position(|d| d.name == name && d.line == tok.line && d.col == tok.col)
        {
            Some(d) => vec![(a, d as u32)],
            None => {
                let decls = declarations(&src, &toks, e.lang.family);
                let m = self.meaning(
                    &cx,
                    &File {
                        f: a,
                        src: &src,
                        toks: &toks,
                        decls: &decls,
                    },
                    i,
                    0,
                );
                if m.defs.is_empty() && !m.sure {
                    return by_name(self);
                }
                m.defs
            }
        };
        if targets.is_empty() {
            return empty_usages(symbol);
        }
        // A call through the interface method a target implements may reach the target.
        let mut shown = targets.clone();
        let mut up: Vec<DefKey> = targets
            .iter()
            .copied()
            .filter(|&k| callable(self.def_of(k).kind))
            .collect();
        for _ in 0..MAX_SUPER_DEPTH {
            up = up
                .into_iter()
                .flat_map(|k| self.impl_steps(cx.g, k, Direction::Dependencies))
                .filter(|k| !shown.contains(k))
                .collect();
            up.sort_unstable();
            up.dedup();
            if up.is_empty() {
                break;
            }
            shown.extend(&up);
        }
        let targets = shown;
        // A guess counts when the name means nothing else in the language, or when the file can
        // name the class that holds the target.
        let family = self.entries[targets[0].0 as usize].lang.family;
        let only_targets = self.defs_by_name[&name]
            .iter()
            .all(|k| targets.contains(k) || self.entries[k.0 as usize].lang.family != family);
        let decls_of = Mutex::new(HashMap::<u32, Arc<Decls>>::new());
        self.usages_of(
            symbol,
            name,
            targets.clone(),
            Some(path),
            limit,
            |f, src, toks, j| {
                let cached = decls_of.lock().get(&f).cloned();
                let decls = cached.unwrap_or_else(|| {
                    let d = Arc::new(declarations(
                        src,
                        toks,
                        self.entries[f as usize].lang.family,
                    ));
                    decls_of.lock().insert(f, d.clone());
                    d
                });
                let m = self.meaning(
                    &cx,
                    &File {
                        f,
                        src,
                        toks,
                        decls: &decls,
                    },
                    j,
                    0,
                );
                if m.sure {
                    return m.defs.iter().any(|d| targets.contains(d));
                }
                only_targets
                    || m.defs
                        .iter()
                        .any(|&d| targets.contains(&d) && self.sees_owner(&cx, f, d))
            },
        )
    }

    /// File `f` can name the type that holds definition `d`: it is that file, or it mentions
    /// the type, and the outer type too when the type is nested.
    fn sees_owner(&self, cx: &Ctx<'_>, f: u32, d: DefKey) -> bool {
        if f == d.0 {
            return true;
        }
        let Some(c) = self.def_of(d).container.as_deref() else {
            return true;
        };
        self.types_for(cx.g, d.0, c)
            .into_iter()
            .filter(|o| o.0 == d.0)
            .any(|o| {
                self.mentions_name(f, c)
                    && self
                        .def_of(o)
                        .container
                        .as_deref()
                        .is_none_or(|outer| self.mentions_name(f, outer))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang;

    fn types(path: &str, src: &str) -> Vec<(String, Option<String>, bool)> {
        let l = lang::for_path(path).unwrap();
        let toks = lex::lex(src, l);
        let mut v: Vec<_> = declarations(src, &toks, l.family)
            .into_iter()
            .flat_map(|(n, ds)| ds.into_iter().map(move |d| (n.clone(), d.ty, d.local)))
            .collect();
        v.sort();
        v
    }

    fn s(n: &str, t: Option<&str>, local: bool) -> (String, Option<String>, bool) {
        (n.into(), t.map(Into::into), local)
    }

    #[test]
    fn java_declarations() {
        let src = "class A {\n  private final Repo repo;\n  List<Item> items = new ArrayList<>();\n  void f(String name, Order[] orders) {\n    var x = new Cart();\n    for (Line l : lines) {}\n    this.cache = new Cache();\n    count = 1;\n    return a ? b : c;\n  }\n}\n";
        assert_eq!(
            types("A.java", src),
            vec![
                s("cache", Some("Cache"), false),
                s("items", Some("List"), true),
                s("l", Some("Line"), true),
                s("name", Some("String"), true),
                s("orders", Some("[]"), true),
                s("repo", Some("Repo"), true),
                s("x", Some("Cart"), true),
            ]
        );
    }

    #[test]
    fn colon_and_go_declarations() {
        assert_eq!(
            types(
                "a.ts",
                "constructor(private repo: Repo, n: number) { const c = new Cart(); f({ key: value }); }"
            ),
            vec![
                s("c", Some("Cart"), false),
                s("n", Some("number"), true),
                s("repo", Some("Repo"), true),
            ]
        );
        assert_eq!(
            types(
                "a.rs",
                "fn f(repo: &mut Arc<Repo>) { let c = Cart::new(); let d = Deck { n: 1 }; }"
            ),
            vec![
                s("c", Some("Cart"), false),
                s("d", Some("Deck"), false),
                s("repo", Some("Repo"), true),
            ]
        );
        assert_eq!(
            types(
                "a.go",
                "func (s *Server) Run(ctx context.Context, n int) {\n\tc := &cart{}\n\tx := y\n\tz.Do()\n}\n"
            ),
            vec![
                s("c", Some("cart"), true),
                s("ctx", Some("Context"), true),
                s("n", Some("int"), true),
                s("s", Some("Server"), true),
            ]
        );
    }
}
