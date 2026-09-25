import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "../../api";
import type { ContextFile } from "../../api/types";
import { ConsoleContext, type ConsoleContextValue } from "../../lib/nav";
import { FileTagInput } from "./FileTagInput";

afterEach(cleanup);

function Harness({ onValue, initial = "" }: { onValue: (v: string, f: ContextFile[]) => void; initial?: string }) {
  const [value, setValue] = useState(initial);
  const [files, setFiles] = useState<ContextFile[]>([]);
  onValue(value, files);
  return (
    <FileTagInput ws="ws" projects={["api"]} label="Request" value={value} onChange={setValue} onFiles={setFiles} />
  );
}

function mount(initial = "") {
  const noop = () => {};
  const ctx = {
    nav: { ws: "ws", activeId: null, tabs: [], open: noop, close: noop, keep: noop, href: (x: string) => x },
    shell: {},
    workspace: { detail: null, reload: noop },
  } as unknown as ConsoleContextValue;
  const seen = { value: "", files: [] as ContextFile[] };
  render(
    <ConsoleContext.Provider value={ctx}>
      <Harness
        initial={initial}
        onValue={(v, f) => {
          seen.value = v;
          seen.files = f;
        }}
      />
    </ConsoleContext.Provider>,
  );
  return { seen, editor: screen.getByLabelText("Request") };
}

/** Type `text` at the end of the editor, with the caret after it. */
function type(editor: HTMLElement, text: string) {
  const node = document.createTextNode(text);
  editor.appendChild(node);
  const r = document.createRange();
  r.setStart(node, text.length);
  r.collapse(true);
  const sel = window.getSelection()!;
  sel.removeAllRanges();
  sel.addRange(r);
  fireEvent.input(editor);
}

describe("file tag input", () => {
  it("puts a picked file in the text as a chip and keeps the tag in the value", async () => {
    vi.spyOn(api, "projectFiles").mockResolvedValue({ paths: ["src/orders.ts", "src/other.ts"], truncated: false });
    const { seen, editor } = mount();
    type(editor, "see @ord");
    const option = await screen.findByRole("option", { name: /orders\.ts/ });
    act(() => {
      fireEvent.mouseDown(option);
    });
    expect(seen.value).toBe("see @api/src/orders.ts ");
    await waitFor(() => expect(seen.files).toEqual([{ project: "api", path: "src/orders.ts" }]));
    const chip = editor.querySelector(".ctx-chip");
    expect(chip?.textContent).toContain("orders.ts");
    expect(editor.textContent).not.toContain("@api/src/orders.ts");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(screen.getByRole("note").textContent).toContain("what each file is for");
  });

  it("shows the note hint only once something is attached", () => {
    mount("plain text");
    expect(screen.queryByRole("note")).toBeNull();
  });

  it("draws tags in a value set from outside as chips, and removes one with its button", () => {
    const { seen, editor } = mount("fix @api/src/a.ts now");
    expect(editor.querySelectorAll(".ctx-chip")).toHaveLength(1);
    expect(editor.textContent).toContain("fix ");
    fireEvent.click(screen.getByRole("button", { name: "Remove @api/src/a.ts" }));
    expect(seen.value).toBe("fix  now");
  });

  it("offers folders and tags a Files panel row dropped on it", async () => {
    vi.spyOn(api, "projectFiles").mockResolvedValue({ paths: ["src/orders/list.ts"], truncated: false });
    const { seen, editor } = mount();
    type(editor, "in @orde");
    const folder = await screen.findByRole("option", { name: /^orders\// });
    act(() => {
      fireEvent.mouseDown(folder);
    });
    expect(seen.value).toBe("in @api/src/orders/ ");

    const data: Record<string, string> = {
      "application/x-ostra-context": JSON.stringify({ project: "api", path: "src/orders/list.ts" }),
    };
    const dataTransfer = { types: Object.keys(data), getData: (t: string) => data[t] ?? "", dropEffect: "" };
    fireEvent.dragOver(editor, { dataTransfer });
    fireEvent.drop(editor, { dataTransfer });
    expect(seen.value).toContain("@api/src/orders/list.ts");
    expect(editor.querySelectorAll(".ctx-chip")).toHaveLength(2);

    // A row from a project the field does not accept is ignored.
    data["application/x-ostra-context"] = JSON.stringify({ project: "web", path: "main.ts" });
    fireEvent.drop(editor, { dataTransfer });
    expect(seen.value).not.toContain("@web/");
  });
});
