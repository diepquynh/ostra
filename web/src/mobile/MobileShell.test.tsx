import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { routes } from "../App";
import { SESSION, WS } from "../api/mock/fixtures";
import type { TreeSession } from "../api/nav";
import { mobileTitle, sessionOf } from "./MobileShell";
import { MOBILE_QUERY } from "./useMobile";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let host: HTMLDivElement | null = null;
let router: ReturnType<typeof createMemoryRouter>;

async function settle(rounds = 5) {
  for (let i = 0; i < rounds; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
  }
}

async function mount(path: string) {
  host = document.createElement("div");
  document.body.appendChild(host);
  router = createMemoryRouter(routes, { initialEntries: [path] });
  await act(async () => {
    root = createRoot(host!);
    root.render(<RouterProvider router={router} />);
  });
  await settle();
}

const button = (name: string) =>
  [...document.querySelectorAll("button")].find(
    (b) =>
      b.getAttribute("aria-label") === name ||
      b.textContent === name ||
      [...b.querySelectorAll("span")].some((s) => s.textContent === name),
  );

async function tap(name: string) {
  const b = button(name);
  if (!b) throw new Error(`no button ${name}`);
  await act(async () => b.click());
  await settle(2);
}

beforeEach(() => {
  vi.stubGlobal(
    "matchMedia",
    (q: string) =>
      ({
        matches: q === MOBILE_QUERY,
        media: q,
        addEventListener: () => {},
        removeEventListener: () => {},
      }) as unknown as MediaQueryList,
  );
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  localStorage.clear();
  vi.unstubAllGlobals();
});

describe("mobile shell", () => {
  it("replaces the tabbed layout at phone width", async () => {
    await mount(`/w/${WS}`);
    expect(document.querySelector(".m-shell")).not.toBeNull();
    expect(document.querySelector(".shell-tabs")).toBeNull();
    expect(document.querySelector(".m-title-main")?.textContent).toBe("shop");
    expect(document.querySelector(".m-footer")?.textContent).toContain("Mock data");
    expect(button("Back")).toBeUndefined();
  });

  it("opens the menu, search, and question sheets", async () => {
    await mount(`/w/${WS}`);
    await tap("Menu");
    const sheet = () => document.querySelector(".m-sheet")?.textContent ?? "";
    expect(sheet()).toContain("Workspaces");
    expect(sheet()).toContain("Run the setup guide again");
    await act(async () => (document.querySelector(".m-scrim") as HTMLElement).click());
    expect(document.querySelector(".m-sheet")).toBeNull();
    await tap("Search");
    expect(sheet()).toContain("Sessions");
    await act(async () => (document.querySelector(".m-scrim") as HTMLElement).click());
    await tap("Ask a quick question");
    expect(sheet()).toContain("Answers are read-only");
  });

  it("goes back through the stack, and to the overview from a deep link", async () => {
    await mount(`/w/${WS}`);
    await tap("Menu");
    await tap("Cost");
    expect(router.state.location.pathname).toBe(`/w/${WS}/cost`);
    expect(document.querySelector(".m-title-main")?.textContent).toBe("Cost");
    await tap("Back");
    expect(router.state.location.pathname).toBe(`/w/${WS}`);
    expect(router.state.historyAction).toBe("POP");

    act(() => root?.unmount());
    host?.remove();
    await mount(`/w/${WS}/s/${SESSION}`);
    await tap("Back");
    expect(router.state.location.pathname).toBe(`/w/${WS}`);
    expect(router.state.historyAction).toBe("REPLACE");
  });
});

const session = (over: Partial<TreeSession>): TreeSession => ({
  id: "s1",
  title: "Order cancel",
  request: "Add order cancellation",
  kind: { kind: "pipeline" },
  status: "waiting",
  open_gates: 1,
  cost_usd: 1,
  updated_at: "2026-09-27T10:00:00Z",
  groups: [
    {
      group: "explore:backend",
      agent: "explore",
      project: "backend",
      status: "ok",
      cost_usd: 0.2,
      runs: [{ id: "x1", run_label: "run 1", status: "ok", stream: "activity", summary: null }],
    },
  ],
  artifacts: [{ path: "/w/.ostra/sessions/s1/spec.md", label: "Spec", kind: "spec", project: null }],
  ...over,
});

describe("mobileTitle", () => {
  const ctx = {
    wsName: "shop",
    root: "/home/me/shop",
    sessions: [session({})],
    projectPath: (k: string) => (k === "backend" ? "/home/me/shop/backend" : undefined),
  };
  it.each([
    ["ws:overview", "shop", "/home/me/shop"],
    ["ws:cost", "Cost", "shop"],
    ["session:s1", "Order cancel", "Session · Waiting for you"],
    ["exec:x1", "Explore · run 1", "backend · ok"],
    ["artifact:/w/.ostra/sessions/s1/spec.md", "Spec", "/w/.ostra/sessions/s1/spec.md"],
    ["project:backend", "backend", "/home/me/shop/backend"],
    ["project:_artifacts", "Artifacts", "/home/me/shop/.ostra/artifacts"],
    ["file:backend:src/main.rs", "main.rs", "backend/src/main.rs"],
  ])("%s", (id, title, sub) => {
    expect(mobileTitle(id, ctx)).toEqual({ title, sub });
  });

  it("finds the session of an execution and an artifact", () => {
    const s = [session({})];
    expect(sessionOf("exec:x1", s)?.id).toBe("s1");
    expect(sessionOf("artifact:/w/.ostra/sessions/s1/spec.md", s)?.id).toBe("s1");
    expect(sessionOf("ws:cost", s)).toBeNull();
  });
});
