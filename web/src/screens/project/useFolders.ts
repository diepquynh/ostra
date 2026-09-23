import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api";
import type { ProjectTreeEntry } from "../../api/types";
import { useProjectFsChanges } from "../../lib/live";
import { throttle } from "../../lib/store";

export type Folder = { entries: ProjectTreeEntry[] | null; error: string | null };

/**
 * Lazily loaded folder listings of one project, one request per folder (`tree?depth=1`). Listings are
 * dropped when the dotfiles setting changes and reloaded, at most twice a second, when files change on disk.
 */
export function useFolders(ws: string, key: string, hidden: boolean) {
  const cacheId = `${key}\n${hidden}`;
  const [cache, setCache] = useState<{ id: string; map: Record<string, Folder> }>({ id: cacheId, map: {} });
  const folders = cache.id === cacheId ? cache.map : {};
  const loading = useRef(new Set<string>());
  const idRef = useRef(cacheId);
  if (idRef.current !== cacheId) {
    idRef.current = cacheId;
    loading.current.clear();
  }

  const load = useCallback(
    (dir: string) => {
      const id = cacheId;
      loading.current.add(dir);
      const put = (fn: (prev: Folder | undefined) => Folder) =>
        setCache((c) => (c.id === id ? { id, map: { ...c.map, [dir]: fn(c.map[dir]) } } : { id, map: { [dir]: fn(undefined) } }));
      api.projectTree(ws, key, { path: dir, depth: 1, hidden }).then(
        (t) => {
          if (idRef.current !== id) return;
          loading.current.delete(dir);
          put(() => ({ entries: t.entries, error: null }));
        },
        (e: Error) => {
          if (idRef.current !== id) return;
          loading.current.delete(dir);
          put((prev) => ({ entries: prev?.entries ?? null, error: e.message }));
        },
      );
    },
    [ws, key, hidden, cacheId],
  );

  /** Load each folder that has no listing and no request in flight. */
  const ensure = useCallback(
    (dirs: string[]) => {
      for (const d of dirs) if (!folders[d] && !loading.current.has(d)) load(d);
    },
    [folders, load],
  );

  const loadedRef = useRef<() => void>(() => {});
  loadedRef.current = () => Object.keys(folders).forEach((d) => load(d));
  const refresh = useMemo(() => throttle(() => loadedRef.current(), 500), []);
  useEffect(() => refresh.cancel, [refresh]);
  useProjectFsChanges(ws, key, refresh);

  return { folders, ensure, reload: load };
}
