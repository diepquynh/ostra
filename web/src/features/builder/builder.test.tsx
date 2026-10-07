import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { routes } from "../../App";
import { api } from "../../api";
import { WS } from "../../api/mock/fixtures";
import { autoLayout } from "./WorkflowBuilder";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

beforeAll(() => {
  // The diagram measures its pane; jsdom has no layout.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

async function mount(path: string) {
  const router = createMemoryRouter(routes, { initialEntries: [path] });
  render(<RouterProvider router={router} />);
  await act(async () => {
    await new Promise((r) => setTimeout(r, 400));
  });
}

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
});

const main = () => document.querySelector("main") as HTMLElement;

describe("workflow builder", () => {
  it("lays out nodes in columns by what they wait for", () => {
    const at = autoLayout([
      { id: "a", after: [] },
      { id: "b", after: ["a"] },
      { id: "c", after: ["a"] },
      { id: "d", after: ["b", "c"] },
    ]);
    expect(at.a[0]).toBe(0);
    expect(at.b[0]).toBe(240);
    expect(at.c).toEqual([240, 120]);
    expect(at.d[0]).toBe(480);
  });

  it("adds a transform after the selected node and saves the graph with its layout", async () => {
    await mount(`/w/${WS}/workflows#implement-with-release-gate`);
    await waitFor(() => expect(main().textContent).toContain("triage"));
    expect(main().textContent).toContain("when high.output not_empty");
    const save = vi.spyOn(api, "saveWorkflow");
    const palette = within(screen.getByLabelText("Node palette"));
    fireEvent.click(palette.getByRole("button", { name: "+ count" }));
    const inspector = within(await screen.findByLabelText("Node settings"));
    fireEvent.change(inspector.getByLabelText("items"), { target: { value: "high.output" } });
    fireEvent.click(within(main()).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    const [, name, file] = save.mock.calls[0];
    expect(name).toBe("implement-with-release-gate");
    const count = file.stage?.find((s) => s.transform === "count");
    expect(count).toMatchObject({ id: "count", inputs: { items: "high.output" } });
    expect(file.layout?.count).toBeDefined();
  });
});

describe("workflows screen", () => {
  it("goes back from the builder to the list", async () => {
    await mount(`/w/${WS}/workflows#implement-with-release-gate`);
    await waitFor(() => expect(main().textContent).toContain("Workflow builder"));
    fireEvent.click(within(main()).getByRole("button", { name: "All workflows" }));
    await waitFor(() => expect(within(main()).getByRole("button", { name: "Open in the builder" })).toBeTruthy());
    expect(main().textContent).not.toContain("Workflow builder");
  });
});

describe("transforms", () => {
  it("builds a composite from steps and saves it", async () => {
    await mount(`/w/${WS}/workflows#transform:risk-files?new`);
    await waitFor(() => expect(within(main()).getByRole("button", { name: "Add step" })).toBeTruthy());
    const save = vi.spyOn(api, "saveTransformFunction");
    fireEvent.click(within(main()).getByRole("button", { name: "Add input" }));
    fireEvent.change(within(main()).getByLabelText("Inputs name"), { target: { value: "findings" } });
    fireEvent.click(within(main()).getByRole("button", { name: "Add step" }));
    fireEvent.change(within(main()).getByLabelText("Step function"), { target: { value: "count" } });
    fireEvent.change(within(main()).getByLabelText("items"), { target: { value: "input.findings" } });
    fireEvent.change(within(main()).getByLabelText("Output step"), { target: { value: "step-1" } });
    fireEvent.click(within(main()).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    const [, name, file] = save.mock.calls[0];
    expect(name).toBe("risk-files");
    expect(file.output).toBe("step-1");
    expect(file.input?.[0].name).toBe("findings");
    expect(file.step?.[0]).toMatchObject({ transform: "count", inputs: { items: "input.findings" } });
  });

  it("shows what a save would be refused for while the graph is edited", async () => {
    await mount(`/w/${WS}/workflows#implement-with-release-gate`);
    await waitFor(() => expect(main().textContent).toContain("triage"));
    const palette = within(screen.getByLabelText("Node palette"));
    expect(palette.getByRole("button", { name: "+ high-files" })).toBeTruthy();
    fireEvent.click(palette.getByRole("button", { name: "+ count" }));
    const inspector = within(await screen.findByLabelText("Node settings"));
    fireEvent.change(inspector.getByLabelText("items"), { target: { value: "nowhere.output" } });
    await waitFor(() => expect(main().textContent).toContain("problem to fix before saving"), { timeout: 3000 });
    expect(main().textContent).toContain("there is no node `nowhere`");
  });
});

describe("plugin workflows", () => {
  it("lists a plugin's workflow and functions and opens the workflow read only", async () => {
    await mount(`/w/${WS}/workflows`);
    await waitFor(() => expect(main().textContent).toContain("plugin gate, read only"));
    expect(main().textContent).toContain("gate:changelog-lines");
    expect(within(main()).queryByRole("button", { name: "gate:changelog-lines" })).toBeNull();
    expect(within(main()).queryByRole("button", { name: "Delete gate:release-check" })).toBeNull();
    fireEvent.click(within(main()).getByRole("button", { name: "gate:release-check" }));
    await waitFor(() => expect(main().textContent).toContain("builds this workflow in code"));
    expect(within(main()).queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.queryByLabelText("Node palette")).toBeNull();
  });
});

describe("agents screen", () => {
  it("edits a workspace agent and keeps Ostra's agents read only", async () => {
    await mount(`/w/${WS}/agents`);
    await waitFor(() => expect(main().textContent).toContain("security-auditor"));
    fireEvent.click(within(main()).getByRole("button", { name: /code-reviewer/ }));
    await waitFor(() => expect(within(main()).getByRole("button", { name: "Duplicate" })).toBeTruthy());
    expect(within(main()).queryByRole("button", { name: "Edit" })).toBeNull();

    fireEvent.click(within(main()).getByRole("button", { name: /security-auditor/ }));
    fireEvent.click(await within(main()).findByRole("button", { name: "Edit" }));
    const save = vi.spyOn(api, "saveAgent");
    fireEvent.change(within(main()).getByLabelText("Description"), {
      target: { value: "Audits secrets and input handling." },
    });
    fireEvent.click(within(main()).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0][1]).toBe("security-auditor");
    expect(save.mock.calls[0][2].description).toBe("Audits secrets and input handling.");
  });
});
