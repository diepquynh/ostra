import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, HttpError } from "../../api";
import type { Commands } from "../../api/gen/Commands";
import { CommandsPanel, commandErrors } from "./CommandsPanel";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const commands: Commands = {
  build: "cargo build",
  test: "cargo test",
  test_one: null,
  format: "cargo fmt",
  lint: null,
  typecheck: null,
  run: null,
};

describe("project commands", () => {
  it("places 422 issues on their fields and keeps other messages general", () => {
    const e = new HttpError(422, "Fix the commands and save again.", [
      { path: "commands.build", message: "One line." },
      { path: "other", message: "Something else." },
    ]);
    expect(commandErrors(e)).toEqual({ fields: { build: "One line." }, general: "Something else." });
    expect(commandErrors(new Error("offline"))).toEqual({ fields: {}, general: "offline" });
  });

  it("edits, saves the draft, and refetches", async () => {
    const save = vi.spyOn(api, "saveProjectCommands").mockResolvedValue({ ...commands, lint: "cargo clippy" });
    const onSaved = vi.fn();
    render(<CommandsPanel ws="w" projectKey="app" commands={commands} onSaved={onSaved} />);
    expect(screen.getByText("cargo build")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.change(screen.getByLabelText("Lint"), { target: { value: "cargo clippy" } });
    fireEvent.click(screen.getByRole("button", { name: "Save commands" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    expect(save).toHaveBeenCalledWith("w", "app", { ...commands, lint: "cargo clippy" });
    expect(screen.queryByRole("button", { name: "Save commands" })).toBeNull();
  });

  it("shows a refused field under its input and stays in edit mode", async () => {
    vi.spyOn(api, "saveProjectCommands").mockRejectedValue(
      new HttpError(422, "Fix the commands and save again.", [
        { path: "commands.build", message: "Write the command on one line." },
      ]),
    );
    const onSaved = vi.fn();
    render(<CommandsPanel ws="w" projectKey="app" commands={commands} onSaved={onSaved} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Save commands" }));
    expect(await screen.findByText("Write the command on one line.")).toBeTruthy();
    expect(onSaved).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Save commands" })).toBeTruthy();
  });

  it("cancels without saving, and offers to add commands when there are none", () => {
    const save = vi.spyOn(api, "saveProjectCommands");
    const empty: Commands = {
      build: null,
      test: null,
      test_one: null,
      format: null,
      lint: null,
      typecheck: null,
      run: null,
    };
    render(<CommandsPanel ws="w" projectKey="app" commands={empty} onSaved={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Add commands" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(save).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Add commands" })).toBeTruthy();
  });
});
