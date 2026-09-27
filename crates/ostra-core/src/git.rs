//! Config overrides for the git commands Ostra runs itself. A repository's `.git/config` is
//! written by whoever controls the folder, including an agent, so it must not choose programs
//! that git starts while Ostra lists files, shows a diff, or stages.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// For every git command Ostra starts: no fsmonitor program, no `ext::` transport, no askpass
/// program from the repository.
pub const NO_EXEC: &[&str] = &[
    "-c",
    "core.fsmonitor=false",
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "core.askPass=",
];

/// For git commands Ostra runs without the user asking (status, listings, diffs, staging during
/// review): [`NO_EXEC`] plus no hooks.
pub static AUTOMATIC: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut out: Vec<String> = NO_EXEC.iter().map(|s| s.to_string()).collect();
    out.push("-c".into());
    out.push(format!("core.hooksPath={}", no_hooks_dir()));
    out
});

/// A hooks dir that holds no hooks. Git for Windows reads `/dev/null/<hook>` as `\dev\null\<hook>`
/// on the current drive, where any local user may create folders, so Windows uses an empty dir
/// under Ostra's own data dir instead.
fn no_hooks_dir() -> String {
    if cfg!(windows) {
        let dir = crate::paths::data_dir().join("no-hooks");
        let _ = crate::paths::create_private_dir_all(&dir);
        dir.to_string_lossy().replace('\\', "/")
    } else {
        "/dev/null".into()
    }
}

/// A path git printed. Git for Windows prints UTF-8, and Unix git prints the raw bytes.
fn path_from_git(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(bytes).into()
    }
    #[cfg(not(unix))]
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

/// Settings for git commands an agent process runs (a harness CLI's shell, an MCP server), so a
/// `.git/config` an agent planted cannot start a program or reach an `ext::` transport. Hooks are
/// kept, because agent commits run the repository's own hooks.
pub const AGENT_CONFIG: &[(&str, &str)] = &[
    ("core.fsmonitor", "false"),
    ("safe.bareRepository", "explicit"),
    ("protocol.ext.allow", "never"),
];

/// `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_<n>`, and `GIT_CONFIG_VALUE_<n>` entries that add `pairs`
/// after the entries `existing` (the parent's `GIT_CONFIG_COUNT`) already holds, so both apply.
pub fn config_env(pairs: &[(&str, &str)], existing: Option<&str>) -> Vec<(String, String)> {
    let base: usize = existing.and_then(|c| c.trim().parse().ok()).unwrap_or(0);
    let mut env = vec![(
        "GIT_CONFIG_COUNT".to_string(),
        (base + pairs.len()).to_string(),
    )];
    for (i, (k, v)) in pairs.iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{}", base + i), k.to_string()));
        env.push((format!("GIT_CONFIG_VALUE_{}", base + i), v.to_string()));
    }
    env
}

/// [`config_env`] of [`AGENT_CONFIG`], appended to this process's `GIT_CONFIG_COUNT`.
pub fn agent_env() -> Vec<(String, String)> {
    config_env(
        AGENT_CONFIG,
        std::env::var("GIT_CONFIG_COUNT").ok().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_env_appends_to_an_existing_count() {
        let env = config_env(&[("a.b", "1")], Some("2"));
        assert_eq!(
            env,
            vec![
                ("GIT_CONFIG_COUNT".to_string(), "3".to_string()),
                ("GIT_CONFIG_KEY_2".to_string(), "a.b".to_string()),
                ("GIT_CONFIG_VALUE_2".to_string(), "1".to_string()),
            ]
        );
        assert_eq!(config_env(&[("a.b", "1")], None)[0].1, "1");
        assert_eq!(
            config_env(&[("a.b", "1")], Some("junk"))[1].0,
            "GIT_CONFIG_KEY_0"
        );
    }
}

/// `git config` arguments that list the filter drivers a repository names.
pub const FILTER_QUERY: &[&str] = &[
    "config",
    "--null",
    "--get-regexp",
    r"^filter\..+\.(clean|smudge|process)$",
];

/// The standard git-lfs drivers, kept so LFS files do not all look modified.
const LFS_DRIVERS: &[&str] = &[
    "git-lfs clean -- %f",
    "git-lfs smudge -- %f",
    "git-lfs filter-process",
];

/// `-c <key>=` for every filter driver in [`FILTER_QUERY`] output except git-lfs, so a status,
/// diff, or staging that Ostra runs itself starts no program a repository's config names.
pub fn blank_filters(query_output: &[u8]) -> Vec<String> {
    let mut out = vec![];
    for entry in query_output.split(|b| *b == 0) {
        let text = String::from_utf8_lossy(entry);
        let Some((key, value)) = text.split_once('\n') else {
            continue;
        };
        if !LFS_DRIVERS.contains(&value.trim()) {
            out.push("-c".to_string());
            out.push(format!("{key}="));
        }
    }
    out
}

/// Repos [`filter_overrides`] reads at most, which bounds the git processes it starts.
const MAX_FILTER_REPOS: usize = 256;

/// [`blank_filters`] for `root` and every submodule git enters from it, for the status, diff,
/// and staging Ostra runs itself.
pub async fn filter_overrides(root: &Path) -> Vec<String> {
    blank_filters_of(root, true).await
}

/// [`blank_filters`] for the submodules git enters from `root` but not `root` itself, for
/// commands the user asked for, so the repository's own filters (git-crypt, say) still apply.
pub async fn submodule_filter_overrides(root: &Path) -> Vec<String> {
    blank_filters_of(root, false).await
}

/// A status, diff, `add`, or commit runs `git status` inside each populated submodule in the
/// index, which reads that repository's own config, including one an agent created and staged.
/// The `-c` overrides reach those child processes, so each such repository's drivers are blanked.
async fn blank_filters_of(root: &Path, with_root: bool) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut queue = vec![(root.to_path_buf(), with_root)];
    let mut read = 0;
    while let Some((repo, blank)) = queue.pop() {
        if read == MAX_FILTER_REPOS {
            tracing::warn!(
                "stopped reading submodule filter drivers at {}: a status there may run one",
                repo.display()
            );
            break;
        }
        read += 1;
        if blank {
            for pair in blank_filters(&query(&repo, FILTER_QUERY).await).chunks(2) {
                if !out.contains(&pair[1]) {
                    out.extend_from_slice(pair);
                }
            }
        }
        let index = query(&repo, &["ls-files", "--stage", "-z"]).await;
        for entry in index.split(|b| *b == 0) {
            let Some(rest) = entry.strip_prefix(b"160000 ") else {
                continue;
            };
            let Some(tab) = rest.iter().position(|b| *b == b'\t') else {
                continue;
            };
            let sub = repo.join(path_from_git(&rest[tab + 1..]));
            if std::fs::symlink_metadata(sub.join(".git")).is_ok() {
                queue.push((sub, true));
            }
        }
    }
    out
}

async fn query(repo: &Path, args: &[&str]) -> Vec<u8> {
    tokio::process::Command::new("git")
        .args(AUTOMATIC.iter())
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .map(|o| o.stdout)
        .unwrap_or_default()
}

#[cfg(test)]
mod filter_tests {
    use super::*;

    #[test]
    fn blanks_every_driver_but_git_lfs() {
        let q = b"filter.x.clean\nsh -c true\0filter.lfs.clean\ngit-lfs clean -- %f\0filter.y.process\nprog\0";
        assert_eq!(
            blank_filters(q),
            ["-c", "filter.x.clean=", "-c", "filter.y.process="]
        );
        assert!(blank_filters(b"").is_empty());
    }

    /// A repo with one filter driver named `name` and a file it would filter, left modified.
    fn repo_with_driver(dir: &Path, name: &str) {
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        std::fs::create_dir_all(dir).unwrap();
        git(&["init", "-q"]);
        std::fs::write(dir.join(".gitattributes"), format!("*.txt filter={name}\n")).unwrap();
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        git(&["config", &format!("filter.{name}.clean"), "false"]);
        git(&["-c", &format!("filter.{name}.clean="), "add", "-A"]);
        git(&["commit", "-qm", "i"]);
    }

    #[tokio::test]
    async fn submodules_in_the_index_have_their_drivers_blanked_at_every_depth() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("top");
        repo_with_driver(&root, "own");
        repo_with_driver(&root.join("a/sub"), "one");
        repo_with_driver(&root.join("a/sub/deep"), "two");
        repo_with_driver(&root.join("untracked"), "three");
        for (dir, path) in [(root.join("a/sub"), "deep"), (root.clone(), "a/sub")] {
            let out = std::process::Command::new("git")
                .args(["-c", "core.fsmonitor=false", "add", path])
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
        }
        assert_eq!(
            filter_overrides(&root).await,
            [
                "-c",
                "filter.own.clean=",
                "-c",
                "filter.one.clean=",
                "-c",
                "filter.two.clean="
            ]
        );
        assert_eq!(
            submodule_filter_overrides(&root).await,
            ["-c", "filter.one.clean=", "-c", "filter.two.clean="]
        );
    }
}
