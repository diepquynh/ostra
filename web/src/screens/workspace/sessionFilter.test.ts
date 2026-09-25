import { describe, expect, it } from "vitest";
import { sessions } from "../../api/mock/fixtures";
import type { SessionSummary } from "../../api/types";
import {
  filterSessions,
  isFiltered,
  NO_FILTERS,
  type SessionFilters,
  sessionProjects,
  sessionTitle,
  statusCounts,
} from "./sessionFilter";

const ids = (list: SessionSummary[]) => list.map((s) => s.id);
const f = (patch: Partial<SessionFilters>): SessionFilters => ({ ...NO_FILTERS, ...patch });

describe("session filters", () => {
  it("list every session, newest update first, with no filters", () => {
    expect(ids(filterSessions(sessions, NO_FILTERS))).toEqual(["s_demo", "s_refund", "s_init", "s_research", "s_n1"]);
    expect(isFiltered(NO_FILTERS)).toBe(false);
  });

  it("filter by status group", () => {
    expect(ids(filterSessions(sessions, f({ status: "active" })))).toEqual(["s_demo", "s_refund", "s_init"]);
    expect(ids(filterSessions(sessions, f({ status: "waiting" })))).toEqual(["s_demo"]);
    expect(ids(filterSessions(sessions, f({ status: "completed" })))).toEqual(["s_research"]);
    expect(ids(filterSessions(sessions, f({ status: "failed" })))).toEqual(["s_n1"]);
  });

  it("filter by kind and project", () => {
    expect(ids(filterSessions(sessions, f({ kind: "init" })))).toEqual(["s_init"]);
    expect(ids(filterSessions(sessions, f({ kind: "pipeline", project: "web" })))).toEqual(["s_demo"]);
    expect(ids(filterSessions(sessions, f({ project: "backend", status: "active" })))).toEqual(["s_demo", "s_refund"]);
  });

  it("match every word of the text against title, request, stage and projects", () => {
    expect(ids(filterSessions(sessions, f({ text: "refund" })))).toEqual(["s_refund", "s_research"]);
    expect(ids(filterSessions(sessions, f({ text: "REFUND webhook" })))).toEqual(["s_refund"]);
    expect(ids(filterSessions(sessions, f({ text: "fact-check" })))).toEqual(["s_refund"]);
    expect(ids(filterSessions(sessions, f({ text: "cancel button" })))).toEqual(["s_demo"]);
    expect(ids(filterSessions(sessions, f({ text: "nothing like this" })))).toEqual([]);
    expect(isFiltered(f({ text: "x" }))).toBe(true);
    expect(isFiltered(f({ text: "   " }))).toBe(false);
  });

  it("count each status tab under the other filters", () => {
    expect(statusCounts(sessions, NO_FILTERS)).toEqual({ all: 5, active: 3, waiting: 1, completed: 1, failed: 1 });
    expect(statusCounts(sessions, f({ project: "web", status: "completed" }))).toEqual({
      all: 2,
      active: 2,
      waiting: 1,
      completed: 0,
      failed: 0,
    });
  });

  it("title a session by its classified title, else its request", () => {
    expect(sessionTitle({ title: "Order cancellation", request: "Add order cancellation" })).toBe("Order cancellation");
    expect(sessionTitle({ title: null, request: "Add order cancellation" })).toBe("Add order cancellation");
    expect(sessionTitle({ title: "  ", request: "Add order cancellation" })).toBe("Add order cancellation");
  });

  it("offer every project that the workspace or a session names", () => {
    expect(sessionProjects(sessions, ["backend", "admin"])).toEqual(["admin", "backend", "web"]);
  });
});
