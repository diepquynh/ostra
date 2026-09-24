import { useEffect, useRef } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal as Xterm, type ITheme } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { socket } from "../../api";
import { linkHandler, muteQueryReplies } from "./terminalSafety";

/** The terminal palette from the design tokens. The pane is dark in both themes. */
function theme(): ITheme {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  const bg = v("--term-bg", "#0d0d0c");
  const fg = v("--term-fg", "#c9c9c2");
  const muted = v("--term-muted", "#6e6e68");
  const red = v("--term-red", "#ff8a80");
  const green = v("--term-green", "#6fcf8f");
  const amber = v("--term-amber", "#f0b458");
  const blue = v("--term-blue", "#82aaff");
  const violet = "#b3a6ff";
  const cyan = "#7fd4d4";
  return {
    background: bg,
    foreground: fg,
    cursor: v("--term-cursor", "#e8e8e3"),
    cursorAccent: bg,
    selectionBackground: "rgba(130, 170, 255, 0.3)",
    black: v("--term-bar", "#151514"),
    red,
    green,
    yellow: amber,
    blue,
    magenta: violet,
    cyan,
    white: fg,
    brightBlack: muted,
    brightRed: red,
    brightGreen: green,
    brightYellow: amber,
    brightBlue: blue,
    brightMagenta: violet,
    brightCyan: cyan,
    brightWhite: "#f2f2ee",
  };
}

export interface XtermScreenProps {
  execution: string;
  /** Replay of a stored transcript: no input, no resizes sent. */
  readOnly: boolean;
  /** Reports the fitted size for the terminal bar. */
  onSize?: (cols: number, rows: number) => void;
}

/**
 * xterm.js on the harness PTY (`term:<execution>`). The first frame after subscribing is the backlog: the screen
 * so far for a live run, or the stored transcript for an ended one. Keystrokes and resizes go back while live.
 */
export default function XtermScreen({ execution, readOnly, onSize }: XtermScreenProps) {
  const host = useRef<HTMLDivElement>(null);
  const sizeCb = useRef(onSize);
  sizeCb.current = onSize;

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const term = new Xterm({
      convertEol: false,
      cursorBlink: !readOnly,
      cursorStyle: readOnly ? "underline" : "block",
      cursorInactiveStyle: "none",
      disableStdin: readOnly,
      fontFamily: '"JetBrains Mono", ui-monospace, "SF Mono", Menlo, Consolas, monospace',
      fontSize: 12.5,
      lineHeight: 1.2,
      scrollback: 10000,
      theme: theme(),
      allowProposedApi: false,
      linkHandler,
    });
    const muted = muteQueryReplies(term.parser);
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    const sock = socket();
    const sendSize = () => {
      try {
        fit.fit();
      } catch {
        return;
      }
      sizeCb.current?.(term.cols, term.rows);
      if (!readOnly) sock.send({ type: "term_resize", execution, cols: term.cols, rows: term.rows });
    };
    sendSize();
    // The mono font may load after the first measure. Setting the family again makes xterm measure the cells anew.
    let disposed = false;
    void document.fonts?.ready.then(() => {
      if (disposed) return;
      const family = term.options.fontFamily;
      term.options.fontFamily = "monospace";
      term.options.fontFamily = family;
      sendSize();
    });
    const unsub = sock.terminal(execution, (data) => term.write(data));
    const offReconnect = sock.onReconnect(sendSize);
    const input = term.onData((data) => {
      if (!readOnly) sock.send({ type: "term_input", execution, data });
    });
    const ro = new ResizeObserver(() => sendSize());
    ro.observe(el);
    if (!readOnly) term.focus();
    return () => {
      disposed = true;
      ro.disconnect();
      input.dispose();
      unsub();
      offReconnect();
      muted.dispose();
      term.dispose();
    };
  }, [execution, readOnly]);

  return <div ref={host} className="ex-xterm" data-readonly={readOnly || undefined} />;
}
