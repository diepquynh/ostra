import { lazy, Suspense, useEffect, useState, type ReactNode } from "react";
import { Banner, Button, IconButton, Spinner, Terminal } from "../../design";
import type { TerminalMode } from "./model";

const XtermScreen = lazy(() => import("./XtermScreen"));

export interface TerminalStreamProps {
  execution: string;
  mode: TerminalMode;
  /** `harness:codex · gpt-5.6-terra` style title. */
  title: string;
  project: string;
  canResume: boolean;
  resuming: boolean;
  onResume: () => void;
  /** Banner above the terminal, such as a paused permission ask. */
  notice?: ReactNode;
}

const HEIGHT = "max(360px, calc(100vh - 400px))";

/** The Terminal stream: the harness's own interface in the design's terminal chrome. */
export function TerminalStream({ execution, mode, title, project, canResume, resuming, onResume, notice }: TerminalStreamProps) {
  const [size, setSize] = useState<string | null>(null);
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!maximized) return;
    const onKey = (ev: KeyboardEvent) => {
      // Escape belongs to the harness while the terminal has focus, so there only Shift+Escape restores.
      const inTerm = (ev.target as HTMLElement | null)?.closest?.(".ex-xterm");
      if (ev.key === "Escape" && (ev.shiftKey || !inTerm)) setMaximized(false);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [maximized]);
  const resume = canResume && mode !== "live" && (
    <Button size="sm" variant="primary" icon="rotate-ccw" onClick={onResume} disabled={resuming}>
      {resuming ? "Resuming…" : "Resume"}
    </Button>
  );

  if (mode === "none") {
    return (
      <Banner tone="neutral" title="No terminal output was stored for this run" actions={resume}>
        {canResume
          ? "Resume reopens the harness session with its own resume command, in a new terminal you can type into."
          : "The harness session has ended and cannot be resumed."}
      </Banner>
    );
  }

  return (
    <div className="ex-stack">
      {mode === "replay" && (
        <Banner tone="info" icon="rotate-ccw" title={canResume ? "Replaying the ended session. Resume to continue." : "Replaying the ended session"} actions={resume}>
          {canResume
            ? "Input is off during the replay. Resume reopens the harness session with its own resume command, and you can type into it again."
            : "Input is off during the replay. This harness session cannot be resumed."}
        </Banner>
      )}
      <div className="ex-term-frame" data-maximized={maximized || undefined}>
        {notice}
        <Terminal
          title={title}
          meta={[project, size].filter(Boolean).join(" · ")}
          live={mode === "live"}
          height={maximized ? "100%" : HEIGHT}
          actions={
            <IconButton
              icon={maximized ? "minimize-2" : "maximize-2"}
              size="sm"
              label={maximized ? "Restore the terminal (Shift+Esc)" : "Maximize the terminal"}
              onClick={() => setMaximized((m) => !m)}
            />
          }
        >
          <Suspense
            fallback={
              <div className="ex-term-loading">
                <Spinner size={11} /> Loading the terminal…
              </div>
            }
          >
            <XtermScreen key={`${execution}:${mode}`} execution={execution} readOnly={mode !== "live"} onSize={(c, r) => setSize(`${c}×${r}`)} />
          </Suspense>
        </Terminal>
      </div>
      <div className="ex-hint">
        {mode === "live"
          ? "This is the harness's own interface, streamed from its terminal. You can type into it. Ostra's guards and permissions still apply through the hook bridge; see Tool calls for every decision."
          : "This is the stored transcript of the harness's terminal. Tool calls lists every decision the hook bridge made during the run."}
      </div>
    </div>
  );
}
