import { type KeyboardEvent, useEffect, useMemo, useState } from "react";
import { Button } from "../core/Button";
import { Icon } from "../core/Icon";
import { Kbd } from "../core/Kbd";
import { Chip } from "../feedback/Chip";
import { Spinner } from "../feedback/Spinner";

export interface FsEntry {
  name: string;
  is_dir?: boolean;
  is_git?: boolean;
  is_ostra_project?: boolean;
}

/** One directory listing, shaped like `GET /api/fs?path=` (backend proposal 19). */
export interface FolderListing {
  /** The listed directory, absolute, with "~" expanded. */
  path: string;
  /** Parent of path, or null at the root. */
  parent?: string | null;
  entries: FsEntry[];
  /** False when path does not exist; the picker shows an error row and disables "Use this folder". */
  exists?: boolean;
  readable?: boolean;
  /** Deepest existing ancestor of a missing path. */
  nearest?: string | null;
  /** Home directory for "~" expansion. */
  home?: string;
}

/** Lists a directory. `path` may start with "~"; the server expands it. */
export type FolderLister = (path: string, init?: { signal?: AbortSignal }) => Promise<FolderListing>;

export interface FolderPickerProps {
  /** The path field. Doubles as the selection: a value without a trailing "/" is the chosen folder. */
  value: string;
  onChange?: (path: string) => void;
  /**
   * Loads listings. With it, the picker manages browsing itself: it lists the directory part of whatever is
   * typed, debounced. Without it, pass browsePath, entries, parent, missing and onBrowse (controlled browsing).
   */
  list?: FolderLister;
  /** With list: the directory shown while the field is empty. Defaults to "~". */
  initialPath?: string;
  /** With list: debounce before a listing request. */
  debounceMs?: number;
  /** Controlled browsing: the directory currently listed. The picker calls onBrowse with the directory part of what is typed. */
  browsePath?: string;
  /** Controlled browsing: subdirectories of browsePath. The picker filters them by the typed name after the last "/". */
  entries?: FsEntry[];
  /** Controlled browsing: parent of browsePath, or null at the root. */
  parent?: string | null;
  /** Controlled browsing: browsePath does not exist; shows an error row and disables "Use this folder". */
  missing?: boolean;
  /** Controlled browsing: deepest existing ancestor of a missing browsePath. */
  nearest?: string | null;
  onBrowse?: (path: string) => void;
  /** Home directory for "~" expansion. With list, the listing's home is used when this is omitted. */
  home?: string;
  /** Creates a folder and its missing parents, like `mkdir -p`. With it, a typed path that does not exist offers "Create". */
  mkdir?: (path: string) => Promise<unknown>;
  height?: number;
}

interface Parsed {
  dir: string;
  prefix: string;
}

const DRIVE = /^[A-Za-z]:/;
const DRIVE_NAME = /^[A-Za-z]:$/;
const DRIVE_ROOT = /^[A-Za-z]:[\\/]$/;

/**
 * A Windows path: it starts with a drive letter, or with "~" when home is a Windows path. Both "\" and "/" separate
 * its parts. On a Windows server, "/" is the list of drives.
 */
export const isWindowsPath = (p: string, home?: string) =>
  DRIVE.test(p) || p.startsWith("~\\") || (p.startsWith("~") && !!home && DRIVE.test(home));

/** Absolute, or starting with "~" and a separator: a path the server resolves the same way from any folder. */
export const isAbsolutePath = (p: string) =>
  p.startsWith("/") || p.startsWith("~/") || p.startsWith("~\\") || /^[A-Za-z]:[\\/]/.test(p);

const lastSep = (p: string, win: boolean) =>
  win ? Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")) : p.lastIndexOf("/");

/** A trailing separator marks a folder being browsed rather than chosen. */
export const endsWithSep = (p: string, home?: string) =>
  p.endsWith("/") || (isWindowsPath(p, home) && p.endsWith("\\"));

/** The path without trailing separators, keeping a root ("/" or "C:\"). */
export function trimSep(p: string, home?: string): string {
  let v = p;
  while (v.length > 1 && endsWithSep(v, home) && !DRIVE_ROOT.test(v)) v = v.slice(0, -1);
  return v;
}

/** The path with one trailing separator: "/" for POSIX, and for Windows the one the path already uses. */
export function withSep(p: string, home?: string): string {
  if (endsWithSep(p, home)) return p;
  if (!isWindowsPath(p, home)) return `${p}/`;
  return !p.includes("\\") && p.slice(2).includes("/") ? `${p}/` : `${p}\\`;
}

/** The last part of a path, without trailing separators. */
export function baseName(p: string, home?: string): string {
  const v = trimSep(p, home);
  return v.slice(lastSep(v, isWindowsPath(v, home)) + 1);
}

/** Split a typed path into the directory to list and the name prefix to filter by. Null when it is not absolute. */
export function splitPath(typed: string, home?: string): Parsed | null {
  const v = home && typed.startsWith("~") ? home + typed.slice(1) : typed;
  const win = isWindowsPath(v, home);
  // Without a known home, "~" paths are listed as typed and the server expands them.
  if (!v.startsWith("/") && !v.startsWith("~") && !win) return null;
  // A bare drive such as "C:" filters the drive list.
  if (DRIVE_NAME.test(v)) return { dir: "/", prefix: v };
  const i = lastSep(v, win);
  if (i < 0) return { dir: v, prefix: "" };
  const dir = i === 0 ? "/" : win && i === 2 ? v.slice(0, 3) : v.slice(0, i);
  return { dir, prefix: v.slice(i + 1) };
}

/** A drive in the list opens as its root; other names join with the directory's own separator. */
const join = (dir: string, name: string) => (dir === "/" && DRIVE_NAME.test(name) ? `${name}\\` : withSep(dir) + name);

/** The folder above, with a trailing separator; a drive root goes up to the drive list. */
function parentOf(p: string): string {
  if (DRIVE_NAME.test(p) || DRIVE_ROOT.test(p)) return "/";
  const win = isWindowsPath(p);
  const t = trimSep(p);
  const i = lastSep(t, win);
  if (i <= 0) return "/";
  return win && i === 2 ? t.slice(0, 3) : t.slice(0, i + 1);
}

interface Browse {
  requested: string;
  current: string;
  entries: FsEntry[];
  parent: string | null;
  missing: boolean;
  nearest: string | null;
  home?: string;
  loading: boolean;
  error: string | null;
  browse: (dir: string) => void;
  /** Lists the current directory again. */
  reload: () => void;
}

function useListing(list: FolderLister | undefined, start: string, debounceMs: number): Browse {
  const [requested, setRequested] = useState(start);
  const [listing, setListing] = useState<FolderListing | null>(null);
  const [loadedFor, setLoadedFor] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    if (!list) return;
    const ctrl = new AbortController();
    const t = setTimeout(() => {
      list(requested, { signal: ctrl.signal }).then(
        (l) => {
          if (ctrl.signal.aborted) return;
          setListing(l);
          setError(null);
          setLoadedFor(requested);
        },
        (e: unknown) => {
          if (ctrl.signal.aborted) return;
          setError(e instanceof Error ? e.message : String(e));
          setLoadedFor(requested);
        },
      );
    }, debounceMs);
    return () => {
      clearTimeout(t);
      ctrl.abort();
    };
  }, [list, requested, debounceMs, nonce]);

  const stale = loadedFor !== requested;
  return {
    requested,
    current: error && !stale ? requested : (listing?.path ?? requested),
    entries: error && !stale ? [] : (listing?.entries ?? []),
    parent: listing?.parent ?? null,
    missing: !stale && (!!error || listing?.exists === false || listing?.readable === false),
    nearest: listing?.nearest ?? null,
    home: listing?.home,
    loading: stale,
    error: stale ? null : error,
    browse: setRequested,
    reload: () => {
      setLoadedFor(null);
      setNonce((n) => n + 1);
    },
  };
}

/**
 * Pick a directory on the server. The path field drives the listing: typing browses the folder before the last "/"
 * and filters it by what follows. Tab completes, arrows pick a match, Enter uses the folder, Backspace after "/" goes up.
 */
export function FolderPicker(props: FolderPickerProps) {
  const { value, onChange, list, initialPath = "~", debounceMs = 120, height = 220 } = props;
  const listed = useListing(list, splitPath(value, props.home)?.dir ?? initialPath, debounceMs);
  const b: Browse = list
    ? listed
    : {
        requested: props.browsePath ?? "/",
        current: props.browsePath ?? "/",
        entries: props.entries ?? [],
        parent: props.parent ?? null,
        missing: !!props.missing,
        nearest: props.nearest ?? null,
        loading: false,
        error: null,
        browse: (dir) => props.onBrowse?.(dir),
        reload: () => props.onBrowse?.(props.browsePath ?? "/"),
      };
  const home = props.home ?? b.home;
  const [hi, setHi] = useState(0);
  const parsed = splitPath(value, home);
  const prefix = parsed ? parsed.prefix.toLowerCase() : "";
  const dir = parsed?.dir;

  // Typing an absolute path browses its directory part.
  useEffect(() => {
    if (dir !== undefined && dir !== b.requested) b.browse(dir);
    setHi(0);
  }, [dir, prefix]);

  const shown = useMemo(() => {
    const dirs = b.entries.filter((e) => e.is_dir !== false);
    if (!prefix) return dirs;
    const starts = dirs.filter((e) => e.name.toLowerCase().startsWith(prefix));
    const has = dirs.filter((e) => !e.name.toLowerCase().startsWith(prefix) && e.name.toLowerCase().includes(prefix));
    return [...starts, ...has];
  }, [b.entries, prefix]);

  const current = b.current;
  const winHome = !!home && isWindowsPath(home);
  const enter = (name: string) => onChange?.(withSep(join(current, name)));
  const selected = trimSep(value, home) || "/";
  const isSelected = !!parsed && !prefix && parsed.dir === current;
  const showSelected = isSelected && !endsWithSep(value, home);

  // The typed folder, when it does not exist yet: the missing directory itself, or a name no entry matches exactly.
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  const target = parsed ? (parsed.prefix ? join(parsed.dir, parsed.prefix) : parsed.dir) : null;
  const canCreate =
    !!props.mkdir &&
    !!target &&
    !b.loading &&
    !b.error &&
    (b.missing ? b.nearest !== null : !!parsed?.prefix && !b.entries.some((e) => e.name === parsed.prefix));
  useEffect(() => setCreateError(null), [target]);
  const create = () => {
    if (!props.mkdir || !target || creating) return;
    setCreating(true);
    setCreateError(null);
    props.mkdir(target).then(
      () => {
        setCreating(false);
        onChange?.(withSep(target));
        b.reload();
      },
      (e: unknown) => {
        setCreating(false);
        setCreateError(e instanceof Error ? e.message : String(e));
      },
    );
  };

  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setHi((x) => Math.min(x + 1, shown.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHi((x) => Math.max(x - 1, 0));
    } else if (e.key === "Tab" && !e.shiftKey && shown[hi] && prefix) {
      e.preventDefault();
      enter(shown[hi].name);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (shown[hi] && prefix) enter(shown[hi].name);
      else if (!b.missing) onChange?.(current);
    } else if (e.key === "Backspace" && parsed && !prefix && endsWithSep(value, home) && value.length > 1) {
      e.preventDefault();
      onChange?.(parentOf(current));
    }
  };

  return (
    <div className="os-field">
      <div className={`os-input os-input--mono ${value && !parsed ? "os-input--invalid" : ""}`}>
        <Icon name="folder" size={14} className="os-input__icon" />
        <input
          value={value}
          placeholder={winHome ? "C:\\path\\to\\folder" : "/absolute/path/to/folder"}
          spellCheck={false}
          autoComplete="off"
          aria-label="Folder path"
          aria-invalid={value && !parsed ? true : undefined}
          onChange={(e) => onChange?.(e.target.value)}
          onKeyDown={onKey}
        />
        {prefix && shown[0] && (
          <span
            style={{
              display: "flex",
              alignItems: "center",
              gap: 4,
              fontSize: "var(--text-xs)",
              color: "var(--text-muted)",
              whiteSpace: "nowrap",
            }}
          >
            <Kbd>Tab</Kbd> {shown[hi] ? shown[hi].name : ""}
          </span>
        )}
      </div>
      {value && !parsed && (
        <span className="os-field__error">
          {winHome
            ? "Type an absolute path, such as C:\\Users\\you\\code or ~\\code."
            : "Type an absolute path, starting with / or ~/."}
        </span>
      )}
      <div className="os-picker">
        <div className="os-picker__bar">
          <Icon
            name={b.missing ? "folder-x" : "folder-open"}
            size={13}
            style={{ color: b.missing ? "var(--bad)" : "var(--text-muted)" }}
          />
          <span className="os-picker__path" title={current}>
            {current}
          </span>
          {b.loading && list && <Spinner size={11} style={{ color: "var(--text-muted)" }} />}
          <Button
            size="sm"
            variant={showSelected ? "default" : "primary"}
            disabled={b.missing}
            onClick={() => onChange?.(current)}
          >
            {showSelected ? "Selected" : "Use this folder"}
          </Button>
        </div>
        <div className="os-picker__list" style={{ maxHeight: height }}>
          {b.missing && (
            <div style={{ padding: "14px 10px", color: "var(--bad)", fontSize: "var(--text-sm)" }}>
              {b.error ? `Could not list ${current}: ${b.error}` : `No folder at ${current}.`}
            </div>
          )}
          {b.missing && b.nearest && (
            <div
              className="os-picker__row"
              title="Open the nearest folder that exists"
              onClick={() => b.nearest && onChange?.(withSep(b.nearest))}
            >
              <Icon name="corner-left-up" size={14} style={{ color: "var(--text-muted)" }} />
              <span style={{ color: "var(--text-secondary)" }}>{b.nearest}</span>
            </div>
          )}
          {canCreate && (
            <div className="os-picker__row" title="Create this folder and any missing parent folders" onClick={create}>
              {creating ? (
                <Spinner size={12} />
              ) : (
                <Icon name="folder-plus" size={14} style={{ color: "var(--accent-fg)" }} />
              )}
              <span style={{ flex: 1 }}>
                {creating ? "Creating " : "Create "}
                <span style={{ fontFamily: "var(--font-mono)" }}>{target}</span>
              </span>
            </div>
          )}
          {createError && (
            <div style={{ padding: "8px 10px", color: "var(--bad)", fontSize: "var(--text-sm)" }}>{createError}</div>
          )}
          {!b.missing && b.parent && !prefix && (
            <div className="os-picker__row" onClick={() => b.parent && onChange?.(withSep(b.parent))}>
              <Icon name="corner-left-up" size={14} style={{ color: "var(--text-muted)" }} />
              <span style={{ color: "var(--text-secondary)" }}>..</span>
            </div>
          )}
          {!b.missing &&
            shown.map((e, i) => {
              const path = join(current, e.name);
              const n = prefix && e.name.toLowerCase().startsWith(prefix) ? prefix.length : 0;
              return (
                <div
                  key={e.name}
                  className={`os-picker__row ${(prefix && i === hi) || selected === path ? "os-picker__row--selected" : ""}`}
                  onMouseEnter={() => prefix && setHi(i)}
                  onClick={() => enter(e.name)}
                  onDoubleClick={() => onChange?.(path)}
                  title="Click to open, double-click to select"
                >
                  <Icon
                    name={e.is_git ? "folder-git-2" : "folder"}
                    size={14}
                    style={{ color: e.is_ostra_project ? "var(--accent-fg)" : "var(--text-muted)" }}
                  />
                  <span style={{ flex: 1 }}>
                    {n ? (
                      <>
                        <strong style={{ fontWeight: 600, color: "var(--text-primary)" }}>{e.name.slice(0, n)}</strong>
                        {e.name.slice(n)}
                      </>
                    ) : (
                      e.name
                    )}
                  </span>
                  {e.is_git && <Chip mono>git</Chip>}
                  {e.is_ostra_project && <Chip tone="ok">Ostra project</Chip>}
                </div>
              );
            })}
          {!b.missing && !b.loading && shown.length === 0 && (
            <div style={{ padding: "14px 10px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
              {prefix && parsed ? `No folder starts with “${parsed.prefix}”.` : "No folders here."}
            </div>
          )}
        </div>
      </div>
      <span className="os-field__hint">
        Type a path to jump there. <Kbd>Tab</Kbd> completes, <Kbd>↑</Kbd>
        <Kbd>↓</Kbd> pick a match, <Kbd>Enter</Kbd> uses the folder, <Kbd>⌫</Kbd> after a “{winHome ? "\\" : "/"}” goes
        up.
      </span>
    </div>
  );
}
