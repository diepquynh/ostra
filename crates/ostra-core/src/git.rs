//! Config overrides for the git commands Ostra runs itself. A repository's `.git/config` is
//! written by whoever controls the folder, including an agent, so it must not choose programs
//! that git starts while Ostra lists files, shows a diff, or stages.

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
pub const AUTOMATIC: &[&str] = &[
    "-c",
    "core.fsmonitor=false",
    "-c",
    "protocol.ext.allow=never",
    "-c",
    "core.askPass=",
    "-c",
    "core.hooksPath=/dev/null",
];

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
    let mut env = vec![("GIT_CONFIG_COUNT".to_string(), (base + pairs.len()).to_string())];
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
        assert_eq!(config_env(&[("a.b", "1")], Some("junk"))[1].0, "GIT_CONFIG_KEY_0");
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
}
