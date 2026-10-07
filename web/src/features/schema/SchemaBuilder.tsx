import { Button, Checkbox, IconButton, Input, Select } from "@ostra/design";
import { useState } from "react";
import { FIELD_TYPES, type FieldType, fromSchema, newNode, problem, rootSchema, type SchemaNode } from "./model";
import "./schema.css";

const TYPE_OPTIONS = FIELD_TYPES.map((t) => ({ value: t, label: t }));
const ITEM_OPTIONS = [{ value: "", label: "any" }, ...TYPE_OPTIONS];

/**
 * Edits an object schema as nested fields, or as JSON. `onChange` gets the schema (null for none) and the
 * first problem that blocks saving it.
 */
export function SchemaBuilder({
  label,
  hint,
  value,
  onChange,
}: {
  label: string;
  hint?: string;
  value: unknown;
  onChange: (schema: unknown, problem: string | null) => void;
}) {
  const [root, setRoot] = useState<SchemaNode | null>(() => (value == null ? newNode("object") : fromSchema(value)));
  const [text, setText] = useState(() => (value == null ? "" : JSON.stringify(value, null, 2)));
  const [json, setJson] = useState(root === null);
  const [note, setNote] = useState<string | null>(null);

  const editTree = (next: SchemaNode) => {
    setRoot(next);
    onChange(rootSchema(next), problem(next));
  };

  const editText = (t: string) => {
    setText(t);
    if (!t.trim()) return onChange(null, null);
    try {
      const parsed = JSON.parse(t);
      onChange(parsed, isPlainObject(parsed) ? null : "Write the data schema as a JSON object, or leave it empty.");
    } catch {
      onChange(null, "Write the data schema as JSON, or leave it empty.");
    }
  };

  const toBuilder = () => {
    if (!json) return;
    setNote(null);
    if (!text.trim()) {
      setRoot(newNode("object"));
      return setJson(false);
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch {
      return setNote("Fix the JSON first, because the builder starts from it.");
    }
    const tree = fromSchema(parsed);
    if (tree?.type !== "object") {
      return setNote(
        "Keep editing this schema as JSON, because it uses keywords the builder does not show (only type, description, properties, required, items, enum, minItems, maxItems, and additionalProperties: false).",
      );
    }
    setRoot(tree);
    setJson(false);
  };

  const toJson = () => {
    if (json) return;
    setNote(null);
    const s = root ? rootSchema(root) : null;
    setText(s == null ? "" : JSON.stringify(s, null, 2));
    setJson(true);
  };

  return (
    <div className="sb">
      <div className="sb-head">
        <span className="sb-label">{label}</span>
        <Button size="sm" variant="ghost" active={!json} onClick={toBuilder}>
          Builder
        </Button>
        <Button size="sm" variant="ghost" active={json} onClick={toJson}>
          JSON
        </Button>
      </div>
      {hint && <div className="wp-muted">{hint}</div>}
      {note && <div className="wp-muted">{note}</div>}
      {json || !root ? (
        <Input
          aria-label={`${label} as JSON`}
          multiline
          mono
          rows={8}
          value={text}
          placeholder='{"type": "object", "properties": {"risk": {"type": "string"}}}'
          onChange={(e) => editText(e.target.value)}
        />
      ) : (
        <>
          <FieldList fields={root.fields} onChange={(fields) => editTree({ ...root, fields })} />
          {root.fields.length > 0 && (
            <Checkbox
              label="Refuse fields not listed here"
              checked={root.closed}
              onChange={(e) => editTree({ ...root, closed: e.target.checked })}
            />
          )}
        </>
      )}
    </div>
  );
}

const isPlainObject = (v: unknown) => typeof v === "object" && v !== null && !Array.isArray(v);

function FieldList({ fields, onChange }: { fields: SchemaNode[]; onChange: (f: SchemaNode[]) => void }) {
  return (
    <div className="sb-list">
      {fields.map((f) => (
        <FieldRow
          key={f.id}
          node={f}
          onChange={(n) => onChange(fields.map((x) => (x.id === f.id ? n : x)))}
          onRemove={() => onChange(fields.filter((x) => x.id !== f.id))}
        />
      ))}
      <div>
        <Button size="sm" icon="plus" onClick={() => onChange([...fields, newNode()])}>
          Add field
        </Button>
      </div>
    </div>
  );
}

function FieldRow({
  node,
  onChange,
  onRemove,
}: {
  node: SchemaNode;
  onChange: (n: SchemaNode) => void;
  onRemove: () => void;
}) {
  return (
    <div className="sb-field">
      <div className="sb-row">
        <Input
          size="sm"
          mono
          aria-label="Field name"
          placeholder="field_name"
          value={node.name}
          className="sb-name"
          onChange={(e) => onChange({ ...node, name: e.target.value })}
        />
        <Select
          size="sm"
          aria-label="Field type"
          value={node.type}
          options={TYPE_OPTIONS}
          onChange={(e) => onChange({ ...node, type: e.target.value as FieldType })}
        />
        <Checkbox
          label="Required"
          checked={node.required}
          onChange={(e) => onChange({ ...node, required: e.target.checked })}
        />
        <IconButton size="sm" icon="trash-2" label="Remove field" onClick={onRemove} />
      </div>
      <TypeDetails node={node} onChange={onChange} />
    </div>
  );
}

/** Settings of a field's type; an array's item is a nameless node shown the same way. */
function TypeDetails({ node, onChange }: { node: SchemaNode; onChange: (n: SchemaNode) => void }) {
  return (
    <div className="sb-details">
      <Input
        size="sm"
        aria-label="Field description"
        placeholder="Description (optional)"
        value={node.description}
        onChange={(e) => onChange({ ...node, description: e.target.value })}
      />
      {node.type === "string" && (
        <Input
          size="sm"
          mono
          aria-label="Allowed values"
          placeholder="Allowed values, comma-separated (optional)"
          value={node.values}
          onChange={(e) => onChange({ ...node, values: e.target.value })}
        />
      )}
      {node.type === "object" && (
        <div className="sb-nested">
          <FieldList fields={node.fields} onChange={(fields) => onChange({ ...node, fields })} />
          {node.fields.length > 0 && (
            <Checkbox
              label="Refuse fields not listed here"
              checked={node.closed}
              onChange={(e) => onChange({ ...node, closed: e.target.checked })}
            />
          )}
        </div>
      )}
      {node.type === "array" && (
        <>
          <div className="sb-row">
            <Select
              size="sm"
              aria-label="Item type"
              value={node.items?.type ?? ""}
              options={ITEM_OPTIONS.map((o) => ({ ...o, label: `Each item: ${o.label}` }))}
              onChange={(e) => {
                const t = e.target.value as FieldType | "";
                onChange({ ...node, items: t ? { ...(node.items ?? newNode(t)), type: t } : null });
              }}
            />
            <Input
              size="sm"
              aria-label="Minimum items"
              placeholder="Min items"
              value={node.minItems}
              className="sb-count"
              onChange={(e) => onChange({ ...node, minItems: e.target.value })}
            />
            <Input
              size="sm"
              aria-label="Maximum items"
              placeholder="Max items"
              value={node.maxItems}
              className="sb-count"
              onChange={(e) => onChange({ ...node, maxItems: e.target.value })}
            />
          </div>
          {node.items && (
            <div className="sb-nested">
              <TypeDetails node={node.items} onChange={(items) => onChange({ ...node, items })} />
            </div>
          )}
        </>
      )}
    </div>
  );
}
