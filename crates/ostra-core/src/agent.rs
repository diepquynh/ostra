use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ts_rs::TS;

/// Every leaf agent Ostra runs: the built-in agents, whose kebab-case names match
/// `assets/agents/<name>/`, and the custom agents a workspace or a plugin defines. Serialized as the
/// bare name either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, TS)]
#[ts(export, type = "string")]
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
    Documentation,
    /// Rule B4: writes the architecture of a book that covers two or more projects.
    SystemArchitecture,
    PromptGeneration,
    Initializer,
    QuickAnswer,
    /// Rule O5: diagnoses a failed or stuck step and tells the engine how to continue.
    Advisor,
    /// Rule CA1: an agent a workspace markdown file or a plugin defines.
    Custom(CustomAgent),
}

/// The name of a custom agent, interned so [`AgentName`] stays `Copy`. Only names that pass
/// [`CustomAgent::new`] exist, so every value is a valid, non-built-in agent name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CustomAgent(&'static str);

/// Longest custom agent name, so it fits tool names such as `submit_<name>` on every harness.
pub const MAX_CUSTOM_AGENT_NAME: usize = 40;

impl CustomAgent {
    /// Rule CA1: a custom agent name is lowercase kebab-case, starts with a letter, and is not a
    /// built-in agent, a retired one, or the `judge` route.
    pub fn new(name: &str) -> Result<CustomAgent, String> {
        let name = name.trim();
        let valid = !name.is_empty()
            && name.len() <= MAX_CUSTOM_AGENT_NAME
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && !name.ends_with('-')
            && !name.contains("--")
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid {
            return Err(format!(
                "Name the agent in lowercase kebab-case of at most {MAX_CUSTOM_AGENT_NAME} characters, starting with a letter: `{name}` is not."
            ));
        }
        if AgentName::builtin(name).is_some()
            || RETIRED_AGENTS.contains(&name)
            || name == JUDGE_ROUTE
        {
            return Err(format!(
                "Give the agent another name: `{name}` is a built-in Ostra agent or route."
            ));
        }
        Ok(CustomAgent(intern(name)))
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

/// Custom agent names live as long as the process. They come from workspace files and plugin
/// manifests, so the set stays small.
pub(crate) fn intern(name: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(n) = names.get(name) {
        return n;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(leaked);
    leaked
}

impl Serialize for AgentName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AgentName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
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
            AgentName::Custom(c) => c.as_str(),
        }
    }

    /// The built-in agent of this name, without the retired aliases.
    pub fn builtin(name: &str) -> Option<AgentName> {
        AgentName::ALL.into_iter().find(|a| a.as_str() == name)
    }

    pub fn is_builtin(self) -> bool {
        !matches!(self, AgentName::Custom(_))
    }

    pub fn custom(self) -> Option<CustomAgent> {
        match self {
            AgentName::Custom(c) => Some(c),
            _ => None,
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
        if let Some(a) = AgentName::builtin(bare) {
            return Ok(a);
        }
        CustomAgent::new(bare)
            .map(AgentName::Custom)
            .map_err(|_| format!("unknown agent `{s}`"))
    }
}

/// Rule CA2: where an agent may write files, enforced by the write-scope guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WriteScope {
    /// No file anywhere.
    ReadOnly,
    /// Its session dir and the OS temp dir.
    #[default]
    Session,
    /// The repo root and its session dir.
    Project,
    /// The project's `.ostra/` runtime and its skills dir, and its session dir (project setup).
    Setup,
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
    /// Rule CA6: the typed-document tool for research documents, and their ownership.
    DocumentResearch,
    /// Rule CA6: the typed-document tool for the spec, and its ownership.
    DocumentSpec,
    /// Rule CA6: the typed-document tool for the plan and its phase files, and their ownership.
    DocumentPlan,
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
    /// Rule CA6: writes a review loop's ledger, which the engine counts to cap the loop.
    ReviewLedger,
    /// Rule CA6: writes the security block file of BLOCKER findings.
    SecurityBlock,
    /// Rule CA6: writes the implementer progress log that re-runs read.
    ProgressLog,
    /// Rule CA6: writes test files and directories in the repo.
    TestFiles,
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
            Capability::DocumentResearch | Capability::DocumentSpec | Capability::DocumentPlan => {
                "Document"
            }
            Capability::Memory => "Memory",
            Capability::MemoryRecall => "MemoryRecall",
            Capability::DocsSearch => "DocsSearch",
            Capability::Code => CODE_TOOLS[0].1,
            Capability::ManageProjects => crate::manage::PROJECT_TOOLS[0].1,
            Capability::Coordinate => crate::coord::COORD_TOOLS[0].1,
            // Grants of file ownership, which add no tool.
            Capability::ReviewLedger
            | Capability::SecurityBlock
            | Capability::ProgressLog
            | Capability::TestFiles => "",
        }
    }

    /// Rule CA6: the typed document this capability grants.
    pub fn document_kind(self) -> Option<crate::doc::DocKind> {
        match self {
            Capability::DocumentResearch => Some(crate::doc::DocKind::Research),
            Capability::DocumentSpec => Some(crate::doc::DocKind::Spec),
            Capability::DocumentPlan => Some(crate::doc::DocKind::Plan),
            _ => None,
        }
    }

    /// Capabilities that write files. Used to derive read-only runs and sandbox modes.
    pub fn writes(self) -> bool {
        matches!(self, Capability::Write | Capability::Edit)
    }
}
