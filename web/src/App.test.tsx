import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, describe, expect, it } from "vitest";
import { routes } from "./App";
import { SESSION, WS } from "./api/mock/fixtures";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let host: HTMLDivElement | null = null;

async function render(path: string): Promise<string> {
  host = document.createElement("div");
  document.body.appendChild(host);
  const router = createMemoryRouter(routes, { initialEntries: [path] });
  await act(async () => {
    root = createRoot(host!);
    root.render(<RouterProvider router={router} />);
  });
  // Mock responses resolve after 80 ms; wait for them and the renders they trigger.
  for (let i = 0; i < 6; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
  }
  return host.textContent ?? "";
}

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

const spec = encodeURIComponent(`/home/me/code/shop/.ostra/sessions/${SESSION}/ostra-spec-20260922-100700-order-cancel.md`);
const ledger = encodeURIComponent(`/home/me/code/shop/.ostra/sessions/${SESSION}/backend/ostra-review-ledger-phase-1.md`);

describe("every screen renders on mock data", () => {
  const cases: [string, string[]][] = [
    ["/", ["Workspaces", "shop", "New workspace"]],
    [`/w/${WS}`, ["New task", "Sessions", "Add order cancellation", "not initialized"]],
    [`/w/${WS}/projects`, ["backend", "Initialize", "Ultracode bootstrap", "Harness"]],
    [`/w/${WS}/settings`, ["Routing", "By phase complexity", "Permissions", "Save settings"]],
    [`/w/${WS}/memory`, ["OrderStateMachine", "Add a lesson"]],
    [`/w/${WS}/cost`, ["By session", "Cache reads per tool call"]],
    [`/w/${WS}/s/${SESSION}`, ["Waiting for you", "Allow once", "Closing gate", "Research", "Build lane: phase graph", "Ostra chose"]],
    [`/w/${WS}/s/s_research`, ["Completion report", "Decided for you"]],
    [`/w/${WS}/x/x_rev2`, ["Activity", "Denied", "What to do instead", "write-scope"]],
    [`/w/${WS}/artifact?path=${spec}`, ["How to read requirements", "EARS", "order cancellation"]],
    [`/w/${WS}/artifact?path=${ledger}`, ["Code Review Ledger", "Iteration 1"]],
    [`/s/${SESSION}`, ["Waiting for you"]],
    ["/nowhere", ["Nothing here"]],
  ];
  for (const [path, expected] of cases) {
    it(path, async () => {
      const text = await render(path);
      for (const e of expected) expect(text, `${path} should contain "${e}"`).toContain(e);
    });
  }
});
