import type {
  AgentDetail,
  AgentInfo,
  BuilderPalette,
  BuiltinStage,
  FunctionDoc,
  FunctionFile,
  PluginInfo,
  StageFile,
  TransformInfo,
  WorkflowDoc,
  WorkflowFile,
} from "../types";

const STAGES: [BuiltinStage, string, string[], boolean][] = [
  ["research", "Research tasks over the code and the Sufficiency judge.", ["research"], false],
  ["track", "The Track judge picks the light or the full track.", [], true],
  ["spec", "The spec, its fact-check, and your approval.", ["spec", "fact-check"], false],
  ["stakes", "The Stakes judge, which can skip the plan.", [], false],
  ["plan", "The plan, its fact-check, and your approval.", ["plan", "fact-check"], false],
  ["build", "Every phase's implement and review loop.", ["implementation", "review", "prompt", "advice"], false],
  ["feedback", "Your review of the implementation.", [], true],
  ["closing", "Format, the closing gate, tests, docs, and the book.", ["path-analysis", "tests", "review"], true],
];

const p = (name: string, kind: TransformInfo["output"], required: boolean, description: string) => ({
  name,
  kind,
  required,
  description,
});

const TRANSFORMS: TransformInfo[] = [
  {
    name: "filter",
    description: "Keeps the items of a list whose field passes a comparison.",
    inputs: [p("items", "array", true, "The list to work on.")],
    variadic: false,
    args: [
      p("field", "string", false, "A dotted path into each item."),
      p("op", "string", true, "eq, ne, gt, ..."),
      p("to", "any", false, "The value to compare against."),
    ],
    output: "array",
    custom: false,
  },
  {
    name: "count",
    description: "Counts the items of a list.",
    inputs: [p("items", "any", true, "The value to count.")],
    variadic: false,
    args: [],
    output: "number",
    custom: false,
  },
  {
    name: "pick",
    description: "Takes one field out of a value.",
    inputs: [p("value", "any", true, "The value to read.")],
    variadic: false,
    args: [p("path", "string", true, "A dotted path.")],
    output: "any",
    custom: false,
  },
  {
    name: "map",
    description: "Takes one field out of every item of a list.",
    inputs: [p("items", "array", true, "The list to work on.")],
    variadic: false,
    args: [p("field", "string", true, "A dotted path into each item.")],
    output: "array",
    custom: false,
  },
  {
    name: "concat",
    description: "Joins its inputs into one list.",
    inputs: [],
    variadic: true,
    args: [],
    output: "array",
    custom: false,
  },
];

export const palette: BuilderPalette = {
  builtin_stages: STAGES.map(([stage, description, contracts, removable]) => ({
    stage,
    uses: `ostra:${stage}`,
    description,
    contracts: contracts as BuilderPalette["builtin_stages"][number]["contracts"],
    removable,
  })),
  plugin_stages: [{ plugin: "gate", stage: "release", description: "Runs the release checks." }],
  transforms: [
    ...TRANSFORMS,
    {
      name: "gate:changelog-lines",
      description: "The lines of the changelog, read by the gate plugin.",
      inputs: [],
      variadic: false,
      args: [],
      output: "array",
      custom: false,
      plugin: "gate",
    },
    {
      name: "high-files",
      description: "The files of findings at a risk.",
      inputs: [p("findings", "array", true, "Findings with a file and a risk.")],
      variadic: false,
      args: [p("risk", "string", true, "The risk to keep.")],
      output: "array",
      custom: true,
    },
  ],
  cond_ops: ["eq", "ne", "gt", "ge", "lt", "le", "contains", "in", "exists", "empty", "not_empty", "truthy", "falsy"],
  tiers: ["fast", "balanced", "advanced", "frontier"],
};

const chain = (ids: BuiltinStage[]): StageFile[] =>
  ids.map((id, i) => ({ id, uses: `ostra:${id}`, after: i ? [ids[i - 1]] : [] }));

const secure: WorkflowFile = {
  description: "The implement pipeline with a security audit and a branch on its findings.",
  base: "implement",
  layout: {},
  stage: [
    ...chain(["research", "track", "spec", "stakes", "plan", "build"]),
    { id: "audit", agent: "security-auditor", after: ["build"] },
    {
      id: "high",
      transform: "filter",
      after: ["audit"],
      inputs: { items: "audit.findings" },
      args: { field: "file", op: "contains", to: "auth" },
    },
    {
      id: "triage",
      prompt: "Group the findings by the fix they need.",
      tier: "fast",
      after: ["high"],
      inputs: { findings: "high.output" },
      when: [{ ref: "high.output", op: "not_empty" }],
      output_schema: { type: "object", required: ["groups"], properties: { groups: { type: "array" } } },
    },
    { id: "feedback", uses: "ostra:feedback", after: ["triage"] },
    { id: "closing", uses: "ostra:closing", after: ["feedback"] },
  ],
};

export const workflowFiles: Record<string, WorkflowFile> = {
  implement: {
    description:
      "Research, the track, a spec and a plan on the full track, the build, your review, and the closing stages.",
    base: "implement",
    stage: chain(["research", "track", "spec", "stakes", "plan", "build", "feedback", "closing"]),
  },
  "implement-with-release-gate": secure,
  "gate:release-check": {
    description: "Research, then the release checks, built in code by the gate plugin.",
    base: "research",
    stage: [
      { id: "research", uses: "ostra:research", after: [] },
      { id: "notes", transform: "gate:changelog-lines", after: ["research"], inputs: {} },
      { id: "release", plugin: "gate:release", after: ["notes"] },
    ],
  },
};

export function workflowDoc(name: string, file: WorkflowFile): WorkflowDoc {
  const plugin = name.includes(":") ? name.split(":")[0] : null;
  return { name, builtin: false, file, resolved: null, issues: [], flattened: false, plugin };
}

export function agentDetail(info: AgentInfo): AgentDetail {
  const editable = info.source.kind === "workspace";
  return {
    info,
    doc: {
      name: info.name,
      description: info.description,
      returns: info.returns,
      default_tier: info.default_tier,
      capabilities: info.capabilities,
      write_scope: info.write_scope,
      brief: ["stack", "commands", "skills", "conventions", "modules"],
      timeout_seconds: info.timeout_secs,
      effort: info.effort,
      data_schema: editable
        ? { type: "object", required: ["risk"], properties: { risk: { type: "string", enum: ["low", "high"] } } }
        : null,
      helper: info.helper,
      prompt: editable
        ? "Read every file the change touched with {{ tool_read }} and report each secret and unsafe input.\n"
        : "",
    },
    editable,
    waiting_approval: false,
    prompt_preview: `# ${info.label}\n\n${info.description}`,
    submit_schema: { type: "object" },
    used_by: info.name === "security-auditor" ? ["implement-with-release-gate/audit"] : [],
  };
}

export const functions: Record<string, FunctionFile> = {
  "high-files": {
    description: "The files of findings at a risk.",
    output: "files",
    input: [{ name: "findings", kind: "array", required: true, description: "Findings with a file and a risk." }],
    arg: [{ name: "risk", kind: "string", required: true, description: "The risk to keep." }],
    step: [
      {
        id: "high",
        transform: "filter",
        inputs: { items: "input.findings" },
        args: { field: "risk", op: "eq", to: "$risk" },
      },
      { id: "files", transform: "map", inputs: { items: "high.output" }, args: { field: "file" } },
    ],
  },
};

export function functionDoc(name: string, file: FunctionFile): FunctionDoc {
  return {
    name,
    file,
    info: {
      name,
      description: file.description,
      inputs: file.input ?? [],
      variadic: false,
      args: file.arg ?? [],
      output: "array",
      custom: true,
    },
    issues: [],
    used_by: [],
  };
}

export const plugins: PluginInfo[] = [
  {
    name: "gate",
    builtin: false,
    config: { name: "gate", command: ["./gate-plugin"], env: {}, enabled: true, timeout_secs: 120 },
    state: "running",
    error: null,
    manifest: {
      name: "gate",
      version: "1.0.0",
      description: "Release checks.",
      agents: [],
      stages: [{ name: "release", description: "Runs the release checks." }],
      contracts: [],
    },
  },
];
