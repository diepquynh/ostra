import type { Heading } from "../../api/gen/Heading";

// A port of `crates/ostra-core/src/outline.rs`: GitHub-style anchor slugs, so the ids the rendered
// markdown carries are the ids in `Artifact.headings` and in `#<id>` deep links.

/** Lowercase, keep letters, digits, `-` and `_`, turn each space into `-`, drop everything else. */
export function slugify(title: string): string {
  let out = "";
  for (const c of title.toLowerCase()) {
    if (c === " ") out += "-";
    else if (c === "-" || c === "_" || /[\p{Alphabetic}\p{N}]/u.test(c)) out += c;
  }
  return out;
}

/** Numbers repeated slugs `-1`, `-2`, exactly as the server's `Slugger` does. */
export class Slugger {
  private seen = new Map<string, number>();

  /** Marks an id as taken without producing one, so client-made ids never collide with server ids. */
  reserve(id: string): void {
    if (!this.seen.has(id)) this.seen.set(id, 0);
  }

  slug(title: string): string {
    const base = slugify(title);
    let slug = base;
    while (this.seen.has(slug)) {
      const n = (this.seen.get(base) ?? 0) + 1;
      this.seen.set(base, n);
      slug = `${base}-${n}`;
    }
    this.seen.set(slug, 0);
    return slug;
  }
}

type HastNode = {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
};

const textOf = (n: HastNode): string => (n.type === "text" ? (n.value ?? "") : (n.children ?? []).map(textOf).join(""));

function walk(n: HastNode, fn: (el: HastNode) => void) {
  if (n.type === "element") fn(n);
  for (const c of n.children ?? []) walk(c, fn);
}

/** The text a reader sees in a heading, whitespace collapsed like the server's outline. */
export const headingText = (n: HastNode): string => textOf(n).replace(/\s+/g, " ").trim();

/**
 * A rehype plugin that gives every heading the id the server listed for it. Headings are matched in document
 * order by level and text; a heading the server did not list (inside a blockquote, say) gets a fresh slug that
 * cannot collide with a listed one.
 */
export function rehypeHeadingIds(server: Heading[]) {
  return () => (tree: HastNode) => {
    const slugger = new Slugger();
    for (const h of server) slugger.reserve(h.id);
    let next = 0;
    walk(tree, (el) => {
      const m = /^h([1-6])$/.exec(el.tagName ?? "");
      if (!m) return;
      const level = Number(m[1]);
      const title = headingText(el);
      if (!title) return;
      let id: string | null = null;
      for (let j = next; j < server.length; j++) {
        if (server[j].level === level && server[j].title === title) {
          id = server[j].id;
          next = j + 1;
          break;
        }
      }
      el.properties = { ...el.properties, id: id ?? slugger.slug(title) };
    });
  };
}
