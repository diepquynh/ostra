import { Icon, LiveMark, REST, Spinner, StatusDot } from "@ostra/design";
import { useEffect, useState } from "react";
import { api, HttpError } from "../../api";
import type { CloneProject, WorkspaceDetail } from "../../api/types";
import { useChannel } from "../../lib/hooks";
import { useWorkspace } from "../../lib/nav";
import { type CloneErrors, CloneForm, cloneErrors } from "../../screens/setup/CloneForm";
import { useInitAfterCreate } from "../../screens/setup/finish";
import { useHome } from "../../screens/setup/folders";
import { useGitCredentials } from "../../screens/setup/GitCredentials";
import { ImportForm } from "../../screens/setup/ImportForm";
import { useReducedMotion, WizardBody } from "../../screens/setup/steps";
import { useWizard, type Wizard } from "../../screens/setup/useWizard";
import {
  type DraftProject,
  expandHome,
  type ImportErrors,
  importErrors,
  projectStart,
} from "../../screens/setup/wizard";
import "./MSetup.css";

export type SetupMode = "onboard" | "new" | "add";

export type MSetupProps = {
  ws: string;
  mode: SetupMode;
  /** Setup ended: with a created workspace id, an added project key, or neither when cancelled. */
  onDone: (createdWorkspace: string | null, addedProject: string | null) => void;
};

/** First-run setup, New workspace and Add project, as a pushed screen instead of a dialog. */
export function MSetup({ ws, mode, onDone }: MSetupProps) {
  if (mode === "add") return <MAddProject ws={ws} onDone={onDone} />;
  return (
    <MWizard
      skipWelcome={mode === "new"}
      onDone={(created) => {
        if (mode === "onboard") void api.completeOnboarding().catch(() => {});
        onDone(created, null);
      }}
    />
  );
}

/**
 * The setup steps on a phone: progress bars, the step body the desktop setup renders, and one row of
 * buttons at the end of the page. `onDone` gets the created workspace id, or null when the user skipped.
 */
export function MWizard({ skipWelcome, onDone }: { skipWelcome: boolean; onDone: (created: string | null) => void }) {
  const w = useWizard(skipWelcome);
  const init = useInitAfterCreate((d) => onDone(d.id));
  const current = isLocked(w) ? w.steps.length - 1 : w.i;
  return (
    <div className="m-page ms-setup">
      <div className="ms-steps">
        <div className="ms-bars">
          {w.steps.map((s, i) => (
            <span key={s.id} className={i <= current ? "ms-bar ms-bar-on" : "ms-bar"} />
          ))}
        </div>
        <span className="ms-step-label">
          Step {current + 1} of {w.steps.length} · {w.steps[current].label}
        </span>
      </div>
      {w.id === "welcome" ? <MWelcome /> : <WizardBody w={w} padding="0" />}
      <MWizardNav w={w} onSkip={() => onDone(null)} onInit={init.init} initializing={init.busy} onOpen={onDone} />
    </div>
  );
}

const STAGES = ["Research", "Requirements", "Design", "Build", "Review", "Test"];

/** The welcome step: what Ostra does, with the stages lighting up one after another. */
function MWelcome() {
  const reduced = useReducedMotion();
  const [n, setN] = useState(reduced ? 2 : 0);
  useEffect(() => {
    if (reduced) return;
    const t = setInterval(() => setN((x) => (x + 1) % (STAGES.length + 1)), 700);
    return () => clearInterval(t);
  }, [reduced]);
  return (
    <div className="ms-welcome">
      <LiveMark state={REST} size={44} />
      <h1>Set up Ostra</h1>
      <p>
        Ostra runs each change through research, requirements, design, build, review and test, and stops to ask you when
        a decision is yours. This takes about two minutes.
      </p>
      <div className="ms-stages">
        {STAGES.map((label, j) => (
          <div key={label}>
            <StatusDot tone={j < n ? "ok" : j === n ? "accent" : "neutral"} pulse={j === n} hollow={j > n} />
            <span>{label}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

type MWizardNavProps = {
  w: Wizard;
  onSkip: () => void;
  onInit: (d: WorkspaceDetail, key: string) => void;
  initializing: boolean;
  onOpen: (id: string) => void;
};

/** The same choices as the desktop `WizardNav`, as full-height touch buttons. */
function MWizardNav({ w, onSkip, onInit, initializing, onOpen }: MWizardNavProps) {
  const last = w.i === w.steps.length - 1;
  const toInit = w.created?.projects.find((p) => p.init_status === "not_initialized");
  const created = w.created;
  if (last && w.done && created)
    return (
      <div className="ms-nav ms-nav-col">
        <button type="button" className="m-btn m-btn-primary ms-next" onClick={() => onOpen(created.id)}>
          Open workspace
        </button>
        {toInit && (
          <button
            type="button"
            className="m-btn ms-next"
            disabled={initializing}
            onClick={() => onInit(created, toInit.key)}
          >
            <Icon name="sparkles" size={15} />
            {initializing ? "Starting…" : `Initialize ${toInit.key}`}
          </button>
        )}
      </div>
    );
  if (isLocked(w)) return null;
  const skipProjects = w.id === "projects" && w.v.projects.length === 0 && w.v.clones.length === 0;
  return (
    <div className="ms-nav">
      {w.i > 0 && (
        <button type="button" className="m-btn ms-back" onClick={() => w.go(w.i - 1)}>
          Back
        </button>
      )}
      {w.i === 0 && (
        <button type="button" className="m-btn m-btn-quiet ms-back" onClick={onSkip}>
          {w.id === "welcome" ? "Skip setup" : "Cancel"}
        </button>
      )}
      {last ? (
        <button type="button" className="m-btn m-btn-primary ms-next" disabled={!w.canCreate} onClick={w.create}>
          {w.validating ? "Checking…" : "Create workspace"}
        </button>
      ) : skipProjects ? (
        <button type="button" className="m-btn m-btn-primary ms-next" onClick={() => w.go(w.i + 1)}>
          Skip for now
        </button>
      ) : (
        <button
          type="button"
          className="m-btn m-btn-primary ms-next"
          disabled={!w.canNext}
          onClick={() => w.go(w.i + 1)}
        >
          {w.id === "welcome" ? "Get started" : "Continue"}
        </button>
      )}
    </div>
  );
}

const isLocked = (w: Wizard) => w.creating || !!w.created;

type Source = "folder" | "git";

/** Add project: import a folder on this machine, or clone a repository into the workspace and import it. */
function MAddProject({ ws, onDone }: { ws: string; onDone: MSetupProps["onDone"] }) {
  const { detail } = useWorkspace();
  const home = useHome();
  const existing = detail?.projects ?? [];
  const [source, setSource] = useState<Source>("folder");
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
      () => onDone(null, draft.key),
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
      () => onDone(null, clone.key),
      (e: Error) => {
        setBusy(false);
        setProgress(null);
        const placed = cloneErrors(e instanceof HttpError ? e.issues : [], e.message);
        setCloneFields(placed.fields);
        setGeneral(placed.general);
      },
    );
  };

  const pick = (s: Source) => {
    if (busy) return;
    setSource(s);
    setGeneral(null);
  };
  const name = detail?.settings.name ?? ws;
  return (
    <div className="m-page ms-setup">
      <div className="ms-head">
        <h2>Add a project</h2>
        <span>
          {source === "folder"
            ? `To ${name}. The folder is referenced by path and never copied.`
            : `To ${name}. Ostra clones the repository into the workspace, then imports the checkout.`}
        </span>
      </div>
      <div className="ms-seg" role="radiogroup" aria-label="Project source">
        <button type="button" role="radio" aria-checked={source === "folder"} onClick={() => pick("folder")}>
          <Icon name="folder-plus" size={14} />
          Folder
        </button>
        <button type="button" role="radio" aria-checked={source === "git"} onClick={() => pick("git")}>
          <Icon name="git-fork" size={14} />
          Clone from git
        </button>
      </div>
      {general && <div className="ms-error">{general}</div>}
      {source === "folder" ? (
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
          <span className="m-help">
            Save tokens and SSH keys under Settings, Git. They stay on the machine that runs Ostra. Leaving this screen
            does not stop a clone that has started.
          </span>
        </>
      )}
      {busy && progress && (
        <div className="ms-progress">
          <Spinner size={12} />
          <span>{progress}</span>
        </div>
      )}
      <div className="ms-nav">
        <button type="button" className="m-btn ms-back" onClick={() => onDone(null, null)}>
          {busy && source === "git" ? "Close" : "Cancel"}
        </button>
        {source === "folder" ? (
          <button
            type="button"
            className="m-btn m-btn-primary ms-next"
            disabled={!draft || busy}
            onClick={submitFolder}
          >
            <Icon name="folder-plus" size={15} />
            {busy ? "Importing…" : "Import project"}
          </button>
        ) : (
          <button type="button" className="m-btn m-btn-primary ms-next" disabled={!clone || busy} onClick={submitClone}>
            <Icon name="git-fork" size={15} />
            {busy ? "Cloning…" : "Clone and import"}
          </button>
        )}
      </div>
    </div>
  );
}
