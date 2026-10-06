//! Rule B8 retrieval eval: questions an agent would ask about Ostra, each with the source files that
//! answer it, run against a book the docs stage wrote about this repository (`book.json`, an Opus
//! documentation run on a snapshot of the repository). Two grades:
//! - By labels (`labels/`, the floors apply): a hit is relevant when its unit states the answer. The
//!   labels belong to this `book.json`; a new book needs new labels.
//! - By cited files, reported only: a hit is relevant when its unit cites one of the question's
//!   files. It survives a rewritten book but counts a unit that cites a file without answering.
//!
//! Questions no unit answers measure the book's coverage, not retrieval, and are counted apart.
//!
//! `OSTRA_EVAL_REPORT=1 cargo test -p ostra-core --test book_retrieval -- --nocapture` prints every
//! miss, and `OSTRA_EVAL_RANKS=<file>` writes each question's rank (0 for none in the top 10).

use ostra_core::book::Book;
use ostra_core::book_search::{Filter, Index, PassageKind, PassageSpec, Unit, UnitSpec};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Floors for the labeled questions the book answers, a little under the scores measured on
/// 2026-09-30 (hit@1 63.0%, hit@5 89.8%, MRR 0.748). A ranking change that lowers one fails the suite.
const MIN_HIT_AT_1: f64 = 0.60;
const MIN_HIT_AT_5: f64 = 0.87;
const MIN_MRR: f64 = 0.72;
const K: usize = 10;

#[derive(Deserialize)]
struct File {
    case: Vec<Case>,
}

#[derive(Deserialize, Clone)]
struct Case {
    id: String,
    kind: String,
    question: String,
    /// Project-relative files, or folders ending in `/`, whose content answers the question.
    gold: Vec<String>,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/evals/book_retrieval")
}

fn cases() -> Vec<Case> {
    let mut out = vec![];
    let mut files: Vec<PathBuf> = std::fs::read_dir(root().join("questions"))
        .expect("tests/evals/book_retrieval/questions")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let parsed: File = toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        out.extend(parsed.case);
    }
    out
}

fn relevant(u: &Unit, gold: &[String]) -> bool {
    u.code_paths.iter().any(|p| {
        let p = p.trim().trim_start_matches("./");
        gold.iter().any(|g| {
            if g.ends_with('/') {
                p.starts_with(g.as_str())
            } else {
                p == g
            }
        })
    })
}

#[derive(Default)]
struct Tally {
    n: usize,
    covered: usize,
    at1: usize,
    at5: usize,
    rr: f64,
    precision: f64,
    chars: usize,
}

impl Tally {
    fn line(&self, name: &str) -> String {
        let c = self.covered.max(1) as f64;
        format!(
            "{name:<12} {:>4} questions, {:>4} covered ({:>5.1}%)  hit@1 {:>5.1}%  hit@5 {:>5.1}%  MRR@{K} {:.3}  P@5 {:.2}  {:>5} chars",
            self.n,
            self.covered,
            100.0 * self.covered as f64 / self.n.max(1) as f64,
            100.0 * self.at1 as f64 / c,
            100.0 * self.at5 as f64 / c,
            self.rr / c,
            self.precision / c,
            self.chars / self.covered.max(1)
        )
    }
}

#[test]
fn questions_are_well_formed() {
    let cases = cases();
    let mut ids = std::collections::HashSet::new();
    for c in &cases {
        assert!(ids.insert(c.id.clone()), "duplicate case id {}", c.id);
        assert!(
            ["lexical", "paraphrase", "conceptual", "cross"].contains(&c.kind.as_str()),
            "{}: kind {}",
            c.id,
            c.kind
        );
        assert!(!c.gold.is_empty(), "{}: no gold files", c.id);
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for g in &c.gold {
            assert!(
                repo.join(g).exists(),
                "{}: gold path {g} does not exist",
                c.id
            );
        }
    }
}

/// `labels/*.toml`: for each case, the units of `book.json` that state its answer, labeled by
/// reading the book. Empty when the book does not answer the case.
fn labels() -> BTreeMap<String, Vec<String>> {
    labels_in("labels")
}

fn labels_in(folder: &str) -> BTreeMap<String, Vec<String>> {
    #[derive(Deserialize)]
    struct F {
        labels: BTreeMap<String, Vec<String>>,
    }
    let mut out = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(root().join(folder)) else {
        return out;
    };
    for e in rd.flatten() {
        let text = std::fs::read_to_string(e.path()).unwrap();
        let f: F =
            toml::from_str(&text).unwrap_or_else(|err| panic!("{}: {err}", e.path().display()));
        // A case may be labeled in several files (a reading pass and a pooled pass); keep the union.
        for (id, units) in f.labels {
            let e: &mut Vec<String> = out.entry(id).or_default();
            for u in units {
                if !e.contains(&u) {
                    e.push(u);
                }
            }
        }
    }
    out
}

/// Grades every case with `is_relevant`; a case with no relevant unit counts as uncovered.
fn grade(
    index: &Index,
    cases: &[Case],
    is_relevant: &dyn Fn(&Case, &Unit) -> bool,
    report: bool,
) -> (Tally, BTreeMap<String, Tally>, String, Vec<String>) {
    let mut all = Tally::default();
    let mut by_kind: BTreeMap<String, Tally> = BTreeMap::new();
    let mut ranks = String::new();
    let mut uncovered = vec![];
    for c in cases {
        by_kind.entry(c.kind.clone()).or_default().n += 1;
        all.n += 1;
        if !index.units.iter().any(|u| is_relevant(c, u)) {
            uncovered.push(c.id.clone());
            continue;
        }
        let hits = index.search(&c.question, &Filter::default(), K);
        let rank = hits
            .iter()
            .position(|h| is_relevant(c, &index.units[h.unit]));
        let top5 = hits
            .iter()
            .take(5)
            .filter(|h| is_relevant(c, &index.units[h.unit]))
            .count();
        let chars = index
            .render(
                &c.question,
                &hits[..hits.len().min(5)],
                std::path::Path::new("/b"),
            )
            .len();
        ranks.push_str(&format!("{} {}\n", c.id, rank.map_or(0, |r| r + 1)));
        for t in [&mut all, by_kind.get_mut(&c.kind).unwrap()] {
            t.covered += 1;
            t.chars += chars;
            t.precision += top5 as f64 / 5.0;
            if let Some(r) = rank {
                t.rr += 1.0 / (r + 1) as f64;
                t.at1 += usize::from(r == 0);
                t.at5 += usize::from(r < 5);
            }
        }
        if report && rank.is_none_or(|r| r >= 5) {
            let top: Vec<&str> = hits
                .iter()
                .take(3)
                .map(|h| index.units[h.unit].id.as_str())
                .collect();
            eprintln!(
                "MISS {} [{}] rank {:?}: {}\n     top {:?}",
                c.id,
                c.kind,
                rank.map(|r| r + 1),
                c.question,
                top
            );
        }
    }
    (all, by_kind, ranks, uncovered)
}

fn print(title: &str, all: &Tally, by_kind: &BTreeMap<String, Tally>) {
    eprintln!("{title}");
    eprintln!("{}", all.line("all"));
    for (k, t) in by_kind {
        eprintln!("{}", t.line(k));
    }
}

#[test]
fn retrieval_over_the_ostra_book() {
    let path = root().join("book.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!(
            "book_retrieval: no {}, so nothing to measure yet",
            path.display()
        );
        return;
    };
    let book: Book = serde_json::from_str(&text).unwrap();
    let index = Index::build(&[book]);
    let report = std::env::var("OSTRA_EVAL_REPORT").is_ok();
    let cases = cases();
    eprintln!(
        "{} units, {} passages",
        index.units.len(),
        index.passages.len()
    );

    let (all, by_kind, _, _) = grade(&index, &cases, &|c, u| relevant(u, &c.gold), false);
    print(
        "Graded by cited files (a unit citing a gold file counts):",
        &all,
        &by_kind,
    );

    let labels = labels();
    if labels.is_empty() {
        return;
    }
    for c in &cases {
        let l = labels
            .get(&c.id)
            .unwrap_or_else(|| panic!("{}: no label", c.id));
        for id in l {
            assert!(
                index.units.iter().any(|u| &u.id == id),
                "{}: label {id} names no unit",
                c.id
            );
        }
    }
    let by_label = |c: &Case, u: &Unit| labels[&c.id].contains(&u.id);
    let (all, by_kind, ranks, uncovered) = grade(&index, &cases, &by_label, report);
    print(
        "Graded by labels (a unit that states the answer counts):",
        &all,
        &by_kind,
    );
    if let Ok(p) = std::env::var("OSTRA_EVAL_RANKS") {
        std::fs::write(p, &ranks).unwrap();
    }
    if report {
        eprintln!("unanswered by the book: {}", uncovered.join(", "));
    }
    let c = all.covered.max(1) as f64;
    assert!(all.at1 as f64 / c >= MIN_HIT_AT_1, "{}", all.line("all"));
    assert!(all.at5 as f64 / c >= MIN_HIT_AT_5, "{}", all.line("all"));
    assert!(all.rr / c >= MIN_MRR, "{}", all.line("all"));
}

#[test]
#[ignore]
fn show_query() {
    let book: Book =
        serde_json::from_str(&std::fs::read_to_string(root().join("book.json")).unwrap()).unwrap();
    let index = Index::build(&[book]);
    let q = std::env::var("Q").unwrap();
    let hits = index.search(&q, &Filter::default(), 5);
    for h in &hits {
        eprintln!("score {:.2} {}", h.score, index.units[h.unit].id);
    }
    eprintln!(
        "{}",
        index.render(&q, &hits, std::path::Path::new("/ws/.ostra/docs"))
    );
}

/// Writes the passages, the cases with their labels, and each case's BM25 unit scores to
/// `OSTRA_EVAL_DUMP`, for comparing other rankers against this one outside Rust.
#[test]
#[ignore]
fn dump_for_rankers() {
    let (index, labels) = corpus(&std::env::var("OSTRA_EVAL_CORPUS").unwrap_or("book".into()));
    let units: Vec<serde_json::Value> = index
        .units
        .iter()
        .map(|u| serde_json::json!({"id": u.id, "title": u.title}))
        .collect();
    let passages: Vec<serde_json::Value> = index
        .passages
        .iter()
        .map(|p| serde_json::json!({"unit": p.unit, "label": p.label, "text": p.text}))
        .collect();
    let cases: Vec<serde_json::Value> = cases()
        .iter()
        .map(|c| {
            let hits = index.search(&c.question, &Filter::default(), index.units.len());
            let bm25: Vec<(usize, f32)> = hits.iter().map(|h| (h.unit, h.score)).collect();
            serde_json::json!({"id": c.id, "kind": c.kind, "question": c.question,
                "labels": labels.get(&c.id), "bm25": bm25})
        })
        .collect();
    let out = serde_json::json!({"units": units, "passages": passages, "cases": cases});
    std::fs::write(std::env::var("OSTRA_EVAL_DUMP").unwrap(), out.to_string()).unwrap();
}

// -----------------------------------------------------------------------------------------------
// The same questions over `docs/`, the hand-written deep dives. Ignored: the labels name headings,
// and `docs/` changes with every feature, so they drift. Run it by hand:
// `cargo test -p ostra-core --test book_retrieval retrieval_over_docs -- --ignored --nocapture`.
// -----------------------------------------------------------------------------------------------

fn slug(h: &str) -> String {
    let mut out = String::new();
    for c in h.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if (c == ' ' || c == '-') && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn inline_code(text: &str) -> String {
    text.split('`')
        .skip(1)
        .step_by(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every page of `docs/` cut into sections at `##` and `###` headings, and each section into
/// paragraphs, lists and tables in windows of five, and code blocks, the way a book is cut.
fn docs_index() -> Index {
    let docs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    let mut pages = vec![];
    let mut stack = vec![docs.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "md") {
                pages.push(p);
            }
        }
    }
    pages.sort();
    let mut units: Vec<UnitSpec> = vec![];
    let mut passages: Vec<PassageSpec> = vec![];
    for page in pages {
        let rel = page
            .strip_prefix(&docs)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(&page).unwrap();
        let title = text
            .lines()
            .find_map(|l| l.strip_prefix("# "))
            .unwrap_or(&rel)
            .trim()
            .to_string();
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        let mut new_unit = |units: &mut Vec<UnitSpec>, anchor: &str, t: String| {
            let n = seen.entry(anchor.to_string()).or_default();
            *n += 1;
            let id = if *n == 1 {
                format!("{rel}#{anchor}")
            } else {
                format!("{rel}#{anchor}-{n}")
            };
            units.push(UnitSpec {
                book: "docs".into(),
                id,
                title: t,
                file: PathBuf::from(&rel),
                code_paths: vec![],
            });
            units.len() - 1
        };
        let mut unit = new_unit(&mut units, "", title.clone());
        let mut h2 = String::new();
        let mut label = String::new();
        let mut block: Vec<String> = vec![];
        let mut fence: Option<String> = None;
        let flush =
            |block: &mut Vec<String>, unit: usize, label: &str, passages: &mut Vec<PassageSpec>| {
                if block.is_empty() {
                    return;
                }
                let lines = std::mem::take(block);
                let push = |passages: &mut Vec<PassageSpec>, kind, l: String, text: String| {
                    let code = inline_code(&text);
                    passages.push(PassageSpec {
                        unit,
                        kind,
                        label: l,
                        text,
                        code,
                    });
                };
                let named = |d: &str| {
                    if label.is_empty() {
                        d.to_string()
                    } else {
                        format!("{label}: {d}")
                    }
                };
                if lines[0].starts_with('|') {
                    let cols: Vec<String> = lines[0]
                        .trim_matches('|')
                        .split('|')
                        .map(|c| c.trim().to_string())
                        .collect();
                    let rows: Vec<String> = lines
                        .iter()
                        .skip(1)
                        .filter(|l| !l.replace(['|', '-', ':', ' '], "").is_empty())
                        .map(|l| {
                            format!(
                                "- {}",
                                l.trim_matches('|')
                                    .split('|')
                                    .map(str::trim)
                                    .collect::<Vec<_>>()
                                    .join(" | ")
                            )
                        })
                        .collect();
                    for chunk in rows.chunks(5) {
                        push(
                            passages,
                            PassageKind::Table,
                            named(&format!("Table ({})", cols.join(" | "))),
                            chunk.join("\n"),
                        );
                    }
                } else if lines[0].starts_with("- ")
                    || lines[0].starts_with("* ")
                    || lines[0]
                        .split_once(". ")
                        .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
                {
                    let mut items: Vec<String> = vec![];
                    for l in &lines {
                        let t = l.trim_start();
                        let starts = t.starts_with("- ")
                            || t.starts_with("* ")
                            || t.split_once(". ").is_some_and(|(n, _)| {
                                !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
                            });
                        if starts && l.len() - t.len() < 2 || items.is_empty() {
                            items.push(t.to_string());
                        } else {
                            let last = items.last_mut().unwrap();
                            last.push(' ');
                            last.push_str(t);
                        }
                    }
                    for chunk in items.chunks(5) {
                        push(passages, PassageKind::List, named("List"), chunk.join("\n"));
                    }
                } else {
                    push(passages, PassageKind::Text, named("Text"), lines.join(" "));
                }
            };
        for line in text.lines() {
            if let Some(lang) = &fence {
                if line.trim_start().starts_with("```") {
                    let kind = if lang == "mermaid" {
                        PassageKind::Diagram
                    } else {
                        PassageKind::Code
                    };
                    let body = std::mem::take(&mut block).join("\n");
                    let l = if label.is_empty() {
                        "Code".to_string()
                    } else {
                        format!("{label}: code")
                    };
                    let code = if kind == PassageKind::Diagram {
                        String::new()
                    } else {
                        body.clone()
                    };
                    passages.push(PassageSpec {
                        unit,
                        kind,
                        label: l,
                        text: body,
                        code,
                    });
                    fence = None;
                } else {
                    block.push(line.to_string());
                }
                continue;
            }
            if let Some(rest) = line.trim_start().strip_prefix("```") {
                flush(&mut block, unit, &label, &mut passages);
                fence = Some(rest.trim().to_string());
                continue;
            }
            if let Some(h) = line.strip_prefix("## ") {
                flush(&mut block, unit, &label, &mut passages);
                h2 = h.trim().to_string();
                label.clear();
                unit = new_unit(&mut units, &slug(h), format!("{title} > {h2}"));
                continue;
            }
            if let Some(h) = line.strip_prefix("### ") {
                flush(&mut block, unit, &label, &mut passages);
                label.clear();
                let t = if h2.is_empty() {
                    format!("{title} > {}", h.trim())
                } else {
                    format!("{title} > {h2} > {}", h.trim())
                };
                unit = new_unit(&mut units, &slug(h), t);
                continue;
            }
            if let Some(h) = line.strip_prefix("#### ") {
                flush(&mut block, unit, &label, &mut passages);
                label = h.trim().to_string();
                continue;
            }
            if line.starts_with("# ") || line.trim_start().starts_with("![") {
                continue;
            }
            if line.trim().is_empty() {
                flush(&mut block, unit, &label, &mut passages);
                continue;
            }
            let is_table = line.starts_with('|');
            if !block.is_empty() && block[0].starts_with('|') != is_table {
                flush(&mut block, unit, &label, &mut passages);
            }
            block.push(line.to_string());
        }
        flush(&mut block, unit, &label, &mut passages);
    }
    Index::from_parts(units, passages)
}

#[test]
#[ignore]
fn retrieval_over_docs() {
    let index = docs_index();
    let cases = cases();
    let labels = labels_in("docs_labels");
    eprintln!(
        "docs: {} units, {} passages",
        index.units.len(),
        index.passages.len()
    );
    if labels.is_empty() {
        return;
    }
    for c in &cases {
        for id in labels.get(&c.id).into_iter().flatten() {
            if !index.units.iter().any(|u| &u.id == id) {
                eprintln!(
                    "{}: label {id} names no section; relabel after a docs change",
                    c.id
                );
            }
        }
    }
    let by_label = |c: &Case, u: &Unit| labels.get(&c.id).is_some_and(|l| l.contains(&u.id));
    let report = std::env::var("OSTRA_EVAL_REPORT").is_ok();
    let (all, by_kind, ranks, _) = grade(&index, &cases, &by_label, report);
    print("docs/, graded by labels:", &all, &by_kind);
    if let Ok(p) = std::env::var("OSTRA_EVAL_RANKS") {
        std::fs::write(p, &ranks).unwrap();
    }
}

/// A corpus by name and its labels: `book` (one writer for the whole repository), `split` (one
/// writer per crate, `book_split.json`), `area` (Rule B9's area writers on Sonnet, `book_area.json`),
/// or `docs` (the pages in `docs/`).
fn corpus(name: &str) -> (Index, BTreeMap<String, Vec<String>>) {
    let book = |f: &str| -> Index {
        let b: Book =
            serde_json::from_str(&std::fs::read_to_string(root().join(f)).unwrap()).unwrap();
        Index::build(&[b])
    };
    match name {
        "docs" => (docs_index(), labels_in("docs_labels")),
        "split" => (book("book_split.json"), labels_in("split_labels")),
        "area" => (book("book_area.json"), labels_in("area_labels")),
        _ => (book("book.json"), labels()),
    }
}

/// The retrieval eval over any corpus: `OSTRA_EVAL_CORPUS=split cargo test -p ostra-core --test
/// book_retrieval retrieval_over_corpus -- --ignored --nocapture`.
#[test]
#[ignore]
fn retrieval_over_corpus() {
    let name = std::env::var("OSTRA_EVAL_CORPUS").unwrap_or("book".into());
    let (index, labels) = corpus(&name);
    let cases = cases();
    eprintln!(
        "{name}: {} units, {} passages",
        index.units.len(),
        index.passages.len()
    );
    for c in &cases {
        for id in labels.get(&c.id).into_iter().flatten() {
            if !index.units.iter().any(|u| &u.id == id) {
                eprintln!("{}: label {id} names no unit", c.id);
            }
        }
    }
    let by_label = |c: &Case, u: &Unit| labels.get(&c.id).is_some_and(|l| l.contains(&u.id));
    let report = std::env::var("OSTRA_EVAL_REPORT").is_ok();
    let (all, by_kind, ranks, _) = grade(&index, &cases, &by_label, report);
    print(&format!("{name}, graded by labels:"), &all, &by_kind);
    if let Ok(p) = std::env::var("OSTRA_EVAL_RANKS") {
        std::fs::write(p, &ranks).unwrap();
    }
}

#[test]
#[ignore]
fn show_units() {
    let (index, _) = corpus(&std::env::var("OSTRA_EVAL_CORPUS").unwrap_or("book".into()));
    let mut out = String::new();
    for (i, u) in index.units.iter().enumerate() {
        out.push_str(&format!("=== UNIT {} === {}\n", u.id, u.title));
        for p in index.passages.iter().filter(|p| p.unit == i) {
            out.push_str(&format!("[{}] {}\n", p.label, p.text));
        }
        out.push('\n');
    }
    std::fs::write(std::env::var("OSTRA_EVAL_UNITS").unwrap(), out).unwrap();
}
