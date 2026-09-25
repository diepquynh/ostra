import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CommandPalette, filterPaletteItems, Menu, type PaletteItem, Tabs, TreeItem } from "./index";

afterEach(cleanup);

const ITEMS: PaletteItem[] = [
  { id: "s1", group: "Sessions", icon: "git-pull-request", label: "Add order cancellation", hint: "waiting" },
  { id: "x1", group: "Executions", icon: "square-terminal", label: "Implementer · Phase 2", hint: "harness:codex" },
  { id: "a1", group: "Artifacts", icon: "file-text", label: "Order spec", hint: "spec.md" },
];

describe("CommandPalette", () => {
  it("filters on label, hint and group, case-insensitively", () => {
    expect(filterPaletteItems(ITEMS, "ORDER").map((i) => i.id)).toEqual(["s1", "a1"]);
    expect(filterPaletteItems(ITEMS, "codex").map((i) => i.id)).toEqual(["x1"]);
    expect(filterPaletteItems(ITEMS, "artifacts").map((i) => i.id)).toEqual(["a1"]);
    expect(filterPaletteItems(ITEMS, "  ")).toBe(ITEMS);
  });

  it("renders nothing while closed", () => {
    render(<CommandPalette open={false} items={ITEMS} />);
    expect(screen.queryByRole("combobox")).toBeNull();
  });

  it("focuses the input, filters as you type and says when nothing matches", () => {
    render(<CommandPalette open items={ITEMS} />);
    const input = screen.getByRole("combobox");
    expect(document.activeElement).toBe(input);
    expect(screen.getAllByRole("option")).toHaveLength(3);
    fireEvent.change(input, { target: { value: "spec" } });
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["Order specspec.md"]);
    fireEvent.change(input, { target: { value: "zzz" } });
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText("Nothing matches “zzz”.")).toBeTruthy();
  });

  it("moves the highlight with arrows, clamps at the ends, and opens with Enter", () => {
    const onSelect = vi.fn();
    const onClose = vi.fn();
    render(<CommandPalette open items={ITEMS} onSelect={onSelect} onClose={onClose} />);
    const input = screen.getByRole("combobox");
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(screen.getAllByRole("option")[0].getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    const options = screen.getAllByRole("option");
    expect(options[2].getAttribute("aria-selected")).toBe("true");
    expect(input.getAttribute("aria-activedescendant")).toBe(options[2].id);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith(ITEMS[2]);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("resets the highlight when the query changes", () => {
    const onSelect = vi.fn();
    render(<CommandPalette open items={ITEMS} onSelect={onSelect} />);
    const input = screen.getByRole("combobox");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.change(input, { target: { value: "order" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith(ITEMS[0]);
  });

  it("closes on Escape and on a scrim click, and returns focus to the opener", () => {
    const onClose = vi.fn();
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const { rerender } = render(<CommandPalette open items={ITEMS} onClose={onClose} />);
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("dialog").parentElement!);
    expect(onClose).toHaveBeenCalledTimes(2);
    rerender(<CommandPalette open={false} items={ITEMS} onClose={onClose} />);
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });
});

function ControlledTabs({ onClose }: { onClose?: (id: string) => void }) {
  const [v, setV] = useState("b");
  return (
    <Tabs
      variant="bar"
      value={v}
      onChange={setV}
      onClose={onClose}
      tabs={[
        { id: "a", label: "Alpha" },
        { id: "b", label: "Beta", count: 2 },
        { id: "c", label: "Gamma" },
      ]}
    />
  );
}

describe("Tabs", () => {
  const selected = () =>
    screen.getAllByRole("tab").find((t) => t.getAttribute("aria-selected") === "true")?.textContent;

  it("selects on click and keeps a single tab stop on the selected tab", () => {
    render(<ControlledTabs />);
    expect(selected()).toBe("Beta2");
    expect(screen.getAllByRole("tab").map((t) => t.tabIndex)).toEqual([-1, 0, -1]);
    fireEvent.click(screen.getByText("Alpha"));
    expect(selected()).toBe("Alpha");
    expect(screen.getAllByRole("tab").map((t) => t.tabIndex)).toEqual([0, -1, -1]);
  });

  it("moves and selects with arrows, wraps, and supports Home and End", () => {
    render(<ControlledTabs />);
    const tabs = screen.getAllByRole("tab");
    fireEvent.keyDown(tabs[1], { key: "ArrowRight" });
    expect(selected()).toBe("Gamma");
    expect(document.activeElement).toBe(tabs[2]);
    fireEvent.keyDown(tabs[2], { key: "ArrowRight" });
    expect(selected()).toBe("Alpha");
    fireEvent.keyDown(tabs[0], { key: "ArrowLeft" });
    expect(selected()).toBe("Gamma");
    fireEvent.keyDown(tabs[2], { key: "Home" });
    expect(selected()).toBe("Alpha");
    fireEvent.keyDown(tabs[0], { key: "End" });
    expect(selected()).toBe("Gamma");
  });

  it("closes a bar tab from its close button or Delete without selecting it", () => {
    const onClose = vi.fn();
    render(<ControlledTabs onClose={onClose} />);
    fireEvent.click(screen.getByRole("button", { name: "Close Alpha" }));
    expect(onClose).toHaveBeenCalledWith("a");
    expect(selected()).toBe("Beta2");
    fireEvent.keyDown(screen.getAllByRole("tab")[2], { key: "Delete" });
    expect(onClose).toHaveBeenCalledWith("c");
  });
});

describe("Menu", () => {
  function setup(onSelect = vi.fn(), onClose = vi.fn()) {
    render(
      <Menu
        open
        onClose={onClose}
        items={[
          { type: "heading", label: "Workspaces" },
          { id: "a", label: "shop", checked: true, onSelect },
          { id: "b", label: "internal-tools", onSelect },
          { type: "divider" },
          { id: "c", label: "New workspace…", onSelect },
        ]}
      />,
    );
    return { onSelect, onClose, items: () => Array.from(document.querySelectorAll<HTMLElement>('[role^="menuitem"]')) };
  }

  it("focuses the first item and moves focus with arrows, Home and End", () => {
    const { items } = setup();
    const [a, b, c] = items();
    expect(items()).toHaveLength(3);
    expect(document.activeElement).toBe(a);
    fireEvent.keyDown(a, { key: "ArrowDown" });
    expect(document.activeElement).toBe(b);
    fireEvent.keyDown(b, { key: "End" });
    expect(document.activeElement).toBe(c);
    fireEvent.keyDown(c, { key: "ArrowDown" });
    expect(document.activeElement).toBe(a);
    fireEvent.keyDown(a, { key: "ArrowUp" });
    expect(document.activeElement).toBe(c);
  });

  it("selects with Enter or click, then closes", () => {
    const { onSelect, onClose, items } = setup();
    fireEvent.keyDown(items()[1], { key: "Enter" });
    expect(onSelect).toHaveBeenLastCalledWith(expect.objectContaining({ id: "b" }));
    fireEvent.click(items()[2]);
    expect(onSelect).toHaveBeenLastCalledWith(expect.objectContaining({ id: "c" }));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("closes on Escape and on an outside click", () => {
    const { onClose, items } = setup();
    fireEvent.keyDown(items()[0], { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    fireEvent.click(document.querySelector(".os-menu-scrim")!);
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("marks the checked item", () => {
    const { items } = setup();
    expect(items()[0].getAttribute("role")).toBe("menuitemradio");
    expect(items()[0].getAttribute("aria-checked")).toBe("true");
  });

  it("renders nothing while closed", () => {
    render(<Menu open={false} items={[{ label: "x" }]} />);
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

describe("TreeItem", () => {
  it("activates with Enter, toggles with the chevron and the arrow keys", () => {
    const onClick = vi.fn();
    const onToggle = vi.fn();
    const { rerender } = render(
      <TreeItem label="Order cancellation" expanded={false} onClick={onClick} onToggle={onToggle} />,
    );
    const row = screen.getByRole("treeitem");
    fireEvent.keyDown(row, { key: "Enter" });
    expect(onClick).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(row, { key: "ArrowRight" });
    expect(onToggle).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(row, { key: "ArrowLeft" });
    expect(onToggle).toHaveBeenCalledTimes(1);
    fireEvent.click(row.querySelector(".os-tree-item__chev")!);
    expect(onToggle).toHaveBeenCalledTimes(2);
    expect(onClick).toHaveBeenCalledTimes(1);
    rerender(<TreeItem label="Order cancellation" expanded onClick={onClick} onToggle={onToggle} />);
    fireEvent.keyDown(screen.getByRole("treeitem"), { key: "ArrowLeft" });
    expect(onToggle).toHaveBeenCalledTimes(3);
    expect(screen.getByRole("treeitem").getAttribute("aria-expanded")).toBe("true");
  });

  it("toggles on row click when it has no onClick", () => {
    const onToggle = vi.fn();
    render(<TreeItem label="Executions" expanded={false} onToggle={onToggle} />);
    fireEvent.click(screen.getByRole("treeitem"));
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it("moves focus between rows of the same tree with Up and Down", () => {
    render(
      <div role="tree">
        <TreeItem label="One" />
        <TreeItem label="Two" />
      </div>,
    );
    const [one, two] = screen.getAllByRole("treeitem");
    one.focus();
    fireEvent.keyDown(one, { key: "ArrowDown" });
    expect(document.activeElement).toBe(two);
    fireEvent.keyDown(two, { key: "ArrowUp" });
    expect(document.activeElement).toBe(one);
  });
});
