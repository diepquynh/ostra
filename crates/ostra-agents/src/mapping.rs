//! `assets/tool-mapping.toml`: capability to tool name per executor, plus harness strategies.

use ostra_core::{AgentName, Capability, ExecutorKind, HarnessKind};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

/// Template token names and the mapping key each resolves through.
const TOKENS: [(&str, &str); 27] = [
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
    ("tool_code_outline", "code:outline"),
    ("tool_code_find", "code:find"),
    ("tool_code_callers", "code:callers"),
    ("tool_code_callees", "code:callees"),
    ("tool_code_implementations", "code:implementations"),
    ("tool_code_neighbors", "code:neighbors"),
    ("tool_code_impact", "code:impact"),
    ("tool_code_map", "code:map"),
    ("tool_project_list", "manage_projects:list"),
    ("tool_project_create", "manage_projects:create"),
    ("tool_subagent_list", "coordinate:list"),
    ("tool_subagent_ask", "coordinate:ask"),
    ("tool_subagent_reply", "coordinate:reply"),
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
        Capability::Code => "code",
        Capability::ManageProjects => "manage_projects",
        Capability::Coordinate => "coordinate",
    }
}

impl Mapping {
    /// The tool name for a mapping key. `code:callers` fills the `code` entry's `{op}` (and
    /// `{Op}`) with one of the code tools.
    pub(crate) fn tool(
        &self,
        capability: &str,
        executor: ExecutorKind,
        agent: AgentName,
    ) -> Option<String> {
        let (key, op) = capability
            .split_once(':')
            .map_or((capability, None), |(k, o)| (k, Some(o)));
        let value = self.capabilities.get(key)?.get(executor.tier_table())?;
        let mut v = value.replace("{agent}", &agent.snake());
        if let Some(op) = op {
            let mut cap = op.to_string();
            cap[..1].make_ascii_uppercase();
            v = v.replace("{op}", op).replace("{Op}", &cap);
        }
        Some(v)
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
            let value = self
                .tool(key, executor, agent)
                .unwrap_or_else(|| key.to_string());
            ctx.insert(token, value);
        }
        ctx.insert("assets_dir", assets_dir.display().to_string());
        ctx
    }

    /// The section that opens a harness executor's prompt: which tool serves each capability the
    /// agent has, and how to load skills and call Ostra's tools on that harness.
    pub(crate) fn vocabulary(
        &self,
        agent: AgentName,
        harness: HarnessKind,
        caps: &[Capability],
    ) -> String {
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
            let ops: Vec<&str> = match *key {
                "code" => ostra_core::agent::CODE_TOOLS
                    .iter()
                    .map(|(op, _)| *op)
                    .collect(),
                "manage_projects" => ostra_core::manage::PROJECT_TOOLS
                    .iter()
                    .map(|(name, _)| name.trim_start_matches("project_"))
                    .collect(),
                "coordinate" => ostra_core::coord::COORD_TOOLS
                    .iter()
                    .map(|(name, _)| name.trim_start_matches("subagent_"))
                    .collect(),
                _ => vec![],
            };
            let tool = if !ops.is_empty() {
                let all: Vec<String> = ops
                    .iter()
                    .filter_map(|op| self.tool(&format!("{key}:{op}"), executor, agent))
                    .collect();
                (!all.is_empty()).then(|| all.join(", "))
            } else {
                self.tool(key, executor, agent)
            };
            if let Some(tool) = tool {
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
            "Do not start this harness's own subagents or delegate through its own tools: Ostra schedules the \
             work, and the only way to reach another agent is Ostra's subagent tools when your tool table lists \
             them.\n",
        );
        if let Some(submit) = self.tool("submit", executor, agent) {
            out.push_str(&format!(
                "\nWhen the task is finished and you have called `{submit}`, reply with only `Done!` and nothing \
                 else, because Ostra reads your result from the submit call and any other text costs output tokens.\n"
            ));
        }
        out
    }
}
