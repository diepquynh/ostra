//! Agent definitions as files: a markdown file with TOML frontmatter between `+++` lines, or an
//! `agent.toml` beside a prompt. Ostra reads its own agents, a workspace's `.ostra/agents/`, and
//! any plugin that ships its agents as files through these functions, so every agent is the same
//! [`PluginAgent`] whatever wrote it.

use crate::{Capability, Effort, PluginAgent, Tier, WriteScope};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The fields of an agent definition file: the frontmatter of a markdown agent, or `agent.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFile {
    /// Defaults to the file's own name.
    #[serde(default)]
    pub name: Option<String>,
    pub description: String,
    #[serde(default)]
    pub default_tier: Option<Tier>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub capabilities: Option<Vec<Capability>>,
    #[serde(default)]
    pub write_scope: Option<WriteScope>,
    #[serde(default)]
    pub effort: BTreeMap<String, Effort>,
    /// JSON Schema (subset) of the submit's `data`, as a TOML table.
    #[serde(default)]
    pub data_schema: Option<toml::Value>,
    #[serde(default)]
    pub helper: bool,
    /// Rule CA5: the result contract it submits. Defaults to `stage`.
    #[serde(default)]
    pub returns: Option<String>,
    /// Rule CA6: the repo brief sections it gets.
    #[serde(default)]
    pub brief: Option<Vec<String>>,
}

impl AgentFile {
    /// The agent this file defines, named `default_name` unless the file names itself.
    pub fn into_agent(self, default_name: &str, prompt: &str) -> Result<PluginAgent, String> {
        let data_schema = match self.data_schema {
            Some(v) => {
                Some(serde_json::to_value(v).map_err(|e| format!("Fix `data_schema`: {e}."))?)
            }
            None => None,
        };
        Ok(PluginAgent {
            name: self.name.unwrap_or_else(|| default_name.to_string()),
            description: self.description.trim().to_string(),
            prompt: Some(prompt.to_string()),
            default_tier: self.default_tier,
            capabilities: self.capabilities,
            write_scope: self.write_scope,
            effort: self.effort,
            timeout_seconds: self.timeout_seconds,
            data_schema,
            helper: self.helper,
            returns: self.returns,
            brief: self.brief,
        })
    }
}

/// Split `+++` TOML frontmatter from the markdown body.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .strip_prefix("+++")?;
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    let end = rest.find("\n+++")?;
    let front = &rest[..end];
    let body = &rest[end + 4..];
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
        .unwrap_or(body);
    Some((front, body))
}

/// Rule CA1: a markdown agent: frontmatter between `+++` lines, then its instructions.
pub fn parse_markdown(default_name: &str, text: &str) -> Result<PluginAgent, String> {
    let (front, body) = split_frontmatter(text).ok_or(
        "Start the file with TOML frontmatter between `+++` lines, holding at least `description`.",
    )?;
    let file: AgentFile =
        toml::from_str(front).map_err(|e| format!("Fix the frontmatter: {}", e.message()))?;
    file.into_agent(default_name, body)
}

/// An `agent.toml` and the prompt beside it, the layout of Ostra's own agents.
pub fn parse_toml(name: &str, agent_toml: &str, prompt: &str) -> Result<PluginAgent, String> {
    let file: AgentFile =
        toml::from_str(agent_toml).map_err(|e| format!("Fix agent.toml: {}", e.message()))?;
    file.into_agent(name, prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_and_toml_give_the_same_agent() {
        let front =
            "description = \"Audits\"\ncapabilities = [\"read\"]\n[effort]\nnative = \"high\"\n";
        let md = parse_markdown("auditor", &format!("+++\n{front}+++\nRead it.\n")).unwrap();
        let toml = parse_toml("auditor", front, "Read it.\n").unwrap();
        assert_eq!(md, toml);
        assert_eq!(md.capabilities, Some(vec![Capability::Read]));
        assert_eq!(md.effort["native"], Effort::High);
        assert!(parse_markdown("x", "no frontmatter").is_err());
        assert!(parse_toml("x", "description = \"d\"\nbogus = 1", "").is_err());
    }
}
