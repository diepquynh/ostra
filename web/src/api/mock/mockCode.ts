// A naive code provider for the mock server: a regex tokenizer, definitions after a definition keyword,
// usages by whole-word match, and imports from `use` and `import ... from` lines.

import type { CodeDeps, CodeFile, CodeGraph, CodeGraphEdge, CodeGraphNode, CodeImport, CodeLocation, CodeSymbol, CodeSymbols, CodeUsages, SymbolKind, TokenClass } from "../types";
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

/** The dependency graph over the mock files' imports; a file's top folder is its package. */
export function mockCodeGraph(key: string, at: { package?: string; path?: string; symbol?: string; line?: number; depth?: number }): CodeGraph {
  if (at.path && at.symbol) return mockSymbolGraph(key, at.path, at.symbol, at.line);
  const files = mockTexts(key).filter((t) => langOf(t.path));
  const pkgOf = (p: string) => (p.includes("/") ? p.split("/")[0] : "");
  const uses = new Map<string, Set<string>>();
  for (const f of files) {
    const targets = imports(key, f.path, f.code).flatMap((i) => (i.target && i.target !== f.path ? [i.target] : []));
    uses.set(f.path, new Set(targets));
  }
  const used = (p: string) => [...uses].filter(([, t]) => t.has(p)).map(([f]) => f);
  const pkgId = (p: string) => `package:${p}`;
  const fileNode = (p: string, column: number): CodeGraphNode => ({
    id: p, kind: "file", label: p.split("/").pop() ?? p, package: pkgOf(p), column, files: 1,
    test: /test/.test(p), dependents: used(p).length, dependencies: uses.get(p)?.size ?? 0,
  });
  const fileEdge = (from: string, to: string): CodeGraphEdge => ({ from, to, weight: 1, names: [], import: true, kind: "uses" });
  /** Longest chain of uses below each node, negated, so users sit left of what they use. */
  const columns = (ids: string[], out: (id: string) => string[]) => {
    const level = new Map<string, number>();
    const visit = (id: string, seen: Set<string>): number => {
      if (level.has(id)) return level.get(id)!;
      if (seen.has(id)) return 0;
      seen.add(id);
      const l = Math.max(-1, ...out(id).map((o) => visit(o, seen))) + 1;
      level.set(id, l);
      return l;
    };
    ids.forEach((id) => visit(id, new Set()));
    return (id: string) => -(level.get(id) ?? 0);
  };
  const base = { indexed_files: files.length, truncated: false };
  if (at.path) {
    if (!uses.has(at.path)) throw Object.assign(new Error(`${at.path} is not in the code index.`), { status: 404 });
    const col = new Map<string, number>([[at.path, 0]]);
    const walk = (next: (p: string) => string[], sign: number) => {
      let frontier = [at.path!];
      for (let d = 1; d <= (at.depth ?? 1); d++) {
        frontier = frontier.flatMap(next).filter((p) => !col.has(p) && col.set(p, sign * d));
      }
    };
    walk((p) => [...(uses.get(p) ?? [])], 1);
    walk(used, -1);
    const ids = [...col.keys()];
    return {
      ...base, view: "file", focus: at.path,
      nodes: ids.map((p) => fileNode(p, col.get(p)!)),
      edges: ids.flatMap((p) => [...(uses.get(p) ?? [])].filter((t) => col.has(t)).map((t) => fileEdge(p, t))),
    };
  }
  if (at.package !== undefined) {
    const pkg = at.package === "." ? "" : at.package;
    const members = files.map((f) => f.path).filter((p) => pkgOf(p) === pkg);
    if (!members.length) throw Object.assign(new Error(`No package ${at.package} in the code index.`), { status: 404 });
    const outside = (p: string) => [...(uses.get(p) ?? [])].filter((t) => pkgOf(t) !== pkg).map((t) => pkgId(pkgOf(t)));
    const inside = (p: string) => [...(uses.get(p) ?? [])].filter((t) => pkgOf(t) === pkg);
    const col = columns(members, (id) => (id.startsWith("package:") ? [] : [...inside(id), ...outside(id)]));
    const others = [...new Set(members.flatMap(outside))];
    return {
      ...base, view: "package", focus: pkg,
      nodes: [
        ...members.map((p) => fileNode(p, col(p))),
        ...others.map((id): CodeGraphNode => {
          const name = id.slice("package:".length);
          return { id, kind: "package", label: name || "(project top)", package: name, column: col(id), files: files.filter((f) => pkgOf(f.path) === name).length, test: false, dependents: 0, dependencies: 0 };
        }),
      ],
      edges: [
        ...members.flatMap((p) => inside(p).map((t) => fileEdge(p, t))),
        ...members.flatMap((p) => [...new Set(outside(p))].map((t) => fileEdge(p, t))),
      ],
    };
  }
  const pkgs = [...new Set(files.map((f) => pkgOf(f.path)))];
  const links = new Map<string, number>();
  for (const [f, targets] of uses)
    for (const t of targets)
      if (pkgOf(t) !== pkgOf(f)) links.set(`${pkgOf(f)}\u0000${pkgOf(t)}`, (links.get(`${pkgOf(f)}\u0000${pkgOf(t)}`) ?? 0) + 1);
  const pairs = [...links].map(([k, n]) => [...k.split("\u0000"), n] as [string, string, number]);
  const col = columns(pkgs, (p) => pairs.filter(([a]) => a === p).map(([, b]) => b));
  return {
    ...base, view: "packages", focus: "",
    nodes: pkgs.map((p) => ({
      id: pkgId(p), kind: "package", label: p || "(project top)", package: p, column: col(p),
      files: files.filter((f) => pkgOf(f.path) === p).length, test: false,
      dependents: pairs.filter(([, b]) => b === p).length, dependencies: pairs.filter(([a]) => a === p).length,
    })),
    edges: pairs.map(([a, b, n]) => ({ from: pkgId(a), to: pkgId(b), weight: n, names: [], import: true, kind: "uses" })),
  };
}

/** One definition: the other definitions its body names to the right, the files that mention it to the left. */
function mockSymbolGraph(key: string, path: string, symbol: string, line?: number): CodeGraph {
  const texts = mockTexts(key).filter((t) => langOf(t.path));
  const own = analyze(key, path, fileText(key, path)).symbols;
  const focus = own.filter((s) => s.name === symbol).sort((a, b) => Math.abs(a.line - (line ?? 0)) - Math.abs(b.line - (line ?? 0)))[0];
  if (!focus) throw Object.assign(new Error(`${symbol} is not defined in ${path}.`), { status: 404 });
  const idOf = (s: CodeSymbol) => `symbol:${path}:${s.line}:${s.name}`;
  const next = own.find((s) => s.line > focus.line)?.line ?? Infinity;
  const lines = fileText(key, path).split("\n");
  const body = lines.slice(focus.line, next - 1).join("\n");
  const word = (name: string) => new RegExp(`\\b${name}\\b`);
  const callees = own.filter((s) => s !== focus && s.kind !== "module" && word(s.name).test(body));
  const callers = texts.filter((t) => t.path !== path && word(symbol).test(t.code)).map((t) => t.path);
  const symNode = (s: CodeSymbol, column: number, ins: number, outs: number): CodeGraphNode => ({
    id: idOf(s), kind: "symbol", label: s.container ? `${s.container}.${s.name}` : s.name, package: path.includes("/") ? path.split("/")[0] : "",
    column, files: 1, test: false, dependents: ins, dependencies: outs,
    symbol: {
      path, name: s.name, kind: s.kind, line: s.line, end_line: s.end_line ?? null, container: s.container ?? null, via: s.via ?? null,
      signature: lines[s.line - 1]?.trim().replace(/\s*\{.*$/, "") ?? s.name,
      members: ["class", "enum", "interface", "type"].includes(s.kind)
        ? own.filter((m) => m.container === s.name && (m.kind === "method" || m.kind === "function"))
            .map((m) => ({ path, name: m.name, kind: m.kind, line: m.line, via: m.via ?? null, signature: lines[m.line - 1]?.trim().replace(/\s*\{.*$/, "") ?? m.name }))
        : [],
      more_members: 0,
    },
  });
  return {
    view: "symbol", focus: idOf(focus), indexed_files: texts.length, truncated: false,
    nodes: [
      symNode(focus, 0, callers.length, callees.length),
      ...callees.map((s) => symNode(s, 1, 1, 0)),
      ...callers.map((p): CodeGraphNode => ({ id: p, kind: "file", label: p.split("/").pop() ?? p, package: p.includes("/") ? p.split("/")[0] : "", column: -1, files: 1, test: false, dependents: 0, dependencies: 1 })),
    ],
    edges: [
      ...callees.map((s): CodeGraphEdge => ({ from: idOf(focus), to: idOf(s), weight: 1, names: [], import: false, kind: "uses" })),
      ...callers.map((p): CodeGraphEdge => ({ from: p, to: idOf(focus), weight: 1, names: [], import: false, kind: "uses" })),
    ],
  };
}
