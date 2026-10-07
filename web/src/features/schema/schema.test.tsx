import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { fromSchema, newNode, problem, rootSchema, toSchema } from "./model";
import { SchemaBuilder } from "./SchemaBuilder";

afterEach(cleanup);

const NESTED = {
  type: "object",
  required: ["risk", "findings"],
  additionalProperties: false,
  properties: {
    risk: { type: "string", enum: ["low", "high"], description: "Overall risk" },
    score: { type: "number" },
    findings: {
      type: "array",
      minItems: 1,
      maxItems: 10,
      items: {
        type: "object",
        required: ["file"],
        properties: {
          file: { type: "string" },
          line: { type: "integer" },
          tags: { type: "array", items: { type: "string" } },
          meta: { type: "object", properties: { fixed: { type: "boolean" } } },
        },
      },
    },
    anything: { type: "array" },
  },
};

describe("schema model", () => {
  it("round-trips a nested schema", () => {
    const tree = fromSchema(NESTED);
    expect(tree).not.toBeNull();
    expect(toSchema(tree!)).toEqual(NESTED);
  });

  it("refuses what the builder cannot show, so JSON mode keeps it", () => {
    expect(fromSchema({ type: "object", properties: { a: { type: ["string", "null"] } } })).toBeNull();
    expect(fromSchema({ type: "object", properties: { a: { type: "string", pattern: "x" } } })).toBeNull();
    expect(fromSchema({ type: "object", required: ["missing"] })).toBeNull();
    expect(fromSchema({ type: "number", enum: [1, 2] })).toBeNull();
    expect(fromSchema({ type: "string", enum: ["a,b"] })).toBeNull();
    expect(fromSchema({ type: "object", additionalProperties: true })).toBeNull();
  });

  it("an empty root declares no schema", () => {
    expect(rootSchema(newNode("object"))).toBeNull();
  });

  it("names the path of the first problem", () => {
    const root = newNode("object");
    const list = newNode("array", "items");
    list.items = newNode("object");
    list.items.fields = [newNode("string", "a"), newNode("string", "a")];
    root.fields = [list];
    expect(problem(root)).toContain("`data.items[].a`");
    list.items.fields = [newNode("string", "")];
    expect(problem(root)).toContain("`data.items[]`");
    list.items.fields = [];
    list.minItems = "3";
    list.maxItems = "1";
    expect(problem(root)).toContain("minimum item count of `data.items`");
  });
});

describe("SchemaBuilder", () => {
  it("builds a nested object field by field", () => {
    const onChange = vi.fn();
    render(<SchemaBuilder label="Data schema" value={null} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Add field" }));
    fireEvent.change(screen.getByLabelText("Field name"), { target: { value: "report" } });
    fireEvent.change(screen.getByLabelText("Field type"), { target: { value: "object" } });
    fireEvent.click(screen.getAllByRole("button", { name: "Add field" })[0]);
    fireEvent.change(screen.getAllByLabelText("Field name")[1], { target: { value: "ok" } });
    fireEvent.change(screen.getAllByLabelText("Field type")[1], { target: { value: "boolean" } });
    expect(onChange).toHaveBeenLastCalledWith(
      {
        type: "object",
        required: ["report"],
        properties: { report: { type: "object", required: ["ok"], properties: { ok: { type: "boolean" } } } },
      },
      null,
    );
  });

  it("reports an unnamed field as a problem", () => {
    const onChange = vi.fn();
    render(<SchemaBuilder label="Data schema" value={null} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Add field" }));
    expect(onChange.mock.calls.at(-1)?.[1]).toContain("Name every field");
  });

  it("opens a schema it cannot show in JSON mode and keeps it", () => {
    const odd = { type: "object", properties: { a: { type: "string", pattern: "^x" } } };
    render(<SchemaBuilder label="Data schema" value={odd} onChange={vi.fn()} />);
    const box = screen.getByLabelText("Data schema as JSON") as HTMLTextAreaElement;
    expect(JSON.parse(box.value)).toEqual(odd);
    fireEvent.click(screen.getByRole("button", { name: "Builder" }));
    expect(screen.getByText(/uses keywords the builder does not show/)).toBeTruthy();
    expect(screen.getByLabelText("Data schema as JSON")).toBeTruthy();
  });

  it("switches between builder and JSON without losing fields", () => {
    render(<SchemaBuilder label="Data schema" value={NESTED} onChange={vi.fn()} />);
    expect(screen.getAllByLabelText("Field name").length).toBe(9);
    fireEvent.click(screen.getByRole("button", { name: "JSON" }));
    const box = screen.getByLabelText("Data schema as JSON") as HTMLTextAreaElement;
    expect(JSON.parse(box.value)).toEqual(NESTED);
    fireEvent.click(screen.getByRole("button", { name: "Builder" }));
    expect(screen.getAllByLabelText("Field name").length).toBe(9);
  });
});
