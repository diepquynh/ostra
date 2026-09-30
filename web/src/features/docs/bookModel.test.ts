import { describe, expect, it } from "vitest";
import { MOCK_BOOK } from "../../api/mock/fixtures.books";
import { bookDocs, bookPages, cell, code, inlineText, prose, sectionMarkdown, sectionPage } from "./bookModel";

describe("bookPages", () => {
  it("orders the pages overview, glossary, architecture, then each project's sections", () => {
    const { nav } = bookPages(MOCK_BOOK);
    expect(nav.map((g) => g.label)).toEqual(["Book", "api", "web"]);
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

  it("renders a section in reading order with code references last", () => {
    const md = sectionMarkdown(MOCK_BOOK.parts[0].sections[0], "api");
    const order = [
      "Turns a cart into a paid order.",
      "## Boundaries",
      "## Assumptions",
      "## Business flow",
      "### Charge an order",
      "```mermaid",
      "### Order states",
      "## Separation of concerns",
      "## Retrying a charge",
      "### Code references",
      "## Code references",
    ];
    const at = order.map((s) => md.indexOf(s));
    expect(at.every((i) => i >= 0)).toBe(true);
    expect([...at].sort((a, b) => a - b)).toEqual(at);
    expect(md.trimEnd().endsWith("| The checkout entry point |")).toBe(true);
  });

  it("puts every section's sub-sections and parts in its table of contents", () => {
    const page = bookDocs(MOCK_BOOK).page(sectionPage("api", "checkout"))!;
    expect(page.title).toBe("Checkout");
    expect(page.toc.map((t) => t.text)).toEqual([
      "Boundaries",
      "Assumptions",
      "Business flow",
      "Separation of concerns",
      "Retrying a charge",
      "Code references",
    ]);
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
