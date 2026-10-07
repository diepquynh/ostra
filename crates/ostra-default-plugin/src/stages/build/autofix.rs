//! Auto-fixable review findings, applied by the engine from their exact Fix text (Step 4 item 5):
//! ``Change `old` to `new` on line N.`` or ``Add `text` above line N: `anchor`.``

#[allow(unused_imports)]
use crate::prelude::*;

use ostra_core::submit::ReviewFinding;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    Replace {
        old: String,
        new: String,
        line: usize,
    },
    Add {
        text: String,
        line: usize,
        anchor: String,
    },
}

/// Backtick-delimited spans, in order. Double backticks delimit spans that contain a backtick.
fn spans(s: &str) -> Vec<(usize, usize, String)> {
    let mut out = vec![];
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let double = i + 1 < bytes.len() && bytes[i + 1] == b'`';
            let delim = if double { "``" } else { "`" };
            let start = i + delim.len();
            if let Some(rel) = s[start..].find(delim) {
                let end = start + rel;
                let mut inner = s[start..end].to_string();
                if double {
                    inner = inner.trim_matches(' ').to_string();
                }
                out.push((i, end + delim.len(), inner));
                i = end + delim.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

fn line_after(s: &str, marker: &str) -> Option<usize> {
    let idx = s.find(marker)?;
    let rest = &s[idx + marker.len()..];
    let digits: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

pub fn parse_fix(fix: &str) -> Option<Fix> {
    let text = fix.trim().trim_start_matches("Fix:").trim();
    let sp = spans(text);
    if text.starts_with("Change") && sp.len() >= 2 {
        let between = &text[sp[0].1..sp[1].0];
        if between.trim() != "to" {
            return None;
        }
        let tail = &text[sp[1].1..];
        let line = line_after(tail, "on line")?;
        return Some(Fix::Replace {
            old: sp[0].2.clone(),
            new: sp[1].2.clone(),
            line,
        });
    }
    if text.starts_with("Add") && sp.len() >= 2 {
        let between = &text[sp[0].1..sp[1].0];
        let line = line_after(between, "above line")?;
        return Some(Fix::Add {
            text: sp[0].2.clone(),
            line,
            anchor: sp[1].2.clone(),
        });
    }
    None
}

/// Apply one fix to file content. Lines are 1-based. The anchor or old text must be on the named
/// line, or within two lines of it, because earlier fixes in the same file may shift lines.
pub fn apply_fix(content: &str, fix: &Fix) -> Result<String, String> {
    let trailing_newline = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let near = |line: usize, needle: &str, lines: &[String]| -> Option<usize> {
        let target = line.checked_sub(1)?;
        let order = [0i64, -1, 1, -2, 2];
        order
            .iter()
            .map(|d| target as i64 + d)
            .filter(|i| *i >= 0 && (*i as usize) < lines.len())
            .map(|i| i as usize)
            .find(|i| lines[*i].contains(needle))
    };
    match fix {
        Fix::Replace { old, new, line } => {
            if old.is_empty() {
                return Err("the old text is empty".into());
            }
            let idx = near(*line, old, &lines)
                .ok_or_else(|| format!("`{old}` is not on or near line {line}"))?;
            if lines[idx].matches(old.as_str()).count() != 1 {
                return Err(format!(
                    "`{old}` appears more than once on line {}",
                    idx + 1
                ));
            }
            lines[idx] = lines[idx].replacen(old.as_str(), new, 1);
        }
        Fix::Add { text, line, anchor } => {
            let idx = near(*line, anchor.trim(), &lines)
                .ok_or_else(|| format!("the anchor is not on or near line {line}"))?;
            let indent: String = lines[idx]
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();
            let added = if text.starts_with(char::is_whitespace) {
                text.clone()
            } else {
                format!("{indent}{text}")
            };
            lines.insert(idx, added);
        }
    }
    let mut out = lines.join("\n");
    if trailing_newline {
        out.push('\n');
    }
    Ok(out)
}

/// Apply findings to files under `repo_root`. Returns (applied lines, failed (line, reason)).
/// Findings in one file are applied bottom-up so line numbers stay valid.
pub fn apply_findings(
    repo_root: &Path,
    findings: &[ReviewFinding],
) -> (Vec<String>, Vec<(String, String)>) {
    let mut applied = vec![];
    let mut failed = vec![];
    let mut by_file: std::collections::BTreeMap<String, Vec<(&ReviewFinding, Fix)>> =
        Default::default();
    for f in findings {
        match parse_fix(&f.fix) {
            Some(fix) => by_file.entry(f.file.clone()).or_default().push((f, fix)),
            None => failed.push((
                f.line(),
                "the Fix text is not in an auto-fixable form".to_string(),
            )),
        }
    }
    for (file, mut fixes) in by_file {
        let path = repo_root.join(&file);
        let normalized = ostra_core::paths::normalize(&path);
        if !ostra_core::paths::is_inside(repo_root, &normalized) {
            for (f, _) in fixes {
                failed.push((f.line(), "the file is outside the project".into()));
            }
            continue;
        }
        let Ok(mut content) = std::fs::read_to_string(&normalized) else {
            for (f, _) in fixes {
                failed.push((f.line(), format!("{file} could not be read")));
            }
            continue;
        };
        fixes.sort_by_key(|(_, fix)| {
            std::cmp::Reverse(match fix {
                Fix::Replace { line, .. } | Fix::Add { line, .. } => *line,
            })
        });
        let mut changed = false;
        for (f, fix) in fixes {
            match apply_fix(&content, &fix) {
                Ok(next) => {
                    content = next;
                    changed = true;
                    applied.push(f.line());
                }
                Err(e) => failed.push((f.line(), e)),
            }
        }
        if changed && let Err(e) = std::fs::write(&normalized, content) {
            failed.push((file, format!("write failed: {e}")));
        }
    }
    (applied, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_forms() {
        assert_eq!(
            parse_fix(
                "Change `void process(UUID id) {` to `void process(final UUID id) {` on line 45."
            ),
            Some(Fix::Replace {
                old: "void process(UUID id) {".into(),
                new: "void process(final UUID id) {".into(),
                line: 45
            })
        );
        assert_eq!(
            parse_fix(
                "Fix: Add `// Publishes the event` above line 88: `this.publisher.publish(event);`."
            ),
            Some(Fix::Add {
                text: "// Publishes the event".into(),
                line: 88,
                anchor: "this.publisher.publish(event);".into()
            })
        );
        assert_eq!(parse_fix("Make the parameter immutable."), None);
        assert_eq!(
            parse_fix("Change ``a `b` c`` to ``d`` on line 3."),
            Some(Fix::Replace {
                old: "a `b` c".into(),
                new: "d".into(),
                line: 3
            })
        );
    }

    #[test]
    fn applies_with_drift() {
        let src = "a\nb\n  let x = 1;\nd\n";
        let out = apply_fix(
            src,
            &Fix::Replace {
                old: "let x".into(),
                new: "let y".into(),
                line: 2,
            },
        )
        .unwrap();
        assert_eq!(out, "a\nb\n  let y = 1;\nd\n");
        let out = apply_fix(
            src,
            &Fix::Add {
                text: "// note".into(),
                line: 3,
                anchor: "let x = 1;".into(),
            },
        )
        .unwrap();
        assert_eq!(out, "a\nb\n  // note\n  let x = 1;\nd\n");
        assert!(
            apply_fix(
                src,
                &Fix::Replace {
                    old: "zzz".into(),
                    new: "q".into(),
                    line: 1
                }
            )
            .is_err()
        );
    }

    #[test]
    fn applies_bottom_up_in_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.rs"), "one\ntwo\nthree\n").unwrap();
        let mk = |fix: &str| ReviewFinding {
            severity: ostra_core::submit::Severity::Low,
            file: "f.rs".into(),
            rule: "C1".into(),
            description: "d".into(),
            fix: fix.into(),
            guidance: None,
        };
        let (ok, bad) = apply_findings(
            dir.path(),
            &[
                mk("Add `// top` above line 1: `one`."),
                mk("Change `three` to `3` on line 3."),
                mk("vague"),
            ],
        );
        assert_eq!(ok.len(), 2);
        assert_eq!(bad.len(), 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.rs")).unwrap(),
            "// top\none\ntwo\n3\n"
        );
    }
}
