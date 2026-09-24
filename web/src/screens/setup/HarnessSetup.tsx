import { lazy, Suspense, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { api, socket } from "../../api";
import type { HarnessSetupAction } from "../../api/gen/HarnessSetupAction";
import type { HarnessSetupTerminal } from "../../api/gen/HarnessSetupTerminal";
import type { HarnessKind, HarnessStatus } from "../../api/types";
import { Banner, Button, IconButton, Spinner, Terminal } from "../../design";
import "../execution/execution.css";
import { HARNESS_LABEL } from "./wizard";

const XtermScreen = lazy(() => import("../execution/XtermScreen"));

export type SetupRun = { harness: HarnessKind; action: HarnessSetupAction; term: HarnessSetupTerminal };

/**
 * Install and login terminals for harness CLIs. While one is open, a `harness_status` on `home` that differs from
 * what the page shows reloads the check, because the server sends one when the command exits.
 */
export function useHarnessSetup(shown: HarnessStatus[] | undefined, reload: () => void) {
  const [run, setRun] = useState<SetupRun | null>(null);
  const [starting, setStarting] = useState<HarnessKind | null>(null);
  const [error, setError] = useState<string | null>(null);
  const current = useRef(shown);
  current.current = shown;
  const reloadRef = useRef(reload);
  reloadRef.current = reload;
  const open = run?.term.terminal;
  useEffect(() => {
    if (!open) return;
    return socket().subscribe("home", (m) => {
      if (m.type === "harness_status" && JSON.stringify(m.statuses) !== JSON.stringify(current.current)) reloadRef.current();
    });
  }, [open]);
  const start = (harness: HarnessKind, action: HarnessSetupAction) => {
    setStarting(harness);
    setError(null);
    api.harnessSetup(harness, action).then(
      (term) => {
        setStarting(null);
        setRun({ harness, action, term });
      },
      (e: unknown) => {
        setStarting(null);
        setError(e instanceof Error ? e.message : String(e));
      },
    );
  };
  return { run, starting, error, start, close: () => setRun(null) };
}

/** The Install or Log in button for one harness row, or nothing when it is ready. */
export function HarnessAction({ h, starting, onStart }: { h: HarnessStatus; starting: boolean; onStart: (action: HarnessSetupAction) => void }) {
  if (h.installed && h.logged_in === true) return null;
  const action: HarnessSetupAction = h.installed ? "login" : "install";
  return (
    <Button size="sm" disabled={starting} onClick={() => onStart(action)}>
      {starting ? "Starting…" : action === "install" ? "Install" : "Log in"}
    </Button>
  );
}

/** The live terminal of an install or login, typed into like a harness execution's Terminal tab. */
export function SetupTerminalPanel({ run, error, onClose, onCheck }: { run: SetupRun | null; error: string | null; onClose: () => void; onCheck: () => void }) {
  const [maximized, setMaximized] = useState(false);
  if (error) return <Banner tone="bad">{error}</Banner>;
  if (!run) return null;
  const label = HARNESS_LABEL[run.harness] ?? run.harness;
  // Keys typed into the terminal belong to the CLI: Escape and Tab must not reach the dialog, which closes or traps focus.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!(e.target as HTMLElement).closest(".ex-xterm")) return;
    e.stopPropagation();
    if (e.key === "Escape" && e.shiftKey) setMaximized(false);
  };
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
      <div className="ex-term-frame" data-maximized={maximized || undefined} onKeyDown={onKeyDown}>
        <Terminal
          title={run.action === "install" ? `Install ${label}` : `Log in to ${label}`}
          meta={run.term.command}
          live
          height={maximized ? "100%" : "max(420px, calc(84vh - 320px))"}
          actions={
            <>
              <IconButton
                icon={maximized ? "minimize-2" : "maximize-2"}
                size="sm"
                label={maximized ? "Restore the terminal (Shift+Esc)" : "Maximize the terminal"}
                onClick={() => setMaximized((m) => !m)}
              />
              <IconButton icon="circle-x" size="sm" label="Close the terminal" onClick={onClose} />
            </>
          }
        >
          <Suspense
            fallback={
              <div className="ex-term-loading">
                <Spinner size={11} /> Loading the terminal…
              </div>
            }
          >
            <XtermScreen key={run.term.terminal} execution={run.term.terminal} readOnly={false} />
          </Suspense>
        </Terminal>
      </div>
      <div style={{ display: "flex", gap: 12, alignItems: "flex-start" }}>
        <span className="os-field__hint" style={{ flex: 1 }}>
          This terminal runs on the machine that runs Ostra. Type into it to answer the installer or finish the login. The check updates when the command
          exits; if a login opens a browser page on another machine, paste the code it shows here.
        </span>
        <Button size="sm" onClick={onCheck}>
          Check again
        </Button>
      </div>
    </div>
  );
}
