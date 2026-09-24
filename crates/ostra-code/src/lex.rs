//! A table-driven tokenizer: comments, strings, numbers, identifiers, keywords, and punctuation
//! for every language in [`crate::lang`]. It never fails; text it does not understand is skipped.
//! Tokens carry byte offsets for the analysis and 1-based lines with UTF-16 columns for the
//! browser. A comment or string that spans lines comes out as one token per line.

use crate::lang::{Family, Lang};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ident,
    Keyword,
    /// A builtin type name.
    Type,
    /// A builtin constant such as `true`.
    Constant,
    Str,
    Num,
    Comment,
    Attr,
    /// A macro call name including its `!` (Rust), or a `#define` target.
    Macro,
    Lifetime,
    Punct(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tok {
    pub kind: Kind,
    /// Byte range in the source.
    pub start: u32,
    pub end: u32,
    /// 1-based.
    pub line: u32,
    /// 0-based, UTF-16.
    pub col: u32,
    /// UTF-16 length.
    pub len: u32,
    /// A later line of a comment or string that started on an earlier line.
    pub cont: bool,
}

impl Tok {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start as usize..self.end as usize]
    }
}

/// Moves forward through the source, turning byte offsets into lines and UTF-16 columns.
#[derive(Default)]
struct Cursor {
    pos: usize,
    line: u32,
    col: u32,
}

impl Cursor {
    fn advance(&mut self, b: &[u8], to: usize) {
        for &c in &b[self.pos..to] {
            if c == b'\n' {
                self.line += 1;
                self.col = 0;
            } else if c & 0xC0 != 0x80 {
                // A 4-byte UTF-8 sequence is a surrogate pair in UTF-16.
                self.col += if c >= 0xF0 { 2 } else { 1 };
            }
        }
        self.pos = to;
    }
}

fn utf16_len(b: &[u8]) -> u32 {
    b.iter()
        .filter(|&&c| c & 0xC0 != 0x80)
        .map(|&c| if c >= 0xF0 { 2 } else { 1 })
        .sum()
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn is_ident_char(c: u8, extra: &[u8]) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 || extra.contains(&c)
}

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    lang: &'a Lang,
    out: Vec<Tok>,
    cur: Cursor,
    /// Only whitespace since the last newline.
    line_start: bool,
}

pub fn lex(src: &str, lang: &Lang) -> Vec<Tok> {
    let mut lx = Lexer {
        src,
        b: src.as_bytes(),
        lang,
        out: Vec::with_capacity(src.len() / 4),
        cur: Cursor {
            pos: 0,
            line: 1,
            col: 0,
        },
        line_start: true,
    };
    lx.run();
    lx.out
}

impl<'a> Lexer<'a> {
    fn at(&self, i: usize) -> u8 {
        self.b.get(i).copied().unwrap_or(0)
    }

    fn starts(&self, i: usize, s: &str) -> bool {
        self.b[i..].starts_with(s.as_bytes())
    }

    fn push(&mut self, kind: Kind, start: usize, end: usize) {
        let end = end.min(self.b.len());
        if end <= start {
            return;
        }
        let mut s = start;
        let mut cont = false;
        while s < end {
            let nl = self.b[s..end]
                .iter()
                .position(|&c| c == b'\n')
                .map(|p| s + p);
            let mut e = nl.unwrap_or(end);
            let next = nl.map(|n| n + 1).unwrap_or(end);
            if e > s && self.b[e - 1] == b'\r' {
                e -= 1;
            }
            if e > s {
                self.cur.advance(self.b, s);
                self.out.push(Tok {
                    kind,
                    start: s as u32,
                    end: e as u32,
                    line: self.cur.line,
                    col: self.cur.col,
                    len: utf16_len(&self.b[s..e]),
                    cont,
                });
            }
            cont = true;
            s = next;
        }
        self.line_start = false;
    }

    fn line_end(&self, i: usize) -> usize {
        self.b[i..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(self.b.len(), |p| i + p)
    }

    fn run(&mut self) {
        let n = self.b.len();
        let fam = self.lang.family;
        let mut i = 0;
        while i < n {
            let c = self.b[i];
            if c == b'\n' {
                self.line_start = true;
                i += 1;
                continue;
            }
            if c == b' ' || c == b'\t' || c == b'\r' || c == 0x0c {
                i += 1;
                continue;
            }
            if let Some((open, close)) = self.lang.block_comment
                && self.starts(i, open)
            {
                let end = self.block_comment(i, open, close);
                self.push(Kind::Comment, i, end);
                i = end;
                continue;
            }
            if self.line_comment(i) {
                let end = self.line_end(i);
                self.push(Kind::Comment, i, end);
                i = end;
                continue;
            }
            if let Some(end) = self.special(i, fam) {
                i = end;
                continue;
            }
            if self.lang.quotes.contains(&c) {
                let end = self.string(i + 1, c, i);
                self.push(Kind::Str, i, end);
                i = end;
                continue;
            }
            if c.is_ascii_digit() || (c == b'.' && self.at(i + 1).is_ascii_digit()) {
                let end = self.number(i);
                self.push(Kind::Num, i, end);
                i = end;
                continue;
            }
            if is_ident_start(c) || (c == b'$' && self.lang.ident_extra.contains(&b'$')) {
                i = self.ident(i);
                continue;
            }
            if c.is_ascii_punctuation() {
                self.push(Kind::Punct(c), i, i + 1);
                i += 1;
                continue;
            }
            // Control bytes: skip one UTF-8 sequence.
            i += 1;
            while i < n && self.b[i] & 0xC0 == 0x80 {
                i += 1;
            }
        }
    }

    fn line_comment(&self, i: usize) -> bool {
        self.lang.line_comments.iter().any(|lc| {
            self.starts(i, lc)
                && match (self.lang.family, *lc) {
                    // `#` starts a shell comment only at the start of a word (`$#`, `a#b`).
                    (Family::Shell, "#") => i == 0 || self.b[i - 1].is_ascii_whitespace(),
                    // PHP 8 attributes are `#[...]`.
                    (Family::Jvm, "#") => self.at(i + 1) != b'[',
                    _ => true,
                }
        })
    }

    fn block_comment(&self, i: usize, open: &str, close: &str) -> usize {
        let n = self.b.len();
        let mut depth = 1;
        let mut j = i + open.len();
        while j < n {
            if self.starts(j, close) {
                depth -= 1;
                j += close.len();
                if depth == 0 || !self.lang.nested_comments {
                    return j;
                }
            } else if self.lang.nested_comments && self.starts(j, open) {
                depth += 1;
                j += open.len();
            } else {
                j += 1;
            }
        }
        n
    }

    /// Scan a string body from `j` (just past the opening quote) to past its closing quote.
    fn string(&self, j: usize, q: u8, open: usize) -> usize {
        let n = self.b.len();
        if self.lang.triple_quotes && q != b'`' && self.at(open + 1) == q && self.at(open + 2) == q
        {
            let mut k = open + 3;
            while k < n {
                if self.b[k] == b'\\' {
                    k += 2;
                    continue;
                }
                if self.b[k] == q && self.at(k + 1) == q && self.at(k + 2) == q {
                    return k + 3;
                }
                k += 1;
            }
            return n;
        }
        let multiline = q == b'`' || self.lang.multiline_strings;
        let escapes = !(q == b'`' && self.lang.family == Family::Go)
            && !(q == b'\'' && self.lang.family == Family::Shell);
        let mut k = j;
        while k < n {
            let c = self.b[k];
            if c == b'\\' && escapes {
                k += 2;
                continue;
            }
            if c == q {
                return k + 1;
            }
            if c == b'\n' && !multiline {
                return k;
            }
            k += 1;
        }
        n
    }

    fn number(&self, i: usize) -> usize {
        let n = self.b.len();
        let mut j = i;
        while j < n {
            let c = self.b[j];
            let exponent = (c == b'+' || c == b'-')
                && matches!(self.b[j - 1], b'e' | b'E')
                && !self.b[i..j].starts_with(b"0x");
            if c.is_ascii_alphanumeric()
                || c == b'_'
                || (c == b'.' && self.at(j + 1).is_ascii_digit())
                || exponent
            {
                j += 1;
            } else {
                break;
            }
        }
        j
    }

    fn ident_end(&self, i: usize) -> usize {
        let mut j = i + 1;
        while j < self.b.len() && is_ident_char(self.b[j], self.lang.ident_extra) {
            j += 1;
        }
        j
    }

    fn ident(&mut self, i: usize) -> usize {
        let end = self.ident_end(i);
        let word = &self.src[i..end];
        let lang = self.lang;
        let next = self.at(end);
        // Python string prefixes: r"", b'', f"""...""".
        if lang.family == Family::Python
            && end - i <= 2
            && lang.quotes.contains(&next)
            && word
                .bytes()
                .all(|c| matches!(c.to_ascii_lowercase(), b'r' | b'b' | b'f' | b'u'))
        {
            let close = self.string(end + 1, next, end);
            self.push(Kind::Str, i, close);
            return close;
        }
        if lang.family == Family::Rust && next == b'!' && self.at(end + 1) != b'=' {
            self.push(Kind::Macro, i, end + 1);
            return end + 1;
        }
        let kind = if lang.is_keyword(word) {
            Kind::Keyword
        } else if lang.constants.contains(&word) {
            Kind::Constant
        } else if lang.types.contains(&word) {
            Kind::Type
        } else {
            Kind::Ident
        };
        self.push(kind, i, end);
        end
    }

    /// Family-specific forms. Returns the end of what it consumed.
    fn special(&mut self, i: usize, fam: Family) -> Option<usize> {
        let c = self.b[i];
        let next = self.at(i + 1);
        match fam {
            Family::Rust => {
                if c == b'#' && (next == b'[' || (next == b'!' && self.at(i + 2) == b'[')) {
                    let end = self.brackets(i + if next == b'!' { 2 } else { 1 });
                    self.push(Kind::Attr, i, end);
                    return Some(end);
                }
                if c == b'\'' {
                    // A lifetime is `'a` not followed by a closing quote.
                    if is_ident_start(next) {
                        let end = self.ident_end(i + 1);
                        if self.at(end) != b'\'' {
                            self.push(Kind::Lifetime, i, end);
                            return Some(end);
                        }
                    }
                    let end = self.string(i + 1, b'\'', i);
                    self.push(Kind::Str, i, end);
                    return Some(end);
                }
                if c == b'r' || c == b'b' {
                    let mut j = i;
                    if self.at(j) == b'b' {
                        j += 1;
                    }
                    if self.at(j) == b'r' && matches!(self.at(j + 1), b'"' | b'#') {
                        let mut k = j + 1;
                        let mut hashes = 0;
                        while self.at(k) == b'#' {
                            hashes += 1;
                            k += 1;
                        }
                        if self.at(k) == b'"' {
                            let close = format!("\"{}", "#".repeat(hashes));
                            let end = self.b[k + 1..]
                                .windows(close.len())
                                .position(|w| w == close.as_bytes())
                                .map_or(self.b.len(), |p| k + 1 + p + close.len());
                            self.push(Kind::Str, i, end);
                            return Some(end);
                        }
                    } else if j > i && matches!(self.at(j), b'"' | b'\'') {
                        let end = self.string(j + 1, self.b[j], j);
                        self.push(Kind::Str, i, end);
                        return Some(end);
                    }
                }
                None
            }
            Family::C if c == b'#' && self.line_start => {
                let mut j = i + 1;
                while matches!(self.at(j), b' ' | b'\t') {
                    j += 1;
                }
                if !is_ident_start(self.at(j)) {
                    return None;
                }
                let end = self.ident_end(j);
                let word = &self.src[j..end];
                self.push(Kind::Keyword, i, end);
                if word == "include" || word == "import" {
                    let mut k = end;
                    while matches!(self.at(k), b' ' | b'\t') {
                        k += 1;
                    }
                    if self.at(k) == b'<' {
                        let close = self.b[k..]
                            .iter()
                            .position(|&c| c == b'>' || c == b'\n')
                            .map_or(self.b.len(), |p| k + p + 1);
                        self.push(Kind::Str, k, close);
                        return Some(close);
                    }
                }
                Some(end)
            }
            Family::Ruby if c == b':' && is_ident_start(next) && i > 0 && self.b[i - 1] != b':' => {
                let end = self.ident_end(i + 1);
                self.push(Kind::Constant, i, end);
                Some(end)
            }
            Family::Ruby if (c == b'@' || c == b'$') && (is_ident_start(next) || next == b'@') => {
                let start = if next == b'@' { i + 2 } else { i + 1 };
                if !is_ident_start(self.at(start)) {
                    return None;
                }
                let end = self.ident_end(start);
                self.push(Kind::Ident, i, end);
                Some(end)
            }
            Family::Shell if c == b'$' => {
                let start = if next == b'{' { i + 2 } else { i + 1 };
                if !is_ident_start(self.at(start)) {
                    return None;
                }
                let end = self.ident_end(start);
                self.push(Kind::Ident, start, end);
                Some(end)
            }
            Family::Lua if c == b'[' && next == b'[' => {
                let end = self.b[i..]
                    .windows(2)
                    .position(|w| w == b"]]")
                    .map_or(self.b.len(), |p| i + p + 2);
                self.push(Kind::Str, i, end);
                Some(end)
            }
            _ if self.lang.at_attributes && c == b'@' => {
                if self.lang.id == "csharp" && next == b'"' {
                    let end = self.string(i + 2, b'"', i + 1);
                    self.push(Kind::Str, i, end);
                    return Some(end);
                }
                if !is_ident_start(next) {
                    return None;
                }
                let mut end = self.ident_end(i + 1);
                while self.at(end) == b'.' && is_ident_start(self.at(end + 1)) {
                    end = self.ident_end(end + 1);
                }
                self.push(Kind::Attr, i, end);
                Some(end)
            }
            _ => None,
        }
    }

    /// From an opening `[`, past its matching `]`, skipping strings.
    fn brackets(&self, open: usize) -> usize {
        let n = self.b.len();
        let mut depth = 0;
        let mut j = open;
        while j < n {
            match self.b[j] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return j + 1;
                    }
                }
                b'"' => {
                    j = self.string(j + 1, b'"', j);
                    continue;
                }
                _ => {}
            }
            j += 1;
        }
        n
    }
}
