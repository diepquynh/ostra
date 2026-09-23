import { useEffect, useMemo, useState } from "react";
import { useParams } from "react-router";
import { api, HttpError } from "../../api";
import type { Complexity, HarnessStatus, ModelChoice, ValidationIssue, WorkspaceDetail, WorkspaceSettings } from "../../api/types";
import { useCrumbs } from "../../components/Layout";
import { ErrorBox, Loading } from "../../components/Status";
import { COMPLEXITY_AGENTS, NATIVE_ONLY, PERMISSION_MODES, ROUTE_KEYS, TIERS } from "../../content/agents";
import { useAsync } from "../../lib/hooks";
import { disablePush, enablePush, pushSupported } from "../../lib/push";
import { STACKS } from "../workspaces/ImportProjectForm";
import { WorkspaceTabs } from "../workspaces/WorkspaceTabs";

const COMPLEXITIES: Complexity[] = ["low", "medium", "high"];

export function executorOptions(harnesses: HarnessStatus[]): { value: string; label: string; disabled: boolean }[] {
  return [
    { value: "native", label: "native (Ostra agent loop)", disabled: false },
    ...harnesses.map((h) => ({
      value: `harness:${h.harness}`,
      label: `harness:${h.harness}${h.installed ? "" : " (not installed)"}`,
      disabled: !h.installed,
    })),
  ];
}

function ModelInput({ value, onChange, listId }: { value: ModelChoice | undefined; onChange: (v: ModelChoice | undefined) => void; listId: string }) {
  const [json, setJson] = useState(() => (value && typeof value === "object" ? JSON.stringify(value) : ""));
  const [bad, setBad] = useState(false);
  if (value && typeof value === "object") {
    return (
      <div className="stack">
        <input
          className="mono"
          value={json}
          title='Per-executor table, for example {"native": "anthropic:claude-sonnet-5", "codex": "gpt-5.6-terra"}'
          onChange={(e) => {
            setJson(e.target.value);
            try {
              const parsed = JSON.parse(e.target.value);
              if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
                setBad(false);
                onChange(parsed as ModelChoice);
              } else setBad(true);
            } catch {
              setBad(true);
            }
          }}
        />
        {bad && <span className="form-error">Not a JSON object.</span>}
        <button className="small ghost" onClick={() => onChange("default")}>
          Use a single value
        </button>
      </div>
    );
  }
  return (
    <div className="row" style={{ flexWrap: "nowrap" }}>
      <input list={listId} value={value ?? ""} placeholder="missing" onChange={(e) => onChange(e.target.value || undefined)} />
      <button
        className="small ghost"
        title="Choose a different model per executor"
        onClick={() => {
          const v = { native: typeof value === "string" ? value : "default" };
          setJson(JSON.stringify(v));
          onChange(v);
        }}
      >
        Per executor
      </button>
    </div>
  );
}

function Issues({ issues, prefix }: { issues: ValidationIssue[]; prefix: string }) {
  const mine = issues.filter((i) => i.path === prefix || i.path.startsWith(`${prefix}.`) || i.path.startsWith(`${prefix}[`));
  if (mine.length === 0) return null;
  return (
    <ul className="issues small" style={{ margin: "4px 0", paddingLeft: 18 }}>
      {mine.map((i, n) => (
        <li key={n}>{i.message}</li>
      ))}
    </ul>
  );
}

function lines(list: string[]): string {
  return list.join("\n");
}
function unlines(text: string): string[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);
}

export function SettingsForm({ detail, onSaved }: { detail: WorkspaceDetail; onSaved: (d: WorkspaceDetail) => void }) {
  const [s, setS] = useState<WorkspaceSettings>(() => structuredClone(detail.settings));
  const [issues, setIssues] = useState<ValidationIssue[]>(detail.validation);
  const [validating, setValidating] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pushMsg, setPushMsg] = useState<string | null>(null);
  const dirty = useMemo(() => JSON.stringify(s) !== JSON.stringify(detail.settings), [s, detail.settings]);
  const execOpts = executorOptions(detail.harnesses);

  useEffect(() => {
    if (!dirty) {
      setIssues(detail.validation);
      return;
    }
    setValidating(true);
    const t = setTimeout(() => {
      api
        .validateSettings(detail.id, s)
        .then(setIssues)
        .catch(() => {})
        .finally(() => setValidating(false));
    }, 400);
    return () => clearTimeout(t);
  }, [s, dirty, detail.id, detail.validation]);

  const update = (fn: (draft: WorkspaceSettings) => void) =>
    setS((prev) => {
      const next = structuredClone(prev);
      fn(next);
      return next;
    });

  const save = async () => {
    setSaving(true);
    setError(null);
    setMessage(null);
    try {
      const d = await api.saveSettings(detail.id, s);
      onSaved(d);
      setMessage("Saved. The next execution uses these settings; no restart is needed.");
    } catch (e) {
      if (e instanceof HttpError && e.issues.length) setIssues(e.issues);
      setError((e as Error).message);
    } finally {
      setSaving(false);
    }
  };

  const setModel = (key: string, v: ModelChoice | undefined) =>
    update((d) => {
      if (v === undefined) delete d.routing.model.byAgent[key];
      else d.routing.model.byAgent[key] = v;
    });
  const setExecutor = (key: string, v: string) =>
    update((d) => {
      if (v === "native") delete d.routing.executor.byAgent[key];
      else d.routing.executor.byAgent[key] = v;
    });

  return (
    <div className="stack">
      <datalist id="tiers">
        {TIERS.map((t) => (
          <option key={t} value={t} />
        ))}
      </datalist>

      <div className="card">
        <h2>General</h2>
        <label className="field">
          <span className="label">Name</span>
          <input value={s.name} onChange={(e) => update((d) => void (d.name = e.target.value))} />
        </label>
        <Issues issues={issues} prefix="name" />
      </div>

      <div className="card">
        <h2>Projects</h2>
        <p className="small muted">Import and initialize projects on the Projects tab. Removing a project here deletes nothing on disk.</p>
        <table>
          <thead>
            <tr>
              <th>Key</th>
              <th>Path</th>
              <th>Stack</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {s.projects.map((p, i) => (
              <tr key={p.key}>
                <td className="mono">{p.key}</td>
                <td className="mono small">{p.path}</td>
                <td>
                  <select value={p.stack ?? ""} onChange={(e) => update((d) => void (d.projects[i].stack = e.target.value || null))}>
                    {STACKS.map((st) => (
                      <option key={st} value={st}>
                        {st || "Detect"}
                      </option>
                    ))}
                  </select>
                </td>
                <td>
                  <button className="small danger" onClick={() => update((d) => void d.projects.splice(i, 1))}>
                    Remove
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <Issues issues={issues} prefix="projects" />
      </div>

      <div className="card">
        <h2>Routing</h2>
        <p className="small muted">
          Each agent runs on an executor (Ostra's native loop, or an installed harness CLI) and a model. A tier (fast, balanced,
          advanced, frontier) resolves through the executor's tier table in <code>~/.config/ostra/config.toml</code>;{" "}
          <code>default</code> uses the agent's own default tier. Native models are written <code>provider:model</code>. Every agent must
          have a route, and a route that does not resolve is refused here, not when the agent starts.
        </p>
        <table className="route-table">
          <thead>
            <tr>
              <th>Agent</th>
              <th>Executor</th>
              <th>Model</th>
            </tr>
          </thead>
          <tbody>
            {ROUTE_KEYS.map(({ key, role }) => (
              <tr key={key}>
                <td>
                  <div className="mono">{key}</div>
                  <div className="small muted">{role}</div>
                  <Issues issues={issues} prefix={`routing.model.byAgent.${key}`} />
                  <Issues issues={issues} prefix={`routing.executor.byAgent.${key}`} />
                </td>
                <td>
                  {NATIVE_ONLY.has(key) ? (
                    <span className="small muted">native only</span>
                  ) : (
                    <select value={s.routing.executor.byAgent[key] ?? "native"} onChange={(e) => setExecutor(key, e.target.value)}>
                      {execOpts.map((o) => (
                        <option key={o.value} value={o.value} disabled={o.disabled}>
                          {o.label}
                        </option>
                      ))}
                    </select>
                  )}
                </td>
                <td>
                  <ModelInput listId="tiers" value={s.routing.model.byAgent[key]} onChange={(v) => setModel(key, v)} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>

        <h3 className="mt">By phase complexity</h3>
        <p className="small muted">
          For the implementer and write-test agents, a phase's complexity picks the route and wins over the table above. Work with no
          phase file counts as low.
        </p>
        {COMPLEXITY_AGENTS.map((agent) => (
          <table className="route-table mb" key={agent}>
            <thead>
              <tr>
                <th>{agent}</th>
                <th>Executor</th>
                <th>Model</th>
              </tr>
            </thead>
            <tbody>
              {COMPLEXITIES.map((c) => {
                const ex = s.routing.executor.byPhaseComplexity[agent]?.[c];
                const mo = s.routing.model.byPhaseComplexity[agent]?.[c];
                return (
                  <tr key={c}>
                    <td>{c}</td>
                    <td>
                      <select
                        value={ex ?? ""}
                        onChange={(e) =>
                          update((d) => {
                            const m = (d.routing.executor.byPhaseComplexity[agent] ??= {});
                            if (e.target.value) m[c] = e.target.value;
                            else delete m[c];
                            if (Object.keys(m).length === 0) delete d.routing.executor.byPhaseComplexity[agent];
                          })
                        }
                      >
                        <option value="">same as the agent route</option>
                        {execOpts.map((o) => (
                          <option key={o.value} value={o.value} disabled={o.disabled}>
                            {o.label}
                          </option>
                        ))}
                      </select>
                    </td>
                    <td>
                      <ModelInput
                        listId="tiers"
                        value={mo}
                        onChange={(v) =>
                          update((d) => {
                            const m = (d.routing.model.byPhaseComplexity[agent] ??= {});
                            if (v === undefined) delete m[c];
                            else m[c] = v;
                            if (Object.keys(m).length === 0) delete d.routing.model.byPhaseComplexity[agent];
                          })
                        }
                      />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ))}
        <Issues issues={issues} prefix="routing.executor.byPhaseComplexity" />
        <Issues issues={issues} prefix="routing.model.byPhaseComplexity" />
      </div>

      <div className="card">
        <h2>Custom instructions</h2>
        <p className="small muted">Added to every agent's brief: the text for all agents first, then the agent's own. Routing settings are never included.</p>
        <label className="field">
          <span className="label">All agents</span>
          <textarea
            rows={3}
            value={s.instructions.all ?? ""}
            onChange={(e) => update((d) => void (d.instructions.all = e.target.value || null))}
          />
        </label>
        {ROUTE_KEYS.filter((r) => r.key !== "judge").map(({ key }) => (
          <label className="field mt" key={key}>
            <span className="label mono">{key}</span>
            <textarea
              rows={1}
              value={s.instructions.agents[key] ?? ""}
              onChange={(e) =>
                update((d) => {
                  if (e.target.value) d.instructions.agents[key] = e.target.value;
                  else delete d.instructions.agents[key];
                })
              }
            />
          </label>
        ))}
      </div>

      <div className="card">
        <h2>Permissions</h2>
        <p className="small muted">
          Two layers check every tool call. Guards come first and nothing overrides them: write scopes, state ownership, the build
          streak limit. Then your permissions: a mode plus rules such as <code>Bash(npm run test *)</code>, <code>Edit(src/**)</code>,
          or <code>WebFetch(domain:docs.rs)</code>. Deny beats ask, and ask beats allow. Each part of a chained command is checked on
          its own.
        </p>
        <div className="stack">
          {PERMISSION_MODES.map((m) => (
            <label key={m.mode} className="check">
              <input
                type="radio"
                name="mode"
                checked={s.permissions.mode === m.mode}
                onChange={() => update((d) => void (d.permissions.mode = m.mode))}
              />
              <strong>{m.label}</strong> <span className="small muted">{m.help}</span>
            </label>
          ))}
        </div>
        <div className="grid-2 mt">
          {(["allow", "ask", "deny"] as const).map((list) => (
            <label className="field" key={list}>
              <span className="label">{list[0].toUpperCase() + list.slice(1)} rules, one per line</span>
              <textarea
                className="mono"
                rows={5}
                value={lines(s.permissions[list])}
                onChange={(e) => update((d) => void (d.permissions[list] = unlines(e.target.value)))}
              />
            </label>
          ))}
        </div>
        <Issues issues={issues} prefix="permissions" />
      </div>

      <div className="card">
        <h2>YOLO and notifications</h2>
        <label className="check">
          <input type="checkbox" checked={s.yolo.default} onChange={(e) => update((d) => void (d.yolo.default = e.target.checked))} />
          Start new sessions with YOLO on
        </label>
        <p className="small muted">
          Under YOLO, Ostra grants every permission ask and answers every gate itself, then lists each decision in the completion
          report. Guards, deny rules, the fact-check PASS requirement, and security blocks still apply. You can toggle it per session.
        </p>
        <label className="check">
          <input
            type="checkbox"
            checked={s.notifications.push}
            onChange={(e) => update((d) => void (d.notifications.push = e.target.checked))}
          />
          Send push notifications when a gate waits, a session completes, a phase is blocked, or a harness needs login
        </label>
        {pushSupported() && (
          <div className="row mt">
            <button
              className="small"
              onClick={async () => {
                setPushMsg((await enablePush()) ?? "This browser will receive notifications.");
              }}
            >
              Enable in this browser
            </button>
            <button
              className="small ghost"
              onClick={async () => {
                await disablePush();
                setPushMsg("This browser no longer receives notifications.");
              }}
            >
              Disable in this browser
            </button>
            {pushMsg && <span className="small">{pushMsg}</span>}
          </div>
        )}
      </div>

      <div className="card row between" style={{ position: "sticky", bottom: 0 }}>
        <div className="small">
          {validating
            ? "Checking settings…"
            : issues.length > 0
              ? `${issues.length} problem${issues.length === 1 ? "" : "s"} to fix before saving.`
              : dirty
                ? "Unsaved changes."
                : "No changes."}
          {message && <span className="muted"> {message}</span>}
          {error && <span className="form-error"> {error}</span>}
        </div>
        <div className="row">
          <button disabled={!dirty} onClick={() => setS(structuredClone(detail.settings))}>
            Discard
          </button>
          <button className="primary" disabled={!dirty || saving || validating || issues.length > 0} onClick={() => void save()}>
            Save settings
          </button>
        </div>
      </div>
      {issues.length > 0 && (
        <div className="card">
          <h3>All problems</h3>
          <ul className="issues small">
            {issues.map((i, n) => (
              <li key={n}>
                <span className="mono">{i.path}</span>: {i.message}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

export function SettingsPage() {
  const { ws = "" } = useParams();
  const detail = useAsync(() => api.workspace(ws), [ws]);
  useCrumbs([{ label: detail.data?.settings.name ?? "Workspace", to: `/w/${ws}` }, { label: "Settings" }]);
  if (detail.error) return <ErrorBox error={detail.error} onRetry={detail.reload} />;
  if (!detail.data) return <Loading />;
  return (
    <div className="page narrow" style={{ maxWidth: 1000 }}>
      <h1>{detail.data.settings.name}</h1>
      <WorkspaceTabs ws={ws} />
      <SettingsForm key={JSON.stringify(detail.data.settings)} detail={detail.data} onSaved={detail.set} />
    </div>
  );
}
