import { describe, expect, it } from "vitest";
import { sessionDetail } from "../../api/mock/fixtures";
import { gateSessions } from "../../api/mock/fixtures.session";
import type { GateView, SessionDetail, StageCard, StoredEvent } from "../../api/types";
import {
  answeredGates,
  defaultLane,
  eventLine,
  laneStates,
  mergeEvents,
  openGatesInOrder,
  phaseNodes,
  stageMeta,
} from "./board";

const stage = (lane: StageCard["lane"], status: StageCard["status"], extra: Partial<StageCard> = {}): StageCard => ({
  stage: "explore",
  lane,
  label: `${lane} ${status}`,
  status,
  project: null,
  phase: null,
  executions: [],
  gate: null,
  detail: null,
  ...extra,
});

const withStages = (stages: StageCard[], status: SessionDetail["summary"]["status"] = "running"): SessionDetail => ({
  ...sessionDetail,
  summary: { ...sessionDetail.summary, status },
  stages,
  gates: [],
});

const gate = (
  id: string,
  kind: "permission" | "closing_gate",
  opened: string,
  answeredAt: string | null = null,
): GateView => ({
  ...sessionDetail.gates[0],
  id,
  payload: kind === "permission" ? sessionDetail.gates[0].payload : { kind: "closing_gate", items: [] },
  opened_at: opened,
  answer: answeredAt ? { kind: "permission", answer: "allow-once" } : null,
  answered_at: answeredAt,
});

const board = (id: string) => gateSessions.find((d) => d.summary.id === id)!;

describe("lane states", () => {
  it("marks waiting, running, done, failed, and pending lanes from their stages", () => {
    const lanes = laneStates(
      withStages([
        stage("research", "done", { detail: "2 docs" }),
        stage("requirements", "done"),
        stage("verification", "failed", { detail: "FAIL, 2 findings" }),
        stage("design", "running", { detail: "Plan v1" }),
        stage("build", "waiting"),
        stage("review", "done"),
        stage("review", "pending"),
      ]),
    );
    expect(lanes.research).toMatchObject({ status: "done", detail: "2 docs" });
    expect(lanes.requirements).toMatchObject({ status: "done", detail: "1 stage" });
    expect(lanes.verification).toMatchObject({ status: "failed", detail: "FAIL, 2 findings" });
    expect(lanes.design).toMatchObject({ status: "current", detail: "Plan v1" });
    expect(lanes.build).toMatchObject({ status: "waiting", detail: "Waiting for you" });
    expect(lanes.review).toMatchObject({ status: "current", detail: "1 of 2 done" });
    expect(lanes.test.status).toBe("pending");
    expect(lanes.research.why).toContain("research pass");
  });

  it("skips an empty lane once a later lane has started, or once the session completed", () => {
    const lanes = laneStates(withStages([stage("research", "done"), stage("build", "running")]));
    expect(lanes.design.status).toBe("skipped");
    expect(lanes.test.status).toBe("pending");
    expect(laneStates(withStages([stage("research", "done")], "completed")).docs.status).toBe("skipped");
    expect(laneStates(withStages([stage("build", "skipped")])).build).toMatchObject({
      status: "skipped",
      detail: "Skipped",
    });
  });

  it("opens on the current gate's lane", () => {
    expect(defaultLane(sessionDetail)).toBe("review");
    expect(defaultLane(board("s_export"))).toBe("requirements");
    expect(defaultLane(board("s_init"))).toBe("design");
    expect(defaultLane(withStages([stage("research", "done"), stage("build", "running")]))).toBe("build");
  });
});

describe("gate order", () => {
  it("puts the permission ask first, then the other open gates oldest first", () => {
    const d = {
      ...sessionDetail,
      gates: [
        gate("c2", "closing_gate", "2026-09-22T10:05:00Z"),
        gate("c1", "closing_gate", "2026-09-22T10:01:00Z"),
        gate("p", "permission", "2026-09-22T10:09:00Z"),
      ],
    };
    expect(openGatesInOrder(d).map((g) => g.id)).toEqual(["p", "c1", "c2"]);
  });

  it("lists answered gates newest answer first", () => {
    const d = {
      ...sessionDetail,
      gates: [
        gate("a", "permission", "2026-09-22T10:00:00Z", "2026-09-22T10:02:00Z"),
        gate("b", "permission", "2026-09-22T10:01:00Z", "2026-09-22T10:08:00Z"),
        gate("open", "closing_gate", "2026-09-22T10:00:00Z"),
      ],
    };
    expect(answeredGates(d).map((g) => g.id)).toEqual(["b", "a"]);
  });
});

describe("board rows", () => {
  it("shows what a running execution is doing on its stage row, else the stage detail and project", () => {
    const execs = new Map(sessionDetail.executions.map((x) => [x.id, x]));
    const running = stage("build", "running", { executions: ["x_imp3"], label: "Phase 3", project: "web" });
    expect(stageMeta(running, execs)).toBe("Update src/components/OrderActions.tsx");
    expect(
      stageMeta(stage("review", "done", { detail: "passed", project: "backend", label: "Review phase 1" }), execs),
    ).toBe("passed · backend");
    expect(stageMeta(stage("research", "done", { project: "web", label: "Explore web" }), execs)).toBeUndefined();
  });

  it("lays phases out in dependency columns for the phase graph", () => {
    const layers = phaseNodes(sessionDetail.phases);
    expect(layers.map((l) => l.map((p) => p.id))).toEqual([[1], [2], [3]]);
    expect(layers[1][0]).toMatchObject({
      status: "reviewing",
      reviewPass: 2,
      complexity: "high",
      testPolicy: "Required",
      dependsOn: [1],
    });
    expect(phaseNodes(board("s_stock").phases)).toEqual([]);
  });
});

describe("event log", () => {
  const ev = (seq: number, message: string): StoredEvent => ({
    seq,
    at: "2026-09-22T10:00:00Z",
    event: { type: "note", message },
  });

  it("merges the snapshot and live events without duplicates, in sequence order", () => {
    expect(mergeEvents([ev(1, "a"), ev(2, "b")], [ev(3, "c"), ev(2, "b")]).map((e) => e.seq)).toEqual([1, 2, 3]);
  });

  it("formats one fixed-width line per event", () => {
    expect(eventLine(ev(1, "Format ran in web"))).toMatch(/^\d\d:\d\d:\d\d {2}note {18}Format ran in web$/);
  });
});
