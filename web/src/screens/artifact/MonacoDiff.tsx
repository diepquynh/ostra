import { DiffEditor, type DiffOnMount } from "@monaco-editor/react";
import type { DiffFile } from "../../api/types";
import { defineThemes, EDITOR_FONT, languageOf, monaco, themeName } from "../../components/monaco";
import type { Theme } from "../../lib/nav";
import { findingLine, type LedgerFinding } from "../../lib/ledger";

const blocking = (severity: string) => severity === "HIGH" || severity === "BLOCKER";

/** A side-by-side diff with the ledger's findings shown as line annotations. */
export default function MonacoDiff({ file, findings, theme }: { file: DiffFile; findings: LedgerFinding[]; theme: Theme }) {
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
        language={languageOf(file.path)}
        theme={themeName(theme)}
        beforeMount={defineThemes}
        onMount={onMount}
        options={{
          readOnly: true,
          renderSideBySide: true,
          minimap: { enabled: false },
          scrollBeyondLastLine: false,
          ...EDITOR_FONT,
          renderOverviewRuler: false,
        }}
      />
    </div>
  );
}
