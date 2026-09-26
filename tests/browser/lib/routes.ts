/** Every route in `crates/ostra-server/src/api.rs`, read from the router so a new route is covered too. */
import fs from "node:fs";
import path from "node:path";
import { REPO_ROOT, type SuiteState } from "./env";

export interface Route {
  path: string;
  methods: string[];
}

export function apiRoutes(): Route[] {
  const src = fs.readFileSync(path.join(REPO_ROOT, "crates/ostra-server/src/api.rs"), "utf8");
  const mcp = fs.readFileSync(path.join(REPO_ROOT, "crates/ostra-server/src/mcp.rs"), "utf8");
  const callback = /CALLBACK_PATH: &str = "([^"]+)"/.exec(mcp)?.[1] ?? "/mcp/oauth/callback";
  const router = src.slice(src.indexOf("pub fn router("), src.indexOf(".merge(crate::bridge::internal_routes())"));
  const out: Route[] = [];
  for (const seg of router.split(".route(").slice(1)) {
    const lit = /^\s*(?:"([^"]+)"|crate::mcp::CALLBACK_PATH)/.exec(seg);
    if (!lit) continue;
    const methods = [...seg.matchAll(/(?:^|[\s(.:])(get|post|put|patch|delete)\(/g)].map((m) => m[1].toUpperCase());
    out.push({ path: lit[1] ?? callback, methods: [...new Set(methods)] });
  }
  if (out.length < 60) throw new Error(`parsed only ${out.length} routes from api.rs`);
  return out;
}

/** A concrete URL path for a route, with ids from this run. */
export function fill(route: string, s: SuiteState): string {
  const exec = s.executions[0].id;
  return route
    .replace("{ws}", s.ws)
    .replace("{key}", s.project)
    .replace("{name}", "pw")
    .replace("{harness}", "claude")
    .replace("{id}", () => {
      if (route.startsWith("/api/sessions/")) return s.session;
      if (route.startsWith("/api/executions/")) return exec;
      return "pw-id";
    });
}

/** Query strings the GET handlers read, so a request that got past the guard would do real work. */
export function query(route: string, s: SuiteState): string {
  if (route.includes("/artifacts")) return `?path=${encodeURIComponent(s.uploads[0].path)}`;
  if (route.endsWith("/file") || route.includes("/code/")) return "?path=README.md&symbol=x";
  if (route.startsWith("/api/fs")) return "?path=%2Fetc";
  if (route.includes("/search")) return "?q=PW";
  if (route.includes("/diff")) return "?project=app&path=README.md";
  if (route.includes("callback")) return "?state=pw-forged&code=pw-forged";
  if (route.includes("harness-skill") || route.includes("/skills/")) return "?path=x";
  return "";
}
