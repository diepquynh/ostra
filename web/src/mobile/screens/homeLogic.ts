import type { Tone } from "@ostra/design";
import type { ContextFile, SessionStatus, SessionSummary } from "../../api/types";
import { fileKey } from "../../features/context/tags";

export type HomeFilter = "all" | "active" | "blocked" | "done";

const GROUPS: Record<HomeFilter, (s: SessionStatus) => boolean> = {
  all: () => true,
  active: (s) => s === "running" || s === "waiting",
  blocked: (s) => s === "waiting" || s === "stalled" || s === "paused",
  done: (s) => s === "completed",
};

export const HOME_FILTERS: { id: HomeFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "active", label: "Active" },
  { id: "blocked", label: "Blocked" },
  { id: "done", label: "Done" },
];

export const STATUS_TONE: Record<SessionStatus, Tone> = {
  running: "accent",
  waiting: "warn",
  paused: "neutral",
  completed: "ok",
  stalled: "bad",
  failed: "bad",
};

/** Sessions in a filter, newest update first. */
export function filterHome(list: SessionSummary[], f: HomeFilter): SessionSummary[] {
  return list.filter((s) => GROUPS[f](s.status)).sort((a, b) => b.updated_at.localeCompare(a.updated_at));
}

export function homeCounts(list: SessionSummary[]): Record<HomeFilter, number> {
  return {
    all: list.length,
    active: list.filter((s) => GROUPS.active(s.status)).length,
    blocked: list.filter((s) => GROUPS.blocked(s.status)).length,
    done: list.filter((s) => GROUPS.done(s.status)).length,
  };
}

/** The stage line of a session row and its tone: blocked sessions say so before the stage. */
export function stageLine(s: Pick<SessionSummary, "status" | "stage_label">): {
  text: string;
  tone: "warn" | "bad" | null;
} {
  if (s.status === "waiting") return { text: `Waiting for you · ${s.stage_label}`, tone: "warn" };
  if (s.status === "stalled") return { text: `Stalled · ${s.stage_label}`, tone: "bad" };
  if (s.status === "failed") return { text: `Failed · ${s.stage_label}`, tone: "bad" };
  if (s.status === "paused") return { text: `Paused · ${s.stage_label}`, tone: null };
  return { text: s.stage_label, tone: null };
}

const TAG = /(^|\s)@([A-Za-z0-9._-]+\/\S*)/g;
const TRAILING = /[.,;:!?)\]}"'`]+$/;

/**
 * Every `@project/path` tag in the text, in order, with whether it names a known file. Without `known`
 * (the file list is still loading) every tag counts as found.
 */
export function taggedFiles(
  text: string,
  projects: string[],
  known: Set<string> | null,
): { tag: string; ok: boolean }[] {
  const seen = new Set<string>();
  const out: { tag: string; ok: boolean }[] = [];
  for (const m of text.matchAll(TAG)) {
    const raw = m[2];
    const project = raw.slice(0, raw.indexOf("/"));
    if (!projects.includes(project)) continue;
    const exact = raw;
    const trimmed = raw.replace(TRAILING, "");
    const tag = known?.has(exact) ? exact : trimmed;
    if (!tag.slice(project.length + 1) || seen.has(tag)) continue;
    seen.add(tag);
    out.push({ tag, ok: !known || known.has(tag) });
  }
  return out;
}

/** Replace the `@query` just before the caret with a completed tag and a space. */
export function completeTag(
  text: string,
  caret: number,
  start: number,
  f: ContextFile,
): { text: string; caret: number } {
  const insert = `@${fileKey(f)} `;
  return { text: text.slice(0, start) + insert + text.slice(caret), caret: start + insert.length };
}
