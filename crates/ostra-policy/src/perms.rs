//! Permission rules in Claude Code's syntax: `Tool` or `Tool(spec)`.
//!
//! - `Bash(npm run test *)`: glob over one simple command, `*` matching anything. A trailing
//!   ` *` also matches the bare command. The legacy `Bash(npm test:*)` form is a prefix match.
//! - `Edit(src/**)`, `Write(...)`, `Read(~/.ssh/**)`: gitignore-style paths. `//x` is absolute,
//!   `~/x` is under home, `/x` and `x/y` are relative to the repo root, and a bare name matches
//!   at any depth. `Edit` rules cover every file-writing tool; `Read` rules cover Grep and Glob.
//! - `WebFetch(domain:docs.rs)`: the host or any subdomain of it.

use crate::bash::SimpleCommand;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::Regex;
use std::path::{Path, PathBuf};

const KNOWN_TOOLS: &[&str] = &[
    "Bash",
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "ApplyPatch",
    "Grep",
    "Glob",
    "Skill",
    "WebSearch",
    "WebFetch",
    "Report",
    "Document",
    "Memory",
    "MemoryRecall",
    "CodeOutline",
    "CodeFind",
    "CodeCallers",
    "CodeCallees",
    "CodeImplementations",
    "CodeNeighbors",
    "CodeImpact",
    "CodeMap",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub raw: String,
    pub tool: String,
    pub spec: Option<String>,
}

pub fn parse_rule(raw: &str) -> Result<Rule, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("Write a rule as `Tool` or `Tool(pattern)`: this one is empty.".into());
    }
    let (tool, spec) = match s.find('(') {
        Some(open) => {
            if !s.ends_with(')') {
                return Err(format!(
                    "Close the parenthesis in `{s}`: a rule is `Tool(pattern)`."
                ));
            }
            let spec = s[open + 1..s.len() - 1].trim();
            if spec.is_empty() {
                return Err(format!(
                    "Put a pattern inside the parentheses of `{s}`, or drop them to match every use."
                ));
            }
            (s[..open].trim(), Some(spec.to_string()))
        }
        None => (s, None),
    };
    let valid_name = tool.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && tool
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.'));
    if !valid_name {
        return Err(format!("`{tool}` is not a tool name."));
    }
    let known = KNOWN_TOOLS.contains(&tool)
        || tool.starts_with("mcp__")
        || tool.starts_with("Other:")
        || tool.starts_with("submit_");
    if !known {
        return Err(format!(
            "`{tool}` is not a tool Ostra knows. Use one of {}, or `Other:<harness tool>`.",
            KNOWN_TOOLS.join(", ")
        ));
    }
    Ok(Rule {
        raw: s.to_string(),
        tool: tool.to_string(),
        spec,
    })
}

/// Settings validation for one rule string.
pub fn validate_rule(rule: &str) -> Result<(), String> {
    let r = parse_rule(rule)?;
    if let Some(spec) = &r.spec {
        match family(&r.tool) {
            Family::Edit | Family::Read => {
                let home = PathBuf::from("/home/x");
                path_matcher(spec, Path::new("/repo"), &home)
                    .map_err(|e| format!("Fix the path pattern in `{}`: {e}", r.raw))?;
            }
            Family::WebFetch => {
                let host = spec
                    .strip_prefix("domain:")
                    .map(str::trim)
                    .unwrap_or_default();
                if host.is_empty() {
                    return Err(format!(
                        "Write WebFetch rules as `WebFetch(domain:example.com)`, not `{}`.",
                        r.raw
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Bash,
    Edit,
    Read,
    WebFetch,
    Other,
}

pub fn family(tool: &str) -> Family {
    match tool {
        "Bash" => Family::Bash,
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "ApplyPatch" => Family::Edit,
        "Read" | "Grep" | "Glob" => Family::Read,
        "WebFetch" => Family::WebFetch,
        _ => Family::Other,
    }
}

/// What a rule is matched against.
pub enum Subject<'a> {
    Bash(&'a SimpleCommand),
    /// A file tool on a resolved absolute path.
    Path {
        tool: &'a str,
        path: &'a Path,
    },
    Url {
        url: &'a str,
    },
    Tool {
        tool: &'a str,
    },
}

impl Subject<'_> {
    fn tool(&self) -> &str {
        match self {
            Subject::Bash(_) => "Bash",
            Subject::Path { tool, .. } | Subject::Tool { tool } => tool,
            Subject::Url { .. } => "WebFetch",
        }
    }
}

fn anchor(spec: &str, repo: &Path, home: &Path) -> String {
    let repo = repo.to_string_lossy();
    let home = home.to_string_lossy();
    let mut pat = if let Some(rest) = spec.strip_prefix("//") {
        format!("/{rest}")
    } else if let Some(rest) = spec.strip_prefix("~/") {
        format!("{}/{rest}", home.trim_end_matches('/'))
    } else if spec == "~" {
        home.to_string()
    } else if let Some(rest) = spec.strip_prefix("./") {
        format!("{}/{rest}", repo.trim_end_matches('/'))
    } else if let Some(rest) = spec.strip_prefix('/') {
        format!("{}/{rest}", repo.trim_end_matches('/'))
    } else if spec.trim_end_matches('/').contains('/') {
        format!("{}/{spec}", repo.trim_end_matches('/'))
    } else {
        format!("{}/**/{spec}", repo.trim_end_matches('/'))
    };
    if pat.ends_with('/') {
        pat.push_str("**");
    }
    pat
}

fn path_matcher(spec: &str, repo: &Path, home: &Path) -> Result<GlobSet, String> {
    let pat = anchor(spec, repo, home);
    let mut set = GlobSetBuilder::new();
    let mut add = |p: &str| -> Result<(), String> {
        set.add(
            GlobBuilder::new(p)
                .literal_separator(true)
                .build()
                .map_err(|e| e.to_string())?,
        );
        Ok(())
    };
    add(&pat)?;
    // `dir/**` also covers `dir` itself, as in gitignore.
    if let Some(bare) = pat.strip_suffix("/**").filter(|b| !b.is_empty()) {
        add(bare)?;
    }
    set.build().map_err(|e| e.to_string())
}

fn bash_regex(pattern: &str) -> Option<Regex> {
    let body = pattern
        .split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(".*");
    Regex::new(&format!("^(?s){body}$")).ok()
}

fn bash_spec_matches(spec: &str, text: &str) -> bool {
    let text = text.trim();
    if let Some(prefix) = spec.strip_suffix(":*") {
        let prefix = prefix.trim();
        return text == prefix || text.starts_with(&format!("{prefix} "));
    }
    if bash_regex(spec).is_some_and(|r| r.is_match(text)) {
        return true;
    }
    spec.strip_suffix(" *")
        .is_some_and(|bare| text == bare.trim())
}

pub fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = if host.starts_with('[') {
        host.split(']').next().map(|h| format!("{h}]"))?
    } else {
        host.split(':').next()?.to_string()
    };
    let host = host.to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

impl Rule {
    /// Does the rule cover this subject? `bash_texts` are the forms of a simple command to try
    /// (full words, without leading assignments, without wrappers).
    pub fn matches(&self, subject: &Subject<'_>, repo: &Path, home: &Path) -> bool {
        let tool = subject.tool();
        let fam = family(&self.tool);
        let same_tool = match fam {
            Family::Other => self.tool == tool,
            f => {
                f == family(tool) && (f != Family::Read || self.tool == "Read" || self.tool == tool)
            }
        };
        if !same_tool {
            return false;
        }
        let Some(spec) = &self.spec else { return true };
        match subject {
            Subject::Bash(cmd) => bash_forms(cmd).iter().any(|t| bash_spec_matches(spec, t)),
            Subject::Path { path, .. } => match path_matcher(spec, repo, home) {
                Ok(m) => path.ancestors().any(|p| m.is_match(p)),
                Err(_) => false,
            },
            Subject::Url { url } => {
                let Some(domain) = spec
                    .strip_prefix("domain:")
                    .map(|d| d.trim().to_ascii_lowercase())
                else {
                    return false;
                };
                url_host(url).is_some_and(|h| h == domain || h.ends_with(&format!(".{domain}")))
            }
            Subject::Tool { .. } => false,
        }
    }
}

/// The texts a Bash rule is matched against.
pub fn bash_forms(cmd: &SimpleCommand) -> Vec<String> {
    let mut forms = vec![cmd.words_text()];
    let no_assign: Vec<String> = cmd
        .words
        .iter()
        .skip_while(|w| {
            let t = w.text();
            t.contains('=')
                && t.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        })
        .map(|w| w.text().to_string())
        .collect();
    forms.push(no_assign.join(" "));
    forms.push(cmd.effective_text());
    forms.dedup();
    forms
}

/// The rule an "always in this workspace" answer adds.
pub fn suggestion_for_bash(cmd: &SimpleCommand) -> Option<String> {
    let idx = cmd.effective_index()?;
    let words = &cmd.words[idx..];
    let first = words.first()?.value.clone()?;
    let mut prefix = first;
    if let Some(second) = words.get(1)
        && let Some(v) = &second.value
        && !v.starts_with('-')
        && !v.contains('/')
        && !second.glob
    {
        prefix.push(' ');
        prefix.push_str(v);
    }
    Some(format!("Bash({prefix} *)"))
}

pub fn suggestion_for_path(tool: &str, path: &Path, repo: &Path) -> String {
    let tool = if family(tool) == Family::Edit {
        "Edit"
    } else {
        tool
    };
    match path.strip_prefix(repo) {
        Ok(rel) => {
            let mut comps = rel.components();
            match (comps.next(), comps.next()) {
                (Some(first), Some(_)) => {
                    format!("{tool}(/{}/**)", first.as_os_str().to_string_lossy())
                }
                (Some(first), None) => format!("{tool}(/{})", first.as_os_str().to_string_lossy()),
                _ => format!("{tool}(/**)"),
            }
        }
        Err(_) => {
            let parent = path.parent().unwrap_or(path).to_string_lossy().to_string();
            format!("{tool}(/{}/**)", parent.trim_end_matches('/'))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash::parse;

    fn cmd(src: &str) -> SimpleCommand {
        parse(src).commands.into_iter().next().unwrap()
    }

    fn m(rule: &str, subject: &Subject<'_>) -> bool {
        parse_rule(rule)
            .unwrap()
            .matches(subject, Path::new("/repo"), Path::new("/home/me"))
    }

    #[test]
    fn bash_patterns() {
        assert!(m(
            "Bash(npm run test *)",
            &Subject::Bash(&cmd("npm run test --watch"))
        ));
        assert!(m(
            "Bash(npm run test *)",
            &Subject::Bash(&cmd("npm run test"))
        ));
        assert!(!m(
            "Bash(npm run test *)",
            &Subject::Bash(&cmd("npm run build"))
        ));
        assert!(m(
            "Bash(npm test:*)",
            &Subject::Bash(&cmd("npm test -- foo"))
        ));
        assert!(!m("Bash(npm test:*)", &Subject::Bash(&cmd("npm tests"))));
        assert!(m("Bash", &Subject::Bash(&cmd("anything at all"))));
        assert!(m(
            "Bash(rm -rf /*)",
            &Subject::Bash(&cmd("sudo rm -rf /var"))
        ));
        assert!(m(
            "Bash(./mvnw *)",
            &Subject::Bash(&cmd("FOO=1 ./mvnw -q compile"))
        ));
        assert!(m(
            "Bash(git push *)",
            &Subject::Bash(&cmd("git push origin main"))
        ));
    }

    #[test]
    fn path_patterns() {
        let p = |s: &'static str| Path::new(s);
        assert!(m(
            "Edit(src/**)",
            &Subject::Path {
                tool: "Write",
                path: p("/repo/src/a/b.ts")
            }
        ));
        assert!(!m(
            "Edit(src/**)",
            &Subject::Path {
                tool: "Write",
                path: p("/repo/lib/b.ts")
            }
        ));
        assert!(m(
            "Edit(/src)",
            &Subject::Path {
                tool: "Edit",
                path: p("/repo/src/x.ts")
            }
        ));
        assert!(m(
            "Read(~/.ssh/**)",
            &Subject::Path {
                tool: "Read",
                path: p("/home/me/.ssh/id_rsa")
            }
        ));
        assert!(m(
            "Read(~/.ssh/**)",
            &Subject::Path {
                tool: "Grep",
                path: p("/home/me/.ssh")
            }
        ));
        assert!(m(
            "Edit(*.env)",
            &Subject::Path {
                tool: "Edit",
                path: p("/repo/a/b/.env")
            }
        ));
        assert!(m(
            "Edit(//etc/**)",
            &Subject::Path {
                tool: "ApplyPatch",
                path: p("/etc/hosts")
            }
        ));
        assert!(!m(
            "Edit(src/*.ts)",
            &Subject::Path {
                tool: "Edit",
                path: p("/repo/src/a/b.ts")
            }
        ));
        assert!(!m(
            "Read(src/**)",
            &Subject::Path {
                tool: "Edit",
                path: p("/repo/src/b.ts")
            }
        ));
    }

    #[test]
    fn web_fetch_domains() {
        assert!(m(
            "WebFetch(domain:docs.rs)",
            &Subject::Url {
                url: "https://docs.rs/tokio"
            }
        ));
        assert!(m(
            "WebFetch(domain:rust-lang.org)",
            &Subject::Url {
                url: "https://doc.rust-lang.org/std"
            }
        ));
        assert!(!m(
            "WebFetch(domain:docs.rs)",
            &Subject::Url {
                url: "https://evil-docs.rs/x"
            }
        ));
        assert!(!m(
            "WebFetch(domain:docs.rs)",
            &Subject::Url {
                url: "https://docs.rs.evil.com/x"
            }
        ));
        assert_eq!(
            url_host("https://user@Example.com:8080/a?b"),
            Some("example.com".into())
        );
    }

    #[test]
    fn validation() {
        assert!(validate_rule("Bash(npm test *)").is_ok());
        assert!(validate_rule("Edit(src/**)").is_ok());
        assert!(validate_rule("WebFetch(domain:docs.rs)").is_ok());
        assert!(validate_rule("WebFetch(docs.rs)").is_err());
        assert!(validate_rule("Bash(npm test").is_err());
        assert!(validate_rule("Bash()").is_err());
        assert!(validate_rule("Frobnicate").is_err());
        assert!(validate_rule("mcp__server__tool").is_ok());
        assert!(validate_rule("Edit(src/[)").is_err());
    }

    #[test]
    fn suggestions() {
        assert_eq!(
            suggestion_for_bash(&cmd("npm run test --watch")).unwrap(),
            "Bash(npm run *)"
        );
        assert_eq!(
            suggestion_for_bash(&cmd("./mvnw -q compile")).unwrap(),
            "Bash(./mvnw *)"
        );
        assert_eq!(
            suggestion_for_path("Write", Path::new("/repo/src/a.ts"), Path::new("/repo")),
            "Edit(/src/**)"
        );
        assert_eq!(
            suggestion_for_path("Edit", Path::new("/repo/README.md"), Path::new("/repo")),
            "Edit(/README.md)"
        );
    }
}
