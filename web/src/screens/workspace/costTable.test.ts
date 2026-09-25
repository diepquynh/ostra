import { describe, expect, it } from "vitest";
import { sessions } from "../../api/mock/fixtures";
import type { CostReport, CostRow } from "../../api/types";
import { COST_GROUPS, costLines, costTotals } from "./costTable";

const row = (key: string, cost: number, extra: Partial<CostRow["usage"]> = {}, n = 1): CostRow => {
  const usage = {
    input_tokens: 12000,
    output_tokens: 1800,
    cache_read_tokens: 90000,
    cache_write_tokens: 0,
    cache_write_1h_tokens: 0,
    cost_usd: cost,
    tool_calls: 10,
    build_ms: 0,
    ...extra,
  };
  return {
    key,
    executions: n,
    usage,
    cache_reads_per_tool_call: usage.tool_calls ? usage.cache_read_tokens / usage.tool_calls : 0,
  };
};

describe("cost tables", () => {
  it("sort each grouping by cost, highest first, then by key", () => {
    const lines = costLines([row("b", 0.2), row("a", 0.2), row("c", 1.5)], "agent", 1.9);
    expect(lines.map((l) => l.id)).toEqual(["c", "a", "b"]);
    expect(lines.map((l) => l.share)).toEqual(["79%", "11%", "11%"]);
  });

  it("label sessions by title and link them, and name the side panel", () => {
    const lines = costLines([row("s_demo", 3), row("side-panel", 0.1), row("s_gone", 0.05)], "session", 3.15, sessions);
    expect(lines.map((l) => [l.label, l.open, l.mono])).toEqual([
      ["Order cancellation", "session:s_demo", false],
      ["Side-panel questions", null, false],
      ["s_gone", "session:s_gone", true],
    ]);
  });

  it("label stages by their pipeline name and keep agents and executors as machine names", () => {
    expect(
      costLines([row("fact-check-spec", 1), row("none", 0.5), row("mystery-stage", 0.1)], "stage", 1.6).map(
        (l) => l.label,
      ),
    ).toEqual(["Fact-check the spec", "No stage", "Mystery stage"]);
    const [agent] = costLines([row("code-reviewer", 1)], "agent", 1);
    expect([agent.label, agent.mono]).toEqual(["code-reviewer", true]);
    const [ex] = costLines([row("harness:codex", 1)], "executor", 1);
    expect([ex.label, ex.mono]).toEqual(["harness:codex", true]);
  });

  it("format numbers and leave empty what did not happen", () => {
    const [l] = costLines(
      [row("judge", 0.004, { tool_calls: 0, build_ms: 0, input_tokens: 1_250_000, output_tokens: 950 })],
      "agent",
      0,
    );
    expect(l).toMatchObject({
      runs: 1,
      input: "1.3M",
      output: "950",
      cacheReads: "90.0k",
      cacheWrites5m: "0",
      cacheWrites1h: "0",
      perCall: "",
      build: "",
      cost: "<$0.01",
      share: "",
    });
    const [b] = costLines([row("implementer", 1.2, { build_ms: 125000 })], "agent", 1.2);
    expect(b).toMatchObject({ perCall: "9.0k", build: "2 min 5 s", cost: "$1.20", share: "100%" });
  });

  it("cover every grouping of the report and total it", () => {
    const report: CostReport = {
      since: null,
      by_session: [],
      by_stage: [],
      by_agent: [],
      by_executor: [],
      total: row("total", 4.18, { tool_calls: 312, build_ms: 0 }, 29),
    };
    expect(COST_GROUPS.map((g) => g.field)).toEqual(["by_session", "by_stage", "by_agent", "by_executor"]);
    expect(costTotals(report)).toEqual({
      total: "$4.18",
      runs: "29",
      cacheReads: "90.0k",
      toolCalls: "312",
      perCall: "288",
      build: "none",
    });
  });
});
