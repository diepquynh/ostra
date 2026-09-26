import type { AgentName, PermissionMode, SandboxMode, SandboxStatus } from "../api/types";

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

/** Workspace sandbox choices. `""` follows the global `[sandbox] mode`, which is required unless it says otherwise. */
export const SANDBOX_MODES: { mode: SandboxMode | ""; label: string; help: string }[] = [
  {
    mode: "",
    label: "Use the global setting",
    help: "Follows [sandbox] mode in ~/.config/ostra/config.toml, which is required unless set.",
  },
  {
    mode: "required",
    label: "Required",
    help: "Agent commands run in the sandbox. An execution that cannot be sandboxed does not start.",
  },
  {
    mode: "auto",
    label: "Auto",
    help: "Agent commands run in the sandbox when this machine has one, and without it, with a warning, otherwise.",
  },
  {
    mode: "off",
    label: "Off",
    help: "Agent commands run with the full rights of your user. Use it only to test a harness the sandbox blocks.",
  },
];

export const PERMISSION_MODES: { mode: PermissionMode; label: string; help: string }[] = [
  { mode: "default", label: "Default", help: "Asks before file edits and before any command no rule allows." },
  {
    mode: "acceptEdits",
    label: "Accept edits",
    help: "File edits inside the project are allowed; unlisted commands still ask.",
  },
  { mode: "plan", label: "Plan (read-only)", help: "Agents may read but not change the project." },
  { mode: "bypass", label: "Bypass", help: "Nothing asks. Guards and deny rules still apply." },
];

/** Agent commands start without a sandbox: the mode is off, or auto on a machine without one. Required never does. */
export const runsUnsandboxed = (s: SandboxStatus) => !s.active && s.mode !== "required";

/** What works only for Ostra's own file tools, not for shell commands, while agent commands run without a sandbox. */
export const UNSANDBOXED_EFFECTS = [
  "Hidden workspace artifacts can be read by a shell command that walks the disk, such as find ~.",
  "Ostra's data dir and your CLI sign-in files can be read by a shell command that does not name them.",
  "A program a command starts can write anywhere your user can. Ostra checks the writes it can read in the command itself.",
  "[sandbox] network = false does not apply.",
];
