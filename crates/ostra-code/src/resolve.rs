//! Turn raw import specs into project files or folders, per language family, from the project's
//! file list and its manifests (`Cargo.toml` package names, `go.mod` module paths).

use crate::lang::{Family, Lang};
use crate::outline::{ImportStyle, RawImport};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: String,
    pub folder: bool,
}

#[derive(Default)]
pub struct Resolver {
    files: HashSet<String>,
    dirs: HashSet<String>,
    by_name: HashMap<String, Vec<String>>,
    dirs_by_name: HashMap<String, Vec<String>>,
    /// Rust crate name (with `_`) to the crate folder.
    rust_crates: HashMap<String, String>,
    /// Go module path to its folder.
    go_modules: Vec<(String, String)>,
    /// npm package name to its folder and entry points (`.`, `./sub`), for workspace packages.
    js_packages: HashMap<String, (String, HashMap<String, String>)>,
    /// Folders holding a package manifest.
    units: HashSet<String>,
}

const MANIFESTS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "go.mod",
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
    "Gemfile",
    "Package.swift",
    "mix.exs",
];

fn is_manifest(name: &str) -> bool {
    MANIFESTS.contains(&name)
        || name.ends_with(".csproj")
        || name.ends_with(".gemspec")
        || name.ends_with(".rockspec")
}

/// The folder of a project-relative path, `""` at the top.
pub fn parent(p: &str) -> &str {
    p.rsplit_once('/').map_or("", |(d, _)| d)
}

fn file_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn stem(p: &str) -> &str {
    let n = file_name(p);
    n.split_once('.').map_or(n, |(s, _)| s)
}

/// Join a relative path onto a project folder, resolving `.` and `..`. `None` when it leaves the
/// project.
pub fn join(dir: &str, rel: &str) -> Option<String> {
    let mut out: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    Some(out.join("/"))
}

fn ancestors(p: &str) -> impl Iterator<Item = &str> {
    let mut cur = Some(parent(p));
    std::iter::from_fn(move || {
        let d = cur?;
        cur = (!d.is_empty()).then(|| parent(d));
        Some(d)
    })
}

const JS_EXTS: &[&str] = &[
    ".ts", ".tsx", ".d.ts", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts", ".vue", ".svelte",
];

fn cargo_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_package = l == "[package]";
            continue;
        }
        if in_package
            && let Some(rest) = l.strip_prefix("name")
            && let Some(v) = rest.trim_start().strip_prefix('=')
        {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// A `package.json`'s name and entry points: each `exports` subpath, else `module` or `main`
/// for `.`. A conditional export takes its `types`, `import`, or `default` file.
fn npm_package(text: &str) -> Option<(String, HashMap<String, String>)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let name = v.get("name")?.as_str()?.to_string();
    fn target(v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(m) => ["types", "import", "default", "module", "require"]
                .iter()
                .find_map(|k| m.get(*k).and_then(target)),
            _ => None,
        }
    }
    let mut entries = HashMap::new();
    match v.get("exports") {
        Some(serde_json::Value::Object(m)) if m.keys().all(|k| k.starts_with('.')) => {
            for (k, t) in m {
                entries.extend(target(t).map(|t| (k.clone(), t)));
            }
        }
        Some(e) => entries.extend(target(e).map(|t| (".".to_string(), t))),
        None => {}
    }
    if !entries.contains_key(".")
        && let Some(main) = ["types", "module", "main"]
            .iter()
            .find_map(|k| v.get(*k)?.as_str())
    {
        entries.insert(".".into(), main.to_string());
    }
    Some((name, entries))
}

impl Resolver {
    /// `read` returns the text of a project file, for the manifests.
    pub fn new(paths: &[String], read: impl Fn(&str) -> Option<String>) -> Self {
        let mut r = Resolver::default();
        for p in paths {
            r.files.insert(p.clone());
            r.by_name
                .entry(file_name(p).to_string())
                .or_default()
                .push(p.clone());
            for d in ancestors(p) {
                if d.is_empty() || !r.dirs.insert(d.to_string()) {
                    break;
                }
                r.dirs_by_name
                    .entry(file_name(d).to_string())
                    .or_default()
                    .push(d.to_string());
            }
            if is_manifest(file_name(p)) {
                r.units.insert(parent(p).to_string());
            }
            match file_name(p) {
                "Cargo.toml" => {
                    if let Some(name) = read(p).as_deref().and_then(cargo_name) {
                        r.rust_crates
                            .insert(name.replace('-', "_"), parent(p).to_string());
                    }
                }
                "package.json" if !p.split('/').any(|seg| seg == "node_modules") => {
                    if let Some((name, entries)) = read(p).as_deref().and_then(npm_package) {
                        r.js_packages.insert(name, (parent(p).to_string(), entries));
                    }
                }
                "go.mod" => {
                    if let Some(text) = read(p)
                        && let Some(m) = text.lines().find_map(|l| l.trim().strip_prefix("module "))
                    {
                        r.go_modules.push((
                            m.trim().trim_matches('"').to_string(),
                            parent(p).to_string(),
                        ));
                    }
                }
                _ => {}
            }
        }
        r.dirs.insert(String::new());
        r
    }

    /// The package folder that owns `dir`: the nearest one with a manifest, `""` when none does.
    pub fn unit_of_dir<'a>(&self, dir: &'a str) -> &'a str {
        let mut d = dir;
        loop {
            if d.is_empty() || self.units.contains(d) {
                return d;
            }
            d = parent(d);
        }
    }

    fn has(&self, p: &str) -> bool {
        self.files.contains(p)
    }

    fn file(&self, p: Option<String>) -> Option<Target> {
        p.filter(|p| self.has(p)).map(|path| Target {
            path,
            folder: false,
        })
    }

    fn first(&self, cands: impl IntoIterator<Item = Option<String>>) -> Option<Target> {
        cands.into_iter().find_map(|c| self.file(c))
    }

    fn folder(&self, p: Option<String>) -> Option<Target> {
        p.filter(|p| !p.is_empty() && self.dirs.contains(p))
            .map(|path| Target { path, folder: true })
    }

    /// The project file whose path ends with `suffix`, preferring the one nearest `from`.
    fn suffix(&self, from: &str, suffix: &str) -> Option<Target> {
        let list = self.by_name.get(file_name(suffix))?;
        let tail = format!("/{suffix}");
        list.iter()
            .filter(|p| *p == suffix || p.ends_with(&tail))
            .max_by_key(|p| {
                let common = p
                    .split('/')
                    .zip(from.split('/'))
                    .take_while(|(a, b)| a == b)
                    .count();
                (common, std::cmp::Reverse(p.len()))
            })
            .map(|p| Target {
                path: p.clone(),
                folder: false,
            })
    }

    pub fn resolve(&self, from: &str, imp: &RawImport, lang: &Lang) -> Option<Target> {
        let dir = parent(from);
        let spec = imp.spec.as_str();
        match lang.family {
            Family::Rust => self.rust(from, imp),
            Family::Js => self.js(from, spec),
            Family::Python => imp
                .alts
                .iter()
                .find_map(|a| self.python(from, a))
                .or_else(|| self.python(from, spec)),
            Family::Go => self.go_modules.iter().find_map(|(m, d)| {
                let rest = spec.strip_prefix(m.as_str())?;
                if !rest.is_empty() && !rest.starts_with('/') {
                    return None;
                }
                self.folder(join(d, rest))
            }),
            Family::Jvm => match imp.style {
                ImportStyle::Require { .. } => self.first([
                    join(dir, spec.trim_start_matches('/')),
                    join("", spec.trim_start_matches('/')),
                ]),
                _ => self.dotted(from, spec, lang),
            },
            Family::C => {
                let quoted = matches!(imp.style, ImportStyle::Include { quoted: true });
                let local = quoted
                    .then(|| {
                        self.first([
                            join(dir, spec),
                            join("", spec),
                            join("include", spec),
                            join("src", spec),
                        ])
                    })
                    .flatten();
                local.or_else(|| self.suffix(from, spec))
            }
            Family::Ruby => {
                let file = if spec.ends_with(".rb") {
                    spec.to_string()
                } else {
                    format!("{spec}.rb")
                };
                if matches!(imp.style, ImportStyle::Require { relative: true }) {
                    self.first([join(dir, &file)])
                } else {
                    self.first([join("lib", &file), join("", &file)])
                        .or_else(|| self.suffix(from, &file))
                }
            }
            Family::Lua => {
                let base = spec.replace('.', "/");
                self.first([
                    join("", &format!("{base}.lua")),
                    join("", &format!("{base}/init.lua")),
                    join(dir, &format!("{base}.lua")),
                ])
                .or_else(|| self.suffix(from, &format!("{base}.lua")))
            }
            Family::Shell => self.first([join(dir, spec), join("", spec)]),
            Family::Data => None,
        }
    }

    fn rust_src(&self, from: &str) -> Option<String> {
        ancestors(from)
            .find(|d| self.has(&join(d, "Cargo.toml").unwrap_or_default()))
            .and_then(|d| join(d, "src"))
    }

    fn rust_root(&self, src: &str) -> Option<String> {
        [join(src, "lib.rs"), join(src, "main.rs")]
            .into_iter()
            .flatten()
            .find(|p| self.has(p))
    }

    /// The folder holding a Rust file's child modules.
    fn rust_mod_dir(&self, from: &str) -> String {
        let dir = parent(from);
        let in_src = self
            .rust_src(from)
            .is_some_and(|s| from.starts_with(&format!("{s}/")));
        match stem(from) {
            "mod" | "lib" | "main" => dir.to_string(),
            _ if !in_src => dir.to_string(),
            s => join(dir, s).unwrap_or_default(),
        }
    }

    /// The file that declares the module whose children live in `dir`.
    fn rust_mod_file(&self, dir: &str, src: Option<&str>) -> Option<String> {
        if Some(dir) == src {
            return self.rust_root(dir);
        }
        [Some(format!("{dir}.rs")), join(dir, "mod.rs")]
            .into_iter()
            .flatten()
            .find(|p| self.has(p))
    }

    fn rust_child(&self, dir: &str, seg: &str) -> Option<String> {
        [
            join(dir, &format!("{seg}.rs")),
            join(dir, &format!("{seg}/mod.rs")),
        ]
        .into_iter()
        .flatten()
        .find(|p| self.has(p))
    }

    fn rust(&self, from: &str, imp: &RawImport) -> Option<Target> {
        let mod_dir = self.rust_mod_dir(from);
        if imp.style == ImportStyle::Mod {
            return self.file(self.rust_child(&mod_dir, &imp.spec));
        }
        let segs: Vec<&str> = imp.spec.split("::").filter(|s| !s.is_empty()).collect();
        let first = *segs.first()?;
        let src = self.rust_src(from);
        let (mut dir, mut file, rest) = match first {
            "crate" => {
                let s = src.clone()?;
                let root = self.rust_root(&s)?;
                (s, root, &segs[1..])
            }
            "self" => (mod_dir.clone(), from.to_string(), &segs[1..]),
            "super" => {
                let mut d = mod_dir.clone();
                let mut i = 0;
                while segs.get(i) == Some(&"super") {
                    d = parent(&d).to_string();
                    i += 1;
                }
                let f = self.rust_mod_file(&d, src.as_deref())?;
                (d, f, &segs[i..])
            }
            name => {
                if let Some(c) = self.rust_crates.get(name) {
                    let s = join(c, "src")?;
                    let root = self.rust_root(&s)?;
                    (s, root, &segs[1..])
                } else {
                    let f = self.rust_child(&mod_dir, name)?;
                    (join(&mod_dir, name)?, f, &segs[1..])
                }
            }
        };
        for seg in rest {
            match self.rust_child(&dir, seg) {
                Some(f) => {
                    file = f;
                    dir = join(&dir, seg)?;
                }
                None => break,
            }
        }
        self.file(Some(file))
    }

    fn js(&self, from: &str, spec: &str) -> Option<Target> {
        let mut bases = vec![];
        if spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".." {
            bases.push(join(parent(from), spec)?);
        } else if let Some(rest) = spec.strip_prefix("@/").or_else(|| spec.strip_prefix("~/")) {
            for d in ancestors(from) {
                let has_manifest = ["package.json", "tsconfig.json"]
                    .iter()
                    .any(|m| join(d, m).is_some_and(|p| self.has(&p)));
                if has_manifest {
                    bases.extend(join(d, &format!("src/{rest}")));
                    bases.extend(join(d, rest));
                }
            }
        } else if let Some(base) = self.js_package(spec) {
            bases.push(base);
        } else {
            bases.push(spec.strip_prefix('/')?.to_string());
        }
        for base in bases {
            let mut cands = vec![base.clone()];
            for alt in [".js", ".jsx", ".mjs", ".cjs"] {
                if let Some(s) = base.strip_suffix(alt) {
                    cands.push(format!("{s}.ts"));
                    cands.push(format!("{s}.tsx"));
                }
            }
            cands.extend(JS_EXTS.iter().map(|e| format!("{base}{e}")));
            cands.extend(JS_EXTS.iter().map(|e| format!("{base}/index{e}")));
            if let Some(t) = self.first(cands.into_iter().map(Some)) {
                return Some(t);
            }
        }
        None
    }

    /// A workspace package import, `@scope/pkg` or `@scope/pkg/sub`, as a path to resolve.
    fn js_package(&self, spec: &str) -> Option<String> {
        let (name, (dir, entries)) = self
            .js_packages
            .iter()
            .filter(|(n, _)| {
                spec == n.as_str()
                    || spec
                        .strip_prefix(n.as_str())
                        .is_some_and(|r| r.starts_with('/'))
            })
            .max_by_key(|(n, _)| n.len())?;
        let sub = format!(".{}", &spec[name.len()..]);
        match entries.get(&sub) {
            Some(t) => join(dir, t),
            None => join(dir, &sub),
        }
    }

    fn python(&self, from: &str, spec: &str) -> Option<Target> {
        let dots = spec.len() - spec.trim_start_matches('.').len();
        let module = spec[dots..].replace('.', "/");
        let try_at = |base: &str| -> Option<Target> {
            let path = join(base, &module)?;
            if module.is_empty() {
                return self.first([join(base, "__init__.py")]);
            }
            self.first([
                Some(format!("{path}.py")),
                Some(format!("{path}.pyi")),
                join(&path, "__init__.py"),
            ])
            .or_else(|| self.folder(Some(path)))
        };
        if dots > 0 {
            let mut base = parent(from).to_string();
            for _ in 1..dots {
                base = parent(&base).to_string();
            }
            return try_at(&base);
        }
        ancestors(from).chain(["src"]).find_map(try_at)
    }

    fn dotted(&self, from: &str, spec: &str, lang: &Lang) -> Option<Target> {
        let parts: Vec<&str> = spec
            .split(['.', '\\'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if parts.last() == Some(&"*") {
            let rel = parts[..parts.len() - 1].join("/");
            let name = file_name(&rel);
            let tail = format!("/{rel}");
            return self.dirs_by_name.get(name)?.iter().find_map(|d| {
                (d == &rel || d.ends_with(&tail)).then(|| Target {
                    path: d.clone(),
                    folder: true,
                })
            });
        }
        let exts: &[&str] = match lang.id {
            "php" => &[".php"],
            "csharp" => &[".cs"],
            "swift" => &[".swift"],
            _ => &[".java", ".kt", ".scala"],
        };
        for n in (1..=parts.len()).rev() {
            let rel = parts[..n].join("/");
            for e in exts {
                if let Some(t) = self.suffix(from, &format!("{rel}{e}")) {
                    return Some(t);
                }
            }
        }
        None
    }
}
