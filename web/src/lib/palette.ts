import type { SearchHit, SearchHitKind, TreeSession } from "../api/nav";
import type { ProjectView } from "../api/types";
import { filterPaletteItems, type IconName, type PaletteItem } from "../design";
import { humanize } from "./format";
import { modHint } from "./keys";

/** Local commands, as the design's App.jsx lists them. `cmd:*` ids run an action instead of opening a tab. */
export function paletteCommands(dockHint = modHint("/")): PaletteItem[] {
  return [
    { id: "ws:overview", group: "Workspace", icon: "plus", label: "New task", hint: "overview" },
    { id: "cmd:new-workspace", group: "Workspace", icon: "box", label: "New workspace…" },
    { id: "cmd:add-project", group: "Workspace", icon: "folder-plus", label: "Add project…" },
    { id: "ws:cost", group: "Workspace", icon: "coins", label: "Cost" },
    { id: "ws:settings", group: "Workspace", icon: "settings", label: "Settings" },
    { id: "cmd:dock", group: "Actions", icon: "message-square", label: "Ask a quick question", hint: dockHint },
    { id: "cmd:theme", group: "Actions", icon: "sun-moon", label: "Toggle light and dark theme" },
  ];
}

/** Result rows before the commands; the palette renders every row, so keep the list short. */
const MAX_ROWS = 200;

const GROUPS = ["Sessions", "Executions", "Artifacts", "Projects", "Files", "Lessons", "Settings", "Workspace", "Actions"];

const KIND: Record<SearchHitKind, { group: string; icon: IconName }> = {
  session: { group: "Sessions", icon: "git-pull-request" },
  execution: { group: "Executions", icon: "activity" },
  artifact: { group: "Artifacts", icon: "file-text" },
  project: { group: "Projects", icon: "folder-git-2" },
  file: { group: "Files", icon: "file-code-2" },
  lesson: { group: "Lessons", icon: "brain" },
  setting: { group: "Settings", icon: "settings" },
};

export function hitToItem(hit: SearchHit): PaletteItem {
  const k = KIND[hit.kind] ?? { group: "Results", icon: "search" as IconName };
  return { id: hit.id, group: k.group, icon: k.icon, label: hit.label, hint: hit.hint ?? undefined };
}

/** Palette rows from data the shell already holds, for an empty query and for servers without search. */
export function localItems(sessions: TreeSession[], projects: ProjectView[], files: { key: string; paths: string[] } | null = null): PaletteItem[] {
  const items: PaletteItem[] = [];
  for (const s of sessions) items.push({ id: `session:${s.id}`, group: "Sessions", icon: "git-pull-request", label: s.title ? `${s.title}: ${s.request}` : s.request, hint: s.status });
  for (const s of sessions)
    for (const g of s.groups)
      for (const r of g.runs)
        items.push({
          id: `exec:${r.id}`,
          group: "Executions",
          icon: r.stream === "terminal" ? "square-terminal" : "activity",
          label: `${humanize(g.agent)} · ${r.run_label} in ${g.project}`,
          hint: r.status,
        });
  for (const s of sessions) for (const a of s.artifacts) items.push({ id: `artifact:${a.path}`, group: "Artifacts", icon: "file-text", label: a.label, hint: a.path.split("/").pop() });
  for (const p of projects) items.push({ id: `project:${p.key}`, group: "Projects", icon: "folder-git-2", label: p.key, hint: p.path });
  if (files) for (const f of files.paths) items.push({ id: `file:${files.key}:${f}`, group: "Files", icon: "file-code-2", label: f, hint: files.key });
  return items;
}

/**
 * The rows the palette shows. An empty query lists local rows and commands. A query shows the server's
 * hits when it has answered (`hits` not null), else the local rows filtered, then the matching commands.
 */
export function mergePalette(query: string, hits: SearchHit[] | null, local: PaletteItem[], commands: PaletteItem[]): PaletteItem[] {
  const q = query.trim();
  const found = !q ? local.filter((it) => it.group !== "Files") : hits ? [...hits].sort((a, b) => b.score - a.score).map(hitToItem) : filterPaletteItems(local, q);
  const all = [...found.slice(0, MAX_ROWS), ...filterPaletteItems(commands, q)];
  const seen = new Set<string>();
  const unique = all.filter((it) => !seen.has(it.id) && (seen.add(it.id), true));
  const rank = (g: string | undefined) => {
    const i = GROUPS.indexOf(g ?? "");
    return i < 0 ? GROUPS.length : i;
  };
  // Stable: rows keep their order within a group.
  return unique.map((it, i) => ({ it, i })).sort((a, b) => rank(a.it.group) - rank(b.it.group) || a.i - b.i).map((x) => x.it);
}

export type PaletteTarget = { kind: "command"; command: string } | { kind: "open"; id: string; anchor?: string };

/**
 * What selecting a palette row does. Lessons open Memory and settings keys open Settings, with the hit id
 * as the anchor so the screen selects that lesson or scrolls to that field.
 */
export function paletteTarget(id: string): PaletteTarget {
  if (id.startsWith("cmd:")) return { kind: "command", command: id.slice(4) };
  if (id.startsWith("lesson:")) return { kind: "open", id: "ws:memory", anchor: id };
  if (id.startsWith("setting:")) return { kind: "open", id: "ws:settings", anchor: id };
  return { kind: "open", id };
}
