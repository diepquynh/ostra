import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mockFile, mockTree } from "../api/mock/projectFiles";
import type { ProjectView } from "../api/types";
import { FilesPanel } from "./FilesPanel";

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
