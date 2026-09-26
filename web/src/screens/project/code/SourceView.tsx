import { colorLine } from "@ostra/design";
import { type MouseEvent, memo, useEffect, useMemo, useRef } from "react";
import type { CodeFile } from "../../../api/types";
import "./code.css";
import { decodeTokens, isSymbolClass, type Span, segments, splitLines, TOKEN_CSS } from "./tokens";

/** A symbol the user clicked: its name and where. Line is 1-based, col is a 0-based UTF-16 index. */
export type SymbolRef = { name: string; line: number; col: number };

export interface SourceViewProps {
  code: string;
  /** Display data from the code provider. Null or token-less falls back to CodeView's own coloring. */
  file: CodeFile | null;
  /** Extension, for the CodeView fallback. */
  language?: string;
  /** The selected symbol name; its occurrences in this file are tinted. */
  selected?: string | null;
  onSymbol?: (s: SymbolRef) => void;
  /** Line to tint and scroll into view. */
  highlightLine?: number | null;
  /** Tint through this line too, for a whole definition. */
  highlightEnd?: number | null;
  /** Where `highlightLine` lands when scrolled to. Default: the middle. */
  scrollBlock?: "center" | "start";
  /** Change it to scroll to `highlightLine` again. */
  scrollNonce?: number;
}

type LineProps = {
  n: number;
  text: string;
  /** Provider tokens; null colors the line the way CodeView does. */
  spans: Span[] | null;
  language: string;
  hl: boolean;
  selected: string | null;
};

const Line = memo(function Line({ n, text, spans, language, hl, selected }: LineProps) {
  return (
    <div className={`os-code__line ${hl ? "os-code__line--hl" : ""}`} data-line={n}>
      <span className="os-code__ln">{n}</span>
      <span>
        {!spans
          ? colorLine(text, language)
          : text === ""
            ? " "
            : segments(text, spans).map((s) => {
                if (!s.cls) return <span key={s.col}>{s.text}</span>;
                const sym = isSymbolClass(s.cls);
                const tok = TOKEN_CSS[s.cls];
                const cls = [tok && `os-tok-${tok}`, sym && "code-sym", sym && s.text === selected && "code-sym--sel"]
                  .filter(Boolean)
                  .join(" ");
                return (
                  <span key={s.col} className={cls || undefined} data-col={sym ? s.col : undefined}>
                    {s.text}
                  </span>
                );
              })}
      </span>
    </div>
  );
});

/**
 * Read-only file text colored by the code provider's tokens. Name tokens are clickable and report the symbol through
 * `onSymbol`. Uses CodeView's markup and, when the provider sent no tokens, its coloring. The dependency graph's
 * preview shows a definition with it.
 */
export function SourceView({
  code,
  file,
  language = "",
  selected = null,
  onSymbol,
  highlightLine = null,
  highlightEnd = null,
  scrollBlock = "center",
  scrollNonce = 0,
}: SourceViewProps) {
  const box = useRef<HTMLDivElement>(null);
  const lines = useMemo(() => splitLines(code), [code]);
  const spans = useMemo(
    () => (file && file.tokens.length > 0 ? decodeTokens(file, lines.length) : null),
    [file, lines.length],
  );
  // Only lines holding the selected name re-render when the selection changes.
  const hasSel = useMemo(() => {
    if (!spans || !selected) return null;
    return spans.map((ls, i) =>
      ls.some((s) => isSymbolClass(s.cls) && s.len === selected.length && lines[i].substr(s.col, s.len) === selected),
    );
  }, [spans, lines, selected]);

  useEffect(() => {
    if (!highlightLine) return;
    const id = requestAnimationFrame(() => {
      const row = box.current?.querySelector<HTMLElement>(`[data-line="${highlightLine}"]`);
      const pane = box.current?.parentElement;
      // Scroll only the pane that holds the view, so the page itself stays put.
      if (row && pane && scrollBlock === "start")
        pane.scrollTop += row.getBoundingClientRect().top - pane.getBoundingClientRect().top - 8;
      else row?.scrollIntoView?.({ block: scrollBlock });
    });
    return () => cancelAnimationFrame(id);
  }, [highlightLine, scrollBlock, scrollNonce, spans]);

  const click = (e: MouseEvent) => {
    const tok = (e.target as HTMLElement).closest<HTMLElement>("[data-col]");
    const row = tok?.closest<HTMLElement>("[data-line]");
    if (!tok || !row || !onSymbol || window.getSelection()?.toString()) return;
    onSymbol({ name: tok.textContent ?? "", line: Number(row.dataset.line), col: Number(tok.dataset.col) });
  };

  return (
    <div ref={box} className="os-code os-code--flush" onClick={click}>
      {lines.map((text, i) => {
        const n = i + 1;
        return (
          <Line
            key={i}
            n={n}
            text={text}
            spans={spans ? spans[i] : null}
            language={language}
            hl={
              highlightLine !== null &&
              (n === highlightLine || (highlightEnd !== null && n > highlightLine && n <= highlightEnd))
            }
            selected={hasSel?.[i] ? selected : null}
          />
        );
      })}
    </div>
  );
}
