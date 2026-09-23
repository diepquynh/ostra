import { useEffect, useState } from "react";
import { useParams, useSearchParams } from "react-router";
import { api } from "../../api";
import type { Lesson } from "../../api/types";
import { useCrumbs } from "../../components/Layout";
import { ErrorBox, Loading } from "../../components/Status";
import { formatTime } from "../../lib/format";
import { useAsync } from "../../lib/hooks";
import { WorkspaceTabs } from "../workspaces/WorkspaceTabs";

function LessonRow({ lesson, onSave, onDelete }: { lesson: Lesson; onSave: (area: string, text: string) => Promise<void>; onDelete: () => Promise<void> }) {
  const [editing, setEditing] = useState(false);
  const [area, setArea] = useState(lesson.area);
  const [text, setText] = useState(lesson.lesson);
  if (editing)
    return (
      <tr>
        <td>
          <input value={area} onChange={(e) => setArea(e.target.value)} />
        </td>
        <td>
          <textarea rows={2} value={text} onChange={(e) => setText(e.target.value)} />
        </td>
        <td colSpan={2}>
          <div className="row">
            <button
              className="small primary"
              disabled={!area.trim() || !text.trim()}
              onClick={async () => {
                await onSave(area.trim(), text.trim());
                setEditing(false);
              }}
            >
              Save
            </button>
            <button className="small" onClick={() => setEditing(false)}>
              Cancel
            </button>
          </div>
        </td>
      </tr>
    );
  return (
    <tr>
      <td className="mono small">{lesson.area}</td>
      <td>{lesson.lesson}</td>
      <td className="small muted nowrap">
        {lesson.source}
        <div>{formatTime(lesson.created_at)}</div>
      </td>
      <td>
        <div className="row" style={{ flexWrap: "nowrap" }}>
          <button className="small" onClick={() => setEditing(true)}>
            Edit
          </button>
          <button
            className="small danger"
            onClick={() => {
              if (confirm("Delete this lesson? Later sessions will no longer recall it.")) void onDelete();
            }}
          >
            Delete
          </button>
        </div>
      </td>
    </tr>
  );
}

export function MemoryPage() {
  const { ws = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const detail = useAsync(() => api.workspace(ws), [ws]);
  const project = params.get("project") ?? detail.data?.projects[0]?.key ?? "";
  const [query, setQuery] = useState("");
  const [debounced, setDebounced] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [newArea, setNewArea] = useState("");
  const [newText, setNewText] = useState("");
  useEffect(() => {
    const t = setTimeout(() => setDebounced(query), 250);
    return () => clearTimeout(t);
  }, [query]);
  const lessons = useAsync(() => (project ? api.lessons(ws, project, debounced || undefined) : Promise.resolve([])), [ws, project, debounced]);
  useCrumbs([{ label: detail.data?.settings.name ?? "Workspace", to: `/w/${ws}` }, { label: "Memory" }]);

  if (detail.error) return <ErrorBox error={detail.error} onRetry={detail.reload} />;
  if (!detail.data) return <Loading />;

  const run = async (fn: () => Promise<unknown>) => {
    setError(null);
    try {
      await fn();
      lessons.reload();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <div className="page">
      <h1>{detail.data.settings.name}</h1>
      <WorkspaceTabs ws={ws} />
      <p className="muted small">
        Lessons are durable, project-scoped facts that agents record when something cost real effort to find out: a constraint the code
        does not state, a version-specific API detail, a workaround. Agents recall them before working in an area and again after a
        failure. Edit or delete any lesson that is wrong or stale.
      </p>
      <div className="row mb">
        <select value={project} onChange={(e) => setParams({ project: e.target.value })} style={{ width: "auto" }}>
          {detail.data.projects.map((p) => (
            <option key={p.key} value={p.key}>
              {p.key}
            </option>
          ))}
        </select>
        <input type="search" placeholder="Search lessons" value={query} onChange={(e) => setQuery(e.target.value)} style={{ maxWidth: 360 }} />
        <span className="spacer" />
        <button onClick={() => setAdding(!adding)} disabled={!project}>
          Add a lesson
        </button>
      </div>
      {error && <div className="banner bad">{error}</div>}
      {adding && (
        <div className="card mb">
          <div className="grid-2">
            <label className="field">
              <span className="label">Area</span>
              <input value={newArea} onChange={(e) => setNewArea(e.target.value)} placeholder="orders::OrderService" />
            </label>
            <label className="field">
              <span className="label">Lesson</span>
              <textarea rows={2} value={newText} onChange={(e) => setNewText(e.target.value)} />
            </label>
          </div>
          <button
            className="primary mt"
            disabled={!newArea.trim() || !newText.trim()}
            onClick={() =>
              void run(async () => {
                await api.saveLesson(ws, project, { id: null, area: newArea.trim(), lesson: newText.trim() });
                setNewArea("");
                setNewText("");
                setAdding(false);
              })
            }
          >
            Save lesson
          </button>
        </div>
      )}
      {detail.data.projects.length === 0 && <div className="card empty">No projects in this workspace.</div>}
      {lessons.error && <ErrorBox error={lessons.error} onRetry={lessons.reload} />}
      {lessons.data && (
        <div className="card" style={{ padding: 0 }}>
          <table>
            <thead>
              <tr>
                <th>Area</th>
                <th>Lesson</th>
                <th>Source</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {lessons.data.map((l) => (
                <LessonRow
                  key={l.id}
                  lesson={l}
                  onSave={(area, lesson) => run(() => api.saveLesson(ws, project, { id: l.id, area, lesson }))}
                  onDelete={() => run(() => api.deleteLesson(ws, project, l.id))}
                />
              ))}
            </tbody>
          </table>
          {lessons.data.length === 0 && <div className="empty">{debounced ? "No lessons match." : "No lessons recorded yet."}</div>}
        </div>
      )}
    </div>
  );
}
