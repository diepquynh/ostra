/**
 * Attacker pages on (a) another localhost port, same-site with the app, so its SameSite=Strict
 * cookie rides along, and (b) another site (a hostname mapped to 127.0.0.1). From each: form
 * POSTs, fetch in every mode, a WebSocket, img/script tags and top-level navigations to every GET
 * route, and a frame of the app. Every request must be refused by the server (seen from the
 * browser's side), nothing may be readable, and the server's state must not change. A rebinding
 * hostname on the app's own port must get 421.
 */
import fs from "node:fs";
import type { Page } from "@playwright/test";
import { Api, signIn } from "../lib/api";
import { attackerPage } from "../lib/attacker";
import { snapshot } from "../lib/effects";
import { EVIL_HOST } from "../lib/env";
import { expect, test } from "../lib/guard";
import { apiRoutes, fill, query } from "../lib/routes";

const cookieOf = (file: string) => JSON.parse(fs.readFileSync(file, "utf8")).cookies[0].value as string;

/** State-changing requests a browser can send cross-origin without a preflight. */
function postTargets(ws: string, session: string, exec: string) {
  return [
    "/api/auth/sessions/revoke-others",
    "/api/auth/signout",
    `/api/workspaces/${ws}/uploads?name=csrf.txt`,
    `/api/workspaces/${ws}/ask`,
    `/api/workspaces/${ws}/sessions`,
    `/api/workspaces/${ws}/projects/app/git/commit`,
    `/api/workspaces/${ws}/projects/app/git/checkout`,
    `/api/workspaces/${ws}/projects/app/git/stage`,
    `/api/workspaces/${ws}/projects/app/code/reindex`,
    `/api/workspaces/${ws}/mcp/pw/logout`,
    `/api/workspaces/${ws}/approve`,
    `/api/sessions/${session}/yolo`,
    `/api/sessions/${session}/amend`,
    `/api/executions/${exec}/resume`,
    "/api/workspaces",
    "/api/fs/mkdir",
    "/api/harnesses/claude/setup",
    "/api/onboarding/complete",
    "/api/push/subscribe",
    "/internal/policy",
    "/internal/mcp",
  ];
}

function attackHtml(app: string, posts: string[], gets: string[]) {
  const body = JSON.stringify({
    question: "CSRF",
    request: "CSRF",
    action: "login",
    message: "csrf",
    branch: "csrf",
    create: true,
    enabled: false,
    paths: ["README.md"],
    name: "csrf",
    root: "/tmp/pw-browser/csrf-ws",
    path: "/tmp/pw-browser/csrf-dir",
    endpoint: "https://127.0.0.1:9/push",
    keys: { p256dh: "x", auth: "x" },
  });
  return `<!doctype html><meta charset=utf-8><title>attacker</title><body>
<iframe name=sink style="width:10px;height:10px"></iframe>
<iframe id=frame-app src="${app}/" style="width:300px;height:200px"></iframe>
<iframe id=frame-api src="${app}/api/workspaces"></iframe>
${posts
  .map(
    (p, i) =>
      `<form id=f${i} method=POST target=sink enctype=text/plain action="${app}${p}"><input type=hidden name='${body.slice(0, -1)},"pad":"' value='"}'></form>`,
  )
  .join("\n")}
<script>
const APP = ${JSON.stringify(app)};
const POSTS = ${JSON.stringify(posts)};
const GETS = ${JSON.stringify(gets)};
const BODY = ${JSON.stringify(body)};
const results = { read: [], ws: [] };
async function tryFetch(label, url, opts) {
  try {
    const r = await fetch(url, opts);
    let text = "";
    try { text = await r.text(); } catch {}
    results.read.push({ label, url, type: r.type, status: r.status, text: text.slice(0, 200) });
  } catch (e) {
    results.read.push({ label, url, error: String(e) });
  }
}
async function run() {
  for (const g of GETS) {
    const img = new Image(); img.src = APP + g;
    const s = document.createElement("script"); s.src = APP + g; document.body.appendChild(s);
    await tryFetch("get-cors-cred", APP + g, { credentials: "include" });
    await tryFetch("get-nocors-cred", APP + g, { mode: "no-cors", credentials: "include" });
  }
  await tryFetch("get-cors-nocred", APP + "/api/workspaces", { credentials: "omit" });
  for (const p of POSTS) {
    await tryFetch("post-nocors", APP + p, { method: "POST", mode: "no-cors", credentials: "include", headers: { "content-type": "text/plain" }, body: BODY });
    await tryFetch("post-nocors-form", APP + p, { method: "POST", mode: "no-cors", credentials: "include", headers: { "content-type": "application/x-www-form-urlencoded" }, body: "a=1" });
    await tryFetch("post-cors-json", APP + p, { method: "POST", credentials: "include", headers: { "content-type": "application/json" }, body: BODY });
    await tryFetch("put-cors", APP + p, { method: "PUT", credentials: "include", body: BODY });
  }
  for (let i = 0; i < POSTS.length; i++) {
    document.getElementById("f" + i).submit();
    await new Promise((r) => setTimeout(r, 60));
  }
  await new Promise((resolve) => {
    const s = new WebSocket(APP.replace("http", "ws") + "/ws");
    const t = setTimeout(() => { results.ws.push("timeout"); resolve(); }, 4000);
    s.onopen = () => { results.ws.push("open"); s.send(JSON.stringify({ type: "subscribe", topic: "home" })); };
    s.onmessage = (m) => results.ws.push("message:" + String(m.data).slice(0, 100));
    s.onerror = () => results.ws.push("error");
    s.onclose = (e) => { results.ws.push("close:" + e.code); clearTimeout(t); resolve(); };
  });
  window.__attackDone = results;
}
run();
</script>`;
}

type Seen = { url: string; status: number; type: string };

/** Record every response the attacker page got from the app's origin, whatever the page could read. */
function watch(page: Page, app: string) {
  const seen: Seen[] = [];
  page.on("response", (r) => {
    if (r.url().startsWith(app)) seen.push({ url: r.url(), status: r.status(), type: r.request().resourceType() });
  });
  page.on("requestfailed", (r) => {
    if (r.url().startsWith(app)) seen.push({ url: r.url(), status: 0, type: r.resourceType() });
  });
  return seen;
}

const ALLOWED_PUBLIC = (url: string, app: string) => {
  const p = new URL(url).pathname;
  // The app shell and its assets are public, and the OAuth callback is outside /api by design
  // (a forged state is refused with 400 and changes nothing).
  return !p.startsWith("/api/") && !p.startsWith("/internal/") && p !== "/ws" && url.startsWith(app);
};

for (const kind of ["another localhost port (same-site)", "another site"] as const) {
  test(`attacker on ${kind}: every request is refused and changes nothing`, async ({ page, state, guard }) => {
    test.setTimeout(240_000);
    const api = new Api(state.app, cookieOf(state.storageState));
    // A second sign-in, so a forged revoke-others would show.
    await signIn(state.app);
    const before = await snapshot(api, state);

    const host = kind === "another site" ? EVIL_HOST : "127.0.0.1";
    const origin = `http://${host}:${state.attackerPort}`;
    guard.origins.push(origin);
    const routes = apiRoutes();
    const gets = routes.filter((r) => r.methods.includes("GET")).map((r) => fill(r.path, state) + query(r.path, state));
    const posts = postTargets(state.ws, state.session, state.executions[0].id);
    const url = origin + attackerPage(`attack-${host}`, attackHtml(state.app, posts, gets));
    const seen = watch(page, state.app);
    await page.goto(url);
    const results = await page.waitForFunction(() => (window as any).__attackDone, null, { timeout: 120_000 });
    const r = (await results.jsonValue()) as { read: any[]; ws: string[] };
    await page.waitForTimeout(1000);

    // Nothing was readable.
    const readable = r.read.filter((x) => x.type === "basic" || x.type === "cors" || (x.text && x.text.length));
    expect(readable, "a cross-origin fetch read a response").toEqual([]);
    expect(r.ws.filter((x) => x === "open" || x.startsWith("message")), "the WebSocket opened").toEqual([]);

    // The server refused every request that reached it; the browser blocked the rest (status 0).
    const accepted = seen.filter((x) => x.status !== 0 && x.status !== 403 && !ALLOWED_PUBLIC(x.url, state.app));
    expect(accepted, "the server accepted a cross-origin request").toEqual([]);
    const refused = seen.filter((x) => x.status === 403).length;
    expect(refused, "the requests reached the server").toBeGreaterThan(gets.length);

    // The app did not render in a frame.
    for (const f of page.frames().filter((f) => f !== page.mainFrame())) {
      const text = await f.evaluate(() => document.body?.innerText ?? "").catch(() => "");
      expect(text, `frame ${f.url()} rendered`).not.toMatch(/PW-|Ostra|Sessions/);
    }

    // Top-level navigations to every GET route: dest=document is allowed only from the app itself.
    const nav = origin + attackerPage(`nav-${host}`, "<!doctype html><title>nav</title><p>nav</p>");
    for (const g of gets) {
      await page.goto(nav);
      const target = state.app + g;
      const [resp] = await Promise.all([
        page.waitForResponse((x) => x.url() === new URL(target).href, { timeout: 10_000 }).catch(() => null),
        page.evaluate((to) => {
          location.href = to;
        }, target),
      ]);
      await page.waitForURL((u) => u.href === new URL(target).href, { timeout: 10_000 });
      const p = new URL(target).pathname;
      if (p.startsWith("/api/") || p === "/ws") expect(resp?.status(), `navigation to ${g}`).toBe(403);
      else expect(resp?.status(), `navigation to ${g}`).toBe(400);
    }
    // The OAuth callback ran with a forged state, was refused, and changed nothing (checked below).

    expect(await snapshot(api, state), "server state changed").toEqual(before);
  });
}

test("a rebinding hostname on the app's port gets 421 on every path", async ({ page, state, guard }) => {
  const rebound = `http://${EVIL_HOST}:${state.appPort}`;
  guard.origins.push(rebound);
  for (const p of ["/", "/api/workspaces", "/ws", `/w/${state.ws}`, "/assets/x.js", "/internal/policy"]) {
    const r = await page.goto(rebound + p);
    expect(r?.status(), p).toBe(421);
  }
});

test("same-origin images and scripts cannot reach the API", async ({ page, state }) => {
  // What an HTML injection on the app origin could do: the cookie is sent, but the guard refuses
  // page content (Sec-Fetch-Dest image, script, style) reaching /api. The Rust test
  // `the_browser_boundary_refuses_page_content_and_other_sites` pins the 403 for each header.
  await page.goto(`${state.app}/w/${state.ws}`);
  // The network layer's view (CDP), because the browser drops a nosniff refusal before Playwright
  // reports a response for it.
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Network.enable");
  const sent = new Map<string, { url: string; dest: string; cookie: boolean }>();
  const status = new Map<string, number>();
  cdp.on("Network.requestWillBeSent", (e) => {
    const u = new URL(e.request.url);
    if (u.pathname.startsWith("/api/") && !["Fetch", "XHR"].includes(e.type ?? ""))
      sent.set(e.requestId, { url: u.pathname, dest: "", cookie: false });
  });
  cdp.on("Network.requestWillBeSentExtraInfo", (e) => {
    const x = sent.get(e.requestId);
    if (!x) return;
    x.dest = e.headers["Sec-Fetch-Dest"] ?? e.headers["sec-fetch-dest"] ?? "";
    x.cookie = !!(e.headers.Cookie ?? e.headers.cookie);
  });
  cdp.on("Network.responseReceivedExtraInfo", (e) => status.set(e.requestId, e.statusCode));
  await page.evaluate(() => {
    const i = new Image();
    i.src = "/api/workspaces";
    const s = document.createElement("script");
    s.src = "/api/auth/sessions";
    document.body.appendChild(s);
    const l = document.createElement("link");
    l.rel = "stylesheet";
    l.href = "/api/onboarding";
    document.head.appendChild(l);
  });
  await page.waitForTimeout(1500);
  const seen = [...sent].map(([id, x]) => `${x.dest} ${x.url} cookie=${x.cookie} ${status.get(id)}`).sort();
  expect(seen).toEqual([
    "image /api/workspaces cookie=true 403",
    "script /api/auth/sessions cookie=true 403",
    "style /api/onboarding cookie=true 403",
  ]);
});
