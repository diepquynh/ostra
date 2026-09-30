import type { AgentInfo, AgentName, Capability, Effort, ExecutorKind, Tier } from "../types";

const NATIVE: Record<Tier, string> = {
  fast: "anthropic:claude-haiku-4-5-20251001",
  balanced: "anthropic:claude-sonnet-5-5",
  advanced: "anthropic:claude-opus-5-5",
  frontier: "anthropic:claude-fable-5-1",
};
const CODEX: Record<Tier, string> = {
  fast: "gpt-5.6-luna",
  balanced: "gpt-5.6-terra",
  advanced: "gpt-5.6-sol",
  frontier: "gpt-5.6-sol",
};

const READ: Capability[] = ["read", "search_text", "glob"];
const WRITE: Capability[] = [
  "read",
  "edit",
  "write",
  "shell",
  "search_text",
  "glob",
  "skill",
  "memory_recall",
  "memory",
  "report",
];

type Row = [AgentName, string, Tier, Capability[], Effort, { executor: ExecutorKind; tier: Tier }];

// Mirrors `assets/agents/*/agent.toml` and the demo workspace's routes in `fixtures.ts`.
const ROWS: Row[] = [
  [
    "explore",
    "Researches the code and external documentation.",
    "advanced",
    [...READ, "write", "shell", "web_search", "web_fetch"],
    "high",
    { executor: "native", tier: "advanced" },
  ],
  [
    "generate-spec",
    "Writes the one spec file from every research document.",
    "advanced",
    [...READ, "write", "edit", "shell"],
    "high",
    { executor: "native", tier: "advanced" },
  ],
  [
    "fact-check",
    "Checks every concrete claim in a spec or plan.",
    "advanced",
    [...READ, "write", "shell", "web_fetch"],
    "high",
    { executor: "native", tier: "advanced" },
  ],
  [
    "plan",
    "Turns one approved spec into phases.",
    "advanced",
    [...READ, "write", "shell"],
    "high",
    { executor: "native", tier: "advanced" },
  ],
  [
    "implementer",
    "Writes the code for one phase.",
    "balanced",
    WRITE,
    "high",
    { executor: "harness:codex", tier: "fast" },
  ],
  [
    "code-reviewer",
    "Reviews each change against the project's rules.",
    "balanced",
    [...READ, "shell"],
    "high",
    { executor: "native", tier: "balanced" },
  ],
  [
    "execution-path-analyzer",
    "Lists every path through the changed code.",
    "balanced",
    [...READ, "shell", "write", "report"],
    "high",
    { executor: "native", tier: "balanced" },
  ],
  [
    "write-test",
    "Writes one test per execution path.",
    "balanced",
    WRITE,
    "high",
    { executor: "harness:codex", tier: "fast" },
  ],
  [
    "documentation",
    "Writes one project's part of the documentation book.",
    "advanced",
    [...READ, "shell"],
    "high",
    { executor: "native", tier: "advanced" },
  ],
  [
    "system-architecture",
    "Writes the architecture of a book of two or more projects.",
    "advanced",
    [...READ, "shell"],
    "high",
    { executor: "native", tier: "advanced" },
  ],
  [
    "prompt-generation",
    "Writes prompts, skills, and agent definitions.",
    "advanced",
    [...READ, "edit", "write", "shell", "skill", "report"],
    "high",
    { executor: "native", tier: "advanced" },
  ],
  [
    "initializer",
    "Scouts a project and generates its skills and inventory.",
    "balanced",
    [...READ, "write", "edit", "shell"],
    "high",
    { executor: "native", tier: "balanced" },
  ],
  [
    "quick-answer",
    "Answers side-panel questions, read-only.",
    "balanced",
    [...READ, "web_search", "web_fetch", "memory_recall"],
    "medium",
    { executor: "native", tier: "balanced" },
  ],
];

const route = (executor: ExecutorKind, tier: Tier) => ({
  executor,
  tier,
  model: (executor === "native" ? NATIVE : CODEX)[tier],
});

const label = (n: string) => n.charAt(0).toUpperCase() + n.slice(1).replace(/-/g, " ");

export const agents: AgentInfo[] = ROWS.map(([name, description, tier, capabilities, effort, current]) => ({
  name,
  label: label(name),
  description,
  default_tier: tier,
  effort: {
    native: effort,
    claude: effort,
    codex: name === "initializer" ? "xhigh" : effort,
    grok: effort,
    agy: effort,
  },
  default_effort: effort,
  capabilities,
  timeout_secs: name === "quick-answer" ? 300 : 1800,
  resolved: route(current.executor, current.tier),
  default_route: route(current.executor, tier),
}));

export const stacks = ["go", "java-spring", "python", "typescript-node"];
