use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ts_rs::TS;

/// Every leaf agent Ostra runs. Kebab-case names match `assets/agents/<name>/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum AgentName {
    Explore,
    GenerateSpec,
    FactCheck,
    Plan,
    Implementer,
    CodeReviewer,
    ExecutionPathAnalyzer,
    WriteTest,
    /// Rule B1: writes one project's part of the documentation book. Logs written before books
    /// say `module-documentation`.
    #[serde(alias = "module-documentation")]
    Documentation,
    /// Rule B4: writes the architecture of a book that covers two or more projects.
    SystemArchitecture,
    PromptGeneration,
    Initializer,
    QuickAnswer,
    /// Rule O5: diagnoses a failed or stuck step and tells the engine how to continue.
    Advisor,
}

impl AgentName {
    pub const ALL: [AgentName; 14] = [
        AgentName::Explore,
        AgentName::GenerateSpec,
        AgentName::FactCheck,
        AgentName::Plan,
        AgentName::Implementer,
        AgentName::CodeReviewer,
        AgentName::ExecutionPathAnalyzer,
        AgentName::WriteTest,
        AgentName::Documentation,
        AgentName::SystemArchitecture,
        AgentName::PromptGeneration,
        AgentName::Initializer,
        AgentName::QuickAnswer,
        AgentName::Advisor,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AgentName::Explore => "explore",
            AgentName::GenerateSpec => "generate-spec",
            AgentName::FactCheck => "fact-check",
            AgentName::Plan => "plan",
            AgentName::Implementer => "implementer",
            AgentName::CodeReviewer => "code-reviewer",
            AgentName::ExecutionPathAnalyzer => "execution-path-analyzer",
            AgentName::WriteTest => "write-test",
            AgentName::Documentation => "documentation",
            AgentName::SystemArchitecture => "system-architecture",
            AgentName::PromptGeneration => "prompt-generation",
            AgentName::Initializer => "initializer",
            AgentName::QuickAnswer => "quick-answer",
            AgentName::Advisor => "advisor",
        }
    }

    /// Sentence-case display name, for example `Code reviewer`.
    pub fn label(self) -> String {
        let spaced = self.as_str().replace('-', " ");
        let mut chars = spaced.chars();
        chars
            .next()
            .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
            .unwrap_or_default()
    }

    /// Snake-case form used in tool names such as `submit_code_reviewer`.
    pub fn snake(self) -> String {
        self.as_str().replace('-', "_")
    }

    pub fn submit_tool_name(self) -> String {
        format!("submit_{}", self.snake())
    }

    /// Agents confined to their session dir and OS temp (write-scope guard).
    pub fn is_session_only(self) -> bool {
        matches!(
            self,
            AgentName::Explore
                | AgentName::GenerateSpec
                | AgentName::FactCheck
                | AgentName::Plan
                | AgentName::CodeReviewer
                | AgentName::ExecutionPathAnalyzer
                | AgentName::Advisor
                | AgentName::Documentation
                | AgentName::SystemArchitecture
        )
    }

    /// Rule O2: the agent that creates a project, because only an approved plan's phase starts one.
    pub fn manages_projects(self) -> bool {
        matches!(self, AgentName::Implementer)
    }

    /// Agents whose model tier is routed by the phase's `**Complexity:**` line.
    pub fn routes_by_complexity(self) -> bool {
        matches!(self, AgentName::Implementer | AgentName::WriteTest)
    }
}

impl fmt::Display for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AgentName {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bare = s
            .trim()
            .trim_start_matches("ostra:")
            .trim_start_matches("ultracode:");
        // Execution rows stored before books name the retired agent.
        if bare == "module-documentation" {
            return Ok(AgentName::Documentation);
        }
        AgentName::ALL
            .into_iter()
            .find(|a| a.as_str() == bare)
            .ok_or_else(|| format!("unknown agent `{s}`"))
    }
}

/// The routing key `judge` sits beside the agent names in `routing.model.byAgent`.
pub const JUDGE_ROUTE: &str = "judge";

/// Agent names a workspace saved before the agent was replaced. Settings may still carry them,
/// and validation ignores them instead of refusing the file.
pub const RETIRED_AGENTS: [&str; 1] = ["module-documentation"];

/// Every key that must have a model route in workspace settings.
pub fn route_keys() -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = AgentName::ALL.iter().map(|a| a.as_str()).collect();
    keys.push(JUDGE_ROUTE);
    keys
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum InitializerMode {
    Detect,
    Scout,
    Propose,
    GenerateSkill,
    GenerateInventory,
    Adopt,
}

impl InitializerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            InitializerMode::Detect => "detect",
            InitializerMode::Scout => "scout",
            InitializerMode::Propose => "propose",
            InitializerMode::GenerateSkill => "generate-skill",
            InitializerMode::GenerateInventory => "generate-inventory",
            InitializerMode::Adopt => "adopt",
        }
    }
}

impl fmt::Display for InitializerMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Tool capabilities an agent definition may declare. Native tool names follow Claude Code's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Capability {
    Read,
    Write,
    Edit,
    Shell,
    SearchText,
    Glob,
    Skill,
    WebSearch,
    WebFetch,
    Report,
    /// The typed-document tool: research, spec, and plan documents (HANDOVER 10.3).
    Document,
    Memory,
    MemoryRecall,
    /// Search over the workspace's documentation books (Rule B8).
    DocsSearch,
    /// The code navigation tools over the project's index and dependency graph ([`CODE_TOOLS`]).
    Code,
    /// The project management tools ([`crate::manage::PROJECT_TOOLS`]).
    ManageProjects,
    /// The subagent coordination tools ([`crate::coord::COORD_TOOLS`]).
    Coordinate,
}

/// The code navigation tools: the operation (`code_{op}` over MCP) and the native tool name.
pub const CODE_TOOLS: [(&str, &str); 8] = [
    ("outline", "CodeOutline"),
    ("find", "CodeFind"),
    ("callers", "CodeCallers"),
    ("callees", "CodeCallees"),
    ("implementations", "CodeImplementations"),
    ("neighbors", "CodeNeighbors"),
    ("impact", "CodeImpact"),
    ("map", "CodeMap"),
];

pub fn is_code_tool(native: &str) -> bool {
    CODE_TOOLS.iter().any(|(_, n)| *n == native)
}

impl Capability {
    /// The native tool name that serves this capability.
    pub fn native_tool(self) -> &'static str {
        match self {
            Capability::Read => "Read",
            Capability::Write => "Write",
            Capability::Edit => "Edit",
            Capability::Shell => "Bash",
            Capability::SearchText => "Grep",
            Capability::Glob => "Glob",
            Capability::Skill => "Skill",
            Capability::WebSearch => "WebSearch",
            Capability::WebFetch => "WebFetch",
            Capability::Report => "Report",
            Capability::Document => "Document",
            Capability::Memory => "Memory",
            Capability::MemoryRecall => "MemoryRecall",
            Capability::DocsSearch => "DocsSearch",
            Capability::Code => CODE_TOOLS[0].1,
            Capability::ManageProjects => crate::manage::PROJECT_TOOLS[0].1,
            Capability::Coordinate => crate::coord::COORD_TOOLS[0].1,
        }
    }

    /// Capabilities that write files. Used to derive read-only runs and sandbox modes.
    pub fn writes(self) -> bool {
        matches!(self, Capability::Write | Capability::Edit)
    }
}
