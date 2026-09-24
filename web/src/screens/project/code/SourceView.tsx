import { memo, useEffect, useMemo, useRef, type MouseEvent } from "react";
import type { CodeFile } from "../../../api/types";
import { CodeView } from "../../../design";
import "./code.css";
import { decodeTokens, isSymbolClass, segments, splitLines, TOKEN_CSS, type Span } from "./tokens";

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
  /** Change it to scroll to `highlightLine` again. */
  scrollNonce?: number;
}

type LineProps = {
  n: number;
  text: string;
  spans: Span[] | undefined;
  hl: boolean;
  selected: string | null;
};

const Line = memo(function Line({ n, text, spans, hl, selected }: LineProps) {
  return (
    <div className={`os-code__line ${hl ? "os-code__line--hl" : ""}`} data-line={n}>
      <span className="os-code__ln">{n}</span>
      <span>
        {text === ""
          ? " "
          : segments(text, spans).map((s) => {
              if (!s.cls) return <span key={s.col}>{s.text}</span>;
              const sym = isSymbolClass(s.cls);
              const tok = TOKEN_CSS[s.cls];
              const cls = [tok && `os-tok-${tok}`, sym && "code-sym", sym && s.text === selected && "code-sym--sel"].filter(Boolean).join(" ");
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
 * `onSymbol`. Uses CodeView's markup, and CodeView itself when the provider sent no tokens.
 */
export function SourceView({ code, file, language, selected = null, onSymbol, highlightLine = null, scrollNonce = 0 }: SourceViewProps) {
  const box = useRef<HTMLDivElement>(null);
  const lines = useMemo(() => splitLines(code), [code]);
  const spans = useMemo(() => (file && file.tokens.length > 0 ? decodeTokens(file, lines.length) : null), [file, lines.length]);
  // Only lines holding the selected name re-render when the selection changes.
  const hasSel = useMemo(() => {
    if (!spans || !selected) return null;
    return spans.map((ls, i) => ls.some((s) => isSymbolClass(s.cls) && s.len === selected.length && lines[i].substr(s.col, s.len) === selected));
  }, [spans, lines, selected]);

  useEffect(() => {
    if (!highlightLine) return;
    const id = requestAnimationFrame(() => box.current?.querySelector(`[data-line="${highlightLine}"]`)?.scrollIntoView?.({ block: "center" }));
    return () => cancelAnimationFrame(id);
  }, [highlightLine, scrollNonce, spans]);

  if (!spans) {
    return (
      <div ref={box}>
        <CodeView flush language={language} code={code} highlight={highlightLine ? [highlightLine] : []} />
      </div>
    );
  }

  const click = (e: MouseEvent) => {
    const tok = (e.target as HTMLElement).closest<HTMLElement>("[data-col]");
    const row = tok?.closest<HTMLElement>("[data-line]");
    if (!tok || !row || !onSymbol || window.getSelection()?.toString()) return;
    onSymbol({
      name: tok.textContent ?? "",
      line: Number(row.dataset.line),
      col: Number(tok.dataset.col),
    });
  };

  return (
    <div ref={box} className="os-code os-code--flush" onClick={click}>
      {lines.map((text, i) => (
        <Line key={i} n={i + 1} text={text} spans={spans[i]} hl={highlightLine === i + 1} selected={hasSel?.[i] ? selected : null} />
      ))}
    </div>
  );
}
