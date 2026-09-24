import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Checkbox, FolderPicker, Input, splitPath, Switch, type FolderLister, type FsEntry } from "./index";

afterEach(cleanup);

describe("Switch", () => {
  function Controlled({ onChange }: { onChange: (v: boolean) => void }) {
    const [on, setOn] = useState(false);
    return (
      <Switch
        label="YOLO"
        tone="warn"
        checked={on}
        onChange={(e) => {
          setOn(e.target.checked);
          onChange(e.target.checked);
        }}
      />
    );
  }

  it("is a switch that follows its checked prop", () => {
    const onChange = vi.fn();
    render(<Controlled onChange={onChange} />);
    const sw = screen.getByRole("switch", { name: "YOLO" }) as HTMLInputElement;
    expect(sw.checked).toBe(false);
    fireEvent.click(sw);
    expect(onChange).toHaveBeenLastCalledWith(true);
    expect(sw.checked).toBe(true);
    fireEvent.click(screen.getByText("YOLO"));
    expect(sw.checked).toBe(false);
    expect(sw.closest("label")?.className).toContain("os-switch--warn");
  });

  it("does not change when controlled without a state update", () => {
    render(<Switch label="Push" checked={false} onChange={() => {}} />);
    const sw = screen.getByRole("switch") as HTMLInputElement;
    fireEvent.click(sw);
    expect(sw.checked).toBe(false);
  });
});

describe("Checkbox", () => {
  it("renders a checkbox with label and description, uncontrolled", () => {
    render(<Checkbox label="Write tests" description="Adds a test stage." defaultChecked />);
    const box = screen.getByRole("checkbox", { name: /Write tests/ }) as HTMLInputElement;
    expect(box.checked).toBe(true);
    fireEvent.click(box);
    expect(box.checked).toBe(false);
    expect(screen.getByText("Adds a test stage.").className).toBe("os-check__desc");
  });

  it("renders radios that behave as a group", () => {
    function Group() {
      const [v, setV] = useState("soft");
      return (
        <>
          <Checkbox radio name="q" label="Soft delete" checked={v === "soft"} onChange={() => setV("soft")} />
          <Checkbox radio name="q" label="Hard delete" checked={v === "hard"} onChange={() => setV("hard")} />
        </>
      );
    }
    render(<Group />);
    const [soft, hard] = screen.getAllByRole("radio") as HTMLInputElement[];
    expect(soft.checked).toBe(true);
    fireEvent.click(hard);
    expect(hard.checked).toBe(true);
    expect(soft.checked).toBe(false);
    expect(hard.closest("label")?.className).toContain("os-check--radio");
  });
});

describe("Input", () => {
  it("links its label and error, and renders a textarea when multiline", () => {
    render(
      <>
        <Input label="Allow rule" defaultValue="Bash(*)" error="Deny beats allow." />
        <Input multiline aria-label="Request" rows={4} />
      </>,
    );
    const field = screen.getByLabelText(/Allow rule/);
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(document.getElementById(field.getAttribute("aria-describedby")!)?.textContent).toBe("Deny beats allow.");
    expect(screen.getByLabelText("Request").tagName).toBe("TEXTAREA");
  });
});

describe("splitPath", () => {
  it("splits the directory from the typed name and expands ~", () => {
    expect(splitPath("/home/me/co", "/home/me")).toEqual({ dir: "/home/me", prefix: "co" });
    expect(splitPath("/home/me/", "/home/me")).toEqual({ dir: "/home/me", prefix: "" });
    expect(splitPath("/us")).toEqual({ dir: "/", prefix: "us" });
    expect(splitPath("~/code/sh", "/home/me")).toEqual({ dir: "/home/me/code", prefix: "sh" });
    expect(splitPath("~/co")).toEqual({ dir: "~", prefix: "co" });
    expect(splitPath("code/sh")).toBeNull();
  });
});

const CODE: FsEntry[] = [
  { name: "shop-backend", is_git: true, is_ostra_project: true },
  { name: "shop-web", is_git: true },
  { name: "workshop" },
  { name: "notes" },
];

describe("FolderPicker, controlled browsing", () => {
  function Harness({ initial, onBrowse, missing }: { initial: string; onBrowse?: (p: string) => void; missing?: boolean }) {
    const [value, setValue] = useState(initial);
    return (
      <>
        <FolderPicker value={value} onChange={setValue} browsePath="/home/me/code" parent="/home/me" entries={CODE} home="/home/me" missing={missing} onBrowse={onBrowse} />
        <output data-testid="value">{value}</output>
      </>
    );
  }
  const input = () => screen.getByLabelText("Folder path");
  const value = () => screen.getByTestId("value").textContent;
  const rows = () => Array.from(document.querySelectorAll(".os-picker__row")).map((r) => r.textContent);

  it("filters by the typed name, starts-with matches first, and Tab completes", () => {
    render(<Harness initial="/home/me/code/shop" />);
    expect(rows()).toEqual(["shop-backendgitOstra project", "shop-webgit", "workshop"]);
    fireEvent.keyDown(input(), { key: "ArrowDown" });
    fireEvent.keyDown(input(), { key: "Tab" });
    expect(value()).toBe("/home/me/code/shop-web/");
  });

  it("browses the directory part of what is typed, with ~ expanded", () => {
    const onBrowse = vi.fn();
    render(<Harness initial="/home/me/code/" onBrowse={onBrowse} />);
    fireEvent.change(input(), { target: { value: "~/notes/x" } });
    expect(onBrowse).toHaveBeenLastCalledWith("/home/me/notes");
  });

  it("uses the listed folder on Enter and goes up on Backspace after a slash", () => {
    render(<Harness initial="/home/me/code/" />);
    expect(rows()[0]).toBe("..");
    fireEvent.keyDown(input(), { key: "Backspace" });
    expect(value()).toBe("/home/me/");
    fireEvent.change(input(), { target: { value: "/home/me/code/" } });
    fireEvent.keyDown(input(), { key: "Enter" });
    expect(value()).toBe("/home/me/code");
  });

  it("opens a row on click and selects it on double-click", () => {
    render(<Harness initial="/home/me/code/" />);
    fireEvent.click(screen.getByText("notes"));
    expect(value()).toBe("/home/me/code/notes/");
    fireEvent.doubleClick(screen.getByText("workshop"));
    expect(value()).toBe("/home/me/code/workshop");
  });

  it("shows an error for a relative path and for a missing folder", () => {
    const { unmount } = render(<Harness initial="code" />);
    expect(screen.getByText("Type an absolute path, starting with / or ~/.")).toBeTruthy();
    unmount();
    render(<Harness initial="/home/me/code/" missing />);
    expect(screen.getByText("No folder at /home/me/code.")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Use this folder" }) as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("FolderPicker, with a list function", () => {
  const FS: Record<string, string[]> = { "/home/me": ["code", "Downloads"], "/home/me/code": ["shop-backend", "shop-web"] };
  const list = vi.fn<FolderLister>(async (raw) => {
    const path = raw.startsWith("~") ? "/home/me" + raw.slice(1) : raw;
    const names = FS[path];
    if (!names) return { path, parent: null, entries: [], exists: false, nearest: "/home/me", home: "/home/me" };
    return { path, parent: "/home", entries: names.map((name) => ({ name, is_dir: true })), home: "/home/me" };
  });

  function Harness({ initial }: { initial: string }) {
    const [value, setValue] = useState(initial);
    return (
      <>
        <FolderPicker value={value} onChange={setValue} list={list} debounceMs={0} />
        <output data-testid="value">{value}</output>
      </>
    );
  }

  it("lists the typed directory, learns home from the listing, and completes", async () => {
    list.mockClear();
    render(<Harness initial="~/co" />);
    await waitFor(() => expect(screen.getByText("de")).toBeTruthy());
    await waitFor(() => expect(list.mock.calls.map((c) => c[0])).toEqual(["~", "/home/me"]));
    fireEvent.keyDown(screen.getByLabelText("Folder path"), { key: "Tab" });
    expect(screen.getByTestId("value").textContent).toBe("/home/me/code/");
    await waitFor(() => expect(screen.getByText("shop-web")).toBeTruthy());
  });

  it("offers the nearest existing folder when the path is missing", async () => {
    render(<Harness initial="/home/me/nope/" />);
    await waitFor(() => expect(screen.getByText("No folder at /home/me/nope.")).toBeTruthy());
    fireEvent.click(screen.getByText("/home/me"));
    expect(screen.getByTestId("value").textContent).toBe("/home/me/");
    await waitFor(() => expect(screen.getByText("Downloads")).toBeTruthy());
  });

  const CREATE = "Create this folder and any missing parent folders";

  it("creates a missing folder with its parents and browses into it", async () => {
    const mkdir = vi.fn(async (path: string) => {
      FS[path] = [];
    });
    function WithMkdir() {
      const [value, setValue] = useState("/home/me/new/deep");
      return (
        <>
          <FolderPicker value={value} onChange={setValue} list={list} mkdir={mkdir} debounceMs={0} />
          <output data-testid="value">{value}</output>
        </>
      );
    }
    render(<WithMkdir />);
    fireEvent.click(await screen.findByTitle(CREATE));
    await waitFor(() => expect(mkdir).toHaveBeenCalledWith("/home/me/new/deep"));
    await waitFor(() => expect(screen.getByTestId("value").textContent).toBe("/home/me/new/deep/"));
    await waitFor(() => expect(screen.getByText("No folders here.")).toBeTruthy());
    expect(screen.queryByTitle(CREATE)).toBeNull();
    delete FS["/home/me/new/deep"];
  });

  it("offers create for a typed name no folder has, and not for an existing one", async () => {
    render(<FolderPicker value="/home/me/code/shop-web" list={list} mkdir={async () => undefined} debounceMs={0} />);
    await waitFor(() => expect(screen.getAllByText("shop-web")).toBeTruthy());
    expect(screen.queryByTitle(CREATE)).toBeNull();
    cleanup();
    render(<FolderPicker value="/home/me/code/shop" list={list} mkdir={async () => undefined} debounceMs={0} />);
    expect((await screen.findByTitle(CREATE)).textContent).toBe("Create /home/me/code/shop");
  });
});
