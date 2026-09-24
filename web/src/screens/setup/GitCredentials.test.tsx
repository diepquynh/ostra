import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "../../api";
import { GitCredentials } from "./GitCredentials";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const KEY = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----\n";

describe("GitCredentials", () => {
  it("imports a private key file and saves it as an ssh credential", async () => {
    vi.spyOn(api, "gitCredentials").mockResolvedValue([]);
    const create = vi
      .spyOn(api, "createGitCredential")
      .mockResolvedValue([{ id: "gc_1", label: "github.com", host: "github.com", kind: "ssh", username: null, has_secret: true }]);
    render(<GitCredentials />);
    expect(screen.getByText(/stay on the machine that runs Ostra/)).toBeTruthy();
    fireEvent.change(screen.getByRole("combobox", { name: "Type" }), { target: { value: "ssh" } });
    fireEvent.change(screen.getByRole("textbox", { name: /^Host/ }), { target: { value: "github.com" } });

    const file = new File([KEY], "id_ed25519");
    fireEvent.change(screen.getByLabelText("Private key file"), { target: { files: [file] } });
    await waitFor(() => expect(screen.getByRole("textbox", { name: /^Private key/ })).toHaveProperty("value", KEY));

    fireEvent.click(screen.getByRole("button", { name: "Add credential" }));
    await waitFor(() => expect(create).toHaveBeenCalled());
    expect(create.mock.calls[0][0]).toMatchObject({ kind: "ssh", host: "github.com", secret: KEY });
    await screen.findByText("SSH key");
  });

  it("refuses a file too large to be a key", async () => {
    vi.spyOn(api, "gitCredentials").mockResolvedValue([]);
    render(<GitCredentials />);
    fireEvent.change(screen.getByRole("combobox", { name: "Type" }), { target: { value: "ssh" } });
    const big = new File(["x".repeat(70 * 1024)], "backup.tar");
    fireEvent.change(screen.getByLabelText("Private key file"), { target: { files: [big] } });
    await screen.findByText(/backup.tar is too large for a private key/);
    expect(screen.getByRole("textbox", { name: /^Private key/ })).toHaveProperty("value", "");
  });
});
