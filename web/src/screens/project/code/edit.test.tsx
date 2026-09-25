import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, socket } from "../../../api";
import { WS } from "../../../api/mock/fixtures";
import { mockFile, mockSaveFile, mockTexts } from "../../../api/mock/projectFiles";
import type { WireMsg } from "../../../api/socket";
import { ConsoleContext, type ConsoleContextValue } from "../../../lib/nav";
import { FileScreen } from "../../FileScreen";

// Monaco needs a real layout engine; a textarea stands in for it.
vi.mock("./FileEditor", () => ({
  default: ({ value, onChange, readOnly }: { value: string; onChange: (v: string) => void; readOnly: boolean }) => (
    <textarea data-testid="editor" value={value} readOnly={readOnly} onChange={(e) => onChange(e.target.value)} />
  ),
}));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const KEY = "backend";
type Handler = (m: WireMsg) => void;
let handlers: Map<string, Set<Handler>>;
let root: Root | null = null;
let host: HTMLDivElement | null = null;

beforeEach(() => {
  handlers = new Map();
  vi.spyOn(socket(), "subscribe").mockImplementation((channel: string, h: Handler) => {
    if (!handlers.has(channel)) handlers.set(channel, new Set());
    handlers.get(channel)!.add(h);
    return () => handlers.get(channel)?.delete(h);
  });
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  vi.restoreAllMocks();
});

async function settle(rounds = 4) {
  for (let i = 0; i < rounds; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
  }
}

function withShell(node: ReactNode, url = "/") {
  const noop = () => {};
  const ctx: ConsoleContextValue = {
    nav: { ws: WS, activeId: null, tabs: [], open: noop, close: noop, keep: noop, href: (id) => id },
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
  return (
    <MemoryRouter initialEntries={[url]}>
      <ConsoleContext.Provider value={ctx}>{node}</ConsoleContext.Provider>
    </MemoryRouter>
  );
}

async function open(path: string, url = "/") {
  host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => {
    root = createRoot(host!);
    root.render(withShell(<FileScreen ws={WS} projectKey={KEY} path={path} />, url));
  });
  await settle();
}

const button = (label: string) =>
  Array.from(host!.querySelectorAll("button")).find(
    (b) => b.getAttribute("aria-label") === label || b.textContent?.replace("⌘S", "").trim() === label,
  ) as HTMLButtonElement | undefined;
const click = (label: string) => act(async () => button(label)!.click());
const editor = () => host!.querySelector<HTMLTextAreaElement>('[data-testid="editor"]')!;
const text = () => host?.textContent ?? "";

async function type(value: string) {
  const el = editor();
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

const diskChanged = (path: string) =>
  act(() =>
    handlers
      .get(`workspace:${WS}`)
      ?.forEach((h) => h({ type: "project_fs_changed", workspace: WS, key: KEY, paths: [path] } as WireMsg)),
  );

// Each test edits its own file, because the mock keeps saved text in memory. Files without git marks open in the
// File view, where Edit is offered.
const paths = mockTexts(KEY)
  .map((t) => t.path)
  .filter((p) => mockFile(KEY, p).git === null);

describe("file edit mode", () => {
  it("saves with the hash the edit started from and keeps editing on the new hash", async () => {
    const path = paths[0];
    const start = mockFile(KEY, path).hash;
    const save = vi.spyOn(api, "saveProjectFile");
    await open(path);
    expect(editor().readOnly).toBe(true);
    expect(editor().value).toBe(mockFile(KEY, path).content);
    await click("Edit this file");
    await settle(2);
    expect(editor().readOnly).toBe(false);
    expect(editor().value).toBe(mockFile(KEY, path).content);
    expect(button("Save")!.disabled).toBe(true);
    await type("fn changed() {}\n");
    expect(text()).toContain("unsaved changes");
    await click("Save");
    await settle();
    expect(save).toHaveBeenCalledWith(WS, KEY, { path, content: "fn changed() {}\n", base_hash: start });
    expect(mockFile(KEY, path).content).toBe("fn changed() {}\n");
    expect(text()).not.toContain("unsaved changes");
    expect(text()).toContain("editing");

    await type("fn again() {}\n");
    await click("Save");
    await settle();
    expect(save.mock.calls[1][2]).toEqual({ path, content: "fn again() {}\n", base_hash: expect.any(String) });
    expect(save.mock.calls[1][2].base_hash).not.toBe(start);
    expect(text()).not.toContain("changed on disk");
  });

  it("shows the conflict on a 409 and overwrites with the latest hash", async () => {
    const path = paths[1];
    const save = vi.spyOn(api, "saveProjectFile");
    await open(path);
    await click("Edit this file");
    await settle(2);
    await type("mine\n");
    // Someone else writes the file, and no push arrives before the save.
    const theirs = mockSaveFile(KEY, path, "theirs\n", mockFile(KEY, path).hash);
    await click("Save");
    await settle();
    expect(text()).toContain("This file changed on disk after you started editing.");
    expect(editor().value).toBe("mine\n");
    expect(button("Save")!.disabled).toBe(true);
    await click("Overwrite");
    await settle();
    expect(save).toHaveBeenLastCalledWith(WS, KEY, { path, content: "mine\n", base_hash: theirs.hash });
    expect(mockFile(KEY, path).content).toBe("mine\n");
    expect(text()).not.toContain("changed on disk");
  });

  it("flags a disk change while the draft is dirty, keeps the draft, and reloads on request", async () => {
    const path = paths[2];
    await open(path);
    await click("Edit this file");
    await settle(2);
    await type("draft\n");
    mockSaveFile(KEY, path, "from an agent\n", mockFile(KEY, path).hash);
    diskChanged(path);
    await settle();
    expect(text()).toContain("This file changed on disk after you started editing.");
    expect(editor().value).toBe("draft\n");
    await click("Reload");
    await settle();
    expect(editor().value).toBe("from an agent\n");
    expect(text()).not.toContain("changed on disk");
  });

  it("follows the disk silently while the draft is clean, and asks before discarding changes", async () => {
    const path = paths[3];
    await open(path);
    await click("Edit this file");
    await settle(2);
    mockSaveFile(KEY, path, "newer\n", mockFile(KEY, path).hash);
    diskChanged(path);
    await settle();
    expect(editor().value).toBe("newer\n");
    expect(text()).not.toContain("changed on disk");

    await type("edited\n");
    await click("Discard");
    expect(button("Discard changes?")).toBeTruthy();
    expect(editor()).toBeTruthy();
    await click("Discard changes?");
    await settle(1);
    expect(editor().readOnly).toBe(true);
    expect(editor().value).toBe("newer\n");
    expect(mockFile(KEY, path).content).toBe("newer\n");
  });

  it("opens a file created in the Files panel straight into edit mode", async () => {
    mockSaveFile(KEY, "new-from-panel.rs", "", null);
    await open("new-from-panel.rs", "/w/x#edit");
    expect(editor()).toBeTruthy();
    expect(editor().value).toBe("");
  });
});
