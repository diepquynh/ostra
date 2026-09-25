import { type KeyboardEvent, type Ref, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api } from "../../api";
import type { ContextFile } from "../../api/types";
import { Icon } from "../../design";
import { cx } from "../../design/cx";
import {
  baseName,
  CONTEXT_DRAG_TYPE,
  fileKey,
  filesInText,
  isFolder,
  rankFiles,
  readContextDrag,
  splitTags,
  tagOf,
  withFolders,
} from "./tags";
import { type PendingUpload, PendingUploadChip, UploadButton } from "./uploads";
import "./context.css";

/** Every file of the given projects and the folders holding them, loaded once the first `@` is typed. */
export function useProjectFiles(ws: string, projects: string[]) {
  const [wanted, setWanted] = useState(false);
  const [files, setFiles] = useState<ContextFile[] | null>(null);
  const key = projects.join("\n");
  // biome-ignore lint/correctness/useExhaustiveDependencies: `key` stands for `projects`.
  useEffect(() => {
    if (!wanted) return;
    let live = true;
    Promise.all(
      projects.map((p) =>
        api.projectFiles(ws, p).then(
          (ix) => ix.paths.map((path) => ({ project: p, path })),
          () => [] as ContextFile[],
        ),
      ),
    ).then((lists) => live && setFiles(withFolders(lists.flat())));
    return () => {
      live = false;
    };
  }, [ws, key, wanted]);
  const known = useMemo(() => (files ? new Set(files.map(fileKey)) : null), [files]);
  return { files, known, load: () => setWanted(true) };
}

// A zero-width space keeps a trailing line break visible; it never reaches the value.
const ZWSP = "\u200b";

function chipNode(f: ContextFile): HTMLElement {
  const chip = document.createElement("span");
  chip.className = `ctx-chip ctx-chip--inline${isFolder(f) ? " ctx-chip--folder" : ""}`;
  chip.contentEditable = "false";
  chip.dataset.project = f.project;
  chip.dataset.path = f.path;
  chip.title = tagOf(f);
  const name = document.createElement("span");
  name.className = "ctx-chip__open";
  name.textContent = baseName(f.path);
  const remove = document.createElement("button");
  remove.type = "button";
  remove.tabIndex = -1;
  remove.className = "ctx-chip__remove";
  remove.setAttribute("aria-label", `Remove ${tagOf(f)}`);
  remove.textContent = "×";
  chip.append(name, remove);
  return chip;
}

/** The editor's content as text, each chip as its `@project/path` tag. */
function serialize(node: Node, root = true): string {
  let out = "";
  node.childNodes.forEach((c) => {
    if (c.nodeType === Node.TEXT_NODE) {
      out += (c.textContent ?? "").replaceAll(ZWSP, "");
    } else if (c instanceof HTMLElement) {
      if (c.dataset.path && c.dataset.project) out += tagOf({ project: c.dataset.project, path: c.dataset.path });
      // A last `<br>` only holds the empty last line open in the browser.
      else if (c.tagName === "BR") out += root && c === node.lastChild ? "" : "\n";
      else if (c.tagName === "DIV" || c.tagName === "P") {
        if (out && !out.endsWith("\n")) out += "\n";
        out += serialize(c, false);
      } else out += serialize(c, false);
    }
  });
  return out;
}

function render(el: HTMLElement, text: string, projects: string[], known: Set<string> | null) {
  const nodes: Node[] = splitTags(text, projects, known).map((s) =>
    "file" in s ? chipNode(s.file) : document.createTextNode(s.text),
  );
  el.replaceChildren(...nodes);
}

type Query = { node: Text; start: number; end: number; query: string };

/** The `@query` typed just before the caret, when the caret is in a text node of `el`. */
function caretQuery(el: HTMLElement): Query | null {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0 || !sel.isCollapsed) return null;
  const node = sel.anchorNode;
  if (!node || node.nodeType !== Node.TEXT_NODE || !el.contains(node)) return null;
  const before = (node.textContent ?? "").slice(0, sel.anchorOffset);
  const m = /(^|\s)@([^\s@]*)$/.exec(before);
  if (!m) return null;
  return { node: node as Text, start: sel.anchorOffset - m[2].length - 1, end: sel.anchorOffset, query: m[2] };
}

/** Where the file list opens: under the caret's line, or above it when the viewport ends first. */
function listPosition(q: Query): { left: number; top?: number; bottom?: number } | null {
  const range = document.createRange();
  range.setStart(q.node, q.start);
  range.setEnd(q.node, q.end);
  const rect = range.getBoundingClientRect?.();
  if (!rect || (rect.width === 0 && rect.height === 0 && rect.top === 0)) return null;
  const left = Math.max(8, Math.min(rect.left, window.innerWidth - 528));
  return window.innerHeight - rect.bottom < 300
    ? { left, bottom: window.innerHeight - rect.top + 4 }
    : { left, top: rect.bottom + 4 };
}

function placeCaret(node: Node, offset: number) {
  const sel = window.getSelection();
  if (!sel) return;
  const r = document.createRange();
  r.setStart(node, offset);
  r.collapse(true);
  sel.removeAllRanges();
  sel.addRange(r);
}

/** The caret range inside `el`, or a range at its end. */
function caretRange(el: HTMLElement): Range {
  const sel = window.getSelection();
  if (sel && sel.rangeCount > 0 && el.contains(sel.getRangeAt(0).startContainer)) return sel.getRangeAt(0);
  const r = document.createRange();
  r.selectNodeContents(el);
  r.collapse(false);
  return r;
}

export type FileTagInputProps = {
  ws: string;
  /** Projects whose files may be tagged. */
  projects: string[];
  /** Text with each tagged file as `@project/path`. */
  value: string;
  onChange: (text: string) => void;
  /** Called with the tagged files whenever they change. */
  onFiles?: (files: ContextFile[]) => void;
  onKeyDown?: (e: KeyboardEvent<HTMLDivElement>) => void;
  placeholder?: string;
  rows?: number;
  disabled?: boolean;
  label?: string;
  ref?: Ref<HTMLDivElement>;
  /** Files picked, dropped, or pasted for this request, shown inside the field until it is sent. */
  uploads?: PendingUpload[];
  /** Turns on uploading: the Upload a file button, and files dropped or pasted on the field. */
  onUploadFiles?: (files: File[]) => void;
  onRemoveUpload?: (key: string) => void;
};

/**
 * Request text with `@` file tagging. Typing `@` lists files of the projects under the caret;
 * picking one puts the file in the text as a chip. The value keeps the tag as `@project/path`.
 */
export function FileTagInput({
  ws,
  projects,
  value,
  onChange,
  onFiles,
  onKeyDown,
  placeholder,
  rows = 3,
  disabled,
  label,
  ref,
  uploads = [],
  onUploadFiles,
  onRemoveUpload,
}: FileTagInputProps) {
  const [dropping, setDropping] = useState(false);
  const el = useRef<HTMLDivElement | null>(null);
  const { files, known, load } = useProjectFiles(ws, projects);
  const [q, setQ] = useState<Query | null>(null);
  const [pos, setPos] = useState<ReturnType<typeof listPosition>>(null);
  const [active, setActive] = useState(0);
  const matches = useMemo(() => (q && files ? rankFiles(files, q.query) : []), [q, files]);
  const tagged = useMemo(() => filesInText(value, projects, known), [value, projects, known]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: report only when the tagged set changes.
  useEffect(() => onFiles?.(tagged), [tagged.map(fileKey).join("\n")]);
  useEffect(() => {
    if (q) load();
  }, [q, load]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new query starts at the first match.
  useEffect(() => setActive(0), [q?.query]);

  // The DOM is the source while typing; it is rebuilt only when the value changes from outside.
  useLayoutEffect(() => {
    const node = el.current;
    if (node && serialize(node) !== value) render(node, value, projects, known);
  }, [value, projects, known]);

  const setRef = (node: HTMLDivElement | null) => {
    el.current = node;
    if (typeof ref === "function") ref(node);
    else if (ref) (ref as { current: HTMLDivElement | null }).current = node;
  };

  const sync = () => {
    const node = el.current;
    if (!node) return;
    onChange(serialize(node));
    refreshQuery();
  };

  const refreshQuery = () => {
    const node = el.current;
    const next = node ? caretQuery(node) : null;
    setQ(next);
    setPos(next ? listPosition(next) : null);
  };

  const insertText = (text: string) => {
    const node = el.current;
    if (!node) return;
    const range = caretRange(node);
    range.deleteContents();
    const t = document.createTextNode(text);
    range.insertNode(t);
    let end = text.length;
    if (text.endsWith("\n") && !t.nextSibling) {
      t.textContent = text + ZWSP;
      end = text.length;
    }
    placeCaret(t, end);
    sync();
  };

  /** Browser editing commands keep the caret and undo right; jsdom lacks them. */
  const command = (name: string, arg?: string) =>
    typeof document.execCommand === "function" && document.execCommand(name, false, arg);

  const pick = (f: ContextFile) => {
    const node = el.current;
    if (!q || !node) return;
    const range = document.createRange();
    range.setStart(q.node, q.start);
    range.setEnd(q.node, q.end);
    range.deleteContents();
    const space = document.createTextNode(" ");
    range.insertNode(space);
    range.insertNode(chipNode(f));
    placeCaret(space, 1);
    node.focus();
    sync();
  };

  /** Put a chip where a Files panel row was dropped, or at the caret. */
  const dropFile = (e: React.DragEvent<HTMLDivElement>) => {
    const node = el.current;
    const f = readContextDrag(e.dataTransfer);
    if (!node || !f) return;
    e.preventDefault();
    if (!projects.includes(f.project)) return;
    const doc = document as Document & {
      caretRangeFromPoint?: (x: number, y: number) => Range | null;
      caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null;
    };
    let range = doc.caretRangeFromPoint?.(e.clientX, e.clientY) ?? null;
    if (!range) {
      const p = doc.caretPositionFromPoint?.(e.clientX, e.clientY);
      if (p) {
        range = document.createRange();
        range.setStart(p.offsetNode, p.offset);
      }
    }
    if (!range || !node.contains(range.startContainer)) range = caretRange(node);
    range.collapse(true);
    const prev = range.startContainer.nodeType === Node.TEXT_NODE ? (range.startContainer.textContent ?? "") : "";
    const lead = range.startOffset > 0 && prev && !/\s/.test(prev[range.startOffset - 1]) ? " " : "";
    const space = document.createTextNode(" ");
    range.insertNode(space);
    range.insertNode(chipNode(f));
    if (lead) range.insertNode(document.createTextNode(lead));
    node.focus();
    placeCaret(space, 1);
    sync();
  };

  const startTag = () => {
    const node = el.current;
    if (!node) return;
    node.focus();
    const text = serialize(node);
    insertText(text && !/\s$/.test(text) ? " @" : "@");
  };

  const keyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (q && matches.length > 0) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const step = e.key === "ArrowDown" ? 1 : -1;
        setActive((a) => (a + step + matches.length) % matches.length);
        return;
      }
      if ((e.key === "Enter" && !e.metaKey && !e.ctrlKey) || e.key === "Tab") {
        e.preventDefault();
        pick(matches[active]);
        return;
      }
    }
    if (q && e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setQ(null);
      return;
    }
    onKeyDown?.(e);
    if (e.defaultPrevented) return;
    if (e.key === "Enter" && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      if (!command("insertLineBreak")) insertText("\n");
    }
  };

  const onClick = (e: React.MouseEvent<HTMLDivElement>) => {
    const remove = (e.target as HTMLElement).closest(".ctx-chip__remove");
    if (remove) {
      e.preventDefault();
      remove.closest(".ctx-chip")?.remove();
      sync();
      return;
    }
    refreshQuery();
  };

  const listId = `${label ?? "request"}-files`.replace(/\s+/g, "-").toLowerCase();
  const open = q !== null;
  const list = open && (
    <div
      id={listId}
      role="listbox"
      className="os-menu ctx-input__list"
      aria-label="Files to tag"
      style={
        pos ? { position: "fixed", left: pos.left, top: pos.top ?? "auto", bottom: pos.bottom ?? "auto" } : undefined
      }
    >
      {files === null ? (
        <div className="os-combobox__empty">Reading the project files…</div>
      ) : matches.length === 0 ? (
        <div className="os-combobox__empty">No file matches “{q?.query}”.</div>
      ) : (
        matches.map((f, i) => (
          <div
            key={fileKey(f)}
            role="option"
            aria-selected={i === active}
            className={cx("os-menu__item", "os-menu__item--tall", i === active && "os-menu__item--active")}
            onMouseDown={(e) => {
              e.preventDefault();
              pick(f);
            }}
            onMouseEnter={() => setActive(i)}
          >
            <Icon name={isFolder(f) ? "folder" : "file"} size={14} className="os-menu__icon" />
            <span className="os-menu__label">
              {baseName(f.path)}
              <span className="os-menu__sub">{fileKey(f)}</span>
            </span>
          </div>
        ))
      )}
    </div>
  );

  return (
    <div className={cx("ctx-input", dropping && "ctx-input--drop")}>
      <div className={cx("os-input", "os-input--multiline", disabled && "ctx-editor--disabled")}>
        <div className="ctx-editor-box">
          {/* biome-ignore lint/a11y/useSemanticElements: a textarea cannot hold file chips inline. */}
          <div
            ref={setRef}
            role="textbox"
            tabIndex={0}
            aria-multiline="true"
            aria-label={label ?? "Request"}
            aria-autocomplete="list"
            aria-expanded={open && matches.length > 0}
            aria-controls={open ? listId : undefined}
            aria-disabled={disabled || undefined}
            contentEditable={!disabled}
            suppressContentEditableWarning
            className="ctx-editor"
            data-empty={value === "" ? "true" : undefined}
            data-placeholder={placeholder}
            style={{ minHeight: `calc(${rows} * 1.5em)` }}
            onInput={sync}
            onKeyDown={keyDown}
            onKeyUp={(e) => {
              if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(e.key) && !q)
                refreshQuery();
            }}
            onClick={onClick}
            onPaste={(e) => {
              e.preventDefault();
              const pasted = Array.from(e.clipboardData.files ?? []);
              if (pasted.length > 0 && onUploadFiles) {
                onUploadFiles(pasted);
                return;
              }
              const text = e.clipboardData.getData("text/plain");
              if (!command("insertText", text)) insertText(text);
            }}
            onBlur={() => setQ(null)}
            onDragOver={(e) => {
              const types = Array.from(e.dataTransfer.types);
              const files = types.includes("Files") && !!onUploadFiles;
              if (!types.includes(CONTEXT_DRAG_TYPE) && !files) return;
              e.preventDefault();
              e.dataTransfer.dropEffect = "copy";
              if (files && !dropping) setDropping(true);
            }}
            onDragLeave={() => setDropping(false)}
            onDrop={(e) => {
              setDropping(false);
              const files = Array.from(e.dataTransfer.files ?? []);
              if (files.length > 0 && onUploadFiles) {
                e.preventDefault();
                onUploadFiles(files);
                return;
              }
              dropFile(e);
            }}
          />
          {uploads.length > 0 && (
            <div className="ctx-editor-uploads" aria-label="Files to upload">
              {uploads.map((u) => (
                <PendingUploadChip key={u.key} upload={u} onRemove={() => onRemoveUpload?.(u.key)} />
              ))}
            </div>
          )}
        </div>
      </div>
      {list && (pos ? createPortal(list, document.body) : list)}
      <div className="ctx-input__files">
        <button
          type="button"
          className="ctx-input__attach"
          onMouseDown={(e) => e.preventDefault()}
          onClick={startTag}
          disabled={disabled}
        >
          <Icon name="at-sign" size={12} /> Tag a file
        </button>
        {onUploadFiles && <UploadButton onFiles={onUploadFiles} disabled={disabled} />}
        {(tagged.length > 0 || uploads.length > 0) && (
          <span className="ctx-input__hint" role="note">
            Say in a few words what each file is for, such as “the diagram shows the new checkout flow”, because the
            agents use your note to decide how to read it.
          </span>
        )}
      </div>
    </div>
  );
}
