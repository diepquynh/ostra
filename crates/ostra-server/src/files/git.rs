//! Git facts for the Files dock: one `git status --porcelain=v2 -z` per project, and unified
//! diffs parsed into hunks. Every call is read-only and takes no optional locks, so it never
//! contends with the engine's own `git add`.

use ostra_core::api::{DiffHunk, DiffLine, DiffLineKind, GitMark};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const GIT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct GitOutput {
    pub stdout: Vec<u8>,
    pub success: bool,
    /// Exit code 1, which `git diff --no-index` uses for "files differ".
    pub differs: bool,
    pub truncated: bool,
}

/// Run git in `root`, reading at most `cap` bytes of stdout. `None` when git cannot start or
/// times out.
pub async fn run(root: &Path, args: &[&str], cap: usize) -> Option<GitOutput> {
    let filters = ostra_core::git::filter_overrides(root).await;
    let mut child = tokio::process::Command::new("git")
        .args(ostra_core::git::AUTOMATIC.iter())
        .args(&filters)
        .arg("--no-optional-locks")
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let work = async {
        let mut buf = Vec::new();
        (&mut stdout)
            .take(cap as u64 + 1)
            .read_to_end(&mut buf)
            .await
            .ok()?;
        let truncated = buf.len() > cap;
        buf.truncate(cap);
        if truncated {
            let _ = child.start_kill();
        }
        let status = child.wait().await.ok()?;
        Some(GitOutput {
            stdout: buf,
            success: status.success(),
            differs: status.code() == Some(1),
            truncated,
        })
    };
    tokio::time::timeout(GIT_TIMEOUT, work).await.ok().flatten()
}

/// The project's path inside its repository (`sub/dir/`, or empty at the top), or `None` when the
/// project is not in a git work tree.
pub async fn prefix(root: &Path) -> Option<String> {
    let out = run(root, &["rev-parse", "--show-prefix"], 64 * 1024).await?;
    out.success
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Per-file git state of one project, with paths relative to the project root.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GitStatus {
    pub repo: bool,
    /// The project's place in its repository, as [`prefix`] returns it.
    pub prefix: String,
    files: BTreeMap<String, (GitMark, bool)>,
    /// Untracked folders git reports as a whole, without their trailing `/`.
    untracked_dirs: Vec<String>,
}

impl GitStatus {
    pub fn file(&self, rel: &str) -> Option<(GitMark, bool)> {
        if let Some(f) = self.files.get(rel) {
            return Some(*f);
        }
        self.untracked_dirs
            .iter()
            .any(|d| under(rel, d))
            .then_some((GitMark::Untracked, false))
    }

    /// A folder's own mark (only `?` for an untracked folder) and whether anything under it changed.
    pub fn dir(&self, rel: &str) -> (Option<GitMark>, bool) {
        if self
            .untracked_dirs
            .iter()
            .any(|d| d == rel || under(rel, d))
        {
            return (Some(GitMark::Untracked), true);
        }
        let changed = if rel.is_empty() {
            !self.files.is_empty() || !self.untracked_dirs.is_empty()
        } else {
            let start = format!("{rel}/");
            self.files
                .range(start.clone()..)
                .next()
                .is_some_and(|(k, _)| k.starts_with(&start))
                || self.untracked_dirs.iter().any(|d| d.starts_with(&start))
        };
        (None, changed)
    }

    /// Every changed file and untracked folder, as `(path, mark, staged)`.
    pub fn changed(&self) -> impl Iterator<Item = (&str, GitMark, bool)> {
        self.files
            .iter()
            .map(|(p, (m, s))| (p.as_str(), *m, *s))
            .chain(
                self.untracked_dirs
                    .iter()
                    .map(|d| (d.as_str(), GitMark::Untracked, false)),
            )
    }
}

fn under(path: &str, dir: &str) -> bool {
    path.len() > dir.len() && path.starts_with(dir) && path.as_bytes()[dir.len()] == b'/'
}

/// Load the status of the project at `root`. A folder outside any git work tree yields an empty
/// status with `repo: false`.
pub async fn status(root: &Path) -> GitStatus {
    let Some(prefix) = prefix(root).await else {
        return GitStatus::default();
    };
    let args = [
        "status",
        "--porcelain=v2",
        "-z",
        "--untracked-files=normal",
        "--ignore-submodules=all",
        "--",
        ".",
    ];
    match run(root, &args, 32 * 1024 * 1024).await {
        Some(out) if out.success => parse_porcelain_v2(&out.stdout, &prefix),
        _ => GitStatus {
            repo: true,
            prefix,
            ..Default::default()
        },
    }
}

/// Map an `XY` pair to one mark: X is the index side, Y the work tree side, `.` unchanged.
fn mark(x: u8, y: u8) -> GitMark {
    match (x, y) {
        (b'R' | b'C', _) => GitMark::Renamed,
        (b'A', _) => GitMark::Added,
        (b'D', _) | (_, b'D') => GitMark::Deleted,
        _ => GitMark::Modified,
    }
}

/// Parse `git status --porcelain=v2 -z` output. Paths in it are relative to the repository root;
/// `prefix` is the project's place in the repository, and entries outside it are dropped.
pub fn parse_porcelain_v2(out: &[u8], prefix: &str) -> GitStatus {
    let mut st = GitStatus {
        repo: true,
        prefix: prefix.to_string(),
        ..Default::default()
    };
    let mut records = out.split(|b| *b == 0).filter(|r| !r.is_empty());
    let local = |p: &[u8]| -> Option<String> {
        let p = String::from_utf8_lossy(p);
        p.strip_prefix(prefix)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    while let Some(rec) = records.next() {
        let fields = |n: usize| rec.splitn(n, |b| *b == b' ').collect::<Vec<_>>();
        match rec[0] {
            b'1' | b'2' | b'u' => {
                let n = match rec[0] {
                    b'1' => 9,
                    b'2' => 10,
                    _ => 11,
                };
                let f = fields(n);
                if rec[0] == b'2' {
                    // The rename's source path follows as its own NUL-terminated record.
                    records.next();
                }
                let (Some(xy), Some(path)) = (f.get(1), f.get(n - 1)) else {
                    continue;
                };
                if xy.len() != 2 || f.len() != n {
                    continue;
                }
                let (x, y) = (xy[0], xy[1]);
                let entry = if rec[0] == b'u' {
                    (GitMark::Modified, false)
                } else {
                    (mark(x, y), x != b'.')
                };
                if let Some(p) = local(path) {
                    st.files.insert(p, entry);
                }
            }
            b'?' => {
                let Some(p) = rec.get(2..).and_then(local) else {
                    continue;
                };
                match p.strip_suffix('/') {
                    Some(dir) => st.untracked_dirs.push(dir.to_string()),
                    None => {
                        st.files.insert(p, (GitMark::Untracked, false));
                    }
                }
            }
            _ => {}
        }
    }
    st
}

/// A `base` for `git diff`: a commit, branch, tag, or revision expression, never an option.
pub fn valid_base(base: &str) -> bool {
    !base.is_empty()
        && base.len() <= 200
        && !base.starts_with('-')
        && base
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/~^@{}-".contains(&b))
}

/// Resolve `base` to a commit id. A repository with no commits yet compares `HEAD` against the
/// empty tree, so a new project's files show as added.
pub async fn resolve_base(root: &Path, base: &str) -> Result<String, String> {
    let spec = format!("{base}^{{commit}}");
    if let Some(out) = run(root, &["rev-parse", "--verify", "--quiet", &spec], 4096).await
        && out.success
    {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    if base == "HEAD"
        && let Some(out) = run(root, &["hash-object", "-t", "tree", "/dev/null"], 4096).await
        && out.success
    {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    Err(format!(
        "No commit named `{base}` in this repository. Use a commit, branch, or tag that exists."
    ))
}

/// The diff of one project-relative file against the resolved `base`, or against nothing when the
/// file is untracked. Returns the parsed diff and whether output hit `cap`.
pub async fn diff_file(
    root: &Path,
    rel: &str,
    base: &str,
    untracked: bool,
    cap: usize,
) -> Option<(ParsedDiff, bool)> {
    let out = if untracked {
        run(
            root,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--no-index",
                "--",
                "/dev/null",
                rel,
            ],
            cap,
        )
        .await?
    } else {
        run(
            root,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "-U3",
                base,
                "--",
                rel,
            ],
            cap,
        )
        .await?
    };
    (out.success || out.differs || out.truncated).then(|| {
        (
            parse_unified(&String::from_utf8_lossy(&out.stdout)),
            out.truncated,
        )
    })
}

/// Added and removed line counts per tracked file against the resolved `base`.
pub async fn numstat(
    root: &Path,
    base: &str,
    rels: &[String],
    prefix: &str,
) -> BTreeMap<String, (u32, u32)> {
    if rels.is_empty() {
        return BTreeMap::new();
    }
    let mut args: Vec<&str> = vec![
        "diff",
        "--numstat",
        "-z",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        base,
        "--",
    ];
    args.extend(rels.iter().map(String::as_str));
    match run(root, &args, 8 * 1024 * 1024).await {
        Some(out) if out.success => parse_numstat_z(&out.stdout, prefix),
        _ => BTreeMap::new(),
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParsedDiff {
    pub hunks: Vec<DiffHunk>,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
}

fn hunk_header(line: &str) -> Option<DiffHunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, header) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(u32, u32)> {
        match r.split_once(',') {
            Some((s, n)) => Some((s.parse().ok()?, n.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (old_start, old_lines) = range(old)?;
    let (new_start, new_lines) = range(new)?;
    Some(DiffHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        header: header.trim().to_string(),
        lines: vec![],
    })
}

/// Parse the unified diff of one file (`git diff` output) into hunks with line numbers.
pub fn parse_unified(text: &str) -> ParsedDiff {
    let mut out = ParsedDiff::default();
    let (mut old_no, mut new_no) = (0u32, 0u32);
    for line in text.split('\n') {
        if let Some(h) = hunk_header(line) {
            old_no = h.old_start;
            new_no = h.new_start;
            out.hunks.push(h);
            continue;
        }
        let Some(hunk) = out.hunks.last_mut() else {
            if line.starts_with("Binary files ") || line == "GIT binary patch" {
                out.binary = true;
            }
            continue;
        };
        if line.starts_with("diff --git ") {
            break;
        }
        let (kind, text) = match line.as_bytes().first() {
            Some(b' ') => (DiffLineKind::Context, &line[1..]),
            Some(b'+') => (DiffLineKind::Add, &line[1..]),
            Some(b'-') => (DiffLineKind::Del, &line[1..]),
            _ => continue,
        };
        let (o, n) = match kind {
            DiffLineKind::Context => (Some(old_no), Some(new_no)),
            DiffLineKind::Add => (None, Some(new_no)),
            DiffLineKind::Del => (Some(old_no), None),
        };
        if o.is_some() {
            old_no += 1;
        }
        if n.is_some() {
            new_no += 1;
        }
        match kind {
            DiffLineKind::Add => out.added += 1,
            DiffLineKind::Del => out.removed += 1,
            DiffLineKind::Context => {}
        }
        hunk.lines.push(DiffLine {
            kind,
            text: text.strip_suffix('\r').unwrap_or(text).to_string(),
            old_no: o,
            new_no: n,
        });
    }
    out
}

/// Parse `git diff --numstat -z` into `path -> (added, removed)`. Binary files count as 0/0.
/// A rename record (`a\tb\t\0old\0new\0`) is keyed by its new path.
pub fn parse_numstat_z(out: &[u8], prefix: &str) -> BTreeMap<String, (u32, u32)> {
    let mut map = BTreeMap::new();
    let mut records = out.split(|b| *b == 0);
    while let Some(rec) = records.next() {
        if rec.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(rec);
        let mut parts = text.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            records.next();
            match records.next() {
                Some(p) => String::from_utf8_lossy(p).to_string(),
                None => continue,
            }
        } else {
            path.to_string()
        };
        let Some(local) = path.strip_prefix(prefix) else {
            continue;
        };
        map.insert(
            local.to_string(),
            (a.parse().unwrap_or(0), r.parse().unwrap_or(0)),
        );
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_v2_with_renames_untracked_and_prefix() {
        let out = b"1 .M N... 100644 100644 100644 aaa aaa sub/a.txt\0\
2 R. N... 100644 100644 100644 bbb bbb R100 sub/moved name.txt\0top.txt\0\
1 A. N... 000000 100644 100644 000 ccc sub/new.rs\0\
1 MD N... 100644 100644 000000 ddd ddd sub/gone.rs\0\
u UU N... 100644 100644 100644 100644 e1 e2 e3 sub/conflict.rs\0\
? sub/notes.md\0\
? sub/scratch/\0\
1 .M N... 100644 100644 100644 fff fff other/x.rs\0\
! sub/target/\0";
        let st = parse_porcelain_v2(out, "sub/");
        assert!(st.repo);
        assert_eq!(st.file("a.txt"), Some((GitMark::Modified, false)));
        assert_eq!(st.file("moved name.txt"), Some((GitMark::Renamed, true)));
        assert_eq!(
            st.file("top.txt"),
            None,
            "the rename source is not an entry"
        );
        assert_eq!(st.file("new.rs"), Some((GitMark::Added, true)));
        assert_eq!(st.file("gone.rs"), Some((GitMark::Deleted, true)));
        assert_eq!(st.file("conflict.rs"), Some((GitMark::Modified, false)));
        assert_eq!(st.file("notes.md"), Some((GitMark::Untracked, false)));
        assert_eq!(
            st.file("scratch/deep/f.txt"),
            Some((GitMark::Untracked, false))
        );
        assert_eq!(st.file("scratchy.txt"), None);
        assert_eq!(
            st.file("x.rs"),
            None,
            "entries outside the project prefix are dropped"
        );
        assert_eq!(st.dir("scratch"), (Some(GitMark::Untracked), true));
        assert_eq!(st.dir(""), (None, true));
        assert_eq!(st.dir("target"), (None, false));
        assert_eq!(st.changed().count(), 7);
    }

    #[test]
    fn porcelain_folder_changes() {
        let st = parse_porcelain_v2(
            b"1 .M N... 100644 100644 100644 a a src/lib/x.rs\0? docs/new/\0",
            "",
        );
        assert_eq!(st.dir("src"), (None, true));
        assert_eq!(st.dir("src/lib"), (None, true));
        assert_eq!(st.dir("sr"), (None, false));
        assert_eq!(st.dir("docs"), (None, true));
        assert_eq!(st.dir("docs/new/inner"), (Some(GitMark::Untracked), true));
        assert_eq!(GitStatus::default().dir(""), (None, false));
    }

    #[test]
    fn unified_diff_hunks_and_line_numbers() {
        let text = "diff --git a/f.rs b/f.rs\nindex 1..2 100644\n--- a/f.rs\n+++ b/f.rs\n@@ -1,3 +1,4 @@ fn main() {\n a\n-b\n+B\n+c\n d\n@@ -10 +11,0 @@\n-gone\n\\ No newline at end of file\n";
        let d = parse_unified(text);
        assert!(!d.binary);
        assert_eq!((d.added, d.removed), (2, 2));
        assert_eq!(d.hunks.len(), 2);
        let h = &d.hunks[0];
        assert_eq!(
            (h.old_start, h.old_lines, h.new_start, h.new_lines),
            (1, 3, 1, 4)
        );
        assert_eq!(h.header, "fn main() {");
        let rows: Vec<(DiffLineKind, &str, Option<u32>, Option<u32>)> = h
            .lines
            .iter()
            .map(|l| (l.kind, l.text.as_str(), l.old_no, l.new_no))
            .collect();
        assert_eq!(
            rows,
            vec![
                (DiffLineKind::Context, "a", Some(1), Some(1)),
                (DiffLineKind::Del, "b", Some(2), None),
                (DiffLineKind::Add, "B", None, Some(2)),
                (DiffLineKind::Add, "c", None, Some(3)),
                (DiffLineKind::Context, "d", Some(3), Some(4)),
            ]
        );
        let h2 = &d.hunks[1];
        assert_eq!(
            (h2.old_start, h2.old_lines, h2.new_start, h2.new_lines),
            (10, 1, 11, 0)
        );
        assert_eq!(h2.lines.len(), 1, "the no-newline marker is not a line");
        assert_eq!(h2.lines[0].old_no, Some(10));
    }

    #[test]
    fn unified_diff_binary() {
        let d = parse_unified(
            "diff --git a/x.png b/x.png\nindex 1..2 100644\nBinary files a/x.png and b/x.png differ\n",
        );
        assert!(d.binary);
        assert!(d.hunks.is_empty());
    }

    #[test]
    fn numstat_z_with_rename_and_binary() {
        let out = b"3\t1\tsub/a.rs\0-\t-\tsub/img.png\0\x32\t0\t\0sub/old.rs\0sub/new.rs\x005\t5\tother.rs\0";
        let m = parse_numstat_z(out, "sub/");
        assert_eq!(m.get("a.rs"), Some(&(3, 1)));
        assert_eq!(m.get("img.png"), Some(&(0, 0)));
        assert_eq!(m.get("new.rs"), Some(&(2, 0)));
        assert!(!m.contains_key("old.rs"));
        assert!(!m.contains_key("other.rs"));
    }

    #[tokio::test]
    async fn listing_and_diffing_start_no_filter_driver() {
        let dir = tempfile::tempdir().unwrap();
        let repo = ostra_core::paths::canonical(dir.path()).unwrap();
        let marker = repo.join("ran");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        std::fs::write(repo.join(".gitattributes"), "*.txt filter=x\n").unwrap();
        let driver = format!(
            "touch {}; cat",
            marker.display().to_string().replace('\\', "/")
        );
        git(&["config", "filter.x.clean", &driver]);
        git(&["config", "filter.x.smudge", &driver]);
        std::fs::write(repo.join("a.txt"), "two\n").unwrap();

        let st = status(&repo).await;
        assert!(st.repo);
        let head = resolve_base(&repo, "HEAD").await.unwrap();
        let _ = diff_file(&repo, "a.txt", &head, false, 1 << 20).await;
        let _ = numstat(&repo, &head, &["a.txt".into()], "").await;
        assert!(!marker.exists(), "a filter driver ran");
    }

    #[tokio::test]
    async fn a_staged_nested_repo_starts_no_filter_driver() {
        let dir = tempfile::tempdir().unwrap();
        let repo = ostra_core::paths::canonical(dir.path()).unwrap();
        let nested = repo.join("a/evil");
        let marker = repo.join("ran");
        let git = |cwd: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(["-c", "core.fsmonitor=false", "-c", "filter.x.clean="])
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        git(&repo, &["init", "-q"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        std::fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q"]);
        std::fs::write(nested.join(".gitattributes"), "*.txt filter=x\n").unwrap();
        std::fs::write(nested.join("f.txt"), "one\n").unwrap();
        git(&nested, &["add", "-A"]);
        git(&nested, &["commit", "-qm", "x"]);
        let driver = format!(
            "touch {}; cat",
            marker.display().to_string().replace('\\', "/")
        );
        git(&nested, &["config", "filter.x.clean", &driver]);
        git(&repo, &["add", "a/evil"]);
        // Same size, newer mtime: git hashes the file again, through the clean filter.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(nested.join("f.txt"), "two\n").unwrap();

        let st = status(&repo).await;
        assert!(st.repo);
        let head = resolve_base(&repo, "HEAD").await.unwrap();
        let _ = numstat(&repo, &head, &["a/evil".into()], "").await;
        assert!(!marker.exists(), "a nested repo's filter driver ran");
        let plain = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(plain.status.success());
        assert!(
            marker.exists(),
            "the test repo no longer triggers the driver"
        );
    }

    #[tokio::test]
    async fn status_diff_and_numstat_on_a_real_repo_subfolder() {
        let dir = tempfile::tempdir().unwrap();
        let repo = ostra_core::paths::canonical(dir.path()).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        std::fs::create_dir_all(repo.join("app/src")).unwrap();
        std::fs::write(repo.join("app/src/a.rs"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(repo.join("app/old.rs"), "x\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        std::fs::write(repo.join("app/src/a.rs"), "one\n2\nthree\n").unwrap();
        std::fs::write(repo.join("app/new.txt"), "n\n").unwrap();
        git(&["mv", "app/old.rs", "app/renamed.rs"]);
        let app = repo.join("app");

        let st = status(&app).await;
        assert!(st.repo);
        assert_eq!(st.prefix, "app/");
        assert_eq!(st.file("src/a.rs"), Some((GitMark::Modified, false)));
        assert_eq!(st.file("renamed.rs"), Some((GitMark::Renamed, true)));
        assert_eq!(st.file("new.txt"), Some((GitMark::Untracked, false)));
        assert_eq!(st.dir("src"), (None, true));

        let head = resolve_base(&app, "HEAD").await.unwrap();
        assert!(resolve_base(&app, "no-such-branch").await.is_err());
        let (d, truncated) = diff_file(&app, "src/a.rs", &head, false, 1 << 20)
            .await
            .unwrap();
        assert!(!truncated);
        assert_eq!((d.added, d.removed, d.hunks.len()), (1, 1, 1));
        let (d, _) = diff_file(&app, "new.txt", "", true, 1 << 20).await.unwrap();
        assert_eq!((d.added, d.removed), (1, 0));
        let counts = numstat(&app, &head, &["src/a.rs".into()], &st.prefix).await;
        assert_eq!(counts.get("src/a.rs"), Some(&(1, 1)));

        let outside = tempfile::tempdir().unwrap();
        assert_eq!(
            status(outside.path()).await,
            GitStatus::default(),
            "a folder outside git has no status and no error"
        );
    }

    #[test]
    fn diff_base_must_not_be_an_option() {
        assert!(valid_base("HEAD"));
        assert!(valid_base("HEAD~2"));
        assert!(valid_base("origin/main"));
        assert!(valid_base("v1.2.3^{commit}"));
        assert!(!valid_base("--output=/tmp/x"));
        assert!(!valid_base("HEAD;rm"));
        assert!(!valid_base(""));
    }
}
