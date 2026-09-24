//! Markdown outlines: ATX and setext headings outside fenced code, with GitHub-style anchor ids.

use crate::api::Heading;
use std::collections::HashMap;

/// Every heading of `markdown` in document order. Headings inside fenced code blocks, indented
/// code, and a leading YAML front matter block are skipped.
pub fn headings(markdown: &str) -> Vec<Heading> {
    let mut slugs = Slugger::default();
    let mut out = vec![];
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = front_matter_end(&lines);
    let mut fence: Option<(char, usize)> = None;
    // The open paragraph, which a setext underline turns into a heading.
    let mut paragraph: Option<String> = None;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if let Some((ch, len)) = fence {
            if closes_fence(line, ch, len) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = opens_fence(line) {
            fence = Some(open);
            paragraph = None;
            continue;
        }
        if line.trim().is_empty() {
            paragraph = None;
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent >= 4 && paragraph.is_none() {
            continue;
        }
        if let Some((level, text)) = atx(line) {
            out.extend(heading(&mut slugs, level, text));
            paragraph = None;
            continue;
        }
        if let Some(level) = setext_level(line)
            && let Some(text) = paragraph.take()
        {
            out.extend(heading(&mut slugs, level, &text));
            continue;
        }
        paragraph = match paragraph {
            Some(mut p) => {
                p.push(' ');
                p.push_str(line.trim());
                Some(p)
            }
            None if starts_block(line) => None,
            None => Some(line.trim().to_string()),
        };
    }
    out
}

/// `None` for a heading with no text, which has nothing to show in an outline.
fn heading(slugs: &mut Slugger, level: u8, raw: &str) -> Option<Heading> {
    let title = inline_text(raw.trim());
    (!title.is_empty()).then(|| Heading {
        id: slugs.slug(&title),
        level,
        title,
    })
}

fn front_matter_end(lines: &[&str]) -> usize {
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return 0;
    }
    lines
        .iter()
        .skip(1)
        .position(|l| matches!(l.trim_end(), "---" | "..."))
        .map(|p| p + 2)
        .unwrap_or(0)
}

fn opens_fence(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let ch = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = t.chars().take_while(|c| *c == ch).count();
    // A backtick fence's info string cannot hold a backtick.
    (len >= 3 && !(ch == '`' && t[len..].contains('`'))).then_some((ch, len))
}

fn closes_fence(line: &str, ch: char, open: usize) -> bool {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return false;
    }
    let len = t.chars().take_while(|c| *c == ch).count();
    len >= open && t[len * ch.len_utf8()..].trim().is_empty()
}

fn atx(line: &str) -> Option<(u8, &str)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let level = t.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut text = rest.trim();
    // A closing sequence of `#` counts only after a space, or as the whole text.
    let stripped = text.trim_end_matches('#');
    if stripped.is_empty() || stripped.ends_with([' ', '\t']) {
        text = stripped.trim_end();
    }
    Some((level as u8, text))
}

fn setext_level(line: &str) -> Option<u8> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let t = t.trim_end();
    let ch = t.chars().next()?;
    let level = match ch {
        '=' => 1,
        '-' => 2,
        _ => return None,
    };
    t.chars().all(|c| c == ch).then_some(level)
}

/// Lines that start a block other than a paragraph, so the next `---` is not a setext underline.
fn starts_block(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('>')
        || t.starts_with("- ")
        || t.starts_with("* ")
        || t.starts_with("+ ")
        || t.starts_with('|')
        || t.starts_with('<')
        || setext_level(line).is_some()
        || t.split_once(". ").is_some_and(|(n, _)| {
            !n.is_empty() && n.len() <= 9 && n.chars().all(|c| c.is_ascii_digit())
        })
}

/// The text a reader sees: code spans, emphasis, links, and images reduced to their text.
fn inline_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() => {
                out.push(chars[i + 1]);
                i += 2;
            }
            '`' => {
                let run = chars[i..].iter().take_while(|c| **c == '`').count();
                let body_start = i + run;
                let close = (body_start..chars.len()).find(|&j| {
                    chars[j..].iter().take_while(|c| **c == '`').count() == run
                        && (j == 0 || chars[j - 1] != '`')
                });
                match close {
                    Some(j) => {
                        let body: String = chars[body_start..j].iter().collect();
                        out.push_str(body.trim());
                        i = j + run;
                    }
                    None => {
                        out.extend(&chars[i..body_start]);
                        i = body_start;
                    }
                }
            }
            '*' => i += 1,
            '!' if chars.get(i + 1) == Some(&'[') => i += 1,
            '[' => match link_end(&chars, i) {
                Some((text_end, end)) => {
                    out.push_str(&inline_text(
                        &chars[i + 1..text_end].iter().collect::<String>(),
                    ));
                    i = end;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            },
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// For `[text](url)` starting at `open`: the index of `]` and the index after `)`.
fn link_end(chars: &[char], open: usize) -> Option<(usize, usize)> {
    let close = (open + 1..chars.len()).find(|&j| chars[j] == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = (close + 2..chars.len()).find(|&j| chars[j] == ')')?;
    Some((close, end + 1))
}

/// GitHub's anchor slugs (github-slugger): lowercase, drop punctuation and symbols, spaces to
/// dashes, and number repeats `-1`, `-2`.
#[derive(Default)]
pub struct Slugger {
    seen: HashMap<String, u32>,
}

impl Slugger {
    pub fn slug(&mut self, title: &str) -> String {
        let base = slugify(title);
        let mut slug = base.clone();
        while self.seen.contains_key(&slug) {
            let n = self.seen.entry(base.clone()).or_default();
            *n += 1;
            slug = format!("{base}-{n}");
        }
        self.seen.insert(slug.clone(), 0);
        slug
    }
}

pub fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(md: &str) -> Vec<(u8, String, String)> {
        headings(md)
            .into_iter()
            .map(|h| (h.level, h.id, h.title))
            .collect()
    }

    fn h(level: u8, id: &str, title: &str) -> (u8, String, String) {
        (level, id.into(), title.into())
    }

    #[test]
    fn slugs_follow_github() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("Phase 1: greeting"), "phase-1-greeting");
        assert_eq!(
            slugify("snake_case and kebab-case"),
            "snake_case-and-kebab-case"
        );
        assert_eq!(slugify("Two  spaces"), "two--spaces");
        assert_eq!(slugify("Ünïcode Straße"), "ünïcode-straße");
        assert_eq!(slugify("C++ & Rust (2024)"), "c--rust-2024");
        let mut s = Slugger::default();
        let got: Vec<String> = ["Reqs", "Reqs", "Reqs-1", "Reqs"]
            .iter()
            .map(|t| s.slug(t))
            .collect();
        assert_eq!(got, ["reqs", "reqs-1", "reqs-1-1", "reqs-2"]);
    }

    #[test]
    fn atx_and_setext_headings_outside_code() {
        let md = "---\ntitle: front\n---\n# Spec: `cancel` orders #\n\nText\n\n## Requirements\n### Requirements\n\
                  ```rust\n# not a heading\n```\n~~~~\n## also not\n~~~~~\n    # indented code\n#no-space\n\n\
                  Setext title\n============\n\nSecond *level*\n---\n\n---\n- item\n---\n####### seven\n## [Link](http://x) ##\n#";
        assert_eq!(
            outline(md),
            vec![
                h(1, "spec-cancel-orders", "Spec: cancel orders"),
                h(2, "requirements", "Requirements"),
                h(3, "requirements-1", "Requirements"),
                h(1, "setext-title", "Setext title"),
                h(2, "second-level", "Second level"),
                h(2, "link", "Link"),
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_hides_the_rest() {
        assert_eq!(outline("# One\n```\n# Two\n"), vec![h(1, "one", "One")]);
        assert_eq!(
            outline("First line\nsecond line\n---"),
            vec![h(2, "first-line-second-line", "First line second line")]
        );
        assert_eq!(
            outline("# One\n````\n```\n# Two\n````\n# Three"),
            vec![h(1, "one", "One"), h(1, "three", "Three")]
        );
    }
}
