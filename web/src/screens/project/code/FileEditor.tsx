import { Editor, type OnMount } from "@monaco-editor/react";
import { Menu } from "@ostra/design";
import { useEffect, useRef, useState } from "react";
import { api } from "../../../api";
import type { CodeLocation, NavigateTarget } from "../../../api/types";
import {
  defineThemes,
  EDITOR_FONT,
  EDITOR_WORD_SEPARATORS,
  languageOf,
  monaco,
  themeName,
} from "../../../components/monaco";
import type { Theme } from "../../../lib/nav";
import type { ChangeBlock } from "../diff";
import { bindModel, navigate, registerHints } from "./hints";
import { clickLanding, IS_MAC, JUMP_LABEL, jumpLanding, type Landing, linkModifier, locationItems } from "./jump";
import type { SymbolRef } from "./SourceView";
import "./code.css";

export type FileEditorProps = {
  ws: string;
  projectKey: string;
  path: string;
  value: string;
  onChange: (value: string) => void;
  theme: Theme;
  /** Cmd/Ctrl+S inside the editor. */
  onSave: () => void;
  /** The draft differs from the saved file, so opening another file would drop it. */
  dirty: boolean;
  /** Open another file at a 1-based line: a project file, or a dependency file when `uri` is set. */
  onOpen: (to: { path: string; uri?: string | null; line: number }) => void;
  /** Showing a dependency file a language server pointed at: its URI and file name. Always read-only. */
  external?: { uri: string; name: string } | null;
  /** Viewing, not editing: the text cannot change, and a click on a name selects it for the code pane. */
  readOnly: boolean;
  /** Why the file cannot be edited at all, or null when the pencil may start an edit. */
  locked: string | null;
  /** From read-only mode: start editing, then put the cursor at `start` and run its action. */
  onEditAt: (start: EditorStart) => void;
  /** Once editing: put the cursor at `line`/`col` (1-based, 0-based UTF-16) and run a Monaco action there. */
  start?: EditorStart | null;
  /** The selected name; its whole-word occurrences are tinted. */
  selected?: string | null;
  onSymbol?: (s: SymbolRef) => void;
  /** Tint `line` (through `end`) and scroll to it; a new `n` scrolls again. */
  reveal?: { line: number; end?: number | null; n: number } | null;
  /** Changed lines against HEAD, marked in the gutter. A click on a mark shows the HEAD text under the block. */
  changes?: ChangeBlock[] | null;
};

export type EditorStart = { line: number; col: number; action?: "replace" | "lineUp" | "lineDown" };

type Code = monaco.editor.IStandaloneCodeEditor;
type Pick = { title: string; locs: CodeLocation[]; top: number; left: number };

const K = monaco.KeyMod;
const C = monaco.KeyCode;
// Cmd on macOS, Ctrl elsewhere.
const JUMPS: { id: string; label: string; key: number; target: NavigateTarget }[] = [
  { id: "ostra.showBaseClass", label: JUMP_LABEL.supertypes, key: K.CtrlCmd | K.Shift | C.KeyI, target: "supertypes" },
  {
    id: "ostra.showImplementation",
    label: JUMP_LABEL.implementations,
    key: K.CtrlCmd | K.Shift | C.KeyM,
    target: "implementations",
  },
];

// Monaco's own find, replace, and line moves, which it binds to these keys but leaves out of the context menu.
// Replace takes text or, with the regex toggle (Alt+R), a pattern whose groups the replacement names as $1.
const BUILTINS: { id: string; label: string; key: number; run: string; edits?: EditorStart["action"] }[] = [
  { id: "ostra.find", label: "Find", key: K.CtrlCmd | C.KeyF, run: "actions.find" },
  {
    id: "ostra.replace",
    label: "Replace",
    key: IS_MAC ? K.CtrlCmd | K.Alt | C.KeyF : K.CtrlCmd | C.KeyH,
    run: "editor.action.startFindReplaceAction",
    edits: "replace",
  },
  {
    id: "ostra.moveLineUp",
    label: "Move line up",
    key: K.Alt | C.UpArrow,
    run: "editor.action.moveLinesUpAction",
    edits: "lineUp",
  },
  {
    id: "ostra.moveLineDown",
    label: "Move line down",
    key: K.Alt | C.DownArrow,
    run: "editor.action.moveLinesDownAction",
    edits: "lineDown",
  },
];

const START_ACTION: Record<NonNullable<EditorStart["action"]>, string> = {
  replace: "editor.action.startFindReplaceAction",
  lineUp: "editor.action.moveLinesUpAction",
  lineDown: "editor.action.moveLinesDownAction",
};

// Scroll, cursor, and folds per file, because switching tabs unmounts the editor. Oldest first.
const viewStates = new Map<string, monaco.editor.ICodeEditorViewState>();
const MAX_VIEW_STATES = 200;
const viewKey = (ws: string, key: string, model: monaco.editor.ITextModel) => `${ws}\0${key}\0${model.uri}`;

const PICK_WIDTH = 380;
const MAX_TINTS = 2000;
const MARK_WORD = { add: "Added", mod: "Changed", del: "Removed below" } as const;

/** The HEAD text of a change block, for a view zone under it. */
function compareNode(b: ChangeBlock, lineHeight: number): HTMLElement {
  const root = document.createElement("div");
  root.className = "code-zone";
  const head = document.createElement("div");
  head.className = "code-zone__head";
  head.style.height = head.style.lineHeight = `${lineHeight}px`;
  const range = (ls: { no: number }[]) =>
    ls.length === 1 ? `line ${ls[0].no}` : `lines ${ls[0].no}–${ls[ls.length - 1].no}`;
  head.textContent = b.old.length
    ? `HEAD ${range(b.old)}. Click the mark again to close.`
    : "New lines. HEAD has nothing here.";
  root.appendChild(head);
  for (const l of b.old) {
    const row = document.createElement("div");
    row.className = "code-zone__line";
    row.style.height = row.style.lineHeight = `${lineHeight}px`;
    row.textContent = l.text || " ";
    root.appendChild(row);
  }
  return root;
}

function message(e: Code, text: string) {
  const at = e.getPosition();
  const c = e.getContribution<
    monaco.editor.IEditorContribution & { showMessage(m: string, p: monaco.IPosition): void }
  >("editor.contrib.messageController");
  if (at && c) c.showMessage(text, at);
}

/**
 * The File view of a project file: Monaco, read-only while viewing and writable while editing, filling its container.
 * Load it with `lazy()`.
 */
export default function FileEditor(props: FileEditorProps) {
  const { ws, projectKey, path, value, onChange, theme } = props;
  // Monaco commands are registered once, so they call the latest props through a ref.
  const latest = useRef(props);
  latest.current = props;
  const [editor, setEditor] = useState<Code | null>(null);
  const [pick, setPick] = useState<Pick | null>(null);
  const box = useRef<HTMLDivElement>(null);

  const go = (e: Code, l: CodeLocation) => {
    const p = latest.current;
    const here = p.external ? l.uri === p.external.uri : !l.uri && l.path === p.path;
    if (!here) {
      if (p.dirty) {
        message(e, `Save or discard your changes before opening ${l.path}, because leaving the editor drops them.`);
        return;
      }
      p.onOpen({ path: l.path, uri: l.uri, line: l.line });
      return;
    }
    const at = { lineNumber: l.line, column: l.col + 1 };
    e.setSelection(new monaco.Selection(l.line, l.col + 1, l.line, l.col + 1 + l.len));
    e.revealPositionInCenterIfOutsideViewport(at);
    e.focus();
    if (p.readOnly) p.onSymbol?.({ name: l.name, line: l.line, col: l.col });
  };

  const land = (e: Code, at: monaco.IPosition, to: Landing) => {
    if (!to) return;
    if ("note" in to) return message(e, to.note);
    if ("go" in to) return go(e, to.go);
    const spot = e.getScrolledVisiblePosition(at);
    const width = box.current?.clientWidth ?? PICK_WIDTH;
    setPick({
      ...to.pick,
      top: (spot?.top ?? 0) + (spot?.height ?? 18) + 4,
      left: Math.max(0, Math.min(spot?.left ?? 0, width - PICK_WIDTH - 8)),
    });
  };

  const jump = async (e: Code, target: NavigateTarget) => {
    const model = e.getModel();
    const at = e.getPosition();
    if (!model || !at) return;
    if (latest.current.external) return message(e, "Base class and implementation jumps work in project files.");
    const reply = await navigate(model, at, target);
    if (reply?.error) return message(e, reply.error);
    // A null result means neither the code index nor a language server knows the name.
    land(e, at, jumpLanding(reply?.result ?? null, target));
  };

  // Ctrl/Cmd+click asks the project's code navigation, which reads the saved file.
  const follow = async (e: Code, at: monaco.Position) => {
    const word = e.getModel()?.getWordAtPosition(at);
    if (!word) return;
    const p = latest.current;
    const col = word.startColumn - 1;
    const doc = p.external ? { uri: p.external.uri } : { path: p.path };
    try {
      const u = await api.codeUsages(p.ws, p.projectKey, word.word, { ...doc, line: at.lineNumber, col });
      land(e, at, clickLanding(u, p.path, at.lineNumber, col, p.external?.uri));
    } catch (err) {
      message(e, err instanceof Error ? err.message : String(err));
    }
  };

  const onMount: OnMount = (e) => {
    e.addCommand(K.CtrlCmd | C.KeyS, () => latest.current.onSave());
    JUMPS.forEach((j, i) => {
      e.addAction({
        id: j.id,
        label: j.label,
        keybindings: [j.key],
        contextMenuGroupId: "navigation",
        contextMenuOrder: i + 1,
        run: (ed) => jump(ed as Code, j.target),
      });
    });
    BUILTINS.forEach((b, i) => {
      e.addAction({
        id: b.id,
        label: b.label,
        keybindings: [b.key],
        contextMenuGroupId: "1_modification",
        contextMenuOrder: i + 1,
        run: (ed) => {
          const p = latest.current;
          if (!b.edits || !p.readOnly) return ed.trigger("contextmenu", b.run, null);
          // Read-only mode: these commands change the text, so they start an edit first.
          if (p.locked) return message(ed as Code, p.locked);
          const at = ed.getPosition();
          p.onEditAt({ line: at?.lineNumber ?? 1, col: (at?.column ?? 1) - 1, action: b.edits });
        },
      });
    });
    // Underline the name under the pointer while the link modifier is held, as a hint that a click follows it.
    const link = e.createDecorationsCollection();
    const underline = (ev: monaco.editor.IEditorMouseEvent | null) => {
      const at = ev?.target.type === monaco.editor.MouseTargetType.CONTENT_TEXT ? ev.target.position : null;
      const word = at && ev && linkModifier(ev.event.browserEvent) ? e.getModel()?.getWordAtPosition(at) : null;
      link.set(
        word && at
          ? [
              {
                range: new monaco.Range(at.lineNumber, word.startColumn, at.lineNumber, word.endColumn),
                options: { inlineClassName: "code-link" },
              },
            ]
          : [],
      );
    };
    e.onMouseMove(underline);
    e.onMouseLeave(() => underline(null));
    e.onKeyUp(() => link.clear());
    e.onMouseDown((ev) => {
      const at = ev.target.position;
      if (ev.target.type !== monaco.editor.MouseTargetType.CONTENT_TEXT || !at || !ev.event.leftButton) return;
      if (linkModifier(ev.event.browserEvent)) {
        link.clear();
        void follow(e, at);
      }
    });
    // A plain click on a name while viewing selects it, unless the click made a selection.
    e.onMouseUp((ev) => {
      const p = latest.current;
      const at = ev.target.position;
      if (!p.readOnly || !p.onSymbol || ev.target.type !== monaco.editor.MouseTargetType.CONTENT_TEXT || !at) return;
      if (linkModifier(ev.event.browserEvent) || !e.getSelection()?.isEmpty()) return;
      const word = e.getModel()?.getWordAtPosition(at);
      if (word) p.onSymbol({ name: word.word, line: at.lineNumber, col: word.startColumn - 1 });
    });
    // Saved as it changes, so the state survives an unmount. A model is saved only once its own state was restored,
    // because switching models scrolls the new one to the top first.
    const keep = () => {
      const model = e.getModel();
      const state = e.saveViewState();
      if (!model || !state || restored.current !== model) return;
      const k = viewKey(latest.current.ws, latest.current.projectKey, model);
      viewStates.delete(k);
      viewStates.set(k, state);
      if (viewStates.size > MAX_VIEW_STATES) viewStates.delete(viewStates.keys().next().value as string);
    };
    e.onDidScrollChange(keep);
    e.onDidChangeCursorPosition(keep);
    if (!latest.current.readOnly) e.focus();
    setEditor(e);
  };
  // Before the `reveal` effect, so a jump to a line wins over the restored view.
  const restored = useRef<monaco.editor.ITextModel | null>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new path means the wrapper switched to another model.
  useEffect(() => {
    const model = editor?.getModel();
    if (!editor || !model) return;
    const state = viewStates.get(viewKey(ws, projectKey, model));
    if (state) editor.restoreViewState(state);
    restored.current = model;
  }, [editor, ws, projectKey, path]);
  // The options effect of the wrapper runs before this one, so the editor is writable when the action runs, and its
  // `onChange` subscription exists, so the edit reaches the draft.
  const start = props.start;
  useEffect(() => {
    if (!editor || !start || props.readOnly) return;
    editor.setPosition({ lineNumber: start.line, column: start.col + 1 });
    editor.revealLineInCenter(start.line);
    editor.focus();
    if (start.action) editor.trigger("viewer", START_ACTION[start.action], null);
  }, [editor, start, props.readOnly]);

  const tints = useRef<monaco.editor.IEditorDecorationsCollection | null>(null);
  const selected = props.selected;
  useEffect(() => {
    const model = editor?.getModel();
    if (!editor || !model) return;
    tints.current ??= editor.createDecorationsCollection();
    const found =
      selected && value
        ? model.findMatches(selected, false, false, true, EDITOR_WORD_SEPARATORS, false, MAX_TINTS)
        : [];
    tints.current.set(found.map((m) => ({ range: m.range, options: { inlineClassName: "code-sym--sel" } })));
  }, [editor, selected, value]);

  const lineTint = useRef<monaco.editor.IEditorDecorationsCollection | null>(null);
  const reveal = props.reveal;
  useEffect(() => {
    if (!editor || !reveal) return;
    lineTint.current ??= editor.createDecorationsCollection();
    const end = Math.max(reveal.line, reveal.end ?? reveal.line);
    lineTint.current.set([
      { range: new monaco.Range(reveal.line, 1, end, 1), options: { isWholeLine: true, className: "code-line--hl" } },
    ]);
    editor.revealLineInCenter(reveal.line);
    editor.setPosition({ lineNumber: reveal.line, column: 1 });
  }, [editor, reveal]);

  const marks = useRef<monaco.editor.IEditorDecorationsCollection | null>(null);
  const zone = useRef<{ id: string; block: ChangeBlock } | null>(null);
  const changes = props.changes;
  useEffect(() => {
    if (!editor) return;
    marks.current ??= editor.createDecorationsCollection();
    marks.current.set(
      (changes ?? []).flatMap((b) =>
        b.lines.map((n) => ({
          range: new monaco.Range(n, 1, n, 1),
          options: {
            linesDecorationsClassName: `code-mark code-mark--${b.kind}`,
            linesDecorationsTooltip: `${MARK_WORD[b.kind]} since HEAD. Click to compare.`,
          },
        })),
      ),
    );
    const close = () =>
      editor.changeViewZones((z) => {
        if (zone.current) z.removeZone(zone.current.id);
        zone.current = null;
      });
    const sub = editor.onMouseDown((ev) => {
      if (ev.target.type !== monaco.editor.MouseTargetType.GUTTER_LINE_DECORATIONS || !ev.target.position) return;
      const n = ev.target.position.lineNumber;
      const b = changes?.find((x) => x.lines.includes(n));
      if (!b) return;
      const same = zone.current?.block === b;
      close();
      if (same) return;
      editor.changeViewZones((z) => {
        const id = z.addZone({
          afterLineNumber: b.anchor,
          heightInLines: b.old.length + 1,
          domNode: compareNode(b, editor.getOption(monaco.editor.EditorOption.lineHeight)),
        });
        zone.current = { id, block: b };
      });
    });
    return () => {
      sub.dispose();
      close();
    };
  }, [editor, changes]);
  // Runs after the editor switched to the model of `path`, because child effects run first.
  const external = props.external;
  useEffect(() => {
    const model = editor?.getModel();
    // Completions and signatures ask about project files, and a dependency file is never edited.
    if (model && !external) bindModel(model, { workspace: ws, key: projectKey, path });
  }, [editor, ws, projectKey, path, external]);
  return (
    <div ref={box} style={{ position: "relative", height: "100%" }}>
      <Editor
        height="100%"
        path={external ? `dep:${external.uri}` : path}
        value={value}
        language={languageOf(external ? external.name.replace(/\.class$/, ".java") : path)}
        theme={themeName(theme)}
        beforeMount={() => {
          defineThemes();
          registerHints();
        }}
        onMount={onMount}
        onChange={(v) => {
          if (!latest.current.readOnly) onChange(v ?? "");
        }}
        options={{
          readOnly: props.readOnly,
          readOnlyMessage: {
            value: props.locked ?? "This file is read-only while you view it. Press the pencil to edit it.",
          },
          lineDecorationsWidth: 12,
          minimap: { enabled: false },
          scrollBeyondLastLine: false,
          overviewRulerLanes: 0,
          automaticLayout: true,
          tabSize: 2,
          detectIndentation: true,
          fixedOverflowWidgets: true,
          ...EDITOR_FONT,
        }}
      />
      <Menu
        open={!!pick}
        label={pick?.title}
        width={PICK_WIDTH}
        style={pick ? { top: pick.top, left: pick.left } : undefined}
        onClose={() => {
          setPick(null);
          editor?.focus();
        }}
        items={
          pick
            ? locationItems(pick.title, pick.locs, (l) => {
                if (editor) go(editor, l);
              })
            : []
        }
      />
    </div>
  );
}
