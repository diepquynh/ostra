import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mockFile, mockTree } from "../api/mock/projectFiles";
import type { ProjectView, SandboxStatus, WorkspaceDetail } from "../api/types";
import { ConsoleContext } from "../lib/nav";
import { FilesPanel, moveTarget } from "./FilesPanel";

afterEach(cleanup);

const projects = [{ key: "backend", init_status: "initialized" } as ProjectView];

function panel(selected: { key: string; path: string } | null = null) {
  const onOpenFile = vi.fn();
  render(
    <FilesPanel
      ws="shop"
      projects={projects}
      project="backend"
      setProject={() => {}}
      selected={selected}
      onOpenFile={onOpenFile}
      onOpenProject={() => {}}
      onAddProject={() => {}}
    />,
  );
  return onOpenFile;
}

const type = (label: string, value: string) => {
  const field = screen.getByLabelText(label);
  fireEvent.change(field, { target: { value } });
  fireEvent.keyDown(field, { key: "Enter" });
};

describe("creating files and folders in the Files panel", () => {
  it("creates an empty file at the top and opens it for editing", async () => {
    const onOpenFile = panel();
    await screen.findByText("migrations");
    fireEvent.click(screen.getByRole("button", { name: "New file" }));
    type("New file name", "NOTES.md");
    await waitFor(() => expect(onOpenFile).toHaveBeenCalledWith("backend", "NOTES.md", { edit: true }));
    expect(mockFile("backend", "NOTES.md").content).toBe("");
    await screen.findByText("NOTES.md");
    expect(screen.queryByLabelText("New file name")).toBeNull();
  });

  it("creates nested folders inside the folder of the open file", async () => {
    panel({ key: "backend", path: "tests/orders_test.rs" });
    await screen.findByText("orders_test.rs");
    fireEvent.click(screen.getByRole("button", { name: "New folder" }));
    type("New folder name", "fixtures/json");
    await screen.findByText("fixtures");
    expect(mockTree("backend", "tests/fixtures").entries.map((e) => e.name)).toEqual(["json"]);
    await screen.findByText("json");
  });

  it("keeps the field and shows why when the name exists", async () => {
    panel();
    await screen.findByText("migrations");
    fireEvent.click(screen.getByRole("button", { name: "New folder" }));
    type("New folder name", "migrations");
    await screen.findByText(/already exists/);
    expect(screen.getByLabelText("New folder name")).toBeTruthy();
  });

  it("cancels on Escape", async () => {
    panel();
    await screen.findByText("migrations");
    fireEvent.click(screen.getByRole("button", { name: "New file" }));
    fireEvent.keyDown(screen.getByLabelText("New file name"), { key: "Escape" });
    expect(screen.queryByLabelText("New file name")).toBeNull();
  });
});

describe("the Artifacts tab", () => {
  function artifacts() {
    const onOpenFile = vi.fn();
    render(
      <FilesPanel
        ws="shop"
        projects={projects}
        project="backend"
        setProject={() => {}}
        selected={null}
        onOpenFile={onOpenFile}
        onOpenProject={() => {}}
        onAddProject={() => {}}
        root="artifacts"
      />,
    );
    return onOpenFile;
  }

  it("lists artifacts with hidden ones marked, and opens one like a file", async () => {
    const onOpenFile = artifacts();
    fireEvent.click(await screen.findByText("notes"));
    expect((await screen.findByText("pricing-draft.md")).closest('[role="treeitem"]')?.getAttribute("title")).toBe(
      "notes/pricing-draft.md · hidden from agents",
    );
    fireEvent.click(await screen.findByText("guides"));
    fireEvent.click(await screen.findByText("style.md"));
    expect(onOpenFile).toHaveBeenCalledWith("_artifacts", "guides/style.md");
  });

  it("hides an artifact and creates a file from the right-click menu", async () => {
    const onOpenFile = artifacts();
    fireEvent.click(await screen.findByText("data"));
    const row = (await screen.findByText("orders-sample.csv")).closest('[role="treeitem"]') as HTMLElement;
    fireEvent.contextMenu(row, { clientX: 20, clientY: 20 });
    fireEvent.click(screen.getByRole("menuitem", { name: "Hide from agents" }));
    await waitFor(() =>
      expect(screen.getByText("orders-sample.csv").closest('[role="treeitem"]')?.getAttribute("title")).toContain(
        "hidden from agents",
      ),
    );
    fireEvent.contextMenu(row, { clientX: 20, clientY: 20 });
    fireEvent.click(screen.getByRole("menuitem", { name: "New file" }));
    type("New file name", "customers.csv");
    await waitFor(() => expect(onOpenFile).toHaveBeenCalledWith("_artifacts", "data/customers.csv", { edit: true }));
  });

  it("moves an artifact dragged onto another folder", async () => {
    artifacts();
    fireEvent.click(await screen.findByText("guides"));
    const file = (await screen.findByText("api-errors.md")).closest('[role="treeitem"]') as HTMLElement;
    const folder = screen.getByText("data").closest('[role="treeitem"]') as HTMLElement;
    const store: Record<string, string> = {};
    const dataTransfer = {
      types: [] as string[],
      setData: (t: string, v: string) => {
        store[t] = v;
        dataTransfer.types.push(t);
      },
      getData: (t: string) => store[t] ?? "",
      effectAllowed: "",
      dropEffect: "",
      files: [],
    };
    fireEvent.dragStart(file, { dataTransfer });
    expect(dataTransfer.effectAllowed).toBe("copyMove");
    fireEvent.dragOver(folder, { dataTransfer });
    fireEvent.drop(folder, { dataTransfer });
    await screen.findByText("api-errors.md");
    expect(mockTree("_artifacts", "data").entries.map((e) => e.name)).toContain("api-errors.md");
    expect(mockTree("_artifacts", "guides").entries.map((e) => e.name)).not.toContain("api-errors.md");
  });
});

describe("hiding a folder of artifacts", () => {
  it("hides the folder as one unit, and shows what it holds only with the folder", async () => {
    render(
      <FilesPanel
        ws="shop"
        projects={projects}
        project="backend"
        setProject={() => {}}
        selected={null}
        onOpenFile={() => {}}
        onOpenProject={() => {}}
        onAddProject={() => {}}
        root="artifacts"
      />,
    );
    const folder = (await screen.findByText("skills")).closest('[role="treeitem"]') as HTMLElement;
    fireEvent.contextMenu(folder, { clientX: 20, clientY: 20 });
    fireEvent.click(screen.getByRole("menuitem", { name: "Hide from agents" }));
    await waitFor(() => expect(folder.getAttribute("title")).toContain("hidden from agents"));
    fireEvent.click(folder);
    const inner = (await screen.findByText("refund-flow")).closest('[role="treeitem"]') as HTMLElement;
    expect(inner.getAttribute("title")).toContain("hidden from agents");
    fireEvent.contextMenu(inner, { clientX: 20, clientY: 20 });
    fireEvent.click(screen.getByRole("menuitem", { name: "Show folder skills to agents" }));
    await waitFor(() => expect(folder.getAttribute("title")).not.toContain("hidden from agents"));
    expect(mockTree("_artifacts", "skills").entries.every((e) => !e.hidden_from_agents)).toBe(true);
  });
});

describe("where a dragged artifact lands", () => {
  it("keeps its name in the target folder and refuses no-op or self moves", () => {
    expect(moveTarget("guides/style.md", "data")).toBe("data/style.md");
    expect(moveTarget("skills/refund-flow/", "")).toBe("refund-flow");
    expect(moveTarget("guides/", "")).toBeNull();
    expect(moveTarget("guides/", "skills")).toBe("skills/guides");
    expect(moveTarget("guides/style.md", "guides")).toBeNull();
    expect(moveTarget("guides/", "guides/inner")).toBeNull();
    expect(moveTarget("guides/", "guides")).toBeNull();
  });
});

const noop = () => {};
const defaultConsole = {
  nav: { ws: "shop", activeId: null, tabs: [], open: noop, close: noop, keep: noop, href: (id: string) => id },
  shell: {
    openDock: noop,
    closeDock: noop,
    taskDraft: null,
    setTaskDraft: noop,
    newWorkspace: noop,
    addProject: noop,
    runSetup: noop,
    browseFiles: noop,
    theme: "dark" as const,
    toggleTheme: noop,
  },
};

describe("the Artifacts tab without a sandbox", () => {
  const withSandbox = (status: Omit<SandboxStatus, "decoys" | "builtin_decoys" | "builtin_hosts">) => {
    const sandbox: SandboxStatus = { decoys: true, builtin_decoys: [], builtin_hosts: [], ...status };
    return render(
      <ConsoleContext.Provider
        value={{
          ...defaultConsole,
          workspace: { detail: { sandbox } as WorkspaceDetail, reload: () => {} },
        }}
      >
        <FilesPanel
          ws="shop"
          projects={projects}
          project="backend"
          setProject={() => {}}
          selected={null}
          onOpenFile={() => {}}
          onOpenProject={() => {}}
          onAddProject={() => {}}
          root="artifacts"
        />
      </ConsoleContext.Provider>,
    );
  };

  it("warns that shell commands can read hidden artifacts when commands run unsandboxed", async () => {
    withSandbox({ mode: "off", available: true, active: false });
    expect(await screen.findByText(/Shell commands can read hidden artifacts here/)).toBeTruthy();
  });

  it("stays quiet when the sandbox is on, or required and missing, because then no command runs unsandboxed", async () => {
    withSandbox({ mode: "auto", available: true, active: true });
    await screen.findByText("guides");
    expect(screen.queryByText(/Shell commands can read hidden artifacts/)).toBeNull();
    cleanup();
    withSandbox({ mode: "required", available: false, active: false, message: "Install bubblewrap" });
    await screen.findByText("guides");
    expect(screen.queryByText(/Shell commands can read hidden artifacts/)).toBeNull();
  });
});
