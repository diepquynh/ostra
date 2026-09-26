import type { FolderLister } from "@ostra/design";
import { useEffect, useState } from "react";
import { api } from "../../api";
import type { Api } from "../../api/client";
import { isChosen } from "./wizard";

/** The FolderPicker's `list` over `GET /api/fs`. The picker debounces and aborts; the signal cancels the request. */
export function makeLister(browse: Api["fsBrowse"]): FolderLister {
  return (path, init) =>
    browse({ path }, { signal: init?.signal }).then((b) => ({
      path: b.path,
      parent: b.parent,
      entries: b.entries,
      exists: b.exists,
      readable: b.readable,
      nearest: b.nearest,
      home: b.home,
    }));
}

export const listFolders: FolderLister = makeLister((opts, init) => api.fsBrowse(opts, init));

/** The FolderPicker's `mkdir` over `POST /api/fs/mkdir`, which creates missing parents too. */
export const makeFolder = (path: string) => api.fsMkdir(path);

export type FolderInfo =
  | { state: "none" }
  | { state: "checking" }
  | { state: "exists"; isGit: boolean; isOstraProject: boolean }
  | { state: "missing" };

/**
 * Whether a chosen folder exists, and whether it is a git checkout or an Ostra project: one `GET /api/fs` of the
 * folder itself, debounced like the picker's own requests. A file or an unreadable folder counts as missing.
 */
export function useFolderInfo(path: string, debounceMs = 120): FolderInfo {
  const [info, setInfo] = useState<{ for: string; info: FolderInfo } | null>(null);
  const chosen = isChosen(path);
  useEffect(() => {
    if (!chosen) return;
    const ctrl = new AbortController();
    const t = setTimeout(() => {
      api.fsBrowse({ path: path.replace(/\/+$/, ""), limit: 1 }, { signal: ctrl.signal }).then(
        (b) => {
          if (ctrl.signal.aborted) return;
          setInfo({
            for: path,
            info:
              b.exists && b.readable
                ? { state: "exists", isGit: b.is_git, isOstraProject: b.is_ostra_project }
                : { state: "missing" },
          });
        },
        () => {
          if (!ctrl.signal.aborted) setInfo({ for: path, info: { state: "missing" } });
        },
      );
    }, debounceMs);
    return () => {
      clearTimeout(t);
      ctrl.abort();
    };
  }, [path, chosen, debounceMs]);
  if (!chosen) return { state: "none" };
  return info?.for === path ? info.info : { state: "checking" };
}

let homeCache: Promise<string | null> | null = null;

/** The server's home folder, so "~/" paths can be expanded before they are sent. */
export function useHome(): string | null {
  const [home, setHome] = useState<string | null>(null);
  useEffect(() => {
    homeCache ??= api.fsBrowse({ path: "~", limit: 1 }).then(
      (b) => b.home,
      () => {
        homeCache = null;
        return null;
      },
    );
    let live = true;
    void homeCache.then((h) => live && setHome(h));
    return () => {
      live = false;
    };
  }, []);
  return home;
}
