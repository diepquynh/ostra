import { describe, expect, it } from "vitest";
import type { EnvironmentStatus } from "../../api/types";
import {
  canContinue,
  expandHome,
  importErrors,
  initialValues,
  isChosen,
  issuesByStep,
  keyError,
  patchName,
  presetsFor,
  projectIndex,
  projectStart,
  repoName,
  stackOptions,
  stepForIssue,
  stepsFor,
  suggestKey,
  toCreateBody,
  tomlFor,
  type WizardValues,
} from "./wizard";

describe("project keys", () => {
  it("take the repository name from a git URL", () => {
    expect(repoName("https://github.com/acme/shop-api.git")).toBe("shop-api");
    expect(repoName("git@github.com:acme/Shop_Web.git")).toBe("Shop_Web");
    expect(suggestKey(repoName("ssh://git@host:2222/team/app/"))).toBe("app");
    expect(repoName("")).toBe("");
  });

  it("derives a key from the folder name with the server's slug rule", () => {
    expect(suggestKey("shop-backend")).toBe("shop-backend");
    expect(suggestKey("Billing Service")).toBe("billing-service");
    expect(suggestKey("__My.App__")).toBe("my-app");
    expect(suggestKey("日本")).toBe("project");
  });

  it("rejects bad and taken keys", () => {
    expect(keyError("backend", [])).toBeNull();
    expect(keyError("", [])).toBe("Give the project a key.");
    expect(keyError("-x", [])).toMatch(/lowercase letters/);
    expect(keyError("Web", [])).toMatch(/lowercase letters/);
    expect(keyError("web", ["web"])).toBe("This key is already used in the workspace.");
  });

  it("places the import endpoint's issues on their fields", () => {
    const issues = [
      { path: "key", message: "`X` is not a project key." },
      { path: "stack", message: "`Go Lang` is not a stack name." },
      { path: "path", message: "/nope does not exist." },
      { path: "key", message: "Choose another key." },
    ];
    expect(importErrors(issues, "Fix these problems")).toEqual({
      fields: {
        key: "`X` is not a project key. Choose another key.",
        stack: "`Go Lang` is not a stack name.",
        path: "/nope does not exist.",
      },
      general: null,
    });
    expect(importErrors([{ path: "", message: "Disk full." }], "x")).toEqual({ fields: {}, general: "Disk full." });
    expect(importErrors([], "The server is not reachable.")).toEqual({
      fields: {},
      general: "The server is not reachable.",
    });
  });

  it("offers detection first, then the server's stacks, keeping an unlisted current value", () => {
    expect(stackOptions(["go", "python"]).map((o) => o.value)).toEqual(["", "go", "python"]);
    expect(stackOptions(["go"], "rust-axum").map((o) => o.label)).toEqual(["Detect from the code", "go", "rust-axum"]);
  });
});

describe("step state", () => {
  it("drops Welcome in the dialog", () => {
    expect(stepsFor(false).map((s) => s.id)).toEqual(["welcome", "check", "name", "projects", "defaults", "review"]);
    expect(stepsFor(true).map((s) => s.id)).toEqual(["check", "name", "projects", "defaults", "review"]);
  });

  it("needs a name and a chosen folder before leaving the name step", () => {
    const v = initialValues();
    expect(canContinue("name", v)).toBe(false);
    expect(canContinue("name", { ...v, name: "shop", root: "/home/me/code/" })).toBe(false);
    expect(canContinue("name", { ...v, name: "shop", root: "/home/me/code/shop" })).toBe(true);
    expect(canContinue("name", { ...v, name: " ", root: "/home/me/code/shop" })).toBe(false);
    expect(canContinue("projects", v)).toBe(true);
  });

  it("lets the name follow the folder until the user types one", () => {
    const v = initialValues();
    expect(patchName(v, "/home/me/code/shop")).toEqual({ root: "/home/me/code/shop", name: "shop" });
    expect(patchName(v, "/home/me/code/")).toEqual({ root: "/home/me/code/", name: "" });
    expect(patchName({ ...v, name: "mine", nameTouched: true }, "/home/me/code/shop")).toEqual({
      root: "/home/me/code/shop",
    });
  });

  it("recognises chosen folders and expands the home folder", () => {
    expect(isChosen("/srv/repos")).toBe(true);
    expect(isChosen("/srv/repos/")).toBe(false);
    expect(isChosen("~/code")).toBe(true);
    expect(isChosen("code")).toBe(false);
    expect(expandHome("~/code", "/home/me")).toBe("/home/me/code");
    expect(expandHome("~/code", null)).toBe("~/code");
    expect(expandHome("/x/~/y", "/home/me")).toBe("/x/~/y");
    expect(projectStart("/home/me/code/shop")).toBe("/home/me/code/");
    expect(projectStart("~/")).toBe("~/");
  });
});

describe("validation issues", () => {
  it("maps each issue path to the step that owns the field", () => {
    expect(stepForIssue("name")).toBe("name");
    expect(stepForIssue("root")).toBe("name");
    expect(stepForIssue("projects[1].key")).toBe("projects");
    expect(stepForIssue("projects[0].path")).toBe("projects");
    expect(stepForIssue("permissions.mode")).toBe("defaults");
    expect(stepForIssue("routing_preset")).toBe("defaults");
    expect(stepForIssue("routing.model.byAgent.plan")).toBe("defaults");
    expect(stepForIssue("yolo.default")).toBe("defaults");
    expect(stepForIssue("notifications.push")).toBe("defaults");
    expect(stepForIssue("limits.session_budget_usd")).toBe("review");
    expect(projectIndex("projects[12].key")).toBe(12);
    expect(projectIndex("root")).toBeNull();
  });

  it("groups issues by step", () => {
    const g = issuesByStep([
      { path: "root", message: "a" },
      { path: "projects[0].key", message: "b" },
      { path: "projects[1].path", message: "c" },
      { path: "routing_preset", message: "d" },
    ]);
    expect(Object.keys(g).sort()).toEqual(["defaults", "name", "projects"]);
    expect(g.projects!.map((i) => i.message)).toEqual(["b", "c"]);
  });
});

const values = (patch: Partial<WizardValues> = {}): WizardValues => ({
  ...initialValues(),
  name: "shop",
  root: "~/code/shop",
  projects: [
    { key: "backend", path: "~/code/shop-backend", stack: "", isGit: true, isOstraProject: true },
    { key: "admin", path: '/srv/repos/shop "admin"', stack: "typescript-node", isGit: true, isOstraProject: false },
  ],
  ...patch,
});

describe("request body and workspace.toml preview", () => {
  it("sends every setting with the home folder expanded", () => {
    expect(toCreateBody(values({ preset: "codex", yolo: true, push: false, mode: "plan" }), "/home/me")).toEqual({
      name: "shop",
      root: "/home/me/code/shop",
      projects: [
        { key: "backend", path: "/home/me/code/shop-backend", stack: null },
        { key: "admin", path: '/srv/repos/shop "admin"', stack: "typescript-node" },
      ],
      permissions: { mode: "plan" },
      yolo: { default: true },
      routing_preset: "codex",
      notifications: { push: false },
    });
  });

  it("writes no executor routes for the native preset", () => {
    const toml = tomlFor(values(), "/home/me");
    expect(toml).not.toContain("routing");
    expect(toml).toContain('[[projects]]\nkey = "backend"\npath = "/home/me/code/shop-backend"\n');
    expect(toml).toContain('path = "/srv/repos/shop \\"admin\\""\nstack = "typescript-node"');
    expect(toml).toContain('[permissions]\nmode = "default"');
    expect(toml).toContain("[yolo]\ndefault = false");
    expect(toml).toContain("[notifications]\npush = true");
  });

  it("routes implementer and write-test to the preset's harness", () => {
    expect(tomlFor(values({ preset: "codex" }))).toContain(
      '[routing.executor.byAgent]\nimplementer = "harness:codex"\n"write-test" = "harness:codex"\n',
    );
    expect(tomlFor(values({ preset: "claude" }))).toContain('implementer = "harness:claude"');
  });

  it("offers only presets for installed, logged-in harnesses", () => {
    const env = (claude: boolean | null, codexInstalled: boolean): EnvironmentStatus => ({
      providers: [],
      harnesses: [
        { harness: "claude", command: "claude", installed: true, version: "1", logged_in: claude },
        { harness: "codex", command: "codex", installed: codexInstalled, version: null, logged_in: true },
      ],
      stacks: [],
    });
    expect(presetsFor(null)).toEqual(["native"]);
    expect(presetsFor(env(true, true))).toEqual(["native", "codex", "claude"]);
    expect(presetsFor(env(null, false))).toEqual(["native"]);
    expect(presetsFor(env(false, true))).toEqual(["native", "codex"]);
  });
});
