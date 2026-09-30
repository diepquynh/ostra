import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, describe, expect, it } from "vitest";
import { routes } from "../App";
import { SESSION, WS } from "../api/mock/fixtures";

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

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  localStorage.clear();
});

const tabs = () =>
  Array.from(document.querySelectorAll<HTMLElement>('[aria-label="Open tabs"] [role="tab"]')).map((t) => {
    const italic = Array.from(t.querySelectorAll("span")).some((s) => s.style.fontStyle === "italic");
    const text = t.textContent ?? "";
    return italic ? `(${text})` : text;
  });

const row = (label: string) =>
  Array.from(document.querySelectorAll<HTMLElement>('[role="treeitem"]')).find((r) =>
    r.textContent?.startsWith(label),
  )!;

async function click(el: Element, detail = 1) {
  await act(async () => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, detail }));
  });
  await settle(2);
}

async function dblclick(el: Element) {
  await act(async () => {
    el.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  });
  await settle(1);
}

async function key(k: string, mods: KeyboardEventInit = {}) {
  await act(async () => {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...mods }));
  });
  await settle(2);
}

describe("workspace shell", () => {
  it("opens the tab the URL names and shows it in the breadcrumbs", async () => {
    await mount(`/w/${WS}/s/${SESSION}`);
    expect(tabs()).toEqual(["Order cancellation"]);
    expect(document.querySelector(".os-crumbs")?.textContent).toBe("shopOrder cancellation");
  });

  it("single-click opens a preview tab, the next single-click replaces it, and a double-click pins it", async () => {
    await mount(`/w/${WS}/s/${SESSION}`);
    await click(row("Explore"));
    expect(router.state.location.pathname).toBe(`/w/${WS}/x/x_exp1`);
    expect(tabs()).toEqual(["Order cancellation", "(Explore · Research)"]);

    await click(row("Generate spec"));
    expect(router.state.location.pathname).toBe(`/w/${WS}/x/x_spec`);
    expect(tabs()).toEqual(["Order cancellation", "(Generate spec · Spec v1)"]);

    await dblclick(row("Generate spec"));
    expect(tabs()).toEqual(["Order cancellation", "Generate spec · Spec v1"]);

    await click(row("Plan"));
    expect(tabs()).toEqual(["Order cancellation", "Generate spec · Spec v1", "(Plan · Plan)"]);
  });

  it("closing the active tab focuses its neighbour and updates the URL", async () => {
    await mount(`/w/${WS}/s/${SESSION}`);
    await click(row("Explore"));
    await dblclick(row("Explore"));
    const close = document.querySelectorAll('[aria-label="Open tabs"] .os-tab__close')[1];
    await click(close);
    expect(tabs()).toEqual(["Order cancellation"]);
    expect(router.state.location.pathname).toBe(`/w/${WS}/s/${SESSION}`);
  });

  it("pins, unpins and bulk-closes tabs from the tab context menu", async () => {
    await mount(`/w/${WS}/cost`);
    for (const page of ["settings", `s/${SESSION}`]) {
      await act(async () => {
        await router.navigate(`/w/${WS}/${page}`);
      });
      await settle(2);
    }
    expect(tabs()).toEqual(["Cost", "Settings", "Order cancellation"]);
    const tab = (label: string) =>
      Array.from(document.querySelectorAll('[aria-label="Open tabs"] [role="tab"]')).find(
        (t) => t.textContent === label,
      )!;
    const menu = async (label: string, action: string) => {
      await act(async () => {
        tab(label).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 10, clientY: 10 }));
      });
      const items = Array.from(document.querySelectorAll('[aria-label="Tab actions"] [role="menuitem"]'));
      await click(items.find((i) => i.textContent?.startsWith(action))!);
    };

    await menu("Order cancellation", "Pin");
    expect(tabs()).toEqual(["Order cancellation", "Cost", "Settings"]);
    expect(document.querySelector('[aria-label="Unpin Order cancellation"]')).not.toBeNull();

    await menu("Settings", "Close all");
    expect(tabs()).toEqual(["Order cancellation"]);
    expect(router.state.location.pathname).toBe(`/w/${WS}/s/${SESSION}`);

    await click(document.querySelector('[aria-label="Unpin Order cancellation"]')!);
    expect(document.querySelector('[aria-label="Unpin Order cancellation"]')).toBeNull();
    expect(JSON.parse(localStorage.getItem(`ostra.ui.${WS}`) ?? "{}").tabs[0].pinned).toBe(false);
  });

  it("follows browser history to a tab that was closed", async () => {
    await mount(`/w/${WS}/cost`);
    await act(async () => {
      await router.navigate(`/w/${WS}/settings`);
    });
    await settle(2);
    expect(tabs()).toEqual(["Cost", "Settings"]);
    await act(async () => {
      await router.navigate(-1);
    });
    await settle(2);
    expect(router.state.location.pathname).toBe(`/w/${WS}/cost`);
    expect(document.querySelector('[aria-label="Open tabs"] [aria-selected="true"]')?.textContent).toBe("Cost");
  });

  it("restores tabs from localStorage and persists changes there", async () => {
    await mount(`/w/${WS}/cost`);
    await click(row("Order cancellation"));
    const saved = JSON.parse(localStorage.getItem(`ostra.ui.${WS}`)!);
    expect(saved.tabs.map((t: { id: string }) => t.id)).toEqual(["ws:cost", `session:${SESSION}`]);
    expect(saved.active).toBe(`session:${SESSION}`);
    act(() => root?.unmount());
    host?.remove();

    await mount(`/w/${WS}`);
    expect(tabs()).toEqual(["Cost", "(Order cancellation)", "Overview"]);
  });

  it("toggles the sidebar, the Files tab and the quick-question dock from the keyboard", async () => {
    await mount(`/w/${WS}/s/${SESSION}`);
    const mod = navigator.platform.includes("Mac") ? { metaKey: true } : { ctrlKey: true };
    await key("E", { ...mod, shiftKey: true });
    expect(document.querySelector('[aria-label="Find a file"]')).not.toBeNull();
    await key("b", mod);
    expect(document.querySelector('[aria-label="Left dock"]')).toBeNull();
    await key("/", mod);
    expect(document.querySelector('aside[aria-label="Quick question"]')?.textContent).toContain(
      "This session's artifacts are included.",
    );
    await key("k", mod);
    expect(document.querySelector('[aria-label="Command palette"]')).not.toBeNull();
  });

  it("runs the workspace's own shortcuts, sequences included, and leaves typed text alone", async () => {
    localStorage.setItem(
      `ostra.shortcuts.${WS}`,
      JSON.stringify({
        v: 1,
        bindings: { palette: ["alt+p"], sidebar: null, settings: ["ctrl+j", "s"], theme: ["t"] },
      }),
    );
    localStorage.setItem("ostra.shortcuts.ws_other", JSON.stringify({ v: 1, bindings: { theme: ["alt+p"] } }));
    await mount(`/w/${WS}/s/${SESSION}`);
    const mod = navigator.platform.includes("Mac") ? { metaKey: true } : { ctrlKey: true };
    await key("b", mod);
    expect(document.querySelector('[aria-label="Left dock"]')).not.toBeNull();
    await key("k", mod);
    expect(document.querySelector('[aria-label="Command palette"]')).toBeNull();
    await key("p", { altKey: true, code: "KeyP" });
    expect(document.querySelector('[aria-label="Command palette"]')).not.toBeNull();
    await key("Escape");

    const theme = document.documentElement.dataset.theme;
    const box = document.createElement("input");
    document.body.appendChild(box);
    await act(async () => {
      box.dispatchEvent(new KeyboardEvent("keydown", { key: "t", bubbles: true }));
    });
    expect(document.documentElement.dataset.theme).toBe(theme);
    box.remove();
    await key("t");
    expect(document.documentElement.dataset.theme).not.toBe(theme);

    await key("j", { ctrlKey: true });
    await key("s");
    expect(router.state.location.pathname).toBe(`/w/${WS}/settings`);
  });

  it("opens a file from the Files tab in a preview tab", async () => {
    await mount(`/w/${WS}/s/${SESSION}`);
    await click(document.querySelectorAll('[aria-label="Left dock"] [role="tab"]')[1]);
    await settle(2);
    await click(row("README.md"));
    expect(router.state.location.pathname).toBe(`/w/${WS}/f/backend/README.md`);
    expect(tabs()).toContain("(README.md)");
    expect(document.body.textContent).toContain("Changed by sessions");
  });
});
