import { Markdown } from "@ostra/design/docs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { MOCK_BOOK } from "../../api/mock/fixtures.books";
import {
  bookDocs,
  bookPages,
  cell,
  code,
  inlineText,
  pageBody,
  prose,
  sectionMarkdown,
  sectionPage,
} from "./bookModel";

describe("bookPages", () => {
  it("orders the pages overview, glossary, architecture, then each project's sections", () => {
    const { nav } = bookPages(MOCK_BOOK);
    expect(nav.map((g) => g.label)).toEqual(["Book", "api: How it works", "web: How it works"]);
    expect(nav.flatMap((g) => g.pages.map((p) => p.id))).toEqual([
      "overview",
      "glossary",
      "architecture",
      "api.checkout",
      "web.cart",
    ]);
  });

  it("leaves out the glossary and architecture pages when the book has none", () => {
    const { nav } = bookPages({ ...MOCK_BOOK, glossary: [], architecture: null });
    expect(nav[0].pages.map((p) => p.id)).toEqual(["overview"]);
  });

  it("renders a section as its summary, its own Markdown, and code references last", () => {
    const md = sectionMarkdown(MOCK_BOOK.parts[0].sections[0], "api");
    const order = [
      "# Checkout",
      "Turns a cart into a paid order.",
      "### Assumptions",
      "```mermaid",
      "### Order states",
      "## Retrying a charge",
      "## Code references",
    ];
    const at = order.map((s) => md.indexOf(s));
    expect(at.every((i) => i >= 0)).toBe(true);
    expect([...at].sort((a, b) => a - b)).toEqual(at);
    expect(md.trimEnd().endsWith("| The checkout entry point |")).toBe(true);
  });

  it("puts the level-2 headings of a section's body in its table of contents", () => {
    const page = bookDocs(MOCK_BOOK).page(sectionPage("api", "checkout"))!;
    expect(page.title).toBe("Checkout");
    expect(page.toc.map((t) => t.text)).toEqual(["Retrying a charge", "Code references"]);
  });

  it("drops raw HTML in a section body", () => {
    const section = {
      ...MOCK_BOOK.parts[0].sections[0],
      body: "<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>",
    };
    const book = { ...MOCK_BOOK, parts: [{ ...MOCK_BOOK.parts[0], sections: [section] }] };
    const docs = bookDocs(book);
    const page = docs.page(sectionPage("api", "checkout"))!;
    const html = renderToStaticMarkup(createElement(Markdown, { docs, page, go: () => {}, theme: "dark" }));
    expect(html).toContain("Turns a cart into a paid order.");
    expect(html).not.toMatch(/alert|<script|<img/);
  });

  it("finds book text in search", () => {
    const hits = bookDocs(MOCK_BOOK).search("idempotency");
    expect(hits.map((h) => h.page.id)).toContain("api.checkout");
  });
});

describe("escaping", () => {
  it("keeps a table cell on one line and in one cell", () => {
    expect(cell("a | b\nc")).toBe("a \\| b c");
  });

  it("renders a title as its characters", () => {
    expect(inlineText("# [x](javascript:y) <b>")).toBe("\\# \\[x\\](javascript:y) \\<b\\>");
  });

  it("stops prose from starting a heading or a fence", () => {
    expect(prose("Text\n# Not a heading\n```\nx")).toBe("Text\n\\# Not a heading\n\\```\nx");
  });

  it("keeps a level-1 heading in a page body from adding a second title", () => {
    expect(pageBody("# Two\n## Kept\n```\n# in code\n```")).toBe("\\# Two\n## Kept\n```\n# in code\n```");
  });

  it("holds backticks in a code span", () => {
    expect(code("a`b")).toBe("``a`b``");
  });

  it("keeps a script in a book field as text", () => {
    const book = {
      ...MOCK_BOOK,
      title: "<script>alert(1)</script>",
      glossary: [{ term: "<img src=x onerror=alert(1)>", definition: "[x](javascript:alert(1))", code_ref: null }],
    };
    const docs = bookDocs(book);
    const types = (id: string) => JSON.stringify(docs.page(id)!.blocks);
    expect(types("overview")).not.toContain('"type":"html"');
    expect(types("glossary")).not.toContain('"type":"html"');
    expect(types("glossary")).not.toContain('"type":"link"');
  });
});
