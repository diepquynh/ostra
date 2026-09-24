// Local Monaco setup shared by the lazy editor modules (the ledger diff and the file editor). Import it only from
// modules that load with `lazy()`, because it pulls Monaco into the chunk that imports it.

import { loader } from "@monaco-editor/react";
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
import type { Theme } from "../lib/nav";

// Bundle Monaco locally: the app runs on localhost and must not fetch code from a CDN.
(self as unknown as { MonacoEnvironment: unknown }).MonacoEnvironment = { getWorker: () => new EditorWorker() };
loader.config({ monaco });

export { monaco };

export const LANG: Record<string, string> = {
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

/** The Monaco language id for a path, from its extension. */
export const languageOf = (path: string): string => LANG[path.split(".").pop()?.toLowerCase() ?? ""] ?? "plaintext";

// Monaco themes take literal colors. These are `--surface-editor` and the diff tints of each theme in tokens/colors.css.
export function defineThemes() {
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

export const themeName = (theme: Theme) => (theme === "dark" ? "ostra-dark" : "ostra-light");

export const EDITOR_FONT = { fontFamily: '"JetBrains Mono", ui-monospace, Menlo, monospace', fontSize: 12.5 };
