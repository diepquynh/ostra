import { Banner, Button, Checkbox, Input, Panel, Select, type SelectOption, Switch, Table } from "@ostra/design";
import { type CSSProperties, type ReactNode, useEffect, useState } from "react";
import type {
  AgentInfo,
  Complexity,
  GlobalSandbox,
  HarnessStatus,
  PermissionRules,
  SandboxMode,
  SandboxStatus,
  ValidationIssue,
} from "../../api/types";
import {
  COMPLEXITY_AGENTS,
  DECOY_GUIDE,
  NATIVE_ONLY,
  PERMISSION_MODES,
  ROUTE_KEYS,
  runsUnsandboxed,
  SANDBOX_MODES,
  SANDBOX_NETWORKS,
  takesHosts,
  UNSANDBOXED_EFFECTS,
} from "../../content/agents";
import { FileTagInput } from "../../features/context/FileTagInput";
import { currentSubscription, disablePush, enablePush, pushSupported } from "../../lib/push";
import { StackInput } from "../setup/StackInput";
import {
  COMPLEXITIES,
  EFFORTS,
  type ModelField,
  modelSelectValue,
  pickModel,
  routeKeys,
  type SettingsForm,
} from "./settingsForm";

export type SectionProps = {
  form: SettingsForm;
  update: (fn: (f: SettingsForm) => void) => void;
  /** Issues for one field id (see `fieldIds`). */
  issues: (field: string) => ValidationIssue[];
};

const ROLE = new Map<string, string>(ROUTE_KEYS.map((r) => [r.key, r.role]));

/** A deep-link target: `setting:<id>` is what ⌘K results and issue links scroll to. */
export function Anchor({ id, children, style }: { id: string; children: ReactNode; style?: CSSProperties }) {
  return (
    <div id={`setting:${id}`} className="wp-anchor" style={style}>
      {children}
    </div>
  );
}

export function FieldIssues({ issues }: { issues: ValidationIssue[] }) {
  if (issues.length === 0) return null;
  return (
    <ul className="wp-issues" style={{ marginTop: 4 }}>
      {issues.map((i, n) => (
        <li key={n}>{i.message}</li>
      ))}
    </ul>
  );
}

const errorText = (list: ValidationIssue[]) => (list.length ? list.map((i) => i.message).join(" ") : null);

// ---------------------------------------------------------------------------------------------
// General
// ---------------------------------------------------------------------------------------------

export function GeneralSection({ form, update, issues }: SectionProps) {
  return (
    <>
      <Panel title="Workspace">
        <Anchor id="name">
          <Input
            label="Name"
            value={form.name}
            error={errorText(issues("name"))}
            onChange={(e) => update((f) => void (f.name = e.target.value))}
          />
        </Anchor>
      </Panel>
      <Panel title="YOLO">
        <Anchor id="yolo.default">
          <div className="wp-stack" style={{ gap: 8 }}>
            <Switch
              tone="warn"
              label="Start new sessions with YOLO on"
              checked={form.yolo}
              onChange={(e) => update((f) => void (f.yolo = e.target.checked))}
            />
            <p className="wp-lead">
              Under YOLO, Ostra grants every permission ask and answers every gate itself, then lists each decision in
              the completion report. Guards, deny rules, the fact-check PASS requirement, security blocks, and the
              session budget still apply. You can switch it per session.
            </p>
          </div>
        </Anchor>
      </Panel>
      <Anchor id="limits">
        <Panel title="Limits" subtitle="bound spend and parallel work">
          <div className="wp-grid-2">
            <Anchor id="limits.max_parallel_executions">
              <Input
                label="Executions at once"
                type="number"
                min={1}
                mono
                value={form.maxParallel}
                error={errorText(issues("limits.max_parallel_executions"))}
                hint="Further spawns wait their turn, because a fan-out stage can otherwise start dozens of executions at once."
                onChange={(e) => update((f) => void (f.maxParallel = e.target.value))}
              />
            </Anchor>
            <Anchor id="limits.session_budget_usd">
              <Input
                label="Session budget in dollars"
                type="number"
                min={0}
                step="any"
                mono
                value={form.budget}
                error={errorText(issues("limits.session_budget_usd"))}
                hint="A session that reaches it pauses for your decision; YOLO never answers that gate. 0 means no limit."
                onChange={(e) => update((f) => void (f.budget = e.target.value))}
              />
            </Anchor>
          </div>
        </Panel>
      </Anchor>
    </>
  );
}

// ---------------------------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------------------------

type ProjectCell = { id: string; i: number };

export function ProjectsSection({
  form,
  update,
  issues,
  onAdd,
  stacks,
}: SectionProps & { onAdd: () => void; stacks: string[] }) {
  const rows: ProjectCell[] = form.projects.map((p, i) => ({ id: `${i}:${p.key}`, i }));
  return (
    <Anchor id="projects">
      <Panel
        title="Projects in this workspace"
        bodyFlush
        actions={
          <Button size="sm" variant="ghost" icon="folder-plus" onClick={onAdd}>
            Add project
          </Button>
        }
      >
        <div className="wp-muted" style={{ padding: "10px 12px 4px" }}>
          Add and initialize projects from the sidebar. Removing a project here deletes nothing on disk, and it takes
          effect when you save.
        </div>
        <Table<ProjectCell>
          dense
          rows={rows}
          empty="No projects yet. Add one with Add project."
          columns={[
            {
              key: "key",
              label: "Key",
              render: ({ i }) => (
                <div>
                  <span className="wp-mono">{form.projects[i].key}</span>
                  <FieldIssues issues={issues(`projects[${i}]`)} />
                </div>
              ),
            },
            {
              key: "path",
              label: "Path",
              render: ({ i }) => (
                <span className="wp-mono" style={{ color: "var(--text-secondary)" }}>
                  {form.projects[i].path}
                </span>
              ),
            },
            {
              key: "stack",
              label: "Stack",
              width: 190,
              render: ({ i }) => (
                <StackInput
                  size="sm"
                  stacks={stacks}
                  aria-label={`Stack of ${form.projects[i].key}`}
                  value={form.projects[i].stack}
                  onChange={(e) => update((f) => void (f.projects[i].stack = e.target.value))}
                />
              ),
            },
            {
              key: "remove",
              label: "",
              width: 80,
              render: ({ i }) => (
                <Button
                  size="sm"
                  variant="ghost"
                  title={`Remove ${form.projects[i].key} from this workspace`}
                  onClick={() => update((f) => void f.projects.splice(i, 1))}
                >
                  Remove
                </Button>
              ),
            },
          ]}
        />
        <div style={{ padding: "0 12px" }}>
          <FieldIssues issues={issues("projects")} />
        </div>
      </Panel>
    </Anchor>
  );
}

// ---------------------------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------------------------

/** Keep a value the file holds selectable even when the option list does not name it. */
function withCurrent(options: SelectOption[], value: string): SelectOption[] {
  return value && !options.some((o) => o.value === value) ? [...options, { value, label: value }] : options;
}

export function executorOptions(harnesses: HarnessStatus[]): SelectOption[] {
  return [
    { value: "native", label: "native" },
    ...harnesses.map((h) => ({
      value: `harness:${h.harness}`,
      label: `harness:${h.harness}${h.installed ? "" : " (not installed)"}`,
    })),
  ];
}

/** `Agent default (advanced)`: the option names the tier from the agent's `agent.toml`. */
function modelOptions(unsetLabel: string, agent?: AgentInfo): SelectOption[] {
  return [
    { value: "unset", label: unsetLabel },
    { value: "default", label: agent ? `Agent default (${agent.default_tier})` : "Agent default" },
    { value: "fast", label: "fast" },
    { value: "balanced", label: "balanced" },
    { value: "advanced", label: "advanced" },
    { value: "frontier", label: "frontier" },
    { value: "custom", label: "Model name…" },
    { value: "per-executor", label: "Per executor…" },
  ];
}

function ModelPicker({
  value,
  onChange,
  unsetLabel,
  label,
  error,
  agent,
}: {
  value: ModelField;
  onChange: (f: ModelField) => void;
  unsetLabel: string;
  label: string;
  error: string | null;
  agent?: AgentInfo;
}) {
  const def = value.kind === "default" ? agent?.default_route : null;
  return (
    <div className="wp-stack wp-cell-input" style={{ gap: 4 }}>
      <Select
        size="sm"
        style={{ width: 170 }}
        aria-label={label}
        value={modelSelectValue(value)}
        onChange={(e) => onChange(pickModel(value, e.target.value))}
        options={modelOptions(unsetLabel, agent)}
      />
      {def && (
        <span
          className="wp-mono wp-muted"
          title={`What Agent default resolves to on ${def.executor}, from the saved executor route`}
        >
          {def.model}
        </span>
      )}
      {(value.kind === "custom" || value.kind === "per-executor") && (
        <Input
          size="sm"
          mono
          aria-label={`${label}, ${value.kind === "custom" ? "model" : "per executor"}`}
          placeholder={
            value.kind === "custom"
              ? "anthropic:claude-sonnet-5"
              : "native = anthropic:claude-sonnet-5, codex = gpt-5.6-terra"
          }
          value={value.text}
          error={error}
          onChange={(e) => onChange({ ...value, text: e.target.value })}
        />
      )}
    </div>
  );
}

const effortDefaultLabel = (agent?: AgentInfo) => (agent ? `Agent default (${agent.default_effort})` : "Agent default");

type AgentRow = { id: string };
type ComplexityRow = { id: string; agent: string; c: Complexity };

export function RoutingSection({
  form,
  update,
  issues,
  harnesses,
  agentInfo,
}: SectionProps & { harnesses: HarnessStatus[]; agentInfo: AgentInfo[] }) {
  const execOpts = executorOptions(harnesses);
  const info = (id: string) => agentInfo.find((a) => a.name === id);
  const byComplexity = (k: string) => (COMPLEXITY_AGENTS as readonly string[]).includes(k) || k in form.complexityModel;
  const agents: AgentRow[] = routeKeys(form).map((id) => ({ id }));
  const complexityRows: ComplexityRow[] = Object.keys(form.complexityModel).flatMap((agent) =>
    COMPLEXITIES.map((c) => ({ id: `${agent}.${c}`, agent, c })),
  );

  return (
    <>
      <p className="wp-lead">
        Each agent runs on an executor (Ostra&apos;s native loop, or an installed harness CLI) and a model. A tier
        resolves through that executor&apos;s tier table in <code>~/.config/ostra/config.toml</code>; Agent default uses
        the agent&apos;s own tier. Native models are written <code>provider:model</code>. A route that does not resolve
        shows here as you edit, because an agent without a route cannot start.
      </p>
      <Anchor id="routing.byAgent">
        <Panel title="Executor and model per agent" subtitle="byPhaseComplexity wins over byAgent" bodyFlush>
          <Table<AgentRow>
            dense
            rows={agents}
            columns={[
              { key: "agent", label: "Agent", render: ({ id }) => <span className="wp-mono wp-nowrap">{id}</span> },
              {
                key: "role",
                label: "Role",
                render: ({ id }) => (
                  <span style={{ color: "var(--text-secondary)", fontSize: "var(--text-sm)" }}>
                    {ROLE.get(id) ?? "Not an agent Ostra knows. Set its model to Not set to remove it."}
                  </span>
                ),
              },
              {
                key: "executor",
                label: "Executor",
                width: 180,
                render: ({ id }) => (
                  <Anchor id={`routing.executor.byAgent.${id}`}>
                    {NATIVE_ONLY.has(id) ? (
                      <span className="wp-mono" style={{ color: "var(--text-muted)" }}>
                        native only
                      </span>
                    ) : (
                      <Select
                        size="sm"
                        mono
                        aria-label={`Executor for ${id}`}
                        style={{ width: 170 }}
                        value={form.executor[id] ?? "native"}
                        onChange={(e) => update((f) => void (f.executor[id] = e.target.value))}
                        options={withCurrent(execOpts, form.executor[id] ?? "native")}
                      />
                    )}
                    <FieldIssues issues={issues(`routing.executor.byAgent.${id}`)} />
                  </Anchor>
                ),
              },
              {
                key: "model",
                label: "Model",
                width: 210,
                render: ({ id }) => {
                  const list = issues(`routing.model.byAgent.${id}`);
                  const field = form.model[id] ?? { kind: "unset", text: "" };
                  const inline = field.kind === "custom" || field.kind === "per-executor";
                  return (
                    <Anchor id={`routing.model.byAgent.${id}`}>
                      <ModelPicker
                        label={`Model for ${id}`}
                        unsetLabel={byComplexity(id) ? "By complexity" : "Not set"}
                        agent={info(id)}
                        value={field}
                        error={inline ? errorText(list) : null}
                        onChange={(v) => update((f) => void (f.model[id] = v))}
                      />
                      {!inline && <FieldIssues issues={list} />}
                    </Anchor>
                  );
                },
              },
              {
                key: "effort",
                label: "Effort",
                width: 130,
                render: ({ id }) => (
                  <Anchor id={`routing.effort.byAgent.${id}`}>
                    {id === "judge" ? (
                      <span className="wp-mono" style={{ color: "var(--text-muted)" }}>
                        fixed
                      </span>
                    ) : (
                      <Select
                        size="sm"
                        aria-label={`Effort for ${id}`}
                        style={{ width: 130 }}
                        value={form.effort[id] ?? ""}
                        onChange={(e) => update((f) => void (f.effort[id] = e.target.value))}
                        options={withCurrent(
                          [
                            { value: "", label: effortDefaultLabel(info(id)) },
                            ...EFFORTS.map((e) => ({ value: e, label: e })),
                          ],
                          form.effort[id] ?? "",
                        )}
                      />
                    )}
                    <FieldIssues issues={issues(`routing.effort.byAgent.${id}`)} />
                  </Anchor>
                ),
              },
            ]}
          />
        </Panel>
      </Anchor>

      <Anchor id="routing.byPhaseComplexity">
        <Panel title="By phase complexity" subtitle="the phase file's Complexity line picks the route" bodyFlush>
          <div className="wp-muted" style={{ padding: "10px 12px 4px" }}>
            For these agents a phase&apos;s complexity picks the route and wins over the table above. Work with no phase
            file counts as low.
          </div>
          <Table<ComplexityRow>
            dense
            rows={complexityRows}
            columns={[
              { key: "agent", label: "Agent", render: (r) => <span className="wp-mono">{r.agent}</span> },
              {
                key: "c",
                label: "Complexity",
                render: (r) => <span style={{ color: "var(--text-secondary)" }}>{r.c}</span>,
              },
              {
                key: "executor",
                label: "Executor",
                width: 220,
                render: (r) => (
                  <Anchor id={`routing.executor.byPhaseComplexity.${r.agent}.${r.c}`}>
                    <Select
                      size="sm"
                      mono
                      aria-label={`Executor for ${r.agent} at ${r.c} complexity`}
                      value={form.complexityExecutor[r.agent]?.[r.c] ?? ""}
                      onChange={(e) =>
                        update((f) => {
                          f.complexityExecutor[r.agent] ??= { low: "", medium: "", high: "" };
                          f.complexityExecutor[r.agent][r.c] = e.target.value;
                        })
                      }
                      options={withCurrent(
                        [{ value: "", label: "Same as the agent route" }, ...execOpts],
                        form.complexityExecutor[r.agent]?.[r.c] ?? "",
                      )}
                    />
                    <FieldIssues issues={issues(`routing.executor.byPhaseComplexity.${r.agent}.${r.c}`)} />
                  </Anchor>
                ),
              },
              {
                key: "model",
                label: "Model",
                width: 240,
                render: (r) => {
                  const path = `routing.model.byPhaseComplexity.${r.agent}.${r.c}`;
                  return (
                    <Anchor id={path}>
                      <ModelPicker
                        label={`Model for ${r.agent} at ${r.c} complexity`}
                        unsetLabel="Same as the agent route"
                        agent={info(r.agent)}
                        value={form.complexityModel[r.agent][r.c]}
                        error={errorText(issues(path))}
                        onChange={(v) => update((f) => void (f.complexityModel[r.agent][r.c] = v))}
                      />
                    </Anchor>
                  );
                },
              },
              {
                key: "effort",
                label: "Effort",
                width: 150,
                render: (r) => {
                  const path = `routing.effort.byPhaseComplexity.${r.agent}.${r.c}`;
                  const current = form.complexityEffort[r.agent]?.[r.c] ?? "";
                  return (
                    <Anchor id={path}>
                      <Select
                        size="sm"
                        aria-label={`Effort for ${r.agent} at ${r.c} complexity`}
                        style={{ width: 150 }}
                        value={current}
                        onChange={(e) =>
                          update((f) => {
                            f.complexityEffort[r.agent] ??= { low: "", medium: "", high: "" };
                            f.complexityEffort[r.agent][r.c] = e.target.value;
                          })
                        }
                        options={withCurrent(
                          [
                            { value: "", label: "Same as the agent effort" },
                            ...EFFORTS.map((e) => ({ value: e, label: e })),
                          ],
                          current,
                        )}
                      />
                      <FieldIssues issues={issues(path)} />
                    </Anchor>
                  );
                },
              },
            ]}
          />
          <div style={{ padding: "0 12px" }}>
            <FieldIssues
              issues={[
                ...issues("routing.executor.byPhaseComplexity"),
                ...issues("routing.model.byPhaseComplexity"),
                ...issues("routing.effort.byPhaseComplexity"),
              ]}
            />
          </div>
        </Panel>
      </Anchor>
    </>
  );
}

// ---------------------------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------------------------

const RULE_LISTS = [
  { key: "allow", label: "Allow", placeholder: "Bash(npm run test *)" },
  { key: "ask", label: "Ask", placeholder: "Edit(migrations/**)" },
  { key: "deny", label: "Deny", placeholder: "Bash(git push *)" },
] as const;

/** Warns what stops working when this workspace's agent commands run without a sandbox. */
export function UnsandboxedBanner({ status }: { status: SandboxStatus }) {
  return (
    <Banner tone="warn" title="Turn the sandbox on to keep these protections, because agent commands run without one">
      <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
        {UNSANDBOXED_EFFECTS.map((t) => (
          <li key={t}>{t}</li>
        ))}
      </ul>
      {status.message && <p style={{ margin: "6px 0 0" }}>{status.message}</p>}
    </Banner>
  );
}

/** Workspace decoy paths, added to the built-in ones. Disabled where the server cannot plant decoys. */
function DecoysPanel({ form, update, issues, sandbox }: SectionProps & { sandbox: SandboxStatus }) {
  return (
    <Panel title="Decoy files" subtitle="added to the built-in ones">
      <div className="wp-stack" style={{ gap: 10 }}>
        <p className="wp-lead">
          Fake credential files in each agent sandbox. Opening one is a containment signal, and three signals from one
          execution pause the session.
        </p>
        {!sandbox.decoys && (
          <Banner tone="info" title="This server cannot plant them">
            <p style={{ margin: "4px 0 0" }}>
              On Linux they need bubblewrap. On macOS they need an admin account, because macOS shows sandbox reports
              only to admins. See{" "}
              <a href={DECOY_GUIDE} target="_blank" rel="noopener noreferrer">
                decoy credential files in the sandboxing guide
              </a>
              .
            </p>
          </Banner>
        )}
        <div>
          <span className="wp-muted">Always planted, and cannot be removed:</span>
          <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
            {sandbox.builtin_decoys.map((d) => (
              <li key={d}>
                <code>{d}</code>
              </li>
            ))}
          </ul>
        </div>
        <Input
          label="This workspace's decoys"
          mono
          multiline
          rows={3}
          disabled={!sandbox.decoys}
          placeholder="~/.aws/credentials"
          value={form.decoys}
          error={errorText(issues("sandbox_decoys"))}
          hint="One path per line, starting with ~/. A decoy covers an existing file, or fills a hidden folder."
          onChange={(e) => update((f) => void (f.decoys = e.target.value))}
        />
      </div>
    </Panel>
  );
}

/** The network choice in place of the global one, and hosts added to the global and built-in ones. */
function NetworkPanel({
  form,
  update,
  issues,
  sandbox,
  global,
}: SectionProps & { sandbox: SandboxStatus; global: GlobalSandbox }) {
  const label = (n: string) => SANDBOX_NETWORKS.find((c) => c.network === n)?.label ?? n;
  const effective = form.network || global.network;
  return (
    <Panel title="Network" subtitle="for this workspace, in place of the global choice">
      <div className="wp-stack" style={{ gap: 10 }}>
        <Checkbox
          radio
          name="sandbox-network"
          checked={form.network === ""}
          onChange={() => update((f) => void (f.network = ""))}
          label={`Use the global setting (${label(global.network)})`}
          description="Follows [sandbox] network in ~/.config/ostra/config.toml."
        />
        {SANDBOX_NETWORKS.map((c) => (
          <Checkbox
            key={c.network}
            radio
            name="sandbox-network"
            checked={form.network === c.network}
            onChange={() => update((f) => void (f.network = c.network))}
            label={c.label}
            description={c.help}
          />
        ))}
        <FieldIssues issues={issues("sandbox_network")} />
        {sandbox.backend === "seatbelt" && (
          <Anchor id="sandbox_loopback">
            <div className="wp-stack" style={{ gap: 8 }}>
              <span className="wp-muted">
                Every macOS sandbox shares this machine's loopback, where your local services listen. This applies under
                Allowlist and Public.
              </span>
              <Checkbox
                radio
                name="sandbox-loopback"
                checked={form.loopback === "open"}
                onChange={() => update((f) => void (f.loopback = "open"))}
                label="Every loopback port but the blocked ones"
                description="Tests reach the servers they start. Ostra's own port is always blocked."
              />
              <Checkbox
                radio
                name="sandbox-loopback"
                checked={form.loopback === "listed"}
                onChange={() => update((f) => void (f.loopback = "listed"))}
                label="Only loopback ports listed in the hosts below"
                description="Your local services stay out of reach, and a test server on another port is refused."
              />
              <Anchor id="sandbox_blocked_ports">
                <Input
                  label="Blocked loopback ports"
                  mono
                  placeholder="5432, 6379"
                  value={form.blockedPorts}
                  error={errorText(issues("sandbox_blocked_ports"))}
                  hint="Ports no command connects to, even when listed below, such as a database or an SSH forward."
                  onChange={(e) => update((f) => void (f.blockedPorts = e.target.value))}
                />
              </Anchor>
            </div>
          </Anchor>
        )}
        <Anchor id="sandbox_allowed_hosts">
          <Input
            label="This workspace's hosts"
            mono
            multiline
            rows={3}
            placeholder={"mirror.corp.example\n*.internal.example:8443\n127.0.0.1:8317"}
            value={form.allowedHosts}
            error={errorText(issues("sandbox_allowed_hosts"))}
            hint={
              takesHosts(effective)
                ? "One per line: a host name, *.domain, or an IP address, with an optional :port (443 and 80 without one). A loopback host needs its port."
                : `These apply under Allowlist and Public only, so ${label(effective)} ignores them.`
            }
            onChange={(e) => update((f) => void (f.allowedHosts = e.target.value))}
          />
        </Anchor>
        <div>
          <span className="wp-muted">
            {global.allowed_hosts.length === 0
              ? "The global config lists no hosts."
              : "Also allowed, from the global config:"}
          </span>
          {global.allowed_hosts.length > 0 && (
            <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
              {global.allowed_hosts.map((h) => (
                <li key={h}>
                  <code>{h}</code>
                </li>
              ))}
            </ul>
          )}
        </div>
        <details>
          <summary className="wp-muted" style={{ cursor: "pointer" }}>
            Built in under Allowlist: {sandbox.builtin_hosts.length} hosts
          </summary>
          <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
            {sandbox.builtin_hosts.map((h) => (
              <li key={h}>
                <code>{h}</code>
              </li>
            ))}
          </ul>
        </details>
      </div>
    </Panel>
  );
}

export function PermissionsSection({
  form,
  update,
  issues,
  global,
  sandbox,
  savedSandbox,
  globalSandbox,
}: SectionProps & {
  global: PermissionRules;
  sandbox: SandboxStatus;
  savedSandbox: SandboxMode | null;
  globalSandbox: GlobalSandbox;
}) {
  // The chosen mode before it is saved: off never sandboxes, auto only where a sandbox works.
  const unsandboxed =
    form.sandbox === "off" ||
    (form.sandbox === "auto" && !sandbox.available) ||
    (form.sandbox === (savedSandbox ?? "") && runsUnsandboxed(sandbox));
  const globalCount = global.allow.length + global.ask.length + global.deny.length;
  return (
    <>
      <p className="wp-lead">
        Two layers check every tool call. Guards come first and nothing overrides them: write scopes, state ownership,
        the build streak limit. Then your permissions: a mode plus rules such as <code>Bash(npm run test *)</code>,{" "}
        <code>Edit(src/**)</code>, or <code>WebFetch(domain:docs.rs)</code>. Each part of a chained command is checked
        on its own.
      </p>
      <div className="wp-grid-2">
        <Anchor id="permissions.mode">
          <Panel title="Mode">
            <div className="wp-stack" style={{ gap: 10 }}>
              {PERMISSION_MODES.map((m) => (
                <Checkbox
                  key={m.mode}
                  radio
                  name="permission-mode"
                  checked={form.mode === m.mode}
                  onChange={() => update((f) => void (f.mode = m.mode))}
                  label={m.label}
                  description={m.help}
                />
              ))}
              <FieldIssues issues={issues("permissions.mode")} />
            </div>
          </Panel>
        </Anchor>
        <Anchor id="sandbox_mode">
          <Panel title="Sandbox" subtitle="for this workspace, in place of the global mode">
            <div className="wp-stack" style={{ gap: 10 }}>
              {SANDBOX_MODES.map((m) => (
                <Checkbox
                  key={m.mode || "global"}
                  radio
                  name="sandbox-mode"
                  checked={form.sandbox === m.mode}
                  onChange={() => update((f) => void (f.sandbox = m.mode))}
                  label={m.label}
                  description={
                    m.mode === "" && sandbox
                      ? `Follows [sandbox] mode in the global config, which is ${sandbox.default_mode} on this machine unless set.`
                      : m.help
                  }
                />
              ))}
              <FieldIssues issues={issues("sandbox_mode")} />
              {unsandboxed && <UnsandboxedBanner status={sandbox} />}
            </div>
          </Panel>
        </Anchor>
        <Anchor id="sandbox_network">
          <NetworkPanel form={form} update={update} issues={issues} sandbox={sandbox} global={globalSandbox} />
        </Anchor>
        <Anchor id="sandbox_decoys">
          <DecoysPanel form={form} update={update} issues={issues} sandbox={sandbox} />
        </Anchor>
        <Panel title="Rules" subtitle="deny beats ask, ask beats allow">
          <div className="wp-stack">
            {RULE_LISTS.map((r) => (
              <Anchor key={r.key} id={`permissions.${r.key}`}>
                <Input
                  label={r.label}
                  mono
                  multiline
                  rows={3}
                  placeholder={r.placeholder}
                  value={form[r.key]}
                  error={errorText(issues(`permissions.${r.key}`))}
                  hint={r.key === "allow" ? "One rule per line." : undefined}
                  onChange={(e) => update((f) => void (f[r.key] = e.target.value))}
                />
              </Anchor>
            ))}
          </div>
        </Panel>
      </div>
      <Panel title="Global rules" subtitle="read-only, from ~/.config/ostra/config.toml">
        <div className="wp-stack" style={{ gap: 8 }}>
          <p className="wp-lead">
            These rules apply in every workspace on this machine, together with the workspace rules above. Edit{" "}
            <code>[permissions]</code> in the global config to change them.
          </p>
          {globalCount === 0 ? (
            <span className="wp-muted">The global config sets no permission rules.</span>
          ) : (
            RULE_LISTS.filter((r) => global[r.key].length > 0).map((r) => (
              <div key={r.key} className="wp-row" style={{ alignItems: "baseline" }}>
                <span className="wp-muted" style={{ width: 48 }}>
                  {r.label}
                </span>
                <span className="wp-row" style={{ gap: 6 }}>
                  {global[r.key].map((rule) => (
                    <code key={rule} className="wp-mono">
                      {rule}
                    </code>
                  ))}
                </span>
              </div>
            ))
          )}
        </div>
      </Panel>
    </>
  );
}

// ---------------------------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------------------------

type InstructionRow = { id: string };

/** An instruction field where `@` tags a project file or a visible workspace artifact (Rule W3). */
function TaggedField({
  error,
  ...props
}: { ws: string; projects: string[]; rows: number; label: string; placeholder?: string; value: string } & {
  error: string | null;
  onChange: (v: string) => void;
}) {
  return (
    <div>
      <FileTagInput {...props} />
      {error && (
        <p className="wp-field-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

export function InstructionsSection({
  form,
  update,
  issues,
  ws,
  projects,
}: SectionProps & { ws: string; projects: string[] }) {
  const rows: InstructionRow[] = ROUTE_KEYS.filter((r) => r.key !== "judge").map((r) => ({ id: r.key }));
  return (
    <>
      <p className="wp-lead">
        Ostra adds these to every agent&apos;s brief: the text for all agents first, then the agent&apos;s own. Routing
        settings are never included. Type @ to tag a project file or a workspace artifact, and each agent gets its path
        to read.
      </p>
      <Anchor id="instructions.all">
        <Panel title="All agents">
          <TaggedField
            ws={ws}
            projects={projects}
            rows={3}
            label="Instructions for all agents"
            placeholder="Write British English in comments and docs. Tag a file or artifact with @."
            value={form.instructionsAll}
            error={errorText(issues("instructions.all"))}
            onChange={(v) => update((f) => void (f.instructionsAll = v))}
          />
        </Panel>
      </Anchor>
      <Anchor id="instructions.agents">
        <Panel title="Per agent" bodyFlush>
          <Table<InstructionRow>
            dense
            rows={rows}
            columns={[
              { key: "agent", label: "Agent", width: 200, render: ({ id }) => <span className="wp-mono">{id}</span> },
              {
                key: "text",
                label: "Instructions",
                render: ({ id }) => (
                  <Anchor id={`instructions.agents.${id}`}>
                    <TaggedField
                      ws={ws}
                      projects={projects}
                      rows={1}
                      label={`Instructions for ${id}`}
                      value={form.instructionsAgents[id] ?? ""}
                      error={errorText(issues(`instructions.agents.${id}`))}
                      onChange={(v) => update((f) => void (f.instructionsAgents[id] = v))}
                    />
                  </Anchor>
                ),
              },
            ]}
          />
        </Panel>
      </Anchor>
    </>
  );
}

// ---------------------------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------------------------

export function NotificationsSection({ form, update }: SectionProps) {
  const supported = pushSupported();
  const [subscribed, setSubscribed] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  useEffect(() => {
    if (!supported) return;
    let alive = true;
    currentSubscription().then(
      (s) => alive && setSubscribed(s !== null),
      () => alive && setSubscribed(false),
    );
    return () => {
      alive = false;
    };
  }, [supported]);

  const run = async (fn: () => Promise<string | null>) => {
    setBusy(true);
    setMessage(null);
    try {
      setMessage(await fn());
      setSubscribed((await currentSubscription()) !== null);
    } catch (e) {
      setMessage((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Anchor id="notifications.push">
      <Panel title="Push notifications">
        <div className="wp-stack">
          <Switch
            label="Send push notifications when a gate waits, a session completes, a phase is blocked, or a harness needs login"
            checked={form.push}
            onChange={(e) => update((f) => void (f.push = e.target.checked))}
          />
          <p className="wp-lead">
            Each notification opens the screen that needs you. This switch applies to every browser you subscribe below.
          </p>
          <div className="wp-row" style={{ borderTop: "1px solid var(--border-subtle)", paddingTop: 12 }}>
            <span style={{ fontWeight: 500 }}>This browser</span>
            <span className="wp-muted">
              {!supported
                ? "This browser does not support push notifications."
                : subscribed === null
                  ? "Checking…"
                  : subscribed
                    ? "Subscribed. It receives notifications."
                    : "Not subscribed."}
            </span>
            <span className="wp-spacer" />
            {supported && (
              <>
                <Button size="sm" disabled={busy || subscribed === true} onClick={() => void run(enablePush)}>
                  Enable in this browser
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy || subscribed !== true}
                  onClick={() =>
                    void run(async () => {
                      await disablePush();
                      return "This browser no longer receives notifications.";
                    })
                  }
                >
                  Disable in this browser
                </Button>
              </>
            )}
          </div>
          {message && <div className="wp-muted">{message}</div>}
        </div>
      </Panel>
    </Anchor>
  );
}
