import type { SessionStatus, SessionSummary } from "../../api/types";

export type StatusFilter = "all" | "active" | "waiting" | "completed" | "failed";
export type KindFilter = "all" | "pipeline" | "init";

export type SessionFilters = {
  status: StatusFilter;
  kind: KindFilter;
  /** A project key, or "all". */
  project: string;
  text: string;
};

export const NO_FILTERS: SessionFilters = { status: "all", kind: "all", project: "all", text: "" };

const STATUS_SETS: Record<Exclude<StatusFilter, "all">, SessionStatus[]> = {
  active: ["running", "waiting"],
  waiting: ["waiting"],
  completed: ["completed"],
  failed: ["failed", "stalled"],
};

export const STATUS_TABS: { id: StatusFilter; label: string; title?: string }[] = [
  { id: "all", label: "All" },
  { id: "active", label: "Active", title: "Running or waiting for you" },
  { id: "waiting", label: "Waiting" },
  { id: "completed", label: "Completed" },
  { id: "failed", label: "Failed", title: "Failed or stalled" },
];

/** The label a session shows: the Classify judge's title, or the request until it is classified. */
export const sessionTitle = (s: Pick<SessionSummary, "title" | "request">): string => s.title?.trim() || s.request;

function matchesStatus(s: SessionSummary, status: StatusFilter): boolean {
  return status === "all" || STATUS_SETS[status].includes(s.status);
}

function matchesText(s: SessionSummary, text: string): boolean {
  const words = text.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const hay = [s.title ?? "", s.request, s.id, s.stage_label, s.category ?? "", ...s.projects].join(" ").toLowerCase();
  return words.every((w) => hay.includes(w));
}

/** Everything but the status filter, so the status tabs can count what each would show. */
function matchesOthers(s: SessionSummary, f: SessionFilters): boolean {
  if (f.kind !== "all" && s.kind.kind !== f.kind) return false;
  if (f.project !== "all" && !s.projects.includes(f.project)) return false;
  return matchesText(s, f.text);
}

const byUpdated = (a: SessionSummary, b: SessionSummary) => b.updated_at.localeCompare(a.updated_at);

/** The sessions the table shows, newest update first. */
export function filterSessions(list: SessionSummary[], f: SessionFilters): SessionSummary[] {
  return list.filter((s) => matchesStatus(s, f.status) && matchesOthers(s, f)).sort(byUpdated);
}

/** How many sessions each status tab would show under the other filters. */
export function statusCounts(list: SessionSummary[], f: SessionFilters): Record<StatusFilter, number> {
  const rest = list.filter((s) => matchesOthers(s, f));
  return {
    all: rest.length,
    active: rest.filter((s) => matchesStatus(s, "active")).length,
    waiting: rest.filter((s) => matchesStatus(s, "waiting")).length,
    completed: rest.filter((s) => matchesStatus(s, "completed")).length,
    failed: rest.filter((s) => matchesStatus(s, "failed")).length,
  };
}

export const isFiltered = (f: SessionFilters) =>
  f.status !== "all" || f.kind !== "all" || f.project !== "all" || f.text.trim() !== "";

/** Project keys that appear in any session, for the project filter. */
export function sessionProjects(list: SessionSummary[], known: string[]): string[] {
  const keys = new Set(known);
  for (const s of list) for (const p of s.projects) keys.add(p);
  return [...keys].sort();
}
