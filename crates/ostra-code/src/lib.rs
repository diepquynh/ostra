//! Code navigation for the Files view: a table-driven tokenizer for about twenty languages, an
//! incremental per-project index of names, definitions, and imports, language servers over LSP,
//! and the provider layer that lets a project swap in its own program for any request
//! (`ostra_core::code`).

pub mod graph;
pub mod hint;
pub mod index;
pub mod lang;
pub mod lex;
pub mod lsp;
pub mod outline;
pub mod provider;
pub mod resolve;
pub mod tools;
pub mod usage;

use ostra_core::code::{CodeFile, CodeImport, CodeSymbol, TokenClass};

pub use index::{Indexes, ProjectIndex};
pub use lsp::{LspPool, LspProvider};
pub use provider::{Answer, CodeProvider, CommandProvider, NativeProvider};

/// Files larger than this are not tokenized or indexed.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The order of `CodeFile::classes`, which every native answer uses.
pub const CLASSES: [TokenClass; 11] = [
    TokenClass::Keyword,
    TokenClass::Type,
    TokenClass::Function,
    TokenClass::Name,
    TokenClass::Constant,
    TokenClass::Macro,
    TokenClass::Attribute,
    TokenClass::String,
    TokenClass::Number,
    TokenClass::Comment,
    TokenClass::Operator,
];

fn class_index(c: TokenClass) -> u32 {
    CLASSES.iter().position(|x| *x == c).unwrap_or(0) as u32
}

/// Tokens, outline, and imports of one file's text, with import targets when a resolver is given.
pub fn render(path: &str, src: &str, resolver: Option<&resolve::Resolver>) -> CodeFile {
    let mut out = CodeFile {
        path: path.to_string(),
        provider: String::new(),
        language: None,
        classes: CLASSES.to_vec(),
        tokens: vec![],
        symbols: vec![],
        imports: vec![],
        warning: None,
    };
    let Some(lang) = lang::for_path(path) else {
        return out;
    };
    let toks = lex::lex(src, lang);
    let an = outline::analyze(src, lang, &toks);
    out.language = Some(lang.id.to_string());
    out.tokens.reserve(toks.len() * 4);
    for (t, c) in toks.iter().zip(&an.classes) {
        if let Some(c) = c {
            out.tokens.extend([t.line, t.col, t.len, class_index(*c)]);
        }
    }
    out.symbols = an
        .symbols
        .into_iter()
        .map(|s| CodeSymbol {
            name: s.name,
            kind: s.kind,
            line: s.line,
            col: s.col,
            end_line: s.end_line,
            container: s.container,
            via: s.via,
        })
        .collect();
    out.imports = an
        .imports
        .into_iter()
        .map(|i| {
            let target = resolver.and_then(|r| r.resolve(path, &i, lang));
            CodeImport {
                folder: target.as_ref().is_some_and(|t| t.folder),
                target: target.map(|t| t.path),
                spec: i.spec,
                line: i.line,
            }
        })
        .collect();
    out
}

/// The trimmed source line holding byte `at`, cut to a readable length.
/// The definition whose name starts at byte `at`, from its line start through its parameter
/// list and before its body, on one line: `func (r *Repo) Save(tx *gorm.DB, id string) error`
/// for a Go method whose parameters wrap.
pub fn signature(src: &str, at: usize) -> String {
    const MAX: usize = 300;
    let at = at.min(src.len());
    let start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let mut depth = 0i32;
    let mut out = String::new();
    let mut gap = false;
    for (i, c) in src[start..].char_indices() {
        let pos = start + i;
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            '{' | ';' if depth == 0 && pos > at => break,
            '\n' if depth == 0 => break,
            _ => {}
        }
        if c.is_whitespace() {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() && !out.ends_with(['(', '[']) && !matches!(c, ')' | ']' | ',') {
            out.push(' ');
        }
        gap = false;
        if matches!(c, ')' | ']') && out.ends_with(',') {
            out.pop();
        }
        out.push(c);
        if out.len() >= MAX {
            out.push('…');
            break;
        }
    }
    out.trim_end_matches([':', ' ']).to_string()
}

pub fn preview(src: &str, at: usize) -> String {
    const MAX: usize = 240;
    let at = at.min(src.len());
    let start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = src[at..].find('\n').map_or(src.len(), |i| at + i);
    let line = src[start..end].trim();
    if line.len() <= MAX {
        return line.to_string();
    }
    let mut cut = MAX;
    while !line.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &line[..cut])
}
