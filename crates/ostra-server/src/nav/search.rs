//! `GET /api/workspaces/:ws/search?q=&limit=`: one ranked list across sessions, executions,
//! artifacts, project files, projects, lessons, and settings keys.
//!
//! Session titles and requests and artifact headings come from the workspace db's FTS5 index.
//! Executions, artifact names, projects, and settings keys are matched in memory, file names come
//! from each project's file name index, and lessons from each project's memory FTS. Every source
//! scores its matches from 0 to 1 by match quality, so one sort merges them.

use crate::api::ApiErr;
use crate::app::App;
use crate::workspace::WorkspaceRt;
use ostra_core::api::{FileIndex, SearchHit, SearchKind, SearchResults, TreeSession};
use ostra_core::config::SETTING_KEYS;
use ostra_core::paths;
use ostra_store::MemoryStore;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 200;
/// Lessons read per project.
const LESSONS_PER_PROJECT: usize = 20;
const HINT_CHARS: usize = 80;
/// Text-index rows read before merging.
const TEXT_ROWS: usize = 200;
/// A match the index found by word prefix that the in-memory scorer cannot place.
const INDEX_ONLY: f64 = 0.45;

/// A query, lowercased once.
pub struct Needle {
    q: String,
    words: Vec<String>,
}

impl Needle {
    pub fn new(q: &str) -> Self {
        let q = q.trim().to_lowercase();
        let words = q.split_whitespace().map(str::to_string).collect();
        Needle { q, words }
    }

    pub fn is_empty(&self) -> bool {
        self.q.is_empty()
    }

    /// How well `text` matches, from 0.3 (a subsequence) to 1.0 (equal, ignoring case).
    pub fn text_score(&self, text: &str) -> Option<f64> {
        if self.q.is_empty() || text.is_empty() {
            return None;
        }
        let hay = lower(text);
        let (h, n) = (hay.as_bytes(), self.q.as_bytes());
        let base = if h == n {
            1.0
        } else if h.starts_with(n) {
            0.9
        } else if find_at_word(h, n) {
            0.8
        } else if find(h, n).is_some() {
            0.65
        } else if self.words.len() > 1 && self.words.iter().all(|w| find(h, w.as_bytes()).is_some())
        {
            0.55
        } else if subsequence(h, n) {
            0.3
        } else {
            return None;
        };
        Some(base - length_penalty(h.len(), n.len()))
    }

    /// How well a project-relative file path matches: exact file name, file name prefix, file
    /// name substring, path substring, then subsequence.
    pub fn file_score(&self, path: &str) -> Option<f64> {
        let hay = lower(path);
        let h = hay.as_bytes();
        let n = self.q.as_bytes();
        let name = &h[h
            .iter()
            .rposition(|b| *b == b'/')
            .map(|i| i + 1)
            .unwrap_or(0)..];
        let base = if name == n {
            1.0
        } else if name.starts_with(n) {
            0.88
        } else if find(name, n).is_some() {
            0.74
        } else if find(h, n).is_some() {
            0.6
        } else if subsequence(name, n) {
            0.4
        } else if subsequence(h, n) {
            0.3
        } else {
            return None;
        };
        Some(base - length_penalty(h.len(), n.len()))
    }
}

/// Shorter matches rank first among equals, by at most 0.05.
fn length_penalty(hay: usize, needle: usize) -> f64 {
    (hay.saturating_sub(needle) as f64 * 0.0005).min(0.05)
}

fn lower(s: &str) -> Cow<'_, str> {
    if s.bytes().any(|b| b.is_ascii_uppercase() || !b.is_ascii()) {
        Cow::Owned(s.to_lowercase())
    } else {
        Cow::Borrowed(s)
    }
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    if n.is_empty() || n.len() > h.len() {
        return None;
    }
    h.windows(n.len()).position(|w| w == n)
}

fn find_at_word(h: &[u8], n: &[u8]) -> bool {
    if n.is_empty() || n.len() > h.len() {
        return false;
    }
    h.windows(n.len())
        .enumerate()
        .any(|(i, w)| w == n && (i == 0 || !h[i - 1].is_ascii_alphanumeric()))
}

fn subsequence(h: &[u8], n: &[u8]) -> bool {
    let mut want = n.iter().filter(|b| !b.is_ascii_whitespace()).peekable();
    for b in h {
        if want.peek() == Some(&b) {
            want.next();
        }
    }
    want.peek().is_none()
}

fn clip(text: &str) -> String {
    ostra_core::api::summary_line(text)
        .chars()
        .take(HINT_CHARS)
        .collect()
}

fn session_label(title: Option<&str>, request: &str) -> String {
    match title {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => clip(request),
    }
}

/// The highest of the scores that matched.
fn best<const N: usize>(scores: [Option<f64>; N]) -> Option<f64> {
    scores.into_iter().flatten().reduce(f64::max)
}

fn hit(kind: SearchKind, id: String, label: String, hint: Option<String>, score: f64) -> SearchHit {
    SearchHit {
        kind,
        id,
        label,
        hint,
        score,
    }
}

/// The best `limit` matching files of one project, best first.
pub fn rank_files(needle: &Needle, key: &str, index: &FileIndex, limit: usize) -> Vec<SearchHit> {
    let mut scored: Vec<(f64, &str)> = index
        .paths
        .iter()
        .filter_map(|p| needle.file_score(p).map(|s| (s, p.as_str())))
        .collect();
    let by_score =
        |a: &(f64, &str), b: &(f64, &str)| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1));
    if scored.len() > limit && limit > 0 {
        scored.select_nth_unstable_by(limit - 1, by_score);
        scored.truncate(limit);
    }
    scored.sort_by(by_score);
    scored
        .into_iter()
        .map(|(score, path)| {
            let name = path.rsplit('/').next().unwrap_or(path).to_string();
            hit(
                SearchKind::File,
                format!("file:{key}:{path}"),
                name,
                Some(format!("{key}/{path}")),
                score,
            )
        })
        .collect()
}

/// Executions and artifact file names from the Sessions tree.
pub fn rank_tree(needle: &Needle, sessions: &[TreeSession]) -> Vec<SearchHit> {
    let mut out = vec![];
    for s in sessions {
        let owner = session_label(s.title.as_deref(), &s.request);
        for g in &s.groups {
            for r in &g.runs {
                let label = format!("{} · {}", g.agent, r.run_label);
                let score = best([
                    needle.text_score(g.agent.as_str()),
                    needle.text_score(&r.run_label),
                    needle.text_score(&format!("{} {}", g.agent, r.run_label)),
                ]);
                if let Some(score) = score {
                    out.push(hit(
                        SearchKind::Execution,
                        format!("exec:{}", r.id),
                        label,
                        Some(format!("{owner} · {}", g.project)),
                        score + 0.02,
                    ));
                }
            }
        }
        for a in &s.artifacts {
            let name = a
                .path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            let score = best([
                needle.text_score(&a.label),
                needle.text_score(&name).map(|s| s * 0.95),
            ]);
            if let Some(score) = score {
                out.push(hit(
                    SearchKind::Artifact,
                    format!("artifact:{}", a.path.display()),
                    a.label.clone(),
                    Some(owner.clone()),
                    score + 0.03,
                ));
            }
        }
    }
    out
}

/// Projects by key and folder name, and settings keys by key and description.
pub fn rank_static(needle: &Needle, projects: &[(String, std::path::PathBuf)]) -> Vec<SearchHit> {
    let mut out = vec![];
    for (key, path) in projects {
        let folder = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        if let Some(score) = best([
            needle.text_score(key),
            needle.text_score(&folder).map(|s| s * 0.9),
        ]) {
            out.push(hit(
                SearchKind::Project,
                format!("project:{key}"),
                key.clone(),
                Some(path.display().to_string()),
                score + 0.03,
            ));
        }
    }
    for (key, about) in SETTING_KEYS {
        let last = key.rsplit('.').next().unwrap_or(key);
        let score = best([
            needle.text_score(key),
            needle.text_score(last).map(|s| s * 0.95),
            needle.text_score(about).map(|s| s * 0.85),
        ]);
        if let Some(score) = score {
            out.push(hit(
                SearchKind::Setting,
                format!("setting:{key}"),
                key.to_string(),
                Some(about.to_string()),
                score,
            ));
        }
    }
    out
}

/// Sort by score, keep the best hit per id, and cut to `limit`.
pub fn merge(mut hits: Vec<SearchHit>, limit: usize) -> Vec<SearchHit> {
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.label.len().cmp(&b.label.len()))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut seen = std::collections::HashSet::new();
    hits.retain(|h| seen.insert(h.id.clone()));
    hits.truncate(limit);
    hits
}

/// Sessions and artifacts from the text index.
fn rank_text(
    needle: &Needle,
    w: &WorkspaceRt,
    q: &str,
    sessions: &HashMap<String, String>,
) -> Result<Vec<SearchHit>, ApiErr> {
    let mut out = vec![];
    for (i, t) in w.db.search_text(q, TEXT_ROWS)?.into_iter().enumerate() {
        let order = i as f64 * 0.0001;
        match t.kind.as_str() {
            "session" => {
                let title = (!t.label.is_empty()).then_some(t.label.as_str());
                let score = best([
                    needle.text_score(&t.label),
                    needle.text_score(&t.body).map(|s| s * 0.95),
                ])
                .unwrap_or(INDEX_ONLY);
                let hint = title.map(|_| clip(&t.body));
                out.push(hit(
                    SearchKind::Session,
                    format!("session:{}", t.reference),
                    session_label(title, &t.body),
                    hint,
                    score + 0.04 - order,
                ));
            }
            "artifact" => {
                let heading = t
                    .body
                    .lines()
                    .skip(1)
                    .filter_map(|h| needle.text_score(h).map(|s| (s, h)))
                    .max_by(|a, b| a.0.total_cmp(&b.0));
                let label_score = needle.text_score(&t.label);
                let score =
                    best([label_score, heading.map(|(s, _)| s * 0.95)]).unwrap_or(INDEX_ONLY);
                let owner = sessions
                    .get(t.session.as_str())
                    .cloned()
                    .unwrap_or_default();
                let hint = match heading {
                    Some((s, h)) if label_score.is_none_or(|l| s > l) => format!("{owner} · {h}"),
                    _ => owner,
                };
                out.push(hit(
                    SearchKind::Artifact,
                    format!("artifact:{}", t.reference),
                    t.label,
                    Some(hint),
                    score + 0.03 - order,
                ));
            }
            _ => {}
        }
    }
    Ok(out)
}

fn rank_lessons(
    needle: &Needle,
    q: &str,
    projects: &[(String, std::path::PathBuf)],
) -> Vec<SearchHit> {
    let mut out = vec![];
    for (key, path) in projects {
        let db = paths::project_memory_db(path);
        // Opening a store creates its file, and search must not write into a project.
        if !db.exists() {
            continue;
        }
        let Ok(store) = MemoryStore::open(&db) else {
            continue;
        };
        for (i, l) in store
            .list(Some(q), LESSONS_PER_PROJECT, 0)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            let score = best([
                needle.text_score(&l.lesson),
                needle.text_score(&l.area).map(|s| s * 0.9),
            ])
            .unwrap_or(INDEX_ONLY)
                - i as f64 * 0.0001;
            out.push(hit(
                SearchKind::Lesson,
                format!("lesson:{key}:{}", l.id),
                clip(&l.lesson),
                Some(format!("{key} · {}", l.area)),
                score,
            ));
        }
    }
    out
}

/// Recent sessions, for an empty query.
fn recent(w: &WorkspaceRt, limit: usize) -> Result<Vec<SearchHit>, ApiErr> {
    let mut sessions = w.db.list_sessions()?;
    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    Ok(sessions
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(i, s)| {
            let hint = s.title.as_ref().map(|_| clip(&s.request));
            hit(
                SearchKind::Session,
                format!("session:{}", s.id),
                session_label(s.title.as_deref(), &s.request),
                hint,
                1.0 - i as f64 * 0.001,
            )
        })
        .collect())
}

pub async fn search(
    app: &Arc<App>,
    w: &Arc<WorkspaceRt>,
    q: &str,
    limit: Option<usize>,
) -> Result<SearchResults, ApiErr> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let needle = Needle::new(q);
    if needle.is_empty() {
        return Ok(SearchResults {
            items: recent(w, limit)?,
        });
    }
    let projects: Vec<(String, std::path::PathBuf)> = w
        .settings()
        .projects
        .into_iter()
        .map(|p| (p.key, p.path))
        .collect();
    let mut hits = rank_static(&needle, &projects);
    let (a, wt, text, plist) = (app.clone(), w.clone(), q.to_string(), projects.clone());
    let blocking = tokio::task::spawn_blocking(move || -> Result<Vec<SearchHit>, ApiErr> {
        let needle = Needle::new(&text);
        let tree = a.nav.tree(&wt)?;
        let owners: HashMap<String, String> = tree
            .sessions
            .iter()
            .map(|s| {
                (
                    s.id.to_string(),
                    session_label(s.title.as_deref(), &s.request),
                )
            })
            .collect();
        let mut out = rank_tree(&needle, &tree.sessions);
        out.extend(rank_text(&needle, &wt, &text, &owners)?);
        out.extend(rank_lessons(&needle, &text, &plist));
        Ok(out)
    });
    for (key, _) in &projects {
        if let Ok(index) = app.files.index(w, key).await {
            hits.extend(rank_files(&needle, key, &index, limit));
        }
    }
    hits.extend(blocking.await.map_err(|e| {
        ApiErr::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })??);
    Ok(SearchResults {
        items: merge(hits, limit),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(paths: &[&str]) -> FileIndex {
        FileIndex {
            paths: paths.iter().map(|p| p.to_string()).collect(),
            truncated: false,
        }
    }

    #[test]
    fn text_scores_order_by_match_quality() {
        let n = Needle::new("Order");
        let s = |t: &str| n.text_score(t).unwrap_or(0.0);
        assert!(s("order") > s("Orders page"));
        assert!(
            s("Orders page") > s("Cancel order"),
            "a prefix beats a later word"
        );
        assert!(
            s("Cancel order") > s("reorder items"),
            "a word start beats a substring"
        );
        assert!(
            s("reorder items") > s("o r d e r"),
            "a substring beats a subsequence"
        );
        assert_eq!(n.text_score("unrelated"), None);
        assert!(
            Needle::new("cancel refund")
                .text_score("Refund and cancel flow")
                .is_some(),
            "every word, any order"
        );
        assert!(Needle::new("straße").text_score("STRASSE Straße").is_some());
    }

    #[test]
    fn files_rank_basename_before_path() {
        let idx = index(&[
            "src/orders/state.rs",
            "src/state.rs",
            "docs/statement.md",
            "state.rs/readme.md",
            "src/estate/x.rs",
            "s/t/a/t/e.txt",
        ]);
        let got: Vec<String> = rank_files(&Needle::new("state.rs"), "api", &idx, 10)
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(
            got,
            [
                "file:api:src/state.rs",
                "file:api:src/orders/state.rs",
                "file:api:state.rs/readme.md",
                "file:api:src/estate/x.rs"
            ]
        );
        let got: Vec<String> = rank_files(&Needle::new("stat"), "api", &idx, 10)
            .into_iter()
            .map(|h| h.label)
            .collect();
        let mut prefixes = got[..3].to_vec();
        prefixes.sort();
        assert_eq!(
            prefixes,
            ["state.rs", "state.rs", "statement.md"],
            "file name prefixes first"
        );
        assert_eq!(
            got.last().map(String::as_str),
            Some("e.txt"),
            "a path subsequence comes last"
        );
        let hits = rank_files(&Needle::new("stat"), "api", &idx, 2);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].hint.as_deref(), Some("api/src/state.rs"));
    }

    #[test]
    fn settings_and_projects_match_in_memory() {
        let projects = vec![(
            "backend".to_string(),
            std::path::PathBuf::from("/code/shop-api"),
        )];
        let hits = merge(rank_static(&Needle::new("budget"), &projects), 10);
        assert_eq!(hits[0].id, "setting:limits.session_budget_usd");
        let hits = merge(rank_static(&Needle::new("shop"), &projects), 10);
        assert_eq!(hits[0].id, "project:backend");
        let hits = merge(rank_static(&Needle::new("routing.exec"), &projects), 10);
        assert_eq!(hits[0].id, "setting:routing.executor");
    }

    #[test]
    fn merge_keeps_the_best_hit_per_id() {
        let h = |id: &str, score| hit(SearchKind::Artifact, id.into(), "x".into(), None, score);
        let got = merge(vec![h("a", 0.5), h("b", 0.9), h("a", 0.7), h("c", 0.1)], 2);
        assert_eq!(
            got.iter()
                .map(|h| (h.id.as_str(), h.score))
                .collect::<Vec<_>>(),
            [("b", 0.9), ("a", 0.7)]
        );
    }

    #[test]
    fn ranking_a_large_index_is_fast() {
        let paths: Vec<String> = (0..50_000)
            .map(|i| {
                format!(
                    "packages/module_{}/src/components/widget_{i}/index_{i}.tsx",
                    i % 97
                )
            })
            .collect();
        let idx = FileIndex {
            paths,
            truncated: false,
        };
        let start = std::time::Instant::now();
        for q in ["widget_4999", "index", "mdl97", "zzz"] {
            let hits = rank_files(&Needle::new(q), "web", &idx, 50);
            assert!(hits.len() <= 50);
        }
        let per_query = start.elapsed() / 4;
        // Release builds take a few milliseconds; debug builds are several times slower.
        let budget = if cfg!(debug_assertions) {
            std::time::Duration::from_millis(500)
        } else {
            std::time::Duration::from_millis(50)
        };
        assert!(per_query < budget, "{per_query:?} per query");
        assert_eq!(
            rank_files(&Needle::new("widget_4999"), "web", &idx, 50)[0].label,
            "index_4999.tsx"
        );
    }
}
