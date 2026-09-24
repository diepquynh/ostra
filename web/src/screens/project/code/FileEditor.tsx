import { Editor, type OnMount } from "@monaco-editor/react";
import { useRef } from "react";
import { defineThemes, EDITOR_FONT, languageOf, monaco, themeName } from "../../../components/monaco";
import type { Theme } from "../../../lib/nav";

export type FileEditorProps = {
  path: string;
  value: string;
  onChange: (value: string) => void;
  theme: Theme;
  /** Cmd/Ctrl+S inside the editor. */
  onSave: () => void;
};

/** A Monaco editor for one project file, filling its container. Load it with `lazy()`. */
export default function FileEditor({ path, value, onChange, theme, onSave }: FileEditorProps) {
  // The Monaco command is registered once, so it calls the latest handler through a ref.
  const save = useRef(onSave);
  save.current = onSave;
  const onMount: OnMount = (editor) => {
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => save.current());
    editor.focus();
  };
  return (
    <Editor
      height="100%"
      path={path}
      value={value}
      language={languageOf(path)}
      theme={themeName(theme)}
      beforeMount={defineThemes}
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
