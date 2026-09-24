import { describe, expect, it } from "vitest";
import { muteQueryReplies, safeLinkTarget, type ParserHooks } from "./terminalSafety";

function fakeParser() {
  const csi = new Map<string, (p: (number | number[])[]) => boolean>();
  const osc = new Map<number, (d: string) => boolean>();
  const dcs = new Set<string>();
  const key = (id: { prefix?: string; intermediates?: string; final: string }) => `${id.prefix ?? ""}${id.intermediates ?? ""}${id.final}`;
  let live = 0;
  const sub = () => {
    live += 1;
    return { dispose: () => (live -= 1) };
  };
  const parser: ParserHooks = {
    registerCsiHandler: (id, cb) => (csi.set(key(id), cb), sub()),
    registerOscHandler: (id, cb) => (osc.set(id, cb), sub()),
    registerDcsHandler: (id) => (dcs.add(key(id)), sub()),
  };
  return { parser, csi, osc, dcs, live: () => live };
}

describe("muteQueryReplies", () => {
  it("swallows queries and lets ordinary sequences through", () => {
    const f = fakeParser();
    const d = muteQueryReplies(f.parser);
    for (const k of ["n", "?n", "c", ">c", "$p", "?$p", ">q", "?u"]) expect(f.csi.get(k)?.([])).toBe(true);
    expect(f.csi.get("t")?.([18])).toBe(true);
    expect(f.csi.get("t")?.([22, 0])).toBe(false);
    expect(f.osc.get(11)?.("?")).toBe(true);
    expect(f.osc.get(4)?.("1;?")).toBe(true);
    expect(f.osc.get(11)?.("rgb:0000/0000/0000")).toBe(false);
    expect(f.dcs.has("$q")).toBe(true);
    d.dispose();
    expect(f.live()).toBe(0);
  });
});

describe("safeLinkTarget", () => {
  it("allows web pages only", () => {
    expect(safeLinkTarget("https://example.com/a")).toBe("https://example.com/a");
    expect(safeLinkTarget("http://localhost:3000")).toBe("http://localhost:3000/");
    expect(safeLinkTarget("javascript:alert(1)")).toBeNull();
    expect(safeLinkTarget("file:///etc/passwd")).toBeNull();
    expect(safeLinkTarget("not a url")).toBeNull();
  });
});
