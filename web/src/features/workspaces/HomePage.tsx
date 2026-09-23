import { useState } from "react";
import { Link } from "react-router";
import { api } from "../../api";
import { useCrumbs } from "../../components/Layout";
import { Chip, ErrorBox, Loading } from "../../components/Status";
import { useAsync, useChannel } from "../../lib/hooks";
import { NewWorkspaceWizard } from "./NewWorkspaceWizard";

export function HomePage() {
  useCrumbs([]);
  const list = useAsync(() => api.workspaces(), []);
  const [creating, setCreating] = useState(false);
  useChannel("home", (m) => {
    if (m.type === "workspace_updated" || m.type === "session_updated") list.reload();
  });

  return (
    <div className="page narrow">
      <div className="row between mb">
        <div>
          <h1>Workspaces</h1>
          <p className="muted">
            A workspace holds your projects and their settings: which executor and model each agent runs on, permissions, memory,
            and custom instructions.
          </p>
        </div>
        {!creating && (
          <button className="primary" onClick={() => setCreating(true)}>
            New workspace
          </button>
        )}
      </div>

      {creating && <NewWorkspaceWizard onCancel={() => setCreating(false)} />}

      {list.error && <ErrorBox error={list.error} onRetry={list.reload} />}
      {list.loading && !list.data && <Loading />}
      {list.data && list.data.length === 0 && !creating && (
        <div className="card empty">
          <p>No workspaces yet.</p>
          <p className="small">Create one, then import the project folders you want Ostra to work on.</p>
        </div>
      )}
      {list.data?.map((w) => (
        <div className="card" key={w.id}>
          <div className="row between">
            <div>
              <h2>{w.available ? <Link to={`/w/${w.id}`}>{w.name}</Link> : w.name}</h2>
              <div className="mono small muted">{w.root}</div>
            </div>
            <div className="row">
              {!w.available && <Chip tone="bad">Folder missing</Chip>}
              <Chip>
                {w.projects} project{w.projects === 1 ? "" : "s"}
              </Chip>
              {w.active_sessions > 0 && (
                <Chip tone="accent">
                  {w.active_sessions} active session{w.active_sessions === 1 ? "" : "s"}
                </Chip>
              )}
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}
