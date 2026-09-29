import type {
  AgentName,
  PermissionMode,
  SandboxMode,
  SandboxNetwork,
  SandboxStatus,
  ToolEnforcement,
} from "../api/types";

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
  { key: "advisor", role: "Diagnoses a failed or stuck step and says how to continue, read-only." },
  { key: "judge", role: "Makes the engine's judgment calls, such as classifying a request." },
];

/** Agents whose model tier depends on the phase's complexity. */
export const COMPLEXITY_AGENTS = ["implementer", "write-test"] as const;

/** Route keys that always run on the native executor. */
export const NATIVE_ONLY = new Set(["judge", "quick-answer"]);

/** Workspace tool enforcement choices. `""` follows the global `tool_enforcement`, which defaults to disabled. */
export const TOOL_ENFORCEMENT: { value: ToolEnforcement | ""; label: string; help: string }[] = [
  { value: "", label: "Use the global setting", help: "Follows tool_enforcement in the global config." },
  {
    value: "disabled",
    label: "Disabled",
    help: "Agents may reach files by any route, such as one script that edits several files, which saves tool calls. Write scope, the report path, and self-protection are off. Ownership, secret, git, and test guards still refuse, and the sandbox still bounds every command. Choose it for capable models.",
  },
  {
    value: "enabled",
    label: "Enabled",
    help: "Every guard runs, which protects the pipeline from a weaker model that writes outside its scope or misnames its report. It costs more tool calls: each refused call is spent, and a model that keeps reaching for a script retries until it uses one edit per call.",
  },
];

/** Workspace sandbox choices. `""` follows the global `[sandbox] mode`; its default is required on Linux and macOS and auto on Windows, which has no sandbox backend yet. */
export const SANDBOX_MODES: { mode: SandboxMode | ""; label: string; help: string }[] = [
  {
    mode: "",
    label: "Use the global setting",
    help: "Follows [sandbox] mode in the global config, which defaults to required on Linux and macOS and to auto on Windows.",
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

/** The sandboxing guide's decoy section on the repository, because the console ships no docs. */
export const DECOY_GUIDE =
  "https://github.com/diepquynh/ostra/blob/master/docs/security/sandboxing.md#decoy-credential-files";

/** Network choices for sandboxed commands, as `[sandbox] network` and `sandbox_network` take them. */
export const SANDBOX_NETWORKS: { network: SandboxNetwork; label: string; help: string }[] = [
  { network: "none", label: "None", help: "Commands reach nothing. A harness CLI still reaches its own model API." },
  {
    network: "allowlist",
    label: "Allowlist",
    help: "Package registries, source hosts, the harness CLIs' model APIs, and the hosts listed below.",
  },
  {
    network: "public",
    label: "Public",
    help: "Any public address, and the hosts listed below. Loopback, private, and link-local addresses stay refused.",
  },
  {
    network: "host",
    label: "Host",
    help: "The host's network as it is, unfiltered: every local service, the LAN, and the cloud metadata address.",
  },
];

/** The allowed hosts apply only under these choices. */
export const takesHosts = (n: SandboxNetwork) => n === "allowlist" || n === "public";

/** What works only for Ostra's own file tools, not for shell commands, while agent commands run without a sandbox. */
export const UNSANDBOXED_EFFECTS = [
  "Hidden workspace artifacts can be read by a shell command that walks the disk, such as find ~.",
  "Ostra's data dir and your CLI sign-in files can be read by a shell command that does not name them.",
  "A program a command starts can write anywhere your user can. Ostra checks the writes it can read in the command itself.",
  "The [sandbox] network setting does not apply: commands reach any host, your local services and LAN included.",
];
