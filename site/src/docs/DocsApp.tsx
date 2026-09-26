import { Icon, IconButton, Input, LiveMark, REST } from "@ostra/design";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { GitHubMark } from "../shared/GitHubMark";
import { REPO, REPO_FILE } from "../shared/links";
import { useSiteTheme } from "../shared/theme";
import { Markdown } from "./Markdown";
import { Docs, MAX_HITS, type Target } from "./model";
import { NAV } from "./pages";

const NARROW = 900;
const WIDE = 1240;
const MIN_QUERY = 2;

const hashOf = (t: Target) => `#${t.page}${t.anchor ? `/${t.anchor}` : ""}`;
const sectionNum = (section?: string) => (section && /^\d+$/.test(section) ? `§${section}` : "");

function useWidth() {
  const [w, setW] = useState(window.innerWidth);
  useEffect(() => {
    const on = () => setW(window.innerWidth);
    window.addEventListener("resize", on);
    return () => window.removeEventListener("resize", on);
  }, []);
  return w;
}

export function DocsApp() {
  const docs = useMemo(() => new Docs(), []);
  const [theme, setTheme] = useSiteTheme();
  const [target, setTarget] = useState<Target>(() => docs.fromHash(location.hash));
  const [query, setQuery] = useState("");
  const [navOpen, setNavOpen] = useState(false);
  const [active, setActive] = useState<string | null>(null);
  const pane = useRef<HTMLElement>(null);
  const width = useWidth();
  const narrow = width < NARROW;

  const page = docs.page(target.page)!;
  const at = docs.pages.indexOf(page);
  const prev = docs.pages[at - 1];
  const next = docs.pages[at + 1];

  useEffect(() => {
    const on = () => setTarget(docs.fromHash(location.hash));
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, [docs]);

  useEffect(() => {
    document.title = `${page.title} · Ostra docs`;
  }, [page]);

  const spy = useCallback(() => {
    const el = pane.current;
    if (!el) return;
    let id: string | null = null;
    for (const t of page.toc) {
      const h = document.getElementById(t.id);
      if (h && h.offsetTop - 96 <= el.scrollTop) id = t.id;
    }
    if (page.toc.length && el.scrollTop + el.clientHeight >= el.scrollHeight - 4) id = page.toc[page.toc.length - 1].id;
    setActive(id);
  }, [page]);

  const scrollToTarget = useCallback(() => {
    const el = pane.current;
    if (!el) return;
    const h = target.anchor ? document.getElementById(target.anchor) : null;
    el.scrollTop = h ? Math.max(0, h.offsetTop - 16) : 0;
    spy();
  }, [target, spy]);

  useEffect(() => {
    const id = requestAnimationFrame(scrollToTarget);
    return () => cancelAnimationFrame(id);
  }, [scrollToTarget]);

  const go = useCallback(
    (t: Target) => {
      setNavOpen(false);
      const hash = hashOf(t);
      if (location.hash === hash) scrollToTarget();
      else location.hash = hash;
    },
    [scrollToTarget],
  );

  const raf = useRef(0);
  const onScroll = () => {
    if (raf.current) return;
    raf.current = requestAnimationFrame(() => {
      raf.current = 0;
      spy();
    });
  };

  const q = query.trim();
  const searching = q.length >= MIN_QUERY;
  const hits = useMemo(() => (searching ? docs.search(q) : []), [docs, q, searching]);
  const showNav = !narrow || navOpen;

  return (
    <div className="docs">
      <header className="docs-bar">
        {narrow && (
          <IconButton
            icon="panel-left"
            label={navOpen ? "Hide the contents" : "Show the contents"}
            active={navOpen}
            onClick={() => setNavOpen((o) => !o)}
          />
        )}
        <a href="../" className="docs-bar__home">
          <LiveMark state={REST} size={20} />
          <span className="docs-bar__name">Ostra</span>
        </a>
        <span className="docs-bar__sep">/</span>
        <span className="docs-bar__crumb">Docs</span>
        <div className="docs-bar__search">
          <Input
            size="sm"
            icon="search"
            placeholder="Search every page"
            aria-label="Search the docs"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setNavOpen(true);
            }}
          />
        </div>
        <a href={REPO} className="site-quiet-link docs-bar__github" aria-label="GitHub">
          <GitHubMark />
          <span>GitHub</span>
        </a>
        <IconButton
          icon={theme === "dark" ? "sun" : "moon"}
          label={theme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
        />
      </header>

      <div className="docs-body">
        {showNav && (
          <aside className={narrow ? "docs-nav docs-nav--overlay" : "docs-nav"}>
            {searching ? (
              <div style={{ display: "grid", gap: 4 }}>
                <div className="site-eyebrow docs-nav__label">
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
                {NAV.map((g) => (
                  <div key={g.label} style={{ display: "grid", gap: 1 }}>
                    <div className="site-eyebrow docs-nav__label">{g.label}</div>
                    {g.pages.map((p) => (
                      <button
                        key={p.id}
                        type="button"
                        className="docs-nav__item"
                        aria-current={p.id === page.id ? "page" : undefined}
                        onClick={() => go({ page: p.id, anchor: null })}
                      >
                        <span style={{ flex: 1, minWidth: 0 }}>{p.title}</span>
                        <span className="docs-nav__num">{sectionNum(p.section)}</span>
                      </button>
                    ))}
                  </div>
                ))}
              </div>
            )}
          </aside>
        )}

        <main ref={pane} onScroll={onScroll} className="docs-main">
          <article className="docs-article">
            <div className="docs-meta">
              <span>{page.group}</span>
              {sectionNum(page.section) && <span className="docs-mono">{sectionNum(page.section)}</span>}
              <a href={REPO_FILE + page.file} className="docs-meta__source">
                <Icon name="file-text" size={12} />
                {page.file}
              </a>
            </div>
            <h1 className="docs-h1">{page.title}</h1>
            <Markdown docs={docs} page={page} go={go} />
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

        {width >= WIDE && (
          <aside className="docs-toc">
            {page.toc.length > 0 && (
              <div style={{ display: "grid", gap: 1 }}>
                <div className="site-eyebrow docs-nav__label">On this page</div>
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
        )}
      </div>
    </div>
  );
}
