import { describe, expect, it } from "vitest";
import { IDLE, markState } from "./LiveMark";

describe("markState", () => {
  it("lights the lanes before the current one and colors the current one by status", () => {
    const m = markState({ lane: "verification", status: "waiting" });
    expect(m.arcs).toEqual(["done", "done", "wait", "off", "off", "off", "off", "off"]);
    expect(m.pearl).toBe("wait");
  });

  it("shows a failed test lane", () => {
    expect(markState({ lane: "test", status: "failed" }).arcs[6]).toBe("fail");
  });

  it("lights every lane once completed", () => {
    const m = markState({ lane: "docs", status: "completed" });
    expect(m.arcs.every((a) => a === "done")).toBe(true);
    expect(m.pearl).toBe("ok");
  });

  it("is dark with no session", () => {
    expect(markState(null)).toBe(IDLE);
  });
});
