/**
 * The attacker's web server: serves `ROOT/attacker/<name>.html` on its own port, which the browser
 * also reaches as `EVIL_HOST` (another site) through `--host-resolver-rules`.
 */
import fs from "node:fs";
import http from "node:http";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { ROOT } from "./env";

export const ATTACKER_DIR = path.join(ROOT, "attacker");

export async function startAttacker(): Promise<{ port: number; close: () => Promise<void> }> {
  fs.mkdirSync(ATTACKER_DIR, { recursive: true });
  const server = http.createServer((req, res) => {
    const name = path.basename(new URL(req.url ?? "/", "http://x").pathname);
    const file = path.join(ATTACKER_DIR, name);
    if (!name || !fs.existsSync(file)) {
      res.writeHead(404);
      res.end();
      return;
    }
    res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
    res.end(fs.readFileSync(file));
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  return {
    port: (server.address() as AddressInfo).port,
    close: () => new Promise((r) => server.close(() => r())),
  };
}

/** Write an attacker page and return its path on the attacker server. */
export function attackerPage(name: string, html: string): string {
  fs.mkdirSync(ATTACKER_DIR, { recursive: true });
  fs.writeFileSync(path.join(ATTACKER_DIR, `${name}.html`), html);
  return `/${name}.html`;
}
