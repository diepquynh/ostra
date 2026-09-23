// Resource ids name everything the console can open in a tab. The URL of the active tab is derived
// from its id, and a URL maps back to exactly one id, so reloads and deep links restore the same tab.
//
//   ws:overview | ws:cost | ws:settings | ws:memory   /w/:ws, /w/:ws/cost, /w/:ws/settings, /w/:ws/memory
//   session:<id>                                     /w/:ws/s/:id
//   exec:<id>                                        /w/:ws/x/:id
//   artifact:<absolute path>                         /w/:ws/artifact?path=<path>
//   project:<key>                                    /w/:ws/p/:key
//   file:<key>:<project-relative path>               /w/:ws/f/:key/<path>

export type WorkspacePage = "overview" | "cost" | "settings" | "memory";

export type Resource =
  | { type: "ws"; page: WorkspacePage }
  | { type: "session"; id: string }
  | { type: "exec"; id: string }
  | { type: "artifact"; path: string }
  | { type: "project"; key: string }
  | { type: "file"; key: string; path: string };

const PAGES: WorkspacePage[] = ["overview", "cost", "settings", "memory"];

/** Parse a resource id. Returns null for anything that is not one. */
export function parseResource(id: string): Resource | null {
  const colon = id.indexOf(":");
  if (colon <= 0) return null;
  const type = id.slice(0, colon);
  const ref = id.slice(colon + 1);
  if (!ref) return null;
  switch (type) {
    case "ws":
      return (PAGES as string[]).includes(ref) ? { type: "ws", page: ref as WorkspacePage } : null;
    case "session":
      return { type: "session", id: ref };
    case "exec":
      return { type: "exec", id: ref };
    case "artifact":
      return { type: "artifact", path: ref };
    case "project":
      return { type: "project", key: ref };
    case "file": {
      // Project keys never contain a colon; the path may.
      const sep = ref.indexOf(":");
      if (sep <= 0 || sep === ref.length - 1) return null;
      return { type: "file", key: ref.slice(0, sep), path: ref.slice(sep + 1) };
    }
    default:
      return null;
  }
}

export function resourceId(r: Resource): string {
  switch (r.type) {
    case "ws":
      return `ws:${r.page}`;
    case "session":
      return `session:${r.id}`;
    case "exec":
      return `exec:${r.id}`;
    case "artifact":
      return `artifact:${r.path}`;
    case "project":
      return `project:${r.key}`;
    case "file":
      return `file:${r.key}:${r.path}`;
  }
}

export const fileId = (key: string, path: string) => `file:${key}:${path}`;

const enc = encodeURIComponent;
const encPath = (p: string) => p.split("/").map(enc).join("/");

/** The URL (path and query) of a resource inside workspace `ws`. `anchor` becomes the hash. */
export function resourcePath(ws: string, id: string, anchor?: string | null): string {
  const base = `/w/${enc(ws)}`;
  const r = parseResource(id);
  const hash = anchor ? `#${anchor}` : "";
  if (!r) return base + hash;
  switch (r.type) {
    case "ws":
      return (r.page === "overview" ? base : `${base}/${r.page}`) + hash;
    case "session":
      return `${base}/s/${enc(r.id)}${hash}`;
    case "exec":
      return `${base}/x/${enc(r.id)}${hash}`;
    case "artifact":
      return `${base}/artifact?path=${enc(r.path)}${hash}`;
    case "project":
      return `${base}/p/${enc(r.key)}${hash}`;
    case "file":
      return `${base}/f/${enc(r.key)}/${encPath(r.path)}${hash}`;
  }
}

const dec = (s: string) => {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
};

/** The workspace and resource id a URL names, or null when the URL is not a workspace resource. */
export function resourceFromPath(pathname: string, search = ""): { ws: string; id: string } | null {
  const m = /^\/w\/([^/]+)(?:\/(.*))?$/.exec(pathname);
  if (!m) return null;
  const ws = dec(m[1]);
  const rest = (m[2] ?? "").replace(/\/+$/, "");
  if (rest === "") return { ws, id: "ws:overview" };
  const [head, ...tail] = rest.split("/");
  if (tail.length === 0 && (PAGES as string[]).includes(head) && head !== "overview") return { ws, id: `ws:${head}` };
  if (head === "s" && tail.length === 1) return { ws, id: `session:${dec(tail[0])}` };
  if (head === "x" && tail.length === 1) return { ws, id: `exec:${dec(tail[0])}` };
  if (head === "p" && tail.length === 1) return { ws, id: `project:${dec(tail[0])}` };
  if (head === "f" && tail.length >= 2) return { ws, id: fileId(dec(tail[0]), tail.slice(1).map(dec).join("/")) };
  if (head === "artifact" && tail.length === 0) {
    const path = new URLSearchParams(search).get("path");
    return path ? { ws, id: `artifact:${path}` } : null;
  }
  return null;
}
