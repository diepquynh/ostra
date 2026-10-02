//! Rule Hard 4: the paths and `path:Symbol` references in a document's typed fields are checked
//! against the repo when the document is written and when it is submitted, so the agent that wrote
//! a wrong reference fixes it, not a fact-check round. The browser view does not run these checks,
//! because a build later creates and deletes the files they name.

use super::check::Issues;
use super::{FileChange, FileSnapshot, PlanDoc, ResearchDoc, SpecDoc};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// The fingerprint of a path that does not exist.
pub(crate) const MISSING: &str = "missing";

/// A reference to code: a repo-relative path, and the identifier its symbol ends in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeRef {
    pub path: String,
    /// `cancel` in `src/order.rs:Order::cancel`. `None` for a bare path or a `path:line`.
    pub symbol: Option<String>,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn looks_like_path(p: &str) -> bool {
    if p.is_empty() || p.chars().any(|c| "*?[]{}<>|\"'".contains(c)) {
        return false;
    }
    if p.contains('/') || p.contains('\\') {
        return true;
    }
    // A bare file name needs an extension that starts with a letter, so `v1.2` is not a path.
    p.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && (1..=10).contains(&ext.len())
            && ext.starts_with(|c: char| c.is_ascii_alphabetic())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

/// Parse `path`, `path:Symbol`, or `path:line`, ignoring prose after the first space. `None` for
/// a URL, a `new:` grounding, a glob, a placeholder, or text that does not start with a path.
pub(crate) fn parse_ref(text: &str) -> Option<CodeRef> {
    let head = text.trim().split_whitespace().next()?.trim_matches('`');
    if head.contains("://") || head.to_ascii_lowercase().starts_with("new:") {
        return None;
    }
    // A Windows drive letter's colon belongs to the path.
    let bytes = head.as_bytes();
    let skip = if bytes.len() > 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        2
    } else {
        0
    };
    let (path, rest) = match head[skip..].find(':') {
        Some(i) => (&head[..skip + i], &head[skip + i + 1..]),
        None => (head, ""),
    };
    let path = path
        .trim_start_matches("./")
        .trim_end_matches([',', ';', ')', '.']);
    looks_like_path(path).then(|| CodeRef {
        path: path.to_string(),
        symbol: symbol_of(rest),
    })
}

/// The identifier a symbol reference ends in: `cancel` for `Order::cancel(&self)`. `None` for a
/// line number or a name too short to search for.
fn symbol_of(rest: &str) -> Option<String> {
    let rest = rest.split('(').next().unwrap_or_default().trim();
    let digits: String = rest.chars().filter(|c| !matches!(c, 'L' | 'l')).collect();
    if !digits.is_empty()
        && digits
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == ':')
    {
        return None;
    }
    let last = rest
        .split(|c: char| !is_ident(c))
        .rfind(|s| !s.is_empty())?;
    (last.chars().count() >= 2 && !last.chars().all(|c| c.is_ascii_digit()))
        .then(|| last.to_string())
}

/// Whether `word` occurs in `text` with no identifier character on either side.
fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

fn file_has_word(path: &Path, word: &str) -> bool {
    std::fs::read(path)
        .map(|b| contains_word(&String::from_utf8_lossy(&b), word))
        .unwrap_or(true)
}

enum Found {
    Missing,
    /// The path exists; `Some(false)` when its symbol does not appear in it.
    Present(Option<bool>),
}

fn find(root: &Path, r: &CodeRef) -> Found {
    let path = root.join(&r.path);
    if path.is_dir() {
        return Found::Present(None);
    }
    if !path.is_file() {
        return Found::Missing;
    }
    Found::Present(r.symbol.as_ref().map(|s| file_has_word(&path, s)))
}

/// A content hash of the file at `path`, `dir` for a directory, or [`MISSING`].
pub(crate) fn fingerprint(path: &Path) -> String {
    if path.is_dir() {
        return "dir".into();
    }
    match std::fs::read(path) {
        Ok(bytes) => Sha256::digest(&bytes)[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        Err(_) => MISSING.into(),
    }
}

/// Every code reference a research document makes, with the element it sits in.
pub(crate) fn research_refs(d: &ResearchDoc) -> Vec<(String, CodeRef)> {
    let mut out = vec![];
    let mut push = |at: &str, text: &str, keep_symbol: bool| {
        if let Some(mut r) = parse_ref(text) {
            if !keep_symbol {
                r.symbol = None;
            }
            out.push((at.to_string(), r));
        }
    };
    // `symbols` are verbatim signatures, which no word search can confirm, so a file is checked
    // by its path alone.
    for f in &d.files {
        push(&f.path, &f.path, false);
    }
    for p in &d.patterns {
        for f in &p.files {
            push(&p.name, f, false);
        }
        if let Some(src) = p.snippet.as_ref().and_then(|s| s.source.as_deref()) {
            push(&p.name, src, false);
        }
    }
    for hop in &d.data_flow {
        if let Some(l) = &hop.location {
            push("data flow", l, true);
        }
    }
    for a in &d.approaches {
        push(&a.name, &a.precedent, true);
    }
    out
}

/// Rule D2a: the files a research document names, fingerprinted against `root` as they are now.
pub(crate) fn snapshot(d: &ResearchDoc, root: &Path) -> FileSnapshot {
    let files = research_refs(d)
        .into_iter()
        .map(|(_, r)| r.path)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|p| {
            let f = fingerprint(&root.join(&p));
            (p, f)
        })
        .collect();
    FileSnapshot {
        root: root.display().to_string(),
        files,
    }
}

pub(crate) fn research(d: &ResearchDoc, out: &mut Issues) {
    let Some(root) = d.snapshot.as_ref().map(|s| PathBuf::from(&s.root)) else {
        return;
    };
    if !root.is_dir() {
        return;
    }
    let mut reported = HashSet::new();
    for (at, r) in research_refs(d) {
        match find(&root, &r) {
            Found::Missing if reported.insert(r.path.clone()) => out.error(
                Some(at),
                format!(
                    "Name a file that exists in `{}`: `{}` is not there. Correct the path, or drop the entry.",
                    d.repo, r.path
                ),
            ),
            Found::Present(Some(false)) => out.warn(
                Some(at),
                format!(
                    "Spell the symbol as the code does: `{}` does not appear in `{}`.",
                    r.symbol.unwrap_or_default(),
                    r.path
                ),
            ),
            _ => {}
        }
    }
}

pub(crate) fn spec(d: &SpecDoc, out: &mut Issues) {
    let roots: BTreeMap<&str, &Path> = d
        .repos
        .iter()
        .map(|r| (r.key.as_str(), Path::new(r.root.as_str())))
        .filter(|(_, root)| root.is_dir())
        .collect();
    if roots.is_empty() {
        return;
    }
    for c in &d.criteria {
        // Rule O2: a criterion for a project the spec introduces has nothing on disk yet.
        if !roots.contains_key(c.repo.as_str()) {
            continue;
        }
        let Some(r) = parse_ref(&c.grounding) else {
            continue;
        };
        // A criterion may rest on code in another repo, such as the endpoint a client consumes.
        match find_any(&roots, &r) {
            Found::Missing => out.error(
                Some(c.id.clone()),
                format!(
                    "Ground the criterion in a file that exists: `{}` is not in `{}` or another repo in scope. Use the real `path:Symbol`, or `new: no precedent found` when the repo has no counterpart.",
                    r.path, c.repo
                ),
            ),
            Found::Present(Some(false)) => out.error(
                Some(c.id.clone()),
                format!(
                    "Name a symbol that `{}` holds: `{}` does not appear in it.",
                    r.path,
                    r.symbol.unwrap_or_default()
                ),
            ),
            Found::Present(_) => {}
        }
    }
    for cc in &d.contracts_consumed {
        let Some(r) = parse_ref(&cc.source) else {
            continue;
        };
        match find_any(&roots, &r) {
            Found::Missing => out.error(
                Some(cc.name.clone()),
                format!(
                    "Cite the consumed contract's real source: `{}` is in no repo in scope.",
                    r.path
                ),
            ),
            Found::Present(Some(false)) => out.error(
                Some(cc.name.clone()),
                format!(
                    "Name a symbol that `{}` holds: `{}` does not appear in it.",
                    r.path,
                    r.symbol.unwrap_or_default()
                ),
            ),
            Found::Present(_) => {}
        }
    }
}

/// The best answer over every repo: the file with the symbol, else the file, else missing.
fn find_any(roots: &BTreeMap<&str, &Path>, r: &CodeRef) -> Found {
    let mut best = Found::Missing;
    for root in roots.values() {
        match find(root, r) {
            Found::Present(Some(false)) => best = Found::Present(Some(false)),
            Found::Missing => {}
            hit => return hit,
        }
    }
    best
}

pub(crate) fn plan(d: &PlanDoc, out: &mut Issues) {
    let roots: Vec<&Path> = d
        .phases
        .iter()
        .map(|p| Path::new(p.repo_root.as_str()))
        .filter(|root| root.is_dir())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // Absolute paths earlier steps create, and those they create or modify.
    let mut created: HashSet<PathBuf> = HashSet::new();
    let mut touched: HashSet<PathBuf> = HashSet::new();
    for p in &d.phases {
        let root = Path::new(p.repo_root.as_str());
        // Rule O2: a project the plan creates has nothing on disk to check yet.
        if !root.is_dir() {
            continue;
        }
        for s in &p.steps {
            let at = Some(format!("step {}", s.id));
            let target = root.join(s.file.trim().trim_start_matches("./"));
            if matches!(s.change, FileChange::Modify | FileChange::Delete)
                && !target.exists()
                && !created.contains(&target)
            {
                out.error(
                    at.clone(),
                    format!(
                        "Use `Create`, or fix the path: the step changes `{}`, which is not in `{}` and no earlier step creates it.",
                        s.file, p.repo
                    ),
                );
            }
            for entry in &s.read_first {
                let Some(r) = parse_ref(entry) else {
                    continue;
                };
                // A path from another repo in the plan counts where it exists.
                let hit = std::iter::once(root)
                    .chain(roots.iter().copied().filter(|other| *other != root))
                    .map(|base| base.join(&r.path))
                    .find(|abs| {
                        abs.exists()
                            || created.contains(abs)
                            || (*abs == target && s.change == FileChange::Create)
                    });
                match hit {
                    None => out.error(
                        at.clone(),
                        format!(
                            "Fix `read_first`: `{}` is not in `{}`, and no earlier step creates it.",
                            r.path, p.repo
                        ),
                    ),
                    // A symbol an earlier step adds is not in the file yet.
                    Some(abs) if abs.is_file() && !touched.contains(&abs) => {
                        if let Some(sym) = r.symbol.as_deref()
                            && !file_has_word(&abs, sym)
                        {
                            out.warn(
                                at.clone(),
                                format!(
                                    "Spell the symbol as the code does: `{sym}` does not appear in `{}`.",
                                    r.path
                                ),
                            );
                        }
                    }
                    Some(_) => {}
                }
            }
            match s.change {
                FileChange::Create => {
                    created.insert(target.clone());
                    touched.insert(target);
                }
                FileChange::Modify => {
                    touched.insert(target);
                }
                FileChange::Delete => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(path: &str, symbol: Option<&str>) -> Option<CodeRef> {
        Some(CodeRef {
            path: path.into(),
            symbol: symbol.map(String::from),
        })
    }

    #[test]
    fn parses_paths_symbols_and_lines() {
        assert_eq!(parse_ref("src/order.rs"), r("src/order.rs", None));
        assert_eq!(
            parse_ref("`src/order.rs:Order::cancel`"),
            r("src/order.rs", Some("cancel"))
        );
        assert_eq!(
            parse_ref("src/UserService.java:UserService.findById(Long id)"),
            r("src/UserService.java", Some("findById"))
        );
        assert_eq!(parse_ref("src/a.ts:42"), r("src/a.ts", None));
        assert_eq!(parse_ref("src/a.ts:L42-L50"), r("src/a.ts", None));
        assert_eq!(
            parse_ref("./src/order.rs (the Order struct)"),
            r("src/order.rs", None)
        );
        assert_eq!(parse_ref("Cargo.toml"), r("Cargo.toml", None));
        assert_eq!(
            parse_ref(r"C:\repo\src\a.rs:run"),
            r(r"C:\repo\src\a.rs", Some("run"))
        );
        for none in [
            "new: no precedent found",
            "https://docs.rs/x",
            "src/**/*.rs",
            "the Order struct in src/order.rs",
            "v1.2",
            "",
        ] {
            assert_eq!(parse_ref(none), None, "{none}");
        }
    }

    #[test]
    fn a_word_match_needs_boundaries() {
        assert!(contains_word("fn cancel(&self)", "cancel"));
        assert!(!contains_word("fn cancelled()", "cancel"));
        assert!(!contains_word("precancel", "cancel"));
    }
}
