//! Editing hints for the file editor: completions, signature help, and supertype or
//! implementation jumps at a cursor of unsaved text. Language servers answer completions and
//! signatures, and the index fills in without one; the index answers jumps first.

use crate::ProjectIndex;
use crate::lsp::convert::{byte_at, utf16_len};
use ostra_core::code::{
    CodeCompletion, CodeCompletionItem, CodeDoc, CodeLocation, CodeNavigation, CodeRange,
    CompletionKind, NATIVE_PROVIDER, NavigateTarget, SymbolKind,
};
use std::collections::HashSet;

/// Index completions per request. The list is incomplete, so the editor asks again as the word
/// grows.
pub const MAX_INDEX_COMPLETIONS: usize = 50;

/// Locations per jump.
pub const MAX_NAVIGATION: usize = 50;
/// Definitions asked about when the name at the cursor is not defined in its own file.
const NAVIGATION_GROUPS: usize = 8;

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

/// The identifier around the cursor: the one it is inside, or the one that ends at it.
pub fn word_at(text: &str, line: u32, col: u32) -> &str {
    let Some(l) = text.split('\n').nth(line.saturating_sub(1) as usize) else {
        return "";
    };
    let at = byte_at(l, col);
    let (before, _) = word_before(text, line, col);
    let after = l[at..].find(|c: char| !is_word(c)).unwrap_or(l.len() - at);
    &l[at - before.len()..at + after]
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

/// The supertypes or implementations of the name at the cursor from the index's implementation
/// links. `None` when the index does not hold the file or defines no type or method by that
/// name, so a language server may answer instead.
pub fn navigate_index(
    ix: &mut ProjectIndex,
    at: &At<'_>,
    target: NavigateTarget,
) -> Option<CodeNavigation> {
    if !ix.by_path.contains_key(at.path) {
        return None;
    }
    let word = word_at(at.text, at.line, at.col);
    if word.is_empty() || word.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    // A name the file defines means that definition; any other is asked across the project.
    let mut r = ix.implementations(word, Some(at.path), Some(at.line), 1, MAX_NAVIGATION);
    if r.definitions == 0 {
        r = ix.implementations(word, None, None, NAVIGATION_GROUPS, MAX_NAVIGATION);
    }
    if r.definitions == 0 {
        return None;
    }
    let mut locations: Vec<CodeLocation> = vec![];
    for x in r.links {
        let list = match target {
            NavigateTarget::Supertypes => x.implements,
            NavigateTarget::Implementations => x.implemented_by,
        };
        for l in list {
            if !locations
                .iter()
                .any(|o| (&o.path, o.line, o.col) == (&l.path, l.line, l.col))
            {
                locations.push(l);
            }
        }
    }
    let truncated = r.truncated || locations.len() > MAX_NAVIGATION;
    locations.truncate(MAX_NAVIGATION);
    Some(CodeNavigation {
        provider: NATIVE_PROVIDER.to_string(),
        symbol: word.to_string(),
        locations,
        truncated,
    })
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

    #[test]
    fn word_at_takes_the_whole_identifier_around_the_cursor() {
        let text = "class Dog extends Animal {}";
        assert_eq!(word_at(text, 1, 20), "Animal");
        assert_eq!(word_at(text, 1, 18), "Animal");
        assert_eq!(word_at(text, 1, 24), "Animal");
        assert_eq!(word_at(text, 1, 25), "");
        assert_eq!(word_at("é_ab x", 1, 1), "é_ab");
        assert_eq!(word_at(text, 3, 0), "");
    }
}
