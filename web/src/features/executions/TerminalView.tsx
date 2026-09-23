import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { socket } from "../../api";

/** The harness's own interface, streamed from its PTY. Keystrokes and resizes go back to it. */
export default function TerminalView({ execution, readOnly }: { execution: string; readOnly: boolean }) {
  const host = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!host.current) return;
    const term = new Terminal({
      convertEol: false,
      cursorBlink: !readOnly,
      disableStdin: readOnly,
      fontFamily: 'ui-monospace, "JetBrains Mono", Menlo, monospace',
      fontSize: 13,
      scrollback: 10000,
      theme: { background: "#0d0d0c" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current);
    const sock = socket();
    const sendSize = () => {
      try {
        fit.fit();
      } catch {
        return;
      }
      sock.send({ type: "term_resize", execution, cols: term.cols, rows: term.rows });
    };
    sendSize();
    const unsub = sock.terminal(execution, (data) => term.write(data));
    const unsubReconnect = sock.onReconnect(sendSize);
    const input = term.onData((data) => {
      if (!readOnly) sock.send({ type: "term_input", execution, data });
    });
    const ro = new ResizeObserver(() => sendSize());
    ro.observe(host.current);
    return () => {
      ro.disconnect();
      input.dispose();
      unsub();
      unsubReconnect();
      term.dispose();
    };
  }, [execution, readOnly]);

  return <div className="terminal-host" ref={host} />;
}
