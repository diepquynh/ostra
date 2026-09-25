import { describe, expect, it } from "vitest";
import { settings as fixture } from "../../api/mock/fixtures";
import type { WorkspaceSettings } from "../../api/types";
import {
  anchorCandidates,
  fieldForIssue,
  fieldIds,
  fieldToModel,
  fromForm,
  joinCommand,
  mapIssues,
  mcpRow,
  modelToField,
  parsePerExecutor,
  pickModel,
  settingKeyOf,
  splitCommand,
  stableJson,
  tabOf,
  toForm,
} from "./settingsForm";

const clone = () => structuredClone(fixture);

describe("settings form round trip", () => {
  it("returns the same settings when nothing is edited", () => {
    const s = clone();
    const { settings, issues } = fromForm(toForm(s), s);
    expect(issues).toEqual([]);
    expect(settings).toEqual(s);
    expect(stableJson(settings)).toBe(stableJson(s));
    expect(stableJson({ b: 1, a: { d: [2, { f: 1, e: 0 }], c: 3 } })).toBe('{"a":{"c":3,"d":[2,{"e":0,"f":1}]},"b":1}');
  });

  it("keeps per-executor tables, concrete models, efforts and fields the form does not know", () => {
    const s: WorkspaceSettings & { extra?: string } = clone();
    s.routing.model.byAgent.plan = { native: "anthropic:claude-opus-5-5", codex: "gpt-5.6-sol" };
    s.routing.model.byAgent.explore = "anthropic:claude-sonnet-5";
    s.routing.model.byAgent.mystery = "fast";
    s.routing.effort = {
      byAgent: { plan: "high", explore: "low" },
      byPhaseComplexity: { implementer: { high: "xhigh" } },
    };
    s.extra = "kept";
    const f = toForm(s);
    expect(f.model.plan).toEqual({
      kind: "per-executor",
      text: "native = anthropic:claude-opus-5-5, codex = gpt-5.6-sol",
    });
    expect(f.model.explore).toEqual({ kind: "custom", text: "anthropic:claude-sonnet-5" });
    expect(fromForm(f, s).settings).toEqual(s);
  });

  it("writes edits into the right settings fields", () => {
    const s = clone();
    const f = toForm(s);
    f.name = "shop-2";
    f.executor["code-reviewer"] = "harness:claude";
    f.executor.implementer = "native";
    f.model.plan = pickModel(f.model.plan, "frontier");
    f.model.judge = pickModel(f.model.judge, "unset");
    f.complexityExecutor.implementer.high = "";
    f.complexityModel["write-test"].low = pickModel(f.complexityModel["write-test"].low, "unset");
    f.complexityModel["write-test"].medium = pickModel(f.complexityModel["write-test"].medium, "unset");
    f.complexityModel["write-test"].high = pickModel(f.complexityModel["write-test"].high, "unset");
    f.effort.explore = "max";
    f.complexityEffort.implementer.high = "max";
    f.instructionsAll = "  ";
    f.instructionsAgents.implementer = "";
    f.instructionsAgents.plan = "Name every risk.";
    f.allow = "Bash(cargo *)\n\n  Bash(npm run test *)  \n";
    f.deny = "";
    f.mode = "acceptEdits";
    f.yolo = true;
    f.push = false;
    f.maxParallel = "5";
    f.budget = "12.5";
    f.projects[1].stack = "";

    const { settings: out, issues } = fromForm(f, s);
    expect(issues).toEqual([]);
    expect(out.name).toBe("shop-2");
    expect(out.routing.executor.byAgent).toEqual({ "code-reviewer": "harness:claude", "write-test": "harness:codex" });
    expect(out.routing.model.byAgent.plan).toBe("frontier");
    expect("judge" in out.routing.model.byAgent).toBe(false);
    expect(out.routing.executor.byPhaseComplexity).toEqual({
      implementer: { low: "harness:codex", medium: "harness:codex" },
    });
    expect(out.routing.model.byPhaseComplexity).toEqual({
      implementer: { low: "fast", medium: "fast", high: "balanced" },
    });
    expect(out.routing.effort).toEqual({
      byAgent: { explore: "max" },
      byPhaseComplexity: { implementer: { high: "max" } },
    });
    expect(out.instructions).toEqual({ all: null, agents: { plan: "Name every risk." } });
    expect(out.permissions).toEqual({
      mode: "acceptEdits",
      allow: ["Bash(cargo *)", "Bash(npm run test *)"],
      ask: [],
      deny: [],
    });
    expect(out.yolo.default).toBe(true);
    expect(out.notifications.push).toBe(false);
    expect(out.limits).toEqual({ max_parallel_executions: 5, session_budget_usd: 12.5 });
    expect(out.projects[1]).toEqual({ key: "web", path: "/home/me/code/shop-web", stack: null });
  });

  it("keeps each project's code provider, which the form does not show", () => {
    const s = clone();
    s.projects[0].code_provider = { command: ["my-nav", "--json"], timeout_secs: 5 };
    const f = toForm(s);
    f.projects[0].key = "api";
    const { settings: out } = fromForm(f, s);
    expect(out.projects[0].code_provider).toEqual({ command: ["my-nav", "--json"], timeout_secs: 5 });
    expect("code_provider" in out.projects[1]).toBe(false);
  });

  it("keeps each project's language servers, which the form does not show", () => {
    const s = clone();
    const servers = [{ command: ["rust-analyzer"], languages: ["rust"], timeout_secs: 10 }];
    s.projects[0].language_servers = servers;
    const { settings: out } = fromForm(toForm(s), s);
    expect(out.projects[0].language_servers).toEqual(servers);
    expect("language_servers" in out.projects[1]).toBe(false);
  });

  it("reports what it cannot parse and keeps the saved value for it", () => {
    const s = clone();
    const f = toForm(s);
    f.model.plan = { kind: "per-executor", text: "native anthropic:x" };
    f.model.explore = { kind: "custom", text: " " };
    f.maxParallel = "two";
    f.budget = "-1";
    const { settings: out, issues } = fromForm(f, s);
    expect(issues.map((i) => i.path)).toEqual([
      "routing.model.byAgent.explore",
      "routing.model.byAgent.plan",
      "limits.max_parallel_executions",
      "limits.session_budget_usd",
    ]);
    expect(out.routing.model.byAgent.plan).toBe("advanced");
    expect(out.limits.max_parallel_executions).toBe(3);
  });
});

describe("model routes", () => {
  it("maps each choice to a field and back", () => {
    for (const m of ["default", "fast", "frontier", "gpt-5.6-terra", { native: "anthropic:a", codex: "b" }]) {
      expect(fieldToModel(modelToField(m)).value).toEqual(m);
    }
    expect(modelToField(undefined)).toEqual({ kind: "unset", text: "" });
    expect(fieldToModel({ kind: "unset", text: "" }).value).toBeUndefined();
  });

  it("parses executor = model tables", () => {
    expect(parsePerExecutor("native = anthropic:x,\ncodex=gpt-y").value).toEqual({
      native: "anthropic:x",
      codex: "gpt-y",
    });
    expect(parsePerExecutor("").error).toMatch(/at least one entry/);
    expect(parsePerExecutor("native: x").error).toMatch(/executor = model/);
  });

  it("seeds a per-executor table from the current tier", () => {
    expect(pickModel({ kind: "tier", text: "balanced" }, "per-executor")).toEqual({
      kind: "per-executor",
      text: "native = balanced",
    });
    expect(pickModel({ kind: "tier", text: "balanced" }, "custom")).toEqual({ kind: "custom", text: "" });
  });
});

describe("issue paths", () => {
  const fields = fieldIds(toForm(fixture));

  it("land on the most specific field", () => {
    expect(fieldForIssue("routing.model.byAgent.plan", fields)).toBe("routing.model.byAgent.plan");
    expect(fieldForIssue("routing.executor.byAgent.judge", fields)).toBe("routing.executor.byAgent.judge");
    expect(fieldForIssue("routing.effort.byAgent.explore", fields)).toBe("routing.effort.byAgent.explore");
    expect(fieldForIssue("routing.effort.byPhaseComplexity.implementer.low", fields)).toBe(
      "routing.effort.byPhaseComplexity.implementer.low",
    );
    expect(fieldForIssue("projects[1].key", fields)).toBe("projects[1]");
    expect(fieldForIssue("projects[7].path", fields)).toBe("projects");
    expect(fieldForIssue("routing.model.byPhaseComplexity.implementer.high", fields)).toBe(
      "routing.model.byPhaseComplexity.implementer.high",
    );
    expect(fieldForIssue("routing.model.byPhaseComplexity.other.low", fields)).toBe("routing.model.byPhaseComplexity");
    expect(fieldForIssue("limits.session_budget_usd", fields)).toBe("limits.session_budget_usd");
    expect(fieldForIssue("name", fields)).toBe("name");
    expect(fieldForIssue("nameless", fields)).toBeNull();
    expect(fieldForIssue("routing.effort.unknown", fields)).toBeNull();
  });

  it("group by field and count per tab", () => {
    const m = mapIssues(
      [
        { path: "name", message: "a" },
        { path: "routing.model.byAgent.plan", message: "b" },
        { path: "routing.model.byAgent.plan", message: "c" },
        { path: "limits.max_parallel_executions", message: "d" },
        { path: "surprise", message: "e" },
      ],
      fields,
    );
    expect(Object.keys(m.byField).sort()).toEqual([
      "limits.max_parallel_executions",
      "name",
      "routing.model.byAgent.plan",
    ]);
    expect(m.byField["routing.model.byAgent.plan"].map((i) => i.message)).toEqual(["b", "c"]);
    expect(m.unmatched.map((i) => i.message)).toEqual(["e"]);
    expect(m.byTab).toEqual({
      general: 3,
      projects: 0,
      git: 0,
      routing: 2,
      mcp: 0,
      permissions: 0,
      instructions: 0,
      notifications: 0,
    });
  });

  it("name the tab of every settings key", () => {
    expect(tabOf("yolo.default")).toBe("general");
    expect(tabOf("projects[0].key")).toBe("projects");
    expect(tabOf("routing.effort")).toBe("routing");
    expect(tabOf("permissions.deny")).toBe("permissions");
    expect(tabOf("instructions.agents")).toBe("instructions");
    expect(tabOf("notifications.push")).toBe("notifications");
  });
});

describe("deep links", () => {
  it("try the field, then its parents, then the panel for a table key", () => {
    expect(anchorCandidates("routing.model.byAgent.plan")).toEqual([
      "setting:routing.model.byAgent.plan",
      "setting:routing.byAgent",
    ]);
    expect(anchorCandidates("routing.effort")).toEqual(["setting:routing.byAgent"]);
    expect(anchorCandidates("routing.model.byPhaseComplexity")[0]).toBe("setting:routing.byPhaseComplexity");
    expect(anchorCandidates("projects[2].key")).toEqual([
      "setting:projects[2].key",
      "setting:projects[2]",
      "setting:projects",
    ]);
    expect(anchorCandidates("limits.session_budget_usd")).toEqual([
      "setting:limits.session_budget_usd",
      "setting:limits",
    ]);
  });

  it("read the settings key from an anchor", () => {
    expect(settingKeyOf("setting:permissions.allow")).toBe("permissions.allow");
    expect(settingKeyOf("gate-1")).toBeNull();
    expect(settingKeyOf(null)).toBeNull();
  });
});

describe("MCP servers", () => {
  it("splits and joins command lines", () => {
    expect(splitCommand(`npx -y "@scope/pkg" --root '/a b' x\\ y ""`)).toEqual([
      "npx",
      "-y",
      "@scope/pkg",
      "--root",
      "/a b",
      "x y",
      "",
    ]);
    expect(splitCommand(`npx "open`)).toBeNull();
    const words = ["uvx", "mcp-server", "--dir", "/a b", "it's", ""];
    expect(splitCommand(joinCommand(words))).toEqual(words);
  });

  it("keeps each server through the form, and only the chosen transport's fields", () => {
    const s = clone();
    const form = toForm(s);
    expect(form.mcp.map((r) => r.transport)).toEqual(["http", "stdio", "http"]);
    expect(form.mcp[0].headers).toBe("Authorization: Bearer ${GITHUB_TOKEN}");
    expect(form.mcp[1].command).toBe("npx -y @upstash/context7-mcp");

    form.mcp[1].transport = "http";
    form.mcp[1].url = "https://docs.example/mcp";
    form.mcp[2].scopes = "read  write";
    const next = mcpRow();
    next.name = "local";
    next.transport = "stdio";
    next.command = "node server.js";
    next.env = "TOKEN=${LOCAL_TOKEN}";
    form.mcp.push(next);
    const { settings, issues } = fromForm(form, s);
    expect(issues).toEqual([]);
    expect(settings.mcp_servers[1]).toEqual({
      name: "docs",
      enabled: true,
      url: "https://docs.example/mcp",
      agents: ["explore", "generate-spec"],
      timeout_secs: 120,
    });
    expect(settings.mcp_servers[2].oauth).toEqual({ scopes: ["read", "write"] });
    expect(settings.mcp_servers[3]).toEqual({
      name: "local",
      enabled: true,
      command: ["node", "server.js"],
      env: { TOKEN: "${LOCAL_TOKEN}" },
      timeout_secs: 120,
    });
  });

  it("reports what the browser can see, and routes issues to the MCP tab", () => {
    const s = clone();
    const form = toForm(s);
    form.mcp[0].headers = "Authorization Bearer x";
    form.mcp[1].command = `npx "open`;
    form.mcp[1].env = "=x";
    form.mcp[2].timeout = "soon";
    const { settings, issues } = fromForm(form, s);
    expect(issues.map((i) => i.path)).toEqual([
      "mcp_servers[0].headers",
      "mcp_servers[1].command",
      "mcp_servers[1].env",
      "mcp_servers[2].timeout_secs",
    ]);
    expect(settings.mcp_servers[1].command).toBeUndefined();
    expect(tabOf("mcp_servers[2].timeout_secs")).toBe("mcp");
    expect(fieldForIssue("mcp_servers[0].headers", fieldIds(form))).toBe("mcp_servers[0].headers");
    expect(fieldForIssue("mcp_servers[4].name", fieldIds(form))).toBe("mcp_servers");
    expect(mapIssues(issues, fieldIds(form)).byTab.mcp).toBe(4);
  });
});
