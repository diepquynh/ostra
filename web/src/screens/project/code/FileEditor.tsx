import { Editor, type OnMount } from "@monaco-editor/react";
import { useEffect, useRef, useState } from "react";
import { defineThemes, EDITOR_FONT, languageOf, monaco, themeName } from "../../../components/monaco";
import type { Theme } from "../../../lib/nav";
import { bindModel, registerHints } from "./hints";

export type FileEditorProps = {
  ws: string;
  projectKey: string;
  path: string;
  value: string;
  onChange: (value: string) => void;
  theme: Theme;
  /** Cmd/Ctrl+S inside the editor. */
  onSave: () => void;
};

/** A Monaco editor for one project file, filling its container. Load it with `lazy()`. */
export default function FileEditor({ ws, projectKey, path, value, onChange, theme, onSave }: FileEditorProps) {
  // The Monaco command is registered once, so it calls the latest handler through a ref.
  const save = useRef(onSave);
  save.current = onSave;
  const [editor, setEditor] = useState<monaco.editor.IStandaloneCodeEditor | null>(null);
  const onMount: OnMount = (e) => {
    e.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => save.current());
    e.focus();
    setEditor(e);
  };
  // Runs after the editor switched to the model of `path`, because child effects run first.
  useEffect(() => {
    const model = editor?.getModel();
    if (model) bindModel(model, { workspace: ws, key: projectKey, path });
  }, [editor, ws, projectKey, path]);
  return (
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
        ...EDITOR_FONT,
      }}
    />
  );
}
