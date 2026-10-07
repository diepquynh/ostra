//! External MCP servers: the names their tools carry inside Ostra, and settings validation.
//!
//! A tool `create_issue` of the workspace server `github` has the bare name
//! `github__create_issue`. Harnesses see it through Ostra's own MCP server
//! (`mcp__ostra__github__create_issue`); the native loop, the policy, and permission rules use
//! the canonical name `mcp__github__create_issue`, Claude Code's spelling.

use crate::agent::AgentName;
use crate::config::{McpServerConfig, ValidationIssue};

pub const PREFIX: &str = "mcp__";
pub const SEPARATOR: &str = "__";

/// Longest bare name. Harnesses prefix `mcp__ostra__` (12 characters) and providers cap tool
/// names at 64.
pub const MAX_BARE_NAME: usize = 52;
pub const MAX_SERVER_NAME: usize = 24;
pub const MAX_TIMEOUT_SECS: u32 = 600;

/// Is `tool` a canonical external MCP tool name? Harness-native MCP calls arrive as
/// `Other:mcp__...` and do not count.
pub fn is_gateway_tool(tool: &str) -> bool {
    tool.strip_prefix(PREFIX)
        .is_some_and(|rest| rest.contains(SEPARATOR))
}

/// Is `bare` (a name served by Ostra's MCP server) an external tool? Ostra's own tools never
/// contain `__`.
pub fn is_gateway_bare(bare: &str) -> bool {
    bare.contains(SEPARATOR)
}

pub fn canonical(bare: &str) -> String {
    format!("{PREFIX}{bare}")
}

pub fn bare(canonical: &str) -> Option<&str> {
    canonical
        .strip_prefix(PREFIX)
        .filter(|rest| rest.contains(SEPARATOR))
}

/// The server a canonical or bare name belongs to.
pub fn server_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(PREFIX).unwrap_or(name);
    rest.split_once(SEPARATOR).map(|(s, _)| s)
}

/// Bare name of `tool` on `server`: `server__tool` when that is a valid tool name, else a
/// sanitized, shortened form with a hash of the original, so two tools never share a name.
pub fn bare_name(server: &str, tool: &str) -> String {
    let plain = format!("{server}{SEPARATOR}{tool}");
    let clean = tool
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if clean && plain.len() <= MAX_BARE_NAME {
        return plain;
    }
    let hash = format!("{:08x}", fnv1a(tool.as_bytes()) as u32);
    let mut sanitized: String = tool
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let room = MAX_BARE_NAME - server.len() - SEPARATOR.len() - 1 - hash.len();
    sanitized.truncate(room);
    format!("{server}{SEPARATOR}{sanitized}_{hash}")
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Server names are lowercase letters, digits, and single dashes, so `__` always splits the
/// server from the tool.
pub fn is_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SERVER_NAME
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Does this server serve `agent`? An empty `agents` list serves every agent.
pub fn serves(server: &McpServerConfig, agent: AgentName) -> bool {
    server.enabled
        && (server.agents.is_empty() || server.agents.iter().any(|a| a == agent.as_str()))
}

/// Save-time checks of `mcp_servers`.
pub fn validate(servers: &[McpServerConfig], agents: &[&str], issues: &mut Vec<ValidationIssue>) {
    let mut push = |path: String, message: String| issues.push(ValidationIssue { path, message });
    let mut seen = std::collections::BTreeSet::new();
    for (i, s) in servers.iter().enumerate() {
        let at = format!("mcp_servers[{i}]");
        if !is_server_name(&s.name) {
            push(
                format!("{at}.name"),
                format!(
                    "Name the server with lowercase letters, digits, and dashes, starting with a letter, at most {MAX_SERVER_NAME} characters. `{}` is not one.",
                    s.name
                ),
            );
        } else if s.name == "ostra" {
            push(
                format!("{at}.name"),
                "Choose another name: `ostra` is Ostra's own MCP server.".into(),
            );
        }
        if !seen.insert(s.name.clone()) {
            push(
                format!("{at}.name"),
                format!("Server name `{}` is used twice.", s.name),
            );
        }
        let has_command = !s.command.is_empty();
        match (&s.url, has_command) {
            (Some(_), true) => push(
                format!("{at}.url"),
                "Set either `command` (a local server) or `url` (a remote server), not both."
                    .into(),
            ),
            (None, false) => push(
                format!("{at}.command"),
                "Set `command` to run a local server, or `url` to reach a remote one.".into(),
            ),
            _ => {}
        }
        if has_command && s.command[0].trim().is_empty() {
            push(
                format!("{at}.command"),
                "Name the server program as the first item of command.".into(),
            );
        }
        if let Some(url) = &s.url
            && !(url.starts_with("https://") || url.starts_with("http://"))
        {
            push(
                format!("{at}.url"),
                format!("Use an http or https URL for the server, not `{url}`."),
            );
        }
        if has_command && !s.headers.is_empty() {
            push(
                format!("{at}.headers"),
                "Headers apply to remote servers only; pass secrets to a local server in `env`."
                    .into(),
            );
        }
        if has_command && s.oauth.is_some() {
            push(
                format!("{at}.oauth"),
                "OAuth applies to remote servers only; remove it or set `url`.".into(),
            );
        }
        for a in &s.agents {
            let known = if agents.is_empty() {
                AgentName::ALL.iter().any(|n| n.as_str() == a)
            } else {
                agents.contains(&a.as_str())
            };
            if !known {
                push(format!("{at}.agents"), format!("`{a}` is not an agent."));
            }
        }
        if s.timeout_secs == 0 || s.timeout_secs > MAX_TIMEOUT_SECS {
            push(
                format!("{at}.timeout_secs"),
                format!("Use a timeout from 1 to {MAX_TIMEOUT_SECS} seconds."),
            );
        }
    }
}

/// What `workspace.toml` holds in place of a literal header or env value, which Ostra keeps
/// encrypted in its registry, so the value never reaches the browser or a folder file.
pub const SAVED_SECRET: &str = "${ostra_secret}";

/// A header or env value typed as plain text rather than read from the server's environment.
pub fn is_literal_value(value: &str) -> bool {
    !value.trim().is_empty() && value != SAVED_SECRET && !value.contains("${")
}

/// Why a `${VAR}` in an MCP server's settings could not be filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandError {
    Unset(String),
    /// The variable holds a credential Ostra keeps from MCP servers (a provider key, a bridge
    /// token, an OAuth client secret).
    Refused(String),
}

/// Replace each `${VAR}` in `value` with the server's environment, refusing every name in
/// `refused`.
pub fn expand_env(
    value: &str,
    refused: &std::collections::BTreeSet<String>,
) -> Result<String, ExpandError> {
    expand_refusing(value, refused, |k| std::env::var(k).ok())
}

pub fn expand_refusing(
    value: &str,
    refused: &std::collections::BTreeSet<String>,
    get: impl Fn(&str) -> Option<String>,
) -> Result<String, ExpandError> {
    let hit = std::cell::RefCell::new(None);
    let out = expand_with(value, |k| {
        if refused.contains(k) {
            *hit.borrow_mut() = Some(k.to_string());
            return None;
        }
        get(k)
    });
    match (hit.into_inner(), out) {
        (Some(k), _) => Err(ExpandError::Refused(k)),
        (None, Ok(v)) => Ok(v),
        (None, Err(k)) => Err(ExpandError::Unset(k)),
    }
}

pub fn expand_with(value: &str, get: impl Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let key = &after[..end];
        let v = get(key).ok_or_else(|| key.to_string())?;
        out.push_str(&v);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(bare_name("github", "create_issue"), "github__create_issue");
        assert!(is_gateway_tool("mcp__github__create_issue"));
        assert!(!is_gateway_tool("Other:mcp__github__create_issue"));
        assert!(!is_gateway_tool("mcp__github"));
        assert_eq!(server_of("mcp__github__a__b"), Some("github"));
        assert_eq!(server_of("github__a"), Some("github"));
        let odd = bare_name("docs", "search.pages/v2");
        assert!(odd.starts_with("docs__search_pages_v2_"), "{odd}");
        assert_ne!(odd, bare_name("docs", "search_pages_v2"));
        let long = bare_name("a-long-server-name", &"x".repeat(80));
        assert_eq!(long.len(), MAX_BARE_NAME);
        assert_ne!(long, bare_name("a-long-server-name", &"x".repeat(81)));
    }

    #[test]
    fn env_expansion() {
        let get = |k: &str| (k == "T").then(|| "tok".to_string());
        assert_eq!(expand_with("Bearer ${T}", get).unwrap(), "Bearer tok");
        assert_eq!(expand_with("${MISSING}x", get).unwrap_err(), "MISSING");
        assert_eq!(expand_with("plain $T", get).unwrap(), "plain $T");
        let refused = ["ANTHROPIC_API_KEY".to_string()].into_iter().collect();
        let get = |k: &str| Some(format!("v-{k}"));
        assert_eq!(
            expand_refusing("Bearer ${GH}", &refused, get).unwrap(),
            "Bearer v-GH"
        );
        assert_eq!(
            expand_refusing("x ${ANTHROPIC_API_KEY}", &refused, get),
            Err(ExpandError::Refused("ANTHROPIC_API_KEY".into()))
        );
        assert_eq!(
            expand_refusing("${NOPE}", &refused, |_| None),
            Err(ExpandError::Unset("NOPE".into()))
        );
    }

    #[test]
    fn validation() {
        let ok = McpServerConfig::remote("docs", "https://example.com/mcp");
        let mut bad = McpServerConfig::remote("Bad_Name", "ftp://x");
        bad.command = vec!["x".into()];
        bad.agents = vec!["nobody".into()];
        bad.timeout_secs = 0;
        let mut issues = vec![];
        validate(&[ok.clone(), ok, bad], &[], &mut issues);
        let paths: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "mcp_servers[1].name",
                "mcp_servers[2].name",
                "mcp_servers[2].url",
                "mcp_servers[2].url",
                "mcp_servers[2].agents",
                "mcp_servers[2].timeout_secs",
            ]
        );
    }
}
