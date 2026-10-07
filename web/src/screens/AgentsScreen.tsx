import { Banner, Button, Checkbox, Chip, Input, Panel, Select } from "@ostra/design";
import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { AgentDetail, AgentDoc, AgentInfo, Capability, PluginInfo, Tier, WriteScope } from "../api/types";
import { SchemaBuilder } from "../features/schema/SchemaBuilder";
import { useAsync } from "../lib/hooks";
import { useWorkspace } from "../lib/nav";
import { LoadError, Loading, Page } from "./workspace/Page";
import "./agents.css";

const SOURCE_LABEL: Record<AgentInfo["source"]["kind"], string> = {
  ostra: "Ostra",
  workspace: "Workspace",
  plugin: "Plugin",
};

const CAPABILITIES: Capability[] = [
  "read",
  "search_text",
  "glob",
  "edit",
  "write",
  "shell",
  "skill",
  "report",
  "coordinate",
  "web_search",
  "web_fetch",
  "code",
  "memory",
  "memory_recall",
  "docs_search",
  "document_research",
  "document_spec",
  "document_plan",
  "review_ledger",
  "security_block",
  "progress_log",
  "test_files",
  "manage_projects",
];

const TIERS: Tier[] = ["fast", "balanced", "advanced", "frontier"];
const SCOPES: WriteScope[] = ["read_only", "session", "project", "setup"];

const blankDoc = (): AgentDoc => ({
  name: "",
  description: "",
  returns: "stage",
  default_tier: "balanced",
  capabilities: ["read", "search_text", "glob", "report", "coordinate"],
  write_scope: "session",
  brief: [],
  timeout_seconds: 1200,
  effort: {},
  data_schema: null,
  helper: false,
  prompt: "",
});

type Selection = { name: string } | { creating: AgentDoc };

/** Rules AG1 to AG3: every agent the workspace can run, the editor for its own, and its plugins. */
export function AgentsScreen({ ws }: { ws: string }) {
  const { detail, reload } = useWorkspace();
  const plugins = useAsync(() => api.plugins(ws), [ws]);
  const [selected, setSelected] = useState<Selection | null>(null);
  const [query, setQuery] = useState("");

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const agents = (detail?.agents ?? []).filter(
      (a) => !q || `${a.name} ${a.description} ${a.returns}`.toLowerCase().includes(q),
    );
    return (["workspace", "plugin", "ostra"] as const)
      .map((kind) => ({ kind, agents: agents.filter((a) => a.source.kind === kind) }))
      .filter((g) => g.agents.length > 0);
  }, [detail, query]);

  if (!detail) {
    return (
      <Page title="Agents">
        <Loading>Reading the agents…</Loading>
      </Page>
    );
  }

  return (
    <Page
      title="Agents"
      sub="Ostra's agents, the workspace's own, and the agents its plugins add. Workflow nodes run any of them."
      actions={
        <Button size="sm" icon="plus" onClick={() => setSelected({ creating: blankDoc() })}>
          New agent
        </Button>
      }
    >
      <div className="wp-row" style={{ alignItems: "flex-start", gap: 16 }}>
        <div className="wp-stack" style={{ flex: "0 0 340px", gap: 12 }}>
          <Input
            aria-label="Filter agents"
            placeholder="Filter agents"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {groups.map((g) => (
            <Panel key={g.kind} title={SOURCE_LABEL[g.kind]}>
              <div className="wp-stack" style={{ gap: 4 }}>
                {g.agents.map((a) => (
                  <Button
                    key={a.name}
                    size="sm"
                    variant={
                      "name" in (selected ?? {}) && (selected as { name: string }).name === a.name ? "default" : "ghost"
                    }
                    onClick={() => setSelected({ name: a.name })}
                    style={{ justifyContent: "space-between", width: "100%" }}
                  >
                    <span style={{ fontFamily: "var(--font-mono)" }}>{a.name}</span>
                    <span className="wp-muted">{a.returns}</span>
                  </Button>
                ))}
              </div>
            </Panel>
          ))}
          <PluginsPanel plugins={plugins.data} error={plugins.error} onRetry={plugins.reload} />
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          {selected === null && <div className="wp-muted">Pick an agent to see its definition.</div>}
          {selected && "name" in selected && (
            <AgentPanel
              key={selected.name}
              ws={ws}
              name={selected.name}
              onDuplicate={(doc) => setSelected({ creating: { ...doc, name: `${doc.name}-copy` } })}
              onChanged={(name) => {
                reload();
                setSelected(name ? { name } : null);
              }}
            />
          )}
          {selected && "creating" in selected && (
            <AgentEditor
              ws={ws}
              initial={selected.creating}
              creating
              onSaved={(d) => {
                reload();
                setSelected({ name: d.info.name });
              }}
              onCancel={() => setSelected(null)}
            />
          )}
        </div>
      </div>
    </Page>
  );
}

function AgentPanel({
  ws,
  name,
  onDuplicate,
  onChanged,
}: {
  ws: string;
  name: string;
  onDuplicate: (doc: AgentDoc) => void;
  onChanged: (name: string | null) => void;
}) {
  const agent = useAsync(() => api.agent(ws, name), [ws, name]);
  const [editing, setEditing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (agent.error) return <LoadError error={agent.error} onRetry={agent.reload} />;
  if (!agent.data) return <Loading />;
  const d = agent.data;
  if (editing) {
    return (
      <AgentEditor
        ws={ws}
        initial={d.doc}
        onSaved={(saved) => {
          agent.set(saved);
          setEditing(false);
          onChanged(saved.info.name);
        }}
        onCancel={() => setEditing(false)}
      />
    );
  }
  const remove = async () => {
    setError(null);
    try {
      await api.deleteAgent(ws, name);
      onChanged(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };
  return (
    <Panel
      title={d.info.label}
      icon="bot"
      actions={
        <div className="wp-row" style={{ gap: 6 }}>
          {d.editable && (
            <Button size="sm" icon="pencil-line" onClick={() => setEditing(true)}>
              Edit
            </Button>
          )}
          <Button size="sm" icon="copy" onClick={() => onDuplicate(d.doc)}>
            Duplicate
          </Button>
          {d.editable && (
            <Button size="sm" icon="trash-2" variant="ghost" onClick={() => void remove()}>
              Delete
            </Button>
          )}
        </div>
      }
    >
      <AgentSummary d={d} />
      {error && <Banner tone="bad">{error}</Banner>}
    </Panel>
  );
}

function AgentSummary({ d }: { d: AgentDetail }) {
  const i = d.info;
  const source =
    i.source.kind === "workspace" ? i.source.file : i.source.kind === "plugin" ? `plugin ${i.source.plugin}` : "Ostra";
  return (
    <div className="wp-stack" style={{ gap: 10 }}>
      {d.waiting_approval && (
        <Banner tone="warn">Its file waits for the workspace's approval in Settings, so no session runs it yet.</Banner>
      )}
      <div>{i.description}</div>
      <div className="wp-row" style={{ gap: 6, flexWrap: "wrap" }}>
        <Chip>{source}</Chip>
        <Chip>returns {i.returns}</Chip>
        <Chip>writes {i.write_scope}</Chip>
        <Chip>{i.resolved ? `${i.resolved.executor} ${i.resolved.model}` : "no route"}</Chip>
        {i.helper && <Chip>helper</Chip>}
        {i.programmatic && <Chip>runs in code</Chip>}
      </div>
      <div className="wp-muted">Capabilities: {i.capabilities.join(", ")}</div>
      {d.used_by.length > 0 && <div className="wp-muted">Used by: {d.used_by.join(", ")}</div>}
      {d.doc.data_schema != null && (
        <details>
          <summary>Output fields (data schema)</summary>
          <pre className="wp-pre">{JSON.stringify(d.doc.data_schema, null, 2)}</pre>
        </details>
      )}
      <details>
        <summary>Submit schema</summary>
        <pre className="wp-pre">{JSON.stringify(d.submit_schema, null, 2)}</pre>
      </details>
      {d.prompt_preview && (
        <details>
          <summary>System prompt (native executor)</summary>
          <pre className="wp-pre" style={{ whiteSpace: "pre-wrap" }}>
            {d.prompt_preview}
          </pre>
        </details>
      )}
    </div>
  );
}

/** Rule AG2: the editor for a workspace agent's file. */
function AgentEditor({
  ws,
  initial,
  creating,
  onSaved,
  onCancel,
}: {
  ws: string;
  initial: AgentDoc;
  creating?: boolean;
  onSaved: (d: AgentDetail) => void;
  onCancel: () => void;
}) {
  const [doc, setDoc] = useState<AgentDoc>(initial);
  const [schema, setSchema] = useState<{ value: unknown; problem: string | null }>({
    value: initial.data_schema,
    problem: null,
  });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    setDoc(initial);
    setSchema({ value: initial.data_schema, problem: null });
  }, [initial]);
  const set = <K extends keyof AgentDoc>(k: K, v: AgentDoc[K]) => setDoc((d) => ({ ...d, [k]: v }));

  const save = async () => {
    setError(schema.problem);
    if (schema.problem) return;
    setBusy(true);
    try {
      onSaved(await api.saveAgent(ws, doc.name, { ...doc, data_schema: schema.value ?? null }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const toggle = (c: Capability) =>
    set(
      "capabilities",
      doc.capabilities.includes(c) ? doc.capabilities.filter((x) => x !== c) : [...doc.capabilities, c],
    );

  return (
    <Panel title={creating ? "New agent" : `Edit ${doc.name}`} icon="bot">
      <div className="wp-stack" style={{ gap: 10 }}>
        <Input
          label="Name"
          value={doc.name}
          disabled={!creating}
          placeholder="security-auditor"
          onChange={(e) => set("name", e.target.value)}
        />
        <Input label="Description" value={doc.description} onChange={(e) => set("description", e.target.value)} />
        <div className="ag-fields">
          <Input
            label="Returns"
            value={doc.returns}
            title="stage for a workflow node, a built-in contract such as review, or <plugin>:<contract>"
            onChange={(e) => set("returns", e.target.value)}
          />
          <Select
            label="Tier"
            value={doc.default_tier ?? "balanced"}
            onChange={(e) => set("default_tier", e.target.value as Tier)}
            options={TIERS}
          />
          <Select
            label="Writes"
            value={doc.write_scope ?? "session"}
            onChange={(e) => set("write_scope", e.target.value as WriteScope)}
            options={SCOPES}
          />
          <Input
            label="Timeout (seconds)"
            type="number"
            value={String(doc.timeout_seconds ?? 1200)}
            onChange={(e) => set("timeout_seconds", Number(e.target.value) || null)}
          />
          <div className="ag-check">
            <Checkbox
              label="Helper"
              title="SendMessage may start this agent as a helper"
              checked={doc.helper}
              onChange={(e) => set("helper", e.target.checked)}
            />
          </div>
        </div>
        <fieldset className="ag-caps">
          <legend>Capabilities</legend>
          {CAPABILITIES.map((c) => (
            <Checkbox key={c} label={c} checked={doc.capabilities.includes(c)} onChange={() => toggle(c)} />
          ))}
        </fieldset>
        <SchemaBuilder
          key={JSON.stringify(initial.data_schema ?? null) + initial.name}
          label="Data schema"
          hint="The output fields of an agent that returns stage. Leave it empty for none."
          value={initial.data_schema}
          onChange={(value, problem) => setSchema({ value, problem })}
        />
        <Input
          label="Prompt"
          multiline
          rows={14}
          value={doc.prompt}
          placeholder="Read every file the change touched with {{ tool_read }} ..."
          onChange={(e) => set("prompt", e.target.value)}
        />
        {error && <Banner tone="bad">{error}</Banner>}
        <div className="wp-row" style={{ gap: 8 }}>
          <Button variant="primary" disabled={busy || !doc.name.trim()} onClick={() => void save()}>
            {busy ? "Saving…" : "Save"}
          </Button>
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </div>
    </Panel>
  );
}

const STATE_TONE: Record<PluginInfo["state"], "ok" | "warn" | "bad" | "neutral"> = {
  running: "ok",
  starting: "neutral",
  disabled: "neutral",
  waiting_approval: "warn",
  failed: "bad",
};

/** Rule AG3: the workspace's plugins and what each is doing. */
function PluginsPanel({
  plugins,
  error,
  onRetry,
}: {
  plugins: PluginInfo[] | null;
  error: Error | null;
  onRetry: () => void;
}) {
  return (
    <Panel title="Plugins" icon="plug-zap">
      {error && <LoadError error={error} onRetry={onRetry} />}
      {plugins?.length === 0 && (
        <div className="wp-muted">No plugins. Add one under `[[plugins]]` in the workspace file.</div>
      )}
      <div className="wp-stack" style={{ gap: 8 }}>
        {plugins?.map((p) => (
          <div key={p.name} className="wp-stack" style={{ gap: 2 }}>
            <div className="wp-row" style={{ gap: 6 }}>
              <span style={{ fontFamily: "var(--font-mono)" }}>{p.name}</span>
              <Chip tone={STATE_TONE[p.state]}>{p.builtin ? "built in" : p.state.replace("_", " ")}</Chip>
            </div>
            {p.error && <div className="wp-muted">{p.error}</div>}
            {p.manifest && (
              <div className="wp-muted">
                {p.manifest.agents.length} agents, {p.manifest.stages.length} stages, {p.manifest.contracts.length}{" "}
                contracts
              </div>
            )}
          </div>
        ))}
      </div>
    </Panel>
  );
}
