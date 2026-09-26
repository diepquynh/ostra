import { Banner, Button, Checkbox, Chip, Input, Panel, Select, Spinner, Switch, Table, type Tone } from "@ostra/design";
import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type {
  AgentInfo,
  McpConnState,
  McpServerConfig,
  McpServerStatus,
  McpToolInfo,
  ValidationIssue,
} from "../../api/types";
import { Anchor, FieldIssues, type SectionProps } from "./SettingsSections";
import { type McpRow, mcpFromRow, mcpRow, stableJson } from "./settingsForm";

/** A URL safe to hand to the browser: a script or data URL would run on Ostra's origin. */
function isWebUrl(u: string): boolean {
  try {
    const p = new URL(u).protocol;
    return p === "https:" || p === "http:";
  } catch {
    return false;
  }
}

const STATE: Record<McpConnState, { label: string; tone: Tone }> = {
  connected: { label: "Connected", tone: "ok" },
  needs_auth: { label: "Needs sign-in", tone: "warn" },
  error: { label: "Not reachable", tone: "bad" },
  idle: { label: "Not connected", tone: "neutral" },
  disabled: { label: "Off", tone: "neutral" },
};

/** How long a sign-in in another tab is waited for. */
const SIGN_IN_WAIT_MS = 120_000;

const errorText = (list: ValidationIssue[]) => (list.length ? list.map((i) => i.message).join(" ") : null);

type McpSectionProps = SectionProps & {
  ws: string;
  /** The saved servers, which the status describes. */
  saved: McpServerConfig[];
  agents: AgentInfo[];
};

export function McpSection({ ws, saved, agents, form, update, issues }: McpSectionProps) {
  const [status, setStatus] = useState<Record<string, McpServerStatus>>({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const savedKey = stableJson(saved);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await api.mcpStatus(ws);
      setStatus(Object.fromEntries(list.map((s) => [s.name, s])));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [ws]);

  // Reconnect when the saved servers change, because the status describes them.
  // biome-ignore lint/correctness/useExhaustiveDependencies: savedKey is the trigger
  useEffect(() => {
    if (saved.length) void load();
    else setStatus({});
  }, [load, savedKey]);

  const setOne = (s: McpServerStatus) => setStatus((prev) => ({ ...prev, [s.name]: s }));

  return (
    <>
      <p className="wp-lead">
        Agents on every executor can use the tools of these servers. Ostra connects to each server itself and offers its
        tools to native runs and to every harness, so the permission rules below apply to all of them. Every tool a
        server lists is offered unless you turn it off here. Tools run without asking; add rules such as{" "}
        <code>mcp__github__create_issue</code> or <code>mcp__github</code> (the whole server) under Permissions to ask
        or deny. Plan mode allows only the tools a server marks read-only. Auth is optional: a server that asks gets a
        Sign in button.
      </p>
      {error && <Banner tone="bad">Could not read the servers&apos; status: {error}</Banner>}
      <Anchor id="mcp_servers">
        <FieldIssues issues={issues("mcp_servers")} />
      </Anchor>
      {form.mcp.map((row, i) => (
        <ServerPanel
          key={row.id}
          ws={ws}
          i={i}
          row={row}
          saved={saved.find((c) => c.name === row.name.trim())}
          status={status[row.name.trim()]}
          loading={loading}
          agents={agents}
          update={update}
          issues={issues}
          onStatus={setOne}
        />
      ))}
      <div className="wp-row">
        <Button icon="plus" size="sm" onClick={() => update((f) => void f.mcp.push(mcpRow()))}>
          Add MCP server
        </Button>
        {form.mcp.length === 0 && <span className="wp-muted">No MCP servers yet.</span>}
        {saved.length > 0 && (
          <Button size="sm" variant="ghost" icon="refresh-ccw" disabled={loading} onClick={() => void load()}>
            Check again
          </Button>
        )}
      </div>
    </>
  );
}

type ServerPanelProps = {
  ws: string;
  i: number;
  row: McpRow;
  saved: McpServerConfig | undefined;
  status: McpServerStatus | undefined;
  loading: boolean;
  agents: AgentInfo[];
  update: SectionProps["update"];
  issues: SectionProps["issues"];
  onStatus: (s: McpServerStatus) => void;
};

function ServerPanel({ ws, i, row, saved, status, loading, agents, update, issues, onStatus }: ServerPanelProps) {
  const at = `mcp_servers[${i}]`;
  const edit = (fn: (r: McpRow) => void) => update((f) => fn(f.mcp[i]));
  const [busy, setBusy] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const waiting = useRef<number | null>(null);
  const [awaitingSignIn, setAwaitingSignIn] = useState(false);
  const stopWaiting = useCallback(() => {
    if (waiting.current) clearInterval(waiting.current);
    waiting.current = null;
    setAwaitingSignIn(false);
  }, []);
  useEffect(() => stopWaiting, [stopWaiting]);

  const unsaved = !saved || stableJson(mcpFromRow(row, at, [])) !== stableJson(saved);
  const name = row.name.trim();
  const state = status?.state;

  const act = async (what: string, fn: () => Promise<void>) => {
    setBusy(what);
    setActionError(null);
    try {
      await fn();
    } catch (e) {
      setActionError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const reconnect = () => act("reconnect", async () => onStatus(await api.mcpRefresh(ws, name)));
  const signOut = () => act("logout", async () => onStatus(await api.mcpLogout(ws, name)));
  const signIn = () =>
    act("login", async () => {
      // Opened before the request, because browsers block a tab opened after an await.
      const tab = window.open("", "_blank");
      try {
        const { authorization_url } = await api.mcpLogin(ws, name);
        if (!isWebUrl(authorization_url)) {
          throw new Error(
            "Ostra refused the sign-in link, because it is not an http or https address. Check the server's OAuth settings.",
          );
        }
        if (tab) {
          tab.opener = null;
          tab.location.href = authorization_url;
        } else window.location.assign(authorization_url);
      } catch (e) {
        tab?.close();
        throw e;
      }
      const started = Date.now();
      stopWaiting();
      setAwaitingSignIn(true);
      waiting.current = window.setInterval(async () => {
        const s = await api.mcpRefresh(ws, name).catch(() => null);
        if (s) onStatus(s);
        if ((s && (s.state === "connected" || s.signed_in)) || Date.now() - started > SIGN_IN_WAIT_MS) stopWaiting();
      }, 2000);
    });

  const chip = unsaved ? (
    <Chip tone="info">Unsaved</Chip>
  ) : !row.enabled ? (
    <Chip>{STATE.disabled.label}</Chip>
  ) : state ? (
    <Chip tone={STATE[state].tone}>{STATE[state].label}</Chip>
  ) : loading ? (
    <span className="wp-row" style={{ gap: 6 }}>
      <Spinner size={12} /> <span className="wp-muted">Connecting…</span>
    </span>
  ) : null;

  return (
    <Anchor id={at}>
      <Panel
        title={name || "New MCP server"}
        subtitle={row.transport === "http" ? "remote, streamable HTTP" : "local command, stdio"}
        icon="plug-zap"
        tone={state === "error" && !unsaved ? "bad" : state === "needs_auth" && !unsaved ? "warn" : undefined}
        actions={
          <div className="wp-row" style={{ gap: 8 }}>
            {chip}
            <Switch
              label="On"
              checked={row.enabled}
              onChange={(e) => edit((r) => void (r.enabled = e.target.checked))}
            />
            <Button
              size="sm"
              variant="ghost"
              icon="trash-2"
              title={`Remove ${name || "this server"}`}
              onClick={() => update((f) => void f.mcp.splice(i, 1))}
            >
              Remove
            </Button>
          </div>
        }
      >
        <div className="wp-stack">
          <FieldIssues issues={issues(at)} />
          <div className="wp-grid-2">
            <Anchor id={`${at}.name`}>
              <Input
                label="Name"
                mono
                placeholder="github"
                hint="Lowercase letters, digits, and dashes. Tools are named mcp__<name>__<tool>."
                value={row.name}
                error={errorText(issues(`${at}.name`))}
                onChange={(e) => edit((r) => void (r.name = e.target.value))}
              />
            </Anchor>
            <Select
              label="Runs as"
              value={row.transport}
              onChange={(e) => edit((r) => void (r.transport = e.target.value as McpRow["transport"]))}
              options={[
                { value: "http", label: "Remote server (URL)" },
                { value: "stdio", label: "Local command" },
              ]}
            />
          </div>
          {row.transport === "http" ? (
            <>
              <Anchor id={`${at}.url`}>
                <Input
                  label="URL"
                  mono
                  placeholder="https://api.githubcopilot.com/mcp/"
                  value={row.url}
                  error={errorText(issues(`${at}.url`))}
                  onChange={(e) => edit((r) => void (r.url = e.target.value))}
                />
              </Anchor>
              <Anchor id={`${at}.headers`}>
                <Input
                  label="Headers"
                  mono
                  multiline
                  rows={2}
                  placeholder="Authorization: Bearer ${GITHUB_TOKEN}"
                  hint="Optional. One per line. ${VAR} reads the Ostra server's environment. A value typed here is saved encrypted on this machine and shows as ${ostra_secret}; keep that line to keep the value."
                  value={row.headers}
                  error={errorText(issues(`${at}.headers`))}
                  onChange={(e) => edit((r) => void (r.headers = e.target.value))}
                />
              </Anchor>
            </>
          ) : (
            <>
              <Anchor id={`${at}.command`}>
                <Input
                  label="Command"
                  mono
                  placeholder="npx -y @upstash/context7-mcp"
                  hint="Runs in the workspace folder. Quote an argument that contains spaces."
                  value={row.command}
                  error={errorText(issues(`${at}.command`))}
                  onChange={(e) => edit((r) => void (r.command = e.target.value))}
                />
              </Anchor>
              <Anchor id={`${at}.env`}>
                <Input
                  label="Environment"
                  mono
                  multiline
                  rows={2}
                  placeholder="API_KEY=${CONTEXT7_KEY}"
                  hint="Optional. KEY=value, one per line. ${VAR} reads the Ostra server's environment. A value typed here is saved encrypted on this machine and shows as ${ostra_secret}; keep that line to keep the value."
                  value={row.env}
                  error={errorText(issues(`${at}.env`))}
                  onChange={(e) => edit((r) => void (r.env = e.target.value))}
                />
              </Anchor>
            </>
          )}
          <div className="wp-grid-2">
            <Anchor id={`${at}.agents`}>
              <AgentPicker row={row} agents={agents} edit={edit} issues={issues(`${at}.agents`)} />
            </Anchor>
            <Anchor id={`${at}.timeout_secs`}>
              <Input
                label="Tool call timeout, seconds"
                value={row.timeout}
                inputMode="numeric"
                error={errorText(issues(`${at}.timeout_secs`))}
                onChange={(e) => edit((r) => void (r.timeout = e.target.value))}
              />
            </Anchor>
          </div>
          {row.transport === "http" && (
            <Anchor id={`${at}.oauth`}>
              <details>
                <summary className="wp-muted" style={{ cursor: "pointer" }}>
                  OAuth client (optional)
                </summary>
                <div className="wp-stack" style={{ marginTop: 10 }}>
                  <p className="wp-lead">
                    Ostra signs in when the server asks and registers itself as a client where the server allows it.
                    Fill these in only for a client you registered by hand.
                  </p>
                  <div className="wp-grid-2">
                    <Input
                      label="Client ID"
                      mono
                      value={row.clientId}
                      onChange={(e) => edit((r) => void (r.clientId = e.target.value))}
                    />
                    <Input
                      label="Client secret variable"
                      mono
                      placeholder="LINEAR_CLIENT_SECRET"
                      hint="The name of an environment variable, not the secret."
                      value={row.clientSecretEnv}
                      onChange={(e) => edit((r) => void (r.clientSecretEnv = e.target.value))}
                    />
                  </div>
                  <Input
                    label="Scopes"
                    mono
                    placeholder="read write"
                    hint="Separated by spaces. Empty requests the scopes the server advertises."
                    value={row.scopes}
                    onChange={(e) => edit((r) => void (r.scopes = e.target.value))}
                  />
                  <FieldIssues issues={issues(`${at}.oauth`)} />
                </div>
              </details>
            </Anchor>
          )}

          {unsaved ? (
            <p className="wp-muted" style={{ margin: 0 }}>
              {saved ? "Save to connect with these changes." : "Save to connect to this server and list its tools."}
            </p>
          ) : (
            row.enabled &&
            status && (
              <>
                {status.message && <Banner tone={state === "needs_auth" ? "warn" : "bad"}>{status.message}</Banner>}
                {actionError && <Banner tone="bad">{actionError}</Banner>}
                <div className="wp-row" style={{ gap: 8 }}>
                  {status.server_info && <span className="wp-muted">{status.server_info}</span>}
                  <span className="wp-spacer" />
                  {status.transport === "http" && (state === "needs_auth" || status.signed_in != null) && (
                    <Button
                      size="sm"
                      variant={state === "needs_auth" ? "primary" : "ghost"}
                      icon="shield-check"
                      disabled={busy !== null}
                      onClick={() => void signIn()}
                    >
                      {busy === "login" ? "Opening…" : status.signed_in ? "Sign in again" : "Sign in"}
                    </Button>
                  )}
                  {status.signed_in && (
                    <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => void signOut()}>
                      {busy === "logout" ? "Signing out…" : "Sign out"}
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant="ghost"
                    icon="refresh-ccw"
                    disabled={busy !== null}
                    onClick={() => void reconnect()}
                  >
                    {busy === "reconnect" ? "Connecting…" : "Reconnect"}
                  </Button>
                </div>
                {awaitingSignIn && <span className="wp-muted">Waiting for the sign-in in the other tab…</span>}
              </>
            )
          )}
          <ToolList row={row} status={unsaved ? undefined : status} edit={edit} />
        </div>
      </Panel>
    </Anchor>
  );
}

function AgentPicker({
  row,
  agents,
  edit,
  issues,
}: {
  row: McpRow;
  agents: AgentInfo[];
  edit: (fn: (r: McpRow) => void) => void;
  issues: ValidationIssue[];
}) {
  const every = row.agents.length === 0;
  return (
    <div className="wp-stack" style={{ gap: 6 }}>
      <Checkbox
        label="Every agent"
        description="Uncheck to choose the agents that get this server's tools."
        checked={every}
        onChange={(e) => edit((r) => void (r.agents = e.target.checked ? [] : agents.map((a) => a.name as string)))}
      />
      {!every && (
        <div className="wp-row" style={{ gap: "4px 14px", paddingLeft: 24 }}>
          {agents.map((a) => (
            <Checkbox
              key={a.name}
              label={a.label}
              checked={row.agents.includes(a.name)}
              onChange={(e) =>
                edit((r) => {
                  r.agents = e.target.checked ? [...r.agents, a.name] : r.agents.filter((n) => n !== a.name);
                })
              }
            />
          ))}
        </div>
      )}
      <FieldIssues issues={issues} />
    </div>
  );
}

type ToolRow = { id: string; tool: McpToolInfo | null; name: string };

function ToolList({
  row,
  status,
  edit,
}: {
  row: McpRow;
  status: McpServerStatus | undefined;
  edit: (fn: (r: McpRow) => void) => void;
}) {
  const listed = status?.state === "connected" ? status.tools : [];
  // Tools turned off that the server no longer lists, or that are unknown while it is not connected.
  const others = row.disabledTools.filter((n) => !listed.some((t) => t.name === n));
  if (listed.length === 0 && others.length === 0) return null;
  const rows: ToolRow[] = [
    ...listed.map((t) => ({ id: t.name, tool: t, name: t.name })),
    ...others.map((n) => ({ id: `off:${n}`, tool: null, name: n })),
  ];
  const offered = (n: string) => !row.disabledTools.includes(n);
  const toggle = (n: string, on: boolean) =>
    edit((r) => {
      r.disabledTools = on ? r.disabledTools.filter((t) => t !== n) : [...r.disabledTools, n];
    });
  const count = listed.filter((t) => offered(t.name)).length;
  return (
    <div className="wp-stack" style={{ gap: 6 }}>
      {listed.length > 0 && (
        <span className="wp-muted">
          {count} of {listed.length} tool{listed.length === 1 ? "" : "s"} offered to agents
        </span>
      )}
      <Table<ToolRow>
        dense
        rows={rows}
        columns={[
          {
            key: "offered",
            label: "Offer",
            width: 64,
            render: (r) => (
              <Switch
                aria-label={`Offer ${r.name} to agents`}
                checked={offered(r.name)}
                onChange={(e) => toggle(r.name, e.target.checked)}
              />
            ),
          },
          {
            key: "tool",
            label: "Tool",
            render: (r) => (
              <div className="wp-stack" style={{ gap: 2 }}>
                <span className="wp-mono">{r.name}</span>
                {r.tool && (
                  <span className="wp-mono" style={{ color: "var(--text-muted)", fontSize: "var(--text-xs)" }}>
                    {r.tool.canonical}
                  </span>
                )}
              </div>
            ),
          },
          {
            key: "description",
            label: "Description",
            render: (r) =>
              r.tool ? (
                <span style={{ color: "var(--text-secondary)" }}>{r.tool.description || "No description."}</span>
              ) : (
                <span className="wp-muted">Turned off; the server does not list it right now.</span>
              ),
          },
          {
            key: "kind",
            label: "",
            width: 96,
            render: (r) => (r.tool?.read_only ? <Chip tone="info">read-only</Chip> : null),
          },
        ]}
      />
    </div>
  );
}
