import type { TreeSession } from "../api/nav";
import type { SessionStatus } from "../api/types";
import type { Crumb, IconName } from "../design";
import { basename, humanize, truncate } from "../lib/format";
import { parseResource } from "../lib/resource";

/** A crumb whose `to` is a resource id. */
export type ResourceCrumb = Crumb & { to?: string };

export type ResourceMeta = {
  label: string;
  icon: IconName;
  crumbs: ResourceCrumb[];
  /** Session status, shown as a dot on the tab. */
  status?: SessionStatus;
  title?: string;
};

export type MetaContext = { wsName: string; sessions: TreeSession[] };

const PAGE: Record<string, [string, IconName]> = {
  overview: ["Overview", "layout-dashboard"],
  cost: ["Cost", "coins"],
  settings: ["Settings", "settings"],
  memory: ["Memory", "brain"],
  skills: ["Skills", "book-open"],
};

export const sessionLabel = (s: Pick<TreeSession, "title" | "request">) => s.title ?? truncate(s.request, 40);

/** Tab label, icon and breadcrumbs of a resource id, from the Sessions tree the shell holds. */
export function resourceMeta(id: string, ctx: MetaContext): ResourceMeta {
  const ws: ResourceCrumb = { label: ctx.wsName, icon: "box", to: "ws:overview" };
  const r = parseResource(id);
  if (!r) return { label: id, icon: "file", crumbs: [ws, { label: id }] };
  switch (r.type) {
    case "ws": {
      const [label, icon] = PAGE[r.page];
      return { label, icon, crumbs: [ws, { label }] };
    }
    case "project":
      return { label: r.key, icon: "folder-git-2", crumbs: [ws, { label: "Projects" }, { label: r.key }] };
    case "file":
      return {
        label: basename(r.path),
        icon: "file-code-2",
        title: `${r.key}/${r.path}`,
        crumbs: [ws, { label: r.key, to: `project:${r.key}` }, ...r.path.split("/").map((label) => ({ label }))],
      };
    case "session": {
      const s = ctx.sessions.find((x) => x.id === r.id);
      const label = s ? sessionLabel(s) : "Session";
      return { label, icon: "git-pull-request", crumbs: [ws, { label }], status: s?.status, title: s?.request };
    }
    case "exec": {
      for (const s of ctx.sessions)
        for (const g of s.groups) {
          const run = g.runs.find((x) => x.id === r.id);
          if (!run) continue;
          const label = `${humanize(g.agent)} · ${run.run_label}`;
          return {
            label,
            icon: run.stream === "terminal" ? "square-terminal" : "activity",
            title: `${label} in ${g.project}`,
            crumbs: [ws, { label: sessionLabel(s), to: `session:${s.id}` }, { label }],
          };
        }
      return { label: "Execution", icon: "activity", crumbs: [ws, { label: "Execution" }] };
    }
    case "artifact": {
      const name = basename(r.path);
      for (const s of ctx.sessions) {
        const a = s.artifacts.find((x) => x.path === r.path);
        if (a)
          return {
            label: a.label,
            icon: "file-text",
            title: r.path,
            crumbs: [ws, { label: sessionLabel(s), to: `session:${s.id}` }, { label: name }],
          };
      }
      return { label: name, icon: "file-text", title: r.path, crumbs: [ws, { label: name }] };
    }
  }
}
