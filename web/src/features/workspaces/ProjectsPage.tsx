import { useState } from "react";
import { useNavigate, useParams } from "react-router";
import { api } from "../../api";
import { useCrumbs } from "../../components/Layout";
import { Chip, ErrorBox, InitStatusChip, Loading } from "../../components/Status";
import { useAsync, useChannel } from "../../lib/hooks";
import { ImportProjectForm } from "./ImportProjectForm";
import { EnvironmentCheck } from "./NewWorkspaceWizard";
import { WorkspaceTabs } from "./WorkspaceTabs";

export function ProjectsPage() {
  const { ws = "" } = useParams();
  const detail = useAsync(() => api.workspace(ws), [ws]);
  const [error, setError] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const navigate = useNavigate();
  useCrumbs([{ label: detail.data?.settings.name ?? "Workspace", to: `/w/${ws}` }, { label: "Projects" }]);
  useChannel(`workspace:${ws}`, (m) => {
    if (m.type === "workspace_updated") detail.reload();
  });

  if (detail.error) return <ErrorBox error={detail.error} onRetry={detail.reload} />;
  if (!detail.data) return <Loading />;
  const d = detail.data;

  const init = async (key: string) => {
    setError(null);
    try {
      const s = await api.initProject(ws, key);
      navigate(`/w/${ws}/s/${s.id}`);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const remove = async (key: string) => {
    if (!confirm(`Remove ${key} from this workspace? Nothing on disk is deleted.`)) return;
    setError(null);
    try {
      detail.set(await api.removeProject(ws, key));
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <div className="page">
      <h1>{d.settings.name}</h1>
      <WorkspaceTabs ws={ws} />
      {error && <div className="banner bad">{error}</div>}

      <div className="row between">
        <div className="section-title">Projects</div>
        {!importing && <button onClick={() => setImporting(true)}>Import a folder</button>}
      </div>
      {importing && (
        <div className="card highlight mb">
          <ImportProjectForm
            ws={ws}
            existing={d.projects}
            onImported={(next) => {
              detail.set(next);
              setImporting(false);
            }}
          />
          <button className="mt" onClick={() => setImporting(false)}>
            Cancel
          </button>
        </div>
      )}
      {d.projects.length === 0 && <div className="card empty">No projects. Import an existing folder to start.</div>}
      {d.projects.map((p) => (
        <div className="card" key={p.key}>
          <div className="row between">
            <div>
              <h2>{p.key}</h2>
              <div className="mono small muted">{p.path}</div>
            </div>
            <div className="row">
              <InitStatusChip status={p.init_status} />
              {p.is_git ? <Chip>git</Chip> : <Chip tone="warn">not a git checkout</Chip>}
              {p.stack && <Chip>{p.stack}</Chip>}
            </div>
          </div>
          {p.init_status === "not_initialized" && (
            <p className="small mt">
              Initializing scouts the code for recurring patterns, proposes skills for your approval, then writes{" "}
              <code>.ostra/INVENTORY.md</code>, <code>.ostra/project.toml</code>, and the skills every agent loads. No pipeline task can
              target this project until it is done.
            </p>
          )}
          {p.ultracode_bootstrap && (
            <p className="small">
              This folder has an Ultracode bootstrap in <code>.ultracode/</code>. Initializing can reuse its skills.
            </p>
          )}
          {p.profile && (
            <div className="small">
              {p.profile.commands.build && (
                <div>
                  Build: <code>{p.profile.commands.build}</code>
                </div>
              )}
              {p.profile.commands.test && (
                <div>
                  Test: <code>{p.profile.commands.test}</code>
                </div>
              )}
              <div className="muted">
                {p.profile.skills.length} skills · {p.profile.review_rules.length} review rules
              </div>
            </div>
          )}
          <div className="row mt">
            {(p.init_status === "not_initialized" || p.init_status === "initialized") && (
              <button className={p.init_status === "not_initialized" ? "primary" : ""} onClick={() => void init(p.key)}>
                {p.init_status === "initialized" ? "Re-initialize" : "Initialize"}
              </button>
            )}
            <button onClick={() => navigate(`/w/${ws}/memory?project=${p.key}`)}>Memory</button>
            <button className="danger" onClick={() => void remove(p.key)}>
              Remove from workspace
            </button>
          </div>
        </div>
      ))}

      <div className="section-title">This machine</div>
      <div className="card">
        <EnvironmentCheck detail={d} />
      </div>
    </div>
  );
}
