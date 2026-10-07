// Resource ids name everything the console can open in a tab. The URL of the active tab is derived
// from its id, and a URL maps back to exactly one id, so reloads and deep links restore the same tab.
//
//   ws:overview | ws:cost | ws:settings | ws:memory | ws:skills | ws:docs | ws:agents | ws:workflows
//                                                    /w/:ws, /w/:ws/<page>; ws:workflows#<name> opens the Workflow builder
//   book:<id>                                        /w/:ws/b/:id   (a documentation book)
//   session:<id>                                     /w/:ws/s/:id
//   exec:<id>                                        /w/:ws/x/:id
//   artifact:<absolute path>                         /w/:ws/artifact?path=<path>
//   project:<key>                                    /w/:ws/p/:key
//   file:<key>:<project-relative path>               /w/:ws/f/:key/<path>
//   dep:<key>:<language server URI>                  /w/:ws/d/:key?uri=<uri>   (a dependency file, read-only)

export type WorkspacePage = "overview" | "cost" | "settings" | "memory" | "skills" | "docs" | "agents" | "workflows";

export type Resource =
  | { type: "ws"; page: WorkspacePage }
  | { type: "session"; id: string }
  | { type: "exec"; id: string }
  | { type: "book"; id: string }
  | { type: "artifact"; path: string }
  | { type: "project"; key: string }
  | { type: "file"; key: string; path: string }
  | { type: "dep"; key: string; uri: string };

const PAGES: WorkspacePage[] = ["overview", "cost", "settings", "memory", "skills", "docs", "agents", "workflows"];

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
    case "book":
      return { type: "book", id: ref };
    case "artifact":
      return { type: "artifact", path: ref };
    case "project":
      return { type: "project", key: ref };
    case "file":
    case "dep": {
      // Project keys never contain a colon; the path or URI may.
      const sep = ref.indexOf(":");
      if (sep <= 0 || sep === ref.length - 1) return null;
      const key = ref.slice(0, sep);
      const rest = ref.slice(sep + 1);
      return type === "file" ? { type: "file", key, path: rest } : { type: "dep", key, uri: rest };
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
    case "book":
      return `book:${r.id}`;
    case "artifact":
      return `artifact:${r.path}`;
    case "project":
      return `project:${r.key}`;
    case "file":
      return `file:${r.key}:${r.path}`;
    case "dep":
      return `dep:${r.key}:${r.uri}`;
  }
}

export const fileId = (key: string, path: string) => `file:${key}:${path}`;
export const depId = (key: string, uri: string) => `dep:${key}:${uri}`;

/** The file name at the end of a dependency URI, such as `ObjectMapper.class` or `print.go`. */
export function depName(uri: string): string {
  const last = uri.split(/[?#]/)[0].split("/").filter(Boolean).pop() ?? uri;
  try {
    return decodeURIComponent(last);
  } catch {
    return last;
  }
}

/** The tab for a code location: a project file, or a dependency file when the location has a URI. */
export const locationId = (key: string, l: { path: string; uri?: string | null }) =>
  l.uri ? depId(key, l.uri) : fileId(key, l.path);

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
    case "book":
      return `${base}/b/${enc(r.id)}${hash}`;
    case "artifact":
      return `${base}/artifact?path=${enc(r.path)}${hash}`;
    case "project":
      return `${base}/p/${enc(r.key)}${hash}`;
    case "file":
      return `${base}/f/${enc(r.key)}/${encPath(r.path)}${hash}`;
    case "dep":
      return `${base}/d/${enc(r.key)}?uri=${enc(r.uri)}${hash}`;
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
  if (head === "b" && tail.length === 1) return { ws, id: `book:${dec(tail[0])}` };
  if (head === "p" && tail.length === 1) return { ws, id: `project:${dec(tail[0])}` };
  if (head === "f" && tail.length >= 2) return { ws, id: fileId(dec(tail[0]), tail.slice(1).map(dec).join("/")) };
  if (head === "d" && tail.length === 1) {
    const uri = new URLSearchParams(search).get("uri");
    return uri ? { ws, id: depId(dec(tail[0]), uri) } : null;
  }
  if (head === "artifact" && tail.length === 0) {
    const path = new URLSearchParams(search).get("path");
    return path ? { ws, id: `artifact:${path}` } : null;
  }
  return null;
}
