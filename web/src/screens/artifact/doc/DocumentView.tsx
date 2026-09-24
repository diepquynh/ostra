import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useLocation } from "react-router";
import type { Document, DocumentView as DocView, FactCheckView } from "../../../api/types";
import { Banner, Button, TreeItem } from "../../../design";
import { useNav } from "../../../lib/nav";
import { buildMarks, normId, parseAnchor, worst, type Chapter, type MarkTone, type Outline } from "./model";
import { DocContext, FactCheckChapter, Ref, type DocCtx } from "./parts";
import { phaseOutline, planOutline } from "./plan";
import { researchOutline } from "./research";
import { specOutline } from "./spec";
import "./doc.css";

function outlineOf(doc: Document): Outline {
  switch (doc.kind) {
    case "research":
      return researchOutline(doc.doc);
    case "spec":
      return specOutline(doc.doc);
    case "plan":
      return planOutline(doc.doc);
    case "phase":
      return phaseOutline(doc.doc);
  }
}

export interface DocumentViewProps {
  /** The artifact's markdown path, the tab's resource. */
  path: string;
  view: DocView;
  /** The fact-check pass over this document, if one ran. */
  check: FactCheckView | null;
  /** Toolbar, title, and notes shown above the chapter. */
  header: ReactNode;
}

/**
 * A typed document as paged chapters: the menu on the left, one chapter at a time on the right.
 * `#chapter` or `#chapter/element` deep-links a chapter and an element in it.
 */
export function DocumentView({ path, view, check, header }: DocumentViewProps) {
  const nav = useNav();
  const location = useLocation();
  const scroller = useRef<HTMLDivElement>(null);
  const marks = useMemo(() => buildMarks(view.issues, check), [view.issues, check]);

  const { chapters, where } = useMemo(() => {
    const o = outlineOf(view.document);
    const chapters: Chapter[] = [...o.chapters];
    if (check) {
      chapters.push({ id: "fact-check", title: "Fact-check", count: check.findings.length, render: () => <FactCheckChapter check={check} /> });
    }
    const tones = new Map<string, MarkTone[]>();
    for (const [key, list] of marks) {
      for (const m of list) {
        const at = (key && o.where.get(key)) || (m.source === "fact-check" && check ? "fact-check" : "overview");
        tones.set(at, [...(tones.get(at) ?? []), m.tone]);
      }
    }
    let parent: Chapter | null = null;
    for (const c of chapters) {
      c.tone = worst(tones.get(c.id) ?? []);
      if (!c.depth) parent = c;
      else if (parent && c.tone) parent.tone = worst([parent.tone ?? c.tone, c.tone]);
    }
    return { chapters, where: o.where };
  }, [view.document, marks, check]);

  const hash = decodeURIComponent(location.hash.replace(/^#/, ""));
  const [at, setAt] = useState(() => parseAnchor(hash));
  useEffect(() => {
    const next = parseAnchor(hash);
    if (next.chapter) setAt(next);
  }, [hash]);

  const current = chapters.find((c) => c.id === at.chapter) ?? chapters[0];

  const show = useCallback(
    (chapter: string, element: string | null) => {
      setAt({ chapter, element });
      nav.open(`artifact:${path}`, { anchor: element ? `${chapter}/${element}` : chapter });
    },
    [nav, path],
  );

  const ctx = useMemo<DocCtx>(
    () => ({
      go: (ref) => {
        if (chapters.some((c) => c.id === ref)) return show(ref, null);
        const key = normId(ref);
        const chapter = where.get(key);
        if (chapter) show(chapter, key);
      },
      open: (id) => nav.open(id),
      marks,
      focus: at.element,
      known: (ref) => where.has(normId(ref)),
    }),
    [chapters, where, show, nav, marks, at.element],
  );

  useEffect(() => {
    const box = scroller.current;
    if (!box) return;
    const el = at.element ? box.querySelector<HTMLElement>(`[data-el="${CSS.escape(at.element)}"]`) : null;
    if (el) el.scrollIntoView?.({ block: "center" });
    else box.scrollTo?.({ top: 0 });
  }, [at, current.id]);

  const index = chapters.indexOf(current);
  const prev = chapters[index - 1];
  const next = chapters[index + 1];

  return (
    <DocContext.Provider value={ctx}>
      <nav className="art-outline" aria-label="Chapters">
        <div className="os-tree-section">Chapters</div>
        <div role="tree">
          {chapters.map((c) => (
            <TreeItem
              key={c.id}
              label={c.title}
              depth={c.depth ?? 0}
              selected={c.id === current.id}
              meta={c.count !== undefined ? String(c.count) : undefined}
              trailing={c.tone || c.attention ? <span className={`doc-dot doc-dot--${c.tone ?? "warn"}`} aria-label="Needs attention" /> : undefined}
              onClick={() => show(c.id, null)}
            />
          ))}
        </div>
        <div style={{ height: 14 }} />
        <div className="os-tree-section">File</div>
        <div className="art-outline__path">{path}</div>
      </nav>
      <div className="art-scroll" ref={scroller}>
        <article className="art-article doc-article">
          {header}
          {current.id === "overview" && view.issues.length > 0 && (
            <Banner tone={view.issues.some((i) => i.level === "error") ? "bad" : "warn"} title={`Ostra's checks found ${view.issues.length} ${view.issues.length === 1 ? "issue" : "issues"}`} style={{ marginBottom: 18 }}>
              <ul className="doc-issues">
                {view.issues.map((i, n) => (
                  <li key={n}>
                    {i.element && <Ref id={i.element} />} {i.message}
                  </li>
                ))}
              </ul>
            </Banner>
          )}
          <h2 className="doc-chapter-title">{current.title}</h2>
          <div className="doc-chapter" data-chapter={current.id}>
            {current.render()}
          </div>
          <div className="doc-pager">
            {prev ? (
              <Button size="sm" variant="ghost" icon="arrow-left" onClick={() => show(prev.id, null)}>
                {prev.title}
              </Button>
            ) : (
              <span />
            )}
            {next && (
              <Button size="sm" variant="ghost" iconRight="arrow-right" onClick={() => show(next.id, null)}>
                {next.title}
              </Button>
            )}
          </div>
        </article>
      </div>
    </DocContext.Provider>
  );
}
