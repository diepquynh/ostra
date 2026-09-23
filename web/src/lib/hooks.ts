import { useCallback, useEffect, useRef, useState } from "react";
import { socket } from "../api";
import type { WireMsg } from "../api/socket";

export type Async<T> = { data: T | null; error: Error | null; loading: boolean; reload: () => void; set: (v: T) => void };

/** Load data with a promise factory. Reloads when `deps` change and when the socket reconnects. */
export function useAsync<T>(load: () => Promise<T>, deps: unknown[]): Async<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    loadRef
      .current()
      .then((v) => {
        if (!cancelled) {
          setData(v);
          setError(null);
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e : new Error(String(e)));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);

  useEffect(() => socket().onReconnect(() => setTick((t) => t + 1)), []);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  return { data, error, loading, reload, set: setData };
}

/** Subscribe to a WebSocket channel for the component's lifetime. */
export function useChannel(channel: string | null, handler: (msg: WireMsg) => void): void {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    if (!channel) return;
    return socket().subscribe(channel, (m) => ref.current(m));
  }, [channel]);
}

/** Call `fn` at most once per `ms`, with a trailing call. */
export function useThrottled(fn: () => void, ms: number): () => void {
  const ref = useRef(fn);
  ref.current = fn;
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef(false);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);
  return useCallback(() => {
    if (timer.current) {
      pending.current = true;
      return;
    }
    ref.current();
    timer.current = setTimeout(function tick() {
      if (pending.current) {
        pending.current = false;
        ref.current();
        timer.current = setTimeout(tick, ms);
      } else {
        timer.current = null;
      }
    }, ms);
  }, [ms]);
}

/** A boolean persisted in localStorage. */
export function useStoredFlag(key: string, initial: boolean): [boolean, (v: boolean) => void] {
  const [value, setValue] = useState<boolean>(() => {
    try {
      const raw = localStorage.getItem(key);
      return raw === null ? initial : raw === "1";
    } catch {
      return initial;
    }
  });
  const set = useCallback(
    (v: boolean) => {
      setValue(v);
      try {
        localStorage.setItem(key, v ? "1" : "0");
      } catch {
        // Storage may be unavailable; the flag then lasts for this page only.
      }
    },
    [key],
  );
  return [value, set];
}
