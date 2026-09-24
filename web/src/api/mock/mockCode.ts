// A naive code provider for the mock server: a regex tokenizer, definitions after a definition keyword,
// usages by whole-word match, and imports from `use` and `import ... from` lines.

import type { CodeDeps, CodeFile, CodeImport, CodeLocation, CodeSymbol, CodeSymbols, CodeUsages, SymbolKind, TokenClass } from "../types";
import { mockTexts } from "./projectFiles";

const KW = new Set(
  "as async await break const continue crate else enum export extern false fn for from function if impl import in interface let loop match mod move mut new null pub ref return self Self static struct super trait true type undefined use var where while yield default class extends implements".split(
    " ",
  ),
);
const DEFS: Record<string, SymbolKind> = {
  fn: "function",
  function: "function",
  struct: "class",
  class: "class",
  enum: "enum",
  trait: "interface",
  interface: "interface",
  type: "type",
  mod: "module",
  const: "constant",
  static: "constant",
};
const RE = /(\/\/.*|#.*)|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`[^`]*`)|(\b\d[\d_.]*\b)|([A-Za-z_][A-Za-z0-9_]*)/g;
const LANG: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  tsx: "typescript",
  js: "javascript",
  toml: "toml",
  sql: "sql",
};

const langOf = (path: string) => LANG[path.split(".").pop() ?? ""] ?? null;

function fileText(key: string, path: string): string {
  const f = mockTexts(key).find((t) => t.path === path);
  if (!f) throw Object.assign(new Error(`${path} is not a file in project \`${key}\`.`), { status: 404 });
  return f.code;
}

function analyze(key: string, path: string, code: string): CodeFile {
  const language = langOf(path);
  const classes: TokenClass[] = ["keyword", "type", "function", "name", "constant", "string", "number", "comment"];
  const idx = (c: TokenClass) => classes.indexOf(c);
  const tokens: number[] = [];
  const symbols: CodeSymbol[] = [];
  if (!language || language === "sql")
    return {
      path,
      provider: "mock",
      language,
      classes: [],
      tokens: [],
      symbols: [],
      imports: imports(key, path, code),
    };
  const hashComments = language === "toml";
  code.split("\n").forEach((text, i) => {
    const line = i + 1;
    let pendingDef: SymbolKind | null = null;
    RE.lastIndex = 0;
    let m: RegExpExecArray | null;
    while ((m = RE.exec(text))) {
      let cls: TokenClass;
      if (m[1]) {
        if (m[1].startsWith("#") !== hashComments) {
          RE.lastIndex = m.index + 1;
          continue;
        }
        cls = "comment";
      } else if (m[2]) cls = "string";
      else if (m[3]) cls = "number";
      else {
        const w = m[4];
        if (KW.has(w)) {
          cls = "keyword";
          pendingDef = DEFS[w] ?? pendingDef;
          tokens.push(line, m.index, w.length, idx(cls));
          continue;
        }
        const call = text
          .slice(m.index + w.length)
          .trimStart()
          .startsWith("(");
        cls = call ? "function" : /^[A-Z][A-Z0-9_]+$/.test(w) ? "constant" : /^[A-Z]/.test(w) ? "type" : "name";
        if (pendingDef) {
          symbols.push({ name: w, kind: pendingDef, line, col: m.index });
          pendingDef = null;
        }
      }
      tokens.push(line, m.index, m[0].length, idx(cls));
    }
  });
  return {
    path,
    provider: "mock",
    language,
    classes,
    tokens,
    symbols,
    imports: imports(key, path, code),
  };
}

function imports(key: string, path: string, code: string): CodeImport[] {
  const paths = mockTexts(key).map((t) => t.path);
  const dir = path.split("/").slice(0, -1);
  const out: CodeImport[] = [];
  code.split("\n").forEach((text, i) => {
    const js = /^\s*import .* from ["']([^"']+)["']/.exec(text);
    const rs = /^\s*use ([\w:]+)/.exec(text);
    if (js) {
      const spec = js[1];
      let target: string | null = null;
      if (spec.startsWith(".")) {
        const segs = [...dir];
        for (const s of spec.split("/")) {
          if (s === "..") segs.pop();
          else if (s !== ".") segs.push(s);
        }
        const base = segs.join("/");
        target = paths.find((p) => p === base || p.replace(/\.(tsx?|jsx?)$/, "") === base) ?? null;
      }
      out.push({ spec, line: i + 1, target, folder: false });
    } else if (rs) {
      const spec = rs[1];
      const mod = spec.startsWith("crate::") ? spec.split("::")[1] : null;
      const target = mod ? (paths.find((p) => p.endsWith(`src/${mod}.rs`) || p.endsWith(`src/${mod}/mod.rs`)) ?? null) : null;
      out.push({ spec, line: i + 1, target, folder: false });
    }
  });
  return out;
}

export function mockCodeFile(key: string, path: string): CodeFile {
  return analyze(key, path, fileText(key, path));
}

function occurrences(key: string, symbol: string) {
  const re = new RegExp(`\\b${symbol.replace(/[^\w]/g, "")}\\b`, "g");
  const out: { loc: CodeLocation; def: CodeSymbol | undefined }[] = [];
  for (const { path, code } of mockTexts(key)) {
    const defs = analyze(key, path, code).symbols;
    code.split("\n").forEach((text, i) => {
      re.lastIndex = 0;
      let m: RegExpExecArray | null;
      while ((m = re.exec(text))) {
        const def = defs.find((d) => d.name === symbol && d.line === i + 1 && d.col === m!.index);
        out.push({
          loc: {
            name: symbol,
            path,
            line: i + 1,
            col: m.index,
            len: symbol.length,
            preview: text.trim(),
            kind: def?.kind ?? null,
          },
          def,
        });
      }
    });
  }
  return out;
}

export function mockCodeUsages(key: string, symbol: string, from?: string): CodeUsages {
  const all = occurrences(key, symbol);
  const refs = all.filter((o) => !o.def).map((o) => o.loc);
  refs.sort((a, b) => Number(b.path === from) - Number(a.path === from));
  return {
    symbol,
    provider: "mock",
    definitions: all.filter((o) => o.def).map((o) => o.loc),
    references: refs,
    truncated: false,
  };
}

export function mockCodeDeps(key: string, path: string): CodeDeps {
  const own = mockCodeFile(key, path).imports;
  const importers = mockTexts(key).flatMap((t) =>
    t.path === path
      ? []
      : imports(key, t.path, t.code)
          .filter((im) => im.target === path)
          .map((im) => ({ path: t.path, line: im.line, spec: im.spec })),
  );
  return { path, provider: "mock", imports: own, importers, truncated: false };
}

export function mockCodeSymbols(key: string, query: string, limit: number): CodeSymbols {
  const q = query.trim().toLowerCase();
  const items: CodeLocation[] = [];
  for (const { path, code } of mockTexts(key)) {
    const lines = code.split("\n");
    for (const s of analyze(key, path, code).symbols) {
      if (q && s.name.toLowerCase().includes(q))
        items.push({
          name: s.name,
          path,
          line: s.line,
          col: s.col,
          len: s.name.length,
          preview: lines[s.line - 1].trim(),
          kind: s.kind,
          container: s.container ?? null,
        });
    }
  }
  items.sort((a, b) => Number(b.preview.includes(query)) - Number(a.preview.includes(query)));
  return {
    provider: "mock",
    items: items.slice(0, limit),
    truncated: items.length > limit,
  };
}
