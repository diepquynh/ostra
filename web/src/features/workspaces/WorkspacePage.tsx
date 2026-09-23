import { useParams } from "react-router";
import { api } from "../../api";
import { useCrumbs, useLayout } from "../../components/Layout";
import { ErrorBox, Loading } from "../../components/Status";
import { useAsync, useChannel } from "../../lib/hooks";
import { SessionList } from "../sessions/SessionList";
import { NewTaskForm } from "./NewTaskForm";
import { WorkspaceTabs } from "./WorkspaceTabs";

export function WorkspacePage() {
  const { ws = "" } = useParams();
  const detail = useAsync(() => api.workspace(ws), [ws]);
  const sessions = useAsync(() => api.sessions(ws), [ws]);
  const { taskDraft, setTaskDraft } = useLayout();
  useCrumbs([{ label: detail.data?.settings.name ?? "Workspace", to: `/w/${ws}` }]);
  useChannel(`workspace:${ws}`, (m) => {
    if (m.type === "session_updated") {
      const list = sessions.data ?? [];
      const i = list.findIndex((s) => s.id === m.summary.id);
      if (i >= 0) sessions.set(list.map((s, j) => (j === i ? m.summary : s)));
      else sessions.set([m.summary, ...list]);
    }
    if (m.type === "workspace_updated") detail.reload();
  });

  if (detail.error) return <ErrorBox error={detail.error} onRetry={detail.reload} />;
  if (!detail.data) return <Loading />;
  const d = detail.data;
  const uninitialized = d.projects.filter((p) => p.init_status === "not_initialized");

  return (
    <div className="page">
      <div className="row between">
        <div>
          <h1>{d.settings.name}</h1>
          <div className="mono small muted">{d.root}</div>
        </div>
      </div>
      <WorkspaceTabs ws={ws} />

      {d.validation.length > 0 && (
        <div className="banner bad">
          Settings have {d.validation.length} problem{d.validation.length === 1 ? "" : "s"}. Open Settings to fix them before starting
          work.
        </div>
      )}
      {d.projects.length === 0 && <div className="banner info">This workspace has no projects yet. Import one under Projects.</div>}
      {uninitialized.length > 0 && (
        <div className="banner">
          {uninitialized.map((p) => p.key).join(", ")} {uninitialized.length === 1 ? "is" : "are"} not initialized. Pipeline tasks
          cannot target an uninitialized project; initialize it under Projects.
        </div>
      )}

      <NewTaskForm
        ws={ws}
        projects={d.projects}
        yoloDefault={d.settings.yolo.default}
        draft={taskDraft}
        onDraftUsed={() => setTaskDraft(null)}
        onCreated={(s) => sessions.set([s, ...(sessions.data ?? [])])}
      />

      <div className="section-title">Sessions</div>
      {sessions.error && <ErrorBox error={sessions.error} onRetry={sessions.reload} />}
      {sessions.data ? <SessionList ws={ws} sessions={sessions.data} /> : <Loading />}
    </div>
  );
}
