import { useState } from "react";
import { useNavigate } from "react-router";
import { api } from "../../api";
import type { WorkspaceDetail } from "../../api/types";
import { FolderPicker } from "../../components/FolderPicker";
import { Chip } from "../../components/Status";
import { ImportProjectForm } from "./ImportProjectForm";

type Step = "name" | "check" | "projects";

export function EnvironmentCheck({ detail }: { detail: WorkspaceDetail }) {
  const anyKey = detail.providers.some((p) => p.has_key);
  return (
    <div className="stack">
      {!anyKey && (
        <div className="banner bad">
          No model provider has a usable API key. Set <code>ANTHROPIC_API_KEY</code> or <code>OPENAI_API_KEY</code> in the environment
          that starts <code>ostra</code>, or configure a keychain entry in <code>~/.config/ostra/config.toml</code>, then restart it.
        </div>
      )}
      <table>
        <thead>
          <tr>
            <th>Provider</th>
            <th>API key</th>
            <th>Source</th>
          </tr>
        </thead>
        <tbody>
          {detail.providers.map((p) => (
            <tr key={p.name}>
              <td>{p.name}</td>
              <td>{p.has_key ? <Chip tone="ok">Usable</Chip> : <Chip tone="warn">Missing</Chip>}</td>
              <td className="mono small">{p.source}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <table>
        <thead>
          <tr>
            <th>Harness</th>
            <th>Installed</th>
            <th>Logged in</th>
            <th>Version</th>
          </tr>
        </thead>
        <tbody>
          {detail.harnesses.map((h) => (
            <tr key={h.harness}>
              <td>
                {h.harness} <span className="mono small muted">{h.command}</span>
              </td>
              <td>{h.installed ? <Chip tone="ok">Yes</Chip> : <Chip>No</Chip>}</td>
              <td>
                {h.logged_in === null ? <Chip>Unknown</Chip> : h.logged_in ? <Chip tone="ok">Yes</Chip> : <Chip tone="warn">No</Chip>}
              </td>
              <td className="mono small">{h.version ?? ""}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="muted small">
        Agents run on the native executor unless workspace settings route them to an installed harness. A harness that is not
        installed cannot be selected.
      </p>
    </div>
  );
}

export function NewWorkspaceWizard({ onCancel }: { onCancel: () => void }) {
  const [step, setStep] = useState<Step>("name");
  const [name, setName] = useState("");
  const [root, setRoot] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [detail, setDetail] = useState<WorkspaceDetail | null>(null);
  const navigate = useNavigate();

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const d = await api.createWorkspace({ name: name.trim(), root: root.trim() });
      setDetail(d);
      setStep("check");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card highlight mb">
      <div className="row between">
        <h2>New workspace</h2>
        <span className="muted small">
          Step {step === "name" ? 1 : step === "check" ? 2 : 3} of 3
        </span>
      </div>

      {step === "name" && (
        <div className="stack">
          <label className="field">
            <span className="label">Name</span>
            <input value={name} onChange={(e) => setName(e.target.value)} placeholder="shop" autoFocus />
          </label>
          <label className="field">
            <span className="label">Directory</span>
            <span>
              Ostra writes <code>.ostra/</code> here: workspace settings, the session database, and session artifacts. Projects are
              referenced by path and never copied.
            </span>
          </label>
          <FolderPicker value={root} onChange={setRoot} />
          {error && <div className="form-error">{error}</div>}
          <div className="row">
            <button className="primary" disabled={busy || !name.trim() || !root.trim().startsWith("/")} onClick={() => void create()}>
              Create workspace
            </button>
            <button onClick={onCancel}>Cancel</button>
          </div>
        </div>
      )}

      {step === "check" && detail && (
        <div className="stack">
          <p>Workspace created. Here is what Ostra found on this machine.</p>
          <EnvironmentCheck detail={detail} />
          <div className="row">
            <button className="primary" onClick={() => setStep("projects")}>
              Next: add projects
            </button>
          </div>
        </div>
      )}

      {step === "projects" && detail && (
        <div className="stack">
          <p>
            Import existing folders. A new project must be created by hand first, then imported. A workspace with no projects is
            valid; you can add them later from its page.
          </p>
          <ImportProjectForm ws={detail.id} existing={detail.projects} onImported={setDetail} />
          {detail.projects.length > 0 && (
            <ul>
              {detail.projects.map((p) => (
                <li key={p.key}>
                  <strong>{p.key}</strong> <span className="mono small muted">{p.path}</span>
                </li>
              ))}
            </ul>
          )}
          <div className="row">
            <button className="primary" onClick={() => navigate(`/w/${detail.id}`)}>
              Open workspace
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
