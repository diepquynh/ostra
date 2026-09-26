import { describe, expect, it } from "vitest";
import { Docs, resolvePath, slug } from "./model";
import type { NavGroup } from "./pages";

const docs = new Docs();

describe("the docs pages", () => {
  it("finds content for every page in the sidebar", () => {
    const empty = docs.pages.filter((p) => p.blocks.length === 0).map((p) => `${p.id} (${p.file} ${p.section})`);
    expect(empty).toEqual([]);
  });

  it("gives every page a unique id", () => {
    const ids = docs.pages.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("drops the section heading and lists the next level as the table of contents", () => {
    const engine = docs.page("engine")!;
    expect(engine.titleLevel).toBe(2);
    expect(engine.blocks.some((b) => b.type === "heading" && b.depth === 2)).toBe(false);
    expect(engine.toc.length).toBeGreaterThan(0);
  });

  it("refuses a page whose file is not a source", () => {
    const nav: NavGroup[] = [{ label: "x", pages: [{ id: "x", title: "X", file: "missing.md" }] }];
    expect(() => new Docs(nav, {})).toThrow("missing.md");
  });
});

describe("sections", () => {
  const nav: NavGroup[] = [
    {
      label: "g",
      pages: [
        { id: "intro", title: "Intro", file: "a.md", section: "" },
        { id: "two", title: "Two", file: "a.md", section: "2" },
        { id: "named", title: "Named", file: "a.md", section: "Named part" },
      ],
    },
  ];
  const src =
    "# Top\n\nLead.\n\n## 2. Second\n\nBody [n](#named-part).\n\n### Sub\n\n### Sub\n\n## Named part\n\nEnd.\n";
  const d = new Docs(nav, { "a.md": src });

  it("splits a file at its ## headings by number, by name, and before the first", () => {
    expect(d.page("intro")!.blocks.map((b) => b.type)).toEqual(["paragraph"]);
    expect(d.page("two")!.toc.map((t) => t.id)).toEqual(["sub", "sub-2"]);
    expect(d.page("named")!.blocks).toHaveLength(1);
  });

  it("sends an anchor naming another section to that section's page", () => {
    expect(d.link("#named-part", d.page("two")!)?.nav).toEqual({ page: "named", anchor: null });
  });
});

describe("links", () => {
  const readme = docs.page("overview")!;

  it("keeps web links as they are", () => {
    expect(docs.link("https://example.com/x", readme)).toEqual({ href: "https://example.com/x", external: true });
  });

  it("does not follow other schemes", () => {
    expect(docs.link("javascript:alert(1)", readme)).toBeNull();
  });

  it("opens a docs file as its page", () => {
    expect(docs.link("docs/providers/README.md", readme)?.nav).toEqual({ page: "providers", anchor: null });
    expect(docs.link("./openai.md", docs.page("providers")!)?.href).toBe("#openai");
  });

  it("sends an anchor into a split file to the page that holds it", () => {
    expect(docs.link("../../README.md#build-and-run", docs.page("providers")!)?.nav?.page).toBe("build-and-run");
  });

  it("links other repository files to GitHub", () => {
    const l = docs.link("../../LICENSE", docs.page("providers")!);
    expect(l).toEqual({ href: "https://github.com/diepquynh/ostra/blob/master/LICENSE", external: true });
  });

  it("reads a page hash, a bare anchor, and anything else as the first page", () => {
    expect(docs.fromHash("#engine/x")).toEqual({ page: "engine", anchor: "x" });
    expect(docs.fromHash("#8-the-engine")).toEqual({ page: "engine", anchor: null });
    expect(docs.fromHash("#%E0%A4%A")).toEqual({ page: "overview", anchor: null });
  });
});

describe("search", () => {
  it("finds a word once per section, titles first", () => {
    const hits = docs.search("sandbox");
    expect(hits.length).toBeGreaterThan(0);
    const keys = hits.map((h) => `${h.page.id}|${h.heading?.id ?? ""}`);
    expect(new Set(keys).size).toBe(keys.length);
    expect(docs.search("the engine")[0].page.id).toBe("engine");
  });

  it("splits the matching text around the match", () => {
    const [h] = docs.search("GATEWAYS");
    expect(h.match.toLowerCase()).toBe("gateways");
  });
});

describe("helpers", () => {
  it("slugs headings the way GitHub does for plain text", () => {
    expect(slug("8. The `engine`")).toBe("8-the-engine");
  });

  it("resolves paths from the linking file's folder", () => {
    expect(resolvePath("docs/providers/README.md", "../../README.md")).toBe("README.md");
    expect(resolvePath("README.md", "./docs/x.md")).toBe("docs/x.md");
  });
});
