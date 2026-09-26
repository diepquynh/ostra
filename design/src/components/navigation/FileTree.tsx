import { type DragEvent, Fragment, type MouseEvent, type ReactNode, useState } from "react";
import type { IconName } from "../core/icons";
import { Spinner } from "../feedback/Spinner";
import { Input } from "../forms/Input";
import { TreeItem } from "./TreeItem";

const FILE_ICON: Record<string, IconName> = {
  rs: "file-code-2",
  ts: "file-code-2",
  tsx: "file-code-2",
  js: "file-code-2",
  jsx: "file-code-2",
  py: "file-code-2",
  go: "file-code-2",
  java: "file-code-2",
  kt: "file-code-2",
  sql: "database",
  toml: "file-cog",
  yaml: "file-cog",
  yml: "file-cog",
  json: "file-json",
  md: "file-text",
  txt: "file-text",
};

/** The icon for a file, by the extension of its last path segment. */
export function fileIcon(path: string): IconName {
  const name = path.split("/").pop() ?? "";
  const dot = name.lastIndexOf(".");
  return (dot > 0 && FILE_ICON[name.slice(dot + 1).toLowerCase()]) || "file";
}

/** The folder holding a `/`-separated path; empty at the root. */
export const parentDir = (path: string) => path.split("/").slice(0, -1).join("/");

export interface FileTreeEntry {
  name: string;
  /** Root-relative and `/`-separated. */
  path: string;
  is_dir: boolean;
}

/** One folder's listing: null entries while it loads. */
export interface FileTreeFolder<T extends FileTreeEntry> {
  entries: T[] | null;
  error: string | null;
}

/** How one row looks, beyond its name and icon. */
export interface FileTreeRow {
  color?: string;
  muted?: boolean;
  meta?: ReactNode;
  trailing?: ReactNode;
  title?: string;
}

export type FileTreeCreating = { kind: "file" | "folder"; dir: string };

export interface FileTreeProps<T extends FileTreeEntry> {
  /** The listing of a folder ("" is the root), or undefined when it is not loaded yet. */
  folder: (dir: string) => FileTreeFolder<T> | undefined;
  /** Open folders by path. */
  open: Record<string, boolean>;
  /** The path of the selected file. */
  selected?: string | null;
  onToggle: (entry: T, open: boolean) => void;
  onOpen: (entry: T) => void;
  onDragStart?: (entry: T, e: DragEvent<HTMLDivElement>) => void;
  /**
   * Whether a drag may drop on a row, and how: `copy` for files from the computer, `move` for rows of this tree, or
   * null to refuse. Only `dataTransfer.types` can be read while dragging.
   */
  dropEffect?: (e: DragEvent<HTMLDivElement>) => "copy" | "move" | null;
  /** A drop on a row: the folder it names, or the folder holding the file. The target folder row is highlighted. */
  onDrop?: (dir: string, e: DragEvent<HTMLDivElement>) => void;
  onContextMenu?: (entry: T, e: MouseEvent<HTMLDivElement>) => void;
  row?: (entry: T) => FileTreeRow;
  /** Shows the name field for a new file or folder at the top of `creating.dir`. */
  creating?: FileTreeCreating | null;
  onCreate?: (name: string) => Promise<void>;
  onCancelCreate?: () => void;
  /** Shown when the root holds nothing. */
  empty: ReactNode;
}

/** The drag carries files from the computer. */
export const dragHasFiles = (e: DragEvent) => Array.from(e.dataTransfer.types).includes("Files");

/** The inline name field for a new file or folder. Enter creates, Escape or an empty blur cancels. */
export function NewEntryField({
  creating,
  depth,
  onCreate,
  onCancel,
}: {
  creating: FileTreeCreating;
  depth: number;
  onCreate: (name: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = () => {
    const n = name.trim().replace(/^\/+|\/+$/g, "");
    if (!n) return onCancel();
    setBusy(true);
    onCreate(n).catch((e: Error) => {
      setError(e.message);
      setBusy(false);
    });
  };
  return (
    <div style={{ paddingLeft: 8 + depth * 14, paddingBlock: 2 }}>
      <Input
        size="sm"
        mono
        autoFocus
        icon={creating.kind === "file" ? "file-plus" : "folder-plus"}
        aria-label={creating.kind === "file" ? "New file name" : "New folder name"}
        placeholder={creating.kind === "file" ? "name.ext or folder/name.ext" : "folder or folder/sub"}
        value={name}
        disabled={busy}
        error={error ?? undefined}
        onChange={(e) => {
          setName(e.target.value);
          setError(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") submit();
          if (e.key === "Escape") onCancel();
        }}
        onBlur={() => !name.trim() && !busy && onCancel()}
      />
    </div>
  );
}

const note = (depth: number, color: string, children: ReactNode) => (
  <div
    style={{
      paddingLeft: 26 + depth * 14,
      minHeight: "var(--row-h)",
      display: "flex",
      alignItems: "center",
      gap: 6,
      color,
      fontSize: "var(--text-sm)",
    }}
  >
    {children}
  </div>
);

/**
 * The rows of a lazily loaded folder tree: folders first, then files, each sorted by name. It renders rows only; put
 * it inside an element with `role="tree"`. The caller loads each open folder and answers `folder`.
 */
export function FileTree<T extends FileTreeEntry>(props: FileTreeProps<T>) {
  const { folder, open, selected, onToggle, onOpen, onDragStart, dropEffect, onDrop, onContextMenu, row, creating } =
    props;
  /** The folder a drag would drop into. */
  const [over, setOver] = useState<string | null>(null);
  const drop = (dir: string) =>
    onDrop &&
    dropEffect && {
      onDragOver: (e: DragEvent<HTMLDivElement>) => {
        const effect = dropEffect(e);
        if (!effect) return;
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = effect;
        if (over !== dir) setOver(dir);
      },
      onDragLeave: (e: DragEvent<HTMLDivElement>) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(null);
      },
      onDrop: (e: DragEvent<HTMLDivElement>) => {
        setOver(null);
        if (!dropEffect(e)) return;
        e.preventDefault();
        e.stopPropagation();
        onDrop(dir, e);
      },
    };
  const target = { background: "var(--surface-active)", boxShadow: "inset 0 0 0 1px var(--accent)" };

  const renderDir = (dir: string, depth: number): ReactNode => {
    const f = folder(dir);
    if (!f || (!f.entries && !f.error))
      return note(
        depth,
        "var(--text-muted)",
        <>
          <Spinner size={10} /> Reading…
        </>,
      );
    if (f.error && !f.entries) return note(depth, "var(--bad)", f.error);
    const entries = [...(f.entries ?? [])].sort((a, b) =>
      a.is_dir === b.is_dir ? a.name.localeCompare(b.name) : a.is_dir ? -1 : 1,
    );
    const field = creating?.dir === dir && props.onCreate && props.onCancelCreate && (
      <NewEntryField
        key={`new-${creating.kind}`}
        creating={creating}
        depth={depth}
        onCreate={props.onCreate}
        onCancel={props.onCancelCreate}
      />
    );
    if (entries.length === 0 && depth === 0)
      return (
        field || (
          <div style={{ padding: "12px 8px", color: "var(--text-muted)", fontSize: "var(--text-sm)" }}>
            {props.empty}
          </div>
        )
      );
    return [
      field,
      ...entries.map((e) => {
        const look = row?.(e) ?? {};
        const label = (
          <span style={look.color ? { color: look.color } : look.muted ? { color: "var(--text-muted)" } : undefined}>
            {e.name}
          </span>
        );
        const common = {
          depth,
          label,
          meta: look.meta,
          trailing: look.trailing,
          title: look.title ?? e.path,
          onDragStart: onDragStart && ((ev: DragEvent<HTMLDivElement>) => onDragStart(e, ev)),
          onContextMenu:
            onContextMenu &&
            ((ev: MouseEvent<HTMLDivElement>) => {
              ev.preventDefault();
              ev.stopPropagation();
              onContextMenu(e, ev);
            }),
          ...drop(e.is_dir ? e.path : parentDir(e.path)),
          style: e.is_dir && over === e.path ? target : undefined,
          onDragEnd: () => setOver(null),
        };
        if (e.is_dir) {
          const isOpen = !!open[e.path];
          return (
            <Fragment key={e.path}>
              <TreeItem
                {...common}
                icon={isOpen ? "folder-open" : "folder"}
                expanded={isOpen}
                onToggle={() => onToggle(e, isOpen)}
              />
              {isOpen && renderDir(e.path, depth + 1)}
            </Fragment>
          );
        }
        return (
          <TreeItem
            key={e.path}
            {...common}
            icon={fileIcon(e.name)}
            selected={selected === e.path}
            onClick={() => onOpen(e)}
          />
        );
      }),
    ];
  };

  return <>{renderDir("", 0)}</>;
}
