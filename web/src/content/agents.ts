import type { AgentName, PermissionMode } from "../api/types";

/** Every route key in `routing.model.byAgent`, with a one-line role. */
export const ROUTE_KEYS: { key: AgentName | "judge"; role: string }[] = [
  { key: "explore", role: "Researches the code and external documentation. The only agent with web search." },
  { key: "generate-spec", role: "Writes the one spec file from every research document." },
  { key: "fact-check", role: "Checks every concrete claim in a spec or plan before approval." },
  { key: "plan", role: "Turns the approved spec into phases." },
  { key: "implementer", role: "Writes the code for one phase." },
  { key: "code-reviewer", role: "Reviews each change against the project's rules and the phase." },
  { key: "execution-path-analyzer", role: "Lists every path through the changed code before tests are written." },
  { key: "write-test", role: "Writes one test per execution path." },
  { key: "module-documentation", role: "Refreshes the area references after all phases pass." },
  { key: "prompt-generation", role: "Writes prompts, skills, and agent definitions." },
  { key: "initializer", role: "Scouts a project and generates its skills and inventory." },
  { key: "quick-answer", role: "Answers side-panel questions, read-only." },
  { key: "judge", role: "Makes the engine's judgment calls, such as classifying a request." },
];

/** Agents whose model tier depends on the phase's complexity. */
export const COMPLEXITY_AGENTS = ["implementer", "write-test"] as const;

/** Route keys that always run on the native executor. */
export const NATIVE_ONLY = new Set(["judge", "quick-answer"]);

export const TIERS = ["fast", "balanced", "advanced", "frontier", "default"];

export const PERMISSION_MODES: { mode: PermissionMode; label: string; help: string }[] = [
  { mode: "default", label: "Default", help: "Asks before file edits and before any command no rule allows." },
  { mode: "acceptEdits", label: "Accept edits", help: "File edits inside the project are allowed; unlisted commands still ask." },
  { mode: "plan", label: "Plan (read-only)", help: "Agents may read but not change the project." },
  { mode: "bypass", label: "Bypass", help: "Nothing asks. Guards and deny rules still apply." },
];
