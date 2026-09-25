import type { ReactNode } from "react";
import type { Severity } from "../../../api/gen/Severity";
import type { DocIssue, FactCheckFinding, FactCheckView } from "../../../api/types";

/** One page of a typed document. Sub-chapters carry `depth: 1` and follow their parent. */
export type Chapter = {
  id: string;
  title: string;
  count?: number;
  depth?: number;
  /** Set by the view from the markers of the elements this chapter holds. */
  tone?: MarkTone;
  /** A reason to look even without markers, for example open questions. */
  attention?: boolean;
  render: () => ReactNode;
};

export type MarkTone = "bad" | "warn" | "info";

/** A note on one element: a fact-check finding or one of Ostra's checks. */
export type Mark = { tone: MarkTone; source: "fact-check" | "check"; text: string };

/** The chapters of a document and which chapter holds each element id. */
export type Outline = { chapters: Chapter[]; where: Map<string, string> };

/**
 * One key per element, however an agent spelled it: `R3`, `ac3.2`, `Phase 2`, `P2`, `Step 2.3`.
 */
export function normId(raw: string): string {
  const s = raw.trim().toLowerCase().replace(/\s+/g, "");
  const phase = /^(?:phase|p)(\d+)$/.exec(s);
  if (phase) return `phase-${phase[1]}`;
  const step = /^(?:step|s)?(\d+\.\d+)$/.exec(s);
  if (step) return `step-${step[1]}`;
  return s;
}

const ID_IN_TEXT = /\b(AC\d+\.\d+|R\d+|E\d+|D\d+|C\d+|Q\d+|step\s*\d+\.\d+|phase\s*\d+)\b/i;

/** The element a finding is about: its `element`, else the first id its location names. */
export function findingElement(f: FactCheckFinding): string | null {
  const raw = f.element?.split(",")[0] ?? ID_IN_TEXT.exec(f.location)?.[1] ?? null;
  return raw ? normId(raw) : null;
}

export const severityTone = (s: Severity): MarkTone =>
  s === "BLOCKER" || s === "HIGH" ? "bad" : s === "MEDIUM" ? "warn" : "info";

const RANK: Record<MarkTone, number> = { bad: 0, warn: 1, info: 2 };
export const worst = (tones: MarkTone[]): MarkTone | undefined => [...tones].sort((a, b) => RANK[a] - RANK[b])[0];

/** Markers per element id; findings and issues with no element land under `""`. */
export function buildMarks(issues: DocIssue[], check: FactCheckView | null): Map<string, Mark[]> {
  const out = new Map<string, Mark[]>();
  const add = (key: string, m: Mark) => out.set(key, [...(out.get(key) ?? []), m]);
  for (const i of issues)
    add(i.element ? normId(i.element) : "", {
      tone: i.level === "error" ? "bad" : "warn",
      source: "check",
      text: i.message,
    });
  for (const f of check?.findings ?? [])
    add(findingElement(f) ?? "", {
      tone: severityTone(f.severity),
      source: "fact-check",
      text: `${f.claim}: ${f.issue}`,
    });
  return out;
}

/** The pass whose findings to show: the latest over the current version, else the latest. */
export function pickFactCheck(all: FactCheckView[] | undefined, target: "spec" | "plan"): FactCheckView | null {
  const mine = (all ?? []).filter((c) => c.target === target);
  return [...mine].reverse().find((c) => c.current && c.verdict) ?? mine[mine.length - 1] ?? null;
}

/** `#chapter` or `#chapter/element`. */
export function parseAnchor(hash: string): { chapter: string | null; element: string | null } {
  if (!hash) return { chapter: null, element: null };
  const [chapter, element] = hash.split("/");
  return { chapter: chapter || null, element: element ? normId(element) : null };
}

/** Collects chapters and the element index while a builder runs. */
export class OutlineBuilder {
  chapters: Chapter[] = [];
  where = new Map<string, string>();

  chapter(c: Chapter, elements: Iterable<string> = []): this {
    this.chapters.push(c);
    for (const e of elements) {
      const k = normId(e);
      if (!this.where.has(k)) this.where.set(k, c.id);
    }
    return this;
  }

  /** Add a chapter only when it has something to show. */
  maybe(show: boolean, c: Chapter, elements: Iterable<string> = []): this {
    return show ? this.chapter(c, elements) : this;
  }

  done(): Outline {
    return { chapters: this.chapters, where: this.where };
  }
}

/** Phase ids grouped by dependency depth, for the phase graph. */
export function layers<T extends { id: number; depends_on: number[] }>(phases: T[]): T[][] {
  const depth = new Map<number, number>();
  const of = (p: T, seen: Set<number>): number => {
    if (depth.has(p.id)) return depth.get(p.id)!;
    if (seen.has(p.id)) return 0;
    seen.add(p.id);
    const deps = p.depends_on.map((d) => phases.find((x) => x.id === d)).filter((x): x is T => !!x);
    const d = deps.length ? Math.max(...deps.map((x) => of(x, seen) + 1)) : 0;
    depth.set(p.id, d);
    return d;
  };
  const out: T[][] = [];
  for (const p of phases) (out[of(p, new Set())] ??= []).push(p);
  return out.filter(Boolean);
}
