import type { Definition, Heading, Root, RootContent } from "mdast";
import { fromMarkdown } from "mdast-util-from-markdown";
import { gfmFromMarkdown } from "mdast-util-gfm";
import { toString as mdText } from "mdast-util-to-string";
import { gfm } from "micromark-extension-gfm";

export type PageDef = {
  /** The page's address in the hash: `#<id>`. */
  id: string;
  title: string;
  /** The Markdown source's key in `sources`. */
  file: string;
  /**
   * One `##` section of the file: its number ("8" matches "## 8. The engine") or its exact heading text. "" is the
   * text before the first `##`. Omit it to show the whole file.
   */
  section?: string;
};

export type NavGroup = { label: string; pages: PageDef[] };

/** Where a link to a source file outside the docs goes; null renders the link as plain text. */
export type FileUrl = (file: string) => string | null;

export type TocEntry = { id: string; text: string };
export type Target = { page: string; anchor: string | null };
export type Page = PageDef & {
  group: string;
  /** The section's content without its own heading, which becomes the page title. */
  blocks: RootContent[];
  /** Depth of the dropped title heading; headings one level below it are the page's table of contents. */
  titleLevel: number;
  toc: TocEntry[];
  ids: Map<Heading, string>;
  definitions: Map<string, Definition>;
};
export type Link = { href: string; external: boolean; nav?: Target };
type IndexEntry = { page: Page; heading: TocEntry | null; text: string; rank: number };
export type Hit = { page: Page; heading: TocEntry | null; pre: string; match: string; post: string };

export const MAX_HITS = 80;

export function slug(s: string): string {
  return s
    .toLowerCase()
    .replace(/`/g, "")
    .replace(/[^a-z0-9\s-]/g, "")
    .trim()
    .replace(/\s+/g, "-");
}

export function parseMarkdown(src: string): Root {
  return fromMarkdown(src, { extensions: [gfm()], mdastExtensions: [gfmFromMarkdown()] });
}

/** A `##` section by number or heading text, "" for the text before the first `##`, or the whole file. */
export function pickSection(root: Root, section: string | undefined): RootContent[] {
  if (section === undefined) return root.children;
  const starts = root.children.flatMap((n, i) => (n.type === "heading" && n.depth === 2 ? [i] : []));
  if (section === "") return root.children.slice(0, starts[0] ?? root.children.length);
  for (const [k, at] of starts.entries()) {
    const text = mdText(root.children[at]);
    if (text === section || text.startsWith(`${section}. `)) return root.children.slice(at, starts[k + 1]);
  }
  return [];
}

/** Searchable text of a block. Table cells are kept apart, which `mdast-util-to-string` would run together. */
export function blockText(node: RootContent): string {
  if (node.type === "table")
    return node.children.map((row) => row.children.map((c) => mdText(c)).join(" · ")).join("\n");
  if (node.type === "definition" || node.type === "html") return "";
  return mdText(node);
}

/** Resolve `rel` against the folder of the repository file `from`. */
export function resolvePath(from: string, rel: string): string {
  const out: string[] = [];
  for (const s of [...from.split("/").slice(0, -1), ...rel.split("/")]) {
    if (s === "..") out.pop();
    else if (s && s !== ".") out.push(s);
  }
  return out.join("/");
}

function collectDefinitions(root: Root): Map<string, Definition> {
  const out = new Map<string, Definition>();
  const walk = (nodes: RootContent[]) => {
    for (const n of nodes) {
      if (n.type === "definition" && !out.has(n.identifier)) out.set(n.identifier, n);
      if ("children" in n) walk(n.children as RootContent[]);
    }
  };
  walk(root.children);
  return out;
}

export class Docs {
  readonly pages: Page[];
  private readonly byId = new Map<string, Page>();
  private readonly byFile = new Map<string, Page[]>();
  private readonly anchors = new Map<string, Target>();
  private readonly index: IndexEntry[] = [];

  readonly nav: NavGroup[];
  private readonly fileUrl: FileUrl;

  constructor(nav: NavGroup[], sources: Record<string, string>, fileUrl: FileUrl = () => null) {
    this.nav = nav;
    this.fileUrl = fileUrl;
    const roots = new Map<string, Root>();
    const root = (file: string) => {
      let r = roots.get(file);
      if (!r) {
        const src = sources[file];
        if (src === undefined) throw new Error(`The docs page file ${file} is not in the sources.`);
        r = parseMarkdown(src);
        roots.set(file, r);
      }
      return r;
    };
    this.pages = nav.flatMap((g) => g.pages.map((def) => this.build(def, g.label, root(def.file))));
    for (const p of this.pages) {
      this.byId.set(p.id, p);
      this.byFile.set(p.file, [...(this.byFile.get(p.file) ?? []), p]);
    }
    this.indexAnchors(roots);
    this.indexSearch();
  }

  private build(def: PageDef, group: string, root: Root): Page {
    let blocks = pickSection(root, def.section);
    let titleLevel = 2;
    const first = blocks[0];
    if (first?.type === "heading") {
      titleLevel = first.depth;
      blocks = blocks.slice(1);
    }
    const ids = new Map<Heading, string>();
    const used = new Map<string, number>();
    const toc: TocEntry[] = [];
    for (const b of blocks) {
      if (b.type !== "heading") continue;
      const text = mdText(b);
      let id = slug(text) || "section";
      const n = used.get(id) ?? 0;
      used.set(id, n + 1);
      if (n) id = `${id}-${n + 1}`;
      ids.set(b, id);
      if (b.depth === titleLevel + 1) toc.push({ id, text });
    }
    return { ...def, group, blocks, titleLevel, toc, ids, definitions: collectDefinitions(root) };
  }

  /** Global anchors: a page's title and its section heading, then every heading id, first one wins. */
  private indexAnchors(roots: Map<string, Root>) {
    const add = (id: string, t: Target) => {
      if (id && !this.anchors.has(id)) this.anchors.set(id, t);
    };
    for (const p of this.pages) {
      add(slug(p.title), { page: p.id, anchor: null });
      const head = p.section ? pickSection(roots.get(p.file)!, p.section)[0] : undefined;
      if (head?.type === "heading") add(slug(mdText(head)), { page: p.id, anchor: null });
    }
    for (const p of this.pages) for (const id of p.ids.values()) add(id, { page: p.id, anchor: id });
  }

  private indexSearch() {
    for (const page of this.pages) {
      this.index.push({ page, heading: null, text: page.title, rank: 0 });
      let heading: TocEntry | null = null;
      for (const b of page.blocks) {
        if (b.type === "heading") {
          heading = { id: page.ids.get(b)!, text: mdText(b) };
          this.index.push({ page, heading, text: heading.text, rank: 1 });
          continue;
        }
        const text = blockText(b).replace(/\s+/g, " ").trim();
        if (text) this.index.push({ page, heading, text, rank: 2 });
      }
    }
  }

  page(id: string): Page | undefined {
    return this.byId.get(id);
  }

  /** A `#page/anchor` hash, or a bare anchor from an older link. Unknown values open the first page. */
  fromHash(hash: string): Target {
    let raw = hash.replace(/^#/, "");
    try {
      raw = decodeURIComponent(raw);
    } catch {
      // A malformed escape: use the text as it is.
    }
    const [id, anchor] = raw.split("/");
    if (this.byId.has(id)) return { page: id, anchor: anchor || null };
    return this.anchors.get(id) ?? { page: this.pages[0].id, anchor: null };
  }

  /**
   * Where a Markdown link on `page` goes: another docs page when its file is in the docs, else `fileUrl` of the file.
   * Null for a scheme the docs do not follow (`javascript:`, `data:`), which then renders as plain text.
   */
  link(href: string, page: Page): Link | null {
    if (/^(https?:|mailto:)/i.test(href)) return { href, external: true };
    if (/^[a-z][a-z0-9+.-]*:/i.test(href)) return null;
    const hashAt = href.indexOf("#");
    const path = hashAt < 0 ? href : href.slice(0, hashAt);
    const anchor = hashAt < 0 ? null : href.slice(hashAt + 1) || null;
    if (!path) {
      const own = anchor && [...page.ids.values()].includes(anchor) ? { page: page.id, anchor } : null;
      return internal(own ?? (anchor ? this.anchors.get(anchor) : undefined) ?? { page: page.id, anchor: null });
    }
    const file = resolvePath(page.file, path);
    const pages = this.byFile.get(file);
    if (!pages) {
      const url = this.fileUrl(file);
      return url ? { href: url + (anchor ? `#${anchor}` : ""), external: true } : null;
    }
    const hit = anchor ? this.anchors.get(anchor) : undefined;
    return internal(hit && this.byId.get(hit.page)?.file === file ? hit : { page: pages[0].id, anchor });
  }

  /** Case-insensitive matches, one per page section, titles before headings before text. */
  search(query: string): Hit[] {
    const q = query.toLowerCase();
    const seen = new Set<string>();
    const hits: { e: IndexEntry; at: number }[] = [];
    for (const e of this.index) {
      const at = e.text.toLowerCase().indexOf(q);
      if (at < 0) continue;
      const key = `${e.page.id}|${e.heading?.id ?? ""}`;
      if (seen.has(key)) continue;
      seen.add(key);
      hits.push({ e, at });
    }
    hits.sort((x, y) => x.e.rank - y.e.rank);
    return hits.slice(0, MAX_HITS).map(({ e, at }) => {
      const start = Math.max(0, at - 48);
      const end = Math.min(e.text.length, at + q.length + 90);
      return {
        page: e.page,
        heading: e.heading,
        pre: (start > 0 ? "…" : "") + e.text.slice(start, at),
        match: e.text.slice(at, at + q.length),
        post: e.text.slice(at + q.length, end) + (end < e.text.length ? "…" : ""),
      };
    });
  }
}

function internal(t: Target): Link {
  return { href: `#${t.page}${t.anchor ? `/${t.anchor}` : ""}`, external: false, nav: t };
}
