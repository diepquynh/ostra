import { Banner, Button } from "@ostra/design";
import { useSessionSummaries } from "../lib/live";
import { useNav, useShell, useWorkspace } from "../lib/nav";
import { NewTask } from "./workspace/NewTask";
import { LoadError, Loading, Page } from "./workspace/Page";
import { SessionsTable } from "./workspace/SessionsTable";

export type WorkspaceScreenProps = {
  /** Workspace id. */
  ws: string;
};

/**
 * Resource `ws:overview`: the New task form and the sessions table. Reads the "Turn into task" text from
 * `useShell().taskDraft` and clears it with `setTaskDraft(null)` once used.
 */
export function WorkspaceScreen({ ws }: WorkspaceScreenProps) {
  const { detail, reload } = useWorkspace();
  const { open } = useNav();
  const { addProject } = useShell();
  const sessions = useSessionSummaries(ws);

  if (!detail)
    return (
      <Page title="Workspace">
        <Loading>Reading the workspace…</Loading>
      </Page>
    );
  const uninitialized = detail.projects.filter((p) => p.init_status === "not_initialized");
  const count = detail.projects.length;

  return (
    <Page
      title={detail.settings.name}
      sub={`${detail.root} · ${count} project${count === 1 ? "" : "s"}`}
      actions={
        <Button size="sm" variant="ghost" icon="folder-plus" onClick={addProject}>
          Add project
        </Button>
      }
    >
      {detail.validation.length > 0 && (
        <Banner
          tone="bad"
          actions={
            <Button size="sm" onClick={() => open("ws:settings")}>
              Open settings
            </Button>
          }
        >
          Settings have {detail.validation.length} problem{detail.validation.length === 1 ? "" : "s"}. Fix them in
          Settings before starting work, because an agent whose route does not resolve cannot start.
        </Banner>
      )}
      {count === 0 && (
        <Banner
          tone="info"
          actions={
            <Button size="sm" icon="folder-plus" onClick={addProject}>
              Add project
            </Button>
          }
        >
          This workspace has no projects yet. Add one to start a task.
        </Banner>
      )}
      {uninitialized.length > 0 && (
        <Banner
          tone="warn"
          actions={
            <>
              {uninitialized.slice(0, 3).map((p) => (
                <Button key={p.key} size="sm" onClick={() => open(`project:${p.key}`)}>
                  Initialize {p.key}
                </Button>
              ))}
              <Button size="sm" variant="ghost" icon="refresh-ccw" onClick={reload}>
                Check again
              </Button>
            </>
          }
        >
          {uninitialized.map((p) => p.key).join(", ")} {uninitialized.length === 1 ? "is" : "are"} not initialized.
          Pipeline tasks cannot target an uninitialized project.
        </Banner>
      )}
      <NewTask
        ws={ws}
        projects={detail.projects}
        yoloDefault={detail.settings.yolo.default}
        onCreated={(s) => {
          sessions.reload();
          open(`session:${s.id}`);
        }}
      />
      {sessions.error && <LoadError error={sessions.error} onRetry={sessions.reload} />}
      <SessionsTable
        sessions={sessions.sessions}
        projects={detail.projects.map((p) => p.key)}
        loading={sessions.loading}
      />
    </Page>
  );
}
