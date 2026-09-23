import type { CSSProperties, ReactNode } from "react";

export interface CodeViewProps {
  code: string;
  /** rs, ts, tsx, js, sql, toml, sh, md, txt. Drives comment syntax; md/txt render without color. */
  language?: string;
  startLine?: number;
  /** Line numbers to tint as selected (a finding, a search hit). */
  highlight?: number[];
  /** Line numbers changed by the current session (green / red tint). */
  added?: number[];
  removed?: number[];
  /** No border or background, for a full-pane viewer. */
  flush?: boolean;
  maxHeight?: number | string;
  style?: CSSProperties;
}

const KW = new Set(
  "as async await break const continue crate else enum export extern false fn for from function if impl import in interface let loop match mod move mut new null pub ref return self Self static struct super trait true type undefined use var where while yield default class extends implements".split(
    " ",
  ),
);
const TYPES = /^[A-Z][A-Za-z0-9_]*$/;
const RE = /(\/\/[^\n]*|#[^\n]*|--[^\n]*)|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`[^`]*`)|(\b\d[\d_.]*\b)|([A-Za-z_][A-Za-z0-9_]*)(?=\s*\()|([A-Za-z_][A-Za-z0-9_]*)/g;

function tokens(line: string, lang: string): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let i = 0;
  let m: RegExpExecArray | null;
  const commentOk = (s: string) =>
    (s.startsWith("//") && lang !== "sql") || (s.startsWith("#") && ["toml", "sh", "py", "md"].includes(lang)) || (s.startsWith("--") && lang === "sql");
  RE.lastIndex = 0;
  while ((m = RE.exec(line))) {
    if (m.index > last) out.push(<span key={i++}>{line.slice(last, m.index)}</span>);
    if (m[1] && !commentOk(m[1])) {
      out.push(<span key={i++}>{m[1][0]}</span>);
      last = m.index + 1;
      RE.lastIndex = last;
      continue;
    }
    const cls = m[1] ? "c" : m[2] ? "s" : m[3] ? "n" : m[4] ? (KW.has(m[4]) ? "k" : "f") : KW.has(m[5]) ? "k" : TYPES.test(m[5]) ? "t" : null;
    out.push(
      cls ? (
        <span key={i++} className={"os-tok-" + cls}>
          {m[0]}
        </span>
      ) : (
        <span key={i++}>{m[0]}</span>
      ),
    );
    last = m.index + m[0].length;
    if (m[1]) break;
  }
  if (last < line.length) out.push(<span key={i++}>{line.slice(last)}</span>);
  return out;
}

/** Read-only source view with line numbers and light syntax color. */
export function CodeView({ code, language = "", startLine = 1, highlight = [], added = [], removed = [], flush, maxHeight, style }: CodeViewProps) {
  const lines = code.replace(/\n$/, "").split("\n");
  return (
    <div className={`os-code ${flush ? "os-code--flush" : ""}`} style={{ maxHeight, ...style }}>
      {lines.map((l, idx) => {
        const n = startLine + idx;
        const cls = highlight.includes(n) ? "os-code__line--hl" : added.includes(n) ? "os-code__line--add" : removed.includes(n) ? "os-code__line--del" : "";
        return (
          <div key={idx} className={`os-code__line ${cls}`}>
            <span className="os-code__ln">{n}</span>
            <span>{l === "" ? " " : language === "md" || language === "txt" ? l : tokens(l, language)}</span>
          </div>
        );
      })}
    </div>
  );
}
