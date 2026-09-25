// Pure helpers for the code display engine: decode a provider's flat token runs into per-line spans, cut a line
// into render segments, and map token classes to the design kit's syntax colors.

import type { CodeFile, TokenClass } from "../../../api/types";

/** One token on one line. `col` and `len` are UTF-16 units, the same as JavaScript string indexes. */
export type Span = { col: number; len: number; cls: TokenClass };

/** A run of line text: plain (`cls` null) or one token. */
export type Segment = { text: string; col: number; cls: TokenClass | null };

/** The `os-tok-*` suffix for each class. Names and operators stay in the text color. */
export const TOKEN_CSS: Record<TokenClass, string | null> = {
  keyword: "k",
  type: "t",
  function: "f",
  name: null,
  constant: "n",
  macro: "f",
  attribute: "k",
  string: "s",
  number: "n",
  comment: "c",
  operator: null,
};

/** Classes that name a symbol, so a click finds its usages. Mirrors `TokenClass::is_symbol` on the server. */
export const isSymbolClass = (c: TokenClass): boolean =>
  c === "type" || c === "function" || c === "name" || c === "constant" || c === "macro";

/**
 * Group the flat `[line, col, len, class]` runs by line in one pass. Index 0 is line 1. Runs with an unknown class,
 * a line outside the file, or a start before the previous token's end are dropped, so a faulty provider cannot break
 * the view.
 */
export function decodeTokens(file: Pick<CodeFile, "tokens" | "classes">, lineCount: number): Span[][] {
  const out: Span[][] = Array.from({ length: lineCount }, () => []);
  const t = file.tokens;
  let lastLine = 0;
  let lastEnd = 0;
  for (let i = 0; i + 3 < t.length; i += 4) {
    const line = t[i];
    const col = t[i + 1];
    const len = t[i + 2];
    const cls = file.classes[t[i + 3]];
    if (!cls || line < 1 || line > lineCount || len <= 0) continue;
    if (line === lastLine && col < lastEnd) continue;
    if (line < lastLine) continue;
    out[line - 1].push({ col, len, cls });
    lastLine = line;
    lastEnd = col + len;
  }
  return out;
}

/** Cut `text` into plain and token segments. Spans past the end of the line are clipped. */
export function segments(text: string, spans: readonly Span[] | undefined): Segment[] {
  if (!spans || spans.length === 0) return text ? [{ text, col: 0, cls: null }] : [];
  const out: Segment[] = [];
  let at = 0;
  for (const s of spans) {
    if (s.col >= text.length) break;
    if (s.col > at) out.push({ text: text.slice(at, s.col), col: at, cls: null });
    const end = Math.min(s.col + s.len, text.length);
    out.push({ text: text.slice(s.col, end), col: s.col, cls: s.cls });
    at = end;
  }
  if (at < text.length) out.push({ text: text.slice(at), col: at, cls: null });
  return out;
}

/** Split file text into lines the way CodeView does: one trailing newline adds no empty last line. */
export const splitLines = (code: string): string[] => code.replace(/\n$/, "").split("\n");

/** The line number an `#L<n>` hash names, or null. */
export function lineFromHash(hash: string): number | null {
  const m = /^#?L(\d+)$/.exec(hash);
  const n = m ? Number(m[1]) : NaN;
  return n >= 1 ? n : null;
}
