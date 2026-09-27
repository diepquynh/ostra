/**
 * The site (site/) as a static host serves it: plain files, no headers beyond a content type, so the only policy is
 * what the pages carry. Built with a docs page of test strings, into the scratch root.
 */
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { REPO_ROOT, ROOT } from "./env";
import { markdown, mermaid } from "./payloads";

export const SITE_DIR = path.join(ROOT, "site");
/** Its own site: Chromium resolves any `*.localhost` to loopback. */
export const SITE_HOST = "ostra-site.localhost";

const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".ttf": "font/ttf",
  ".json": "application/json",
  ".png": "image/png",
};

/** The console shot is built into site/public/console by the build step; the pages here, with the test page. */
export function buildSite(fake: string) {
  const site = path.join(REPO_ROOT, "site");
  if (!process.env.PW_SKIP_BUILD) execFileSync("npm", ["run", "build:console"], { cwd: site, stdio: "inherit" });
  if (!fs.existsSync(path.join(site, "public/console/index.html")))
    throw new Error("site/public/console is missing: run the suite once without PW_SKIP_BUILD");
  execFileSync("npx", ["vite", "build", "--outDir", SITE_DIR, "--emptyOutDir"], {
    cwd: site,
    env: { ...process.env, VITE_TEST_DOC: `${markdown("docs", fake)}\n\n${mermaid("docs", fake)}` },
    stdio: "inherit",
  });
}

export async function startSite(): Promise<{ origin: string; close: () => Promise<void> }> {
  const server = http.createServer((req, res) => {
    let rel = decodeURIComponent(new URL(req.url ?? "/", "http://x").pathname);
    if (rel.endsWith("/")) rel += "index.html";
    const file = path.resolve(SITE_DIR, `.${rel}`);
    if (!file.startsWith(`${SITE_DIR}/`) || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
      res.writeHead(404);
      res.end();
      return;
    }
    res.writeHead(200, { "content-type": TYPES[path.extname(file)] ?? "application/octet-stream" });
    res.end(fs.readFileSync(file));
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  return {
    origin: `http://${SITE_HOST}:${(server.address() as AddressInfo).port}`,
    close: () => new Promise((r) => server.close(() => r())),
  };
}
