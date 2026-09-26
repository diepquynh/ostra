//! Pure conversions between LSP shapes and `ostra_core::code` shapes. LSP lines are 0-based and
//! Ostra's are 1-based; both count columns in UTF-16 code units.

use ostra_core::code::{
    CodeCompletion, CodeCompletionItem, CodeDoc, CodeFile, CodeParameter, CodeRange, CodeSignature,
    CodeSignatureHelp, CodeSymbol, CodeTextEdit, CompletionKind, SymbolKind, TokenClass,
};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

fn pos(v: &Value) -> Option<(u32, u32)> {
    Some((
        v.get("line")?.as_u64()? as u32,
        v.get("character")?.as_u64()? as u32,
    ))
}

pub fn range(v: &Value) -> Option<Range> {
    let (start_line, start_col) = pos(v.get("start")?)?;
    let (end_line, end_col) = pos(v.get("end")?)?;
    Some(Range {
        start_line,
        start_col,
        end_line,
        end_col,
    })
}

/// `Location`, `Location[]`, or `LocationLink[]`, as (uri, range of the name).
pub fn locations(v: &Value) -> Vec<(String, Range)> {
    let one = |l: &Value| -> Option<(String, Range)> {
        if let Some(uri) = l.get("targetUri").and_then(Value::as_str) {
            let r = l
                .get("targetSelectionRange")
                .or_else(|| l.get("targetRange"))?;
            return Some((uri.to_string(), range(r)?));
        }
        Some((l.get("uri")?.as_str()?.to_string(), range(l.get("range")?)?))
    };
    match v {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        Value::Object(_) => one(v).into_iter().collect(),
        _ => vec![],
    }
}

/// `TypeHierarchyItem`s as (uri, name range).
pub fn hierarchy_items(v: &Value) -> Vec<(String, Range)> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let r = i.get("selectionRange").or_else(|| i.get("range"))?;
            Some((i.get("uri")?.as_str()?.to_string(), range(r)?))
        })
        .collect()
}

/// LSP `SymbolKind` numbers. Value kinds (string, number, key, null) have no Ostra kind.
pub fn symbol_kind(n: u64) -> Option<SymbolKind> {
    Some(match n {
        2..=4 => SymbolKind::Module,
        5 | 23 => SymbolKind::Class,
        6 | 9 => SymbolKind::Method,
        7 | 8 => SymbolKind::Field,
        10 => SymbolKind::Enum,
        11 => SymbolKind::Interface,
        12 => SymbolKind::Function,
        13 => SymbolKind::Variable,
        14 | 22 => SymbolKind::Constant,
        26 => SymbolKind::Type,
        _ => return None,
    })
}

/// A `textDocument/documentSymbol` answer, flattened in source order with each child's parent as
/// its container.
pub fn document_symbols(v: &Value) -> Vec<CodeSymbol> {
    fn walk(items: &[Value], container: Option<&str>, out: &mut Vec<CodeSymbol>) {
        for s in items {
            let Some(name) = s.get("name").and_then(Value::as_str) else {
                continue;
            };
            let kind = s.get("kind").and_then(Value::as_u64).and_then(symbol_kind);
            // DocumentSymbol has selectionRange; SymbolInformation has location.range.
            let full = s
                .get("range")
                .or_else(|| s.get("location").and_then(|l| l.get("range")))
                .and_then(range);
            let at = s.get("selectionRange").and_then(range).or(full);
            if let (Some(kind), Some(at)) = (kind, at) {
                out.push(CodeSymbol {
                    name: name.to_string(),
                    kind,
                    line: at.start_line + 1,
                    col: at.start_col,
                    end_line: full.map(|r| r.end_line + 1),
                    container: container.map(str::to_string).or_else(|| {
                        s.get("containerName")
                            .and_then(Value::as_str)
                            .filter(|c| !c.is_empty())
                            .map(str::to_string)
                    }),
                    via: None,
                });
            }
            if let Some(kids) = s.get("children").and_then(Value::as_array) {
                walk(kids, Some(name), out);
            }
        }
    }
    let mut out = vec![];
    if let Some(items) = v.as_array() {
        walk(items, None, &mut out);
    }
    out.sort_by_key(|s| (s.line, s.col));
    out
}

/// A server's semantic token legend, mapped onto Ostra's classes.
#[derive(Debug, Clone, Default)]
pub struct Legend {
    types: Vec<Option<TokenClass>>,
    /// Modifier bits that make a variable a constant.
    constant_bits: u32,
}

impl Legend {
    pub fn from_caps(caps: &Value) -> Option<Legend> {
        let p = caps.get("semanticTokensProvider")?;
        let full = p.get("full")?;
        if full == &Value::Bool(false) || full.is_null() {
            return None;
        }
        let l = p.get("legend")?;
        let names = |k: &str| -> Vec<String> {
            l.get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|s| s.as_str().unwrap_or("").to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        let constant_bits = names("tokenModifiers")
            .iter()
            .enumerate()
            .filter(|(i, m)| *i < 32 && matches!(m.as_str(), "readonly" | "constant"))
            .fold(0, |acc, (i, _)| acc | (1 << i));
        Some(Legend {
            types: names("tokenTypes").iter().map(|t| token_class(t)).collect(),
            constant_bits,
        })
    }
}

/// Standard LSP token types plus the common rust-analyzer, clangd, and gopls additions.
/// Punctuation and brackets have no class, so the built-in tokenizer colors them.
fn token_class(t: &str) -> Option<TokenClass> {
    Some(match t {
        "namespace"
        | "variable"
        | "parameter"
        | "property"
        | "label"
        | "lifetime"
        | "selfParameter"
        | "unresolvedReference"
        | "toolModule"
        | "crateRoot" => TokenClass::Name,
        "type" | "class" | "enum" | "interface" | "struct" | "typeParameter" | "builtinType"
        | "typeAlias" | "union" | "trait" | "concept" => TokenClass::Type,
        "function" | "method" | "macroBang" => TokenClass::Function,
        "macro" | "derive" => TokenClass::Macro,
        "enumMember" | "boolean" | "constParameter" | "const" | "static" => TokenClass::Constant,
        "keyword" | "modifier" | "selfKeyword" | "selfTypeKeyword" => TokenClass::Keyword,
        "comment" => TokenClass::Comment,
        "string" | "regexp" | "character" | "escapeSequence" | "formatSpecifier" => {
            TokenClass::String
        }
        "number" => TokenClass::Number,
        "operator" => TokenClass::Operator,
        "decorator" | "attribute" | "builtinAttribute" | "deriveHelper" => TokenClass::Attribute,
        _ => return None,
    })
}

/// A semantic token as Ostra keeps it: 1-based line, UTF-16 column and length, class.
pub type Token = (u32, u32, u32, TokenClass);

/// Decode the relative `data` of a `textDocument/semanticTokens/full` answer.
pub fn decode_tokens(data: &[u32], legend: &Legend) -> Vec<Token> {
    let mut out = Vec::with_capacity(data.len() / 5);
    let (mut line, mut col) = (0u32, 0u32);
    for &[dl, dc, len, ty, mods] in data.as_chunks::<5>().0 {
        if dl > 0 {
            line = line.saturating_add(dl);
            col = dc;
        } else {
            col = col.saturating_add(dc);
        }
        let Some(Some(mut class)) = legend.types.get(ty as usize).copied() else {
            continue;
        };
        if class == TokenClass::Name && mods & legend.constant_bits != 0 {
            class = TokenClass::Constant;
        }
        if len > 0 {
            out.push((line + 1, col, len, class));
        }
    }
    out
}

/// Put the server's tokens over the built-in ones. A built-in token that overlaps a server token
/// is dropped; the rest stay, so the server may send only what it knows more about.
pub fn overlay(file: &mut CodeFile, server: &[Token]) {
    let mut by_line: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    for &(line, col, len, _) in server {
        by_line.entry(line).or_default().push((col, col + len));
    }
    let mut merged: Vec<[u32; 4]> = file
        .tokens
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|t| {
            let (s, e) = (t[1], t[1] + t[2]);
            by_line
                .get(&t[0])
                .is_none_or(|spans| spans.iter().all(|&(a, b)| e <= a || b <= s))
        })
        .copied()
        .collect();
    for &(line, col, len, class) in server {
        let idx = match file.classes.iter().position(|c| *c == class) {
            Some(i) => i,
            None => {
                file.classes.push(class);
                file.classes.len() - 1
            }
        };
        merged.push([line, col, len, idx as u32]);
    }
    merged.sort_unstable_by_key(|t| (t[0], t[1]));
    file.tokens = merged.into_iter().flatten().collect();
}

pub fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// Byte offset in `line` of UTF-16 column `col`, clamped to the line.
pub fn byte_at(line: &str, col: u32) -> usize {
    let mut units = 0u32;
    for (i, ch) in line.char_indices() {
        if units >= col {
            return i;
        }
        units += ch.len_utf16() as u32;
    }
    line.len()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The UTF-16 column of the whole-word `name` in `line` nearest to `col`.
pub fn find_name(line: &str, name: &str, col: u32) -> Option<u32> {
    if name.is_empty() {
        return None;
    }
    line.match_indices(name)
        .filter(|(i, _)| {
            let before = line[..*i].chars().next_back();
            let after = line[i + name.len()..].chars().next();
            !before.is_some_and(is_word) && !after.is_some_and(is_word)
        })
        .map(|(i, _)| utf16_len(&line[..i]))
        .min_by_key(|c| c.abs_diff(col))
}

/// The `languageId` a server expects in `didOpen` for a file of Ostra language `lang`.
pub fn language_id(lang: &str, path: &str) -> String {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match (lang, ext) {
        ("typescript", "tsx") => "typescriptreact",
        ("javascript", "jsx") => "javascriptreact",
        ("shell", _) => "shellscript",
        (l, _) => l,
    }
    .to_string()
}

/// Items kept from one completion answer; the list is marked incomplete past this.
pub const MAX_COMPLETIONS: usize = 1000;

fn code_range(v: &Value) -> Option<CodeRange> {
    let r = range(v)?;
    Some(CodeRange {
        line: r.start_line + 1,
        col: r.start_col,
        end_line: r.end_line + 1,
        end_col: r.end_col,
    })
}

fn text_edit(v: &Value) -> Option<CodeTextEdit> {
    Some(CodeTextEdit {
        range: code_range(&v["range"])?,
        text: v["newText"].as_str()?.to_string(),
    })
}

/// `string | MarkupContent`, dropping empty text.
pub fn doc(v: &Value) -> Option<CodeDoc> {
    let (text, markdown) = match v {
        Value::String(s) => (s.as_str(), false),
        Value::Object(_) => (v["value"].as_str()?, v["kind"] == "markdown"),
        _ => return None,
    };
    (!text.trim().is_empty()).then(|| CodeDoc {
        text: text.to_string(),
        markdown,
    })
}

pub fn completion_kind(n: u64) -> Option<CompletionKind> {
    use CompletionKind::*;
    const KINDS: [CompletionKind; 25] = [
        Text,
        Method,
        Function,
        Constructor,
        Field,
        Variable,
        Class,
        Interface,
        Module,
        Property,
        Unit,
        Value,
        Enum,
        Keyword,
        Snippet,
        Color,
        File,
        Reference,
        Folder,
        EnumMember,
        Constant,
        Struct,
        Event,
        Operator,
        TypeParameter,
    ];
    KINDS.get((n as usize).checked_sub(1)?).copied()
}

fn completion_item(v: &Value) -> Option<CodeCompletionItem> {
    let label = v["label"].as_str()?.to_string();
    let edit = &v["textEdit"];
    // An `InsertReplaceEdit` has `insert` and `replace` ranges; inserting keeps the text after.
    let range = code_range(&edit["range"]).or_else(|| code_range(&edit["insert"]));
    let insert = edit["newText"]
        .as_str()
        .or_else(|| v["insertText"].as_str())
        .unwrap_or(&label)
        .to_string();
    let text = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let tags = v["tags"].as_array().map(Vec::as_slice).unwrap_or_default();
    Some(CodeCompletionItem {
        kind: v["kind"].as_u64().and_then(completion_kind),
        detail: text("detail"),
        doc: doc(&v["documentation"]),
        insert,
        snippet: v["insertTextFormat"] == 2,
        range,
        filter: text("filterText"),
        sort: text("sortText"),
        preselect: v["preselect"] == true,
        // CompletionItemTag 1 is Deprecated.
        deprecated: v["deprecated"] == true || tags.iter().any(|t| t == 1),
        edits: v["additionalTextEdits"]
            .as_array()
            .map(|a| a.iter().filter_map(text_edit).collect())
            .unwrap_or_default(),
        commit_chars: v["commitCharacters"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        label,
    })
}

/// `CompletionItem[] | CompletionList | null`.
pub fn completion(v: &Value) -> CodeCompletion {
    let (items, mut incomplete) = match v {
        Value::Array(a) => (a.as_slice(), false),
        Value::Object(_) => (
            v["items"].as_array().map(Vec::as_slice).unwrap_or_default(),
            v["isIncomplete"] == true,
        ),
        _ => (&[][..], false),
    };
    if items.len() > MAX_COMPLETIONS {
        incomplete = true;
    }
    CodeCompletion {
        provider: String::new(),
        items: items
            .iter()
            .take(MAX_COMPLETIONS)
            .filter_map(completion_item)
            .collect(),
        incomplete,
        warning: None,
    }
}

fn signature(v: &Value) -> Option<CodeSignature> {
    let label = v["label"].as_str()?.to_string();
    let mut from = 0usize;
    let parameters = v["parameters"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let (start, end) = match &p["label"] {
                Value::String(name) => match label[from..].find(name.as_str()) {
                    Some(i) => {
                        let at = from + i;
                        from = at + name.len();
                        (utf16_len(&label[..at]), utf16_len(&label[..from]))
                    }
                    None => (0, 0),
                },
                Value::Array(a) => (
                    a.first().and_then(Value::as_u64).unwrap_or(0) as u32,
                    a.get(1).and_then(Value::as_u64).unwrap_or(0) as u32,
                ),
                _ => (0, 0),
            };
            CodeParameter {
                start,
                end,
                doc: doc(&p["documentation"]),
            }
        })
        .collect();
    Some(CodeSignature {
        doc: doc(&v["documentation"]),
        parameters,
        active_parameter: v["activeParameter"].as_u64().map(|n| n as u32),
        label,
    })
}

/// `SignatureHelp | null`, and None when it has no signatures.
pub fn signature_help(v: &Value) -> Option<CodeSignatureHelp> {
    let signatures: Vec<CodeSignature> = v["signatures"]
        .as_array()?
        .iter()
        .filter_map(signature)
        .collect();
    if signatures.is_empty() {
        return None;
    }
    let active =
        (v["activeSignature"].as_u64().unwrap_or(0) as u32).min(signatures.len() as u32 - 1);
    Some(CodeSignatureHelp {
        provider: String::new(),
        signatures,
        active_signature: active,
        active_parameter: v["activeParameter"].as_u64().unwrap_or(0) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tokens_decode_relative_positions_and_constant_modifiers() {
        let caps = json!({"semanticTokensProvider": {"full": true, "legend": {
            "tokenTypes": ["function", "variable", "punctuation"],
            "tokenModifiers": ["declaration", "readonly"]}}});
        let legend = Legend::from_caps(&caps).unwrap();
        // fn at (0,3) len 4; variable readonly at (0,10) len 1; punctuation skipped; variable at (2,1).
        let data = [0, 3, 4, 0, 1, 0, 7, 1, 1, 2, 0, 2, 1, 2, 0, 2, 1, 1, 1, 0];
        assert_eq!(
            decode_tokens(&data, &legend),
            vec![
                (1, 3, 4, TokenClass::Function),
                (1, 10, 1, TokenClass::Constant),
                (3, 1, 1, TokenClass::Name),
            ]
        );
        assert!(Legend::from_caps(&json!({"semanticTokensProvider": {"range": true}})).is_none());
    }

    #[test]
    fn overlay_replaces_overlapping_builtin_tokens_only() {
        let mut f = crate::render("a.rs", "fn main() { x }", None);
        let before = f.tokens.len() / 4;
        overlay(&mut f, &[(1, 3, 4, TokenClass::Macro)]);
        assert_eq!(f.tokens.len() / 4, before);
        let macro_idx = f
            .classes
            .iter()
            .position(|c| *c == TokenClass::Macro)
            .unwrap() as u32;
        assert!(f.tokens.chunks(4).any(|t| t == [1, 3, 4, macro_idx]));
        assert!(
            f.tokens.chunks(4).any(|t| t[1] == 0 && t[2] == 2),
            "`fn` stays"
        );
        assert!(f.tokens.chunks(4).is_sorted_by_key(|t| (t[0], t[1])));
    }

    #[test]
    fn document_symbols_flatten_with_containers() {
        let r = |l: u32, c: u32, el: u32| json!({"start": {"line": l, "character": c}, "end": {"line": el, "character": 1}});
        let v = json!([{"name": "S", "kind": 23, "range": r(0, 0, 4), "selectionRange": r(0, 7, 0),
            "children": [{"name": "go", "kind": 6, "range": r(2, 4, 3), "selectionRange": r(2, 7, 2)},
                         {"name": "\"k\"", "kind": 15, "range": r(1, 0, 1), "selectionRange": r(1, 0, 1)}]}]);
        let s = document_symbols(&v);
        assert_eq!(s.len(), 2);
        assert_eq!(
            (s[0].name.as_str(), s[0].line, s[0].col, s[0].end_line),
            ("S", 1, 7, Some(5))
        );
        assert_eq!(
            (s[1].kind, s[1].container.as_deref()),
            (SymbolKind::Method, Some("S"))
        );
    }

    #[test]
    fn locations_accept_every_shape() {
        let r = json!({"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}});
        assert_eq!(locations(&json!({"uri": "file:///a", "range": r})).len(), 1);
        assert_eq!(
            locations(
                &json!([{"targetUri": "file:///b", "targetRange": r, "targetSelectionRange": r}])
            )[0]
            .0,
            "file:///b"
        );
        assert!(locations(&Value::Null).is_empty());
    }

    #[test]
    fn hierarchy_items_prefer_the_name_range() {
        let whole =
            json!({"start": {"line": 1, "character": 0}, "end": {"line": 9, "character": 1}});
        let name =
            json!({"start": {"line": 1, "character": 6}, "end": {"line": 1, "character": 12}});
        let got = hierarchy_items(&json!([
            {"name": "Animal", "kind": 5, "uri": "file:///a", "range": whole, "selectionRange": name},
            {"name": "broken"}
        ]));
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].1.start_line, got[0].1.start_col), (1, 6));
        assert!(hierarchy_items(&Value::Null).is_empty());
    }

    #[test]
    fn find_name_matches_whole_words_by_utf16_column() {
        assert_eq!(find_name("let aé = a + ab;", "a", 13), Some(9));
        assert_eq!(find_name("let aé = a + ab;", "a", 0), Some(9));
        assert_eq!(find_name("é x", "x", 0), Some(2));
        assert_eq!(find_name("xy", "x", 0), None);
        assert_eq!(byte_at("éx", 1), 2);
    }

    #[test]
    fn completion_reads_lists_edits_and_snippets() {
        let v = json!({"isIncomplete": true, "items": [
            {"label": "push", "kind": 2, "detail": "fn(&mut self, T)",
             "documentation": {"kind": "markdown", "value": "Appends."},
             "insertTextFormat": 2,
             "textEdit": {"range": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 6}},
                          "newText": "push(${1:value})"},
             "additionalTextEdits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                                      "newText": "use std::vec::Vec;\n"}],
             "tags": [1]},
            {"label": "len", "insertText": "len"},
            {"label": "pop", "textEdit": {"newText": "pop",
                "insert": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 3}},
                "replace": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}}}},
            {"kind": 3}
        ]});
        let c = completion(&v);
        assert!(c.incomplete);
        assert_eq!(c.items.len(), 3);
        let push = &c.items[0];
        assert_eq!(push.kind, Some(CompletionKind::Method));
        assert!(push.snippet && push.deprecated);
        assert_eq!(push.insert, "push(${1:value})");
        assert_eq!(
            push.range,
            Some(CodeRange {
                line: 4,
                col: 4,
                end_line: 4,
                end_col: 6
            })
        );
        assert_eq!(push.doc.as_ref().map(|d| d.markdown), Some(true));
        assert_eq!(push.edits[0].range.line, 1);
        assert_eq!(c.items[1].insert, "len");
        assert_eq!(c.items[1].range, None);
        assert_eq!(c.items[2].range.map(|r| r.end_col), Some(3));
        assert_eq!(completion(&json!([{"label": "x"}])).items[0].insert, "x");
        assert!(completion(&Value::Null).items.is_empty());
    }

    #[test]
    fn signature_help_finds_parameter_spans() {
        let v = json!({"activeSignature": 5, "activeParameter": 1, "signatures": [{
            "label": "fn copy(a: &str, b: &str) -> é",
            "documentation": "Copies.",
            "parameters": [{"label": "a: &str"}, {"label": "b: &str"}, {"label": [3, 7]}, {"label": "zzz"}]
        }]});
        let h = signature_help(&v).unwrap();
        assert_eq!(h.active_signature, 0);
        assert_eq!(h.active_parameter, 1);
        let p = &h.signatures[0].parameters;
        assert_eq!((p[0].start, p[0].end), (8, 15));
        assert_eq!((p[1].start, p[1].end), (17, 24));
        assert_eq!((p[2].start, p[2].end), (3, 7));
        assert_eq!((p[3].start, p[3].end), (0, 0));
        assert_eq!(
            h.signatures[0].doc.as_ref().map(|d| d.markdown),
            Some(false)
        );
        assert!(signature_help(&json!({"signatures": []})).is_none());
        assert!(signature_help(&Value::Null).is_none());
    }
}
