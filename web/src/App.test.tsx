import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, describe, expect, it } from "vitest";
import { routes } from "./App";
import { SESSION, WS } from "./api/mock/fixtures";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let host: HTMLDivElement | null = null;
let router: ReturnType<typeof createMemoryRouter> | null = null;

/** Mock responses resolve after 80 ms; wait for them and the renders they trigger. */
async function settle(rounds = 6) {
  for (let i = 0; i < rounds; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
  }
}

async function render(path: string): Promise<string> {
  host = document.createElement("div");
  document.body.appendChild(host);
  router = createMemoryRouter(routes, { initialEntries: [path] });
  await act(async () => {
    root = createRoot(host!);
    root.render(<RouterProvider router={router!} />);
  });
  await settle();
  return document.body.textContent ?? "";
}

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  router = null;
  localStorage.clear();
});

const spec = encodeURIComponent(
  `/home/me/code/shop/.ostra/sessions/${SESSION}/ostra-spec-20260922-100700-order-cancel.md`,
);
const ledger = encodeURIComponent(
  `/home/me/code/shop/.ostra/sessions/${SESSION}/backend/ostra-review-ledger-phase-1.md`,
);

describe("every screen renders on mock data inside the shell", () => {
  const shell = ["Sessions", "Files", "Order cancellation", "Mock data", "waiting for you", "Go to anything"];
  const cases: [string, string[]][] = [
    ["/", ["Workspaces", "shop", "New workspace", "Folder missing"]],
    [`/w/${WS}`, [...shell, "New task", "Write tests", "Phase 2 review, pass 2 of 3", "not initialized", "Overview"]],
    [`/w/${WS}/p/backend`, [...shell, "backend", "Commands", "cargo test --workspace", "Skills", "Browse files"]],
    [`/w/${WS}/p/web`, [...shell, "web", "Initialize web", "Ultracode bootstrap", "Not initialized"]],
    [
      `/w/${WS}/settings`,
      [...shell, "General", "Routing", "Permissions", "Session budget in dollars", "Checked as you edit"],
    ],
    [`/w/${WS}/memory`, [...shell, "OrderStateMachine", "Add a lesson", "Memory"]],
    [`/w/${WS}/cost`, [...shell, "By session", "Cache reads per tool call"]],
    [
      `/w/${WS}/s/${SESSION}`,
      [...shell, "Waiting for you", "Allow once", "Closing gate", "Research", "Phase graph", "Add context"],
    ],
    [`/w/${WS}/s/s_research`, ["Completion report", "Decided for you", "Refund event publishing"]],
    [
      `/w/${WS}/x/x_rev2`,
      [...shell, "Activity", "Denied", "What to do instead", "write-scope", "Code reviewer · Phase 2 · pass 2"],
    ],
    [
      `/w/${WS}/artifact?path=${spec}`,
      ["How to read requirements", "EARS", "order cancellation", "Spec: order cancellation"],
    ],
    [`/w/${WS}/artifact?path=${ledger}`, ["Code Review Ledger", "Iteration 1"]],
    [`/w/${WS}/f/backend/crates/orders/src/service.rs`, [...shell, "service.rs", "CannotCancel", "read-only"]],
    [`/s/${SESSION}`, ["Waiting for you", "Allow once"]],
    ["/nowhere", ["Nothing here"]],
  ];
  for (const [path, expected] of cases) {
    it(path, async () => {
      const text = await render(path);
      for (const e of expected) expect(text, `${path} should contain "${e}"`).toContain(e);
    });
  }

  it("redirects the old projects page to the overview", async () => {
    await render(`/w/${WS}/projects`);
    expect(router!.state.location.pathname).toBe(`/w/${WS}`);
  });
});
