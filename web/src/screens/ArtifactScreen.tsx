import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useLocation } from "react-router";
import { api, downloadUrl } from "../api";
import { Markdown } from "../components/Markdown";
import { EARS_NOTE } from "../content/stages";
import { Banner, Button, Chip, Icon, IconButton, Spinner, TreeItem } from "../design";
import { basename } from "../lib/format";
import { useAsync, useStoredFlag } from "../lib/hooks";
import { useWorkspaceTree } from "../lib/live";
import { useNav, useShell } from "../lib/nav";
import { ArtifactMarkdown } from "./artifact/ArtifactMarkdown";
import { DocumentView } from "./artifact/doc/DocumentView";
import { pickFactCheck } from "./artifact/doc/model";
import { approvalBadges, artifactKind, hasEarsNote, KIND_LABEL } from "./artifact/kind";
import { LedgerView } from "./artifact/LedgerView";
import "./artifact/artifact.css";

export type ArtifactScreenProps = {
  ws: string;
  /** Absolute path of the session-dir file (`GET /api/artifacts?path=`). */
  path: string;
};

const SCROLL_OFFSET = 16;

const byId = (box: HTMLElement, id: string) =>
  Array.from(box.querySelectorAll<HTMLElement>("[id]")).find((e) => e.id === id) ?? null;

/**
 * Resource `artifact:<path>`: a spec, plan, report or review ledger. The shell gives this screen the whole
 * center pane without an outer scroll, so it scrolls its own content (outline plus body).
 */
export function ArtifactScreen({ ws, path }: ArtifactScreenProps) {
  const nav = useNav();
  const shell = useShell();
  const location = useLocation();
  const art = useAsync(() => api.artifact(path), [path]);
  const tree = useWorkspaceTree(ws);
  const owner = tree.sessions.find((s) => s.artifacts.some((a) => a.path === path)) ?? null;
  const ref = owner?.artifacts.find((a) => a.path === path) ?? null;
  const kind = artifactKind(path, ref?.kind);
  const detail = useAsync(() => (owner ? api.session(owner.id) : Promise.resolve(null)), [owner?.id]);
  const badges = approvalBadges(detail.data, kind);
  const [noteHidden, setNoteHidden] = useStoredFlag("ostra.earsNoteHidden", false);
  const scroller = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState<string | null>(null);
  const headings = useMemo(() => art.data?.headings ?? [], [art.data]);
  const outline = useMemo(() => headings.filter((h) => h.level <= 3), [headings]);
  const minLevel = outline.reduce((m, h) => Math.min(m, h.level), 6);
  const scrolledTo = useRef<string | null>(null);

  const scrollTo = useCallback((id: string, smooth = true) => {
    const box = scroller.current;
    const el = box && byId(box, id);
    if (!box || !el) return false;
    const top = el.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop - SCROLL_OFFSET;
    box.scrollTo?.({ top, behavior: smooth ? "smooth" : "auto" });
    scrolledTo.current = id;
    setActive(id);
    return true;
  }, []);

  const go = useCallback(
    (id: string) => {
      scrollTo(id);
      nav.open(`artifact:${path}`, { anchor: id });
    },
    [nav, path, scrollTo],
  );

  // A `#<id>` deep link scrolls once the content is rendered. An outline click already scrolled there.
  const hash = decodeURIComponent(location.hash.replace(/^#/, ""));
  useEffect(() => {
    if (!hash || !art.data || scrolledTo.current === hash) return;
    requestAnimationFrame(() => scrollTo(hash, false));
  }, [hash, art.data, scrollTo]);

  // The outline marks the last heading above the top of the view.
  useEffect(() => {
    const box = scroller.current;
    if (!box || outline.length === 0) return;
    const onScroll = () => {
      const limit = box.getBoundingClientRect().top + SCROLL_OFFSET * 3;
      let current: string | null = outline[0].id;
      for (const h of outline) {
        const el = byId(box, h.id);
        if (el && el.getBoundingClientRect().top <= limit) current = h.id;
      }
      setActive(current);
    };
    box.addEventListener("scroll", onScroll, { passive: true });
    return () => box.removeEventListener("scroll", onScroll);
  }, [outline]);

  const title = ref?.label ?? headings.find((h) => h.level === 1)?.title ?? basename(path);
  const typed = art.data?.document ?? null;
  const [asMarkdown, setAsMarkdown] = useState(false);
  const target = typed?.document.kind === "spec" ? "spec" : typed && typed.document.kind !== "research" ? "plan" : null;
  const check = target ? pickFactCheck(detail.data?.fact_checks, target) : null;

  const toolbar = (
    <div className="art-toolbar">
      <Chip tone="accent">{KIND_LABEL[kind]}</Chip>
      {ref?.project && <Chip mono>{ref.project}</Chip>}
      {badges.map((b) => (
        <Chip key={b.label} tone={b.tone} icon={b.icon}>
          {b.label}
        </Chip>
      ))}
      <span style={{ flex: 1 }} />
      {hasEarsNote(kind) && noteHidden && (
        <Button size="sm" variant="ghost" icon="info" onClick={() => setNoteHidden(false)}>
          How to read requirements
        </Button>
      )}
      {typed && (
        <Button
          size="sm"
          variant="ghost"
          icon={asMarkdown ? "list-tree" : "file-text"}
          onClick={() => setAsMarkdown(!asMarkdown)}
        >
          {asMarkdown ? "Document" : "Markdown"}
        </Button>
      )}
      {owner && (
        <IconButton
          size="sm"
          icon="kanban"
          label="Open the session board"
          onClick={() => nav.open(`session:${owner.id}`)}
        />
      )}
      <IconButton
        size="sm"
        icon="message-square"
        label="Ask about this artifact"
        onClick={() => shell.openDock(`In ${basename(path)}, `)}
      />
      <IconButton size="sm" icon="copy" label="Copy path" onClick={() => void navigator.clipboard?.writeText(path)} />
      <a
        className="art-download"
        href={downloadUrl(path)}
        download={basename(path)}
        aria-label="Download"
        title="Download"
      >
        <Icon name="download" size={14} />
      </a>
      <IconButton size="sm" icon="refresh-ccw" label="Reload" onClick={art.reload} />
    </div>
  );
  const earsNote = hasEarsNote(kind) && !noteHidden && (
    <Banner
      tone="info"
      title={EARS_NOTE.title}
      actions={<IconButton size="sm" icon="x" label="Hide the note" onClick={() => setNoteHidden(true)} />}
      style={{ marginBottom: 22 }}
    >
      <Markdown text={EARS_NOTE.body} className="art-ears" />
    </Banner>
  );

  if (typed && !asMarkdown) {
    return (
      <div className="art-screen">
        <DocumentView
          path={path}
          view={typed}
          check={check}
          header={
            <>
              {toolbar}
              <h1 className="art-title">{title}</h1>
              {earsNote}
            </>
          }
        />
      </div>
    );
  }

  return (
    <div className="art-screen">
      <nav className="art-outline" aria-label="Outline">
        <div className="os-tree-section">Outline</div>
        {outline.length === 0 && <div className="art-outline__empty">{art.data ? "No headings." : "…"}</div>}
        <div role="tree">
          {outline.map((h) => (
            <TreeItem
              key={h.id}
              label={h.title}
              title={h.title}
              depth={h.level - minLevel}
              selected={active === h.id}
              onClick={() => go(h.id)}
            />
          ))}
        </div>
        <div style={{ height: 14 }} />
        <div className="os-tree-section">File</div>
        <div className="art-outline__path">{path}</div>
      </nav>
      <div className="art-scroll" ref={scroller}>
        <article className="art-article">
          {toolbar}
          {!headings.some((h) => h.level === 1) && <h1 className="art-title">{title}</h1>}
          {earsNote}
          {art.error ? (
            <Banner
              tone="bad"
              title="Ostra could not read this artifact"
              actions={
                <Button size="sm" onClick={art.reload}>
                  Try again
                </Button>
              }
            >
              {art.error.message}
            </Banner>
          ) : !art.data ? (
            <div className="art-note">
              <Spinner size={11} /> Reading the artifact…
            </div>
          ) : art.data.binary ? (
            <Banner
              tone="info"
              title="No preview for this file"
              actions={
                <a className="os-btn os-btn--sm" href={downloadUrl(path)} download={basename(path)}>
                  <Icon name="download" size={12} /> Download
                </a>
              }
            >
              It is not text, or it is larger than 4 MB. Download it to open it on your computer. The agents read it
              from {path}.
            </Banner>
          ) : (
            <>
              {kind === "ledger" && (
                <LedgerView path={path} content={art.data.content} session={owner?.id ?? null} theme={shell.theme} />
              )}
              <ArtifactMarkdown text={art.data.content} headings={art.data.headings} onAnchor={go} />
            </>
          )}
        </article>
      </div>
    </div>
  );
}
