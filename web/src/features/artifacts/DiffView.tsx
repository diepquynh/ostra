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
import type { DiffFile } from "../../api/extra";
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

const dark = () => window.matchMedia?.("(prefers-color-scheme: dark)").matches;

/** A side-by-side diff with the ledger's findings shown as line annotations. */
export default function DiffView({ file, findings }: { file: DiffFile; findings: LedgerFinding[] }) {
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
          className: f.severity === "HIGH" || f.severity === "BLOCKER" ? "ledger-line-bad" : "ledger-line-warn",
          glyphMarginHoverMessage: { value: `**${f.severity} ${f.rule}** ${f.description}\n\nFix: ${f.fix}` },
          hoverMessage: { value: `**${f.severity} ${f.rule}** ${f.description}\n\nFix: ${f.fix}` },
        },
      }));
    modified.createDecorationsCollection(decorations);
  };
  return (
    <div style={{ height: 420, border: "1px solid var(--border)", borderRadius: 6 }}>
      <DiffEditor
        original={file.original}
        modified={file.modified}
        language={LANG[ext] ?? "plaintext"}
        theme={dark() ? "vs-dark" : "vs"}
        onMount={onMount}
        options={{ readOnly: true, renderSideBySide: true, minimap: { enabled: false }, scrollBeyondLastLine: false }}
      />
    </div>
  );
}
