use ostra_code::Indexes;
use ostra_code::provider::{
    Answer, CodeProvider, CommandProvider, NativeProvider, ask, parse_answer,
};
use ostra_core::api::FileIndex;
use ostra_core::code::{PROTOCOL_VERSION, ProviderRequest, SymbolKind};
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const FILES: &[(&str, &str)] = &[
    ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
    ("crates/core/Cargo.toml", "[package]\nname = \"my-core\"\n"),
    (
        "crates/core/src/lib.rs",
        "pub mod api;\npub fn helper() {}\n",
    ),
    (
        "crates/core/src/api.rs",
        "pub struct Thing;\nimpl Thing {\n    pub fn go() {}\n}\n",
    ),
    ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
    (
        "crates/app/src/main.rs",
        "use my_core::api::Thing;\nmod util;\nfn main() {\n    Thing::go();\n    util::run();\n}\n",
    ),
    (
        "crates/app/src/util.rs",
        "use super::Thing;\npub fn run() {\n    let t = Thing;\n}\n",
    ),
    ("web/package.json", "{}"),
    (
        "web/src/a.ts",
        "import { b } from \"./b\";\nimport x from \"@/lib/x\";\nimport React from \"react\";\n",
    ),
    ("web/src/b.ts", "export const b = 1;\n"),
    ("web/src/lib/x.tsx", "export default function x() {}\n"),
    ("py/pkg/__init__.py", ""),
    ("py/pkg/mod.py", "from .other import thing\n\nthing()\n"),
    ("py/pkg/other.py", "def thing():\n    pass\n"),
    ("go.mod", "module example.com/app\n\ngo 1.22\n"),
    ("server/server.go", "package server\n\nfunc Start() {}\n"),
    (
        "cmd/main.go",
        "package main\n\nimport \"example.com/app/server\"\n\nfunc main() { server.Start() }\n",
    ),
];

fn project() -> tempfile::TempDir {
    static CACHE: std::sync::Once = std::sync::Once::new();
    // SAFETY: set once, before any test here starts a sandboxed program that reads it.
    CACHE.call_once(|| unsafe {
        std::env::set_var(
            "OSTRA_SANDBOX_CACHE",
            std::env::temp_dir().join("ostra-test-sandbox-cache"),
        );
    });
    let dir = tempfile::tempdir().unwrap();
    for (p, text) in FILES {
        write(dir.path(), p, text);
    }
    dir
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn list(extra: &[&str]) -> Arc<FileIndex> {
    let mut paths: Vec<String> = FILES.iter().map(|(p, _)| p.to_string()).collect();
    paths.extend(extra.iter().map(|s| s.to_string()));
    paths.sort();
    Arc::new(FileIndex {
        paths,
        truncated: false,
    })
}

fn targets(
    ix: &Indexes<&'static str>,
    root: &Path,
    l: &Arc<FileIndex>,
    path: &str,
) -> Vec<(String, Option<String>)> {
    ix.with(&"p", root, l, |i| i.deps(path))
        .imports
        .into_iter()
        .map(|i| (i.spec, i.target))
        .collect()
}

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

#[test]
fn imports_resolve_per_language() {
    let dir = project();
    let root = dir.path();
    let ix = Indexes::default();
    let l = list(&[]);
    assert_eq!(
        targets(&ix, root, &l, "crates/app/src/main.rs"),
        vec![
            ("my_core::api::Thing".into(), s("crates/core/src/api.rs")),
            ("util".into(), s("crates/app/src/util.rs")),
        ]
    );
    assert_eq!(
        targets(&ix, root, &l, "crates/app/src/util.rs"),
        vec![("super::Thing".into(), s("crates/app/src/main.rs"))]
    );
    assert_eq!(
        targets(&ix, root, &l, "web/src/a.ts"),
        vec![
            ("./b".into(), s("web/src/b.ts")),
            ("@/lib/x".into(), s("web/src/lib/x.tsx")),
            ("react".into(), None),
        ]
    );
    assert_eq!(
        targets(&ix, root, &l, "py/pkg/mod.py"),
        vec![(".other".into(), s("py/pkg/other.py"))]
    );
    let go = ix.with(&"p", root, &l, |i| i.deps("cmd/main.go"));
    assert_eq!(go.imports[0].target.as_deref(), Some("server"));
    assert!(go.imports[0].folder);
}

#[test]
fn importers_include_crates_and_go_packages() {
    let dir = project();
    let root = dir.path();
    let ix = Indexes::default();
    let l = list(&[]);
    let api = ix.with(&"p", root, &l, |i| i.deps("crates/core/src/api.rs"));
    let by: Vec<_> = api
        .importers
        .iter()
        .map(|i| (i.path.as_str(), i.line))
        .collect();
    assert_eq!(
        by,
        vec![("crates/app/src/main.rs", 1), ("crates/core/src/lib.rs", 1)]
    );
    let lib = ix.with(&"p", root, &l, |i| i.deps("crates/core/src/lib.rs"));
    assert!(lib.importers.is_empty());
    let server = ix.with(&"p", root, &l, |i| i.deps("server/server.go"));
    assert_eq!(server.importers[0].path, "cmd/main.go");
}

#[test]
fn usages_find_definitions_and_references_then_follow_edits() {
    let dir = project();
    let root = dir.path();
    let ix = Indexes::default();
    let l = list(&[]);
    let u = ix.with(&"p", root, &l, |i| {
        i.usages("Thing", Some("crates/app/src/util.rs"), 100)
    });
    let defs: Vec<_> = u
        .definitions
        .iter()
        .map(|d| (d.path.as_str(), d.line, d.kind))
        .collect();
    assert_eq!(
        defs,
        vec![("crates/core/src/api.rs", 1, Some(SymbolKind::Class))]
    );
    let refs: Vec<_> = u
        .references
        .iter()
        .map(|r| (r.path.as_str(), r.line, r.col))
        .collect();
    assert_eq!(
        refs,
        vec![
            ("crates/app/src/util.rs", 1, 11),
            ("crates/app/src/util.rs", 3, 12),
            ("crates/app/src/main.rs", 1, 18),
            ("crates/app/src/main.rs", 4, 4),
            ("crates/core/src/api.rs", 2, 5),
        ]
    );
    assert_eq!(u.references[1].preview, "let t = Thing;");

    write(root, "crates/app/src/util.rs", "pub fn run() {}\n");
    ix.touch(&"p", "crates/app/src/util.rs");
    let u = ix.with(&"p", root, &l, |i| i.usages("Thing", None, 100));
    assert!(
        u.references
            .iter()
            .all(|r| r.path != "crates/app/src/util.rs")
    );

    write(
        root,
        "web/src/c.ts",
        "import { b } from './b';\nexport class Thing {}\n",
    );
    let l2 = list(&["web/src/c.ts"]);
    let u = ix.with(&"p", root, &l2, |i| i.usages("Thing", None, 100));
    assert_eq!(u.definitions.len(), 2);
    let b = ix.with(&"p", root, &l2, |i| i.deps("web/src/b.ts"));
    let by: Vec<_> = b.importers.iter().map(|i| i.path.as_str()).collect();
    assert_eq!(by, vec!["web/src/a.ts", "web/src/c.ts"]);

    let capped = ix.with(&"p", root, &l2, |i| i.usages("Thing", None, 1));
    assert_eq!(capped.references.len(), 1);
    assert!(capped.truncated);
}

#[test]
fn symbol_search_ranks_exact_prefix_substring_initials() {
    let dir = project();
    let root = dir.path();
    write(
        root,
        "src/x.py",
        "def parse_name():\n    pass\nclass Something: pass\n",
    );
    let ix = Indexes::default();
    let l = list(&["src/x.py"]);
    let names = |q: &str| -> Vec<String> {
        ix.with(&"p", root, &l, |i| i.symbols(q, 20))
            .items
            .into_iter()
            .map(|i| format!("{} {}", i.name, i.path))
            .collect()
    };
    assert_eq!(
        names("thing"),
        vec![
            "thing py/pkg/other.py",
            "Thing crates/core/src/api.rs",
            "Something src/x.py"
        ]
    );
    assert_eq!(names("pn"), vec!["parse_name src/x.py"]);
    assert!(names("").is_empty());
}

#[cfg(unix)]
fn script(dir: &Path, body: &str) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("provider.sh");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    vec![p.to_string_lossy().into_owned()]
}

#[cfg(unix)]
#[tokio::test]
async fn command_provider_answers_defers_and_falls_back() {
    let dir = project();
    let root = dir.path().to_path_buf();
    // Inside the project, because the provider runs sandboxed with a private /tmp.
    let bin = root.join("tools");
    std::fs::create_dir_all(&bin).unwrap();
    let command = script(
        &bin,
        r#"read line
case "$line" in
  *'"op":"symbols"'*) echo '{"items":[{"name":"Remote","path":"src/r.rs","line":2,"col":0,"len":6,"preview":"struct Remote;"}]}' ;;
  *'"op":"usages"'*) echo null ;;
  *'"op":"file"'*) sleep 5 ;;
  *) echo "no index for this" >&2; exit 3 ;;
esac"#,
    );
    let native: Arc<dyn CodeProvider> = Arc::new(NativeProvider {
        indexes: Arc::new(Indexes::default()),
        key: "p",
        root: root.clone(),
        list: list(&[]),
    });
    let external: Arc<dyn CodeProvider> = Arc::new(CommandProvider {
        command,
        root: root.clone(),
        timeout: Duration::from_secs(1),
    });
    let chain = [external, native];
    let r = root.to_string_lossy().into_owned();

    let Answer::Symbols(sy) = ask(
        &chain,
        &ProviderRequest::Symbols {
            version: PROTOCOL_VERSION,
            root: r.clone(),
            query: "re".into(),
            limit: 10,
        },
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(sy.provider, "provider.sh");
    assert_eq!(sy.items[0].name, "Remote");
    assert_eq!(sy.warning, None);

    let Answer::Usages(u) = ask(
        &chain,
        &ProviderRequest::Usages {
            version: PROTOCOL_VERSION,
            root: r.clone(),
            symbol: "Thing".into(),
            path: None,
            line: None,
            col: None,
            limit: 50,
        },
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(u.provider, "native");
    assert_eq!(u.warning, None);
    assert_eq!(u.definitions.len(), 1);

    let Answer::Deps(d) = ask(
        &chain,
        &ProviderRequest::Deps {
            version: PROTOCOL_VERSION,
            root: r.clone(),
            path: "crates/app/src/main.rs".into(),
        },
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(d.provider, "native");
    let w = d.warning.unwrap();
    assert!(
        w.contains("provider.sh") && w.contains("no index for this"),
        "{w}"
    );

    let Answer::File(f) = ask(
        &chain,
        &ProviderRequest::File {
            version: PROTOCOL_VERSION,
            root: r,
            path: "crates/core/src/api.rs".into(),
        },
    )
    .await
    .unwrap() else {
        panic!()
    };
    assert_eq!(f.provider, "native");
    assert!(f.warning.unwrap().contains("longer than 1 seconds"));
    assert_eq!(f.symbols[0].name, "Thing");
    assert!(!f.tokens.is_empty());
}

#[test]
fn external_answers_are_checked() {
    let req = ProviderRequest::Usages {
        version: 1,
        root: "/r".into(),
        symbol: "x".into(),
        path: None,
        line: None,
        col: None,
        limit: 10,
    };
    let loc = |path: &str, line: u32| {
        format!(
            r#"{{"definitions":[],"references":[{{"name":"x","path":"{path}","line":{line},"col":0,"len":1,"preview":""}}]}}"#
        )
    };
    assert!(
        parse_answer(&req, loc("src/a.rs", 1).as_bytes())
            .unwrap()
            .is_some()
    );
    assert!(parse_answer(&req, loc("../etc/passwd", 1).as_bytes()).is_err());
    assert!(parse_answer(&req, loc("/abs", 1).as_bytes()).is_err());
    assert!(parse_answer(&req, loc("src/a.rs", 0).as_bytes()).is_err());
    assert!(parse_answer(&req, b"  null\n").unwrap().is_none());
    assert!(parse_answer(&req, b"{\"references\":5}").is_err());

    let file = ProviderRequest::File {
        version: 1,
        root: "/r".into(),
        path: "a.rs".into(),
    };
    assert!(
        parse_answer(&file, br#"{"classes":["keyword"],"tokens":[1,0,2,0]}"#)
            .unwrap()
            .is_some()
    );
    assert!(parse_answer(&file, br#"{"classes":["keyword"],"tokens":[1,0,2]}"#).is_err());
    assert!(parse_answer(&file, br#"{"classes":["keyword"],"tokens":[1,0,2,1]}"#).is_err());
    let Some(Answer::File(f)) = parse_answer(&file, br#"{"path":"other.rs"}"#).unwrap() else {
        panic!()
    };
    assert_eq!(f.path, "a.rs");
}

#[test]
fn request_json_names_the_op() {
    let req = ProviderRequest::Deps {
        version: PROTOCOL_VERSION,
        root: "/r".into(),
        path: "a.rs".into(),
    };
    assert_eq!(
        serde_json::to_string(&req).unwrap(),
        r#"{"op":"deps","version":1,"root":"/r","path":"a.rs"}"#
    );
}
