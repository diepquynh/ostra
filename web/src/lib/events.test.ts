import { describe, expect, it } from "vitest";
import { sessionDetail } from "../api/mock/fixtures";
import type { DecisionView, PhaseView, SessionEvent } from "../api/types";
import {
  activeSecurityBlocks,
  applyDelta,
  currentGate,
  decisionChoice,
  denialAdvice,
  emptyActivity,
  foldActivity,
  phaseLayers,
  splitDecidedForYou,
  stagesByLane,
  toolSummary,
} from "./events";

describe("activity folding", () => {
  it("merges text, attaches policy and results, and ignores replayed deltas", () => {
    let s = emptyActivity();
    s = applyDelta(s, 1, { kind: "text", text: "Hello " });
    s = applyDelta(s, 2, { kind: "text", text: "world" });
    s = applyDelta(s, 3, { kind: "tool_call", call_id: "c1", call: { tool: "Bash", input: { command: "ls" } } });
    s = applyDelta(s, 4, { kind: "policy", call_id: "c1", decision: { decision: "allow", rule: null } });
    s = applyDelta(s, 5, { kind: "tool_output", call_id: "c1", chunk: "a\n" });
    s = applyDelta(s, 6, { kind: "tool_result", call_id: "c1", output: "a\nb\n", is_error: false, duration_ms: 12 });
    s = applyDelta(s, 3, { kind: "text", text: "replayed" });
    expect(s.entries).toHaveLength(2);
    expect(s.entries[0]).toMatchObject({ kind: "text", text: "Hello world" });
    expect(s.entries[1]).toMatchObject({ kind: "tool", live: "a\n", output: "a\nb\n", done: true, durationMs: 12 });
    expect(s.lastSeq).toBe(6);
  });

  it("folds a REST snapshot and records usage and session ids", () => {
    const s = foldActivity([
      { seq: 1, at: "", delta: { kind: "thinking", text: "a" } },
      { seq: 2, at: "", delta: { kind: "thinking", text: "b" } },
      { seq: 3, at: "", delta: { kind: "native_session_id", id: "abc" } },
      {
        seq: 4,
        at: "",
        delta: {
          kind: "usage",
          usage: {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 3,
            cache_write_tokens: 0,
            cache_write_1h_tokens: 0,
            cost_usd: 0.1,
            tool_calls: 1,
            build_ms: 0,
            context_tokens: 0,
          },
        },
      },
    ]);
    expect(s.entries).toEqual([{ kind: "thinking", seq: 1, text: "ab", turn: null }]);
    expect(s.nativeSessionId).toBe("abc");
    expect(s.usage?.cost_usd).toBe(0.1);
  });

  it("attaches each native response's cost to its thinking and splits it across its tool calls", () => {
    const usage = (cost_usd: number) => ({
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      cache_write_1h_tokens: 0,
      cost_usd,
      tool_calls: 0,
      build_ms: 0,
      context_tokens: 0,
    });
    const read = { tool: "Read", input: { file_path: "a" } };
    const s = foldActivity([
      { seq: 1, at: "", delta: { kind: "thinking", text: "first" } },
      { seq: 2, at: "", delta: { kind: "turn", usage: usage(0.4), call_ids: ["a", "b"] } },
      { seq: 3, at: "", delta: { kind: "tool_call", call_id: "a", call: read } },
      { seq: 4, at: "", delta: { kind: "tool_call", call_id: "b", call: read } },
      { seq: 5, at: "", delta: { kind: "thinking", text: "second" } },
      { seq: 6, at: "", delta: { kind: "turn", usage: usage(0.1), call_ids: [] } },
      { seq: 7, at: "", delta: { kind: "thinking", text: "third" } },
      { seq: 8, at: "", delta: { kind: "turn", usage: usage(0.3), call_ids: [] } },
      { seq: 9, at: "", delta: { kind: "turn", usage: usage(0.2), call_ids: [] } },
    ]);
    expect(s.entries).toMatchObject([
      { kind: "thinking", text: "first", turn: { cost: 0.4, calls: 2 } },
      { kind: "tool", callId: "a", cost: 0.2 },
      { kind: "tool", callId: "b", cost: 0.2 },
      { kind: "thinking", text: "second", turn: { cost: 0.1, calls: 0 } },
      { kind: "thinking", text: "third", turn: { cost: 0.3, calls: 0 } },
      { kind: "turn", turn: { cost: 0.2, calls: 0 } },
    ]);
  });

  it("summarizes tool calls", () => {
    expect(toolSummary({ tool: "Read", input: { file_path: "/a/b.ts" } })).toBe("/a/b.ts");
    expect(toolSummary({ tool: "Grep", input: { pattern: "foo", path: "src" } })).toBe("foo in src");
    expect(toolSummary({ tool: "submit_explore", input: {} })).toContain("submitted");
  });

  it("explains denials", () => {
    expect(denialAdvice({ decision: "allow", rule: null })).toBeNull();
    expect(denialAdvice({ decision: "deny", reason: "x", rule: { layer: "guard", rule: "build-streak" } })).toContain(
      "STUCK",
    );
    expect(
      denialAdvice({ decision: "ask", reason: "x", rule: { layer: "permission", rule: "mode:default" } }),
    ).toContain("Allow it once");
  });
});

describe("board helpers", () => {
  const phase = (id: number, deps: number[] | null): PhaseView => ({
    info: {
      id,
      deliverable: null,
      project: "p",
      title: `P${id}`,
      complexity: "low",
      test_policy: "Required",
      depends_on: deps,
      file: null,
      test_rationale: null,
    },
    status: "queued",
    review_iterations: 0,
    tests: "none",
    security_block: false,
  });

  it("layers the phase DAG, treating unreadable dependencies as depending on earlier phases", () => {
    const layers = phaseLayers([phase(1, []), phase(2, [1]), phase(3, []), phase(4, [2, 3]), phase(5, null)]);
    expect(layers.map((l) => l.map((p) => p.info.id))).toEqual([[1, 3], [2], [4], [5]]);
  });

  it("survives a dependency cycle", () => {
    expect(phaseLayers([phase(1, [2]), phase(2, [1])]).flat()).toHaveLength(2);
  });

  it("groups stages by lane in SDLC order", () => {
    const lanes = stagesByLane(sessionDetail.stages);
    expect([...lanes.keys()][0]).toBe("research");
    expect(lanes.get("build")!.length).toBeGreaterThan(0);
  });

  it("puts permission asks first", () => {
    expect(currentGate(sessionDetail)?.payload.kind).toBe("permission");
    expect(currentGate({ ...sessionDetail, gates: [] })).toBeNull();
  });

  it("phrases decisions", () => {
    const d = (judge: DecisionView["judge"], output: unknown): DecisionView => ({
      id: "d",
      judge,
      subject: null,
      input_summary: "",
      output,
      reason: "",
      overridden: false,
      can_override: true,
      at: "",
    });
    expect(decisionChoice(d("stakes", { stakes: "low" }))).toContain("skipped");
    expect(decisionChoice(d("classify", { category: "IMPLEMENT", projects: ["a"] }))).toBe(
      "to treat this as Implement in a",
    );
    expect(decisionChoice(d("sufficiency", { items: [{ needed: true }, { needed: false }] }))).toBe(
      "to run 1 more research pass",
    );
  });

  it("splits the Decided for you section out of a completion report", () => {
    const r = splitDecidedForYou("# Done\n\nBody.\n\n## Decided for you\n\n- A\n\n## Stages not run\n\n- Tests");
    expect(r.decided).toBe("- A");
    expect(r.body).toContain("Stages not run");
    expect(r.body).not.toContain("Decided");
    expect(splitDecidedForYou("# Done").decided).toBeNull();
  });

  it("tracks security blocks until a clean review clears them", () => {
    const block = (n: number): SessionEvent => ({
      type: "security_block",
      project: "p",
      phase: 1,
      tests: false,
      findings: Array.from({ length: n }, () => ({
        severity: "BLOCKER" as const,
        file: "a",
        rule: "SEC-BLOCK-X",
        description: "",
        fix: "",
        guidance: null,
      })),
    });
    expect(activeSecurityBlocks([block(1)])).toHaveLength(1);
    expect(activeSecurityBlocks([block(1), block(0)])).toHaveLength(0);
  });
});
