use crate::text::truncate_end;
use crate::{ToolEnv, ToolOutput, bool_arg, required, str_arg, u64_arg};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{
    BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkMatch,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAX_GLOB_RESULTS: usize = 100;
const MAX_GREP_OUTPUT: usize = 30_000;

/// A path for tool output, with forward slashes on every OS so agents and tests read one form.
fn disp(p: &Path) -> String {
    let s = p.display().to_string();
    if cfg!(windows) { s.replace('\\', "/") } else { s }
}

fn walker(
    root: &Path,
    glob: Option<&str>,
    file_type: Option<&str>,
) -> Result<ignore::Walk, String> {
    let mut b = ignore::WalkBuilder::new(root);
    b.hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .parents(true)
        .require_git(false);
    let secret = SecretFilter::new();
    b.filter_entry(move |e| e.file_name() != ".git" && !secret.hides(e.path()));
    if let Some(glob) = glob.filter(|g| !g.trim().is_empty()) {
        let mut ob = ignore::overrides::OverrideBuilder::new(root);
        for g in split_globs(glob) {
            ob.add(&g).map_err(|e| format!("Invalid glob `{g}`: {e}"))?;
        }
        b.overrides(
            ob.build()
                .map_err(|e| format!("Invalid glob `{glob}`: {e}"))?,
        );
    }
    if let Some(t) = file_type.filter(|t| !t.trim().is_empty()) {
        let mut tb = ignore::types::TypesBuilder::new();
        tb.add_defaults();
        tb.select(type_alias(t.trim()));
        let types = tb
            .build()
            .map_err(|e| format!("Unknown file type `{t}`: {e}"))?;
        b.types(types);
    }
    Ok(b.build())
}

/// Credential stores and Ostra's data dir (except its agent assets), which a search from a
/// parent dir must not walk into.
struct SecretFilter {
    roots: Vec<PathBuf>,
    assets: Vec<PathBuf>,
}

impl SecretFilter {
    fn new() -> Self {
        let home = ostra_core::paths::home()
            .unwrap_or_default();
        let both = |p: PathBuf| {
            [ostra_core::paths::canonical(&p).ok(), Some(p)]
                .into_iter()
                .flatten()
        };
        SecretFilter {
            roots: ostra_core::paths::secret_paths(&home)
                .into_iter()
                .flat_map(both)
                .collect(),
            assets: both(ostra_core::paths::data_dir().join("assets")).collect(),
        }
    }

    fn hides(&self, p: &Path) -> bool {
        self.roots.iter().any(|r| p.starts_with(r))
            && !self
                .assets
                .iter()
                .any(|a| p.starts_with(a) || a.starts_with(p))
    }
}

fn type_alias(t: &str) -> &str {
    match t {
        "rs" => "rust",
        "python" => "py",
        "golang" => "go",
        "javascript" | "jsx" | "mjs" => "js",
        "typescript" | "tsx" => "ts",
        "kt" => "kotlin",
        "rb" => "ruby",
        "md" => "markdown",
        "yml" => "yaml",
        "c++" | "cc" => "cpp",
        "sh" | "bash" => "sh",
        other => other,
    }
}

/// `{a,b}` braces stay one glob; whitespace or commas outside braces separate globs.
fn split_globs(glob: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut depth = 0;
    for c in glob.chars() {
        match c {
            '{' => {
                depth += 1;
                cur.push(c);
            }
            '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' | ' ' if depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn sort_by_mtime(paths: &mut [PathBuf]) {
    let mut keyed: Vec<(SystemTime, PathBuf)> =
        paths.iter().map(|p| (mtime(p), p.clone())).collect();
    keyed.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (slot, (_, p)) in paths.iter_mut().zip(keyed) {
        *slot = p;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Files,
    Content,
    Count,
}

struct GrepArgs {
    pattern: String,
    root: PathBuf,
    glob: Option<String>,
    file_type: Option<String>,
    mode: Mode,
    case_insensitive: bool,
    line_numbers: bool,
    before: usize,
    after: usize,
    head_limit: Option<usize>,
    multiline: bool,
}

struct ContentSink<'a> {
    path: &'a str,
    lines: &'a mut Vec<String>,
    line_numbers: bool,
    last_line: Option<u64>,
    count: u64,
}

impl ContentSink<'_> {
    fn push(&mut self, line_number: Option<u64>, bytes: &[u8], sep: char) {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end_matches(['\n', '\r']);
        let mut first = true;
        for (i, part) in text.split('\n').enumerate() {
            let n = line_number.map(|n| n + i as u64);
            if let (Some(last), Some(n)) = (self.last_line, n)
                && first
                && n > last + 1
                && !self.lines.is_empty()
            {
                self.lines.push("--".into());
            }
            first = false;
            let shown = if part.len() > 500 {
                format!("{}...", &part[..crate::text::floor_boundary(part, 500)])
            } else {
                part.to_string()
            };
            match (self.line_numbers, n) {
                (true, Some(n)) => self
                    .lines
                    .push(format!("{}{sep}{n}{sep}{shown}", self.path)),
                _ => self.lines.push(format!("{}{sep}{shown}", self.path)),
            }
            if n.is_some() {
                self.last_line = n;
            }
        }
    }
}

impl Sink for ContentSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, m: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.count += 1;
        self.push(m.line_number(), m.bytes(), ':');
        Ok(true)
    }

    fn context(&mut self, _s: &Searcher, c: &SinkContext<'_>) -> Result<bool, Self::Error> {
        if matches!(c.kind(), SinkContextKind::Before | SinkContextKind::After) {
            self.push(c.line_number(), c.bytes(), '-');
        }
        Ok(true)
    }
}

struct CountSink(u64);

impl Sink for CountSink {
    type Error = std::io::Error;
    fn matched(&mut self, _s: &Searcher, _m: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.0 += 1;
        Ok(true)
    }
}

fn grep_blocking(a: GrepArgs) -> ToolOutput {
    let mut mb = RegexMatcherBuilder::new();
    mb.case_insensitive(a.case_insensitive).multi_line(true);
    if a.multiline {
        mb.dot_matches_new_line(true);
    } else {
        mb.line_terminator(Some(b'\n'));
    }
    let matcher = match mb.build(&a.pattern) {
        Ok(m) => m,
        Err(e) => return ToolOutput::err(format!("Invalid regex `{}`: {e}", a.pattern)),
    };
    let mut sb = SearcherBuilder::new();
    sb.binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .multi_line(a.multiline)
        .before_context(if a.mode == Mode::Content { a.before } else { 0 })
        .after_context(if a.mode == Mode::Content { a.after } else { 0 });
    let mut searcher = sb.build();

    let files: Vec<PathBuf> = if a.root.is_file() {
        vec![a.root.clone()]
    } else {
        let walk = match walker(&a.root, a.glob.as_deref(), a.file_type.as_deref()) {
            Ok(w) => w,
            Err(e) => return ToolOutput::err(e),
        };
        walk.filter_map(Result::ok)
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .map(|e| e.into_path())
            .collect()
    };

    let limit = a.head_limit.unwrap_or(usize::MAX);
    match a.mode {
        Mode::Files => {
            let mut hits: Vec<PathBuf> = files
                .into_iter()
                .filter(|p| {
                    let mut sink = CountSink(0);
                    searcher.search_path(&matcher, p, &mut sink).is_ok() && sink.0 > 0
                })
                .collect();
            if hits.is_empty() {
                return ToolOutput::ok("No files found");
            }
            sort_by_mtime(&mut hits);
            let total = hits.len();
            hits.truncate(limit);
            let mut out = format!("Found {total} file{}\n", if total == 1 { "" } else { "s" });
            for p in &hits {
                out.push_str(&format!("{}\n", disp(p)));
            }
            ToolOutput::ok(truncate_end(out.trim_end(), MAX_GREP_OUTPUT))
        }
        Mode::Count => {
            let mut rows = vec![];
            let mut total = 0;
            for p in files {
                let mut sink = CountSink(0);
                if searcher.search_path(&matcher, &p, &mut sink).is_ok() && sink.0 > 0 {
                    total += sink.0;
                    rows.push(format!("{}:{}", disp(&p), sink.0));
                }
            }
            if rows.is_empty() {
                return ToolOutput::ok("No matches found");
            }
            let files = rows.len();
            rows.truncate(limit);
            ToolOutput::ok(truncate_end(
                &format!(
                    "{}\n\nFound {total} matches across {files} files",
                    rows.join("\n")
                ),
                MAX_GREP_OUTPUT,
            ))
        }
        Mode::Content => {
            let mut lines = vec![];
            let mut stopped_early = false;
            for p in files {
                if lines.len() >= limit {
                    stopped_early = true;
                    break;
                }
                let shown = disp(&p);
                let mut file_lines = vec![];
                let mut sink = ContentSink {
                    path: &shown,
                    lines: &mut file_lines,
                    line_numbers: a.line_numbers,
                    last_line: None,
                    count: 0,
                };
                if searcher.search_path(&matcher, &p, &mut sink).is_ok() && sink.count > 0 {
                    if !lines.is_empty() && (a.before > 0 || a.after > 0) {
                        lines.push("--".into());
                    }
                    lines.extend(file_lines);
                }
            }
            if lines.is_empty() {
                return ToolOutput::ok("No matches found");
            }
            let truncated = stopped_early || lines.len() > limit;
            lines.truncate(limit);
            let mut out = lines.join("\n");
            if truncated {
                out.push_str(&format!(
                    "\n\n(Output limited to {limit} lines by head_limit.)"
                ));
            }
            ToolOutput::ok(truncate_end(&out, MAX_GREP_OUTPUT))
        }
    }
}

pub async fn grep(env: &ToolEnv, input: &Value) -> ToolOutput {
    let pattern = match required(input, "pattern") {
        Ok(p) => p.to_string(),
        Err(e) => return e,
    };
    let root = str_arg(input, "path")
        .map(|p| env.resolve(p))
        .unwrap_or_else(|| env.cwd());
    if !root.exists() {
        return ToolOutput::err(format!("Path does not exist: {}", root.display()));
    }
    let mode = match str_arg(input, "output_mode").unwrap_or("files_with_matches") {
        "files_with_matches" => Mode::Files,
        "content" => Mode::Content,
        "count" => Mode::Count,
        other => {
            return ToolOutput::err(format!(
                "Unknown output_mode `{other}`. Use `files_with_matches`, `content`, or `count`."
            ));
        }
    };
    let ctx = u64_arg(input, "-C").map(|n| n as usize);
    let args = GrepArgs {
        pattern,
        root,
        glob: str_arg(input, "glob").map(str::to_string),
        file_type: str_arg(input, "type").map(str::to_string),
        mode,
        case_insensitive: bool_arg(input, "-i").unwrap_or(false),
        line_numbers: bool_arg(input, "-n").unwrap_or(true),
        before: u64_arg(input, "-B")
            .map(|n| n as usize)
            .or(ctx)
            .unwrap_or(0),
        after: u64_arg(input, "-A")
            .map(|n| n as usize)
            .or(ctx)
            .unwrap_or(0),
        head_limit: u64_arg(input, "head_limit")
            .map(|n| n as usize)
            .filter(|n| *n > 0),
        multiline: bool_arg(input, "multiline").unwrap_or(false),
    };
    tokio::task::spawn_blocking(move || grep_blocking(args))
        .await
        .unwrap_or_else(|e| ToolOutput::err(format!("Search failed: {e}")))
}

fn glob_blocking(pattern: String, root: PathBuf) -> ToolOutput {
    let matcher = match globset::GlobBuilder::new(&pattern)
        .literal_separator(true)
        .build()
    {
        Ok(g) => g.compile_matcher(),
        Err(e) => return ToolOutput::err(format!("Invalid glob `{pattern}`: {e}")),
    };
    let walk = match walker(&root, None, None) {
        Ok(w) => w,
        Err(e) => return ToolOutput::err(e),
    };
    let mut hits: Vec<PathBuf> = walk
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter(|e| {
            e.path().strip_prefix(&root).is_ok_and(|rel| {
                // Match with forward slashes so a `**/*.rs` pattern matches on Windows too.
                matcher.is_match(rel) || matcher.is_match(rel.to_string_lossy().replace('\\', "/"))
            })
        })
        .map(|e| e.into_path())
        .collect();
    if hits.is_empty() {
        return ToolOutput::ok("No files found");
    }
    sort_by_mtime(&mut hits);
    let total = hits.len();
    hits.truncate(MAX_GLOB_RESULTS);
    let mut out: String = hits.iter().map(|p| format!("{}\n", disp(p))).collect();
    if total > MAX_GLOB_RESULTS {
        out.push_str(&format!(
            "(Results are truncated: showing {MAX_GLOB_RESULTS} of {total}. Use a more specific path or pattern.)\n"
        ));
    }
    ToolOutput::ok(out.trim_end().to_string())
}

pub async fn glob(env: &ToolEnv, input: &Value) -> ToolOutput {
    let pattern = match required(input, "pattern") {
        Ok(p) => p.to_string(),
        Err(e) => return e,
    };
    let root = str_arg(input, "path")
        .map(|p| env.resolve(p))
        .unwrap_or_else(|| env.cwd());
    if !root.is_dir() {
        return ToolOutput::err(format!("Not a directory: {}", root.display()));
    }
    tokio::task::spawn_blocking(move || glob_blocking(pattern, root))
        .await
        .unwrap_or_else(|e| ToolOutput::err(format!("Search failed: {e}")))
}

#[cfg(test)]
mod tests {
    use crate::testutil::{env_in, run};
    use serde_json::json;
    use std::fs;

    fn fixture(root: &std::path::Path) {
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join(".ostra/skills/convention")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(
            root.join("src/main.rs"),
            "fn main() {\n    let order = 1;\n    println!(\"{order}\");\n}\n",
        )
        .unwrap();
        fs::write(root.join("src/deep/lib.rs"), "pub fn Order() {}\n").unwrap();
        fs::write(root.join("src/app.js"), "const order = 2;\n").unwrap();
        fs::write(root.join("target/gen.rs"), "let order = 3;\n").unwrap();
        fs::write(
            root.join(".ostra/skills/convention/SKILL.md"),
            "order rules\n",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn grep_modes() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let root = env.config().repo_root.clone();
        fixture(&root);
        let out = run(&env, "Grep", json!({"pattern": "order"})).await;
        assert!(out.text.starts_with("Found 3 files"), "{}", out.text);
        assert!(!out.text.contains("target/gen.rs"));
        assert!(out.text.contains(".ostra/skills/convention/SKILL.md"));
        let out = run(
            &env,
            "Grep",
            json!({"pattern": "order", "-i": true, "type": "rust"}),
        )
        .await;
        assert!(out.text.starts_with("Found 2 files"), "{}", out.text);
        let out = run(&env, "Grep", json!({"pattern": "order", "glob": "*.js"})).await;
        assert!(out.text.starts_with("Found 1 file\n") && out.text.contains("app.js"));
        let out = run(
            &env,
            "Grep",
            json!({"pattern": "let order", "output_mode": "content", "-A": 1}),
        )
        .await;
        assert!(
            out.text.contains("main.rs:2:    let order = 1;"),
            "{}",
            out.text
        );
        assert!(out.text.contains("main.rs-3-"), "{}", out.text);
        let out = run(
            &env,
            "Grep",
            json!({"pattern": "order", "output_mode": "count", "path": "src"}),
        )
        .await;
        assert!(out.text.contains("main.rs:2"), "{}", out.text);
        let out = run(&env, "Grep", json!({"pattern": "nothing-here"})).await;
        assert_eq!(out.text, "No files found");
        let out = run(&env, "Grep", json!({"pattern": "("})).await;
        assert!(out.is_error);
        let out = run(&env, "Grep", json!({"pattern": "main\\(\\) \\{\\n    let", "multiline": true, "output_mode": "files_with_matches"})).await;
        assert!(out.text.contains("main.rs"), "{}", out.text);
        let out = run(
            &env,
            "Grep",
            json!({"pattern": "order", "output_mode": "content", "head_limit": 1}),
        )
        .await;
        assert!(out.text.contains("head_limit"));
    }

    #[tokio::test]
    async fn glob_matches_relative_paths() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let root = env.config().repo_root.clone();
        fixture(&root);
        let out = run(&env, "Glob", json!({"pattern": "**/*.rs"})).await;
        assert!(out.text.contains("src/main.rs") && out.text.contains("src/deep/lib.rs"));
        assert!(!out.text.contains("target/gen.rs"));
        let out = run(&env, "Glob", json!({"pattern": "*.rs", "path": "src"})).await;
        assert!(
            out.text.contains("main.rs") && !out.text.contains("lib.rs"),
            "{}",
            out.text
        );
        let out = run(&env, "Glob", json!({"pattern": "*.nope"})).await;
        assert_eq!(out.text, "No files found");
        for i in 0..120 {
            fs::write(root.join(format!("f{i}.txt")), "").unwrap();
        }
        let out = run(&env, "Glob", json!({"pattern": "*.txt"})).await;
        assert!(out.text.contains("showing 100 of 120"));
    }

    #[test]
    fn search_skips_credential_stores() {
        let Some(home) = ostra_core::paths::home() else {
            return;
        };
        let f = super::SecretFilter::new();
        assert!(f.hides(&home.join(".ssh/id_ed25519")));
        assert!(f.hides(&home.join(".aws")));
        assert!(!f.hides(&home.join("code/app/main.rs")));
        assert!(!f.hides(&ostra_core::paths::data_dir().join("assets/skills/x.md")));
        assert!(f.hides(&ostra_core::paths::data_dir().join("registry.db")));
    }

    #[test]
    fn globs_split_outside_braces() {
        assert_eq!(super::split_globs("*.{ts,tsx}"), vec!["*.{ts,tsx}"]);
        assert_eq!(super::split_globs("*.ts, *.js"), vec!["*.ts", "*.js"]);
    }
}
