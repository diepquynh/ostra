import { Banner, Button, Dialog, Input, Table } from "@ostra/design";
import { type Page as DocPage, MAX_HITS, Markdown, type Target } from "@ostra/design/docs";
import "@ostra/design/docs.css";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { Book } from "../api/gen/Book";
import type { BookSummary } from "../api/gen/BookSummary";
import { bookDocs, sectionPage } from "../features/docs/bookModel";
import { buildExportHtml, diagramsDrawn, download } from "../features/docs/exportHtml";
import { useAsync } from "../lib/hooks";
import { useNav, useShell } from "../lib/nav";
import { LoadError, Loading, Page } from "./workspace/Page";
import "./docs.css";

const MIN_QUERY = 2;

/** Resource `ws:docs`: the workspace's documentation books, newest first. A book opens as `book:<id>`. */
export function DocsScreen({ ws }: { ws: string }) {
  const { open } = useNav();
  const books = useAsync(() => api.books(ws), [ws]);
  return (
    <Page
      title="Documentation"
      sub="Books the docs stage wrote for this workspace. Each covers a set of projects; ask for documentation in a new task to write or update one."
    >
      {books.error && <LoadError error={books.error} onRetry={books.reload} />}
      {books.loading && !books.data && <Loading>Reading the books…</Loading>}
      {books.data && books.data.length === 0 && (
        <div className="wp-muted">No books yet. Start a task that asks for documentation to write the first one.</div>
      )}
      {books.data && books.data.length > 0 && (
        <Table<BookSummary>
          rows={books.data}
          onRowClick={(b) => open(`book:${b.id}`)}
          columns={[
            { key: "title", label: "Book", render: (b) => b.title || b.id },
            { key: "projects", label: "Projects", render: (b) => b.projects.join(", ") },
            { key: "sections", label: "Sections", num: true, render: (b) => String(b.sections) },
            { key: "arch", label: "Architecture", render: (b) => (b.has_architecture ? "Yes" : "No") },
            { key: "updated", label: "Updated", render: (b) => new Date(b.updated_at).toLocaleString() },
          ]}
        />
      )}
    </Page>
  );
}

/** Resource `book:<id>`: one book as pages, with the sidebar, search, table of contents, and HTML export. */
export function BookScreen({ ws, id }: { ws: string; id: string }) {
  const book = useAsync(() => api.book(ws, id), [ws, id]);
  if (book.error)
    return (
      <div style={{ padding: 24 }}>
        <LoadError error={book.error} onRetry={book.reload} />
      </div>
    );
  if (!book.data)
    return (
      <div style={{ padding: 24 }}>
        <Loading>Reading the book…</Loading>
      </div>
    );
  return <BookReader ws={ws} book={book.data} />;
}

function BookReader({ ws, book }: { ws: string; book: Book }) {
  const { theme } = useShell();
  const { close, open } = useNav();
  const docs = useMemo(() => bookDocs(book), [book]);
  const [target, setTarget] = useState<Target>({ page: docs.pages[0].id, anchor: null });
  const [query, setQuery] = useState("");
  const [active, setActive] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const pane = useRef<HTMLElement>(null);
  const printRoot = useRef<HTMLDivElement>(null);

  const page = docs.page(target.page) ?? docs.pages[0];
  const at = docs.pages.indexOf(page);
  const prev = docs.pages[at - 1];
  const next = docs.pages[at + 1];

  const subsections = useMemo(() => {
    const out = new Map<string, Set<string>>();
    for (const part of book.parts)
      for (const s of part.sections)
        out.set(
          sectionPage(part.project, s.id),
          new Set(s.subsections.map((x) => x.title.replace(/\s+/g, " ").trim())),
        );
    return out;
  }, [book]);
  const subsOf = (p: DocPage) => p.toc.filter((t) => subsections.get(p.id)?.has(t.text));

  const spy = useCallback(() => {
    const el = pane.current;
    if (!el) return;
    let id: string | null = null;
    for (const t of page.toc) {
      const h = el.querySelector<HTMLElement>(`[id="${CSS.escape(t.id)}"]`);
      if (h && h.offsetTop - 96 <= el.scrollTop) id = t.id;
    }
    setActive(id);
  }, [page]);

  useEffect(() => {
    const el = pane.current;
    if (!el) return;
    const h = target.anchor ? el.querySelector<HTMLElement>(`[id="${CSS.escape(target.anchor)}"]`) : null;
    el.scrollTop = h ? Math.max(0, h.offsetTop - 16) : 0;
    spy();
  }, [target, spy]);

  const go = useCallback((t: Target) => setTarget({ ...t }), []);

  const q = query.trim();
  const searching = q.length >= MIN_QUERY;
  const hits = useMemo(() => (searching ? docs.search(q) : []), [docs, q, searching]);

  const exportBook = () => {
    setExportError(null);
    setExporting(true);
  };

  useEffect(() => {
    if (!exporting) return;
    let live = true;
    (async () => {
      const root = printRoot.current;
      if (!root) return;
      await diagramsDrawn(root);
      if (!live) return;
      const html = buildExportHtml({
        title: book.title || book.id,
        pages: docs.pages.map((p) => ({ id: p.id, title: p.title, group: p.group })),
        rendered: root,
        theme,
      });
      download(`${book.id}.html`, html);
    })()
      .catch((e: unknown) => live && setExportError(e instanceof Error ? e.message : String(e)))
      .finally(() => live && setExporting(false));
    return () => {
      live = false;
    };
  }, [exporting, book, docs, theme]);

  return (
    <div className="bk">
      <header className="bk-bar">
        <span className="bk-bar__title">{book.title || book.id}</span>
        <div className="bk-bar__search">
          <Input
            size="sm"
            icon="search"
            placeholder="Search this book"
            aria-label="Search this book"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <Button size="sm" icon="download" disabled={exporting} onClick={exportBook}>
          {exporting ? "Exporting…" : "Export HTML"}
        </Button>
        <Button size="sm" variant="danger" onClick={() => setConfirmDelete(true)}>
          Delete
        </Button>
      </header>
      {exportError && <Banner tone="bad">The book could not be exported: {exportError}</Banner>}

      <div className="docs-body">
        <aside className="docs-nav" aria-label="Book contents">
          {searching ? (
            <div style={{ display: "grid", gap: 4 }}>
              <div className="bk-eyebrow docs-nav__label">
                {hits.length ? `${hits.length}${hits.length === MAX_HITS ? "+" : ""} matches` : "No matches"}
              </div>
              {hits.map((h) => (
                <button
                  key={`${h.page.id}|${h.heading?.id ?? ""}`}
                  type="button"
                  className="docs-hit"
                  onClick={() => go({ page: h.page.id, anchor: h.heading?.id ?? null })}
                >
                  <span className="docs-hit__where">
                    {h.page.title}
                    {h.heading && ` › ${h.heading.text}`}
                  </span>
                  <span className="docs-hit__text">
                    {h.pre}
                    <mark>{h.match}</mark>
                    {h.post}
                  </span>
                </button>
              ))}
            </div>
          ) : (
            <div style={{ display: "grid", gap: 18 }}>
              {docs.nav.map((g) => (
                <div key={g.label} style={{ display: "grid", gap: 1 }}>
                  <div className="bk-eyebrow docs-nav__label">{g.label}</div>
                  {g.pages.map((def) => {
                    const p = docs.page(def.id)!;
                    return (
                      <div key={p.id} style={{ display: "grid", gap: 1 }}>
                        <button
                          type="button"
                          className="docs-nav__item"
                          aria-current={p.id === page.id ? "page" : undefined}
                          onClick={() => go({ page: p.id, anchor: null })}
                        >
                          {p.title}
                        </button>
                        {subsOf(p).map((t) => (
                          <button
                            key={t.id}
                            type="button"
                            className="docs-nav__item bk-nav__sub"
                            onClick={() => go({ page: p.id, anchor: t.id })}
                          >
                            {t.text}
                          </button>
                        ))}
                      </div>
                    );
                  })}
                </div>
              ))}
            </div>
          )}
        </aside>

        <main ref={pane} onScroll={spy} className="docs-main">
          <article className="docs-article">
            <div className="docs-meta">
              <span>{page.group}</span>
            </div>
            <h1 className="docs-h1">{page.title}</h1>
            <Markdown docs={docs} page={page} go={go} theme={theme} />
            <nav className="docs-pager">
              <div>
                {prev && (
                  <button type="button" className="docs-pager__btn" onClick={() => go({ page: prev.id, anchor: null })}>
                    <span className="docs-pager__dir">Previous</span>
                    <span className="docs-pager__title">{prev.title}</span>
                  </button>
                )}
              </div>
              <div>
                {next && (
                  <button
                    type="button"
                    className="docs-pager__btn"
                    style={{ textAlign: "right" }}
                    onClick={() => go({ page: next.id, anchor: null })}
                  >
                    <span className="docs-pager__dir">Next</span>
                    <span className="docs-pager__title">{next.title}</span>
                  </button>
                )}
              </div>
            </nav>
          </article>
        </main>

        <aside className="docs-toc bk-toc">
          {page.toc.length > 0 && (
            <div style={{ display: "grid", gap: 1 }}>
              <div className="bk-eyebrow docs-nav__label">On this page</div>
              {page.toc.map((t) => (
                <button
                  key={t.id}
                  type="button"
                  className="docs-toc__item"
                  aria-current={t.id === active ? "location" : undefined}
                  onClick={() => go({ page: page.id, anchor: t.id })}
                >
                  {t.text}
                </button>
              ))}
            </div>
          )}
        </aside>
      </div>

      {exporting && (
        <div ref={printRoot} className="bk-print" aria-hidden="true">
          {docs.pages.map((p) => (
            <section key={p.id} data-page={p.id} className="docs-article">
              <div className="docs-meta">
                <span>{p.group}</span>
              </div>
              <h1 className="docs-h1">{p.title}</h1>
              <Markdown docs={docs} page={p} go={() => {}} theme={theme} />
            </section>
          ))}
        </div>
      )}

      {confirmDelete && (
        <Dialog
          title={`Delete ${book.id}?`}
          width={480}
          onClose={() => setConfirmDelete(false)}
          footer={
            <>
              <span className="wp-spacer" />
              <Button onClick={() => setConfirmDelete(false)}>Keep it</Button>
              <Button
                variant="danger"
                onClick={() =>
                  void api
                    .deleteBook(ws, book.id)
                    .then(() => {
                      setConfirmDelete(false);
                      open("ws:docs");
                      close(`book:${book.id}`);
                    })
                    .catch((e: unknown) => setDeleteError(e instanceof Error ? e.message : String(e)))
                }
              >
                Delete the book
              </Button>
            </>
          }
        >
          <div className="wp-stack">
            <span>
              This deletes <code>.ostra/docs/{book.id}/</code> with every file in it. The next documentation task on{" "}
              {book.projects.join(", ")} writes a new book.
            </span>
            {deleteError && <Banner tone="bad">{deleteError}</Banner>}
          </div>
        </Dialog>
      )}
    </div>
  );
}
