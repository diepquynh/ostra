//! Sandbox settings checked at save time (Pattern 7): the global `[sandbox]` table and a
//! workspace's own sandbox entries. A value the sandbox cannot enforce is an issue here, never a
//! silent fallback at spawn time.

use ostra_core::config::{SandboxConfig, ValidationIssue, WorkspaceSettings};

/// A workspace's sandbox entries: allowed hosts, decoys, readable paths, and blocked ports.
pub fn workspace(ws: &WorkspaceSettings) -> Vec<ValidationIssue> {
    let mut issues = validate_hosts("sandbox_allowed_hosts", &ws.sandbox_allowed_hosts);
    issues.extend(validate_decoys(&ws.sandbox_decoys));
    issues.extend(validate_readable("sandbox_readable", &ws.sandbox_readable));
    for (i, d) in ws.sandbox_decoys.iter().enumerate() {
        if ws.sandbox_readable.iter().any(|r| r.trim() == d.trim()) {
            issues.push(ValidationIssue {
                path: format!("sandbox_decoys[{i}]"),
                message: format!(
                    "Remove `{d}` from the decoys or from the readable paths, because a decoy there would replace the file agents are allowed to read."
                ),
            });
        }
    }
    issues.extend(validate_blocked_ports(&ws.sandbox_blocked_ports));
    issues
}

/// An `allowed_hosts` list that does not parse, one issue per entry, under `path`.
fn validate_hosts(path: &str, hosts: &[String]) -> Vec<ValidationIssue> {
    hosts
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            let message = match crate::egress::HostRule::parse(h) {
                None => format!(
                    "Write `{h}` as a host name, `*.domain`, or an IP address, optionally with `:port`, because the egress proxy matches hosts, not URLs."
                ),
                Some(r) if r.is_loopback() && r.port().is_none() => format!(
                    "Add the port to `{h}`, such as `{h}:8080`, because programs connect to loopback directly and the sandbox forwards only the ports you list."
                ),
                Some(_) => return None,
            };
            Some(ValidationIssue {
                path: format!("{path}[{i}]"),
                message,
            })
        })
        .collect()
}

/// Blocked loopback ports a workspace may list, because each is a rule in every sandbox policy.
pub const MAX_BLOCKED_PORTS: usize = 64;

/// A `sandbox_blocked_ports` list: real ports, at most [`MAX_BLOCKED_PORTS`].
fn validate_blocked_ports(ports: &[u16]) -> Vec<ValidationIssue> {
    let mut issues: Vec<ValidationIssue> = ports
        .iter()
        .enumerate()
        .filter(|(_, p)| **p == 0)
        .map(|(i, _)| ValidationIssue {
            path: format!("sandbox_blocked_ports[{i}]"),
            message: "Write a port from 1 to 65535, because port 0 names no service.".into(),
        })
        .collect();
    if ports.len() > MAX_BLOCKED_PORTS {
        issues.push(ValidationIssue {
            path: "sandbox_blocked_ports".into(),
            message: format!(
                "List at most {MAX_BLOCKED_PORTS} ports, because each one is a rule in every sandbox policy."
            ),
        });
    }
    issues
}

/// A `sandbox_decoys` list: `~/` file paths, no harness sign-in file, at most
/// [`crate::decoy::MAX_WORKSPACE_DECOYS`].
fn validate_decoys(decoys: &[String]) -> Vec<ValidationIssue> {
    let mut issues: Vec<ValidationIssue> = decoys
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            crate::decoy::invalid(d).map(|message| ValidationIssue {
                path: format!("sandbox_decoys[{i}]"),
                message,
            })
        })
        .collect();
    let max = crate::decoy::MAX_WORKSPACE_DECOYS;
    if decoys.len() > max {
        issues.push(ValidationIssue {
            path: "sandbox_decoys".into(),
            message: format!(
                "List at most {max} decoys, because each one is a watched file and a mount in every agent sandbox."
            ),
        });
    }
    issues
}

/// Readable paths one list may hold, because each one is a mount in every sandbox.
pub const MAX_READABLE: usize = 32;

/// A readable list (`extra_readable` or `sandbox_readable`): absolute or `~/` paths that name
/// neither the home folder, a dir above it, nor any of Ostra's own files.
fn validate_readable(key: &str, entries: &[String]) -> Vec<ValidationIssue> {
    let home = ostra_core::paths::home().unwrap_or_default();
    let private = ostra_core::paths::ostra_private_paths();
    let mut issues: Vec<ValidationIssue> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let t = e.trim();
            let p = match t.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => std::path::PathBuf::from(t),
            };
            let message = if !(t.starts_with('/') || t.starts_with("~/")) || t.contains('\0') {
                format!(
                    "Write `{t}` as an absolute path or one starting with `~/`, because a relative path would depend on where an execution starts."
                )
            } else if home.starts_with(&p) || p.components().count() < 2 {
                format!(
                    "Name the credential file or its dir instead of `{t}`, such as `~/.m2/settings.xml`, because each entry lets agents read everything under it."
                )
            } else if private.iter().any(|o| o.starts_with(&p) || p.starts_with(o)) {
                format!(
                    "Remove `{t}`, because it reaches Ostra's own data, config, or key file, which agents never read."
                )
            } else {
                return None;
            };
            Some(ValidationIssue {
                path: format!("{key}[{i}]"),
                message,
            })
        })
        .collect();
    if entries.len() > MAX_READABLE {
        issues.push(ValidationIssue {
            path: key.into(),
            message: format!(
                "List at most {MAX_READABLE} readable paths, because each one is a mount in every agent sandbox."
            ),
        });
    }
    issues
}

/// Every `[sandbox]` path must be absolute or start with `~/`, because a relative path would
/// resolve against whatever directory an execution happens to start in.
pub fn global(cfg: &SandboxConfig) -> Vec<ValidationIssue> {
    let mut issues = vec![];
    for (key, list) in [
        ("extra_writable", &cfg.extra_writable),
        ("extra_hidden", &cfg.extra_hidden),
    ] {
        for (i, p) in list.iter().enumerate() {
            let p = p.trim();
            if !(p.starts_with('/') || p.starts_with("~/")) || p.contains('\0') {
                issues.push(ValidationIssue {
                    path: format!("sandbox.{key}[{i}]"),
                    message: format!(
                        "Write `{p}` as an absolute path or one starting with `~/`, because a relative path would depend on where an execution starts."
                    ),
                });
            }
        }
    }
    issues.extend(validate_readable(
        "sandbox.extra_readable",
        &cfg.extra_readable,
    ));
    issues.extend(validate_hosts("sandbox.allowed_hosts", &cfg.allowed_hosts));
    if let Some(u) = &cfg.upstream_proxy
        && crate::egress::parse_upstream(u).is_none()
    {
        issues.push(ValidationIssue {
            path: "sandbox.upstream_proxy".into(),
            message: format!(
                "Write `{u}` as `http://host:port`, without a path or credentials, because the egress proxy connects to it with plain HTTP CONNECT."
            ),
        });
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_hosts_are_hosts_and_loopback_needs_a_port() {
        let bad = SandboxConfig {
            allowed_hosts: vec![
                "https://x.dev/path".into(),
                "*.corp.example:8443".into(),
                "localhost".into(),
                "127.0.0.1:8317".into(),
                "[::1]".into(),
            ],
            ..Default::default()
        };
        let issues = global(&bad);
        let paths: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "sandbox.allowed_hosts[0]",
                "sandbox.allowed_hosts[2]",
                "sandbox.allowed_hosts[4]"
            ]
        );
        assert!(issues[1].message.starts_with("Add the port to `localhost`"));
    }

    #[test]
    fn workspace_decoys_are_validated_and_capped() {
        let paths = |decoys: Vec<String>| -> Vec<String> {
            let ws = WorkspaceSettings {
                name: "x".into(),
                sandbox_decoys: decoys,
                ..Default::default()
            };
            workspace(&ws)
                .into_iter()
                .map(|i| i.path)
                .filter(|p| p.starts_with("sandbox_decoys"))
                .collect()
        };
        assert!(paths(vec!["~/.aws/credentials".into()]).is_empty());
        assert_eq!(
            paths(vec![
                "~/.aws/credentials".into(),
                "/etc/shadow".into(),
                "~/.codex/auth.json".into()
            ]),
            vec!["sandbox_decoys[1]", "sandbox_decoys[2]"]
        );
        let many = (0..=crate::decoy::MAX_WORKSPACE_DECOYS)
            .map(|i| format!("~/.d{i}"))
            .collect();
        assert_eq!(paths(many), vec!["sandbox_decoys"]);
    }

    #[test]
    fn readable_paths_name_credentials_and_never_ostra_or_the_whole_home() {
        let home = ostra_core::paths::home().unwrap_or_default();
        let data = ostra_core::paths::data_dir();
        let ws = WorkspaceSettings {
            name: "x".into(),
            sandbox_readable: vec![
                "~/.m2/settings.xml".into(),
                "relative/.netrc".into(),
                "~/".into(),
                "/".into(),
                home.to_string_lossy().into_owned(),
                data.join("registry.db").to_string_lossy().into_owned(),
                "~/.git-credentials".into(),
            ],
            sandbox_decoys: vec!["~/.git-credentials".into()],
            ..Default::default()
        };
        let paths: Vec<String> = workspace(&ws).into_iter().map(|i| i.path).collect();
        assert_eq!(
            paths,
            [
                "sandbox_readable[1]",
                "sandbox_readable[2]",
                "sandbox_readable[3]",
                "sandbox_readable[4]",
                "sandbox_readable[5]",
                "sandbox_decoys[0]"
            ]
        );
        let cfg = SandboxConfig {
            extra_readable: vec!["~/.docker/config.json".into(), "netrc".into()],
            ..Default::default()
        };
        let issues: Vec<String> = global(&cfg).into_iter().map(|i| i.path).collect();
        assert_eq!(issues, ["sandbox.extra_readable[1]"]);
    }
}
