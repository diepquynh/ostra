import { parentDir } from "@ostra/design";
import { moveTarget } from "../../shell/FilesPanel";

export type ProjectTab = "overview" | "files" | "git";

/** Where a project screen stands, from the URL hash: `#files`, `#git`, or `#dir=<folder>` inside Files. */
export type ProjectView = { tab: ProjectTab; dir: string };

export function parseProjectHash(hash: string): ProjectView {
  const h = hash.replace(/^#/, "");
  if (h === "git") return { tab: "git", dir: "" };
  if (h === "files") return { tab: "files", dir: "" };
  if (h.startsWith("dir=")) {
    let dir = h.slice(4);
    try {
      dir = decodeURIComponent(dir);
    } catch {
      // A hand-typed hash keeps its raw text.
    }
    dir = dir.replace(/^\/+|\/+$/g, "");
    return { tab: "files", dir };
  }
  return { tab: "overview", dir: "" };
}

export function projectHash(v: ProjectView): string {
  if (v.dir) return `#dir=${encodeURIComponent(v.dir)}`;
  return v.tab === "overview" ? "" : `#${v.tab}`;
}

/** A typed name for New file or New folder, trimmed of slashes; null when nothing is left. */
export function cleanName(raw: string): string | null {
  const name = raw
    .trim()
    .replace(/^\/+|\/+$/g, "")
    .replace(/\/{2,}/g, "/");
  return name ? name : null;
}

/**
 * Folders an artifact can move into: the top level and every folder the file list and the loaded listings show,
 * except its own folder, itself, and anything inside it.
 */
export function moveDestinations(from: string, filePaths: string[], knownDirs: string[]): string[] {
  const dirs = new Set<string>([""]);
  for (const p of filePaths) for (let d = parentDir(p); d; d = parentDir(d)) dirs.add(d);
  for (const d of knownDirs) if (d) dirs.add(d);
  return [...dirs].filter((d) => moveTarget(from, d) !== null).sort((a, b) => a.localeCompare(b));
}

/** The stack depth the mobile shell keeps in each history entry's state. */
export const stackDepth = (state: unknown): number => (state as { depth?: number } | null)?.depth ?? 0;
