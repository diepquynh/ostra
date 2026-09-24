//! One pass over a file's tokens: display classes, the definitions (outline), and the raw import
//! specs. Definitions come from keyword rules (`fn`, `class`, `def`), member rules inside class
//! bodies, and a few family forms (C function bodies, Go receivers, Python indentation). Names
//! are matched by text; there is no type information.

use crate::lang::{Def, Family, Lang};
use crate::lex::{Kind, Tok};
use ostra_core::code::{SymbolKind, TokenClass};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: u32,
    pub col: u32,
    pub len: u32,
    pub end_line: Option<u32>,
    pub container: Option<String>,
    /// Byte offset of the name.
    pub byte: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStyle {
    /// A path of names: Rust `use`, Python modules, Java and C# imports, PHP `use`.
    Use,
    /// Rust `mod name;`.
    Mod,
    /// A string module spec: JS, Go, Lua.
    Module,
    Include {
        quoted: bool,
    },
    Require {
        relative: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImport {
    pub spec: String,
    pub line: u32,
    pub style: ImportStyle,
    /// More specific specs to try first (`from a import b` tries `a.b`).
    pub alts: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Analysis {
    /// One per token; `None` for punctuation and plain words of data files.
    pub classes: Vec<Option<TokenClass>>,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<RawImport>,
}

struct Scope {
    /// Brace depth inside this scope.
    depth: i32,
    sym: Option<usize>,
    name: Option<String>,
    class_like: bool,
    kind: Option<SymbolKind>,
}

struct Pending {
    sym: Option<usize>,
    name: Option<String>,
    class_like: bool,
    kind: Option<SymbolKind>,
    depth: i32,
    paren: i32,
}

struct PyScope {
    indent: u32,
    sym: usize,
    name: String,
    class: bool,
}

/// A Go `const (`, `var (`, or `type (` block.
struct Block {
    def: Def,
    paren: i32,
}

const NOT_BEFORE_MEMBER: &[&str] = &[
    "new",
    "return",
    "await",
    "throw",
    "typeof",
    "case",
    "yield",
    "else",
    "in",
    "of",
    "is",
    "as",
    "extends",
    "implements",
    "delete",
    "void",
];

struct A<'a> {
    src: &'a str,
    lang: &'a Lang,
    toks: &'a [Tok],
    /// Indices of tokens that are neither comments nor string continuations.
    sig: Vec<usize>,
    def_kind: Vec<Option<SymbolKind>>,
    consumed: Vec<bool>,
    out: Analysis,
    depth: i32,
    paren: i32,
    scopes: Vec<Scope>,
    pending: Option<Pending>,
    py: Vec<PyScope>,
    block: Option<Block>,
    in_init: bool,
    line_indent: u32,
}

pub fn analyze(src: &str, lang: &Lang, toks: &[Tok]) -> Analysis {
    let sig: Vec<usize> = toks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind != Kind::Comment && !t.cont)
        .map(|(i, _)| i)
        .collect();
    let mut a = A {
        src,
        lang,
        toks,
        consumed: vec![false; sig.len()],
        sig,
        def_kind: vec![None; toks.len()],
        out: Analysis::default(),
        depth: 0,
        paren: 0,
        scopes: vec![],
        pending: None,
        py: vec![],
        block: None,
        in_init: false,
        line_indent: 0,
    };
    if lang.family != Family::Data {
        a.run();
    }
    a.classify();
    a.out
}

fn all_caps(s: &str) -> bool {
    s.len() > 1
        && s.bytes().any(|c| c.is_ascii_uppercase())
        && !s.bytes().any(|c| c.is_ascii_lowercase())
}

fn class_of(kind: SymbolKind) -> TokenClass {
    match kind {
        SymbolKind::Function | SymbolKind::Method => TokenClass::Function,
        SymbolKind::Class | SymbolKind::Enum | SymbolKind::Interface | SymbolKind::Type => {
            TokenClass::Type
        }
        SymbolKind::Constant => TokenClass::Constant,
        SymbolKind::Macro => TokenClass::Macro,
        SymbolKind::Module | SymbolKind::Variable | SymbolKind::Field => TokenClass::Name,
    }
}

fn sym_kind(def: Def, member: bool) -> SymbolKind {
    match def {
        Def::Function if member => SymbolKind::Method,
        Def::Function => SymbolKind::Function,
        Def::Class | Def::Impl => SymbolKind::Class,
        Def::Enum => SymbolKind::Enum,
        Def::Interface => SymbolKind::Interface,
        Def::Type => SymbolKind::Type,
        Def::Module => SymbolKind::Module,
        Def::Constant => SymbolKind::Constant,
        Def::Variable if member => SymbolKind::Field,
        Def::Variable => SymbolKind::Variable,
        Def::Macro => SymbolKind::Macro,
    }
}

/// A string token's text without prefix letters and quotes.
pub fn unquote(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_ascii_alphabetic())
        .trim_matches(|c| matches!(c, '"' | '\'' | '`' | '<' | '>'))
}

impl<'a> A<'a> {
    fn t(&self, k: usize) -> Option<&'a Tok> {
        self.sig.get(k).map(|&i| &self.toks[i])
    }

    fn txt(&self, k: usize) -> &'a str {
        self.t(k).map_or("", |t| t.text(self.src))
    }

    fn kind(&self, k: usize) -> Option<Kind> {
        self.t(k).map(|t| t.kind)
    }

    fn is_p(&self, k: usize, c: u8) -> bool {
        self.kind(k) == Some(Kind::Punct(c))
    }

    fn is_name(&self, k: usize) -> bool {
        matches!(self.kind(k), Some(Kind::Ident | Kind::Type))
    }

    fn line(&self, k: usize) -> u32 {
        self.t(k).map_or(0, |t| t.line)
    }

    fn new_line(&self, k: usize) -> bool {
        k == 0 || self.line(k - 1) != self.line(k)
    }

    /// The previous significant token, skipping attributes.
    fn prev(&self, k: usize) -> Option<usize> {
        (0..k).rev().find(|&j| self.kind(j) != Some(Kind::Attr))
    }

    /// The token closing the bracket opened at `k`.
    fn matching(&self, k: usize) -> Option<usize> {
        let (open, close) = match self.kind(k)? {
            Kind::Punct(b'(') => (b'(', b')'),
            Kind::Punct(b'[') => (b'[', b']'),
            Kind::Punct(b'{') => (b'{', b'}'),
            Kind::Punct(b'<') => (b'<', b'>'),
            _ => return None,
        };
        let mut d = 0;
        for j in k..self.sig.len().min(k + 2000) {
            if self.is_p(j, open) {
                d += 1;
            } else if self.is_p(j, close) && !(close == b'>' && self.is_p(j - 1, b'-')) {
                d -= 1;
                if d == 0 {
                    return Some(j);
                }
            }
        }
        None
    }

    fn container(&self) -> Option<String> {
        if self.lang.family == Family::Python {
            return self.py.last().map(|p| p.name.clone());
        }
        self.scopes.iter().rev().find_map(|s| s.name.clone())
    }

    fn class_body(&self) -> Option<&Scope> {
        self.scopes
            .last()
            .filter(|s| s.class_like && s.depth == self.depth && self.paren == 0)
    }

    fn is_member(&self) -> bool {
        match self.lang.family {
            Family::Python => self.py.last().is_some_and(|p| p.class),
            _ => self.class_body().is_some(),
        }
    }

    fn add(&mut self, k: usize, kind: SymbolKind, container: Option<String>) -> usize {
        let t = self.t(k).expect("a name token");
        self.consumed[k] = true;
        self.def_kind[self.sig[k]] = Some(kind);
        self.out.symbols.push(Symbol {
            name: t.text(self.src).trim_end_matches('!').to_string(),
            kind,
            line: t.line,
            col: t.col,
            len: t.len,
            end_line: None,
            container,
            byte: t.start,
        });
        self.out.symbols.len() - 1
    }

    /// Record a definition and expect its body.
    fn define(&mut self, k: usize, kind: SymbolKind, container: Option<String>) {
        let name = self.txt(k).to_string();
        let idx = self.add(k, kind, container);
        let class_like = matches!(
            kind,
            SymbolKind::Class | SymbolKind::Enum | SymbolKind::Interface
        );
        match self.lang.family {
            Family::Python => self.py.push(PyScope {
                indent: self.line_indent,
                sym: idx,
                name,
                class: class_like,
            }),
            Family::Ruby | Family::Lua => {}
            _ => {
                self.pending = Some(Pending {
                    sym: Some(idx),
                    name: (class_like || kind == SymbolKind::Module).then_some(name),
                    class_like,
                    kind: Some(kind),
                    depth: self.depth,
                    paren: self.paren,
                })
            }
        }
    }

    fn run(&mut self) {
        let py = self.lang.family == Family::Python;
        let asi = matches!(self.lang.family, Family::Js | Family::Go)
            || matches!(self.lang.id, "kotlin" | "swift" | "scala");
        for k in 0..self.sig.len() {
            let t = self.t(k).expect("in range");
            if self.new_line(k) && self.paren == 0 {
                self.line_indent = t.col;
                if py {
                    while self.py.last().is_some_and(|p| p.indent >= t.col) {
                        let p = self.py.pop().expect("checked");
                        // The token before, counting the later lines of a multi-line string.
                        self.out.symbols[p.sym].end_line = Some(self.toks[self.sig[k] - 1].line);
                    }
                }
                if asi {
                    self.in_init = false;
                }
            }
            match t.kind {
                Kind::Punct(b'{') if !py => {
                    self.depth += 1;
                    let scope = match self.pending.take() {
                        Some(p) if p.depth == self.depth - 1 && p.paren == self.paren => Scope {
                            depth: self.depth,
                            sym: p.sym,
                            name: p.name,
                            class_like: p.class_like,
                            kind: p.kind,
                        },
                        other => {
                            self.pending = other;
                            Scope {
                                depth: self.depth,
                                sym: None,
                                name: None,
                                class_like: false,
                                kind: None,
                            }
                        }
                    };
                    self.scopes.push(scope);
                }
                Kind::Punct(b'}') if !py => {
                    if let Some(s) = self.scopes.pop()
                        && let Some(i) = s.sym
                    {
                        self.out.symbols[i].end_line = Some(t.line);
                    }
                    self.depth = (self.depth - 1).max(0);
                    if self.class_body().is_some() && !asi {
                        self.in_init = false;
                    }
                }
                Kind::Punct(b'(' | b'[') => self.paren += 1,
                Kind::Punct(b'{') => self.paren += 1,
                Kind::Punct(b')' | b']') | Kind::Punct(b'}') => {
                    self.paren = (self.paren - 1).max(0);
                    if self.block.as_ref().is_some_and(|b| self.paren < b.paren) {
                        self.block = None;
                    }
                }
                Kind::Punct(b';') => {
                    if self
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.depth == self.depth && p.paren == self.paren)
                    {
                        self.pending = None;
                    }
                    if self.class_body().is_some() {
                        self.in_init = false;
                    }
                }
                Kind::Punct(b',') if self.class_body().is_some() => self.in_init = false,
                Kind::Punct(b'=')
                    if self.class_body().is_some()
                        && !self.is_p(k + 1, b'=')
                        && !self.is_p(k + 1, b'>')
                        && !k.checked_sub(1).and_then(|j| self.t(j)).is_some_and(|p| {
                            matches!(p.kind, Kind::Punct(b'=' | b'!' | b'<' | b'>'))
                        }) =>
                {
                    self.in_init = true
                }
                Kind::Keyword => {
                    self.keyword(k);
                    self.imports(k);
                }
                Kind::Macro if t.text(self.src) == "macro_rules!" && self.is_name(k + 1) => {
                    let c = self.container();
                    self.add(k + 1, SymbolKind::Macro, c);
                }
                Kind::Ident | Kind::Type if !self.consumed[k] => {
                    self.name(k);
                    self.imports(k);
                }
                _ => {}
            }
        }
        let last = self.toks.last().map_or(1, |t| t.line);
        for p in self.py.drain(..) {
            self.out.symbols[p.sym].end_line = Some(last);
        }
        self.out.symbols.sort_by_key(|s| (s.line, s.col));
    }

    fn keyword(&mut self, k: usize) {
        let text = self.txt(k);
        let fam = self.lang.family;
        if fam == Family::C && text.starts_with('#') {
            if text.ends_with("define")
                && matches!(
                    self.kind(k + 1),
                    Some(Kind::Ident | Kind::Type | Kind::Constant)
                )
            {
                self.consumed[k + 1] = true;
                let c = self.container();
                self.add(k + 1, SymbolKind::Macro, c);
            }
            return;
        }
        let Some(def) = self.lang.def(text) else {
            return;
        };
        let member = self.is_member();
        match (fam, def) {
            (_, Def::Impl) => self.impl_block(k),
            (Family::Go, Def::Function) => self.go_func(k),
            (Family::Go, Def::Type | Def::Constant | Def::Variable) => {
                if self.is_p(k + 1, b'(') {
                    self.block = Some(Block {
                        def,
                        paren: self.paren + 1,
                    });
                } else if self.depth == 0 && self.is_name(k + 1) {
                    self.go_def(k + 1, def);
                }
            }
            (Family::Js, Def::Constant | Def::Variable) => self.js_var(k, def),
            (Family::Js, Def::Type) => {
                if self.is_name(k + 1) && (self.is_p(k + 2, b'=') || self.is_p(k + 2, b'<')) {
                    let c = self.container();
                    self.define(k + 1, SymbolKind::Type, c);
                }
            }
            (Family::Rust, Def::Constant) => {
                let mut j = k + 1;
                if matches!(self.txt(j), "fn" | "unsafe" | "async" | "extern") {
                    return;
                }
                if self.txt(j) == "mut" {
                    j += 1;
                }
                if self.is_name(j) && self.txt(j) != "_" {
                    let c = self.container();
                    self.add(j, SymbolKind::Constant, c);
                }
            }
            (Family::Jvm, Def::Constant | Def::Variable) => {
                if self.paren == 0 && (member || self.depth == 0) && self.is_name(k + 1) {
                    let c = self.container();
                    let kind = match (member, text) {
                        (true, "const") => SymbolKind::Constant,
                        (true, _) => SymbolKind::Field,
                        _ => sym_kind(def, false),
                    };
                    self.add(k + 1, kind, c);
                }
            }
            _ => self.generic(k, def, member),
        }
    }

    fn generic(&mut self, k: usize, def: Def, member: bool) {
        let fam = self.lang.family;
        let mut j = k + 1;
        // C++ `enum class Color`, Kotlin `fun <T> name`, JS `function* gen`.
        if matches!(self.txt(j), "class" | "struct") && fam == Family::C {
            j += 1;
        }
        if self.is_p(j, b'<') {
            j = self.matching(j).map_or(j, |m| m + 1);
        }
        if self.is_p(j, b'*') {
            j += 1;
        }
        if fam == Family::Ruby && self.txt(j) == "self" && self.is_p(j + 1, b'.') {
            j += 2;
        }
        if !self.is_name(j) {
            return;
        }
        let mut container = self.container();
        if def == Def::Function {
            while (self.is_p(j + 1, b'.') || (fam == Family::Lua && self.is_p(j + 1, b':')))
                && self.is_name(j + 2)
            {
                container = Some(self.txt(j).to_string());
                j += 2;
            }
        }
        if fam == Family::C
            && !(self.is_p(j + 1, b'{') || self.is_p(j + 1, b':') || self.txt(j + 1) == "final")
        {
            return;
        }
        self.define(j, sym_kind(def, member), container);
    }

    /// Rust `impl` and Swift `extension`: the implemented type becomes the container.
    fn impl_block(&mut self, k: usize) {
        let mut cand = None;
        let mut angle = 0;
        let mut stop = false;
        for j in k + 1..self.sig.len().min(k + 300) {
            if self.is_p(j, b'<') {
                angle += 1;
            } else if self.is_p(j, b'>') && !self.is_p(j - 1, b'-') {
                angle -= 1;
            } else if angle <= 0 {
                if self.is_p(j, b'{') || self.is_p(j, b';') {
                    break;
                }
                match (self.kind(j), self.txt(j)) {
                    (Some(Kind::Keyword), "for") => cand = None,
                    (Some(Kind::Keyword), "where") => stop = true,
                    // `impl $name` in a macro template names no type.
                    _ if self.is_name(j) && !stop && !self.is_p(j - 1, b'$') => cand = Some(j),
                    _ => {}
                }
            }
        }
        self.pending = Some(Pending {
            sym: None,
            name: cand.map(|j| self.txt(j).to_string()),
            class_like: true,
            kind: Some(SymbolKind::Class),
            depth: self.depth,
            paren: self.paren,
        });
    }

    fn go_func(&mut self, k: usize) {
        let mut j = k + 1;
        let mut container = None;
        if self.is_p(j, b'(') {
            let Some(close) = self.matching(j) else {
                return;
            };
            let names: Vec<usize> = (j + 1..close)
                .filter(|&i| self.is_name(i) && !self.is_p(i - 1, b'['))
                .collect();
            container = names
                .get(1)
                .or(names.first())
                .map(|&i| self.txt(i).to_string());
            j = close + 1;
        }
        if self.is_name(j) {
            let kind = if container.is_some() {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            self.define(j, kind, container);
        }
    }

    fn go_def(&mut self, k: usize, def: Def) {
        let mut next = k + 1;
        if self.is_p(next, b'[') {
            next = self.matching(next).map_or(next, |m| m + 1);
        }
        let kind = match (def, self.txt(next)) {
            (Def::Type, "struct") => SymbolKind::Class,
            (Def::Type, "interface") => SymbolKind::Interface,
            _ => sym_kind(def, false),
        };
        self.define(k, kind, None);
    }

    fn js_var(&mut self, k: usize, def: Def) {
        let top = self.depth == 0
            || self
                .scopes
                .last()
                .is_some_and(|s| s.kind == Some(SymbolKind::Module) && s.depth == self.depth);
        if self.paren != 0 || !top || !self.is_name(k + 1) {
            return;
        }
        let name = k + 1;
        let mut eq = name + 1;
        if self.is_p(eq, b':') {
            eq = (eq..self.sig.len().min(eq + 80))
                .find(|&i| {
                    (self.is_p(i, b'=') && !self.is_p(i + 1, b'>'))
                        || self.is_p(i, b';')
                        || self.line(i) != self.line(name)
                })
                .unwrap_or(eq);
        }
        let v = eq + 1;
        let arrow = |i: usize| self.is_p(i, b'=') && self.is_p(i + 1, b'>');
        let kind = if !self.is_p(eq, b'=') {
            sym_kind(def, false)
        } else if matches!(self.txt(v), "function" | "async") {
            SymbolKind::Function
        } else if self.txt(v) == "class" {
            SymbolKind::Class
        } else if (self.is_p(v, b'(')
            && self
                .matching(v)
                .is_some_and(|m| arrow(m + 1) || self.is_p(m + 1, b':')))
            || (self.is_name(v) && arrow(v + 1))
        {
            SymbolKind::Function
        } else if all_caps(self.txt(name)) {
            SymbolKind::Constant
        } else {
            SymbolKind::Variable
        };
        let c = self.container();
        self.define(name, kind, c);
    }

    /// Rules for a name token that no keyword claimed.
    fn name(&mut self, k: usize) {
        let fam = self.lang.family;
        if let Some(b) = &self.block
            && self.paren == b.paren
            && self.new_line(k)
        {
            let def = b.def;
            self.go_def(k, def);
            return;
        }
        match fam {
            Family::Python => self.py_assign(k),
            Family::Shell => {
                if self.new_line(k) && self.is_p(k + 1, b'(') && self.is_p(k + 2, b')') {
                    self.add(k, SymbolKind::Function, None);
                }
            }
            Family::C if self.is_p(k + 1, b'(') && self.class_body().is_none() => {
                self.c_function(k)
            }
            _ => {}
        }
        if !self.consumed[k]
            && !matches!(
                fam,
                Family::Python | Family::Ruby | Family::Lua | Family::Shell
            )
        {
            self.member(k);
        }
    }

    fn py_assign(&mut self, k: usize) {
        if self.paren != 0 || !self.new_line(k) {
            return;
        }
        let assigns = (self.is_p(k + 1, b'=') && !self.is_p(k + 2, b'=')) || self.is_p(k + 1, b':');
        if !assigns {
            return;
        }
        let t = self.t(k).expect("in range");
        let name = self.txt(k);
        match self.py.last() {
            None if t.col == 0 => {
                let kind = if all_caps(name) {
                    SymbolKind::Constant
                } else {
                    SymbolKind::Variable
                };
                self.add(k, kind, None);
            }
            Some(p) if p.class => {
                let c = Some(p.name.clone());
                self.add(k, SymbolKind::Field, c);
            }
            _ => {}
        }
    }

    /// C and C++: `type name(args) {` outside class bodies is a function definition.
    fn c_function(&mut self, k: usize) {
        let Some(p) = self.prev(k) else {
            return;
        };
        let prev_ok = match self.kind(p) {
            Some(Kind::Ident | Kind::Type) => true,
            Some(Kind::Keyword) => !matches!(
                self.txt(p),
                "return" | "else" | "case" | "goto" | "sizeof" | "new" | "delete" | "throw"
            ),
            Some(Kind::Punct(c)) => matches!(c, b'*' | b'&' | b'>' | b':' | b'~'),
            _ => false,
        };
        if !prev_ok {
            return;
        }
        let Some(close) = self.matching(k + 1) else {
            return;
        };
        let mut j = close + 1;
        while matches!(
            self.txt(j),
            "const" | "noexcept" | "override" | "final" | "volatile"
        ) {
            j += 1;
        }
        if !self.is_p(j, b'{') {
            return;
        }
        let owner = (self.is_p(p, b':') && p >= 2 && self.is_p(p - 1, b':') && self.is_name(p - 2))
            .then(|| self.txt(p - 2).to_string());
        let (kind, container) = match owner {
            Some(o) => (SymbolKind::Method, Some(o)),
            None => (SymbolKind::Function, self.container()),
        };
        self.define(k, kind, container);
    }

    /// Fields, methods, and enum members directly inside a class-like body.
    fn member(&mut self, k: usize) {
        let Some(scope) = self.class_body() else {
            return;
        };
        if self.in_init {
            return;
        }
        let container = scope.name.clone();
        let enum_body = scope.kind == Some(SymbolKind::Enum);
        let fam = self.lang.family;
        let prev = self.prev(k);
        if enum_body
            && prev.is_none_or(|p| self.is_p(p, b'{') || self.is_p(p, b','))
            && b",}(={;".iter().any(|&c| self.is_p(k + 1, c))
        {
            self.add(k, SymbolKind::Constant, container);
            return;
        }
        let prev_ok = prev.is_some_and(|p| match self.kind(p) {
            Some(Kind::Keyword) => !NOT_BEFORE_MEMBER.contains(&self.txt(p)),
            Some(Kind::Ident | Kind::Type | Kind::Attr) => true,
            Some(Kind::Punct(c)) => b"{};)>]*&".contains(&c) || (c == b',' && fam == Family::Rust),
            _ => false,
        });
        if !prev_ok {
            return;
        }
        let colon = self.is_p(k + 1, b':') && !self.is_p(k + 2, b':');
        match fam {
            Family::Go => {
                if self.new_line(k) {
                    let kind = if self.is_p(k + 1, b'(') {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Field
                    };
                    self.add(k, kind, container);
                }
            }
            Family::Rust => {
                if colon {
                    self.add(k, SymbolKind::Field, container);
                }
            }
            _ => {
                let keyword_methods = matches!(self.lang.id, "kotlin" | "swift" | "scala" | "php");
                if self.is_p(k + 1, b'(') {
                    if !keyword_methods {
                        self.define(k, SymbolKind::Method, container);
                    }
                } else if colon
                    || (self.is_p(k + 1, b'=')
                        && !self.is_p(k + 2, b'=')
                        && !self.is_p(k + 2, b'>'))
                    || self.is_p(k + 1, b';')
                    || (self.is_p(k + 1, b'?')
                        && (self.is_p(k + 2, b':') || self.is_p(k + 2, b'(')))
                    || (self.is_p(k + 1, b'!') && self.is_p(k + 2, b':'))
                    || (self.lang.id == "csharp" && self.is_p(k + 1, b'{'))
                {
                    self.add(k, SymbolKind::Field, container);
                }
            }
        }
    }

    fn push_import(&mut self, k: usize, spec: String, style: ImportStyle, alts: Vec<String>) {
        if spec.is_empty() {
            return;
        }
        let line = self.line(k);
        self.out.imports.push(RawImport {
            spec,
            line,
            style,
            alts,
        });
    }

    fn str_at(&self, k: usize) -> Option<String> {
        (self.kind(k) == Some(Kind::Str)).then(|| unquote(self.txt(k)).to_string())
    }

    /// Token texts from `from` while `keep` holds, joined without spaces except between words.
    fn joined(&self, from: usize, keep: impl Fn(usize) -> bool) -> (String, usize) {
        let mut s = String::new();
        let mut j = from;
        while j < self.sig.len() && keep(j) {
            if j > from && self.is_word(j) && self.is_word(j - 1) {
                s.push(' ');
            }
            s.push_str(self.txt(j));
            j += 1;
        }
        (s, j)
    }

    fn is_word(&self, k: usize) -> bool {
        matches!(
            self.kind(k),
            Some(Kind::Ident | Kind::Type | Kind::Keyword | Kind::Constant)
        )
    }

    fn imports(&mut self, k: usize) {
        let text = self.txt(k);
        let kw = self.kind(k) == Some(Kind::Keyword);
        match self.lang.family {
            Family::Rust if kw => match text {
                "use" => {
                    let (s, _) = self.joined(k + 1, |j| !self.is_p(j, b';') && j < k + 500);
                    let mut specs = vec![];
                    expand_use("", &s, &mut specs);
                    for spec in specs {
                        self.push_import(k, spec, ImportStyle::Use, vec![]);
                    }
                }
                "mod" if self.is_name(k + 1) && self.is_p(k + 2, b';') => {
                    let spec = self.txt(k + 1).to_string();
                    self.push_import(k, spec, ImportStyle::Mod, vec![]);
                }
                "crate" if self.txt(k.wrapping_sub(1)) == "extern" && self.is_name(k + 1) => {
                    let spec = self.txt(k + 1).to_string();
                    self.push_import(k, spec, ImportStyle::Use, vec![]);
                }
                _ => {}
            },
            Family::Js => {
                let spec = match text {
                    "import" if kw && !self.is_p(k + 1, b'.') => {
                        if self.is_p(k + 1, b'(') {
                            self.str_at(k + 2)
                        } else {
                            self.str_at(k + 1).or_else(|| self.js_from(k))
                        }
                    }
                    "export"
                        if kw
                            && (self.is_p(k + 1, b'{')
                                || self.is_p(k + 1, b'*')
                                || (self.txt(k + 1) == "type" && self.is_p(k + 2, b'{'))) =>
                    {
                        self.js_from(k)
                    }
                    "require" if !kw && self.is_p(k + 1, b'(') => self.str_at(k + 2),
                    _ => None,
                };
                if let Some(s) = spec {
                    self.push_import(k, s, ImportStyle::Module, vec![]);
                }
            }
            Family::Python if kw && self.new_line(k) => match text {
                "import" => {
                    let line = self.line(k);
                    let mut j = k + 1;
                    while self.line(j) == line {
                        let (name, end) = self.joined(j, |i| {
                            self.line(i) == line && (self.is_name(i) || self.is_p(i, b'.'))
                        });
                        if !name.is_empty() {
                            self.push_import(k, name, ImportStyle::Use, vec![]);
                        }
                        j = end;
                        if self.txt(j) == "as" {
                            j += 2;
                        }
                        if !self.is_p(j, b',') {
                            break;
                        }
                        j += 1;
                    }
                }
                "from" => {
                    let (module, end) =
                        self.joined(k + 1, |i| self.is_name(i) || self.is_p(i, b'.'));
                    if self.txt(end) != "import" {
                        return;
                    }
                    let line = self.line(k);
                    let paren = self.is_p(end + 1, b'(');
                    let mut alts = vec![];
                    let sep = if module.ends_with('.') { "" } else { "." };
                    for j in end + 1..self.sig.len().min(end + 400) {
                        if (!paren && self.line(j) != line) || (paren && self.is_p(j, b')')) {
                            break;
                        }
                        if self.is_name(j) && self.txt(j - 1) != "as" && self.txt(j) != "as" {
                            alts.push(format!("{module}{sep}{}", self.txt(j)));
                        }
                    }
                    self.push_import(k, module, ImportStyle::Use, alts);
                }
                _ => {}
            },
            Family::Go if kw && text == "import" => {
                if self.is_p(k + 1, b'(') {
                    let close = self.matching(k + 1).unwrap_or(k + 1);
                    for j in k + 2..close {
                        if let Some(s) = self.str_at(j) {
                            self.push_import(j, s, ImportStyle::Module, vec![]);
                        }
                    }
                } else if let Some(s) = self.str_at(k + 1).or_else(|| self.str_at(k + 2)) {
                    self.push_import(k, s, ImportStyle::Module, vec![]);
                }
            }
            Family::Jvm if kw => match text {
                "import" | "using" if !self.is_p(k + 1, b'(') => {
                    let mut j = k + 1;
                    if self.txt(j) == "static" {
                        j += 1;
                    }
                    if self.is_p(j + 1, b'=') {
                        j += 2;
                    }
                    let line = self.line(k);
                    let (s, _) = self.joined(j, |i| {
                        self.line(i) == line
                            && (self.is_name(i) || self.is_p(i, b'.') || self.is_p(i, b'*'))
                    });
                    self.push_import(k, s, ImportStyle::Use, vec![]);
                }
                "use" if self.lang.id == "php" && !self.is_p(k + 1, b'(') => {
                    let (s, _) = self.joined(k + 1, |j| !self.is_p(j, b';') && j < k + 200);
                    for part in s.split(',') {
                        let spec = part.split(" as ").next().unwrap_or("").trim();
                        self.push_import(k, spec.to_string(), ImportStyle::Use, vec![]);
                    }
                }
                "require" | "require_once" | "include" | "include_once" => {
                    if let Some(s) = (k + 1..k + 6).find_map(|j| self.str_at(j)) {
                        self.push_import(k, s, ImportStyle::Require { relative: true }, vec![]);
                    }
                }
                _ => {}
            },
            Family::C if kw && (text.ends_with("include") || text.ends_with("import")) => {
                if let Some(t) = self.t(k + 1).filter(|t| t.kind == Kind::Str) {
                    let raw = t.text(self.src);
                    let quoted = raw.starts_with('"');
                    self.push_import(
                        k,
                        unquote(raw).to_string(),
                        ImportStyle::Include { quoted },
                        vec![],
                    );
                }
            }
            Family::Ruby if kw && matches!(text, "require" | "require_relative" | "load") => {
                let j = if self.is_p(k + 1, b'(') { k + 2 } else { k + 1 };
                if let Some(s) = self.str_at(j) {
                    let relative = text == "require_relative" || text == "load";
                    self.push_import(k, s, ImportStyle::Require { relative }, vec![]);
                }
            }
            Family::Lua if !kw && text == "require" => {
                let j = if self.is_p(k + 1, b'(') { k + 2 } else { k + 1 };
                if let Some(s) = self.str_at(j) {
                    self.push_import(k, s, ImportStyle::Module, vec![]);
                }
            }
            Family::Shell if kw && text == "source" && self.new_line(k) => {
                if let Some(t) = self.t(k + 1).filter(|t| t.line == self.line(k)) {
                    let rest = &self.src[t.start as usize..];
                    let word = rest.split_whitespace().next().unwrap_or("");
                    if !word.contains('$') {
                        let spec = unquote(word).to_string();
                        self.push_import(k, spec, ImportStyle::Require { relative: true }, vec![]);
                    }
                }
            }
            _ => {}
        }
    }

    /// The string after `from` in one JS import or export statement.
    fn js_from(&self, k: usize) -> Option<String> {
        for j in k + 1..self.sig.len().min(k + 400) {
            if self.is_p(j, b';')
                || (j > k + 1
                    && matches!(self.txt(j), "import" | "export")
                    && self.kind(j) == Some(Kind::Keyword))
            {
                return None;
            }
            if self.txt(j) == "from" {
                return self.str_at(j + 1);
            }
        }
        None
    }

    fn classify(&mut self) {
        let data = self.lang.family == Family::Data;
        let mut classes: Vec<Option<TokenClass>> = self
            .toks
            .iter()
            .map(|t| match t.kind {
                Kind::Keyword | Kind::Lifetime => Some(TokenClass::Keyword),
                Kind::Type => Some(TokenClass::Type),
                Kind::Constant => Some(TokenClass::Constant),
                Kind::Str => Some(TokenClass::String),
                Kind::Num => Some(TokenClass::Number),
                Kind::Comment => Some(TokenClass::Comment),
                Kind::Attr => Some(TokenClass::Attribute),
                Kind::Macro => Some(TokenClass::Macro),
                Kind::Punct(_) | Kind::Ident => None,
            })
            .collect();
        for k in 0..self.sig.len() {
            let i = self.sig[k];
            if self.toks[i].kind != Kind::Ident {
                continue;
            }
            let text = self.txt(k);
            classes[i] = if data {
                if self.is_p(k + 1, b'=') || self.is_p(k + 1, b':') {
                    Some(TokenClass::Attribute)
                } else if k > 0
                    && (self.is_p(k - 1, b'<')
                        || (self.is_p(k - 1, b'/') && self.is_p(k.saturating_sub(2), b'<')))
                {
                    Some(TokenClass::Keyword)
                } else {
                    None
                }
            } else if let Some(kind) = self.def_kind[i] {
                Some(class_of(kind))
            } else if self.is_p(k + 1, b'(') {
                Some(TokenClass::Function)
            } else if all_caps(text) {
                Some(TokenClass::Constant)
            } else if text.starts_with(|c: char| c.is_ascii_uppercase())
                && self.lang.family != Family::Shell
            {
                Some(TokenClass::Type)
            } else {
                Some(TokenClass::Name)
            };
        }
        self.out.classes = classes;
    }
}

/// Expand one Rust `use` tree into paths: `a::{b, c::d}` gives `a::b` and `a::c::d`.
pub fn expand_use(prefix: &str, s: &str, out: &mut Vec<String>) {
    let s = s.trim();
    if let Some(open) = s.find('{') {
        let close = s.rfind('}').unwrap_or(s.len());
        let head = &s[..open];
        let inner = &s[open + 1..close.max(open + 1)];
        let mut depth = 0;
        let mut start = 0;
        let full = format!("{prefix}{head}");
        for (i, c) in inner.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                ',' if depth == 0 => {
                    expand_use(&full, &inner[start..i], out);
                    start = i + 1;
                }
                _ => {}
            }
        }
        expand_use(&full, &inner[start..], out);
        return;
    }
    if s.is_empty() {
        return;
    }
    let path = s.split(" as ").next().unwrap_or(s).trim();
    let joined = if path == "self" {
        prefix.trim_end_matches("::").to_string()
    } else {
        format!("{prefix}{path}")
    };
    let joined = joined.trim_end_matches("::*").trim_end_matches("::");
    if !joined.is_empty() {
        out.push(joined.to_string());
    }
}
