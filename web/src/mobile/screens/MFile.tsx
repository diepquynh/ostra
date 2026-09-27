import { Chip, Icon, Spinner } from "@ostra/design";
import { useEffect, useMemo, useRef, useState } from "react";
import { useLocation } from "react-router";
import { api } from "../../api";
import { useAsync } from "../../lib/hooks";
import { useProjectFsChanges } from "../../lib/live";
import { lineFromHash, splitLines } from "../../screens/project/code/tokens";
import { useFileEdit } from "../../screens/project/code/useFileEdit";
import { diffRows } from "../../screens/project/diff";
import { extOf, GIT_MARK } from "../../screens/project/files";

type View = "code" | "diff" | "outline";

/** `file:<key>:<path>` on a phone: the text with line numbers, its changes against HEAD, its outline, and a plain editor. */
export function MFile({ ws, projectKey, path }: { ws: string; projectKey: string; path: string }) {
  const file = useAsync(() => api.projectFile(ws, projectKey, path), [ws, projectKey, path]);
  const changed = !!file.data?.git;
  const diff = useAsync(
    () => (changed ? api.projectDiff(ws, projectKey, path) : Promise.resolve(null)),
    [ws, projectKey, path, changed],
  );
  const code = useAsync(() => api.codeFile(ws, projectKey, path), [ws, projectKey, path]);
  const edit = useFileEdit(ws, projectKey, path, file);
  const hash = useLocation().hash;
  const [view, setView] = useState<View>("code");
  const [hl, setHl] = useState<number | null>(() => lineFromHash(hash));
  const [saved, setSaved] = useState<string | null>(null);
  const linesRef = useRef<HTMLDivElement>(null);

  useProjectFsChanges(ws, projectKey, (paths) => {
    if (paths.includes(path)) {
      file.reload();
      diff.reload();
      code.reload();
    }
  });

  // `#edit`, from New file, opens the file in edit mode once.
  const autoEdit = useRef(hash === "#edit");
  useEffect(() => {
    const f = file.data;
    if (!autoEdit.current || !f || f.hash === null || f.read_only) return;
    autoEdit.current = false;
    edit.start();
  }, [file.data, edit]);

  // A finished save leaves edit mode with a note, the way the design's Save does.
  const wasSaving = useRef(false);
  useEffect(() => {
    if (wasSaving.current && !edit.saving && !edit.dirty && !edit.conflict && !edit.error && edit.editing) {
      edit.discard();
      setSaved(`Saved ${path.split("/").pop()}`);
    }
    wasSaving.current = edit.saving;
  }, [edit, path]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: the lines render only once the file loads.
  useEffect(() => {
    if (view !== "code" || !hl) return;
    const id = requestAnimationFrame(() =>
      linesRef.current?.querySelector(`[data-line="${hl}"]`)?.scrollIntoView?.({ block: "center" }),
    );
    return () => cancelAnimationFrame(id);
  }, [hl, view, file.data]);

  const f = file.data;
  const d = diff.data && diff.data.hunks.length > 0 ? diff.data : null;
  const rows = useMemo(() => (d ? diffRows(d.hunks) : []), [d]);
  const lines = useMemo(() => (f?.content ? splitLines(f.content) : []), [f?.content]);
  const symbols = code.data?.path === path ? code.data.symbols : [];

  if (!f)
    return (
      <div className="mp-pad">
        {file.error ? (
          <span className="mp-error">{file.error.message}</span>
        ) : (
          <span className="m-muted" style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <Spinner size={11} /> Reading the file…
          </span>
        )}
      </div>
    );

  const mark = f.git ? GIT_MARK[f.git] : null;
  const locked =
    f.hash === null
      ? f.truncated
        ? "Read-only: larger than the browser size cap"
        : f.binary
          ? "Read-only: binary"
          : "Read-only: not UTF-8 text"
      : f.read_only;
  const tabs: { id: View; label: string; count?: number }[] = [
    { id: "code", label: "Code" },
    ...(d ? [{ id: "diff" as const, label: "Changes" }] : []),
    { id: "outline", label: "Outline", count: symbols.length },
  ];
  const shown: View = tabs.some((t) => t.id === view) ? view : "code";

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <div style={{ padding: "0 16px", display: "flex", gap: 8, alignItems: "center", minHeight: 36 }}>
        <Chip mono>{code.data?.language ?? (extOf(path) || "text")}</Chip>
        {mark && (
          <span className="mp-mark" style={{ color: mark.color }} title={mark.word}>
            {mark.letter}
          </span>
        )}
        <span style={{ flex: 1, fontSize: 12, color: "var(--text-muted)" }}>
          {f.binary ? "Binary" : `${lines.length} ${lines.length === 1 ? "line" : "lines"}`}
        </span>
        {!edit.editing &&
          (locked ? (
            <span
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                fontSize: 12,
                color: "var(--text-muted)",
                minWidth: 0,
              }}
            >
              <Icon name="lock" size={13} style={{ flex: "none" }} />
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{locked}</span>
            </span>
          ) : (
            <button
              type="button"
              className="m-btn m-btn-sm"
              onClick={() => {
                setSaved(null);
                edit.start();
              }}
            >
              <Icon name="pencil" size={14} />
              Edit
            </button>
          ))}
      </div>
      {f.truncated && (
        <span className="m-help" style={{ padding: "0 16px" }}>
          Only the start of this file is shown, because it is larger than the size Ostra reads for the browser.
        </span>
      )}
      {!edit.editing && (
        <>
          <div className="mp-ftabs" role="tablist" aria-label="View">
            {tabs.map((t) => (
              <button type="button" role="tab" key={t.id} aria-selected={shown === t.id} onClick={() => setView(t.id)}>
                {t.label}
                {t.count ? <span className="mp-count">{t.count}</span> : null}
              </button>
            ))}
          </div>
          {shown === "code" &&
            (f.binary ? (
              <span className="m-muted" style={{ padding: "0 16px" }}>
                This file is binary, so Ostra does not show its text.
              </span>
            ) : lines.length === 0 || (lines.length === 1 && lines[0] === "") ? (
              <span className="m-muted" style={{ padding: "0 16px" }}>
                This file is empty.
              </span>
            ) : (
              <div className="mp-code">
                <div className="mp-lines" ref={linesRef}>
                  {lines.map((t, i) => {
                    const n = i + 1;
                    const on = hl === n;
                    return (
                      <span key={n} style={{ display: "contents" }}>
                        <span data-line={n} className={`mp-ln${on ? " mp-hl" : ""}`}>
                          {n}
                        </span>
                        <span className={`mp-lt${on ? " mp-hl" : ""}`}>{t || " "}</span>
                      </span>
                    );
                  })}
                </div>
              </div>
            ))}
          {shown === "diff" && d && (
            <>
              <span style={{ padding: "0 16px", fontSize: 12, color: "var(--text-muted)" }}>
                Against {d.base} · {d.added} added, {d.removed} removed
              </span>
              <div className="mp-code">
                <div className="mp-diff">
                  {rows.map((r, i) =>
                    r.type === "gap" ? (
                      <div key={i} className="mp-gap">
                        {r.skipped ? `${r.skipped} unchanged lines · ` : ""}
                        {r.header}
                      </div>
                    ) : (
                      <div
                        key={i}
                        className={`mp-drow${r.type === "add" ? " mp-add" : r.type === "del" ? " mp-del" : ""}`}
                      >
                        <span>{r.old ?? ""}</span>
                        <span>{r.new ?? ""}</span>
                        <span>{r.type === "add" ? "+" : r.type === "del" ? "−" : " "}</span>
                        <span>{r.text || " "}</span>
                      </div>
                    ),
                  )}
                </div>
              </div>
              {d.truncated && (
                <span className="m-help" style={{ padding: "0 16px" }}>
                  The diff was cut at the size cap.
                </span>
              )}
            </>
          )}
          {shown === "outline" && (
            <div style={{ display: "flex", flexDirection: "column" }}>
              {symbols.map((s) => (
                <button
                  type="button"
                  key={`${s.line}:${s.col}:${s.name}`}
                  className="mp-node-btn"
                  style={{ minHeight: 44, gap: 10, borderBottom: "1px solid var(--border-subtle)" }}
                  onClick={() => {
                    setHl(s.line);
                    setView("code");
                  }}
                >
                  <span
                    style={{
                      width: 64,
                      flex: "none",
                      fontFamily: "var(--font-mono)",
                      fontSize: 11,
                      color: "var(--text-muted)",
                    }}
                  >
                    {s.kind}
                  </span>
                  <span className="mp-node-name" style={{ flex: 1 }}>
                    {s.container ? `${s.container}::` : ""}
                    {s.name}
                  </span>
                  <span style={{ fontFamily: "var(--font-mono)", fontSize: 11, color: "var(--text-muted)" }}>
                    {s.line}
                  </span>
                </button>
              ))}
              {symbols.length === 0 && (
                <span className="m-muted" style={{ padding: "8px 16px" }}>
                  {code.loading ? "Reading the symbols…" : "No symbols found."}
                </span>
              )}
            </div>
          )}
        </>
      )}
      {edit.editing && (
        <>
          {edit.conflict && (
            <div className="mp-callout" style={{ margin: "0 16px" }}>
              <span style={{ fontSize: 13.5, lineHeight: 1.5, textWrap: "pretty" }}>
                This file changed on disk since you started editing.
              </span>
              <div className="mp-tools">
                <button
                  type="button"
                  className="m-btn"
                  style={{ flex: 1, background: "var(--surface-raised)" }}
                  onClick={edit.reload}
                >
                  Reload
                </button>
                <button type="button" className="m-btn m-btn-primary" style={{ flex: 1 }} onClick={edit.overwrite}>
                  Overwrite
                </button>
              </div>
            </div>
          )}
          {f.read_only && (
            <span className="m-help" style={{ padding: "0 16px" }}>
              {f.read_only}
            </span>
          )}
          <div style={{ padding: "0 16px" }}>
            <textarea
              className="mp-editor"
              aria-label={`Edit ${path}`}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              rows={22}
              value={edit.draft}
              onChange={(e) => edit.change(e.target.value)}
            />
          </div>
          {edit.error && (
            <span className="mp-error" style={{ padding: "0 16px" }}>
              {edit.error}
            </span>
          )}
          <div style={{ padding: "0 16px", display: "flex", gap: 8 }}>
            <button type="button" className="m-btn" onClick={edit.discard}>
              {!edit.dirty ? "Done" : edit.confirmDiscard ? "Discard changes?" : "Discard"}
            </button>
            <button
              type="button"
              className="m-btn m-btn-primary"
              style={{ flex: 1 }}
              disabled={!edit.dirty || edit.saving || edit.conflict || !!f.read_only}
              onClick={edit.save}
            >
              {edit.saving ? "Saving…" : "Save"}
            </button>
          </div>
        </>
      )}
      {saved && !edit.editing && (
        <span
          style={{
            padding: "0 16px",
            display: "flex",
            gap: 6,
            alignItems: "center",
            fontSize: 12.5,
            color: "var(--ok)",
          }}
        >
          <Icon name="check" size={14} />
          {saved}
        </span>
      )}
    </div>
  );
}
