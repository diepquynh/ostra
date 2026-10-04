import { Banner, Button, Chip, Input, Select } from "@ostra/design";
import {
  applyNodeChanges,
  Background,
  type Connection,
  Controls,
  type Edge,
  Handle,
  MiniMap,
  type Node,
  type NodeChange,
  type NodeProps,
  Position,
  ReactFlow,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useEffect, useMemo, useState } from "react";
import { api } from "../../api";
import type {
  AgentInfo,
  BuilderPalette,
  Condition,
  CondOp,
  Contract,
  OnFail,
  StageFile,
  StageScope,
  Tier,
  TransformInfo,
  WorkflowFile,
} from "../../api/types";
import { useShell } from "../../lib/nav";
import "./builder.css";

type Kind = "builtin" | "agent" | "plugin" | "transform" | "prompt";

export function kindOf(s: StageFile): Kind {
  if (s.uses) return "builtin";
  if (s.agent) return "agent";
  if (s.plugin) return "plugin";
  if (s.transform) return "transform";
  return "prompt";
}

const KIND_LABEL: Record<Kind, string> = {
  builtin: "Ostra stage",
  agent: "Agent",
  plugin: "Plugin stage",
  transform: "Transform",
  prompt: "Prompt",
};

const subtitle = (s: StageFile) => s.uses ?? s.agent ?? s.plugin ?? s.transform ?? (s.prompt ? "model call" : "");

/** The column of each node: one more than the deepest node it waits for. */
export function autoLayout(stages: StageFile[]): Record<string, [number, number]> {
  const depth: Record<string, number> = {};
  const visit = (id: string, seen: Set<string>): number => {
    if (depth[id] !== undefined) return depth[id];
    if (seen.has(id)) return 0;
    seen.add(id);
    const s = stages.find((x) => x.id === id);
    const d = Math.max(-1, ...(s?.after ?? []).map((a) => visit(a, seen))) + 1;
    depth[id] = d;
    return d;
  };
  for (const s of stages) visit(s.id, new Set());
  const rows: Record<number, number> = {};
  const out: Record<string, [number, number]> = {};
  for (const s of stages) {
    const d = depth[s.id] ?? 0;
    const r = rows[d] ?? 0;
    rows[d] = r + 1;
    out[s.id] = [d * 240, r * 120];
  }
  return out;
}

type NodeData = { stage: StageFile; problem: boolean };

function StageNode({ data, selected }: NodeProps<Node<NodeData>>) {
  const s = data.stage;
  const kind = kindOf(s);
  return (
    <div
      className={`wb-node wb-node--${kind}${selected ? " wb-node--selected" : ""}${data.problem ? " wb-node--problem" : ""}`}
    >
      <Handle type="target" position={Position.Left} />
      <div className="wb-node__kind">{KIND_LABEL[kind]}</div>
      <div className="wb-node__id">{s.id}</div>
      <div className="wb-node__sub">{subtitle(s)}</div>
      {(s.when?.length ?? 0) > 0 && (
        <div className="wb-node__when" title={s.when?.map(condText).join(s.when_mode === "any" ? " or " : " and ")}>
          when {s.when?.length === 1 ? condText(s.when[0]) : `${s.when?.length} conditions`}
        </div>
      )}
      <Handle type="source" position={Position.Right} />
    </div>
  );
}

const nodeTypes = { stage: StageNode };

const condText = (c: Condition) =>
  `${c.ref} ${c.op}${c.value !== undefined && c.value !== null ? ` ${JSON.stringify(c.value)}` : ""}`;

const parseValue = (text: string): unknown => {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
};

const showValue = (v: unknown) => (v === undefined || v === null ? "" : typeof v === "string" ? v : JSON.stringify(v));

function uniqueId(base: string, stages: StageFile[]): string {
  const clean =
    base
      .toLowerCase()
      .replace(/[^a-z0-9-]+/g, "-")
      .replace(/^-+|-+$/g, "") || "node";
  const start = /^[a-z]/.test(clean) ? clean : `n-${clean}`;
  let id = start;
  for (let i = 2; stages.some((s) => s.id === id); i++) id = `${start}-${i}`;
  return id;
}

export type WorkflowBuilderProps = {
  ws: string;
  name: string;
  initial: WorkflowFile;
  palette: BuilderPalette;
  agents: AgentInfo[];
  readOnly?: boolean;
  onSaved: (name: string) => void;
};

/** Rule WB1: a workflow as a diagram. Each edge is an `after` entry; positions go to `[layout]`. */
export function WorkflowBuilder({ ws, name, initial, palette, agents, readOnly, onSaved }: WorkflowBuilderProps) {
  const { theme } = useShell();
  const [file, setFile] = useState<WorkflowFile>(initial);
  const [pos, setPos] = useState<Record<string, [number, number]>>(() => ({
    ...autoLayout(initial.stage ?? []),
    ...(initial.layout ?? {}),
  }));
  const [selected, setSelected] = useState<string | null>(null);
  // The diagram shows a node only once it knows its size, so sizes survive rebuilding the nodes.
  const [measured, setMeasured] = useState<Record<string, { width: number; height: number }>>({});
  const [issues, setIssues] = useState<string[]>([]);
  // Rule WB6: what a save would be refused for, checked by the server while the user edits.
  const [problems, setProblems] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const stages = file.stage ?? [];

  useEffect(() => {
    if (readOnly) return;
    let live = true;
    const t = setTimeout(() => {
      api
        .checkWorkflow(ws, name, file)
        .then((p) => live && setProblems(p))
        .catch(() => {});
    }, 500);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [ws, name, file, readOnly]);

  const setStages = (f: (s: StageFile[]) => StageFile[]) => {
    setSaved(false);
    setFile((w) => ({ ...w, stage: f(w.stage ?? []) }));
  };
  const update = (id: string, patch: Partial<StageFile>) =>
    setStages((ss) => ss.map((s) => (s.id === id ? { ...s, ...patch } : s)));

  const nodes: Node<NodeData>[] = useMemo(
    () =>
      stages.map((s) => ({
        id: s.id,
        type: "stage",
        position: { x: pos[s.id]?.[0] ?? 0, y: pos[s.id]?.[1] ?? 0 },
        data: { stage: s, problem: problems.some((p) => p.includes(`\`${s.id}\``)) },
        selected: s.id === selected,
        measured: measured[s.id],
      })),
    [stages, pos, selected, measured, problems],
  );
  const edges: Edge[] = useMemo(
    () =>
      stages.flatMap((s) =>
        (s.after ?? []).map((a) => ({
          id: `${a}->${s.id}`,
          source: a,
          target: s.id,
          animated: (s.when?.length ?? 0) > 0,
        })),
      ),
    [stages],
  );

  const onNodesChange = (changes: NodeChange<Node<NodeData>>[]) => {
    const moved = applyNodeChanges(changes, nodes);
    setPos((p) => {
      const next = { ...p };
      for (const n of moved) next[n.id] = [Math.round(n.position.x), Math.round(n.position.y)];
      return next;
    });
    for (const c of changes) if (c.type === "select" && c.selected) setSelected(c.id);
    const sizes = changes.flatMap((c) =>
      c.type === "dimensions" && c.dimensions ? [[c.id, c.dimensions] as const] : [],
    );
    if (sizes.length > 0) setMeasured((m) => ({ ...m, ...Object.fromEntries(sizes) }));
  };

  const onConnect = (c: Connection) => {
    if (readOnly || !c.source || !c.target || c.source === c.target) return;
    setStages((ss) =>
      ss.map((s) =>
        s.id === c.target && !(s.after ?? []).includes(c.source) ? { ...s, after: [...(s.after ?? []), c.source] } : s,
      ),
    );
  };

  const removeEdges = (gone: Edge[]) =>
    setStages((ss) =>
      ss.map((s) => ({
        ...s,
        after: (s.after ?? []).filter((a) => !gone.some((e) => e.source === a && e.target === s.id)),
      })),
    );

  const removeNodes = (ids: string[]) => {
    setStages((ss) =>
      ss
        .filter((s) => !ids.includes(s.id))
        .map((s) => ({ ...s, after: (s.after ?? []).filter((a) => !ids.includes(a)) })),
    );
    if (selected && ids.includes(selected)) setSelected(null);
  };

  const add = (base: string, stage: Omit<StageFile, "id" | "after">) => {
    if (readOnly) return;
    const id = uniqueId(base, stages);
    const after = selected ? [selected] : [];
    const at = selected && pos[selected] ? pos[selected] : [0, 0];
    setPos((p) => ({ ...p, [id]: [at[0] + 240, at[1] + 40] }));
    setStages((ss) => [...ss, { id, after, ...stage }]);
    setSelected(id);
  };

  const rename = (from: string, to: string) => {
    const id = uniqueId(
      to,
      stages.filter((s) => s.id !== from),
    );
    const swap = (ref: string) => (ref === from || ref.startsWith(`${from}.`) ? id + ref.slice(from.length) : ref);
    setStages((ss) =>
      ss.map((s) => ({
        ...s,
        id: s.id === from ? id : s.id,
        after: (s.after ?? []).map((a) => (a === from ? id : a)),
        inputs: s.inputs ? Object.fromEntries(Object.entries(s.inputs).map(([k, v]) => [k, swap(v)])) : s.inputs,
        when: s.when?.map((c) => ({ ...c, ref: swap(c.ref) })),
      })),
    );
    setPos((p) => {
      const { [from]: at, ...rest } = p;
      return at ? { ...rest, [id]: at } : rest;
    });
    setSelected(id);
  };

  const save = async () => {
    setBusy(true);
    setIssues([]);
    try {
      const layout = Object.fromEntries(stages.map((s) => [s.id, pos[s.id] ?? [0, 0]])) as Record<
        string,
        [number, number]
      >;
      const doc = await api.saveWorkflow(ws, name, { ...file, layout });
      setFile(doc.file);
      setSaved(true);
      onSaved(name);
    } catch (e) {
      const err = e as Error & { issues?: { message: string }[] };
      setIssues(err.issues?.length ? err.issues.map((i) => i.message) : [err.message]);
    } finally {
      setBusy(false);
    }
  };

  const current = stages.find((s) => s.id === selected) ?? null;
  const stageAgents = agents.filter((a) => a.returns === "stage" || a.returns.includes(":"));
  const present = new Set(stages.map((s) => s.uses).filter(Boolean));

  return (
    <div className="wb">
      <div className="wb-bar">
        <span className="wb-bar__name">{name}</span>
        <Input
          aria-label="Description"
          placeholder="What this workflow is for"
          value={file.description ?? ""}
          disabled={readOnly}
          onChange={(e) => setFile((w) => ({ ...w, description: e.target.value }))}
          style={{ flex: 1 }}
        />
        <Chip>base {file.base}</Chip>
        {!readOnly && (
          <Button variant="primary" icon="check" disabled={busy} onClick={() => void save()}>
            {busy ? "Saving…" : saved ? "Saved" : "Save"}
          </Button>
        )}
      </div>
      {problems.length > 0 && issues.length === 0 && (
        <Banner
          tone="warn"
          title={`${problems.length} problem${problems.length === 1 ? "" : "s"} to fix before saving`}
        >
          {problems.map((p) => (
            <div key={p}>{p}</div>
          ))}
        </Banner>
      )}
      {issues.length > 0 && (
        <Banner tone="bad">
          {issues.map((i) => (
            <div key={i}>{i}</div>
          ))}
        </Banner>
      )}
      <div className="wb-main">
        {!readOnly && (
          <aside className="wb-palette" aria-label="Node palette">
            <PaletteGroup title="Ostra stages">
              {palette.builtin_stages
                .filter((b) => !present.has(b.uses))
                .map((b) => (
                  <PaletteItem
                    key={b.uses}
                    label={b.stage}
                    title={b.description}
                    onAdd={() => add(b.stage, { uses: b.uses })}
                  />
                ))}
            </PaletteGroup>
            <PaletteGroup title="Agents">
              {stageAgents.map((a) => (
                <PaletteItem
                  key={a.name}
                  label={a.name}
                  title={a.description}
                  onAdd={() => add(a.name, { agent: a.name })}
                />
              ))}
            </PaletteGroup>
            {palette.plugin_stages.length > 0 && (
              <PaletteGroup title="Plugin stages">
                {palette.plugin_stages.map((p) => (
                  <PaletteItem
                    key={`${p.plugin}:${p.stage}`}
                    label={`${p.plugin}:${p.stage}`}
                    title={p.description}
                    onAdd={() => add(p.stage, { plugin: `${p.plugin}:${p.stage}` })}
                  />
                ))}
              </PaletteGroup>
            )}
            <PaletteGroup title="Transforms">
              {palette.transforms
                .filter((t) => !t.custom && !t.plugin)
                .map((t) => (
                  <PaletteItem
                    key={t.name}
                    label={t.name}
                    title={t.description}
                    onAdd={() => add(t.name, { transform: t.name, inputs: {}, args: {} })}
                  />
                ))}
            </PaletteGroup>
            {palette.transforms.some((t) => t.plugin) && (
              <PaletteGroup title="Plugin transforms">
                {palette.transforms
                  .filter((t) => t.plugin)
                  .map((t) => (
                    <PaletteItem
                      key={t.name}
                      label={t.name}
                      title={t.description}
                      onAdd={() => add(t.name.split(":")[1] ?? t.name, { transform: t.name, inputs: {}, args: {} })}
                    />
                  ))}
              </PaletteGroup>
            )}
            {palette.transforms.some((t) => t.custom) && (
              <PaletteGroup title="Your transforms">
                {palette.transforms
                  .filter((t) => t.custom)
                  .map((t) => (
                    <PaletteItem
                      key={t.name}
                      label={t.name}
                      title={t.description}
                      onAdd={() => add(t.name, { transform: t.name, inputs: {}, args: {} })}
                    />
                  ))}
              </PaletteGroup>
            )}
            <PaletteGroup title="Model">
              <PaletteItem
                label="prompt"
                title="One model call that answers with JSON matching an output schema."
                onAdd={() =>
                  add("prompt", {
                    prompt: "",
                    inputs: {},
                    output_schema: { type: "object", required: ["answer"], properties: { answer: { type: "string" } } },
                  })
                }
              />
            </PaletteGroup>
          </aside>
        )}
        <div className="wb-canvas">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            onNodesChange={onNodesChange}
            onConnect={onConnect}
            onEdgesDelete={readOnly ? undefined : removeEdges}
            onNodesDelete={readOnly ? undefined : (ns) => removeNodes(ns.map((n) => n.id))}
            onPaneClick={() => setSelected(null)}
            nodesConnectable={!readOnly}
            deleteKeyCode={readOnly ? null : ["Backspace", "Delete"]}
            fitView
            colorMode={theme}
            proOptions={{ hideAttribution: true }}
          >
            <Background />
            <Controls />
            <MiniMap pannable zoomable />
          </ReactFlow>
        </div>
        {current && (
          <Inspector
            key={current.id}
            stage={current}
            stages={stages}
            palette={palette}
            agents={agents}
            readOnly={readOnly}
            onChange={(patch) => update(current.id, patch)}
            onRename={(to) => rename(current.id, to)}
            onDelete={() => removeNodes([current.id])}
          />
        )}
      </div>
    </div>
  );
}

function PaletteGroup({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="wb-palette__group">
      <div className="wb-palette__title">{title}</div>
      {children}
    </div>
  );
}

function PaletteItem({ label, title, onAdd }: { label: string; title: string; onAdd: () => void }) {
  return (
    <button type="button" className="wb-palette__item" title={title} onClick={onAdd}>
      + {label}
    </button>
  );
}

const SCOPES: StageScope[] = ["session", "project", "phase"];
const ON_FAIL: OnFail[] = ["gate", "retry", "continue", "fail"];

function Inspector({
  stage,
  stages,
  palette,
  agents,
  readOnly,
  onChange,
  onRename,
  onDelete,
}: {
  stage: StageFile;
  stages: StageFile[];
  palette: BuilderPalette;
  agents: AgentInfo[];
  readOnly?: boolean;
  onChange: (patch: Partial<StageFile>) => void;
  onRename: (to: string) => void;
  onDelete: () => void;
}) {
  const kind = kindOf(stage);
  const [id, setId] = useState(stage.id);
  const fn: TransformInfo | undefined = palette.transforms.find((t) => t.name === stage.transform);
  const builtin = palette.builtin_stages.find((b) => b.uses === stage.uses);
  const refs = [
    ...stages.filter((s) => s.id !== stage.id).map((s) => `${s.id}.output`),
    "session.track",
    "session.category",
    "scope.project",
  ];
  const listId = `wb-refs-${stage.id}`;
  return (
    <aside className="wb-inspector" aria-label="Node settings">
      <div className="wb-inspector__head">
        <Chip>{KIND_LABEL[kind]}</Chip>
        {!readOnly && (
          <Button size="sm" variant="ghost" icon="trash-2" onClick={onDelete}>
            Remove
          </Button>
        )}
      </div>
      <datalist id={listId}>
        {refs.map((r) => (
          <option key={r} value={r} />
        ))}
      </datalist>
      <Input
        label="Node id"
        value={id}
        disabled={readOnly}
        onChange={(e) => setId(e.target.value)}
        onBlur={() => id !== stage.id && onRename(id)}
      />
      <div className="wp-muted">{subtitle(stage)}</div>
      {builtin && <div className="wp-muted">{builtin.description}</div>}

      {kind === "builtin" &&
        builtin?.contracts.map((c: Contract) => (
          <Select
            key={c}
            aria-label={`Agent for ${c}`}
            value={stage.agents?.[c] ?? ""}
            disabled={readOnly}
            onChange={(e) => {
              const next = { ...(stage.agents ?? {}) };
              if (e.target.value) next[c] = e.target.value;
              else delete next[c];
              onChange({ agents: next });
            }}
            options={[
              { value: "", label: `${c}: Ostra's agent` },
              ...agents.filter((a) => a.returns === c).map((a) => ({ value: a.name, label: `${c}: ${a.name}` })),
            ]}
          />
        ))}

      {kind === "prompt" && (
        <>
          <Input
            label="Prompt"
            multiline
            rows={6}
            value={stage.prompt ?? ""}
            disabled={readOnly}
            onChange={(e) => onChange({ prompt: e.target.value })}
          />
          <Select
            aria-label="Tier"
            value={stage.tier ?? "balanced"}
            disabled={readOnly}
            onChange={(e) => onChange({ tier: e.target.value as Tier })}
            options={palette.tiers.map((t) => ({ value: t, label: `Tier: ${t}` }))}
          />
          <JsonField
            label="Output schema"
            value={stage.output_schema}
            readOnly={readOnly}
            onChange={(v) => onChange({ output_schema: v })}
          />
        </>
      )}

      {(kind === "agent" || kind === "plugin") && (
        <Input
          label="Instructions"
          multiline
          rows={4}
          value={stage.instructions ?? ""}
          disabled={readOnly}
          onChange={(e) => onChange({ instructions: e.target.value || null })}
        />
      )}

      {kind !== "builtin" && (
        <>
          <div className="wb-inspector__title">Inputs</div>
          <InputsEditor
            stage={stage}
            fn={fn}
            listId={listId}
            readOnly={readOnly}
            onChange={(inputs) => onChange({ inputs })}
          />
          {fn && fn.args.length > 0 && (
            <>
              <div className="wb-inspector__title">Arguments</div>
              {fn.args.map((p) => (
                <Input
                  key={p.name}
                  label={`${p.name}${p.required ? "" : " (optional)"}: ${p.kind}`}
                  title={p.description}
                  value={showValue(stage.args?.[p.name])}
                  disabled={readOnly}
                  onChange={(e) => {
                    const next = { ...(stage.args ?? {}) };
                    if (e.target.value === "") delete next[p.name];
                    else next[p.name] = p.kind === "string" ? e.target.value : parseValue(e.target.value);
                    onChange({ args: next });
                  }}
                />
              ))}
            </>
          )}
          <div className="wb-inspector__title">Runs when</div>
          <ConditionsEditor
            stage={stage}
            ops={palette.cond_ops}
            listId={listId}
            readOnly={readOnly}
            onChange={onChange}
          />
          <div className="wb-fields">
            <Select
              label="Runs once per"
              value={stage.scope ?? "session"}
              disabled={readOnly}
              onChange={(e) => onChange({ scope: e.target.value as StageScope })}
              options={SCOPES}
            />
            <Select
              label="On failure"
              value={stage.on_fail ?? "gate"}
              disabled={readOnly}
              onChange={(e) => onChange({ on_fail: e.target.value as OnFail })}
              options={ON_FAIL}
            />
            <Input
              label="Max rounds"
              type="number"
              value={String(stage.max_rounds ?? 3)}
              disabled={readOnly}
              onChange={(e) => onChange({ max_rounds: Number(e.target.value) || 3 })}
            />
          </div>
        </>
      )}
    </aside>
  );
}

function InputsEditor({
  stage,
  fn,
  listId,
  readOnly,
  onChange,
}: {
  stage: StageFile;
  fn?: TransformInfo;
  listId: string;
  readOnly?: boolean;
  onChange: (inputs: Record<string, string>) => void;
}) {
  const inputs = stage.inputs ?? {};
  const names = fn && !fn.variadic ? fn.inputs.map((p) => p.name) : Object.keys(inputs);
  const [fresh, setFresh] = useState("");
  return (
    <div className="wp-stack" style={{ gap: 6 }}>
      {names.map((n) => (
        <div key={n} className="wp-row" style={{ gap: 6 }}>
          <Input
            label={n}
            list={listId}
            placeholder="node.output"
            value={inputs[n] ?? ""}
            disabled={readOnly}
            onChange={(e) => {
              const next = { ...inputs };
              if (e.target.value) next[n] = e.target.value;
              else delete next[n];
              onChange(next);
            }}
          />
          {(!fn || fn.variadic) && !readOnly && (
            <Button
              size="sm"
              variant="ghost"
              icon="x"
              aria-label={`Remove input ${n}`}
              onClick={() => {
                const { [n]: _, ...rest } = inputs;
                onChange(rest);
              }}
            />
          )}
        </div>
      ))}
      {(!fn || fn.variadic) && !readOnly && (
        <div className="wp-row" style={{ gap: 6 }}>
          <Input
            aria-label="New input name"
            placeholder="input name"
            value={fresh}
            onChange={(e) => setFresh(e.target.value)}
          />
          <Button
            size="sm"
            icon="plus"
            disabled={!/^[a-zA-Z][a-zA-Z0-9_]*$/.test(fresh)}
            onClick={() => {
              onChange({ ...inputs, [fresh]: "" });
              setFresh("");
            }}
          >
            Add input
          </Button>
        </div>
      )}
    </div>
  );
}

const TAKES_VALUE: CondOp[] = ["eq", "ne", "gt", "ge", "lt", "le", "contains", "in"];

function ConditionsEditor({
  stage,
  ops,
  listId,
  readOnly,
  onChange,
}: {
  stage: StageFile;
  ops: CondOp[];
  listId: string;
  readOnly?: boolean;
  onChange: (patch: Partial<StageFile>) => void;
}) {
  const when = stage.when ?? [];
  const set = (i: number, c: Condition) => onChange({ when: when.map((x, j) => (j === i ? c : x)) });
  return (
    <div className="wp-stack" style={{ gap: 6 }}>
      {when.length === 0 && <div className="wp-muted">Always, once the nodes it waits for are done.</div>}
      {when.map((c, i) => (
        <div key={`${i}-${c.ref}`} className="wp-row" style={{ gap: 6, flexWrap: "wrap" }}>
          <Input
            aria-label="Reference"
            list={listId}
            value={c.ref}
            disabled={readOnly}
            onChange={(e) => set(i, { ...c, ref: e.target.value })}
          />
          <Select
            aria-label="Operator"
            value={c.op}
            disabled={readOnly}
            onChange={(e) => {
              const op = e.target.value as CondOp;
              set(i, TAKES_VALUE.includes(op) ? { ...c, op } : { ref: c.ref, op });
            }}
            options={ops.map((o) => ({ value: o, label: o }))}
          />
          {TAKES_VALUE.includes(c.op) && (
            <Input
              aria-label="Value"
              value={showValue(c.value)}
              disabled={readOnly}
              onChange={(e) => set(i, { ...c, value: parseValue(e.target.value) })}
            />
          )}
          {!readOnly && (
            <Button
              size="sm"
              variant="ghost"
              icon="x"
              aria-label="Remove condition"
              onClick={() => onChange({ when: when.filter((_, j) => j !== i) })}
            />
          )}
        </div>
      ))}
      {!readOnly && (
        <div className="wp-row" style={{ gap: 6 }}>
          <Button
            size="sm"
            icon="plus"
            onClick={() =>
              onChange({ when: [...when, { ref: `${stage.after?.[0] ?? "session"}.output`, op: "truthy" }] })
            }
          >
            Add condition
          </Button>
          {when.length > 1 && (
            <Select
              aria-label="Combine conditions"
              value={stage.when_mode ?? "all"}
              onChange={(e) => onChange({ when_mode: e.target.value as "all" | "any" })}
              options={[
                { value: "all", label: "All must hold" },
                { value: "any", label: "One is enough" },
              ]}
            />
          )}
        </div>
      )}
    </div>
  );
}

function JsonField({
  label,
  value,
  readOnly,
  onChange,
}: {
  label: string;
  value: unknown;
  readOnly?: boolean;
  onChange: (v: unknown) => void;
}) {
  const [text, setText] = useState(() => JSON.stringify(value ?? {}, null, 2));
  const [bad, setBad] = useState(false);
  return (
    <div className="wp-stack" style={{ gap: 4 }}>
      <Input
        label={label}
        multiline
        rows={6}
        value={text}
        disabled={readOnly}
        onChange={(e) => {
          setText(e.target.value);
          try {
            onChange(JSON.parse(e.target.value));
            setBad(false);
          } catch {
            setBad(true);
          }
        }}
      />
      {bad && <div className="wp-muted">Write it as JSON; the last valid version is kept.</div>}
    </div>
  );
}
