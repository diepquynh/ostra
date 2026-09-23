import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, socket } from "../api";
import { sessionSummary, WS } from "../api/mock/fixtures";
import type { WireMsg } from "../api/socket";
import { activityStore, treeStore, useActivity, useSearch, useWorkspaceTree } from "./live";

type Handler = (m: WireMsg) => void;
let handlers: Map<string, Set<Handler>>;
const emit = (channel: string, m: WireMsg) => act(() => handlers.get(channel)?.forEach((h) => h(m)));

beforeEach(() => {
  handlers = new Map();
  vi.spyOn(socket(), "subscribe").mockImplementation((channel: string, h: Handler) => {
    if (!handlers.has(channel)) handlers.set(channel, new Set());
    handlers.get(channel)!.add(h);
    return () => handlers.get(channel)?.delete(h);
  });
});

afterEach(() => {
  cleanup();
  treeStore.clear();
  activityStore.clear();
  vi.restoreAllMocks();
});

describe("useWorkspaceTree", () => {
  it("loads the tree, newest first, and applies patches and summary updates", async () => {
    const { result } = renderHook(() => useWorkspaceTree(WS));
    await waitFor(() => expect(result.current.sessions.length).toBe(13));
    expect(result.current.sessions[0].id).toBe("s_demo");
    const groups = result.current.sessions[0].groups;
    expect(groups.length).toBeGreaterThan(0);

    emit(`workspace:${WS}`, { type: "session_updated", summary: { ...sessionSummary, title: "Cancel orders", updated_at: "2026-09-22T11:00:00Z" } });
    expect(result.current.sessions[0]).toMatchObject({ id: "s_demo", title: "Cancel orders" });
    expect(result.current.sessions[0].groups).toBe(groups);

    const patched = { ...result.current.sessions[12], updated_at: "2026-09-22T12:00:00Z", status: "running" as const };
    emit(`workspace:${WS}`, { type: "tree_patch", workspace: WS, session: patched });
    expect(result.current.sessions[0]).toMatchObject({ id: patched.id, status: "running" });
    expect(result.current.sessions).toHaveLength(13);
  });

  it("reports a failed load as an error", async () => {
    vi.spyOn(api, "tree").mockRejectedValue(new Error("tree broke"));
    const { result } = renderHook(() => useWorkspaceTree(WS));
    await waitFor(() => expect(result.current.error?.message).toBe("tree broke"));
    expect(result.current.sessions).toEqual([]);
  });
});

describe("useActivity", () => {
  it("loads the counts and spend, then follows activity messages", async () => {
    const { result } = renderHook(() => useActivity(WS));
    await waitFor(() => expect(result.current.activity?.spend_week_usd).toBe(4.18));
    expect(result.current.activity?.spend_today_usd).toBe(1.84);
    expect(result.current.activity?.open_gates.length).toBeGreaterThan(0);

    emit(`workspace:${WS}`, { type: "activity", workspace: WS, activity: { running: [], open_gates: [], spend_today_usd: 2, spend_week_usd: 5, today_since: "2026-09-24T00:00:00Z", week_since: "2026-09-18T00:00:00Z" } });
    expect(result.current.activity).toEqual({ running: [], open_gates: [], spend_today_usd: 2, spend_week_usd: 5, today_since: "2026-09-24T00:00:00Z", week_since: "2026-09-18T00:00:00Z" });
  });
});

describe("useSearch", () => {
  it("returns server hits for a query and nothing for an empty one", async () => {
    const search = vi.spyOn(api, "search");
    const { result, rerender } = renderHook(({ q }) => useSearch(WS, q, 0), { initialProps: { q: "refund" } });
    await waitFor(() => expect(result.current.items.length).toBeGreaterThan(0));
    expect(result.current.unavailable).toBe(false);
    expect(result.current.items.every((h) => typeof h.score === "number")).toBe(true);

    rerender({ q: "  " });
    expect(result.current.items).toEqual([]);
    expect(search).toHaveBeenCalledTimes(1);
  });

  it("marks search unavailable when the request fails, and recovers on the next query", async () => {
    const search = vi.spyOn(api, "search").mockRejectedValueOnce(new Error("offline"));
    const { result, rerender } = renderHook(({ q }) => useSearch(WS, q, 0), { initialProps: { q: "refund" } });
    await waitFor(() => expect(result.current.unavailable).toBe(true));
    expect(result.current.items).toEqual([]);

    rerender({ q: "order" });
    await waitFor(() => expect(result.current.items.length).toBeGreaterThan(0));
    expect(result.current.unavailable).toBe(false);
    expect(search).toHaveBeenCalledTimes(2);
  });
});
