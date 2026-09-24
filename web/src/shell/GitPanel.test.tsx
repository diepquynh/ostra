import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mockGit } from "../api/mock/mockGit";
import type { ProjectView } from "../api/types";
import { GitPanel } from "./GitPanel";

afterEach(cleanup);

function panel(key: string) {
  const onOpenFile = vi.fn();
  render(
    <GitPanel ws="shop" projects={[{ key, init_status: "initialized" } as ProjectView]} project={key} setProject={() => {}} selected={null} onOpenFile={onOpenFile} onAddProject={() => {}} />,
  );
  return onOpenFile;
}

const section = (title: string) => screen.getByText(title).closest("div")!.parentElement!;

describe("the Git panel", () => {
  it("stages a change, commits it, and opens files but not deleted ones", async () => {
    const onOpenFile = panel("git-a");
    await screen.findByText("Staged changes");
    fireEvent.click(screen.getByRole("button", { name: "Stage README.md" }));
    await waitFor(() => expect(within(section("Staged changes")).queryByText("README.md")).not.toBeNull());
    expect(mockGit.status("git-a").staged.map((c) => c.path)).toEqual(["README.md", "src/lib.rs"]);

    fireEvent.click(screen.getByText("lib.rs"));
    expect(onOpenFile).toHaveBeenCalledWith("git-a", "src/lib.rs");
    fireEvent.click(screen.getByText("old_name.rs"));
    expect(onOpenFile).toHaveBeenCalledTimes(1);

    const commit = screen.getByRole("button", { name: "Commit 2 files" });
    expect(commit).toHaveProperty("disabled", true);
    fireEvent.change(screen.getByLabelText("Commit message"), { target: { value: "Update the readme" } });
    fireEvent.click(commit);
    await screen.findByText(/\] Update the readme/);
    expect(screen.queryByText("Staged changes")).toBeNull();
    expect(mockGit.status("git-a").ahead).toBe(1);
  });

  it("creates a branch from the branch menu", async () => {
    panel("git-b");
    fireEvent.click(await screen.findByTitle("Switch or create a branch"));
    fireEvent.click(await screen.findByText("Create a branch…"));
    const field = screen.getByLabelText("New branch name");
    fireEvent.change(field, { target: { value: "feature/x" } });
    fireEvent.keyDown(field, { key: "Enter" });
    await screen.findByText("Created and switched to feature/x.");
    expect(mockGit.status("git-b").branch).toBe("feature/x");
  });
});
