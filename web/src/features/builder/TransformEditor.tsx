import { Banner, Button, Checkbox, Input, Panel, Select } from "@ostra/design";
import { useState } from "react";
import { api } from "../../api";
import type { FunctionFile, FunctionStep, TransformInfo, TransformParam, ValueKind } from "../../api/types";

const KINDS: ValueKind[] = ["any", "bool", "number", "string", "array", "object"];

const parseValue = (text: string): unknown => {
  if (text.startsWith("$")) return text;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
};
const showValue = (v: unknown) => (v === undefined || v === null ? "" : typeof v === "string" ? v : JSON.stringify(v));

export type TransformEditorProps = {
  ws: string;
  name: string;
  initial: FunctionFile;
  transforms: TransformInfo[];
  usedBy: string[];
  onSaved: () => void;
  onDeleted: () => void;
};

/** Rule WB7: a composite transform function: its inputs, arguments, steps, and output step. */
export function TransformEditor({ ws, name, initial, transforms, usedBy, onSaved, onDeleted }: TransformEditorProps) {
  const [file, setFile] = useState<FunctionFile>(initial);
  const [issues, setIssues] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const steps = file.step ?? [];
  const set = (patch: Partial<FunctionFile>) => {
    setSaved(false);
    setFile((f) => ({ ...f, ...patch }));
  };
  const setStep = (i: number, patch: Partial<FunctionStep>) =>
    set({ step: steps.map((s, j) => (j === i ? { ...s, ...patch } : s)) });

  const save = async () => {
    setBusy(true);
    setIssues([]);
    try {
      const doc = await api.saveTransformFunction(ws, name, file);
      setFile(doc.file);
      setSaved(true);
      onSaved();
    } catch (e) {
      const err = e as Error & { issues?: { message: string }[] };
      setIssues(err.issues?.length ? err.issues.map((i) => i.message) : [err.message]);
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    try {
      await api.deleteTransformFunction(ws, name);
      onDeleted();
    } catch (e) {
      setIssues([(e as Error).message]);
    }
  };

  // A composite runs inside Ostra in one step, so it calls only Ostra's and the workspace's functions.
  const others = transforms.filter((t) => t.name !== name && !t.plugin);
  return (
    <div className="wp-stack" style={{ gap: 12 }}>
      <div className="wp-row" style={{ gap: 8 }}>
        <span className="wb-bar__name">{name}</span>
        <Input
          aria-label="Description"
          placeholder="What the function gives"
          value={file.description ?? ""}
          onChange={(e) => set({ description: e.target.value })}
          style={{ flex: 1 }}
        />
        <Button variant="primary" icon="check" disabled={busy} onClick={() => void save()}>
          {busy ? "Saving…" : saved ? "Saved" : "Save"}
        </Button>
        <Button variant="ghost" icon="trash-2" onClick={() => void remove()}>
          Delete
        </Button>
      </div>
      {usedBy.length > 0 && <div className="wp-muted">Used by: {usedBy.join(", ")}</div>}
      {issues.length > 0 && (
        <Banner tone="bad">
          {issues.map((i) => (
            <div key={i}>{i}</div>
          ))}
        </Banner>
      )}
      <ParamsPanel
        title="Inputs"
        hint="Values a workflow node wires in. Steps read them as input.<name>."
        params={file.input ?? []}
        onChange={(input) => set({ input })}
      />
      <ParamsPanel
        title="Arguments"
        hint="Fixed values set on the node. A step argument written $<name> takes one."
        params={file.arg ?? []}
        onChange={(arg) => set({ arg })}
      />
      <Panel
        title="Steps"
        icon="list-checks"
        subtitle="They run in order; a step reads earlier steps as <step>.output."
      >
        <div className="wp-stack" style={{ gap: 10 }}>
          {steps.map((s, i) => (
            <StepRow
              key={`${i}-${s.id}`}
              step={s}
              refs={[
                ...(file.input ?? []).map((p) => `input.${p.name}`),
                ...steps.slice(0, i).map((e) => `${e.id}.output`),
              ]}
              transforms={others}
              onChange={(patch) => setStep(i, patch)}
              onRemove={() => set({ step: steps.filter((_, j) => j !== i) })}
            />
          ))}
          <div className="wp-row" style={{ gap: 8 }}>
            <Button
              size="sm"
              icon="plus"
              onClick={() =>
                set({ step: [...steps, { id: `step-${steps.length + 1}`, transform: "pick", inputs: {}, args: {} }] })
              }
            >
              Add step
            </Button>
            <Select
              aria-label="Output step"
              value={file.output}
              onChange={(e) => set({ output: e.target.value })}
              options={[
                { value: "", label: "Output: pick a step" },
                ...steps.map((s) => ({ value: s.id, label: `Output: ${s.id}` })),
              ]}
            />
          </div>
        </div>
      </Panel>
    </div>
  );
}

function ParamsPanel({
  title,
  hint,
  params,
  onChange,
}: {
  title: string;
  hint: string;
  params: TransformParam[];
  onChange: (p: TransformParam[]) => void;
}) {
  const edit = (i: number, patch: Partial<TransformParam>) =>
    onChange(params.map((p, j) => (j === i ? { ...p, ...patch } : p)));
  return (
    <Panel title={title} subtitle={hint}>
      <div className="wp-stack" style={{ gap: 6 }}>
        {params.map((p, i) => (
          <div key={i} className="wp-row" style={{ gap: 6, flexWrap: "wrap" }}>
            <Input aria-label={`${title} name`} value={p.name} onChange={(e) => edit(i, { name: e.target.value })} />
            <Select
              aria-label={`${title} kind`}
              value={p.kind}
              onChange={(e) => edit(i, { kind: e.target.value as ValueKind })}
              options={KINDS.map((k) => ({ value: k, label: k }))}
            />
            <Checkbox label="Required" checked={p.required} onChange={(e) => edit(i, { required: e.target.checked })} />
            <Input
              aria-label={`${title} description`}
              placeholder="What it is"
              value={p.description}
              onChange={(e) => edit(i, { description: e.target.value })}
              style={{ flex: 1 }}
            />
            <Button
              size="sm"
              variant="ghost"
              icon="x"
              aria-label={`Remove ${p.name}`}
              onClick={() => onChange(params.filter((_, j) => j !== i))}
            />
          </div>
        ))}
        <div>
          <Button
            size="sm"
            icon="plus"
            onClick={() =>
              onChange([...params, { name: `value${params.length + 1}`, kind: "any", required: true, description: "" }])
            }
          >
            Add {title.toLowerCase().replace(/s$/, "")}
          </Button>
        </div>
      </div>
    </Panel>
  );
}

function StepRow({
  step,
  refs,
  transforms,
  onChange,
  onRemove,
}: {
  step: FunctionStep;
  refs: string[];
  transforms: TransformInfo[];
  onChange: (patch: Partial<FunctionStep>) => void;
  onRemove: () => void;
}) {
  const fn = transforms.find((t) => t.name === step.transform);
  const listId = `tf-refs-${step.id}`;
  const inputs = step.inputs ?? {};
  const names = fn && !fn.variadic ? fn.inputs.map((p) => p.name) : Object.keys(inputs);
  const [fresh, setFresh] = useState("");
  return (
    <div className="wp-stack" style={{ gap: 6, borderLeft: "2px solid var(--border-default)", paddingLeft: 8 }}>
      <datalist id={listId}>
        {refs.map((r) => (
          <option key={r} value={r} />
        ))}
      </datalist>
      <div className="wp-row" style={{ gap: 6 }}>
        <Input aria-label="Step id" value={step.id} onChange={(e) => onChange({ id: e.target.value })} />
        <Select
          aria-label="Step function"
          value={step.transform}
          onChange={(e) => onChange({ transform: e.target.value, inputs: {}, args: {} })}
          options={[
            ...(fn ? [] : [{ value: step.transform, label: `${step.transform} (not found)` }]),
            ...transforms.map((t) => ({ value: t.name, label: t.custom ? `${t.name} (yours)` : t.name })),
          ]}
        />
        <span className="wp-muted" style={{ flex: 1 }}>
          {fn?.description}
        </span>
        <Button size="sm" variant="ghost" icon="x" aria-label={`Remove step ${step.id}`} onClick={onRemove} />
      </div>
      <div className="wp-row" style={{ gap: 6, flexWrap: "wrap" }}>
        {names.map((n) => (
          <Input
            key={n}
            label={n}
            list={listId}
            placeholder="input.name or step.output"
            value={inputs[n] ?? ""}
            onChange={(e) => {
              const next = { ...inputs };
              if (e.target.value) next[n] = e.target.value;
              else delete next[n];
              onChange({ inputs: next });
            }}
          />
        ))}
        {fn?.variadic && (
          <>
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
                onChange({ inputs: { ...inputs, [fresh]: "" } });
                setFresh("");
              }}
            >
              Add input
            </Button>
          </>
        )}
      </div>
      {fn && fn.args.length > 0 && (
        <div className="wp-row" style={{ gap: 6, flexWrap: "wrap" }}>
          {fn.args.map((p) => (
            <Input
              key={p.name}
              label={`${p.name}: ${p.kind}`}
              title={p.description}
              value={showValue(step.args?.[p.name])}
              onChange={(e) => {
                const next = { ...(step.args ?? {}) };
                if (e.target.value === "") delete next[p.name];
                else next[p.name] = p.kind === "string" ? e.target.value : parseValue(e.target.value);
                onChange({ args: next });
              }}
            />
          ))}
        </div>
      )}
    </div>
  );
}
