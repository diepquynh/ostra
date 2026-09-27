import { describe, expect, it } from "vitest";
import { sessionDetail } from "../../api/mock/fixtures";
import type { ExecutionView, PhaseView, StageCard } from "../../api/types";
import { laneStates } from "../../screens/session/board";
import { execGroups, laneCards, phaseWaves, stageTarget } from "./sessionView";

const phase = (id: number, depends_on: number[] | null, status: PhaseView["status"] = "queued"): PhaseView => ({
  info: {
    id,
    deliverable: null,
    project: "backend",
    title: `Phase ${id}`,
    complexity: "medium",
    test_policy: "Required",
    depends_on,
    file: id === 1 ? "/s/phase-1.md" : null,
    test_rationale: null,
  },
  status,
  review_iterations: 0,
  tests: "none",
  security_block: false,
});

const stage = (extra: Partial<StageCard>): StageCard => ({
  stage: "explore",
  lane: "build",
  label: "Build",
  status: "running",
  project: null,
  phase: null,
  executions: [],
  gate: null,
  detail: null,
  ...extra,
});

describe("phaseWaves", () => {
  it("puts independent phases in one parallel wave and names dependencies", () => {
    const w = phaseWaves([phase(1, []), phase(2, []), phase(3, [1, 2])]);
    expect(w.map((x) => x.label)).toEqual(["Wave 1 · runs in parallel", "Wave 2"]);
    expect(w[1].phases[0].meta).toBe("Medium · tests required · after P1, P2");
    expect(w[0].phases[0].file).toBe("/s/phase-1.md");
  });
});

describe("laneCards", () => {
  it("has one card per lane, in pipeline order, with a detail line", () => {
    const cards = laneCards(laneStates(sessionDetail));
    expect(cards.map((c) => c.lane)).toEqual([
      "research",
      "requirements",
      "verification",
      "design",
      "build",
      "review",
      "test",
      "docs",
      "done",
    ]);
    for (const c of cards) if (c.status === "pending") expect(c.detail).toBe("Not started");
  });
});

describe("execGroups", () => {
  it("follows the server's groups and resolves each run", () => {
    const groups = execGroups(sessionDetail, Date.parse("2030-01-01T00:00:00Z"));
    expect(groups.map((g) => g.key)).toEqual(sessionDetail.execution_groups.map((g) => g.group));
    const rows = groups.flatMap((g) => g.rows.map((r) => r.id));
    expect(rows.length).toBe(sessionDetail.execution_groups.reduce((n, g) => n + g.executions.length, 0));
  });
});

describe("stageTarget", () => {
  const execs = new Map<string, ExecutionView>(
    [
      { id: "x1", status: "ok" },
      { id: "x2", status: "running" },
    ].map((x) => [x.id, x as ExecutionView]),
  );
  it("opens the gate of a waiting stage", () => {
    expect(stageTarget(stage({ status: "waiting", gate: "g1", executions: ["x1"] }), execs)).toEqual({ gate: "g1" });
  });
  it("opens the running execution, else the latest", () => {
    expect(stageTarget(stage({ executions: ["x2", "x1"] }), execs)).toEqual({ exec: "x2" });
    expect(stageTarget(stage({ status: "done", executions: ["x1"] }), execs)).toEqual({ exec: "x1" });
  });
  it("goes nowhere for a stage with no work yet", () => {
    expect(stageTarget(stage({ status: "pending" }), execs)).toBeNull();
  });
});
