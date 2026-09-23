import { describe, expect, it } from "vitest";
import { SocketManager, channelsFor, decodePtyFrame, type SocketLike, type WireMsg } from "./socket";
import type { ServerMsg } from "./types";

class FakeSocket implements SocketLike {
  binaryType = "blob";
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  sent: string[] = [];
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  receive(data: unknown) {
    this.onmessage?.({ data });
  }
}

function setup() {
  const sockets: FakeSocket[] = [];
  const timers: (() => void)[] = [];
  const mgr = new SocketManager(
    "ws://x/ws",
    () => {
      const s = new FakeSocket();
      sockets.push(s);
      return s;
    },
    (fn) => {
      timers.push(fn);
      return 0 as unknown as ReturnType<typeof setTimeout>;
    },
  );
  mgr.start();
  return { mgr, sockets, timers };
}

const sent = (s: FakeSocket) => s.sent.map((m) => JSON.parse(m));

describe("SocketManager", () => {
  it("subscribes once per channel and unsubscribes when the last handler leaves", () => {
    const { mgr, sockets } = setup();
    sockets[0].open();
    const a = mgr.subscribe("session:s1", () => {});
    const b = mgr.subscribe("session:s1", () => {});
    expect(sent(sockets[0])).toEqual([{ type: "subscribe", channel: "session:s1" }]);
    a();
    expect(sent(sockets[0])).toHaveLength(1);
    b();
    expect(sent(sockets[0])[1]).toEqual({ type: "unsubscribe", channel: "session:s1" });
  });

  it("resubscribes after reconnect and fires reconnect listeners only on later opens", () => {
    const { mgr, sockets, timers } = setup();
    let reconnects = 0;
    mgr.onReconnect(() => reconnects++);
    mgr.subscribe("workspace:w1", () => {});
    sockets[0].open();
    expect(reconnects).toBe(0);
    expect(sent(sockets[0])).toEqual([{ type: "subscribe", channel: "workspace:w1" }]);
    sockets[0].close();
    expect(mgr.state).toBe("closed");
    expect(timers).toHaveLength(1);
    timers[0]();
    expect(sockets).toHaveLength(2);
    sockets[1].open();
    expect(reconnects).toBe(1);
    expect(sent(sockets[1])).toEqual([{ type: "subscribe", channel: "workspace:w1" }]);
  });

  it("routes messages to their channel", () => {
    const { mgr, sockets } = setup();
    sockets[0].open();
    const got: WireMsg[] = [];
    mgr.subscribe("execution:x1", (m) => got.push(m));
    mgr.subscribe("execution:x2", () => {
      throw new Error("wrong channel");
    });
    sockets[0].receive(
      JSON.stringify({ type: "execution_delta", execution: "x1", seq: 1, at: "2026-01-01T00:00:00Z", delta: { kind: "text", text: "hi" } }),
    );
    sockets[0].receive("not json");
    expect(got).toHaveLength(1);
  });

  it("delivers PTY frames to terminal handlers", () => {
    const { mgr, sockets } = setup();
    sockets[0].open();
    const chunks: string[] = [];
    const off = mgr.terminal("x9", (d) => chunks.push(new TextDecoder().decode(d)));
    expect(sent(sockets[0])).toEqual([{ type: "subscribe", channel: "term:x9" }]);
    const id = new TextEncoder().encode("x9");
    const body = new TextEncoder().encode("hello");
    const frame = new Uint8Array([id.length, ...id, ...body]);
    sockets[0].receive(frame.buffer);
    expect(chunks).toEqual(["hello"]);
    off();
    expect(sent(sockets[0])[1]).toEqual({ type: "unsubscribe", channel: "term:x9" });
  });

  it("does not send while closed", () => {
    const { mgr, sockets } = setup();
    mgr.send({ type: "term_input", execution: "x", data: "a" });
    expect(sockets[0].sent).toHaveLength(0);
  });
});

describe("frames and channels", () => {
  it("decodes frames and rejects short ones", () => {
    expect(decodePtyFrame(new Uint8Array([]))).toBeNull();
    expect(decodePtyFrame(new Uint8Array([5, 1]))).toBeNull();
    const f = decodePtyFrame(new Uint8Array([1, 65, 66]));
    expect(f?.execution).toBe("A");
    expect(Array.from(f!.data)).toEqual([66]);
  });

  it("maps session updates to session, workspace, and home", () => {
    const msg = { type: "session_updated", summary: { id: "s1", workspace: "w1" } } as unknown as ServerMsg;
    expect(channelsFor(msg)).toEqual(["session:s1", "workspace:w1", "home"]);
  });

  it("maps file changes, tree patches and activity to the workspace channel", () => {
    const fs: WireMsg = { type: "project_fs_changed", workspace: "w1", key: "backend", paths: ["src/a.rs"] };
    const tree = { type: "tree_patch", workspace: "w1", session: { id: "s1" } } as unknown as WireMsg;
    const activity = { type: "activity", workspace: "w1", activity: { running: [], open_gates: [], spend_today_usd: 0, spend_week_usd: 0, today_since: "2026-09-24T00:00:00Z", week_since: "2026-09-18T00:00:00Z" } } as WireMsg;
    expect(channelsFor(fs)).toEqual(["workspace:w1"]);
    expect(channelsFor(tree)).toEqual(["workspace:w1"]);
    expect(channelsFor(activity)).toEqual(["workspace:w1"]);
  });

  it("delivers a workspace message to workspace subscribers only", () => {
    const { mgr, sockets } = setup();
    sockets[0].open();
    const got: string[] = [];
    mgr.subscribe("workspace:w1", (m) => got.push(m.type));
    mgr.subscribe("workspace:w2", (m) => got.push(`wrong ${m.type}`));
    mgr.subscribe("home", (m) => got.push(`home ${m.type}`));
    sockets[0].receive(JSON.stringify({ type: "project_fs_changed", workspace: "w1", key: "web", paths: [] }));
    sockets[0].receive(JSON.stringify({ type: "activity", workspace: "w1", activity: { running: [], open_gates: [], spend_today_usd: 0, spend_week_usd: 0 } }));
    expect(got).toEqual(["project_fs_changed", "activity"]);
  });
});
