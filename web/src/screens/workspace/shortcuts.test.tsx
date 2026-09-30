import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, describe, expect, it } from "vitest";
import { routes } from "../../App";
import { WS } from "../../api/mock/fixtures";
import { SEQUENCE_MS } from "../../lib/shortcuts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

async function mount() {
  const router = createMemoryRouter(routes, { initialEntries: [`/w/${WS}/settings#setting:shortcuts`] });
  render(<RouterProvider router={router} />);
  await act(async () => {
    await new Promise((r) => setTimeout(r, 400));
  });
  await waitFor(() =>
    expect(screen.getByRole("tab", { name: "Shortcuts" }).getAttribute("aria-selected")).toBe("true"),
  );
}

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const stored = () => JSON.parse(localStorage.getItem(`ostra.shortcuts.${WS}`) ?? "null")?.bindings ?? null;
const row = (label: string) =>
  Array.from(document.querySelectorAll<HTMLElement>("main tbody tr")).find((tr) => tr.textContent?.startsWith(label))!;
const mac = navigator.platform.includes("Mac");

async function record(label: string, ...presses: [string, KeyboardEventInit?][]) {
  fireEvent.click(within(row(label)).getByRole("button", { name: /^(Set|Change)$/ }));
  const box = await screen.findByRole("textbox", { name: `Press the new shortcut for ${label}` });
  expect(document.activeElement).toBe(box);
  for (const [key, init] of presses) fireEvent.keyDown(box, { key, ...init });
  if (presses.length === 1)
    await act(async () => {
      await new Promise((r) => setTimeout(r, SEQUENCE_MS));
    });
}

describe("shortcut settings", () => {
  it("records a combo and a two-stroke sequence into this workspace's storage", async () => {
    await mount();
    await record("Toggle light and dark theme", ["F9"]);
    await waitFor(() => expect(stored()).toEqual({ theme: ["F9"] }));
    expect(row("Toggle light and dark theme").textContent).toContain("F9");

    await record("Open settings", ["j", { ctrlKey: true }], ["s"]);
    expect(stored()).toEqual({ theme: ["F9"], settings: ["ctrl+j", "s"] });

    fireEvent.click(screen.getByRole("button", { name: "Reset all" }));
    expect(localStorage.getItem(`ostra.shortcuts.${WS}`)).toBeNull();
  });

  it("asks before taking a combo another action uses, and unbinds it there", async () => {
    await mount();
    const mod = mac ? { metaKey: true } : { ctrlKey: true };
    await record("Open the command palette", ["b", mod]);
    expect(stored()).toBeNull();
    const banner = await screen.findByText(/is used by Toggle the sidebar/);
    fireEvent.click(within(banner.closest(".os-banner") as HTMLElement).getByRole("button", { name: "Use it here" }));
    expect(stored()).toEqual({ palette: [mac ? "meta+b" : "ctrl+b"], sidebar: null });
    expect(row("Toggle the sidebar").textContent).toContain("Not set");

    fireEvent.click(within(row("Toggle the sidebar")).getByRole("button", { name: "Reset" }));
    expect(stored()).toEqual({ palette: [mac ? "meta+b" : "ctrl+b"] });
  });

  it("warns about a combo the browser keeps and cancels on Escape", async () => {
    await mount();
    await record("New task", ["t", mac ? { metaKey: true } : { ctrlKey: true }]);
    expect(await screen.findByText(/The browser may keep/)).not.toBeNull();

    fireEvent.click(within(row("Open settings")).getByRole("button", { name: "Set" }));
    fireEvent.keyDown(await screen.findByRole("textbox", { name: /Press the new shortcut/ }), { key: "Escape" });
    expect(screen.queryByRole("textbox", { name: /Press the new shortcut/ })).toBeNull();
    expect(stored()).toEqual({ "new-task": [mac ? "meta+t" : "ctrl+t"] });
  });

  it("notes the browser's limits and offers to install Ostra as an app", async () => {
    await mount();
    expect(screen.getByText(/cannot override the browser's built-in shortcuts/)).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Install Ostra as an app" }));
    await waitFor(() =>
      expect(screen.getByText(/HTTPS address or localhost|install icon|Firefox|Safari/)).not.toBeNull(),
    );
  });
});
