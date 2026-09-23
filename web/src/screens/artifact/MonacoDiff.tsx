import { DiffEditor, loader, type DiffOnMount } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor/editor.api";
import "monaco-editor/languages/definitions/go/register";
import "monaco-editor/languages/definitions/java/register";
import "monaco-editor/languages/definitions/javascript/register";
import "monaco-editor/languages/definitions/kotlin/register";
import "monaco-editor/languages/definitions/markdown/register";
import "monaco-editor/languages/definitions/python/register";
import "monaco-editor/languages/definitions/ruby/register";
import "monaco-editor/languages/definitions/rust/register";
import "monaco-editor/languages/definitions/sql/register";
import "monaco-editor/languages/definitions/typescript/register";
import "monaco-editor/languages/definitions/yaml/register";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import type { DiffFile } from "../../api/types";
import type { Theme } from "../../lib/nav";
import { findingLine, type LedgerFinding } from "../../lib/ledger";

// Bundle Monaco locally: the app runs on localhost and must not fetch code from a CDN.
(self as unknown as { MonacoEnvironment: unknown }).MonacoEnvironment = { getWorker: () => new EditorWorker() };
loader.config({ monaco });

const LANG: Record<string, string> = {
  ts: "typescript",
  tsx: "typescript",
  js: "javascript",
  jsx: "javascript",
  java: "java",
  kt: "kotlin",
  go: "go",
  py: "python",
  rs: "rust",
  rb: "ruby",
  md: "markdown",
  json: "json",
  yml: "yaml",
  yaml: "yaml",
  css: "css",
  html: "html",
  sql: "sql",
};

// Monaco themes take literal colors. These are `--surface-editor` and the diff tints of each theme in tokens/colors.css.
function defineThemes() {
  const colors = (dark: boolean) => ({
    "editor.background": dark ? "#1a1a19" : "#ffffff",
    "editorGutter.background": dark ? "#1a1a19" : "#ffffff",
    "diffEditor.insertedTextBackground": dark ? "#6fcf8f22" : "#1f7a3f1f",
    "diffEditor.removedTextBackground": dark ? "#ff8a8022" : "#b3261e1a",
    "diffEditor.insertedLineBackground": dark ? "#6fcf8f14" : "#1f7a3f12",
    "diffEditor.removedLineBackground": dark ? "#ff8a8014" : "#b3261e10",
  });
  monaco.editor.defineTheme("ostra-dark", { base: "vs-dark", inherit: true, rules: [], colors: colors(true) });
  monaco.editor.defineTheme("ostra-light", { base: "vs", inherit: true, rules: [], colors: colors(false) });
}

const blocking = (severity: string) => severity === "HIGH" || severity === "BLOCKER";

/** A side-by-side diff with the ledger's findings shown as line annotations. */
export default function MonacoDiff({ file, findings, theme }: { file: DiffFile; findings: LedgerFinding[]; theme: Theme }) {
  const ext = file.path.split(".").pop() ?? "";
  const onMount: DiffOnMount = (editor) => {
    const modified = editor.getModifiedEditor();
    const decorations = findings
      .map((f) => ({ f, line: findingLine(f) }))
      .filter((x): x is { f: LedgerFinding; line: number } => x.line !== null)
      .map(({ f, line }) => ({
        range: new monaco.Range(line, 1, line, 1),
        options: {
          isWholeLine: true,
          className: blocking(f.severity) ? "art-ledger-line--bad" : "art-ledger-line--warn",
          linesDecorationsClassName: blocking(f.severity) ? "art-ledger-mark--bad" : "art-ledger-mark--warn",
          hoverMessage: { value: `**${f.id ? `${f.id} ` : ""}${f.severity} ${f.rule}** ${f.description}\n\nFix: ${f.fix}` },
        },
      }));
    modified.createDecorationsCollection(decorations);
  };
  const lines = Math.max(file.original.split("\n").length, file.modified.split("\n").length);
  return (
    <div className="art-monaco" style={{ height: Math.min(420, Math.max(120, lines * 19 + 24)) }}>
      <DiffEditor
        original={file.original}
        modified={file.modified}
        language={LANG[ext] ?? "plaintext"}
        theme={theme === "dark" ? "ostra-dark" : "ostra-light"}
        beforeMount={defineThemes}
        onMount={onMount}
        options={{
          readOnly: true,
          renderSideBySide: true,
          minimap: { enabled: false },
          scrollBeyondLastLine: false,
          fontFamily: '"JetBrains Mono", ui-monospace, Menlo, monospace',
          fontSize: 12.5,
          renderOverviewRuler: false,
        }}
      />
    </div>
  );
}
