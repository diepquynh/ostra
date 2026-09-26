/** A signed-in REST client for setup and for checking server-side state after an attack. */
import { execFileSync } from "node:child_process";
import { OSTRA_BIN, ostraEnv } from "./env";

export class Api {
  constructor(
    readonly base: string,
    readonly cookie: string,
  ) {}

  async req<T = any>(method: string, path: string, body?: unknown, raw?: BodyInit): Promise<T> {
    const r = await fetch(`${this.base}${path}`, {
      method,
      headers: {
        cookie: `ostra_session=${this.cookie}`,
        ...(body !== undefined ? { "content-type": "application/json" } : {}),
      },
      body: raw ?? (body !== undefined ? JSON.stringify(body) : undefined),
    });
    const text = await r.text();
    if (!r.ok) throw new Error(`${method} ${path} -> ${r.status}: ${text}`);
    return (text ? JSON.parse(text) : undefined) as T;
  }
  get = <T = any>(p: string) => this.req<T>("GET", p);
  post = <T = any>(p: string, b?: unknown) => this.req<T>("POST", p, b ?? {});
}

/** Mint a one-time sign-in URL the way a user would, with `ostra url`. */
export function mintUrl(): string {
  return execFileSync(OSTRA_BIN, ["url"], { env: ostraEnv(), encoding: "utf8" }).trim();
}

export async function signIn(base: string): Promise<string> {
  const token = mintUrl().split("#token=")[1];
  const r = await fetch(`${base}/api/auth/exchange`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token }),
  });
  if (r.status !== 204) throw new Error(`sign-in failed: ${r.status} ${await r.text()}`);
  const set = r.headers.get("set-cookie") ?? "";
  const m = /ostra_session=([^;]+)/.exec(set);
  if (!m) throw new Error(`no cookie in ${set}`);
  return m[1];
}
