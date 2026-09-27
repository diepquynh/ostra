import { Banner, Chip, DiffView, Icon, IconButton, Spinner, StatusDot, type Tone } from "@ostra/design";
import { type ReactNode, type RefObject, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useLocation } from "react-router";
import { api, downloadUrl } from "../../api";
import type { DiffFile, DocumentView as DocView, FactCheckView } from "../../api/types";
import { Markdown } from "../../components/Markdown";
import { EARS_NOTE } from "../../content/stages";
import { basename } from "../../lib/format";
import { useAsync, useStoredFlag } from "../../lib/hooks";
import { type LedgerFinding, ledgerLoop, parseLedger } from "../../lib/ledger";
import { useWorkspaceTree } from "../../lib/live";
import { useNav, useShell } from "../../lib/nav";
import { ArtifactMarkdown } from "../../screens/artifact/ArtifactMarkdown";
import { docChapters } from "../../screens/artifact/doc/DocumentView";
import { buildMarks, normId, parseAnchor, pickFactCheck } from "../../screens/artifact/doc/model";
import { DocContext, type DocCtx, Ref } from "../../screens/artifact/doc/parts";
import { approvalBadges, artifactKind, hasEarsNote, KIND_LABEL } from "../../screens/artifact/kind";
import { sessionFromPath } from "../../screens/artifact/LedgerView";
import { inlineCode } from "../../screens/execution/inline";
import { unifiedDiff } from "./lineDiff";
import "../../screens/artifact/artifact.css";
import "../../screens/artifact/doc/doc.css";
import "./MArtifact.css";

const SEVERITY_TONE: Record<string, Tone> = { BLOCKER: "bad", HIGH: "bad", MEDIUM: "warn", LOW: "neutral" };
const IMAGE = /\.(png|jpe?g|gif|webp|avif)$/i;
const sameFile = (a: string, b: string) => a.endsWith(b) || b.endsWith(a);

const byId = (box: HTMLElement | null, id: string) =>
  box ? (Array.from(box.querySelectorAll<HTMLElement>("[id]")).find((e) => e.id === id) ?? null) : null;

/** A typed spec, plan, phase file or research as paged chapters under a row of chapter chips. */
function MDocument({
  path,
  view,
  check,
  root,
  note,
}: {
  path: string;
  view: DocView;
  check: FactCheckView | null;
  root: RefObject<HTMLDivElement | null>;
  /** Shown under the chapter chips on the first chapter, such as the EARS reading note. */
  note?: ReactNode;
}) {
  const nav = useNav();
  const location = useLocation();
  const marks = useMemo(() => buildMarks(view.issues, check), [view.issues, check]);
  const { chapters, where } = useMemo(() => docChapters(view.document, check, marks), [view.document, marks, check]);

  const hash = decodeURIComponent(location.hash.replace(/^#/, ""));
  const [at, setAt] = useState(() => parseAnchor(hash));
  useEffect(() => {
    const next = parseAnchor(hash);
    if (next.chapter) setAt(next);
  }, [hash]);
  const current = chapters.find((c) => c.id === at.chapter) ?? chapters[0];

  // The anchor goes in the URL without a history entry, so Back leaves the artifact instead of paging chapters.
  const show = useCallback(
    (chapter: string, element: string | null) => {
      setAt({ chapter, element });
      nav.open(`artifact:${path}`, { anchor: element ? `${chapter}/${element}` : chapter, preview: true });
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

  const chips = useRef<HTMLDivElement>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs on every chapter or element change.
  useEffect(() => {
    const el = at.element ? root.current?.querySelector<HTMLElement>(`[data-el="${CSS.escape(at.element)}"]`) : null;
    if (el) el.scrollIntoView?.({ block: "center" });
    else if (at.chapter) root.current?.scrollIntoView?.({ block: "start" });
    chips.current
      ?.querySelector<HTMLElement>('[aria-current="true"]')
      ?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [at, current.id]);

  const index = chapters.indexOf(current);
  const prev = chapters[index - 1];
  const next = chapters[index + 1];

  return (
    <DocContext.Provider value={ctx}>
      <div className="ma-chiprow" ref={chips} role="tablist" aria-label="Chapters">
        {chapters.map((c) => (
          <button
            type="button"
            key={c.id}
            role="tab"
            className="ma-chapter"
            aria-current={c.id === current.id}
            aria-selected={c.id === current.id}
            onClick={() => show(c.id, null)}
          >
            {c.title}
            {c.count !== undefined && <span className="ma-count">{c.count}</span>}
            {(c.tone || c.attention) && (
              <StatusDot tone={c.tone === "bad" ? "bad" : c.tone === "info" ? "info" : "warn"} size={6} />
            )}
          </button>
        ))}
      </div>
      {index === 0 && note}
      <article className="ma-pad ma-doc doc-article">
        {current.id === "overview" && view.issues.length > 0 && (
          <Banner
            tone={view.issues.some((i) => i.level === "error") ? "bad" : "warn"}
            title={`Ostra's checks found ${view.issues.length} ${view.issues.length === 1 ? "issue" : "issues"}`}
            style={{ margin: 0 }}
          >
            <ul className="doc-issues">
              {view.issues.map((i, n) => (
                <li key={n}>
                  {i.element && <Ref id={i.element} />} {i.message}
                </li>
              ))}
            </ul>
          </Banner>
        )}
        <h2 className="ma-chapter-title">{current.title}</h2>
        <div className="doc-chapter" data-chapter={current.id}>
          {current.render()}
        </div>
      </article>
      <div className="ma-pad ma-pager">
        <button type="button" className="m-btn" disabled={!prev} onClick={() => prev && show(prev.id, null)}>
          <Icon name="chevron-left" size={15} /> Previous
        </button>
        <button type="button" className="m-btn" disabled={!next} onClick={() => next && show(next.id, null)}>
          Next <Icon name="chevron-right" size={15} />
        </button>
      </div>
    </DocContext.Provider>
  );
}

function FindingCard({ f }: { f: LedgerFinding }) {
  return (
    <div className="ma-card">
      <div className="ma-card__head">
        {f.id && (
          <Chip mono tone="accent">
            {f.id}
          </Chip>
        )}
        <Chip tone={SEVERITY_TONE[f.severity] ?? "neutral"}>{f.severity}</Chip>
        <span className="ma-muted-sm">Pass {f.iteration}</span>
      </div>
      <span className="ma-card__body">{inlineCode(f.description)}</span>
      {f.fix && <span className="ma-card__sub">Fix: {inlineCode(f.fix)}</span>}
      <span className="ma-meta">
        {f.file}
        {f.rule && ` · ${f.rule}`}
      </span>
    </div>
  );
}

/** A review ledger: its findings as cards, then the loop's changes as unified diffs. */
function MLedger({ path, content, session }: { path: string; content: string; session: string | null }) {
  const findings = useMemo(() => parseLedger(content), [content]);
  const loop = ledgerLoop(path);
  const sid = session ?? sessionFromPath(path);
  const [diff, setDiff] = useState<DiffFile[] | null>(null);
  const [missing, setMissing] = useState(false);
  useEffect(() => {
    setDiff(null);
    setMissing(false);
    if (!sid || !loop?.project) {
      setMissing(true);
      return;
    }
    let alive = true;
    api.diff(sid, loop.project, loop.phase).then(
      (d) => alive && setDiff(d),
      () => alive && setMissing(true),
    );
    return () => {
      alive = false;
    };
  }, [sid, loop?.project, loop?.phase]);

  return (
    <div className="ma-pad" style={{ gap: 12 }}>
      <span className="m-help">
        The reviewer appends one iteration per pass, and the fix agent records FIXED or WONTFIX against each finding.
        The loop stops when a pass comes back clean, or asks you after 3 passes.
      </span>
      <span className="m-label">Findings · {findings.length}</span>
      {findings.length === 0 && <span className="m-muted">No findings in this ledger.</span>}
      {findings.map((f, i) => (
        <FindingCard key={`${f.iteration}:${f.id || i}`} f={f} />
      ))}
      <span className="m-label" style={{ marginTop: 8 }}>
        Changes in this loop
      </span>
      {diff === null && !missing && (
        <span className="m-muted" style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <Spinner size={11} /> Loading the diff for this loop…
        </span>
      )}
      {(missing || (diff && diff.length === 0)) && <span className="m-muted">No diff is available for this loop.</span>}
      {diff?.map((f) => {
        const mine = findings.filter((x) => sameFile(f.path, x.file));
        return (
          <div key={f.path} className="ma-diff">
            <div className="ma-diff__head">
              <Icon name="file-diff" size={14} style={{ color: "var(--text-muted)", flex: "none" }} />
              <span className="ma-diff__path">{f.path}</span>
            </div>
            {mine.length > 0 && (
              <div className="ma-diff__chips">
                {mine.map((x, i) => (
                  <Chip key={`${x.iteration}:${x.id || i}`} tone={SEVERITY_TONE[x.severity] ?? "neutral"}>
                    {x.id || x.rule} {x.severity}
                  </Chip>
                ))}
              </div>
            )}
            <div className="ma-diff__body">
              <DiffView lines={unifiedDiff(f.original, f.modified)} />
            </div>
          </div>
        );
      })}
    </div>
  );
}

/** `artifact:<path>`: a spec, plan, research, report, review ledger or upload, sized for a phone. */
export function MArtifact({ ws, path }: { ws: string; path: string }) {
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
  const [asMarkdown, setAsMarkdown] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const typed = art.data?.document ?? null;
  const target = typed?.document.kind === "spec" ? "spec" : typed && typed.document.kind !== "research" ? "plan" : null;
  const check = target ? pickFactCheck(detail.data?.fact_checks, target) : null;
  const outline = useMemo(() => (art.data?.headings ?? []).filter((h) => h.level === 2), [art.data]);

  const go = useCallback(
    (id: string) => {
      byId(root.current, id)?.scrollIntoView?.({ block: "start", behavior: "smooth" });
      nav.open(`artifact:${path}`, { anchor: id, preview: true });
    },
    [nav, path],
  );
  // A `#<id>` deep link into a markdown artifact scrolls once the content is rendered.
  const hash = decodeURIComponent(location.hash.replace(/^#/, ""));
  const scrolled = useRef(false);
  useEffect(() => {
    if (!hash || typed || !art.data || scrolled.current) return;
    scrolled.current = true;
    requestAnimationFrame(() => byId(root.current, hash)?.scrollIntoView?.({ block: "start" }));
  }, [hash, typed, art.data]);

  const showNote = hasEarsNote(kind) && !noteHidden && (!typed || !asMarkdown);
  const note = showNote && (
    <div className="ma-pad">
      <Banner
        tone="info"
        title={EARS_NOTE.title}
        actions={<IconButton size="sm" icon="x" label="Hide the note" onClick={() => setNoteHidden(true)} />}
        style={{ margin: 0 }}
      >
        <Markdown text={EARS_NOTE.body} className="art-ears" />
      </Banner>
    </div>
  );

  let body: ReactNode;
  if (art.error)
    body = (
      <div className="ma-pad">
        <Banner tone="bad" title="Ostra could not read this artifact" style={{ margin: 0 }}>
          {art.error.message}
        </Banner>
        <button type="button" className="m-btn" onClick={art.reload}>
          Try again
        </button>
      </div>
    );
  else if (!art.data)
    body = (
      <span className="ma-pad m-muted" style={{ flexDirection: "row", alignItems: "center" }}>
        <Spinner size={11} /> Reading the artifact…
      </span>
    );
  else if (art.data.binary)
    body = (
      <div className="ma-pad">
        {IMAGE.test(path) ? (
          <img className="ma-image" src={downloadUrl(path)} alt={ref?.label ?? basename(path)} />
        ) : (
          <Banner tone="info" title="No preview for this file" style={{ margin: 0 }}>
            It is not text, or it is larger than 4 MB. Download it to open it on this device. The agents read it from{" "}
            {path}.
          </Banner>
        )}
      </div>
    );
  else if (typed && !asMarkdown) body = <MDocument path={path} view={typed} check={check} root={root} note={note} />;
  else if (typed)
    body = (
      <div className="ma-pad">
        <span className="ma-meta">{path}</span>
        <pre className="ma-source">{art.data.content}</pre>
      </div>
    );
  else
    body = (
      <>
        {outline.length > 1 && (
          <div className="ma-chiprow" role="navigation" aria-label="Outline">
            {outline.map((h) => (
              <button type="button" key={h.id} className="ma-chapter" onClick={() => go(h.id)}>
                {h.title}
              </button>
            ))}
          </div>
        )}
        {kind === "ledger" && <MLedger path={path} content={art.data.content} session={owner?.id ?? null} />}
        <article className="ma-pad ma-md">
          <ArtifactMarkdown text={art.data.content} headings={art.data.headings} onAnchor={go} />
        </article>
      </>
    );

  return (
    <div className="ma-page" ref={root}>
      <div className="ma-pad ma-chips">
        <Chip tone="accent">{KIND_LABEL[kind]}</Chip>
        {ref?.project && <Chip mono>{ref.project}</Chip>}
        {badges.map((b) => (
          <Chip key={b.label} tone={b.tone} icon={b.icon}>
            {b.label}
          </Chip>
        ))}
      </div>
      {typed && (
        <div className="ma-pad">
          <div className="ma-segment" role="tablist" aria-label="View">
            <button type="button" role="tab" aria-selected={!asMarkdown} onClick={() => setAsMarkdown(false)}>
              <Icon name="file-text" size={14} /> Document
            </button>
            <button type="button" role="tab" aria-selected={asMarkdown} onClick={() => setAsMarkdown(true)}>
              <Icon name="file-code-2" size={14} /> Markdown
            </button>
          </div>
        </div>
      )}
      {!(typed && !asMarkdown) && note}
      {body}
      <div className="ma-pad ma-actions">
        {owner && (
          <button type="button" className="m-btn m-btn-sm" onClick={() => nav.open(`session:${owner.id}`)}>
            <Icon name="kanban" size={14} /> Session board
          </button>
        )}
        <button type="button" className="m-btn m-btn-sm" onClick={() => shell.openDock(`In ${basename(path)}, `)}>
          <Icon name="message-square" size={14} /> Ask about it
        </button>
        <a className="m-btn m-btn-sm" href={downloadUrl(path)} download={basename(path)}>
          <Icon name="download" size={14} /> Download
        </a>
        <button
          type="button"
          className="m-btn m-btn-sm"
          onClick={() => void navigator.clipboard?.writeText(path).catch(() => {})}
        >
          <Icon name="copy" size={14} /> Copy path
        </button>
        {hasEarsNote(kind) && noteHidden && (
          <button type="button" className="m-btn m-btn-sm" onClick={() => setNoteHidden(false)}>
            <Icon name="info" size={14} /> How to read requirements
          </button>
        )}
      </div>
    </div>
  );
}
