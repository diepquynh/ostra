import dns from "node:dns";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/** Everything the suite creates lives here, so a run never touches the user's own Ostra data. */
export const ROOT = "/tmp/pw-browser";
export const STATE_FILE = path.join(ROOT, "state.json");
export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
export const OSTRA_BIN = process.env.OSTRA_BIN ?? path.join(REPO_ROOT, "target-browser/debug/ostra");
export const CHROME =
  process.env.PW_CHROME ?? path.join(process.env.HOME ?? "", ".cache/ms-playwright/chromium-1217/chrome-linux64/chrome");
/** A hostname only this browser resolves, mapped to 127.0.0.1 with `--host-resolver-rules`. */
export const EVIL_HOST = "evil-ostra.test";

// Ostra serves its pages on a private `ostra-<hex>.localhost` name. Chromium maps `*.localhost` to
// loopback itself, but this machine's resolver may answer `::1` only while the scratch server
// listens on 127.0.0.1, so the suite's own requests map the name the same way.
const systemLookup = dns.lookup;
(dns as { lookup: unknown }).lookup = (host: string, opts: unknown, cb?: unknown) => {
  const done = (typeof opts === "function" ? opts : cb) as (...a: unknown[]) => void;
  const o = (typeof opts === "function" ? {} : (opts ?? {})) as { all?: boolean };
  if (/\.localhost$/i.test(host)) {
    return o.all ? done(null, [{ address: "127.0.0.1", family: 4 }]) : done(null, "127.0.0.1", 4);
  }
  return (systemLookup as (...a: unknown[]) => void)(host, opts, cb);
};

/** The environment every `ostra` process of the suite runs with. */
export function ostraEnv(): NodeJS.ProcessEnv {
  return {
    PATH: `${path.join(ROOT, "bin")}:/usr/local/bin:/usr/bin:/bin`,
    HOME: path.join(ROOT, "home"),
    OSTRA_CONFIG: path.join(ROOT, "config.toml"),
    OSTRA_DATA_DIR: path.join(ROOT, "data"),
    OSTRA_MASTER_KEY_FILE: path.join(ROOT, "master.key"),
    // No price catalog download: nothing in a run leaves this machine.
    OSTRA_MODELS_DEV_URL: "",
    OSTRA_LOG: "info",
    PW_FAKE_ANTHROPIC_KEY: "sk-ant-fake-browser-suite",
    PW_FAKE_ANTHROPIC_URL: process.env.PW_FAKE_ANTHROPIC_URL ?? "",
    GIT_CONFIG_NOSYSTEM: "1",
  };
}

export interface SuiteState {
  /** The Ostra server at its private name, `http://ostra-<hex>.localhost:<port>`. */
  app: string;
  appPort: number;
  /** The homepage and docs (site/), served as static files the way a static host does. */
  site: string;
  pid: number;
  /** The fake model API, which also counts canary hits. */
  fake: string;
  fakePort: number;
  /** The attacker page server, reached as 127.0.0.1 and as EVIL_HOST. */
  attackerPort: number;
  storageState: string;
  ws: string;
  project: string;
  repo: string;
  session: string;
  sessionDir: string;
  uploads: { name: string; path: string }[];
  executions: { id: string; agent: string }[];
  branch: string;
  mcp: string;
}

export function readState(): SuiteState {
  return JSON.parse(fs.readFileSync(STATE_FILE, "utf8")) as SuiteState;
}
