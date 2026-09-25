//! The code navigation tools an agent calls (`ostra_core::agent::CODE_TOOLS`): input parsing and
//! compact text answers over one project's index. Paths in and out are project-relative; an
//! absolute path inside the project is accepted too.

use crate::graph::Direction;
use crate::index::{ProjectIndex, read_text};
use crate::render;
use ostra_core::code::{CodeLocation, SymbolKind};
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Write;
use std::path::Path;

const DEFAULT_LIMIT: usize = 40;
const MAX_LIMIT: usize = 300;
const MAX_NAMES: usize = 12;

fn kind_word(k: SymbolKind) -> &'static str {
    match k {
        SymbolKind::Function => "fn",
        SymbolKind::Method => "method",
        SymbolKind::Class => "class",
        SymbolKind::Enum => "enum",
        SymbolKind::Interface => "interface",
        SymbolKind::Type => "type",
        SymbolKind::Module => "module",
        SymbolKind::Constant => "const",
        SymbolKind::Variable => "var",
        SymbolKind::Field => "field",
        SymbolKind::Macro => "macro",
    }
}

fn lines(line: u32, end: Option<u32>) -> String {
    match end {
        Some(e) if e > line => format!("L{line}-{e}"),
        _ => format!("L{line}"),
    }
}

fn names(v: &[String]) -> String {
    if v.len() <= MAX_NAMES {
        return v.join(", ");
    }
    format!(
        "{}, +{} more",
        v[..MAX_NAMES].join(", "),
        v.len() - MAX_NAMES
    )
}

fn loc(l: &CodeLocation) -> String {
    let mut s = format!("{}:{}", l.path, l.line);
    if let Some(k) = l.kind {
        write!(s, " {} {}", kind_word(k), l.name).unwrap();
    }
    if let Some(c) = &l.container {
        write!(s, " in {c}").unwrap();
    }
    if let Some(v) = &l.via {
        write!(s, " (impl {v})").unwrap();
    }
    s
}

fn limit(v: Option<usize>) -> usize {
    v.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// A model-given path as the index keys it.
fn rel(root: &Path, p: &str) -> String {
    let p = p.trim();
    let path = Path::new(p);
    if path.is_absolute()
        && let Ok(r) = path.strip_prefix(root)
    {
        return r.to_string_lossy().into_owned();
    }
    let p = p.trim_start_matches("./");
    // An agent that works across the workspace names files from the workspace root, starting
    // with the project's folder.
    if let Some(dir) = root.file_name().and_then(|d| d.to_str())
        && let Some(rest) = p.strip_prefix(dir).and_then(|r| r.strip_prefix('/'))
        && !root.join(p).exists()
        && root.join(rest).exists()
    {
        return rest.to_string();
    }
    p.to_string()
}

fn parse<T: for<'de> Deserialize<'de>>(input: &Value) -> Result<T, String> {
    serde_json::from_value(input.clone()).map_err(|e| format!("Invalid input: {e}."))
}

fn not_indexed(path: &str) -> String {
    format!(
        "`{path}` is not in the code index. Give a project-relative path of a source file; the index \
         leaves out ignored files, files over 1 MB, and languages it does not parse."
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathIn {
    path: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindIn {
    query: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallersIn {
    symbol: String,
    path: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CalleesIn {
    path: String,
    symbol: String,
    line: Option<u32>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImplementationsIn {
    symbol: String,
    path: Option<String>,
    line: Option<u32>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImpactIn {
    #[serde(default)]
    paths: Vec<String>,
    symbol: Option<String>,
    path: Option<String>,
    direction: Option<Direction>,
    depth: Option<u32>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MapIn {
    prefix: Option<String>,
}

/// Run code tool `tool` (a native name such as `CodeCallers`). `Err` is a tool error for the model.
pub fn run(ix: &mut ProjectIndex, tool: &str, input: &Value) -> Result<String, String> {
    let root = ix.root().to_path_buf();
    match tool {
        "CodeOutline" => outline(ix, &root, parse(input)?),
        "CodeFind" => find(ix, parse(input)?),
        "CodeCallers" => callers(ix, &root, parse(input)?),
        "CodeCallees" => callees(ix, &root, parse(input)?),
        "CodeImplementations" => implementations(ix, &root, parse(input)?),
        "CodeNeighbors" => neighbors(ix, &root, parse(input)?),
        "CodeImpact" => impact(ix, &root, parse(input)?),
        "CodeMap" => map(ix, parse(input)?),
        other => Err(format!("Unknown code tool `{other}`.")),
    }
}

fn outline(ix: &mut ProjectIndex, root: &Path, a: PathIn) -> Result<String, String> {
    let path = rel(root, &a.path);
    let src = read_text(&root.join(&path)).ok_or_else(|| not_indexed(&path))?;
    let f = render(&path, &src, Some(&ix.resolver));
    let Some(lang) = f.language else {
        return Err(not_indexed(&path));
    };
    let n = src.lines().count();
    let mut out = format!("{path} ({lang}, {n} lines)\n");
    if !f.imports.is_empty() {
        out.push_str("imports:\n");
        for i in &f.imports {
            let to = i.target.as_deref().unwrap_or("external");
            writeln!(out, "  L{} {} -> {to}", i.line, i.spec).unwrap();
        }
    }
    let cap = limit(a.limit.or(Some(MAX_LIMIT)));
    out.push_str("symbols:\n");
    for s in f.symbols.iter().take(cap) {
        let indent = if s.container.is_some() { "    " } else { "  " };
        write!(
            out,
            "{indent}{} {} {}",
            lines(s.line, s.end_line),
            kind_word(s.kind),
            s.name
        )
        .unwrap();
        if let Some(c) = &s.container {
            write!(out, " in {c}").unwrap();
        }
        out.push('\n');
    }
    if f.symbols.len() > cap {
        writeln!(out, "  ({} more symbols)", f.symbols.len() - cap).unwrap();
    }
    Ok(out)
}

fn find(ix: &mut ProjectIndex, a: FindIn) -> Result<String, String> {
    let r = ix.symbols(&a.query, limit(a.limit));
    if r.items.is_empty() {
        return Ok(format!("No definition matches `{}`.", a.query));
    }
    let mut out = String::new();
    for l in &r.items {
        writeln!(out, "{}  {}", loc(l), l.preview).unwrap();
    }
    if r.truncated {
        out.push_str("(more matches; narrow the query or raise limit)\n");
    }
    Ok(out)
}

fn callers(ix: &mut ProjectIndex, root: &Path, a: CallersIn) -> Result<String, String> {
    let mut path = a.path.map(|p| rel(root, &p));
    if path.is_none() {
        // One defining file makes the answer exact; several leave it by name.
        let files = ix.definition_files(&a.symbol);
        if let [only] = files.as_slice() {
            path = Some(only.clone());
        }
    }
    let c = ix.callers(&a.symbol, path.as_deref(), limit(a.limit));
    let mut out = String::new();
    if c.definitions.is_empty() {
        writeln!(out, "`{}` has no definition in the index.", a.symbol).unwrap();
    } else {
        out.push_str("defined at:\n");
        for d in &c.definitions {
            writeln!(out, "  {}", loc(d)).unwrap();
        }
        if path.is_none() && c.definitions.len() > 1 {
            out.push_str(
                "  (several definitions: pass `path` of the one you mean to drop mentions of the others)\n",
            );
        }
    }
    if c.callers.is_empty() {
        out.push_str("no callers found\n");
        return Ok(out);
    }
    let files = {
        let mut f: Vec<&str> = c.callers.iter().map(|x| x.path.as_str()).collect();
        f.dedup();
        f.len()
    };
    writeln!(
        out,
        "used by ({} places in {files} files):",
        c.callers.len()
    )
    .unwrap();
    for x in &c.callers {
        let at = x
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join(",");
        match (&x.name, x.kind) {
            (Some(n), Some(k)) => {
                let within = x
                    .container
                    .as_deref()
                    .map(|c| format!(" in {c}"))
                    .unwrap_or_default();
                writeln!(
                    out,
                    "  {}  {} {n}{within}  lines {at}",
                    x.path,
                    kind_word(k)
                )
                .unwrap();
            }
            _ if x.import => writeln!(out, "  {}  import  line {at}", x.path).unwrap(),
            _ => writeln!(out, "  {}  top level  lines {at}", x.path).unwrap(),
        }
    }
    if c.truncated {
        out.push_str("(more callers; raise limit)\n");
    }
    Ok(out)
}

fn callees(ix: &mut ProjectIndex, root: &Path, a: CalleesIn) -> Result<String, String> {
    let path = rel(root, &a.path);
    let c = ix
        .callees(&path, &a.symbol, a.line, limit(a.limit))
        .ok_or_else(|| {
            format!(
                "`{}` is not defined in `{path}`. Use CodeOutline on the file to see its definitions.",
                a.symbol
            )
        })?;
    let mut out = format!(
        "{} in {path} {} uses:\n",
        c.symbol,
        lines(c.line, c.end_line)
    );
    if c.end_line.is_none() {
        out.push_str("  (the body's end is unknown; only its first line was read)\n");
    }
    if c.uses.is_empty() {
        out.push_str("  nothing defined in this project\n");
    }
    for l in &c.uses {
        writeln!(out, "  {}", loc(l)).unwrap();
    }
    if c.truncated {
        out.push_str("(more; raise limit)\n");
    }
    Ok(out)
}

/// Definitions a CodeImplementations answer lists when the name has several.
const MAX_IMPL_GROUPS: usize = 8;

fn implementations(
    ix: &mut ProjectIndex,
    root: &Path,
    a: ImplementationsIn,
) -> Result<String, String> {
    let path = a.path.map(|p| rel(root, &p));
    let r = ix.implementations(
        &a.symbol,
        path.as_deref(),
        a.line,
        MAX_IMPL_GROUPS,
        limit(a.limit),
    );
    if r.definitions == 0 {
        return Err(match &path {
            Some(p) => format!(
                "No type or method `{}` is defined in `{p}`. Use CodeOutline on the file to see its definitions.",
                a.symbol
            ),
            None => format!(
                "No type or method `{}` is in the index. Use CodeFind to look the name up.",
                a.symbol
            ),
        });
    }
    let mut out = String::new();
    if r.definitions > 1 {
        writeln!(
            out,
            "`{}` has {} definitions; showing the ones with implementation links. Pass `path` for one.",
            a.symbol, r.definitions
        )
        .unwrap();
    }
    for x in &r.links {
        writeln!(out, "{}", loc(&x.definition)).unwrap();
        let section = |out: &mut String, title: &str, v: &[CodeLocation]| {
            if v.is_empty() {
                writeln!(out, "  {title}: none").unwrap();
                return;
            }
            writeln!(out, "  {title} ({}):", v.len()).unwrap();
            for l in v {
                writeln!(out, "    {}", loc(l)).unwrap();
            }
        };
        section(&mut out, "implements or extends", &x.implements);
        section(&mut out, "implemented or extended by", &x.implemented_by);
    }
    if r.truncated {
        out.push_str("(more; pass `path` or raise limit)\n");
    }
    Ok(out)
}

fn neighbors(ix: &mut ProjectIndex, root: &Path, a: PathIn) -> Result<String, String> {
    let path = rel(root, &a.path);
    let cap = limit(a.limit);
    let n = ix.neighbors(&path, cap).ok_or_else(|| not_indexed(&path))?;
    let unit = if n.unit.is_empty() {
        "."
    } else {
        n.unit.as_str()
    };
    let mut out = format!(
        "{path} (package {unit}{})\n",
        if n.test { ", test" } else { "" }
    );
    let side = |out: &mut String, title: &str, links: &[crate::graph::Link]| {
        writeln!(out, "{title} ({}):", links.len()).unwrap();
        if links.is_empty() {
            out.push_str("  none\n");
        }
        for l in links {
            write!(out, "  {}", l.path).unwrap();
            if l.module {
                out.push_str("  [child module]");
            } else if !l.import_lines.is_empty() {
                let ls: Vec<String> = l.import_lines.iter().map(|x| x.to_string()).collect();
                write!(out, "  [import L{}]", ls.join(",")).unwrap();
            }
            if !l.names.is_empty() {
                write!(out, "  {}", names(&l.names)).unwrap();
            }
            out.push('\n');
        }
    };
    side(&mut out, "uses", &n.uses);
    side(&mut out, "used by", &n.used_by);
    if n.truncated {
        writeln!(out, "(lists cut at {cap}; raise limit)").unwrap();
    }
    let related: Vec<_> = ix
        .related(&path, 8)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.distance == 2)
        .take(5)
        .collect();
    if !related.is_empty() {
        out.push_str("also related (through a shared neighbor):\n");
        for r in related {
            let via = r.via.map(|v| format!(" via {v}")).unwrap_or_default();
            writeln!(out, "  {}{via}", r.path).unwrap();
        }
    }
    Ok(out)
}

/// The one file that defines `symbol`, or `path` when given.
fn defining_file(
    ix: &ProjectIndex,
    root: &Path,
    symbol: &str,
    path: Option<String>,
) -> Result<String, String> {
    if let Some(p) = path {
        return Ok(rel(root, &p));
    }
    match ix.definition_files(symbol).as_slice() {
        [] => Err(format!("`{symbol}` has no definition in the index.")),
        [only] => Ok(only.clone()),
        many => Err(format!(
            "`{symbol}` is defined in {} files: {}. Pass `path`, the file of the one you mean.",
            many.len(),
            many.join(", ")
        )),
    }
}

fn symbol_impact(
    ix: &mut ProjectIndex,
    root: &Path,
    symbol: &str,
    path: Option<String>,
    depth: u32,
    cap: usize,
) -> Result<String, String> {
    let file = defining_file(ix, root, symbol, path)?;
    let r = ix.symbol_impact(symbol, &file, depth, cap);
    if r.definitions.is_empty() {
        return Err(format!(
            "`{symbol}` is not defined in `{file}`. Use CodeFind to see where it is defined."
        ));
    }
    let mut out = String::from("changing ");
    for d in &r.definitions {
        write!(out, "{} ", loc(d)).unwrap();
    }
    out.push_str("affects:\n");
    if r.hops.is_empty() {
        out.push_str("  nothing: no other definition uses it\n");
    }
    let mut files: Vec<&str> = vec![];
    for (i, hop) in r.hops.iter().enumerate() {
        if i == 0 {
            out.push_str("hop 1 (uses it directly):\n");
        } else {
            writeln!(out, "hop {} (uses something in hop {i}):", i + 1).unwrap();
        }
        for u in hop {
            let at = u
                .lines
                .iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let who = match (&u.name, u.kind) {
                (Some(n), Some(k)) => {
                    let within = u
                        .container
                        .as_deref()
                        .map(|c| format!(" in {c}"))
                        .unwrap_or_default();
                    format!("{} {n}{within}", kind_word(k))
                }
                _ => "top level".into(),
            };
            let via = if i == 0 {
                String::new()
            } else {
                format!("  via {}", u.uses)
            };
            writeln!(out, "  {}  {who}  lines {at}{via}", u.path).unwrap();
            if !files.contains(&u.path.as_str()) {
                files.push(&u.path);
            }
        }
    }
    if !files.is_empty() {
        writeln!(
            out,
            "files to change or recheck ({}): {}",
            files.len(),
            files.join(", ")
        )
        .unwrap();
    }
    if r.truncated {
        out.push_str("(cut short; lower depth or raise limit)\n");
    }
    Ok(out)
}

fn impact(ix: &mut ProjectIndex, root: &Path, a: ImpactIn) -> Result<String, String> {
    let depth = a.depth.unwrap_or(3).clamp(1, 10);
    let cap = limit(a.limit.or(Some(100)));
    match (&a.symbol, a.paths.is_empty()) {
        (Some(_), false) => {
            return Err("Give either `symbol` or `paths`, not both.".into());
        }
        (Some(sym), true) => {
            if a.direction == Some(Direction::Dependencies) {
                return Err(
                    "With `symbol` the walk follows its users. For what a symbol uses, call CodeCallees."
                        .into(),
                );
            }
            return symbol_impact(ix, root, sym, a.path, depth, cap);
        }
        (None, true) => {
            return Err("Give `symbol` (one function or type) or `paths` (whole files).".into());
        }
        (None, false) => {}
    }
    let seeds: Vec<String> = a.paths.iter().map(|p| rel(root, p)).collect();
    let dir = a.direction.unwrap_or(Direction::Dependents);
    let r = ix.reach(&seeds, dir, depth, cap);
    let mut out = String::new();
    for m in &r.missing {
        writeln!(out, "{}", not_indexed(m)).unwrap();
    }
    let what = match dir {
        Direction::Dependents => "depend on",
        Direction::Dependencies => "are used by",
    };
    writeln!(
        out,
        "{} files {what} {} within {depth} hops:",
        r.reached.len(),
        seeds.join(", ")
    )
    .unwrap();
    let mut last = 0;
    for x in &r.reached {
        if x.distance != last {
            last = x.distance;
            writeln!(out, "hop {last}:").unwrap();
        }
        write!(out, "  {}", x.path).unwrap();
        if last > 1 {
            write!(out, "  via {}", x.via).unwrap();
        }
        if !x.names.is_empty() {
            write!(out, "  {}", names(&x.names)).unwrap();
        }
        out.push('\n');
    }
    if r.truncated {
        out.push_str("(cut short; lower depth or raise limit)\n");
    }
    Ok(out)
}

fn map(ix: &mut ProjectIndex, a: MapIn) -> Result<String, String> {
    let prefix = a.prefix.unwrap_or_default();
    let prefix = prefix.trim_start_matches("./").trim_end_matches('/');
    let inside = |p: &str| prefix.is_empty() || p == prefix || p.starts_with(&format!("{prefix}/"));
    let m = ix.units();
    let mut out = format!("{} files indexed\n", ix.files());
    out.push_str("packages (files; depends on, with link counts):\n");
    for u in m
        .units
        .iter()
        .filter(|u| inside(&u.path) || u.path.is_empty())
    {
        let deps: Vec<String> = u
            .depends_on
            .iter()
            .map(|d| {
                format!(
                    "{} ({})",
                    if d.path.is_empty() { "." } else { &d.path },
                    d.links
                )
            })
            .collect();
        let path = if u.path.is_empty() {
            "."
        } else {
            u.path.as_str()
        };
        write!(out, "  {path}  {} files", u.files).unwrap();
        if u.tests > 0 {
            write!(out, ", {} tests", u.tests).unwrap();
        }
        if !deps.is_empty() {
            write!(out, "; {}", deps.join(", ")).unwrap();
        }
        out.push('\n');
    }
    if !m.cycles.is_empty() {
        out.push_str("package cycles:\n");
        for c in &m.cycles {
            writeln!(out, "  {}", c.join(" <-> ")).unwrap();
        }
    }
    let hubs: Vec<_> = ix
        .hubs(200)
        .into_iter()
        .filter(|h| inside(&h.path))
        .take(15)
        .collect();
    if !hubs.is_empty() {
        out.push_str("most depended-on files (dependents):\n");
        for h in hubs {
            writeln!(out, "  {}  {}", h.path, h.dependents).unwrap();
        }
    }
    let cycles: Vec<_> = ix
        .cycles(50)
        .into_iter()
        .filter(|c| c.iter().any(|p| inside(p)))
        .take(10)
        .collect();
    if !cycles.is_empty() {
        out.push_str("import cycles:\n");
        for c in cycles {
            writeln!(out, "  {}", c.join(" <-> ")).unwrap();
        }
    }
    Ok(out)
}
