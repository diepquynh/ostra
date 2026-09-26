import { type ReactNode, useEffect, useRef, useState } from "react";
import { Icon } from "../core/Icon";

export type TerminalTone = "" | "muted" | "fn" | "ok" | "add" | "bad" | "del" | "warn";

export type TerminalLine = [TerminalTone, string] | { tone: TerminalTone; text: string };

export interface TerminalProps {
  /** Harness + model, e.g. "codex · gpt-5.6-terra". */
  title: string;
  /** Secondary bar text: cwd, size, pid. */
  meta?: string;
  /** Output lines for the static rendering (docs, previews, tests). Ignored when children are given. */
  lines?: TerminalLine[];
  /** Running session: shows the Live marker and, with lines, a cursor and auto-follow of new output. */
  live?: boolean;
  /** With lines: show the input row (the user can type into the harness). xterm.js takes input itself. */
  interactive?: boolean;
  onInput?: (text: string) => void;
  height?: number | string;
  /** Extra controls on the right of the bar. */
  actions?: ReactNode;
  /**
   * The live screen, typically the xterm.js host element. It fills the screen area, which keeps the design's
   * padding and background; give the host `flex: 1; min-height: 0` so the fit addon measures the right size.
   */
  children?: ReactNode;
}

/** Terminal pane for a harness execution: the bar chrome plus either the static lines or a live xterm.js screen. */
export function Terminal({
  title,
  meta,
  lines = [],
  live,
  interactive,
  onInput,
  height = 420,
  actions,
  children,
}: TerminalProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [stick, setStick] = useState(true);
  const [value, setValue] = useState("");
  const slot = children != null;

  useEffect(() => {
    if (!slot && stick && ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [lines.length, stick, slot]);

  const onScroll = () => {
    const el = ref.current;
    if (el && !slot) setStick(el.scrollHeight - el.scrollTop - el.clientHeight < 24);
  };

  return (
    <div className="os-term" style={{ height }}>
      <div className="os-term__bar">
        <Icon name="square-terminal" size={13} />
        <span className="os-term__title">{title}</span>
        {meta && <span>{meta}</span>}
        <span style={{ flex: 1 }} />
        {live ? (
          <span style={{ display: "inline-flex", alignItems: "center", gap: 5, color: "var(--term-green)" }}>
            <span className="os-dot os-dot--pulse" style={{ background: "var(--term-green)" }} />
            Live
          </span>
        ) : (
          <span>Session ended</span>
        )}
        {actions}
      </div>
      {slot ? (
        <div className="os-term__screen" style={{ display: "flex", flexDirection: "column", overflow: "hidden" }}>
          {children}
        </div>
      ) : (
        <div
          className="os-term__screen"
          ref={ref}
          onScroll={onScroll}
          role="log"
          aria-live={live ? "polite" : undefined}
        >
          {lines.map((l, i) => {
            const [tone, text] = Array.isArray(l) ? l : [l.tone, l.text];
            return (
              <div key={i} className={`os-term__line ${tone ? "os-term__line--" + tone : ""}`}>
                {text}
              </div>
            );
          })}
          {live && (
            <div className="os-term__line">
              <span className="os-term__cursor" />
            </div>
          )}
        </div>
      )}
      {!slot && interactive && live && (
        <form
          className="os-term__input"
          onSubmit={(e) => {
            e.preventDefault();
            if (value && onInput) onInput(value);
            setValue("");
          }}
        >
          <span style={{ color: "var(--term-muted)", fontFamily: "var(--font-mono)" }}>›</span>
          <input
            value={value}
            onChange={(e) => setValue(e.target.value)}
            placeholder="Type into the harness session. Enter sends."
            aria-label="Terminal input"
          />
        </form>
      )}
      {!slot && !stick && live && (
        <button
          type="button"
          onClick={() => setStick(true)}
          className="os-btn os-btn--sm"
          style={{ position: "absolute", right: 16, bottom: 44 }}
        >
          Follow output
        </button>
      )}
    </div>
  );
}
