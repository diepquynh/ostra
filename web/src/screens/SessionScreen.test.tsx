import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import { SESSION, WS } from "../api/mock/fixtures";
import { ConsoleContext, type ConsoleContextValue } from "../lib/nav";
import { SessionScreen } from "./SessionScreen";

afterEach(cleanup);

function mount(id: string) {
  const open = vi.fn();
  const noop = () => {};
  const ctx: ConsoleContextValue = {
    nav: { ws: WS, activeId: `session:${id}`, tabs: [], open, close: noop, keep: noop, href: (x) => x },
    shell: {
      openDock: noop,
      closeDock: noop,
      taskDraft: null,
      setTaskDraft: noop,
      newWorkspace: noop,
      addProject: noop,
      runSetup: noop,
      browseFiles: noop,
      theme: "dark",
      toggleTheme: noop,
    },
    workspace: { detail: null, reload: noop },
  };
  const router = createMemoryRouter(
    [
      {
        path: "*",
        element: (
          <ConsoleContext.Provider value={ctx}>
            <SessionScreen ws={WS} id={id} />
          </ConsoleContext.Provider>
        ),
      },
    ],
    { initialEntries: [`/w/${WS}/s/${id}`] },
  );
  render(<RouterProvider router={router} />);
  return { open };
}

const gates = () => within(screen.getByRole("region", { name: "Waiting for you" }));

describe("session board", () => {
  it("renders the demo session: header, lanes, open gates, lane stages, phase graph, and a decisions tab", async () => {
    const { open } = mount(SESSION);
    await screen.findByRole("heading", { name: "Order cancellation" });
    expect(screen.getByText(/^Add order cancellation: customers can cancel an order/)).toBeTruthy();
    expect((screen.getByRole("switch", { name: "YOLO" }) as HTMLInputElement).checked).toBe(false);
    expect(screen.getByRole("button", { name: "Pause" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Stop the session" })).toBeTruthy();
    // Tagged files render as chips, not as their paths.
    expect(screen.getByRole("button", { name: "service.ts" })).toBeTruthy();
    expect(screen.getByRole("list", { name: "Context you added" }).textContent).toContain(
      "Refunds follow the same rule",
    );

    const laneTabs = within(screen.getByRole("tablist", { name: "Pipeline lanes" }));
    expect(laneTabs.getAllByRole("tab")).toHaveLength(9);
    expect(laneTabs.getByRole("tab", { selected: true }).textContent).toContain("Review");
    const views = within(screen.getByRole("tablist", { name: "Session views" }));
    expect(views.getAllByRole("tab").map((t) => t.textContent)).toEqual(["Overview2", "Decisions2"]);
    expect(screen.queryByText("Artifacts")).toBeNull();

    const cards = gates().getAllByRole("region");
    expect(cards.map((c) => c.getAttribute("aria-label"))).toEqual(["Allow a shell command?", "Closing gate for web"]);
    expect(gates().getByRole("button", { name: "Allow once" })).toBeTruthy();
    expect(gates().getByRole("button", { name: /Always in this workspace/ }).textContent).toContain(
      "Bash(./gradlew *)",
    );

    expect(screen.getByText(/You cannot review code you wrote an hour ago/)).toBeTruthy();
    expect(screen.getByText("Review phase 2")).toBeTruthy();
    expect(screen.getByText("Cancellation service and endpoint")).toBeTruthy();
    expect(screen.queryByText(/Ostra chose/, { selector: ".os-decision__text" })).toBeNull();
    expect(screen.getByText("Show answered gates (3)")).toBeTruthy();

    fireEvent.click(screen.getByText("Review phase 2"));
    expect(screen.getByText(/Protects against:/).parentElement?.textContent).toContain(
      "Defects that compound across phases",
    );
    fireEvent.click(screen.getByRole("button", { name: /Code reviewer · Phase 2 · pass 2/ }));
    expect(open).toHaveBeenLastCalledWith("exec:x_rev2");

    fireEvent.click(laneTabs.getByRole("tab", { name: /Research/ }));
    expect(screen.getByText("Explore backend")).toBeTruthy();
    expect(screen.queryByText("Cancellation service and endpoint")).toBeNull();

    fireEvent.click(views.getByRole("tab", { name: /Decisions/ }));
    expect(screen.getAllByText(/Ostra chose/, { selector: ".os-decision__text" })).toHaveLength(2);
    expect(screen.queryByRole("region", { name: "Waiting for you" })).toBeNull();
  });

  it("queues added context with its tagged files", async () => {
    mount(SESSION);
    const amend = vi.spyOn(api, "amend");
    const field = await screen.findByLabelText("Context");
    field.textContent = "Also see @backend/src/a.ts, please";
    fireEvent.input(field);
    fireEvent.click(screen.getByRole("button", { name: /Queue for the next step/ }));
    await waitFor(() => expect(amend).toHaveBeenCalledTimes(1));
    expect(amend.mock.calls[0][1]).toEqual({
      text: "Also see @backend/src/a.ts, please",
      files: [{ project: "backend", path: "src/a.ts" }],
      uploads: [],
      delivery: "queue",
    });
  });

  it("asks before sending context now, because running work is interrupted", async () => {
    mount(SESSION);
    const amend = vi.spyOn(api, "amend");
    const field = await screen.findByLabelText("Context");
    field.textContent = "Stop using the legacy table";
    fireEvent.input(field);
    fireEvent.click(screen.getByRole("button", { name: "Send now" }));
    const dialog = await screen.findByRole("dialog", { name: "Interrupt running work?" });
    expect(amend).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Interrupt and send" }));
    await waitFor(() => expect(amend).toHaveBeenCalledTimes(1));
    expect(amend.mock.calls[0][1]).toMatchObject({ delivery: "now" });
  });

  it("pauses the session from the header", async () => {
    mount(SESSION);
    const pause = vi.spyOn(api, "pauseSession");
    fireEvent.click(await screen.findByRole("button", { name: "Pause" }));
    await waitFor(() => expect(pause).toHaveBeenCalledWith(SESSION));
  });

  it.each([
    ["s_export", "Questions about the requirements", "Send the answers"],
    ["s_guest", "Approve the spec", "Approve the spec"],
    ["s_stock", "Approve the plan", "Approve the plan"],
    ["s_payments", "The spec fact-check keeps failing", "Run another fact-check round"],
    ["s_search", "Review of phase 1 reached its cap", "Run another fix and review pass"],
    ["s_labels", "Phase 2 is stuck", "Re-run with this fact"],
    ["s_labels", "Phase 3 is blocked", "Retry the phase"],
    ["s_receipts", "Claude Code could not run", "Retry after login"],
    ["s_receipts", "Implementer failed", "Retry the execution"],
    ["s_images", "The session reached its budget", "Raise the budget"],
    ["s_init", "Approve the skills", "Approve the skills"],
  ])("%s shows the gate %s with one primary button, %s", async (id, title, primary) => {
    mount(id);
    const card = await screen.findByRole("region", { name: title });
    const primaries = within(card)
      .getAllByRole("button")
      .filter((b) => b.className.includes("os-btn--primary"));
    expect(primaries.map((b) => b.textContent)).toEqual([primary]);
  });

  it("shows a security block with its guidance and no dismiss button", async () => {
    mount("s_search");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Security block in backend, phase 1");
    expect(alert.textContent).toContain("Research parameter binding");
    expect(within(alert).queryAllByRole("button")).toHaveLength(0);
  });

  it("checks the answer before sending it, then records the answer", async () => {
    mount("s_labels");
    const card = await screen.findByRole("region", { name: "Phase 2 is stuck" });
    fireEvent.click(within(card).getByRole("button", { name: "Re-run with this fact" }));
    expect(within(card).getByRole("alert").textContent).toContain("Write the missing fact first");
    fireEvent.change(within(card).getByLabelText(/The missing fact/), { target: { value: "Use carrier SDK v2.8" } });
    fireEvent.click(within(card).getByRole("button", { name: "Re-run with this fact" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Phase 2 is stuck" })).toBeNull(), {
      timeout: 2000,
    });
    fireEvent.click(await screen.findByText(/Show answered gates \(1\)/));
    const answered = await screen.findByRole("region", { name: "Phase 2 is stuck" });
    expect(answered.textContent).toContain("Answered by you");
    expect(answered.textContent).toContain("Re-run with this fact: Use carrier SDK v2.8");
  });

  it("builds the skills answer from the approval table", async () => {
    mount("s_init");
    const card = await screen.findByRole("region", { name: "Approve the skills" });
    expect(within(card).getByText(/4 of 4 kept/)).toBeTruthy();
    fireEvent.change(within(card).getByLabelText("Decision for page-route"), { target: { value: "drop" } });
    expect(within(card).getByText(/3 of 4 kept/)).toBeTruthy();
  });
});
