import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api, HttpError } from "../../api";
import type { ProviderStatus } from "../../api/types";
import { ProviderCredentials } from "./ProviderCredentials";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const provider = (over: Partial<ProviderStatus> = {}): ProviderStatus => ({
  name: "anthropic",
  has_key: false,
  source: "none",
  base_url: null,
  base_url_source: "default",
  saved: { base_url: null, has_api_key: false, has_auth_token: false },
  ...over,
});

describe("ProviderCredentials", () => {
  it("saves one kind of secret and clears the other", async () => {
    const saved = provider({
      has_key: true,
      source: "saved",
      saved: { base_url: "https://gw.example", has_api_key: false, has_auth_token: true },
    });
    const spy = vi.spyOn(api, "saveProvider").mockResolvedValue(saved);
    const onSaved = vi.fn();
    render(<ProviderCredentials providers={[provider()]} onSaved={onSaved} />);
    fireEvent.change(screen.getByLabelText("Base URL for Anthropic"), { target: { value: "https://gw.example" } });
    fireEvent.change(screen.getByLabelText("Key type for Anthropic"), { target: { value: "auth_token" } });
    fireEvent.change(screen.getByLabelText("Auth token for Anthropic"), { target: { value: "tok" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(saved));
    expect(spy).toHaveBeenCalledWith("anthropic", { base_url: "https://gw.example", auth_token: "tok", api_key: "" });
    expect((screen.getByLabelText("Auth token for Anthropic") as HTMLInputElement).value).toBe("");
  });

  it("says when the environment shadows a saved key, and shows field issues", async () => {
    const p = provider({
      has_key: true,
      source: "env:ANTHROPIC_API_KEY",
      saved: { base_url: null, has_api_key: true, has_auth_token: false },
    });
    vi.spyOn(api, "saveProvider").mockRejectedValue(
      new HttpError(422, "bad", [{ path: "base_url", message: "Enter the base URL as http:// or https://." }]),
    );
    render(<ProviderCredentials providers={[p]} onSaved={() => {}} />);
    expect(screen.getByText(/ANTHROPIC_API_KEY is set in the environment/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Base URL for Anthropic"), { target: { value: "gw" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(screen.getByText("Enter the base URL as http:// or https://.")).toBeTruthy());
  });
});
