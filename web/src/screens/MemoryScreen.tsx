import { useEffect, useRef, useState } from "react";
import { useLocation } from "react-router";
import { api } from "../api";
import type { Lesson } from "../api/types";
import { Banner, Button, Dialog, IconButton, Input, Panel, Select, Table, Tabs, type TableColumn } from "../design";
import { formatTime } from "../lib/format";
import { useAsync } from "../lib/hooks";
import { useShell, useWorkspace } from "../lib/nav";
import { flash, LoadError, Loading, Page, useAfterPaint, useAnchor } from "./workspace/Page";

export type MemoryScreenProps = { ws: string };

/** A lesson a deep link names: `lesson:<project key>:<id>`. Project keys never contain a colon. */
export function parseLessonAnchor(anchor: string | null): { project: string; id: number } | null {
  const m = anchor ? /^lesson:([^:]+):(\d+)$/.exec(anchor) : null;
  return m ? { project: m[1], id: Number(m[2]) } : null;
}

/** The project a `?project=<key>` query or a `project:<key>` anchor names. */
function projectFromLocation(search: string, anchor: string | null): string | null {
  const q = new URLSearchParams(search).get("project");
  if (q) return q;
  if (anchor?.startsWith("project:")) return anchor.slice(8) || null;
  return parseLessonAnchor(anchor)?.project ?? null;
}

type Editing = { mode: "add" } | { mode: "edit"; lesson: Lesson } | { mode: "delete"; lesson: Lesson } | null;

const SEARCH_DELAY_MS = 250;
const MAX_TABS = 6;

/**
 * Resource `ws:memory`: lessons per project, searchable and editable. `?project=<key>` preselects a project;
 * a `#lesson:<key>:<id>` anchor (from ⌘K) selects that lesson and scrolls to it.
 */
export function MemoryScreen({ ws }: MemoryScreenProps) {
  const { detail } = useWorkspace();
  const { addProject } = useShell();
  const { search } = useLocation();
  const { anchor, nonce } = useAnchor();
  const projects = detail?.projects ?? [];

  const [picked, setPicked] = useState<string | null>(() => projectFromLocation(search, anchor));
  const project = picked && projects.some((p) => p.key === picked) ? picked : (projects[0]?.key ?? null);
  const [query, setQuery] = useState("");
  const [debounced, setDebounced] = useState("");
  const [selected, setSelected] = useState<number | null>(() => parseLessonAnchor(anchor)?.id ?? null);
  const [editing, setEditing] = useState<Editing>(null);
  const [error, setError] = useState<string | null>(null);
  const scroller = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const t = setTimeout(() => setDebounced(query.trim()), SEARCH_DELAY_MS);
    return () => clearTimeout(t);
  }, [query]);

  useEffect(() => {
    const target = parseLessonAnchor(anchor);
    const p = projectFromLocation(search, anchor);
    if (p) setPicked(p);
    if (target) {
      setSelected(target.id);
      setQuery("");
      setDebounced("");
    }
  }, [anchor, nonce, search]);

  const lessons = useAsync(() => (project ? api.lessons(ws, project, debounced || undefined) : Promise.resolve([])), [ws, project, debounced]);
  const rows = lessons.data ?? [];

  // Ring the selected row once it is on screen.
  const flashed = useRef<string | null>(null);
  useAfterPaint(() => {
    if (selected === null || !lessons.data) return;
    const k = `${project}:${selected}:${nonce}`;
    if (flashed.current === k) return;
    const row = scroller.current?.querySelector(".os-row--selected");
    if (row) {
      flashed.current = k;
      flash(row);
    }
  }, [selected, lessons.data, nonce, project]);

  const run = async (fn: () => Promise<unknown>) => {
    setError(null);
    try {
      await fn();
      setEditing(null);
      lessons.reload();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const columns: TableColumn<Lesson>[] = [
    { key: "area", label: "Area", width: 220, render: (l) => <span className="wp-mono">{l.area}</span> },
    { key: "lesson", label: "Lesson", render: (l) => <span className="wp-lesson">{l.lesson}</span> },
    {
      key: "source",
      label: "Recorded by",
      width: 150,
      render: (l) => (
        <span className="wp-stage">
          <span className="wp-mono" style={{ color: "var(--text-secondary)" }}>
            {l.source}
          </span>
          <span style={{ color: "var(--text-muted)", fontSize: "var(--text-xs)" }}>{formatTime(l.created_at)}</span>
        </span>
      ),
    },
    {
      key: "actions",
      label: "",
      width: 64,
      render: (l) => (
        <span className="wp-row" style={{ gap: 2, flexWrap: "nowrap" }} onClick={(e) => e.stopPropagation()}>
          <IconButton size="sm" icon="pencil" label="Edit this lesson" onClick={() => setEditing({ mode: "edit", lesson: l })} />
          <IconButton size="sm" icon="x" label="Delete this lesson" onClick={() => setEditing({ mode: "delete", lesson: l })} />
        </span>
      ),
    },
  ];

  if (!detail) {
    return (
      <Page title="Memory">
        <Loading>Reading the projects…</Loading>
      </Page>
    );
  }

  return (
    <Page
      title="Memory"
      actions={
        project && (
          <Button size="sm" icon="plus" onClick={() => setEditing({ mode: "add" })}>
            Add a lesson
          </Button>
        )
      }
    >
      <p className="wp-lead">
        Lessons are project facts that agents record when something cost real effort to find out: a constraint the code does not state, a
        version-specific API detail, a workaround. Agents recall them before working in an area and again after a failure. Edit or delete any
        lesson that is wrong or stale, because agents act on what they recall.
      </p>
      {projects.length === 0 ? (
        <Banner tone="info" actions={<Button size="sm" icon="folder-plus" onClick={addProject}>Add project</Button>}>
          This workspace has no projects yet. Lessons belong to a project, so add one first.
        </Banner>
      ) : (
        <>
          <div className="wp-row">
            {projects.length <= MAX_TABS ? (
              <Tabs
                variant="segmented"
                label="Project"
                value={project ?? ""}
                onChange={(k) => {
                  setPicked(k);
                  setSelected(null);
                }}
                tabs={projects.map((p) => ({ id: p.key, label: p.key }))}
              />
            ) : (
              <Select
                size="sm"
                aria-label="Project"
                value={project ?? ""}
                onChange={(e) => {
                  setPicked(e.target.value);
                  setSelected(null);
                }}
                options={projects.map((p) => p.key)}
              />
            )}
            <span className="wp-spacer" />
            <Input
              size="sm"
              icon="search"
              type="search"
              aria-label="Search lessons"
              placeholder="Search lessons"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              style={{ width: 260 }}
            />
          </div>
          {error && <Banner tone="bad">{error}</Banner>}
          {lessons.error && <LoadError error={lessons.error} onRetry={lessons.reload} />}
          <div ref={scroller}>
            <Panel bodyFlush>
              <Table<Lesson>
                columns={columns}
                rows={rows}
                selectedKey={selected ?? undefined}
                onRowClick={(l) => setSelected(l.id)}
                empty={
                  lessons.loading && !lessons.data ? (
                    "Reading the lessons…"
                  ) : debounced ? (
                    <span className="wp-row" style={{ justifyContent: "center" }}>
                      No lessons match “{debounced}”.
                      <Button size="sm" variant="ghost" icon="x" onClick={() => setQuery("")}>
                        Clear the search
                      </Button>
                    </span>
                  ) : (
                    <span className="wp-row" style={{ justifyContent: "center" }}>
                      No lessons recorded for {project} yet. Agents add them as they work, or add one yourself.
                      <Button size="sm" icon="plus" onClick={() => setEditing({ mode: "add" })}>
                        Add a lesson
                      </Button>
                    </span>
                  )
                }
              />
            </Panel>
          </div>
        </>
      )}
      {(editing?.mode === "add" || editing?.mode === "edit") && project && (
        <LessonDialog
          project={project}
          lesson={editing.mode === "edit" ? editing.lesson : null}
          error={error}
          onClose={() => {
            setEditing(null);
            setError(null);
          }}
          onSave={(area, text) =>
            run(async () => {
              const saved = await api.saveLesson(ws, project, { id: editing.mode === "edit" ? editing.lesson.id : null, area, lesson: text });
              setSelected(saved.id);
            })
          }
        />
      )}
      {editing?.mode === "delete" && project && (
        <Dialog
          title="Delete this lesson?"
          width={480}
          onClose={() => setEditing(null)}
          footer={
            <>
              <span className="wp-spacer" />
              <Button onClick={() => setEditing(null)}>Keep it</Button>
              <Button variant="danger" onClick={() => void run(() => api.deleteLesson(ws, project, editing.lesson.id))}>
                Delete the lesson
              </Button>
            </>
          }
        >
          <div className="wp-stack">
            <span style={{ color: "var(--text-secondary)" }}>Later sessions in {project} will no longer recall it.</span>
            <div className="wp-stack" style={{ gap: 4, padding: "8px 10px", border: "1px solid var(--border-subtle)", borderRadius: "var(--radius-sm)" }}>
              <span className="wp-mono" style={{ color: "var(--text-muted)" }}>
                {editing.lesson.area}
              </span>
              <span>{editing.lesson.lesson}</span>
            </div>
            {error && <Banner tone="bad">{error}</Banner>}
          </div>
        </Dialog>
      )}
    </Page>
  );
}

function LessonDialog({
  project,
  lesson,
  error,
  onClose,
  onSave,
}: {
  project: string;
  lesson: Lesson | null;
  error: string | null;
  onClose: () => void;
  onSave: (area: string, text: string) => Promise<void>;
}) {
  const [area, setArea] = useState(lesson?.area ?? "");
  const [text, setText] = useState(lesson?.lesson ?? "");
  const [busy, setBusy] = useState(false);
  const ok = area.trim() !== "" && text.trim() !== "" && !busy;
  const save = async () => {
    if (!ok) return;
    setBusy(true);
    await onSave(area.trim(), text.trim());
    setBusy(false);
  };
  return (
    <Dialog
      title={lesson ? "Edit the lesson" : "Add a lesson"}
      subtitle={project}
      width={560}
      onClose={onClose}
      footer={
        <>
          <span className="wp-spacer" />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!ok} onClick={() => void save()}>
            Save the lesson
          </Button>
        </>
      }
    >
      <div className="wp-stack">
        <Input
          label="Area"
          mono
          placeholder="order::OrderService"
          hint="The module, file, or topic the lesson applies to. Agents recall lessons by area."
          value={area}
          onChange={(e) => setArea(e.target.value)}
        />
        <Input
          label="Lesson"
          multiline
          rows={4}
          placeholder="Status changes must go through OrderStateMachine; setStatus skips the event publisher."
          hint="One fact, stated so an agent can act on it."
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        {error && <Banner tone="bad">{error}</Banner>}
      </div>
    </Dialog>
  );
}
