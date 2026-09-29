import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { settings, workspaceDetail } from "../../api/mock/fixtures";
import type { GlobalSandbox, SandboxStatus, ValidationIssue, WorkspaceSettings } from "../../api/types";
import { DECOY_GUIDE } from "../../content/agents";
import { PermissionsSection } from "./SettingsSections";
import { fromForm, type SettingsForm, toForm } from "./settingsForm";

afterEach(cleanup);

function mount({
  sandbox = workspaceDetail.sandbox,
  globalSandbox = workspaceDetail.global_sandbox,
  saved = {},
  issues = [],
}: {
  sandbox?: SandboxStatus;
  globalSandbox?: GlobalSandbox;
  saved?: Partial<WorkspaceSettings>;
  issues?: ValidationIssue[];
} = {}) {
  const out: { form: SettingsForm | null } = { form: null };
  function Harness() {
    const [form, setForm] = useState(() => toForm({ ...settings, ...saved }));
    out.form = form;
    return (
      <PermissionsSection
        form={form}
        update={(fn) =>
          setForm((f) => {
            const next = structuredClone(f);
            fn(next);
            return next;
          })
        }
        issues={(field) => issues.filter((i) => i.path.startsWith(field))}
        global={{ allow: [], ask: [], deny: [] }}
        sandbox={sandbox}
        savedSandbox={null}
        globalSandbox={globalSandbox}
        globalEnforcement="disabled"
      />
    );
  }
  render(<Harness />);
  return out;
}

const hostsField = () => screen.getByLabelText(/^This workspace.s hosts/) as HTMLTextAreaElement;

describe("decoy files panel", () => {
  const decoys = () => screen.getByLabelText(/^This workspace.s decoys/) as HTMLTextAreaElement;

  it("lists the built-in decoys as fixed and edits the workspace's own", () => {
    mount({ saved: { sandbox_decoys: ["~/.aws/credentials"] } });
    expect(decoys().disabled).toBe(false);
    expect(decoys().value).toBe("~/.aws/credentials");
    expect(screen.getByText("~/.ssh/id_rsa")).toBeTruthy();
    expect(screen.queryByText(/cannot plant them/)).toBeNull();
  });

  it("is disabled with a link to the sandboxing guide where the server cannot plant decoys", () => {
    mount({ sandbox: { ...workspaceDetail.sandbox, backend: "seatbelt", decoys: false } });
    expect(decoys().disabled).toBe(true);
    expect(screen.getByText(/cannot plant them/)).toBeTruthy();
    const link = screen.getByRole("link", { name: /sandboxing guide/ }) as HTMLAnchorElement;
    expect(link.href).toBe(DECOY_GUIDE);
    expect(link.rel).toContain("noopener");
  });

  it("round-trips the workspace decoys through the form", () => {
    const form = toForm(settings);
    form.decoys = "~/.aws/credentials\n\n~/.kube/config\n";
    expect(fromForm(form, settings).settings.sandbox_decoys).toEqual(["~/.aws/credentials", "~/.kube/config"]);
  });
});

describe("network panel", () => {
  it("names the global choice, lists the global and built-in hosts, and saves a workspace choice", () => {
    const out = mount({ globalSandbox: { network: "public", allowed_hosts: ["mirror.corp.example"] } });
    const global = screen.getByLabelText(/^Use the global setting \(Public\)/) as HTMLInputElement;
    expect(global.checked).toBe(true);
    expect(screen.getByText("mirror.corp.example")).toBeTruthy();
    expect(screen.getByText(/Built in under Allowlist: 6 hosts/)).toBeTruthy();
    expect(screen.getByText("registry.npmjs.org")).toBeTruthy();

    fireEvent.click(screen.getByLabelText(/^None/));
    expect(screen.getByText(/apply under Allowlist and Public only, so None ignores them/)).toBeTruthy();
    fireEvent.change(hostsField(), { target: { value: "localhost:8317\n\n*.internal.example\n" } });

    const saved = fromForm(out.form as SettingsForm, settings).settings;
    expect(saved.sandbox_network).toBe("none");
    expect(saved.sandbox_allowed_hosts).toEqual(["localhost:8317", "*.internal.example"]);
  });

  it("follows the global choice again when the workspace's is cleared", () => {
    const out = mount({ saved: { sandbox_network: "host", sandbox_allowed_hosts: ["mirror.lan"] } });
    expect((screen.getByLabelText(/^Host/) as HTMLInputElement).checked).toBe(true);
    expect(hostsField().value).toBe("mirror.lan");
    fireEvent.click(screen.getByLabelText(/^Use the global setting \(Allowlist\)/));
    expect(fromForm(out.form as SettingsForm, settings).settings.sandbox_network).toBeNull();
  });

  it("shows a host the server refused on the hosts field", () => {
    mount({
      issues: [{ path: "sandbox_allowed_hosts[0]", message: "Add the port to `localhost`, such as `localhost:8080`." }],
    });
    expect(screen.getByText(/Add the port to `localhost`/)).toBeTruthy();
  });

  it("sets a macOS server's loopback choice and blocked ports, open by default", () => {
    const out = mount({ sandbox: { ...workspaceDetail.sandbox, backend: "seatbelt", decoys: false } });
    expect((screen.getByLabelText(/^Every loopback port but the blocked ones/) as HTMLInputElement).checked).toBe(true);
    fireEvent.click(screen.getByLabelText(/^Only loopback ports listed/));
    fireEvent.change(screen.getByLabelText(/^Blocked loopback ports/), {
      target: { value: "5432, 6379 57365 x 70000" },
    });
    const saved = fromForm(out.form as SettingsForm, settings).settings;
    expect(saved.sandbox_loopback).toBe("listed");
    expect(saved.sandbox_blocked_ports).toEqual([5432, 6379, 57365]);
  });

  it("shows no loopback choice where each sandbox has a loopback of its own", () => {
    mount();
    expect(screen.queryByLabelText(/^Blocked loopback ports/)).toBeNull();
  });
});

describe("tool enforcement panel", () => {
  it("follows the global setting by default and saves the workspace's own choice", () => {
    const out = mount();
    const global = screen.getByLabelText(/^Use the global setting \(Disabled\)/) as HTMLInputElement;
    expect(global.checked).toBe(true);
    expect(fromForm(out.form as SettingsForm, settings).settings.tool_enforcement).toBeNull();
    fireEvent.click(screen.getByLabelText(/^Enabled/));
    expect(fromForm(out.form as SettingsForm, settings).settings.tool_enforcement).toBe("enabled");
  });

  it("shows a saved workspace choice", () => {
    mount({ saved: { tool_enforcement: "disabled" } });
    expect((screen.getByLabelText(/^Disabled/) as HTMLInputElement).checked).toBe(true);
  });
});
