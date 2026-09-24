import { describe, expect, it } from "vitest";
import { parseResource, resourceFromPath, resourceId, resourcePath } from "./resource";

const split = (url: string) => {
  const u = new URL(url, "http://x");
  return [u.pathname, u.search] as const;
};

describe("resource ids and routes", () => {
  const cases: [string, string][] = [
    ["ws:overview", "/w/shop"],
    ["ws:cost", "/w/shop/cost"],
    ["ws:settings", "/w/shop/settings"],
    ["ws:memory", "/w/shop/memory"],
    ["ws:skills", "/w/shop/skills"],
    ["session:s_1", "/w/shop/s/s_1"],
    ["exec:x9", "/w/shop/x/x9"],
    ["artifact:/home/me/shop/.ostra/sessions/s 1/ostra-spec.md", "/w/shop/artifact?path=%2Fhome%2Fme%2Fshop%2F.ostra%2Fsessions%2Fs%201%2Fostra-spec.md"],
    ["project:backend", "/w/shop/p/backend"],
    ["file:backend:crates/orders/src/service.rs", "/w/shop/f/backend/crates/orders/src/service.rs"],
    ["file:web:src/a b/c#d.ts", "/w/shop/f/web/src/a%20b/c%23d.ts"],
  ];

  for (const [id, url] of cases) {
    it(`${id} <-> ${url}`, () => {
      expect(resourcePath("shop", id)).toBe(url);
      expect(resourceFromPath(...split(url))).toEqual({ ws: "shop", id });
    });
  }

  it("round-trips every id through the URL", () => {
    for (const [id] of cases) expect(resourceFromPath(...split(resourcePath("w 1", id)))).toEqual({ ws: "w 1", id });
  });

  it("puts the anchor in the hash", () => {
    expect(resourcePath("shop", "session:s1", "gate-g_perm")).toBe("/w/shop/s/s1#gate-g_perm");
  });

  it("ignores a trailing slash and query parameters of other screens", () => {
    expect(resourceFromPath("/w/shop/", "")).toEqual({ ws: "shop", id: "ws:overview" });
    expect(resourceFromPath("/w/shop/x/x1", "?tab=terminal")).toEqual({ ws: "shop", id: "exec:x1" });
    expect(resourceFromPath("/w/shop/memory", "?project=web")).toEqual({ ws: "shop", id: "ws:memory" });
  });

  it("rejects URLs that name no resource", () => {
    expect(resourceFromPath("/", "")).toBeNull();
    expect(resourceFromPath("/s/s1", "")).toBeNull();
    expect(resourceFromPath("/w/shop/artifact", "")).toBeNull();
    expect(resourceFromPath("/w/shop/overview", "")).toBeNull();
    expect(resourceFromPath("/w/shop/f/backend", "")).toBeNull();
    expect(resourceFromPath("/w/shop/projects", "")).toBeNull();
  });

  it("parses ids and rejects malformed ones", () => {
    expect(parseResource("file:web:a:b.txt")).toEqual({ type: "file", key: "web", path: "a:b.txt" });
    expect(parseResource("ws:nope")).toBeNull();
    expect(parseResource("file:web")).toBeNull();
    expect(parseResource("file:web:")).toBeNull();
    expect(parseResource("session:")).toBeNull();
    expect(parseResource("cmd:theme")).toBeNull();
    expect(parseResource("plain")).toBeNull();
    const r = parseResource("artifact:/a/b.md")!;
    expect(resourceId(r)).toBe("artifact:/a/b.md");
  });
});
