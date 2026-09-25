import type { PtyFrame, SocketState } from "./extra";
import type { ClientMsg, ServerMsg } from "./types";

// Dynamic, so the fixtures stay out of the production bundle (see `api/index.ts`).
const mockSocket: SocketFactory | null =
  import.meta.env.VITE_MOCK === "1" ? (await import("./mock/mockSocket")).mockSocket : null;
/** Every message the server can send. */
export type WireMsg = ServerMsg;

type MsgHandler = (msg: WireMsg) => void;

type HintAsk = Extract<ClientMsg, { type: "code_complete" | "code_signature" | "code_navigate" }>;
/** A hint request without its type and id. */
export type HintRequest<T extends HintAsk["type"]> = Omit<Extract<HintAsk, { type: T }>, "type" | "id">;
type HintReplies = {
  code_complete: Extract<ServerMsg, { type: "code_completion" }>;
  code_signature: Extract<ServerMsg, { type: "code_signature_help" }>;
  code_navigate: Extract<ServerMsg, { type: "code_navigation" }>;
};
type HintReply = HintReplies[keyof HintReplies];

/** A hint the server has not answered by then resolves to null. */
const HINT_TIMEOUT_MS = 30_000;
type PtyHandler = (data: Uint8Array) => void;

/** The subset of the browser WebSocket the manager uses, so tests can pass a fake. */
export interface SocketLike {
  binaryType: string;
  readyState: number;
  onopen: ((ev: unknown) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  send(data: string): void;
  close(): void;
}

export type SocketFactory = (url: string) => SocketLike;

const OPEN = 1;

/** Decode a binary PTY frame: byte 0 is the id length n, then n id bytes, then terminal bytes. */
export function decodePtyFrame(buf: ArrayBuffer | Uint8Array): PtyFrame | null {
  const bytes = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  if (bytes.length < 1) return null;
  const n = bytes[0];
  if (bytes.length < 1 + n) return null;
  const execution = new TextDecoder().decode(bytes.subarray(1, 1 + n));
  return { execution, data: bytes.subarray(1 + n) };
}

/** Which channel a server message belongs to, so the manager can route it. */
export function channelsFor(msg: WireMsg): string[] {
  switch (msg.type) {
    case "session_event":
      return [`session:${msg.session}`];
    case "execution_delta":
    case "execution_status":
      return [`execution:${msg.execution}`];
    case "session_updated":
      return [`session:${msg.summary.id}`, `workspace:${msg.summary.workspace}`, "home"];
    case "workspace_updated":
      return [`workspace:${msg.workspace}`, "home"];
    case "harness_status":
      return ["home"];
    case "project_fs_changed":
    case "git_progress":
    case "tree_patch":
    case "activity":
      return [`workspace:${msg.workspace}`];
    default:
      return [];
  }
}

/**
 * One reconnecting socket shared by every screen. Channels are reference counted, resubscribed
 * after a reconnect, and reconnect listeners fire so screens can refetch their REST snapshot.
 */
export class SocketManager {
  private socket: SocketLike | null = null;
  private handlers = new Map<string, Set<MsgHandler>>();
  private pty = new Map<string, Set<PtyHandler>>();
  private reconnectListeners = new Set<() => void>();
  private stateListeners = new Set<(s: SocketState) => void>();
  private anyListeners = new Set<MsgHandler>();
  private attempts = 0;
  private everOpened = false;
  private stopped = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private hintSeq = 0;
  private hints = new Map<number, (reply: HintReply | null) => void>();
  private latestHint = new Map<HintAsk["type"], number>();
  state: SocketState = "closed";

  constructor(
    private url: string,
    private factory: SocketFactory,
    // Wrapped, because the browser's setTimeout throws "Illegal invocation" when called as a method.
    private schedule: (fn: () => void, ms: number) => ReturnType<typeof setTimeout> = (fn, ms) => setTimeout(fn, ms),
  ) {}

  start(): void {
    this.stopped = false;
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    if (this.timer) clearTimeout(this.timer);
    this.socket?.close();
    this.socket = null;
    this.setState("closed");
  }

  /** Subscribe to a channel. Returns the unsubscribe function. */
  subscribe(channel: string, handler: MsgHandler): () => void {
    let set = this.handlers.get(channel);
    const isNew = !set;
    if (!set) {
      set = new Set();
      this.handlers.set(channel, set);
    }
    set.add(handler);
    if (isNew) this.send({ type: "subscribe", channel });
    return () => {
      const s = this.handlers.get(channel);
      if (!s) return;
      s.delete(handler);
      if (s.size === 0) {
        this.handlers.delete(channel);
        this.send({ type: "unsubscribe", channel });
      }
    };
  }

  /** Receive raw PTY bytes for one execution. Subscribes to `term:<execution>`. */
  terminal(execution: string, handler: PtyHandler): () => void {
    let set = this.pty.get(execution);
    if (!set) {
      set = new Set();
      this.pty.set(execution, set);
    }
    set.add(handler);
    const unsub = this.subscribe(`term:${execution}`, () => {});
    return () => {
      set?.delete(handler);
      if (set && set.size === 0) this.pty.delete(execution);
      unsub();
    };
  }

  onAny(handler: MsgHandler): () => void {
    this.anyListeners.add(handler);
    return () => this.anyListeners.delete(handler);
  }

  onReconnect(fn: () => void): () => void {
    this.reconnectListeners.add(fn);
    return () => this.reconnectListeners.delete(fn);
  }

  onState(fn: (s: SocketState) => void): () => void {
    this.stateListeners.add(fn);
    return () => this.stateListeners.delete(fn);
  }

  send(msg: ClientMsg): void {
    if (this.socket && this.socket.readyState === OPEN) {
      this.socket.send(JSON.stringify(msg));
    }
  }

  /**
   * Ask for completions, signature help, or a supertype or implementation jump. Resolves to null when the socket is closed, when the server does not
   * answer in time, or when a newer request of the same type replaces this one, because the server stops it.
   */
  hint<T extends HintAsk["type"]>(type: T, req: HintRequest<T>): Promise<HintReplies[T] | null> {
    if (!this.socket || this.socket.readyState !== OPEN) return Promise.resolve(null);
    const id = ++this.hintSeq;
    const older = this.latestHint.get(type);
    if (older !== undefined) this.settleHint(older, null);
    this.latestHint.set(type, id);
    return new Promise((resolve) => {
      const timer = this.schedule(() => this.settleHint(id, null), HINT_TIMEOUT_MS);
      this.hints.set(id, (reply) => {
        clearTimeout(timer);
        resolve(reply as HintReplies[T] | null);
      });
      this.send({ ...req, type, id } as unknown as HintAsk);
    });
  }

  private settleHint(id: number, reply: HintReply | null) {
    const done = this.hints.get(id);
    if (!done) return;
    this.hints.delete(id);
    done(reply);
  }

  channels(): string[] {
    return [...this.handlers.keys()];
  }

  private setState(s: SocketState) {
    this.state = s;
    this.stateListeners.forEach((fn) => fn(s));
  }

  private connect() {
    if (this.stopped) return;
    this.setState("connecting");
    const sock = this.factory(this.url);
    sock.binaryType = "arraybuffer";
    this.socket = sock;
    sock.onopen = () => {
      this.attempts = 0;
      this.setState("open");
      for (const channel of this.handlers.keys()) {
        sock.send(JSON.stringify({ type: "subscribe", channel } satisfies ClientMsg));
      }
      if (this.everOpened) this.reconnectListeners.forEach((fn) => fn());
      this.everOpened = true;
    };
    sock.onmessage = (ev) => this.dispatch(ev.data);
    sock.onerror = () => {
      // The close handler that follows schedules the reconnect.
    };
    sock.onclose = () => {
      if (this.socket !== sock) return;
      this.socket = null;
      this.setState("closed");
      for (const id of [...this.hints.keys()]) this.settleHint(id, null);
      if (this.stopped) return;
      const delay = Math.min(10000, 500 * 2 ** this.attempts);
      this.attempts += 1;
      this.timer = this.schedule(() => this.connect(), delay);
    };
  }

  private dispatch(data: unknown) {
    if (typeof data === "string") {
      let msg: WireMsg;
      try {
        msg = JSON.parse(data) as WireMsg;
      } catch {
        return;
      }
      if (msg.type === "code_completion" || msg.type === "code_signature_help" || msg.type === "code_navigation") {
        this.settleHint(msg.id, msg);
        return;
      }
      this.anyListeners.forEach((fn) => fn(msg));
      for (const channel of channelsFor(msg)) {
        this.handlers.get(channel)?.forEach((fn) => fn(msg));
      }
      return;
    }
    if (data instanceof ArrayBuffer || data instanceof Uint8Array) {
      const frame = decodePtyFrame(data);
      if (frame) this.pty.get(frame.execution)?.forEach((fn) => fn(frame.data));
    }
  }
}

function defaultUrl(): string {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${location.host}/ws`;
}

let shared: SocketManager | null = null;

/** The app-wide socket. In mock mode it never connects. */
export function socket(): SocketManager {
  if (!shared) {
    const factory: SocketFactory = mockSocket ?? ((url) => new WebSocket(url) as unknown as SocketLike);
    shared = new SocketManager(defaultUrl(), factory);
    shared.start();
  }
  return shared;
}
