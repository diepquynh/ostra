// A JSON Schema subset as an editable tree; the subset is what `schema_check.rs` checks.

export const FIELD_TYPES = ["string", "number", "integer", "boolean", "object", "array"] as const;
export type FieldType = (typeof FIELD_TYPES)[number];

export interface SchemaNode {
  id: number;
  name: string;
  type: FieldType;
  required: boolean;
  description: string;
  /** Comma-separated allowed values of a string, kept as typed so commas do not jump. */
  values: string;
  /** additionalProperties: false */
  closed: boolean;
  minItems: string;
  maxItems: string;
  fields: SchemaNode[];
  items: SchemaNode | null;
}

let nextId = 1;

export function newNode(type: FieldType = "string", name = ""): SchemaNode {
  return {
    id: nextId++,
    name,
    type,
    required: true,
    description: "",
    values: "",
    closed: false,
    minItems: "",
    maxItems: "",
    fields: [],
    items: null,
  };
}

type Json = Record<string, unknown>;

const isObj = (v: unknown): v is Json => typeof v === "object" && v !== null && !Array.isArray(v);
const KEYS: Record<FieldType, string[]> = {
  string: ["enum"],
  number: [],
  integer: [],
  boolean: [],
  object: ["properties", "required", "additionalProperties"],
  array: ["items", "minItems", "maxItems"],
};
const count = (v: unknown) => (v === undefined ? "" : Number.isInteger(v) && (v as number) >= 0 ? String(v) : null);

/** The tree for `schema`, or null when it uses something the builder cannot show (JSON mode keeps it). */
export function fromSchema(schema: unknown, name = "", required = false): SchemaNode | null {
  if (!isObj(schema) || typeof schema.type !== "string" || !FIELD_TYPES.includes(schema.type as FieldType)) return null;
  const type = schema.type as FieldType;
  const allowed = ["type", "description", ...KEYS[type]];
  if (Object.keys(schema).some((k) => !allowed.includes(k))) return null;
  if (schema.description !== undefined && typeof schema.description !== "string") return null;
  const node = newNode(type, name);
  node.required = required;
  node.description = (schema.description as string | undefined) ?? "";
  if (schema.enum !== undefined) {
    const e = schema.enum;
    if (!Array.isArray(e) || e.some((v) => typeof v !== "string" || v.includes(",") || v.trim() !== v || !v))
      return null;
    node.values = e.join(", ");
  }
  if (type === "object") {
    const props = schema.properties ?? {};
    const req = schema.required ?? [];
    if (!isObj(props) || !Array.isArray(req) || req.some((r) => typeof r !== "string" || !(r in props))) return null;
    if (schema.additionalProperties !== undefined && schema.additionalProperties !== false) return null;
    node.closed = schema.additionalProperties === false;
    for (const [k, v] of Object.entries(props)) {
      const child = fromSchema(v, k, req.includes(k));
      if (!child) return null;
      node.fields.push(child);
    }
  }
  if (type === "array") {
    const min = count(schema.minItems);
    const max = count(schema.maxItems);
    if (min === null || max === null) return null;
    node.minItems = min;
    node.maxItems = max;
    if (schema.items !== undefined) {
      node.items = fromSchema(schema.items);
      if (!node.items) return null;
    }
  }
  return node;
}

export function toSchema(node: SchemaNode): Json {
  const out: Json = { type: node.type };
  if (node.description.trim()) out.description = node.description.trim();
  if (node.type === "string") {
    const values = splitValues(node.values);
    if (values.length) out.enum = values;
  }
  if (node.type === "object") {
    if (node.fields.length) out.properties = Object.fromEntries(node.fields.map((f) => [f.name.trim(), toSchema(f)]));
    const req = node.fields.filter((f) => f.required).map((f) => f.name.trim());
    if (req.length) out.required = req;
    if (node.closed) out.additionalProperties = false;
  }
  if (node.type === "array") {
    if (node.items) out.items = toSchema(node.items);
    if (/^\d+$/.test(node.minItems.trim())) out.minItems = Number(node.minItems);
    if (/^\d+$/.test(node.maxItems.trim())) out.maxItems = Number(node.maxItems);
  }
  return out;
}

/** The root's schema, or null when it declares nothing. */
export function rootSchema(root: SchemaNode): Json | null {
  return root.fields.length || root.closed || root.description.trim() ? toSchema(root) : null;
}

export function splitValues(text: string): string[] {
  return text
    .split(",")
    .map((v) => v.trim())
    .filter(Boolean);
}

/** The first thing to fix before the tree is a valid schema, naming the field's path. */
export function problem(node: SchemaNode, path = "data"): string | null {
  if (node.type === "object") {
    const seen = new Set<string>();
    for (const f of node.fields) {
      const name = f.name.trim();
      if (!name) return `Name every field of \`${path}\`.`;
      if (seen.has(name)) return `Rename one of the two \`${path}.${name}\` fields: names must differ.`;
      seen.add(name);
      const p = problem(f, `${path}.${name}`);
      if (p) return p;
    }
  }
  if (node.type === "array") {
    for (const [label, v] of [
      ["minimum", node.minItems],
      ["maximum", node.maxItems],
    ]) {
      if (v.trim() && !/^\d+$/.test(v.trim())) return `Give the ${label} item count of \`${path}\` as a whole number.`;
    }
    if (node.minItems.trim() && node.maxItems.trim() && Number(node.minItems) > Number(node.maxItems))
      return `Lower the minimum item count of \`${path}\` to at most its maximum.`;
    if (node.items) return problem(node.items, `${path}[]`);
  }
  return null;
}
