//! The Git dock: a project's branch and changes, and the git commands the browser runs on them
//! (stage, unstage, commit, fetch, push, switch and create branches). Every command that changes
//! the checkout waits for sessions working in the project, holds the project's git claim, and
//! announces the change so the Files dock reloads.

use crate::app::App;
use crate::files::git::run;
use crate::git::{GitAuth, GitError, branch_ok, claim, git_quiet, remote_auth, run_git};
use ostra_core::api::{
    GitBranch, GitChange, GitCheckoutRequest, GitMark, GitOpResult, GitPaths, GitRepoStatus,
};
use ostra_core::ids::SessionId;
use ostra_workspace::WorkspaceRt;
use std::path::Path;
use std::time::Duration;

/// Each change list holds at most this many paths.
const LIST_CAP: usize = 2_000;
const BRANCH_CAP: usize = 500;
const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
/// Commit hooks may run formatters and tests.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const REMOTE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub fn busy_text(s: &SessionId, key: &str) -> String {
    format!(
        "Stop session {s} or wait for it to finish, then try again, because it is working in `{key}`."
    )
}

fn mark_of(c: u8) -> Option<GitMark> {
    match c {
        b'M' | b'T' => Some(GitMark::Modified),
        b'A' => Some(GitMark::Added),
        b'D' => Some(GitMark::Deleted),
        b'R' | b'C' => Some(GitMark::Renamed),
        _ => None,
    }
}

fn push_capped(list: &mut Vec<GitChange>, truncated: &mut bool, c: GitChange) {
    if list.len() < LIST_CAP {
        list.push(c);
    } else {
        *truncated = true;
    }
}

/// Parse `git status --porcelain=v2 --branch -z`. Paths are relative to the repository root;
/// entries outside `prefix`, the project's place in the repository, are dropped.
pub fn parse_status(out: &[u8], prefix: &str) -> GitRepoStatus {
    let mut st = GitRepoStatus {
        is_git: true,
        ..Default::default()
    };
    let local = |p: &[u8]| -> Option<String> {
        let p = String::from_utf8_lossy(p);
        p.strip_prefix(prefix)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let mut records = out.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        let text = String::from_utf8_lossy(rec);
        if let Some(h) = text.strip_prefix("# ") {
            let (k, v) = h.split_once(' ').unwrap_or((h, ""));
            match k {
                "branch.oid" if v != "(initial)" => st.head = Some(v.chars().take(7).collect()),
                "branch.head" if v != "(detached)" => st.branch = Some(v.to_string()),
                "branch.upstream" => st.upstream = Some(v.to_string()),
                "branch.ab" => {
                    for part in v.split(' ') {
                        let n = |s: &str| s.parse().unwrap_or(0);
                        if let Some(a) = part.strip_prefix('+') {
                            st.ahead = n(a);
                        } else if let Some(b) = part.strip_prefix('-') {
                            st.behind = n(b);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let n = match rec[0] {
            b'1' => 9,
            b'2' => 10,
            b'u' => 11,
            b'?' => {
                if let Some(p) = rec.get(2..).and_then(local) {
                    let c = GitChange {
                        path: p.trim_end_matches('/').to_string(),
                        mark: GitMark::Untracked,
                        orig_path: None,
                    };
                    push_capped(&mut st.unstaged, &mut st.truncated, c);
                }
                continue;
            }
            _ => continue,
        };
        let f: Vec<&[u8]> = rec.splitn(n, |b| *b == b' ').collect();
        // The rename's source path follows as its own NUL-terminated record.
        let orig = if rec[0] == b'2' { records.next() } else { None };
        if f.len() != n || f[1].len() != 2 {
            continue;
        }
        let Some(path) = local(f[n - 1]) else {
            continue;
        };
        let (x, y) = (f[1][0], f[1][1]);
        if rec[0] == b'u' {
            let c = GitChange {
                path,
                mark: GitMark::Modified,
                orig_path: None,
            };
            push_capped(&mut st.conflicted, &mut st.truncated, c);
            continue;
        }
        if let Some(mark) = mark_of(x) {
            let c = GitChange {
                path: path.clone(),
                mark,
                orig_path: orig.and_then(local),
            };
            push_capped(&mut st.staged, &mut st.truncated, c);
        }
        if let Some(mark) = mark_of(y) {
            let c = GitChange {
                path,
                mark,
                orig_path: None,
            };
            push_capped(&mut st.unstaged, &mut st.truncated, c);
        }
    }
    st
}

/// The project's branch and changes. A folder outside any git work tree answers `is_git: false`.
pub async fn status(w: &WorkspaceRt, key: &str, root: &Path) -> GitRepoStatus {
    let busy = crate::git::session_in(w, key)
        .ok()
        .flatten()
        .map(|s| busy_text(&s, key));
    let Some(prefix) = crate::files::git::prefix(root).await else {
        return GitRepoStatus {
            busy,
            ..Default::default()
        };
    };
    let args = [
        "status",
        "--porcelain=v2",
        "--branch",
        "-z",
        "--untracked-files=normal",
        "--ignore-submodules=all",
        "--",
        ".",
    ];
    let mut st = match run(root, &args, 32 * 1024 * 1024).await {
        Some(out) if out.success => parse_status(&out.stdout, &prefix),
        _ => GitRepoStatus {
            is_git: true,
            ..Default::default()
        },
    };
    if !prefix.is_empty()
        && let Some(out) = run(
            root,
            &["diff", "--cached", "--name-only", "-z"],
            8 * 1024 * 1024,
        )
        .await
        && out.success
    {
        st.staged_elsewhere = out
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty() && !p.starts_with(prefix.as_bytes()))
            .count() as u32;
    }
    st.busy = busy;
    st
}

/// Local and remote-tracking branches, the current one first, then by name.
pub async fn branches(root: &Path) -> Vec<GitBranch> {
    let format = "%(refname)%00%(refname:short)%00%(HEAD)%00%(upstream:short)%00%(objectname:short)%00%(subject)";
    let args = [
        "for-each-ref",
        &format!("--format={format}"),
        &format!("--count={BRANCH_CAP}"),
        "refs/heads",
        "refs/remotes",
    ];
    let Some(out) = run(root, &args, 4 * 1024 * 1024)
        .await
        .filter(|o| o.success)
    else {
        return vec![];
    };
    let mut list: Vec<GitBranch> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\0').collect();
            let [full, short, head, upstream, commit, subject] = f[..] else {
                return None;
            };
            let remote = full.starts_with("refs/remotes/");
            if remote && full.ends_with("/HEAD") {
                return None;
            }
            Some(GitBranch {
                name: short.to_string(),
                remote,
                current: head == "*",
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                commit: commit.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect();
    list.sort_by(|a, b| {
        (b.current, !b.remote)
            .cmp(&(a.current, !a.remote))
            .then(a.name.cmp(&b.name))
    });
    list
}

/// Everything a changing command needs: the claim, and git output with local authentication.
pub struct Op<'a> {
    app: &'a App,
    w: &'a WorkspaceRt,
    key: &'a str,
    root: &'a Path,
}

impl<'a> Op<'a> {
    pub fn new(app: &'a App, w: &'a WorkspaceRt, key: &'a str, root: &'a Path) -> Self {
        Op { app, w, key, root }
    }

    /// Run `f` under the project's git claim, refused while a session works in the project, then
    /// announce the change and answer the new status.
    pub async fn run<F, Fut>(&self, f: F) -> Result<GitOpResult, GitError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<String, GitError>>,
    {
        if let Some(s) = crate::git::session_in(self.w, self.key).ok().flatten() {
            return Err(GitError::Busy(busy_text(&s, self.key)));
        }
        if crate::files::git::prefix(self.root).await.is_none() {
            return Err(GitError::Git(format!(
                "`{}` is not in a git repository. Run git init in it first.",
                self.key
            )));
        }
        let result = {
            let _claim = claim(self.w, self.key, self.root)?;
            f().await
        };
        crate::files::announce_git(self.app, &self.w.id, self.key);
        let output = result?;
        Ok(GitOpResult {
            output,
            status: status(self.w, self.key, self.root).await,
        })
    }

    pub async fn local(&self, args: &[&str], timeout: Duration) -> Result<String, GitError> {
        let auth = GitAuth::new(None).map_err(|e| GitError::Git(e.to_string()))?;
        let mut all = vec!["--literal-pathspecs"];
        all.extend_from_slice(args);
        run_git(Some(self.root), &auth, &all, timeout, |_| {})
            .await
            .map_err(GitError::Git)
    }

    /// Project-relative paths as pathspecs, each checked to stay inside the project. No paths
    /// means the whole project.
    pub fn pathspecs(&self, req: &GitPaths) -> Result<Vec<String>, GitError> {
        if req.paths.is_empty() {
            return Ok(vec![".".into()]);
        }
        req.paths
            .iter()
            .map(|p| {
                let c = crate::files::tree::contain(self.root, p).map_err(GitError::Invalid)?;
                Ok(if c.rel.is_empty() { ".".into() } else { c.rel })
            })
            .collect()
    }

    pub async fn stage(&self, req: &GitPaths) -> Result<GitOpResult, GitError> {
        let specs = self.pathspecs(req)?;
        self.run(|| async {
            let mut args = vec!["add", "-A", "--"];
            args.extend(specs.iter().map(String::as_str));
            self.local(&args, LOCAL_TIMEOUT).await
        })
        .await
    }

    pub async fn unstage(&self, req: &GitPaths) -> Result<GitOpResult, GitError> {
        let specs = self.pathspecs(req)?;
        self.run(|| async {
            // Before the first commit there is no HEAD to restore from, so the index entry goes.
            let born = git_quiet(self.root, &["rev-parse", "--verify", "-q", "HEAD"])
                .await
                .is_some();
            let mut args = if born {
                vec!["restore", "--staged", "--"]
            } else {
                vec!["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--"]
            };
            args.extend(specs.iter().map(String::as_str));
            self.local(&args, LOCAL_TIMEOUT).await
        })
        .await
    }

    pub async fn commit(&self, message: &str) -> Result<GitOpResult, GitError> {
        let message = message.trim();
        if message.is_empty() {
            return Err(GitError::Invalid("Write a commit message first.".into()));
        }
        let before = status(self.w, self.key, self.root).await;
        if before.staged.is_empty() && before.staged_elsewhere == 0 {
            return Err(GitError::Invalid(
                "Stage the changes to commit first. Nothing is staged.".into(),
            ));
        }
        if !before.conflicted.is_empty() {
            return Err(GitError::Invalid(
                "Resolve the conflicted files and stage them, then commit.".into(),
            ));
        }
        self.run(|| async {
            self.local(&["commit", "--no-edit", "-m", message], COMMIT_TIMEOUT)
                .await
        })
        .await
    }

    /// The current branch's remote: its configured one, else `origin`, else the only remote.
    async fn remote(&self, branch: Option<&str>) -> Result<String, GitError> {
        if let Some(b) = branch
            && let Some(r) = git_quiet(
                self.root,
                &["config", "--get", &format!("branch.{b}.remote")],
            )
            .await
        {
            return Ok(r);
        }
        let remotes = git_quiet(self.root, &["remote"]).await.unwrap_or_default();
        let names: Vec<&str> = remotes
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        match names[..] {
            [] => Err(GitError::Invalid(
                "Add a remote to the repository first, because it has none to push to or fetch from.".into(),
            )),
            _ if names.contains(&"origin") => Ok("origin".into()),
            [only] => Ok(only.into()),
            _ => Err(GitError::Invalid(format!(
                "Set an upstream for this branch with git, because the repository has several remotes: {}.",
                names.join(", ")
            ))),
        }
    }

    async fn branch(&self) -> Option<String> {
        git_quiet(self.root, &["symbolic-ref", "--short", "-q", "HEAD"]).await
    }

    pub async fn fetch(&self) -> Result<GitOpResult, GitError> {
        let branch = self.branch().await;
        let remote = self.remote(branch.as_deref()).await?;
        let auth = remote_auth(self.app, self.root, &remote, false).await?;
        self.run(|| async {
            run_git(
                Some(self.root),
                &auth,
                &["fetch", "--prune", "--", &remote],
                REMOTE_TIMEOUT,
                |_| {},
            )
            .await
            .map_err(GitError::Git)
        })
        .await
    }

    /// Push the current branch to its upstream, or to a same-named branch on the remote, which
    /// then becomes its upstream.
    pub async fn push(&self) -> Result<GitOpResult, GitError> {
        let Some(branch) = self.branch().await else {
            return Err(GitError::Invalid(
                "Switch to a branch first, because HEAD is detached and there is nothing to push."
                    .into(),
            ));
        };
        let remote = self.remote(Some(&branch)).await?;
        let merge = git_quiet(
            self.root,
            &["config", "--get", &format!("branch.{branch}.merge")],
        )
        .await;
        let auth = remote_auth(self.app, self.root, &remote, true).await?;
        let refspec = match &merge {
            Some(m) => format!("HEAD:{m}"),
            None => format!("HEAD:refs/heads/{branch}"),
        };
        self.run(|| async {
            let mut args = vec!["push", "--porcelain"];
            if merge.is_none() {
                args.push("--set-upstream");
            }
            args.extend(["--", remote.as_str(), refspec.as_str()]);
            run_git(Some(self.root), &auth, &args, REMOTE_TIMEOUT, |_| {})
                .await
                .map_err(GitError::Git)
        })
        .await
    }

    pub async fn checkout(&self, req: &GitCheckoutRequest) -> Result<GitOpResult, GitError> {
        let name = req.branch.trim();
        if !branch_ok(name) || name.ends_with('/') || name.ends_with(".lock") {
            return Err(GitError::Invalid(format!("`{name}` is not a branch name.")));
        }
        let start = req
            .start
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(s) = start.filter(|s| !crate::files::git::valid_base(s)) {
            return Err(GitError::Invalid(format!(
                "`{s}` is not a commit or branch name."
            )));
        }
        let list = branches(self.root).await;
        let local = |n: &str| list.iter().any(|b| !b.remote && b.name == n);
        let args: Vec<String> = if req.create {
            if local(name) {
                return Err(GitError::Invalid(format!(
                    "A branch named `{name}` already exists. Choose another name, or switch to it."
                )));
            }
            let mut a = vec!["switch".into(), "-c".into(), name.into()];
            a.extend(start.map(str::to_string));
            a
        } else if local(name) {
            vec!["switch".into(), name.into()]
        } else if let Some(b) = list.iter().find(|b| b.remote && b.name == name) {
            // A remote branch checks out as a local branch of the same name that tracks it.
            let short = b.name.split_once('/').map_or(b.name.as_str(), |(_, s)| s);
            if local(short) {
                vec!["switch".into(), short.into()]
            } else {
                vec!["switch".into(), "--track".into(), name.into()]
            }
        } else {
            return Err(GitError::Invalid(format!("No branch named `{name}`.")));
        };
        let out = self
            .run(|| async {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                self.local(&args, LOCAL_TIMEOUT).await
            })
            .await;
        crate::git::workspace_updated(self.app, self.w);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(parts: &[&str]) -> Vec<u8> {
        parts.iter().flat_map(|p| p.bytes().chain([0])).collect()
    }

    #[test]
    fn status_splits_staged_unstaged_and_conflicts() {
        let out = rec(&[
            "# branch.oid 0123456789abcdef",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "1 M. N... 100644 100644 100644 a a app/src/lib.rs",
            "1 .M N... 100644 100644 100644 a a app/README.md",
            "1 MD N... 100644 100644 000000 a a app/both.rs",
            "2 R. N... 100644 100644 100644 a a R100 app/new.rs",
            "app/old.rs",
            "u UU N... 100644 100644 100644 100644 a b c app/merge.rs",
            "? app/notes.md",
            "? app/scratch/",
            "? other/outside.rs",
            "1 M. N... 100644 100644 100644 a a other/x.rs",
        ]);
        let st = parse_status(&out, "app/");
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.head.as_deref(), Some("0123456"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        let paths = |l: &[GitChange]| {
            l.iter()
                .map(|c| (c.path.clone(), c.mark))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            paths(&st.staged),
            [
                ("src/lib.rs".into(), GitMark::Modified),
                ("both.rs".into(), GitMark::Modified),
                ("new.rs".into(), GitMark::Renamed),
            ]
        );
        assert_eq!(st.staged[2].orig_path.as_deref(), Some("old.rs"));
        assert_eq!(
            paths(&st.unstaged),
            [
                ("README.md".into(), GitMark::Modified),
                ("both.rs".into(), GitMark::Deleted),
                ("notes.md".into(), GitMark::Untracked),
                ("scratch".into(), GitMark::Untracked),
            ]
        );
        assert_eq!(
            paths(&st.conflicted),
            [("merge.rs".into(), GitMark::Modified)]
        );
    }

    #[test]
    fn a_new_repository_has_no_head_and_detached_has_no_branch() {
        let st = parse_status(&rec(&["# branch.oid (initial)", "# branch.head main"]), "");
        assert_eq!((st.head, st.branch.as_deref()), (None, Some("main")));
        let st = parse_status(
            &rec(&["# branch.oid abcdef1234", "# branch.head (detached)"]),
            "",
        );
        assert_eq!(st.branch, None);
    }
}
