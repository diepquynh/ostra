import type { GitMark, ProjectTreeEntry } from "../../api/types";
import type { IconName } from "../../design";

const FILE_ICON: Record<string, IconName> = {
  rs: "file-code-2",
  ts: "file-code-2",
  tsx: "file-code-2",
  js: "file-code-2",
  jsx: "file-code-2",
  py: "file-code-2",
  go: "file-code-2",
  java: "file-code-2",
  kt: "file-code-2",
  sql: "database",
  toml: "file-cog",
  yaml: "file-cog",
  yml: "file-cog",
  json: "file-json",
  md: "file-text",
  txt: "file-text",
};

/** Lowercase extension of the last path segment, or "" when it has none. */
export function extOf(path: string): string {
  const name = path.split("/").pop() ?? "";
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

export const fileIcon = (path: string): IconName => FILE_ICON[extOf(path)] ?? "file";

export const entryIcon = (e: Pick<ProjectTreeEntry, "is_dir" | "name">, open = false): IconName =>
  e.is_dir ? (open ? "folder-open" : "folder") : fileIcon(e.name);

export const GIT_MARK: Record<GitMark, { letter: string; color: string; word: string }> = {
  M: { letter: "M", color: "var(--warn)", word: "modified" },
  A: { letter: "A", color: "var(--ok)", word: "added" },
  "?": { letter: "U", color: "var(--ok)", word: "untracked" },
  D: { letter: "D", color: "var(--bad)", word: "deleted" },
  R: { letter: "R", color: "var(--info)", word: "renamed" },
};

export function fmtSize(b: number): string {
  if (b < 1024) return `${b} B`;
  if (b < 1048576) return `${(b / 1024).toFixed(1)} KB`;
  return `${(b / 1048576).toFixed(1)} MB`;
}

const DAY = 86_400_000;

/** "today", "yesterday", "3 days ago", "2 weeks ago", or the date for anything older than eight weeks. */
export function modifiedLabel(iso: string | null, nowMs: number = Date.now()): string {
  if (!iso) return "";
  const t = new Date(iso);
  const startOf = (ms: number) => {
    const d = new Date(ms);
    d.setHours(0, 0, 0, 0);
    return d.getTime();
  };
  const days = Math.round((startOf(nowMs) - startOf(t.getTime())) / DAY);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 7) return `${days} days ago`;
  if (days < 56) {
    const w = Math.floor(days / 7);
    return w === 1 ? "1 week ago" : `${w} weeks ago`;
  }
  return t.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

/** The Kind column: Folder, Link, or the extension in capitals. */
export function kindOf(e: Pick<ProjectTreeEntry, "is_dir" | "is_symlink" | "name">): string {
  if (e.is_dir) return "Folder";
  if (e.is_symlink) return "Link";
  const ext = extOf(e.name);
  return ext ? ext.toUpperCase() : "File";
}

export type SortKey = "name" | "kind" | "size" | "modified";
export type Sort = { key: SortKey; dir: 1 | -1 };

/** Folders first, then by the sort key; ties fall back to the name so the order is stable. */
export function sortEntries<T extends Pick<ProjectTreeEntry, "is_dir" | "is_symlink" | "name" | "size" | "modified">>(entries: T[], sort: Sort): T[] {
  const byName = (a: T, b: T) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
  const cmp = (a: T, b: T): number => {
    switch (sort.key) {
      case "size":
        return a.size - b.size;
      case "modified":
        return (a.modified ?? "").localeCompare(b.modified ?? "");
      case "kind":
        return kindOf(a).localeCompare(kindOf(b));
      default:
        return 0;
    }
  };
  return [...entries].sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    return (cmp(a, b) || byName(a, b)) * sort.dir;
  });
}

/** The next sort after clicking a column header: the same column flips, a new one starts ascending. */
export const toggleSort = (s: Sort, key: SortKey): Sort => (s.key === key ? { key, dir: s.dir === 1 ? -1 : 1 } : { key, dir: 1 });

/** Every ancestor folder of a project-relative path, outermost first. */
export function parentsOf(path: string): string[] {
  const segs = path.split("/").filter(Boolean);
  return segs.slice(0, -1).map((_, i) => segs.slice(0, i + 1).join("/"));
}

export const parentOf = (path: string) => path.split("/").slice(0, -1).join("/");

export const isHiddenPath = (path: string) => path.split("/").some((s) => s.startsWith("."));
