import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, HttpError } from "../../api";
import { httpApi } from "../../api/client";
import { workspaceDetail } from "../../api/mock/fixtures";
import type { FsBrowse } from "../../api/types";
import { FolderPicker } from "../../design";
import { ConsoleContext, type ConsoleContextValue } from "../../lib/nav";
import { AddProjectDialog } from "./AddProjectDialog";
import { makeLister } from "./folders";
import { NewWorkspaceDialog } from "./NewWorkspaceDialog";

vi.mock("../execution/XtermScreen", () => ({
  default: ({ execution }: { execution: string }) => (
    <div data-testid="xterm" className="ex-xterm" tabIndex={0}>
      {execution}
    </div>
  ),
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const browse = (path: string, names: string[], exists = true): FsBrowse => ({
  path,
  parent: path === "/" ? null : path.split("/").slice(0, -1).join("/") || "/",
  home: "/home/me",
  exists,
  readable: exists,
  nearest: exists ? path : "/home/me",
  entries: names.map((name) => ({ name, is_dir: true, is_git: name.startsWith("shop"), is_ostra_project: false })),
  truncated: false,
  is_git: false,
  is_ostra_project: false,
});

describe("FolderPicker over GET /api/fs", () => {
  function Harness({ start }: { start: string }) {
    const [v, setV] = useState(start);
    return <FolderPicker value={v} onChange={setV} list={makeLister(httpApi.fsBrowse)} debounceMs={10} />;
  }

  it("lists the typed folder, aborts the superseded request, and marks git folders", async () => {
    const signals: AbortSignal[] = [];
    const urls: string[] = [];
    const fetchMock = vi.fn((url: string, init: RequestInit) => {
      urls.push(url);
      signals.push(init.signal!);
      const path = new URL(url, "http://x").searchParams.get("path")!;
      const body =
        path === "/home/me/code"
          ? browse(path, ["billing-service", "shop-backend"])
          : path === "/nope"
            ? browse(path, [], false)
            : browse(path, ["code", "notes"]);
      return Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));
    });
    vi.stubGlobal("fetch", fetchMock);
    render(<Harness start="/home/me/" />);
    await screen.findByText("notes");
    expect(urls[0]).toBe("/api/fs?path=%2Fhome%2Fme");

    const input = screen.getByRole("textbox", { name: "Folder path" });
    fireEvent.change(input, { target: { value: "/home/me/code/" } });
    await screen.findByText("shop-backend");
    expect(screen.getByText("billing-service")).toBeTruthy();
    expect(screen.getAllByText("git")).toHaveLength(1);

    fireEvent.change(input, { target: { value: "/home/me/code/sh" } });
    await waitFor(() => expect(screen.queryByText("billing-service")).toBeNull());
    expect(screen.getByText("op-backend")).toBeTruthy();

    fireEvent.change(input, { target: { value: "/nope/" } });
    fireEvent.change(input, { target: { value: "/home/me/" } });
    await screen.findByText("notes");
    expect(urls.some((u) => u.includes("nope"))).toBe(false);

    fireEvent.change(input, { target: { value: "/nope/" } });
    await screen.findByText("No folder at /nope.");
    expect(screen.getByRole("button", { name: "Use this folder" })).toHaveProperty("disabled", true);
    expect(signals.every((s) => s instanceof AbortSignal)).toBe(true);
  });

  it("aborts an in-flight listing when the path changes", async () => {
    const aborted: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(
        (url: string, init: RequestInit) =>
          new Promise<Response>((resolve, reject) => {
            const path = new URL(url, "http://x").searchParams.get("path")!;
            init.signal!.addEventListener("abort", () => {
              aborted.push(path);
              reject(new DOMException("aborted", "AbortError"));
            });
            setTimeout(() => resolve(new Response(JSON.stringify(browse(path, ["a"])))), path === "/slow" ? 500 : 5);
          }),
      ),
    );
    render(<Harness start="/slow/" />);
    await new Promise((r) => setTimeout(r, 50));
    fireEvent.change(screen.getByRole("textbox", { name: "Folder path" }), { target: { value: "/fast/" } });
    await waitFor(() => expect(aborted).toEqual(["/slow"]));
  });
});

const ctx = (open = vi.fn()): ConsoleContextValue =>
  ({
    nav: {
      ws: workspaceDetail.id,
      activeId: null,
      tabs: [],
      open,
      close: vi.fn(),
      pin: vi.fn(),
      href: (id: string) => id,
    },
    shell: {},
    workspace: { detail: workspaceDetail, reload: vi.fn() },
  }) as unknown as ConsoleContextValue;

function Where() {
  const l = useLocation();
  return <div data-testid="where">{l.pathname}</div>;
}

const type = (label: string | RegExp, value: string) =>
  fireEvent.change(screen.getByRole("textbox", { name: label }), { target: { value } });
const click = (name: string | RegExp) => fireEvent.click(screen.getByRole("button", { name }));

describe("New workspace dialog", () => {
  function renderDialog(onClose = vi.fn()) {
    render(
      <MemoryRouter initialEntries={["/w/ws_demo"]}>
        <Routes>
          <Route
            path="*"
            element={
              <>
                <NewWorkspaceDialog onClose={onClose} />
                <Where />
              </>
            }
          />
        </Routes>
      </MemoryRouter>,
    );
    return onClose;
  }

  it("installs a missing harness and logs in an installed one in a terminal", async () => {
    const setup = vi.spyOn(api, "harnessSetup");
    const onClose = renderDialog();
    await screen.findByText("Antigravity");
    // The rows reveal their results one by one.
    expect(await screen.findAllByRole("button", { name: "Install" }, { timeout: 4000 })).toHaveLength(1);
    await act(async () => click("Install"));
    expect(setup).toHaveBeenLastCalledWith("agy", "install");
    expect(await screen.findByText("Install Antigravity")).toBeTruthy();
    expect((await screen.findByTestId("xterm")).textContent).toBe("setup_agy_install");
    // Escape belongs to the CLI in the terminal, so the dialog stays open.
    fireEvent.keyDown(screen.getByTestId("xterm"), { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    click("Maximize the terminal");
    expect(screen.getByTestId("xterm").closest(".ex-term-frame")?.hasAttribute("data-maximized")).toBe(true);
    fireEvent.keyDown(screen.getByTestId("xterm"), { key: "Escape", shiftKey: true });
    expect(screen.getByTestId("xterm").closest(".ex-term-frame")?.hasAttribute("data-maximized")).toBe(false);
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => click("Log in"));
    expect(setup).toHaveBeenLastCalledWith("grok", "login");
    expect(await screen.findByText("Log in to Grok Build")).toBeTruthy();
    click("Close the terminal");
    expect(screen.queryByTestId("xterm")).toBeNull();
  });

  it("queues clones in the projects step and clones them after the workspace is created", async () => {
    const created = vi.spyOn(api, "createWorkspace");
    const clone = vi
      .spyOn(api, "cloneProject")
      .mockImplementationOnce(async () => workspaceDetail)
      .mockRejectedValueOnce(new HttpError(502, "fatal: Authentication failed"));
    renderDialog();
    await screen.findByText("Anthropic");
    click("Continue");
    type("Folder path", "/home/me/code/shop-three");
    await waitFor(() =>
      expect((screen.getByRole("textbox", { name: /^Name/ }) as HTMLInputElement).value).toBe("shop-three"),
    );
    click("Continue");

    await screen.findByText("Import a folder");
    fireEvent.click(screen.getByRole("tab", { name: /Clone from git/ }));
    for (const url of ["https://github.com/acme/shop-api.git", "https://github.com/acme/private.git"]) {
      type(/^Repository URL/, url);
      await waitFor(() =>
        expect(screen.getByRole("button", { name: "Add to the list" })).toHaveProperty("disabled", false),
      );
      click("Add to the list");
    }
    await screen.findByRole("button", { name: "Remove private" });
    expect(screen.getByRole("textbox", { name: /^Repository URL/ })).toHaveProperty("value", "");
    // A queued key is taken, so the same repository cannot be queued twice.
    type(/^Repository URL/, "https://github.com/acme/shop-api.git");
    expect(screen.getByRole("button", { name: "Add to the list" })).toHaveProperty("disabled", true);
    click("Continue");
    click("Continue");

    await screen.findByText("The settings pass validation.");
    expect(document.querySelector(".os-code")!.textContent).toContain('path = "/home/me/code/shop-three/shop-api"');
    click("Create workspace");
    expect(await screen.findByText("Clone shop-api")).toBeTruthy();
    await screen.findByText("Workspace ready", undefined, { timeout: 5000 });
    expect(created.mock.calls[0][0].projects).toEqual([]);
    expect(clone.mock.calls.map((c) => [c[0], c[1].key])).toEqual([
      ["ws_new", "shop-api"],
      ["ws_new", "private"],
    ]);
    expect(screen.getByText("fatal: Authentication failed")).toBeTruthy();
    expect(screen.getByText("private was not cloned")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Open workspace" })).toBeTruthy();
  }, 15000);

  it("walks the steps, maps a server issue back to its step, then creates the workspace", async () => {
    const onClose = renderDialog();
    await screen.findByText("Check this machine", { selector: ".os-dialog__sub *, h2" });
    await screen.findByText("Anthropic");
    click("Continue");

    // Name and folder: the name follows the chosen folder.
    expect(screen.getByRole("button", { name: "Continue" })).toHaveProperty("disabled", true);
    type("Folder path", "/home/me/code/shop");
    await waitFor(() =>
      expect((screen.getByRole("textbox", { name: /^Name/ }) as HTMLInputElement).value).toBe("shop"),
    );
    click("Continue");

    // Add projects: the key comes from the folder name.
    await screen.findByText("Import a folder");
    type("Folder path", "/home/me/code/billing-service");
    await waitFor(() =>
      expect((screen.getByRole("textbox", { name: /^Project key/ }) as HTMLInputElement).value).toBe("billing-service"),
    );
    await waitFor(() => expect(screen.getByRole("button", { name: "Add project" })).toHaveProperty("disabled", false));
    click("Add project");
    await screen.findByRole("button", { name: "Remove billing-service" });
    click("Continue");

    // Defaults: presets for the logged-in harnesses.
    await screen.findByText("Where implementers run");
    fireEvent.click(screen.getByRole("tab", { name: "Implementers on Codex" }));
    click("Continue");

    // Review: /home/me/code/shop is already registered, so the issue links back to the name step.
    await screen.findByText("That folder is already a registered workspace.");
    expect(document.querySelector(".os-code")!.textContent).toContain('implementer = "harness:codex"');
    expect(screen.getByRole("button", { name: "Create workspace" })).toHaveProperty("disabled", true);
    click(/Go to Name and folder/);
    await screen.findByRole("textbox", { name: /^Name/ });
    expect(screen.getByRole("alert").textContent).toBe("That folder is already a registered workspace.");

    type("Folder path", "/home/me/code/shop-two");
    await waitFor(() =>
      expect((screen.getByRole("textbox", { name: /^Name/ }) as HTMLInputElement).value).toBe("shop-two"),
    );
    click("Continue");
    click("Continue");
    click("Continue");
    await screen.findByText("The settings pass validation.");
    click("Create workspace");
    await screen.findByText("Creating the workspace");
    expect(screen.getByText("Import billing-service")).toBeTruthy();
    await screen.findByText("Workspace ready", undefined, { timeout: 5000 });
    click("Open workspace");
    expect(onClose).toHaveBeenCalled();
    expect(screen.getByTestId("where").textContent).toBe("/w/ws_new");
  }, 15000);
});

describe("Add project dialog", () => {
  it("derives the key, shows key errors inline, and imports", async () => {
    const onAdded = vi.fn();
    render(
      <ConsoleContext.Provider value={ctx()}>
        <AddProjectDialog ws="ws_demo" onClose={vi.fn()} onAdded={onAdded} />
      </ConsoleContext.Provider>,
    );
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/to shop\./)).toBeTruthy();
    type("Folder path", "/home/me/code/shop-backend");
    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: /^Project key/ })).toHaveProperty("value", "shop-backend"),
    );
    // The folder is already the workspace's backend project.
    await screen.findByText("This folder is already added as backend.");
    expect(screen.getByRole("button", { name: "Import project" })).toHaveProperty("disabled", true);

    type("Folder path", "/home/me/code/shop-admin");
    type(/^Project key/, "Admin UI");
    await screen.findByText(/Use lowercase letters/);
    type(/^Project key/, "web");
    await screen.findByText("This key is already used in the workspace.");
    type(/^Project key/, "admin");
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Import project" })).toHaveProperty("disabled", false),
    );
    await act(async () => click("Import project"));
    await waitFor(() => expect(onAdded).toHaveBeenCalled());
    expect(onAdded.mock.calls[0][1]).toBe("admin");
    expect(onAdded.mock.calls[0][0].projects.some((p: { key: string }) => p.key === "admin")).toBe(true);
  });

  it("places the server's 422 issues on the key and stack fields", async () => {
    const issues = [
      { path: "key", message: "A project named `admin` already exists in this workspace. Choose another key." },
      { path: "stack", message: "`Go Lang` is not a stack name." },
    ];
    vi.spyOn(api, "importProject").mockRejectedValueOnce(
      new HttpError(422, "Fix these problems and import the project again.", issues),
    );
    render(
      <ConsoleContext.Provider value={ctx()}>
        <AddProjectDialog ws="ws_demo" onClose={vi.fn()} onAdded={vi.fn()} />
      </ConsoleContext.Provider>,
    );
    type("Folder path", "/home/me/code/shop-admin");
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Import project" })).toHaveProperty("disabled", false),
    );
    await act(async () => click("Import project"));
    await screen.findByText(/already exists in this workspace/);
    expect(screen.getByRole("textbox", { name: /^Project key/ }).getAttribute("aria-invalid")).toBe("true");
    expect(screen.getByText("`Go Lang` is not a stack name.").getAttribute("role")).toBe("alert");
    expect(screen.queryByText("Fix these problems and import the project again.")).toBeNull();
  });

  it("clones from git with the key from the URL and places the server's issues on their fields", async () => {
    vi.spyOn(api, "gitCredentials").mockResolvedValue([
      { id: "gc_1", label: "Work", host: "github.com", kind: "https", username: null, has_secret: true },
    ]);
    const clone = vi
      .spyOn(api, "cloneProject")
      .mockRejectedValueOnce(new HttpError(422, "x", [{ path: "url", message: "Ostra does not clone file:// URLs." }]))
      .mockRejectedValueOnce(new HttpError(502, "fatal: Authentication failed"));
    render(
      <ConsoleContext.Provider value={ctx()}>
        <AddProjectDialog ws="ws_demo" onClose={vi.fn()} onAdded={vi.fn()} />
      </ConsoleContext.Provider>,
    );
    fireEvent.click(screen.getByRole("tab", { name: /Clone from git/ }));
    type(/^Repository URL/, "https://github.com/acme/shop-api.git");
    expect(screen.getByRole("textbox", { name: /^Project key/ })).toHaveProperty("value", "shop-api");
    await screen.findByRole("option", { name: /Work/ });
    fireEvent.change(screen.getByRole("combobox", { name: /^Credential/ }), { target: { value: "gc_1" } });
    await act(async () => click("Clone and import"));
    expect(clone.mock.calls[0][1]).toMatchObject({
      url: "https://github.com/acme/shop-api.git",
      key: "shop-api",
      credential: "gc_1",
    });
    await screen.findByText("Ostra does not clone file:// URLs.");
    expect(screen.getByRole("textbox", { name: /^Repository URL/ }).getAttribute("aria-invalid")).toBe("true");
    await act(async () => click("Clone and import"));
    await screen.findByText("fatal: Authentication failed");
  });
});
