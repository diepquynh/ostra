import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createMemoryRouter, MemoryRouter, RouterProvider } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { routes } from "../../App";
import { api } from "../../api";
import { WS } from "../../api/mock/fixtures";
import { ConsoleContext, type ConsoleContextValue } from "../../lib/nav";
import { WorkspaceScreen } from "../WorkspaceScreen";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let router: ReturnType<typeof createMemoryRouter>;

async function mount(path: string) {
  router = createMemoryRouter(routes, { initialEntries: [path] });
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
const rowTexts = () => Array.from(main().querySelectorAll("tbody tr")).map((tr) => tr.textContent ?? "");

describe("memory screen", () => {
  it("adds, edits and deletes a lesson against the mock server", async () => {
    await mount(`/w/${WS}/memory?project=web`);
    await waitFor(() => expect(rowTexts().some((t) => t.includes("CSRF header"))).toBe(true));
    expect(rowTexts().some((t) => t.includes("OrderStateMachine"))).toBe(false);

    fireEvent.click(within(main()).getAllByRole("button", { name: /Add a lesson/ })[0]);
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText(/^Area/), { target: { value: "src/routes" } });
    fireEvent.change(within(dialog).getByLabelText(/^Lesson/), {
      target: { value: "Routes load lazily, so a new page needs an import() entry." },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save the lesson" }));
    await waitFor(() => expect(rowTexts().some((t) => t.includes("Routes load lazily"))).toBe(true));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(main().querySelector(".os-row--selected")?.textContent).toContain("Routes load lazily");

    const added = Array.from(main().querySelectorAll("tbody tr")).find((tr) =>
      tr.textContent?.includes("Routes load lazily"),
    ) as HTMLElement;
    fireEvent.click(within(added).getByRole("button", { name: "Edit this lesson" }));
    const edit = await screen.findByRole("dialog");
    expect((within(edit).getByLabelText(/^Area/) as HTMLInputElement).value).toBe("src/routes");
    fireEvent.change(within(edit).getByLabelText(/^Lesson/), {
      target: { value: "Routes load lazily; add an import() entry per page." },
    });
    fireEvent.click(within(edit).getByRole("button", { name: "Save the lesson" }));
    await waitFor(() => expect(rowTexts().some((t) => t.includes("add an import() entry per page"))).toBe(true));

    const edited = Array.from(main().querySelectorAll("tbody tr")).find((tr) =>
      tr.textContent?.includes("per page"),
    ) as HTMLElement;
    fireEvent.click(within(edited).getByRole("button", { name: "Delete this lesson" }));
    const confirm = await screen.findByRole("dialog");
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete the lesson" }));
    await waitFor(() => expect(rowTexts().some((t) => t.includes("per page"))).toBe(false));
    expect(rowTexts().some((t) => t.includes("CSRF header"))).toBe(true);
  });

  it("selects the lesson a deep link names and searches within the project", async () => {
    await mount(`/w/${WS}/memory#lesson:backend:4`);
    await waitFor(() => expect(main().querySelector(".os-row--selected")?.textContent).toContain("idempotent"));

    fireEvent.change(within(main()).getByLabelText("Search lessons"), { target: { value: "docker" } });
    await waitFor(() => expect(rowTexts()).toHaveLength(1));
    expect(rowTexts()[0]).toContain("Testcontainers");
    fireEvent.change(within(main()).getByLabelText("Search lessons"), { target: { value: "no such lesson" } });
    await waitFor(() => expect(main().textContent).toContain("No lessons match"));
  });
});

describe("skills screen", () => {
  const skillRows = () => Array.from(main().querySelectorAll(".sk-row")).map((b) => b.textContent ?? "");

  it("creates, edits and deletes a skill against the mock server", async () => {
    await mount(`/w/${WS}/skills?project=backend`);
    await waitFor(() => expect(skillRows().some((t) => t.includes("axum-handler"))).toBe(true));

    fireEvent.click(within(main()).getByRole("button", { name: /New skill/ }));
    fireEvent.change(await within(main()).findByLabelText(/^Name/), { target: { value: "migration" } });
    expect((within(main()).getByLabelText(/^SKILL\.md/) as HTMLTextAreaElement).value).toContain("name: migration");
    fireEvent.click(within(main()).getByRole("button", { name: "Create the skill" }));
    await waitFor(() => expect(skillRows().some((t) => t.includes("migration"))).toBe(true));
    expect(main().querySelector(".sk-row[data-active]")?.textContent).toContain("migration");

    const editor = () => within(main()).getByLabelText(/^SKILL\.md/) as HTMLTextAreaElement;
    await waitFor(() => expect(editor().value).toContain("# migration"));
    expect((within(main()).getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(editor(), { target: { value: `${editor().value}\nRun sqlx migrate.\n` } });
    fireEvent.click(within(main()).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect((within(main()).getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true),
    );

    fireEvent.click(within(main()).getByRole("button", { name: /Delete/ }));
    const confirm = await screen.findByRole("dialog");
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete the skill" }));
    await waitFor(() => expect(skillRows().some((t) => t.includes("migration"))).toBe(false));
  });
});

describe("settings screen", () => {
  it("opens the tab a setting deep link names and rings the field", async () => {
    await mount(`/w/${WS}/settings#setting:permissions.deny`);
    await waitFor(() =>
      expect(screen.getByRole("tab", { name: "Permissions" }).getAttribute("aria-selected")).toBe("true"),
    );
    await waitFor(() =>
      expect(document.getElementById("setting:permissions.deny")?.classList.contains("wp-flash")).toBe(true),
    );
  });

  it("validates as you edit, shows issues on the field, and saves", async () => {
    const validate = vi.spyOn(api, "validateSettings");
    const save = vi.spyOn(api, "saveSettings");
    await mount(`/w/${WS}/settings`);
    const name = within(main()).getByLabelText("Name") as HTMLInputElement;
    expect(name.value).toBe("shop");
    expect(screen.getByRole("status", { name: "" }).textContent).toBe("Checked as you edit");

    fireEvent.change(name, { target: { value: "" } });
    await waitFor(() => expect(validate).toHaveBeenCalled());
    await waitFor(() => expect(main().textContent).toContain("The workspace needs a name."));
    expect(name.getAttribute("aria-invalid")).toBe("true");
    expect((within(main()).getByRole("button", { name: /Save/ }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.change(name, { target: { value: "shop-renamed" } });
    await waitFor(() => expect(main().textContent).not.toContain("The workspace needs a name."));
    await waitFor(() =>
      expect((within(main()).getByRole("button", { name: /Save/ }) as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(within(main()).getByRole("button", { name: /Save/ }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0][1].name).toBe("shop-renamed");
    await waitFor(() => expect(main().textContent).toContain("Saved to .ostra/workspace.toml"));

    // Put the mock back for the other tests.
    fireEvent.change(within(main()).getByLabelText("Name"), { target: { value: "shop" } });
    await waitFor(() =>
      expect((within(main()).getByRole("button", { name: /Save/ }) as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(within(main()).getByRole("button", { name: /Save/ }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  });

  it("maps 422 issues from save to their fields and tabs", async () => {
    vi.spyOn(api, "validateSettings").mockResolvedValue([]);
    await mount(`/w/${WS}/settings`);
    fireEvent.click(screen.getByRole("tab", { name: "Routing" }));
    const executor = within(main()).getByLabelText("Executor for plan") as HTMLSelectElement;
    fireEvent.change(executor, { target: { value: "harness:agy" } });
    await waitFor(() =>
      expect((within(main()).getByRole("button", { name: /Save/ }) as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(within(main()).getByRole("button", { name: /Save/ }));
    await waitFor(() => expect(main().textContent).toContain("routes to harness:agy, which is not installed"));
    const cell = document.getElementById("setting:routing.executor.byAgent.plan")!;
    expect(cell.textContent).toContain("not installed");
    expect(screen.getByRole("tab", { name: /Routing/ }).textContent).toContain("1");
  });

  it("shows what Agent default resolves to, the server's stacks, and the global rules", async () => {
    await mount(`/w/${WS}/settings`);
    fireEvent.click(screen.getByRole("tab", { name: "Routing" }));
    const model = within(main()).getByLabelText("Model for plan") as HTMLSelectElement;
    const labels = Array.from(model.options).map((o) => o.textContent);
    expect(labels).toContain("Agent default (advanced)");
    fireEvent.change(model, { target: { value: "default" } });
    await waitFor(() =>
      expect(document.getElementById("setting:routing.model.byAgent.plan")?.textContent).toContain(
        "anthropic:claude-opus-5-5",
      ),
    );
    const effort = within(main()).getByLabelText("Effort for quick-answer") as HTMLSelectElement;
    expect(effort.options[0].textContent).toBe("Agent default (medium)");

    fireEvent.click(screen.getByRole("tab", { name: "Projects" }));
    const stack = within(main()).getByLabelText("Stack of backend") as HTMLSelectElement;
    expect(Array.from(stack.options).map((o) => o.value)).toEqual([
      "",
      "go",
      "java-spring",
      "python",
      "typescript-node",
    ]);

    fireEvent.click(screen.getByRole("tab", { name: "Permissions" }));
    expect(main().textContent).toContain("read-only, from ~/.config/ostra/config.toml");
    expect(main().textContent).toContain("Bash(rm -rf /*)");
  });
});

describe("cost screen", () => {
  it("shows the status bar's week by default and all time on request", async () => {
    const cost = vi.spyOn(api, "cost");
    await mount(`/w/${WS}/cost`);
    await waitFor(() => expect(main().querySelector(".wp-stats")?.textContent).toContain("This week$4.18"));
    expect(cost).toHaveBeenLastCalledWith(WS, "2026-09-16T00:00:00Z");
    fireEvent.click(screen.getByRole("tab", { name: "All time" }));
    await waitFor(() => expect(main().querySelector(".wp-stats")?.textContent).toContain("All time$5.02"));
    expect(cost).toHaveBeenLastCalledWith(WS, null);
  });
});

describe("workspace screen", () => {
  it("takes the quick dock's task draft once, then starts a session and opens it", async () => {
    const setTaskDraft = vi.fn();
    const open = vi.fn();
    const detail = await api.workspace(WS);
    const base = { ...detail, projects: detail.projects.map((p) => ({ ...p, init_status: "initialized" as const })) };
    const ctx = (draft: string | null): ConsoleContextValue => ({
      nav: { ws: WS, activeId: "ws:overview", tabs: [], open, close: () => {}, keep: () => {}, href: (id) => id },
      shell: {
        openDock: () => {},
        closeDock: () => {},
        taskDraft: draft,
        setTaskDraft,
        newWorkspace: () => {},
        addProject: () => {},
        runSetup: () => {},
        browseFiles: () => {},
        theme: "dark",
        toggleTheme: () => {},
      },
      workspace: { detail: base, reload: () => {} },
    });
    const view = (draft: string | null) => (
      <MemoryRouter>
        <ConsoleContext.Provider value={ctx(draft)}>
          <WorkspaceScreen ws={WS} />
        </ConsoleContext.Provider>
      </MemoryRouter>
    );
    const { rerender } = render(view("Why does checkout retry twice?"));
    const request = screen.getByLabelText("Request") as HTMLTextAreaElement;
    await waitFor(() => expect(request.value).toBe("Why does checkout retry twice?"));
    expect(setTaskDraft).toHaveBeenCalledWith(null);
    rerender(view(null));
    expect(request.value).toBe("Why does checkout retry twice?");

    // Two initialized projects show the pin buttons; pinning one sends it with the request.
    const create = vi.spyOn(api, "createSession");
    fireEvent.click(screen.getByRole("button", { name: "web" }));
    fireEvent.keyDown(request, { key: "Enter", metaKey: true, ctrlKey: true });
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(create.mock.calls[0][1]).toEqual({
      request: "Why does checkout retry twice?",
      options: { tests: false, docs: false, yolo: false },
      projects: ["web"],
    });
    await waitFor(() => expect(open).toHaveBeenCalledWith("session:s_new"));
    expect(request.value).toBe("");
  });

  it("filters the sessions table and opens a row in a preview tab", async () => {
    await mount(`/w/${WS}`);
    await waitFor(() => expect(rowTexts()).toHaveLength(13));
    expect(rowTexts()[0]).toContain("Order cancellation");

    fireEvent.change(within(main()).getByLabelText("Filter sessions"), { target: { value: "webhook" } });
    await waitFor(() => expect(rowTexts()).toHaveLength(1));
    fireEvent.click(within(main()).getByRole("tab", { name: /Completed/ }));
    await waitFor(() => expect(main().textContent).toContain("No sessions match these filters."));
    fireEvent.click(within(main()).getByRole("button", { name: /Clear filters/ }));
    await waitFor(() => expect(rowTexts()).toHaveLength(13));

    fireEvent.click(main().querySelectorAll("tbody tr")[1]);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/w/${WS}/s/s_refund`));
  });
});
