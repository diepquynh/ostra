import { useState } from "react";
import { api, HttpError } from "../../api";
import type { CloneProject, ValidationIssue, WorkspaceDetail } from "../../api/types";
import { Banner, Button, Dialog, Spinner, Tabs } from "../../design";
import { useChannel } from "../../lib/hooks";
import { useWorkspace } from "../../lib/nav";
import { type CloneErrors, CloneForm } from "./CloneForm";
import { useHome } from "./folders";
import { useGitCredentials } from "./GitCredentials";
import { ImportForm } from "./ImportForm";
import { type DraftProject, expandHome, type ImportErrors, importErrors, projectStart } from "./wizard";

export type AddProjectDialogProps = {
  ws: string;
  onClose: () => void;
  /** The project was imported: the updated workspace and the new project's key. */
  onAdded: (detail: WorkspaceDetail, key: string) => void;
};

type Mode = "folder" | "git";

const CLONE_FIELDS = ["url", "key", "path", "stack", "branch", "credential"] as const;

function cloneErrors(issues: ValidationIssue[], message: string): { fields: CloneErrors; general: string | null } {
  const fields: CloneErrors = {};
  const rest: string[] = [];
  for (const i of issues) {
    const f = CLONE_FIELDS.find((x) => x === i.path);
    if (f) fields[f] = fields[f] ? `${fields[f]} ${i.message}` : i.message;
    else rest.push(i.message);
  }
  return { fields, general: rest.length ? rest.join(" ") : issues.length ? null : message };
}

/**
 * Add a project to the active workspace: import a folder like /add-dir, or clone a git repository into the
 * workspace and import the checkout.
 */
export function AddProjectDialog({ ws, onClose, onAdded }: AddProjectDialogProps) {
  const { detail } = useWorkspace();
  const home = useHome();
  const existing = detail?.projects ?? [];
  const [mode, setMode] = useState<Mode>("folder");
  const [draft, setDraft] = useState<DraftProject | null>(null);
  const [clone, setClone] = useState<CloneProject | null>(null);
  const [busy, setBusy] = useState(false);
  const [fields, setFields] = useState<ImportErrors>({});
  const [cloneFields, setCloneFields] = useState<CloneErrors>({});
  const [general, setGeneral] = useState<string | null>(null);
  const [progress, setProgress] = useState<string | null>(null);
  const creds = useGitCredentials();

  useChannel(busy && clone ? `workspace:${ws}` : null, (m) => {
    if (m.type === "git_progress" && m.key === clone?.key) setProgress(m.line);
  });

  const submitFolder = () => {
    if (!draft) return;
    setBusy(true);
    setFields({});
    setGeneral(null);
    api.importProject(ws, { path: expandHome(draft.path, home), key: draft.key, stack: draft.stack || null }).then(
      (d) => onAdded(d, draft.key),
      (e: Error) => {
        setBusy(false);
        const placed = importErrors(e instanceof HttpError ? e.issues : [], e.message);
        setFields(placed.fields);
        setGeneral(placed.general);
      },
    );
  };

  const submitClone = () => {
    if (!clone) return;
    setBusy(true);
    setCloneFields({});
    setGeneral(null);
    setProgress(null);
    const body = { ...clone, path: clone.path ? expandHome(clone.path, home) : undefined };
    api.cloneProject(ws, body).then(
      (d) => onAdded(d, clone.key),
      (e: Error) => {
        setBusy(false);
        setProgress(null);
        const placed = cloneErrors(e instanceof HttpError ? e.issues : [], e.message);
        setCloneFields(placed.fields);
        setGeneral(placed.general);
      },
    );
  };

  const ready = mode === "folder" ? !!draft : !!clone;
  return (
    <Dialog
      title="Add a project"
      subtitle={
        mode === "folder"
          ? `to ${detail?.settings.name ?? ws}. Like /add-dir: the folder is referenced by path, never copied.`
          : `to ${detail?.settings.name ?? ws}. Ostra clones the repository into the workspace, then imports the checkout.`
      }
      onClose={onClose}
      width={640}
      footer={
        <>
          {busy && progress && (
            <span
              style={{
                display: "flex",
                gap: 6,
                alignItems: "center",
                minWidth: 0,
                color: "var(--text-muted)",
                font: "var(--text-sm)/1.2 var(--font-mono)",
              }}
            >
              <Spinner size={11} />
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{progress}</span>
            </span>
          )}
          <span style={{ flex: 1 }} />
          <Button onClick={onClose}>{busy && mode === "git" ? "Close" : "Cancel"}</Button>
          {mode === "folder" ? (
            <Button variant="primary" icon="folder-plus" disabled={!ready || busy} onClick={submitFolder}>
              {busy ? "Importing…" : "Import project"}
            </Button>
          ) : (
            <Button variant="primary" icon="git-fork" disabled={!ready || busy} onClick={submitClone}>
              {busy ? "Cloning…" : "Clone and import"}
            </Button>
          )}
        </>
      }
    >
      <Tabs
        label="Project source"
        value={mode}
        onChange={(id) => {
          if (busy) return;
          setMode(id as Mode);
          setGeneral(null);
        }}
        tabs={[
          { id: "folder", label: "Folder on this machine", icon: "folder-plus" },
          { id: "git", label: "Clone from git", icon: "git-fork" },
        ]}
      />
      {general && <Banner tone="bad">{general}</Banner>}
      {mode === "folder" ? (
        <ImportForm
          compact
          stacks={detail?.stacks ?? []}
          takenKeys={existing.map((p) => p.key)}
          takenPaths={Object.fromEntries(existing.map((p) => [p.path, p.key]))}
          initialPath={detail ? projectStart(detail.root) : "~/"}
          onDraft={(d) => {
            setDraft(d);
            setFields({});
            setGeneral(null);
          }}
          serverErrors={fields}
        />
      ) : (
        <>
          <CloneForm
            takenKeys={existing.map((p) => p.key)}
            stacks={detail?.stacks ?? []}
            root={detail?.root ?? ""}
            credentials={creds.list}
            serverErrors={cloneFields}
            disabled={busy}
            onDraft={(c) => {
              setClone(c);
              setCloneFields({});
            }}
          />
          <span className="wp-muted" style={{ fontSize: "var(--text-sm)" }}>
            Save tokens and SSH keys under Settings, Git; they stay on the machine that runs Ostra. Closing this dialog
            does not stop a clone that has started.
          </span>
        </>
      )}
    </Dialog>
  );
}
