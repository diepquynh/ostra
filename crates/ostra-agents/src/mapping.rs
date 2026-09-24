//! `assets/tool-mapping.toml`: capability to tool name per executor, plus harness strategies.

use ostra_core::{AgentName, Capability, ExecutorKind, HarnessKind};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

/// Template token names and the mapping key each resolves through.
const TOKENS: [(&str, &str); 14] = [
    ("tool_read", "read"),
    ("tool_write", "write"),
    ("tool_edit", "edit"),
    ("tool_shell", "shell"),
    ("tool_search_text", "search_text"),
    ("tool_glob", "glob"),
    ("tool_skill", "skill"),
    ("tool_web_search", "web_search"),
    ("tool_web_fetch", "web_fetch"),
    ("tool_report", "report"),
    ("tool_document", "document"),
    ("tool_memory", "memory"),
    ("tool_memory_recall", "memory_recall"),
    ("tool_submit", "submit"),
];

#[derive(Debug, Deserialize)]
pub(crate) struct Mapping {
    capabilities: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    strategies: BTreeMap<String, BTreeMap<String, String>>,
}

pub(crate) fn get() -> &'static Mapping {
    static MAP: OnceLock<Mapping> = OnceLock::new();
    MAP.get_or_init(|| {
        let text = crate::asset_text("tool-mapping.toml").unwrap_or_else(|e| panic!("{e}"));
        toml::from_str(&text).unwrap_or_else(|e| panic!("assets/tool-mapping.toml is invalid: {e}"))
    })
}

fn capability_key(c: Capability) -> &'static str {
    match c {
        Capability::Read => "read",
        Capability::Write => "write",
        Capability::Edit => "edit",
        Capability::Shell => "shell",
        Capability::SearchText => "search_text",
        Capability::Glob => "glob",
        Capability::Skill => "skill",
        Capability::WebSearch => "web_search",
        Capability::WebFetch => "web_fetch",
        Capability::Report => "report",
        Capability::Document => "document",
        Capability::Memory => "memory",
        Capability::MemoryRecall => "memory_recall",
    }
}

impl Mapping {
    pub(crate) fn tool(&self, capability: &str, executor: ExecutorKind, agent: AgentName) -> Option<String> {
        let value = self.capabilities.get(capability)?.get(executor.tier_table())?;
        Some(value.replace("{agent}", &agent.snake()))
    }

    /// Template context: every `tool_*` token plus `assets_dir`, rendered bare as Ultracode's
    /// generator did, because prompts already backtick the tokens that need it.
    pub(crate) fn context(
        &self,
        agent: AgentName,
        executor: ExecutorKind,
        assets_dir: &Path,
    ) -> BTreeMap<&'static str, String> {
        let mut ctx = BTreeMap::new();
        for (token, key) in TOKENS {
            let value = self.tool(key, executor, agent).unwrap_or_else(|| key.to_string());
            ctx.insert(token, value);
        }
        ctx.insert("assets_dir", assets_dir.display().to_string());
        ctx
    }

    /// The section that opens a harness executor's prompt: which tool serves each capability the
    /// agent has, and how to load skills and call Ostra's tools on that harness.
    pub(crate) fn vocabulary(&self, agent: AgentName, harness: HarnessKind, caps: &[Capability]) -> String {
        let executor = ExecutorKind::Harness(harness);
        let table = harness.as_str();
        let mut out = String::new();
        out.push_str("## Tool vocabulary\n\n");
        out.push_str(&format!(
            "You are running in {} under Ostra. This prompt names each tool by what it does. Use these tools, \
             because they are the ones this run provides:\n\n",
            harness.display_name()
        ));
        out.push_str("| Capability | Tool |\n| --- | --- |\n");
        let mut keys: Vec<&str> = caps.iter().map(|c| capability_key(*c)).collect();
        keys.push("submit");
        for key in &keys {
            if let Some(tool) = self.tool(key, executor, agent) {
                out.push_str(&format!("| {} | {} |\n", key.replace('_', " "), tool));
            }
        }
        out.push('\n');
        let wants = |strategy: &str| match strategy {
            "skill" => caps.contains(&Capability::Skill) || caps.contains(&Capability::Read),
            "write" => caps.contains(&Capability::Write) || caps.contains(&Capability::Edit),
            _ => true,
        };
        for (strategy, per) in &self.strategies {
            if !wants(strategy) {
                continue;
            }
            if let Some(text) = per.get(table) {
                out.push_str(text.trim());
                out.push_str("\n\n");
            }
        }
        out.push_str(
            "Do not start subagents or delegate to another agent: every Ostra agent is a leaf, and Ostra \
             schedules all other work itself.\n",
        );
        out
    }
}
