import { Editor, type OnMount } from "@monaco-editor/react";
import { useEffect, useRef, useState } from "react";
import type { CodeLocation, CodeNavigation, NavigateTarget } from "../../../api/types";
import { defineThemes, EDITOR_FONT, languageOf, monaco, themeName } from "../../../components/monaco";
import { Menu } from "../../../design";
import type { Theme } from "../../../lib/nav";
import { bindModel, navigate, registerHints } from "./hints";

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
  /** Open another project file at a 1-based line. */
  onOpen: (path: string, line: number) => void;
};

type Code = monaco.editor.IStandaloneCodeEditor;
type Pick = { nav: CodeNavigation; target: NavigateTarget; top: number; left: number };

const K = monaco.KeyMod;
const C = monaco.KeyCode;
// Cmd on macOS, Ctrl elsewhere.
const JUMPS: { id: string; label: string; key: number; target: NavigateTarget }[] = [
  { id: "ostra.showBaseClass", label: "Show base class", key: K.CtrlCmd | K.Shift | C.KeyI, target: "supertypes" },
  {
    id: "ostra.showImplementation",
    label: "Show implementation",
    key: K.CtrlCmd | K.Shift | C.KeyM,
    target: "implementations",
  },
];

const MAC = /Mac|iPhone|iPad/.test(navigator.userAgent);
// Monaco's own find, replace, and line moves, which it binds to these keys but leaves out of the context menu.
// Replace takes text or, with the regex toggle (Alt+R), a pattern whose groups the replacement names as $1.
const BUILTINS: { id: string; label: string; key: number; run: string }[] = [
  { id: "ostra.find", label: "Find", key: K.CtrlCmd | C.KeyF, run: "actions.find" },
  {
    id: "ostra.replace",
    label: "Replace",
    key: MAC ? K.CtrlCmd | K.Alt | C.KeyF : K.CtrlCmd | C.KeyH,
    run: "editor.action.startFindReplaceAction",
  },
  { id: "ostra.moveLineUp", label: "Move line up", key: K.Alt | C.UpArrow, run: "editor.action.moveLinesUpAction" },
  {
    id: "ostra.moveLineDown",
    label: "Move line down",
    key: K.Alt | C.DownArrow,
    run: "editor.action.moveLinesDownAction",
  },
];

const PICK_WIDTH = 380;

function message(e: Code, text: string) {
  const at = e.getPosition();
  const c = e.getContribution<
    monaco.editor.IEditorContribution & { showMessage(m: string, p: monaco.IPosition): void }
  >("editor.contrib.messageController");
  if (at && c) c.showMessage(text, at);
}

const heading = (p: Pick) =>
  `${p.target === "supertypes" ? `What ${p.nav.symbol} extends` : `What implements ${p.nav.symbol}`}${p.nav.truncated ? ` (first ${p.nav.locations.length})` : ""}`;

/** A Monaco editor for one project file, filling its container. Load it with `lazy()`. */
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
    if (l.path !== p.path) {
      if (p.dirty) {
        message(e, `Save or discard your changes before opening ${l.path}, because leaving the editor drops them.`);
        return;
      }
      p.onOpen(l.path, l.line);
      return;
    }
    const at = { lineNumber: l.line, column: l.col + 1 };
    e.setSelection(new monaco.Selection(l.line, l.col + 1, l.line, l.col + 1 + l.len));
    e.revealPositionInCenterIfOutsideViewport(at);
    e.focus();
  };

  const jump = async (e: Code, target: NavigateTarget) => {
    const model = e.getModel();
    const at = e.getPosition();
    if (!model || !at) return;
    const reply = await navigate(model, at, target);
    if (reply?.error) return message(e, reply.error);
    const nav = reply?.result;
    // Neither the code index nor a language server knows the name.
    if (!nav) return;
    const [first, ...rest] = nav.locations;
    if (!first) {
      return message(
        e,
        target === "supertypes"
          ? `${nav.symbol} extends or implements nothing that the project defines.`
          : `Nothing in the project extends or implements ${nav.symbol}.`,
      );
    }
    if (rest.length === 0) return go(e, first);
    const spot = e.getScrolledVisiblePosition(at);
    const width = box.current?.clientWidth ?? PICK_WIDTH;
    setPick({
      nav,
      target,
      top: (spot?.top ?? 0) + (spot?.height ?? 18) + 4,
      left: Math.max(0, Math.min(spot?.left ?? 0, width - PICK_WIDTH - 8)),
    });
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
        run: (ed) => ed.trigger("contextmenu", b.run, null),
      });
    });
    e.focus();
    setEditor(e);
  };
  // Runs after the editor switched to the model of `path`, because child effects run first.
  useEffect(() => {
    const model = editor?.getModel();
    if (model) bindModel(model, { workspace: ws, key: projectKey, path });
  }, [editor, ws, projectKey, path]);
  return (
    <div ref={box} style={{ position: "relative", height: "100%" }}>
      <Editor
        height="100%"
        path={path}
        value={value}
        language={languageOf(path)}
        theme={themeName(theme)}
        beforeMount={() => {
          defineThemes();
          registerHints();
        }}
        onMount={onMount}
        onChange={(v) => onChange(v ?? "")}
        options={{
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
        label={pick ? heading(pick) : undefined}
        width={PICK_WIDTH}
        style={pick ? { top: pick.top, left: pick.left } : undefined}
        onClose={() => {
          setPick(null);
          editor?.focus();
        }}
        items={
          pick
            ? [
                { type: "heading", label: heading(pick) },
                ...pick.nav.locations.map((l) => ({
                  id: `${l.path}:${l.line}:${l.col}`,
                  label: `${l.container ? `${l.container}.` : ""}${l.name} · ${l.path}:${l.line}`,
                  sub: l.preview || undefined,
                  icon: "braces" as const,
                  onSelect: () => {
                    if (editor) go(editor, l);
                  },
                })),
              ]
            : []
        }
      />
    </div>
  );
}
