import { Banner, Chip, Icon, Spinner, StatusDot, Switch } from "@ostra/design";
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import { useProjectFiles } from "../../features/context/FileTagInput";
import {
  ARTIFACTS_ROOT,
  activeQuery,
  filesInText,
  formatBytes,
  isFolder,
  rankFiles,
} from "../../features/context/tags";
import { useUploads } from "../../features/context/uploads";
import { formatCost, relativeTime } from "../../lib/format";
import { useAsync } from "../../lib/hooks";
import { useActivity, useSessionSummaries } from "../../lib/live";
import { useNav, useShell, useWorkspace } from "../../lib/nav";
import { sessionTitle } from "../../screens/workspace/sessionFilter";
import {
  completeTag,
  filterHome,
  HOME_FILTERS,
  type HomeFilter,
  homeCounts,
  STATUS_TONE,
  stageLine,
  taggedFiles,
} from "./homeLogic";
import "./MHome.css";

/** `ws:overview`: the gate call-out, New task, the session list and the projects. */
export function MHome({ ws }: { ws: string }) {
  const { detail, reload } = useWorkspace();
  const { open } = useNav();
  const { addProject } = useShell();
  const sessions = useSessionSummaries(ws);
  const { activity } = useActivity(ws);
  const [filter, setFilter] = useState<HomeFilter>("all");
  const artifacts = useAsync(() => api.artifacts(ws), [ws]);

  const counts = homeCounts(sessions.sessions);
  const rows = filterHome(sessions.sessions, filter);
  const gates = activity?.open_gates ?? [];
  const gate = gates[0];
  const projects = detail?.projects ?? [];
  const uninitialized = projects.filter((p) => p.init_status === "not_initialized");
  const artCount = artifacts.data?.artifacts.filter((a) => !a.hidden).length;

  return (
    <div className="m-page">
      {gate && (
        <button
          type="button"
          className="mh-gate"
          onClick={() =>
            open(`session:${gate.session}`, gate.id.startsWith("session:") ? {} : { anchor: `gate-${gate.id}` })
          }
        >
          <Icon name="shield-alert" size={18} style={{ color: "var(--warn)", flex: "none" }} />
          <span className="m-row-body">
            <span style={{ fontSize: 14, fontWeight: 600 }}>
              {gates.length === 1 ? "1 gate is waiting for you" : `${gates.length} gates are waiting for you`}
            </span>
            <span style={{ fontSize: 12, color: "var(--text-secondary)" }}>
              {[gate.session_title, gate.title].filter(Boolean).join(" · ")}
            </span>
          </span>
          <Icon name="chevron-right" size={16} style={{ color: "var(--text-muted)" }} />
        </button>
      )}
      {detail && detail.validation.length > 0 && (
        <Banner tone="bad" style={{ margin: 0 }}>
          Settings have {detail.validation.length} problem{detail.validation.length === 1 ? "" : "s"}. Fix them in{" "}
          <button type="button" className="mh-inline-link" onClick={() => open("ws:settings")}>
            Settings
          </button>{" "}
          before starting work, because an agent whose route does not resolve cannot start.
          {detail.fixes.length > 0 && (
            <>
              {" "}
              <button
                type="button"
                className="mh-inline-link"
                onClick={() => {
                  api.fixSettings(ws).then(reload, () => open("ws:settings"));
                }}
              >
                Fix {detail.fixes.length === 1 ? "it" : `${detail.fixes.length}`} now
              </button>
            </>
          )}
        </Banner>
      )}
      {uninitialized.length > 0 && (
        <Banner tone="warn" style={{ margin: 0 }}>
          {uninitialized.map((p) => p.key).join(", ")} {uninitialized.length === 1 ? "is" : "are"} not initialized.
          Pipeline tasks cannot target an uninitialized project. Open it below to initialize it.{" "}
          <button type="button" className="mh-inline-link" onClick={reload}>
            Check again
          </button>
        </Banner>
      )}

      {detail ? (
        <NewTaskSection ws={ws} onStarted={sessions.reload} />
      ) : (
        <span className="m-muted" style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <Spinner size={11} /> Reading the workspace…
        </span>
      )}

      <section className="m-section">
        <span className="m-label">Sessions</span>
        <div className="mh-seg" role="tablist" aria-label="Session filter">
          {HOME_FILTERS.map((f) => (
            <button
              type="button"
              role="tab"
              key={f.id}
              aria-selected={filter === f.id}
              className="mh-seg__item"
              onClick={() => setFilter(f.id)}
            >
              {f.label}
              <span className="mh-seg__count">{counts[f.id]}</span>
            </button>
          ))}
        </div>
        {sessions.error && (
          <Banner tone="bad" style={{ margin: 0 }}>
            {sessions.error.message}
          </Banner>
        )}
        <div style={{ display: "flex", flexDirection: "column" }}>
          {rows.map((s) => {
            const line = stageLine(s);
            return (
              <button type="button" key={s.id} className="mh-session" onClick={() => open(`session:${s.id}`)}>
                <span style={{ paddingTop: 5 }}>
                  <StatusDot tone={STATUS_TONE[s.status]} pulse={s.status === "running"} />
                </span>
                <span className="m-row-body" style={{ gap: 3 }}>
                  <span className="m-row-title">{sessionTitle(s)}</span>
                  <span
                    style={{
                      fontSize: 12,
                      color: line.tone ? `var(--${line.tone})` : "var(--text-secondary)",
                    }}
                  >
                    {line.text}
                  </span>
                  <span className="m-mono-sub">
                    {[s.projects.join(", "), relativeTime(s.updated_at)].filter(Boolean).join(" · ")}
                  </span>
                </span>
                <span style={{ display: "flex", alignItems: "center", gap: 6, paddingTop: 1 }}>
                  {s.kind.kind === "init" && <Chip tone="info">Init</Chip>}
                  {s.yolo && <Chip tone="warn">YOLO</Chip>}
                  <span style={{ fontFamily: "var(--font-mono)", fontSize: 12, color: "var(--text-secondary)" }}>
                    {formatCost(s.cost_usd)}
                  </span>
                </span>
              </button>
            );
          })}
          {sessions.loading && sessions.sessions.length === 0 && (
            <span className="m-muted" style={{ display: "flex", gap: 8, padding: "16px 2px" }}>
              <Spinner size={11} /> Reading the sessions…
            </span>
          )}
          {!sessions.loading && rows.length === 0 && (
            <span className="m-muted" style={{ padding: "16px 2px" }}>
              {sessions.sessions.length === 0
                ? "No sessions yet. Describe a change above to start one."
                : "No sessions in this filter."}
            </span>
          )}
        </div>
      </section>

      <section className="m-section" style={{ gap: 6 }}>
        <span className="m-label">Projects</span>
        <button type="button" className="m-row" onClick={() => open(`project:${ARTIFACTS_ROOT}`)}>
          <Icon name="package" size={16} style={{ color: "var(--text-muted)" }} />
          <span className="m-row-body">
            <span className="m-row-title">Workspace artifacts</span>
            <span className="m-mono-sub">
              {artCount === undefined
                ? "uploads, samples, exports"
                : `${artCount} ${artCount === 1 ? "file" : "files"} · uploads, samples, exports`}
            </span>
          </span>
          <Icon name="chevron-right" size={16} style={{ color: "var(--text-muted)" }} />
        </button>
        {projects.map((p) => (
          <button type="button" key={p.key} className="m-row" onClick={() => open(`project:${p.key}`)}>
            <Icon name="folder-git-2" size={16} style={{ color: "var(--text-muted)" }} />
            <span className="m-row-body">
              <span className="m-row-title">{p.key}</span>
              <span className="m-mono-sub">{[p.path, p.stack].filter(Boolean).join(" · ")}</span>
            </span>
            {p.init_status === "not_initialized" && <Chip tone="warn">Not initialized</Chip>}
            {p.init_status === "missing" && <Chip tone="bad">Folder missing</Chip>}
            <Icon name="chevron-right" size={16} style={{ color: "var(--text-muted)" }} />
          </button>
        ))}
        <button type="button" className="m-row" onClick={addProject}>
          <Icon name="folder-plus" size={16} style={{ color: "var(--text-muted)" }} />
          <span className="m-row-body">
            <span className="m-row-title" style={{ color: "var(--text-secondary)" }}>
              Add project
            </span>
          </span>
        </button>
      </section>
    </div>
  );
}

/** Collapsed to one line until tapped, then the request, tags, uploads, projects and options. */
function NewTaskSection({ ws, onStarted }: { ws: string; onStarted: () => void }) {
  const { detail } = useWorkspace();
  const { open } = useNav();
  const { taskDraft, setTaskDraft } = useShell();
  const [composing, setComposing] = useState(false);
  const [text, setText] = useState("");
  const [caret, setCaret] = useState(0);
  const [pins, setPins] = useState<string[]>([]);
  const yoloDefault = detail?.settings.yolo.default ?? false;
  const [yolo, setYolo] = useState(yoloDefault);
  const [tests, setTests] = useState(false);
  const [docs, setDocs] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const uploads = useUploads(ws);
  const field = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);

  const initialized = useMemo(
    () => (detail?.projects ?? []).filter((p) => p.init_status === "initialized").map((p) => p.key),
    [detail],
  );
  const roots = useMemo(() => [...initialized, ARTIFACTS_ROOT], [initialized]);
  const files = useProjectFiles(ws, initialized);

  useEffect(() => setYolo(yoloDefault), [yoloDefault]);
  // "Turn into task" from the question sheet: take the text once, then clear it.
  useEffect(() => {
    if (taskDraft === null) return;
    setText(taskDraft);
    setComposing(true);
    setTaskDraft(null);
  }, [taskDraft, setTaskDraft]);
  useEffect(() => {
    if (composing) field.current?.focus();
  }, [composing]);

  const q = composing ? activeQuery(text, caret) : null;
  const suggest = q && files.files ? rankFiles(files.files, q.query, 30) : [];
  // The tag being typed is not judged until the caret leaves it.
  const tags = taggedFiles(text, roots, files.known).filter((t) => t.tag !== q?.query);
  // The file list loads once the first `@` is typed, for suggestions and to check tags.
  const wantFiles = !!q || /(^|\s)@\S+\//.test(text);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `load` only flips a flag.
  useEffect(() => {
    if (wantFiles) files.load();
  }, [wantFiles]);

  const canStart = !busy && !uploads.busy && text.trim() !== "" && initialized.length > 0;
  const reset = () => {
    setComposing(false);
    setText("");
    setPins([]);
    setError(null);
    uploads.clear();
  };

  const start = async () => {
    if (!canStart) return;
    setBusy(true);
    setError(null);
    try {
      const s = await api.createSession(ws, {
        request: text.trim(),
        options: { tests, docs, yolo },
        projects: pins,
        files: filesInText(text, roots, files.known),
        uploads: uploads.ids,
      });
      reset();
      onStarted();
      open(`session:${s.id}`);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const syncCaret = () => setCaret(field.current?.selectionStart ?? text.length);
  const tagAt = () => {
    const next = text + (text && !/\s$/.test(text) ? " @" : "@");
    setText(next);
    setCaret(next.length);
    requestAnimationFrame(() => {
      field.current?.focus();
      field.current?.setSelectionRange(next.length, next.length);
    });
  };

  return (
    <section className="m-section">
      <span className="m-label">New task</span>
      {!composing ? (
        <button type="button" className="mh-compose" onClick={() => setComposing(true)}>
          <Icon name="plus" size={16} />
          Describe the change you want
        </button>
      ) : (
        <div className="m-card">
          <textarea
            ref={field}
            className="m-input"
            rows={4}
            aria-label="Request"
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setCaret(e.target.selectionStart ?? e.target.value.length);
            }}
            onSelect={syncCaret}
            onKeyUp={syncCaret}
            placeholder="Add order cancellation: customers can cancel until the order ships."
          />
          {q && (suggest.length > 0 || !files.files) && (
            <div className="mh-suggest" role="listbox" aria-label="Files">
              {!files.files && (
                <span className="m-muted" style={{ display: "flex", gap: 8, padding: "12px 10px" }}>
                  <Spinner size={11} /> Reading the files…
                </span>
              )}
              {suggest.map((f) => (
                <button
                  type="button"
                  role="option"
                  aria-selected={false}
                  key={`${f.project}/${f.path}`}
                  className="mh-suggest__row"
                  onClick={() => {
                    const done = completeTag(text, caret, q.start, f);
                    setText(done.text);
                    setCaret(done.caret);
                    requestAnimationFrame(() => {
                      field.current?.focus();
                      field.current?.setSelectionRange(done.caret, done.caret);
                    });
                  }}
                >
                  <Icon
                    name={isFolder(f) ? "folder" : "file-code-2"}
                    size={14}
                    style={{ color: "var(--text-muted)", flex: "none" }}
                  />
                  <span className="mh-suggest__label">
                    {f.project}/{f.path}
                  </span>
                </button>
              ))}
            </div>
          )}
          {tags.length > 0 && (
            <div className="mh-tight" style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              <span style={{ fontSize: 12, color: "var(--text-secondary)" }}>Tagged files</span>
              <div className="mh-chips">
                {tags.map((t) => (
                  <span key={t.tag} className={`mh-tag${t.ok ? "" : " mh-tag--bad"}`}>
                    <Icon
                      name={t.ok ? "at-sign" : "circle-alert"}
                      size={12}
                      style={{ color: t.ok ? "var(--text-muted)" : "var(--bad)", flex: "none" }}
                    />
                    {t.tag}
                    {!t.ok && " · not found"}
                  </span>
                ))}
              </div>
            </div>
          )}
          {uploads.items.length > 0 && (
            <div className="mh-chips mh-tight">
              {uploads.items.map((u) => (
                <button
                  type="button"
                  key={u.key}
                  className={`mh-upload${u.error ? " mh-tag--bad" : ""}`}
                  aria-label={`Remove ${u.name}`}
                  title={u.error ?? undefined}
                  onClick={() => uploads.remove(u.key)}
                >
                  {u.id || u.error ? (
                    <Icon name="paperclip" size={12} style={{ color: "var(--text-muted)" }} />
                  ) : (
                    <Spinner size={10} />
                  )}
                  {u.name}
                  <span style={{ color: "var(--text-muted)" }}>{formatBytes(u.size)}</span>
                  <Icon name="x" size={12} />
                </button>
              ))}
            </div>
          )}
          {uploads.items.some((u) => u.error) && (
            <span className="mh-tight" style={{ fontSize: 12, color: "var(--bad)", textWrap: "pretty" }}>
              {uploads.items
                .filter((u) => u.error)
                .map((u) => `${u.name}: ${u.error}`)
                .join(" ")}
            </span>
          )}
          <div className="mh-tight" style={{ display: "flex", gap: 8 }}>
            <button type="button" className="m-btn mh-small" onClick={() => picker.current?.click()}>
              <Icon name="paperclip" size={14} /> Attach
            </button>
            <button type="button" className="m-btn mh-small" onClick={tagAt}>
              <Icon name="at-sign" size={14} /> Tag a file
            </button>
            <input
              ref={picker}
              type="file"
              multiple
              hidden
              aria-label="Attach files"
              onChange={(e) => {
                uploads.add(Array.from(e.target.files ?? []));
                e.target.value = "";
              }}
            />
          </div>
          <span className="m-help" style={{ marginTop: -6 }}>
            Type @ to tag a file or folder in an initialized project. Uploads are saved as workspace artifacts and every
            agent in the session can read them.
          </span>
          {initialized.length > 1 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              <span style={{ fontSize: 12, color: "var(--text-secondary)" }}>
                Projects{" "}
                <span style={{ color: "var(--text-muted)" }}>
                  {pins.length === 0 ? "· none pinned lets Ostra choose" : ""}
                </span>
              </span>
              <div className="mh-chips" style={{ gap: 8 }}>
                {initialized.map((k) => {
                  const on = pins.includes(k);
                  return (
                    <button
                      type="button"
                      key={k}
                      aria-pressed={on}
                      className="mh-pin"
                      onClick={() => setPins((p) => (on ? p.filter((x) => x !== k) : [...p, k]))}
                    >
                      {on && <Icon name="check" size={13} style={{ color: "var(--accent)" }} />}
                      {k}
                    </button>
                  );
                })}
              </div>
            </div>
          )}
          <div className="mh-options">
            <Switch label="Write tests" checked={tests} onChange={(e) => setTests(e.target.checked)} />
            <Switch label="Write docs" checked={docs} onChange={(e) => setDocs(e.target.checked)} />
          </div>
          {docs && (
            <span className="m-help">
              The request steers the book: name the readers, the topics to cover in depth, and what to leave out. Tag or
              upload design notes, specs, or artifacts, and the writers use them as sources. To give every docs run the
              same instructions, write them in{" "}
              <button
                type="button"
                className="mh-inline-link"
                onClick={() => open("ws:settings", { anchor: "setting:instructions.agents.documentation" })}
              >
                Settings
              </button>
              .
            </span>
          )}
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <Switch label="YOLO" tone="warn" checked={yolo} onChange={(e) => setYolo(e.target.checked)} />
            <span className="m-help">
              Ostra answers every gate for you and lists each decision in the completion report.
            </span>
          </div>
          {initialized.length === 0 && (
            <span className="m-help" style={{ color: "var(--warn)" }}>
              No project is initialized yet, so no pipeline task can start.
            </span>
          )}
          {error && (
            <Banner tone="bad" style={{ margin: 0 }}>
              {error}
            </Banner>
          )}
          <div style={{ display: "flex", gap: 8 }}>
            <button type="button" className="m-btn m-btn-quiet" onClick={reset}>
              Cancel
            </button>
            <button
              type="button"
              className="m-btn m-btn-primary"
              style={{ flex: 1 }}
              disabled={!canStart}
              onClick={() => void start()}
            >
              {busy ? "Starting…" : "Start the session"}
            </button>
          </div>
        </div>
      )}
    </section>
  );
}
