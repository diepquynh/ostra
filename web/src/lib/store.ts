import { useCallback, useSyncExternalStore } from "react";
import { socket } from "../api";

export type Snapshot<T> = { data: T | null; error: Error | null; loading: boolean };

/** What a store definition may do with its entry while it has subscribers. */
export interface Feed<T> {
  get: () => T | null;
  /** Replace the data. Return the previous value to leave it unchanged. */
  update: (fn: (prev: T | null) => T | null) => void;
  reload: () => void;
}

export interface StoreDef<T> {
  load: (key: string, feed: Feed<T>) => Promise<T>;
  /** Live updates while the entry has subscribers. Returns the cleanup. */
  live?: (key: string, feed: Feed<T>) => () => void;
}

type Entry<T> = {
  snap: Snapshot<T>;
  listeners: Set<() => void>;
  running: boolean;
  gen: number;
  cleanup: (() => void) | null;
  stopTimer: ReturnType<typeof setTimeout> | null;
};

const EMPTY: Snapshot<never> = { data: null, error: null, loading: false };

/**
 * Data shared by every component that reads the same key: one fetch, one socket subscription, and
 * a reload after each reconnect. The last value stays cached after the last reader leaves, so a
 * screen that comes back renders at once and refreshes in the background.
 */
export class SharedStore<T> {
  private entries = new Map<string, Entry<T>>();
  constructor(private def: StoreDef<T>) {}

  snapshot(key: string): Snapshot<T> {
    return this.entry(key).snap;
  }

  peek(key: string): T | null {
    return this.entries.get(key)?.snap.data ?? null;
  }

  subscribe(key: string, listener: () => void): () => void {
    const e = this.entry(key);
    e.listeners.add(listener);
    if (e.stopTimer) {
      clearTimeout(e.stopTimer);
      e.stopTimer = null;
    }
    if (!e.running) this.start(key, e);
    return () => {
      e.listeners.delete(listener);
      if (e.listeners.size > 0 || e.stopTimer) return;
      // Deferred so a remount in the same tick (StrictMode, tab switches) keeps the subscription.
      e.stopTimer = setTimeout(() => {
        e.stopTimer = null;
        if (e.listeners.size === 0) this.stop(e);
      }, 0);
    };
  }

  reload(key: string): void {
    const e = this.entries.get(key);
    if (e?.running) this.load(key, e);
  }

  /** Forget every cached entry. Tests call this between cases. */
  clear(): void {
    this.entries.forEach((e) => this.stop(e));
    this.entries.clear();
  }

  private entry(key: string): Entry<T> {
    let e = this.entries.get(key);
    if (!e) {
      e = { snap: { data: null, error: null, loading: true }, listeners: new Set(), running: false, gen: 0, cleanup: null, stopTimer: null };
      this.entries.set(key, e);
    }
    return e;
  }

  private set(e: Entry<T>, snap: Snapshot<T>) {
    e.snap = snap;
    e.listeners.forEach((fn) => fn());
  }

  private feed(key: string, e: Entry<T>): Feed<T> {
    return {
      get: () => e.snap.data,
      update: (fn) => {
        const next = fn(e.snap.data);
        if (next !== e.snap.data) this.set(e, { ...e.snap, data: next });
      },
      reload: () => this.load(key, e),
    };
  }

  private start(key: string, e: Entry<T>) {
    e.running = true;
    const feed = this.feed(key, e);
    const offLive = this.def.live?.(key, feed);
    const offReconnect = socket().onReconnect(() => this.load(key, e));
    e.cleanup = () => {
      offLive?.();
      offReconnect();
    };
    this.load(key, e);
  }

  private stop(e: Entry<T>) {
    e.running = false;
    e.gen += 1;
    e.cleanup?.();
    e.cleanup = null;
  }

  private load(key: string, e: Entry<T>) {
    const gen = ++e.gen;
    if (!e.snap.loading) this.set(e, { ...e.snap, loading: true });
    this.def.load(key, this.feed(key, e)).then(
      (data) => gen === e.gen && this.set(e, { data, error: null, loading: false }),
      (err: unknown) => gen === e.gen && this.set(e, { ...e.snap, error: err instanceof Error ? err : new Error(String(err)), loading: false }),
    );
  }
}

/** Read a shared store entry. A null key reads nothing. */
export function useShared<T>(store: SharedStore<T>, key: string | null): Snapshot<T> & { reload: () => void } {
  const subscribe = useCallback((cb: () => void) => (key === null ? () => {} : store.subscribe(key, cb)), [store, key]);
  const get = useCallback(() => (key === null ? (EMPTY as Snapshot<T>) : store.snapshot(key)), [store, key]);
  const snap = useSyncExternalStore(subscribe, get, get);
  const reload = useCallback(() => key !== null && store.reload(key), [store, key]);
  return { ...snap, reload };
}

/** Call `fn` at most once per `ms`, with a trailing call. */
export function throttle(fn: () => void, ms: number): (() => void) & { cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pending = false;
  const run = () => {
    if (timer) {
      pending = true;
      return;
    }
    fn();
    timer = setTimeout(function tick() {
      if (pending) {
        pending = false;
        fn();
        timer = setTimeout(tick, ms);
      } else timer = null;
    }, ms);
  };
  run.cancel = () => {
    if (timer) clearTimeout(timer);
    timer = null;
    pending = false;
  };
  return run;
}
