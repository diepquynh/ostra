//! Editing hints for the file editor: completions and signature help at a cursor of unsaved
//! text. Language servers answer both; without one, completions come from the names the
//! project's index defines.

use crate::ProjectIndex;
use crate::lsp::convert::{byte_at, utf16_len};
use ostra_core::code::{
    CodeCompletion, CodeCompletionItem, CodeDoc, CodeRange, CompletionKind, NATIVE_PROVIDER,
    SymbolKind,
};
use std::collections::HashSet;

/// Index completions per request. The list is incomplete, so the editor asks again as the word
/// grows.
pub const MAX_INDEX_COMPLETIONS: usize = 50;

/// A cursor in the unsaved text of a project file. `line` is 1-based; `col` is 0-based UTF-16.
pub struct At<'a> {
    pub path: &'a str,
    pub text: &'a str,
    pub line: u32,
    pub col: u32,
    /// The typed character that asked, when one did.
    pub trigger: Option<&'a str>,
    /// The editor already shows hints from an earlier request.
    pub retrigger: bool,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The identifier part that ends at the cursor, and its start column.
pub fn word_before(text: &str, line: u32, col: u32) -> (&str, u32) {
    let Some(l) = text.split('\n').nth(line.saturating_sub(1) as usize) else {
        return ("", col);
    };
    let end = byte_at(l, col);
    let start = l[..end]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(end, |(i, _)| i);
    (&l[start..end], utf16_len(&l[..start]))
}

fn kind(k: Option<SymbolKind>) -> CompletionKind {
    match k {
        Some(SymbolKind::Function) => CompletionKind::Function,
        Some(SymbolKind::Method) => CompletionKind::Method,
        Some(SymbolKind::Class) => CompletionKind::Class,
        Some(SymbolKind::Enum) => CompletionKind::Enum,
        Some(SymbolKind::Interface) => CompletionKind::Interface,
        Some(SymbolKind::Type) => CompletionKind::TypeParameter,
        Some(SymbolKind::Module) => CompletionKind::Module,
        Some(SymbolKind::Constant) => CompletionKind::Constant,
        Some(SymbolKind::Field) => CompletionKind::Field,
        Some(SymbolKind::Macro) => CompletionKind::Snippet,
        Some(SymbolKind::Variable) | None => CompletionKind::Variable,
    }
}

/// Names the index defines that start like the word at the cursor. Empty after a trigger
/// character, because the index does not know members.
pub fn from_index(ix: &ProjectIndex, at: &At<'_>) -> CodeCompletion {
    let mut out = CodeCompletion {
        provider: NATIVE_PROVIDER.to_string(),
        items: vec![],
        incomplete: true,
        warning: None,
    };
    let (word, start) = word_before(at.text, at.line, at.col);
    if at.trigger.is_some() || word.is_empty() {
        return out;
    }
    let lower = word.to_lowercase();
    let range = CodeRange {
        line: at.line,
        col: start,
        end_line: at.line,
        end_col: at.col,
    };
    let mut seen = HashSet::new();
    let found = ix.symbols(word, MAX_INDEX_COMPLETIONS * 4);
    for loc in found.items {
        if !loc.name.to_lowercase().starts_with(&lower) || !seen.insert(loc.name.clone()) {
            continue;
        }
        out.items.push(CodeCompletionItem {
            label: loc.name.clone(),
            kind: Some(kind(loc.kind)),
            detail: (!loc.preview.is_empty()).then(|| loc.preview.clone()),
            doc: Some(CodeDoc {
                text: format!("{}:{}", loc.path, loc.line),
                markdown: false,
            }),
            insert: loc.name,
            snippet: false,
            range: Some(range),
            filter: None,
            sort: None,
            preselect: false,
            deprecated: false,
            edits: vec![],
            commit_chars: vec![],
        });
        if out.items.len() == MAX_INDEX_COMPLETIONS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_before_stops_at_non_word_characters() {
        let text = "fn main() {\n    let x = self.pars\n}";
        assert_eq!(word_before(text, 2, 21), ("pars", 17));
        assert_eq!(word_before(text, 2, 17), ("", 17));
        assert_eq!(word_before(text, 2, 19), ("pa", 17));
        assert_eq!(word_before("é_ab", 1, 4), ("é_ab", 0));
        assert_eq!(word_before(text, 9, 0), ("", 0));
    }
}
