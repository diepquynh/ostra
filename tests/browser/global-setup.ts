/**
 * Builds Ostra, starts a scratch server with a fake model API, and drives one YOLO session whose
 * every agent output carries test strings. Specs only render what this leaves behind.
 */
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { Api, mintUrl, signIn } from "./lib/api";
import { startAttacker } from "./lib/attacker";
import { OSTRA_BIN, ostraEnv, REPO_ROOT, ROOT, STATE_FILE, type SuiteState } from "./lib/env";
import { startFakeModel } from "./lib/fake-model";
import { htmlFile, inline, markdown, mermaid, svgFile } from "./lib/payloads";
import { BRANCH, makeRepo, makeStubHarness, makeStubMcp, REPO, writeConfig } from "./lib/scenario";
import { buildSite, startSite } from "./lib/site";

const freePort = () =>
  new Promise<number>((resolve) => {
    const s = net.createServer();
    s.listen(0, "127.0.0.1", () => {
      const p = (s.address() as net.AddressInfo).port;
      s.close(() => resolve(p));
    });
  });

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function build() {
  if (process.env.PW_SKIP_BUILD) return;
  execFileSync("npm", ["run", "build"], { cwd: path.join(REPO_ROOT, "web"), stdio: "inherit" });
  execFileSync("cargo", ["build", "-p", "ostra-server"], {
    cwd: REPO_ROOT,
    env: { ...process.env, CARGO_TARGET_DIR: path.join(REPO_ROOT, "target-browser") },
    stdio: "inherit",
  });
}

function stopOld() {
  const pidFile = path.join(ROOT, "server.pid");
  if (!fs.existsSync(pidFile)) return;
  const pid = Number(fs.readFileSync(pidFile, "utf8"));
  try {
    // Only a server this suite started: its command line names the scratch binary.
    const cmd = fs.readFileSync(`/proc/${pid}/cmdline`, "utf8");
    if (cmd.includes("ostra") && cmd.includes("serve")) process.kill(pid, "SIGTERM");
  } catch {}
}

async function waitFor<T>(what: string, f: () => Promise<T | undefined>, ms = 60_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    try {
      const v = await f();
      if (v !== undefined) return v;
    } catch {}
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(200);
  }
}

export default async function globalSetup() {
  build();
  stopOld();
  if (!ROOT.startsWith("/tmp/")) throw new Error("scratch root must be under /tmp");
  fs.rmSync(ROOT, { recursive: true, force: true });
  fs.mkdirSync(path.join(ROOT, "home"), { recursive: true });
  fs.mkdirSync(path.join(ROOT, "data"), { recursive: true });

  const fake = await startFakeModel();
  const attacker = await startAttacker();
  process.env.PW_FAKE_ANTHROPIC_URL = fake.origin;
  // The site's test page carries canary URLs on the fake's port, so the site builds once the fake is up.
  buildSite(fake.origin);
  const site = await startSite();

  makeRepo(fake.origin);
  const harness = makeStubHarness(fake.origin);
  const mcp = makeStubMcp(fake.origin);
  writeConfig(harness);

  const port = await freePort();
  const loopback = `http://127.0.0.1:${port}`;
  const log = fs.openSync(path.join(ROOT, "server.out"), "a");
  const child = spawn(OSTRA_BIN, ["serve", "--no-open", "--port", String(port), "--bind", "127.0.0.1"], {
    env: ostraEnv(),
    stdio: ["ignore", log, log],
    detached: true,
  });
  child.unref();
  fs.writeFileSync(path.join(ROOT, "server.pid"), String(child.pid));
  await waitFor("server", async () => ((await fetch(`${loopback}/`, { redirect: "manual" })).status ? true : undefined));
  await waitFor("server.json", async () => (fs.existsSync(path.join(ROOT, "data/server.json")) ? true : undefined));
  // A sign-in link names the private host that page loads on 127.0.0.1 redirect to.
  const app = new URL(mintUrl()).origin;
  if (!app.endsWith(`.localhost:${port}`)) throw new Error(`unexpected sign-in origin ${app}`);

  const cookie = await signIn(app);
  const api = new Api(app, cookie);
  const storageState = path.join(ROOT, "storage-state.json");
  fs.writeFileSync(
    storageState,
    JSON.stringify({
      cookies: [
        {
          name: "ostra_session",
          value: cookie,
          domain: new URL(app).hostname,
          path: "/",
          expires: Math.floor(Date.now() / 1000) + 86400,
          httpOnly: true,
          secure: false,
          sameSite: "Strict",
        },
      ],
      origins: [],
    }),
  );

  let ws: any;
  try {
    ws = await api.post("/api/workspaces", { name: inline("wsname"), root: path.join(ROOT, "ws") });
  } catch {
    ws = await api.post("/api/workspaces", { name: "pw", root: path.join(ROOT, "ws") });
  }
  ws = await api.post(`/api/workspaces/${ws.id}/projects`, { path: REPO, key: "app", stack: null });
  if (ws.validation?.length) throw new Error(`workspace invalid: ${JSON.stringify(ws.validation)}`);
  await api.post("/api/onboarding/complete");

  const settings = (await api.get(`/api/workspaces/${ws.id}`)).settings;
  settings.mcp_servers = [{ name: "pw", enabled: true, command: [process.execPath, mcp], timeout_secs: 20 }];
  await api.req("PATCH", `/api/workspaces/${ws.id}`, settings);

  // Lessons and skills: text agents write and the Memory and Skills screens render.
  await api
    .req("PATCH", `/api/workspaces/${ws.id}/projects/app/memory`, {
      id: null,
      area: inline("mem-area"),
      lesson: markdown("memory", fake.origin),
    })
    .catch((e) => console.warn(`[pw] no lesson: ${e}`));
  await api
    .req("PUT", `/api/workspaces/${ws.id}/projects/app/skills/pw-skill`, {
      kind: "other",
      component_type: null,
      content: `---\nname: pw-skill\ndescription: ${inline("skill-desc").replace(/"/g, "'")}\n---\n\n${markdown("skill", fake.origin)}`,
    })
    .catch((e) => console.warn(`[pw] no skill: ${e}`));

  const uploads = [];
  for (const [name, body] of [
    ["evil.svg", svgFile("upload", fake.origin)],
    ["evil.html", htmlFile("upload", fake.origin)],
    ["evil.md", markdown("upload-md", fake.origin)],
    [`name <img src=x onerror=window.__pwned='upname'>.txt`, "x"],
  ]) {
    uploads.push(
      await api.req("POST", `/api/workspaces/${ws.id}/uploads?name=${encodeURIComponent(name)}`, undefined, body),
    );
  }

  const s = await api.post(`/api/workspaces/${ws.id}/sessions`, {
    request: markdown("request", fake.origin),
    options: { tests: false, docs: false, yolo: true },
    projects: [],
    uploads: uploads.map((u: any) => u.id),
  });
  const detail = await waitFor(
    "session end",
    async () => {
      const d = await api.get(`/api/sessions/${s.id}`);
      return ["completed", "failed", "stalled"].includes(d.summary.status) ? d : undefined;
    },
    120_000,
  );
  if (detail.summary.status !== "completed")
    console.warn(`[pw] session ended ${detail.summary.status}; rendering what it produced`);

  writeBook(path.join(ROOT, "ws"), fake.origin);

  const sessionDir = findSessionDir(path.join(ROOT, "ws"), s.id);
  const state: SuiteState = {
    app,
    appPort: port,
    site: site.origin,
    pid: child.pid ?? 0,
    fake: fake.origin,
    fakePort: fake.port,
    attackerPort: attacker.port,
    storageState,
    ws: ws.id,
    project: "app",
    repo: REPO,
    session: s.id,
    sessionDir,
    uploads: listFiles(sessionDir).filter((f) => /evil|name /.test(path.basename(f))).map((f) => ({ name: path.basename(f), path: f })),
    executions: detail.executions.map((e: any) => ({ id: e.id, agent: e.agent })),
    branch: BRANCH,
    mcp,
  };
  fs.writeFileSync(STATE_FILE, JSON.stringify(state, null, 2));

  return async () => {
    if (!process.env.PW_KEEP_SERVER) {
      try {
        process.kill(state.pid, "SIGTERM");
      } catch {}
    }
    await fake.close();
    await attacker.close();
    await site.close();
  };
}

/**
 * A documentation book whose every field carries test strings, written where the engine writes books. The Docs view
 * and the exported HTML render it.
 */
function writeBook(ws: string, fake: string) {
  const diagrams = [...mermaid("book", fake).matchAll(/```mermaid\n([\s\S]*?)```/g)].map((m, i) => ({
    title: inline(`book-diagram-${i}`),
    kind: "flowchart",
    source: m[1],
  }));
  const unit = (tag: string) => ({
    id: tag,
    title: inline(`${tag}-title`),
    purpose: markdown(tag, fake),
    boundaries: { owns: [inline(`${tag}-owns`)], does_not_own: [markdown(`${tag}-notown`, fake)] },
    assumptions: [markdown(`${tag}-assume`, fake)],
    business_flow: [{ actor: inline(`${tag}-actor`), action: markdown(`${tag}-action`, fake), outcome: inline(`${tag}-out`) }],
    diagrams,
    tables: [{ title: inline(`${tag}-table`), columns: [inline(`${tag}-col`)], rows: [[markdown(`${tag}-cell`, fake)]] }],
    concerns: [{ component: inline(`${tag}-comp`), responsibility: inline(`${tag}-resp`) }],
    code_refs: [{ path: "src/<img src=x onerror=window.__pwned='ref'>.rs", symbol: "a`b|c", lines: "1-2", note: inline(`${tag}-note`) }],
  });
  const now = new Date().toISOString();
  const book = {
    id: "pw_book",
    title: inline("book-title"),
    projects: ["app"],
    updated_at: now,
    sessions: [],
    architecture: {
      overview: markdown("book-arch", fake),
      diagram: diagrams[0],
      components: [{ name: inline("book-c"), project: "app", role: inline("book-role"), owns: [inline("book-own")] }],
      links: [{ from: inline("book-c"), to: inline("book-c"), protocol: inline("book-proto"), mode: "sync", payload: inline("book-pay") }],
      failure_recovery: [{ failure: inline("book-f"), detection: inline("book-d"), recovery: inline("book-r") }],
      scalability: [{ component: inline("book-c"), scales_by: inline("book-s"), limit: inline("book-l") }],
    },
    parts: [{ project: "app", overview: markdown("book-overview", fake), updated_at: now, sections: [{ ...unit("book"), subsections: [unit("book-sub")] }] }],
    glossary: [{ term: inline("book-term"), definition: markdown("book-def", fake), code_ref: inline("book-code") }],
  };
  const dir = path.join(ws, ".ostra", "docs", book.id);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "book.json"), JSON.stringify(book, null, 2));
}

function listFiles(dir: string): string[] {
  const out: string[] = [];
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) out.push(...listFiles(p));
    else out.push(p);
  }
  return out;
}

function findSessionDir(root: string, id: string): string {
  const hit = (dir: string): string | null => {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      if (!e.isDirectory()) continue;
      const p = path.join(dir, e.name);
      if (e.name === id) return p;
      const r = hit(p);
      if (r) return r;
    }
    return null;
  };
  const d = hit(root);
  if (!d) throw new Error(`no session dir for ${id} under ${root}`);
  return d;
}
