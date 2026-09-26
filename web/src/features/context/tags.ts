import type { ContextFile } from "../../api/types";

/** The tag root of workspace artifacts: `@_artifacts/<path>`. No project key can take it. */
export const ARTIFACTS_ROOT = "_artifacts";

/** `@project/path`, the tag that names a file in request text. */
export const tagOf = (f: ContextFile) => `@${f.project}/${f.path}`;
export const fileKey = (f: ContextFile) => `${f.project}/${f.path}`;
export const isFolder = (f: ContextFile) => f.path.endsWith("/");
/** The last path segment; a folder keeps its trailing `/`. */
export const baseName = (path: string) => {
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path;
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) + (path.endsWith("/") ? "/" : "");
};

/** The folders holding `files`, each once and ending in `/`, followed by the files. */
export function withFolders(files: ContextFile[]): ContextFile[] {
  const seen = new Set<string>();
  const folders: ContextFile[] = [];
  for (const f of files) {
    const parts = f.path.split("/");
    for (let i = 1; i < parts.length; i++) {
      const path = `${parts.slice(0, i).join("/")}/`;
      const key = `${f.project}/${path}`;
      if (seen.has(key)) continue;
      seen.add(key);
      folders.push({ project: f.project, path });
    }
  }
  return [...folders, ...files];
}

/** The drag type a Files panel row carries, so a request field can tag what is dropped on it. */
export const CONTEXT_DRAG_TYPE = "application/x-ostra-context";

export function setContextDrag(dt: DataTransfer, f: ContextFile) {
  dt.setData(CONTEXT_DRAG_TYPE, JSON.stringify(f));
  dt.setData("text/plain", tagOf(f));
  // An artifact also moves when dropped on another folder of the Artifacts tab.
  dt.effectAllowed = f.project === ARTIFACTS_ROOT ? "copyMove" : "copy";
}

export function readContextDrag(dt: DataTransfer): ContextFile | null {
  try {
    const v = JSON.parse(dt.getData(CONTEXT_DRAG_TYPE)) as ContextFile;
    return typeof v?.project === "string" && typeof v?.path === "string" ? v : null;
  } catch {
    return null;
  }
}

const TAG = /(^|\s)@([A-Za-z0-9._-]+)\/(\S+)/g;
const TRAILING = /[.,;:!?)\]}"'`]+$/;

export type Segment = { text: string } | { file: ContextFile; raw: string };

/**
 * A tag's file. Sentence punctuation after a tag is not part of the path, so a path that is not in
 * `known` is retried without it.
 */
function resolve(project: string, path: string, known: Set<string> | null): { path: string; rest: string } | null {
  if (!known || known.has(`${project}/${path}`)) return { path, rest: "" };
  const trimmed = path.replace(TRAILING, "");
  if (trimmed && trimmed !== path && known.has(`${project}/${trimmed}`))
    return { path: trimmed, rest: path.slice(trimmed.length) };
  return null;
}

/**
 * Text split into plain runs and file tags. With `known`, only tags naming a known file become
 * segments; without it, every tag of a listed project does, minus trailing punctuation.
 */
export function splitTags(text: string, projects: string[], known: Set<string> | null = null): Segment[] {
  const out: Segment[] = [];
  let last = 0;
  for (const m of text.matchAll(TAG)) {
    const [, lead, project, rawPath] = m;
    if (!projects.includes(project)) continue;
    const r = known ? resolve(project, rawPath, known) : { path: rawPath.replace(TRAILING, ""), rest: "" };
    if (!r?.path) continue;
    const rest = r.rest || rawPath.slice(r.path.length);
    const start = (m.index ?? 0) + lead.length;
    if (start > last) out.push({ text: text.slice(last, start) });
    const raw = `@${project}/${r.path}`;
    out.push({ file: { project, path: r.path }, raw });
    last = start + raw.length;
    if (rest) {
      out.push({ text: rest });
      last += rest.length;
    }
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/** The distinct files tagged in `text`, in order of first mention. */
export function filesInText(text: string, projects: string[], known: Set<string> | null = null): ContextFile[] {
  const seen = new Set<string>();
  const out: ContextFile[] = [];
  for (const s of splitTags(text, projects, known)) {
    if (!("file" in s) || seen.has(fileKey(s.file))) continue;
    seen.add(fileKey(s.file));
    out.push(s.file);
  }
  return out;
}

/** Remove every tag of `file` from `text`, with the space after it. */
export function removeTag(text: string, file: ContextFile): string {
  const tag = tagOf(file).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return text.replace(new RegExp(`(^|\\s)${tag}(?=\\s|$|[.,;:!?)])\\s?`, "g"), "$1");
}

/** The `@query` being typed just before the caret, if any. */
export function activeQuery(text: string, caret: number): { start: number; query: string } | null {
  const m = /(^|\s)@([^\s@]*)$/.exec(text.slice(0, caret));
  if (!m) return null;
  return { start: caret - m[2].length - 1, query: m[2] };
}

/** Files matching a query, best first: basename prefix, then basename substring, then path substring. */
export function rankFiles(files: ContextFile[], query: string, limit = 8): ContextFile[] {
  const q = query.toLowerCase();
  if (!q) return files.slice(0, limit);
  const scored: [number, ContextFile][] = [];
  for (const f of files) {
    const full = fileKey(f).toLowerCase();
    const base = baseName(f.path).toLowerCase();
    const score = base.startsWith(q) ? 0 : base.includes(q) ? 1 : full.includes(q) ? 2 : -1;
    if (score >= 0) scored.push([score * 10000 + full.length, f]);
    if (scored.length > 5000) break;
  }
  return scored
    .sort((a, b) => a[0] - b[0])
    .slice(0, limit)
    .map(([, f]) => f);
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}
