import type { SocketLike } from "../socket";
import type { ClientMsg, ServerMsg } from "../types";
import { isResumed, LIVE_DELTAS, terminalBacklog, terminalLive } from "./fixtures.execution";

const STEP_MS = 1800;

function frame(execution: string, text: string): ArrayBuffer {
  const id = new TextEncoder().encode(execution);
  const data = new TextEncoder().encode(text);
  const out = new Uint8Array(1 + id.length + data.length);
  out[0] = id.length;
  out.set(id, 1);
  out.set(data, 1 + id.length);
  return out.buffer;
}

/**
 * A socket for `VITE_MOCK=1`: it opens, streams the fixture deltas of running executions on `execution:<id>`,
 * sends each terminal's backlog on `term:<id>` and then its live chunks, and echoes keystrokes back. Under
 * vitest it never opens, so tests see no timers.
 */
export function mockSocket(): SocketLike {
  const timers = new Map<string, ReturnType<typeof setInterval>>();
  const stop = (channel: string) => {
    const t = timers.get(channel);
    if (t) clearInterval(t);
    timers.delete(channel);
  };
  const emit = (m: ServerMsg) => sock.onmessage?.({ data: JSON.stringify(m) });
  const pty = (execution: string, text: string) => sock.onmessage?.({ data: frame(execution, text) });

  const stream = <T>(channel: string, items: T[], send: (item: T) => void) => {
    stop(channel);
    let i = 0;
    if (items.length === 0) return;
    timers.set(
      channel,
      setInterval(() => {
        send(items[i++]);
        if (i >= items.length) stop(channel);
      }, STEP_MS),
    );
  };

  const onSubscribe = (channel: string) => {
    const [kind, id] = [channel.slice(0, channel.indexOf(":")), channel.slice(channel.indexOf(":") + 1)];
    if (kind === "execution") {
      // Live items continue the snapshot's seqs, so they apply after it.
      stream(channel, LIVE_DELTAS[id] ?? [], (item) => emit({ type: "execution_delta", execution: id, seq: item.seq, at: new Date().toISOString(), delta: item.delta }));
    }
    if (kind === "term") {
      const backlog = terminalBacklog(id);
      if (backlog === null) return;
      setTimeout(() => pty(id, backlog), 120);
      if (!isResumed(id)) stream(channel, terminalLive(id), (chunk) => pty(id, chunk));
    }
  };

  const sock: SocketLike = {
    binaryType: "arraybuffer",
    readyState: 0,
    onopen: null,
    onclose: null,
    onerror: null,
    onmessage: null,
    send(data: string) {
      const msg = JSON.parse(data) as ClientMsg;
      if (msg.type === "subscribe") onSubscribe(msg.channel);
      if (msg.type === "unsubscribe") stop(msg.channel);
      if (msg.type === "term_input") pty(msg.execution, msg.data.replace(/\r/g, "\r\n"));
    },
    close() {
      for (const c of [...timers.keys()]) stop(c);
      sock.readyState = 3;
    },
  };
  if (import.meta.env.MODE !== "test") {
    setTimeout(() => {
      sock.readyState = 1;
      sock.onopen?.({});
    }, 50);
  }
  return sock;
}
