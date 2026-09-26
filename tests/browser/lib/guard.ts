/**
 * The fixture every spec uses. A test fails on any sign that a test string ran or loaded:
 * `window.__pwned` set, a canary request, a CSP violation on the app origin, an unexpected
 * dialog, popup, or navigation away from the app.
 */
import { test as base, type BrowserContext, expect, type Page } from "@playwright/test";
import { canaryHits } from "./fake-model";
import { readState, type SuiteState } from "./env";

export interface Finding {
  kind: "pwned" | "csp" | "dialog" | "navigation" | "popup" | "canary";
  detail: string;
  page?: string;
}

const MARK = "__PW_FINDING__";

/** Runs in every frame before any page script. Reports through the console, which CSP cannot block. */
const INIT = `(() => {
  const report = (kind, detail) => console.log(${JSON.stringify(MARK)} + JSON.stringify({ kind, detail, page: location.href }));
  let v;
  try {
    Object.defineProperty(window, "__pwned", { configurable: false, get() { return v; }, set(x) { v = x; report("pwned", String(x)); } });
  } catch {}
  document.addEventListener("securitypolicyviolation", (e) => report("csp", JSON.stringify({
    directive: e.effectiveDirective, blocked: e.blockedURI, source: e.sourceFile, line: e.lineNumber, sample: e.sample,
  })), true);
})();`;

export class Guard {
  readonly findings: Finding[] = [];
  /** Dialogs the test expects, such as the terminal's own link confirm. Each is dismissed. */
  readonly expectedDialogs: RegExp[] = [];
  readonly dialogs: string[] = [];
  /** Origins a main frame may show. The app's, plus any a test adds (attacker pages). */
  readonly origins: string[];
  /** CSP violations count only on these origins; an attacker page's own violations are expected. */
  readonly cspOrigins: string[];
  allowPopups = false;
  readonly popups: Page[] = [];
  private canaryStart = canaryHits().length;

  constructor(readonly state: SuiteState) {
    this.origins = [state.app, "about:blank"];
    this.cspOrigins = [state.app];
  }

  add(f: Finding) {
    this.findings.push(f);
    console.log(`[pw finding] ${f.kind}: ${f.detail}${f.page ? ` (on ${f.page})` : ""}`);
  }

  attach(context: BrowserContext) {
    context.on("console", (msg) => {
      const t = msg.text();
      if (t.startsWith(MARK)) {
        const f = JSON.parse(t.slice(MARK.length)) as Finding;
        if (f.kind === "csp" && !this.cspOrigins.some((o) => f.page?.startsWith(o))) return;
        this.add(f);
      }
    });
    const watch = (page: Page) => {
      page.on("dialog", async (d) => {
        this.dialogs.push(d.message());
        if (!this.expectedDialogs.some((r) => r.test(d.message())))
          this.add({ kind: "dialog", detail: `${d.type()}: ${d.message()}`, page: page.url() });
        await d.dismiss().catch(() => {});
      });
      page.on("framenavigated", (frame) => {
        const url = frame.url();
        if (frame !== page.mainFrame()) {
          if (/^(javascript|data|vbscript):/i.test(url))
            this.add({ kind: "navigation", detail: `subframe to ${url}`, page: page.url() });
          return;
        }
        if (!this.origins.some((o) => url === o || url.startsWith(o === "about:blank" ? o : `${o}/`)))
          this.add({ kind: "navigation", detail: `main frame to ${url}` });
      });
    };
    context.pages().forEach(watch);
    context.on("page", (p) => {
      watch(p);
      if (context.pages().length > 1) {
        this.popups.push(p);
        if (!this.allowPopups) this.add({ kind: "popup", detail: `opened ${p.url() || "(pending)"}` });
      }
    });
    return context.addInitScript(INIT);
  }

  newCanaryHits() {
    return canaryHits().slice(this.canaryStart);
  }

  check() {
    for (const h of this.newCanaryHits())
      this.add({ kind: "canary", detail: `${h.id}${h.referer ? ` from ${h.referer}` : ""}` });
    expect(this.findings, `security findings:\n${this.findings.map((f) => `- ${f.kind}: ${f.detail} ${f.page ?? ""}`).join("\n")}`).toEqual([]);
  }
}

export const test = base.extend<{ guard: Guard; state: SuiteState }>({
  state: async ({}, use) => use(readState()),
  storageState: async ({}, use) => use(readState().storageState),
  guard: async ({ context, state }, use) => {
    const g = new Guard(state);
    await g.attach(context);
    await use(g);
    // Let late loads (images, lazy chunks) land before judging.
    await new Promise((r) => setTimeout(r, 500));
    g.check();
  },
  // The guard attaches before the first page exists, so its init script covers every load.
  page: async ({ page, guard: _guard }, use) => use(page),
});

export { expect };
