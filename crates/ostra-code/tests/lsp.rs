//! The LSP provider against an in-process fake server over a pipe, and one live test against
//! rust-analyzer.

use ostra_code::hint::At;
use ostra_code::lsp::rpc::Transport;
use ostra_code::lsp::{Connector, LspPool, LspProvider};
use ostra_code::provider::{Answer, CodeProvider, NativeProvider, ask};
use ostra_code::{CLASSES, Indexes};
use ostra_core::api::FileIndex;
use ostra_core::code::{CompletionKind, NavigateTarget, PROTOCOL_VERSION, ProviderRequest, SymbolKind, TokenClass};
use ostra_core::config::{LanguageServerConfig, SandboxMode};
use parking_lot::Mutex;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const A: &str = "fn helper() {}\nfn main() { helper(); }\n";
const B: &str = "use crate::helper;\nfn x() { helper() }\n";

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), A).unwrap();
    std::fs::write(dir.path().join("b.rs"), B).unwrap();
    std::fs::write(dir.path().join("c.py"), "def helper():\n    pass\n").unwrap();
    dir
}

fn range(l: u32, c: u32, len: u32) -> Value {
    json!({"start": {"line": l, "character": c}, "end": {"line": l, "character": c + len}})
}

/// What the fake server saw, by method.
type Log = Arc<Mutex<Vec<(String, Value)>>>;

async fn fake_server(root: PathBuf, io: tokio::io::DuplexStream, log: Log) {
    let uri = |rel: &str| {
        url::Url::from_file_path(root.join(rel))
            .unwrap()
            .to_string()
    };
    let (r, mut w) = tokio::io::split(io);
    let mut r = BufReader::new(r);
    loop {
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            if line.trim().is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                len = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; len];
        r.read_exact(&mut body).await.unwrap();
        let msg: Value = serde_json::from_slice(&body).unwrap();
        let method = msg["method"].as_str().unwrap_or("").to_string();
        log.lock().push((method.clone(), msg["params"].clone()));
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let result = match method.as_str() {
            "initialize" => json!({"capabilities": {
                "semanticTokensProvider": {"full": true, "legend": {
                    "tokenTypes": ["function", "punctuation"], "tokenModifiers": []}},
                "documentSymbolProvider": true,
                "definitionProvider": true,
                "referencesProvider": true,
                "workspaceSymbolProvider": true,
                "completionProvider": {"triggerCharacters": ["."]},
                "signatureHelpProvider": {"triggerCharacters": ["("], "retriggerCharacters": [","]},
                "implementationProvider": true,
                "typeHierarchyProvider": true,
            }}),
            "textDocument/implementation" => json!([
                {"targetUri": uri("b.rs"), "targetRange": range(1, 0, 19), "targetSelectionRange": range(1, 3, 1)},
            ]),
            "textDocument/prepareTypeHierarchy" => json!([
                {"name": "helper", "kind": 12, "uri": uri("a.rs"), "range": range(0, 0, 14), "selectionRange": range(0, 3, 6)},
            ]),
            "typeHierarchy/supertypes" => json!([
                {"name": "x", "kind": 12, "uri": uri("b.rs"), "range": range(1, 0, 19), "selectionRange": range(1, 3, 1)},
            ]),
            "textDocument/completion" => json!({"isIncomplete": false, "items": [
                {"label": "helper", "kind": 3, "detail": "fn()",
                 "textEdit": {"range": range(1, 12, 3), "newText": "helper()"}},
            ]}),
            "textDocument/signatureHelp" => json!({"signatures": [
                {"label": "helper(a: u32)", "parameters": [{"label": "a: u32"}]},
            ], "activeParameter": 0}),
            // `helper` on line 1 as a function; `(` as punctuation, which has no class.
            "textDocument/semanticTokens/full" => json!({"data": [0, 3, 6, 0, 0, 0, 6, 1, 1, 0]}),
            "textDocument/documentSymbol" => json!([
                {"name": "helper", "kind": 12, "range": range(0, 0, 14), "selectionRange": range(0, 3, 6)},
                {"name": "main", "kind": 12, "range": range(1, 0, 23), "selectionRange": range(1, 3, 4)},
            ]),
            // From b.rs, `helper` is defined in a library source outside the project and in a class
            // inside a jar; from anywhere else, in a.rs.
            "textDocument/definition" if msg["params"]["textDocument"]["uri"] == uri("b.rs") => {
                json!([
                    {"uri": url::Url::from_file_path(dep_dir(&root).join("lib.rs")).unwrap().to_string(), "range": range(0, 7, 6)},
                    {"uri": JAR_CLASS, "range": range(2, 7, 6)},
                ])
            }
            "textDocument/definition" => json!([{"uri": uri("a.rs"), "range": range(0, 3, 6)}]),
            "java/classFileContents" if msg["params"]["uri"] == JAR_CLASS => json!(CLASS_TEXT),
            "textDocument/references" => json!([
                {"uri": uri("b.rs"), "range": range(1, 9, 6)},
                {"uri": uri("a.rs"), "range": range(1, 12, 6)},
                {"uri": "file:///elsewhere/std.rs", "range": range(0, 0, 6)},
            ]),
            "workspace/symbol" => json!([
                {"name": "helper", "kind": 12, "location": {"uri": uri("a.rs"), "range": range(0, 0, 14)}},
                {"name": "helperish", "kind": 12, "location": {"uri": uri("b.rs"), "range": range(0, 0, 1)}},
            ]),
            "shutdown" => Value::Null,
            _ => Value::Null,
        };
        let reply =
            serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
        let head = format!("Content-Length: {}\r\n\r\n", reply.len());
        if w.write_all(head.as_bytes()).await.is_err() || w.write_all(&reply).await.is_err() {
            return;
        }
    }
}

const JAR_CLASS: &str = "jdt://contents/lib.jar/com.acme/Thing.class?=p";
const CLASS_TEXT: &str = "package com.acme;\npublic class Thing {\n  void helper() {}\n}\n";

/// A folder beside the project, standing in for a package cache.
fn dep_dir(root: &Path) -> PathBuf {
    root.with_extension("dep")
}

struct Rig {
    dir: tempfile::TempDir,
    pool: LspPool<String>,
    log: Log,
    starts: Arc<AtomicUsize>,
}

fn rig() -> Rig {
    let dir = project();
    let log: Log = Arc::default();
    let starts = Arc::new(AtomicUsize::new(0));
    let (l, s) = (log.clone(), starts.clone());
    let connect: Connector = Arc::new(move |_cmd: &[String], root: &Path, _sandbox|  {
        s.fetch_add(1, Ordering::SeqCst);
        let (ours, theirs) = tokio::io::duplex(1 << 20);
        tokio::spawn(fake_server(root.to_path_buf(), theirs, l.clone()));
        let (r, w) = tokio::io::split(ours);
        Ok(Transport {
            read: Box::new(r),
            write: Box::new(w),
            stderr: None,
            child: None,
            sandbox: None,
        })
    });
    Rig {
        dir,
        pool: LspPool::with_connector(connect),
        log,
        starts,
    }
}

impl Rig {
    fn chain(&self) -> Vec<Arc<dyn CodeProvider>> {
        let root = self.dir.path().to_path_buf();
        let native: Arc<dyn CodeProvider> = Arc::new(NativeProvider {
            indexes: Arc::new(Indexes::default()),
            key: "p".to_string(),
            root: root.clone(),
            list: Arc::new(FileIndex {
                paths: vec!["a.rs".into(), "b.rs".into(), "c.py".into()],
                truncated: false,
            }),
        });
        vec![
            Arc::new(LspProvider {
                pool: self.pool.clone(),
                key: "p".to_string(),
                root,
                config: LanguageServerConfig {
                    command: vec!["/usr/bin/fake-ls".into()],
                    languages: vec!["rust".into()],
                    timeout_secs: 5,
                    initialization_options: None,
                },
                base: native.clone(),
                sandbox: None,
            }),
            native,
        ]
    }

    fn lsp(&self) -> LspProvider<String> {
        let chain = self.chain();
        LspProvider {
            pool: self.pool.clone(),
            key: "p".to_string(),
            root: self.dir.path().to_path_buf(),
            config: LanguageServerConfig {
                command: vec!["/usr/bin/fake-ls".into()],
                languages: vec!["rust".into()],
                timeout_secs: 5,
                initialization_options: None,
            },
            base: chain[1].clone(),
            sandbox: None,
        }
    }

    fn params(&self, method: &str) -> Vec<Value> {
        self.log
            .lock()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }

    fn methods(&self) -> Vec<String> {
        self.log.lock().iter().map(|(m, _)| m.clone()).collect()
    }
}

fn root(r: &Rig) -> String {
    r.dir.path().to_string_lossy().into_owned()
}

fn file(r: &Rig, path: &str) -> ProviderRequest {
    ProviderRequest::File {
        version: PROTOCOL_VERSION,
        root: root(r),
        path: path.into(),
    }
}

fn usages(r: &Rig, at: Option<(u32, u32)>) -> ProviderRequest {
    ProviderRequest::Usages {
        version: PROTOCOL_VERSION,
        root: root(r),
        symbol: "helper".into(),
        path: Some("a.rs".into()),
        line: at.map(|a| a.0),
        col: at.map(|a| a.1),
        limit: 100,
    }
}

const EDITED: &str = "fn helper() {}\nfn main() { hel }\n";

fn at<'a>(text: &'a str, path: &'a str, trigger: Option<&'a str>, retrigger: bool) -> At<'a> {
    At {
        path,
        text,
        line: 2,
        col: 15,
        trigger,
        retrigger,
    }
}

#[tokio::test]
async fn completion_sends_the_unsaved_text_and_reads_the_list() {
    let r = rig();
    let wait = Duration::from_secs(5);
    let c = r
        .lsp()
        .complete(&at(EDITED, "a.rs", None, false), wait)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(c.provider, "fake-ls");
    assert_eq!(c.items.len(), 1);
    assert_eq!(c.items[0].insert, "helper()");
    assert_eq!(c.items[0].kind, Some(CompletionKind::Function));
    assert_eq!(c.items[0].range.map(|x| (x.line, x.col)), Some((2, 12)));
    assert_eq!(
        r.params("textDocument/didOpen")[0]["textDocument"]["text"],
        EDITED
    );
    let asked = &r.params("textDocument/completion")[0];
    assert_eq!(asked["position"], json!({"line": 1, "character": 15}));
    assert_eq!(asked["context"], json!({"triggerKind": 1}));

    // A character the server does not list gets an empty answer without a request.
    let none = r
        .lsp()
        .complete(&at(EDITED, "a.rs", Some(":"), false), wait)
        .await
        .unwrap()
        .unwrap();
    assert!(none.items.is_empty());
    r.lsp()
        .complete(&at(EDITED, "a.rs", Some("."), false), wait)
        .await
        .unwrap();
    let asked = r.params("textDocument/completion");
    assert_eq!(asked.len(), 2);
    assert_eq!(
        asked[1]["context"],
        json!({"triggerKind": 2, "triggerCharacter": "."})
    );
    // The same text is not sent twice.
    assert!(r.params("textDocument/didChange").is_empty());

    // Other languages are left to the next provider.
    let py = r
        .lsp()
        .complete(&at("x", "c.py", None, false), wait)
        .await
        .unwrap();
    assert!(py.is_none());
}

#[tokio::test]
async fn signature_help_maps_trigger_and_retrigger_characters() {
    let r = rig();
    let wait = Duration::from_secs(5);
    let lsp = r.lsp();
    let h = lsp
        .signature(&at(EDITED, "a.rs", Some("("), false), wait)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(h.signatures[0].label, "helper(a: u32)");
    assert_eq!(
        (
            h.signatures[0].parameters[0].start,
            h.signatures[0].parameters[0].end
        ),
        (7, 13)
    );
    // `,` only retriggers, and a content change while showing asks with kind 3.
    assert!(
        lsp.signature(&at(EDITED, "a.rs", Some(","), false), wait)
            .await
            .unwrap()
            .is_none()
    );
    lsp.signature(&at(EDITED, "a.rs", Some(","), true), wait)
        .await
        .unwrap();
    lsp.signature(&at(EDITED, "a.rs", None, true), wait)
        .await
        .unwrap();
    let kinds: Vec<Value> = r
        .params("textDocument/signatureHelp")
        .iter()
        .map(|p| p["context"]["triggerKind"].clone())
        .collect();
    assert_eq!(kinds, vec![json!(2), json!(2), json!(3)]);
}

#[tokio::test]
async fn file_overlays_server_tokens_and_outline() {
    let r = rig();
    let Answer::File(f) = ask(&r.chain(), &file(&r, "a.rs")).await.unwrap() else {
        panic!()
    };
    assert_eq!(
        (f.provider.as_str(), f.warning.as_deref()),
        ("fake-ls", None)
    );
    let function = f
        .classes
        .iter()
        .position(|c| *c == TokenClass::Function)
        .unwrap() as u32;
    assert!(f.tokens.chunks(4).any(|t| t == [1, 3, 6, function]));
    assert!(
        f.tokens
            .chunks(4)
            .any(|t| t[0] == 1 && t[1] == 0 && f.classes[t[3] as usize] == TokenClass::Keyword),
        "the built-in `fn` keyword stays"
    );
    assert_eq!(f.classes[..CLASSES.len()], CLASSES);
    let outline: Vec<_> = f
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.line, s.col, s.end_line))
        .collect();
    assert_eq!(
        outline,
        vec![("helper", 1, 3, Some(1)), ("main", 2, 3, Some(2))]
    );
    assert_eq!(f.imports.len(), 0);
    let didopen = r
        .log
        .lock()
        .iter()
        .find(|(m, _)| m == "textDocument/didOpen")
        .cloned()
        .unwrap();
    assert_eq!(didopen.1["textDocument"]["languageId"], "rust");
    assert_eq!(didopen.1["textDocument"]["text"], A);
}

#[tokio::test]
async fn usages_at_a_position_use_definition_and_references() {
    let r = rig();
    let Answer::Usages(u) = ask(&r.chain(), &usages(&r, Some((2, 12)))).await.unwrap() else {
        panic!()
    };
    assert_eq!(u.provider, "fake-ls");
    let defs: Vec<_> = u
        .definitions
        .iter()
        .map(|l| (l.path.as_str(), l.line, l.col, l.len))
        .collect();
    assert_eq!(defs, vec![("a.rs", 1, 3, 6)]);
    assert_eq!(u.definitions[0].preview, "fn helper() {}");
    // The asked file comes first, and locations outside the project are dropped.
    let refs: Vec<_> = u
        .references
        .iter()
        .map(|l| (l.path.as_str(), l.line, l.col))
        .collect();
    assert_eq!(refs, vec![("a.rs", 2, 12), ("b.rs", 2, 9)]);
    let refq = r
        .log
        .lock()
        .iter()
        .find(|(m, _)| m == "textDocument/references")
        .cloned()
        .unwrap();
    assert_eq!(refq.1["position"], json!({"line": 1, "character": 12}));
    assert_eq!(refq.1["context"]["includeDeclaration"], false);
}

#[tokio::test]
async fn usages_without_a_position_start_from_workspace_symbols() {
    let r = rig();
    let Answer::Usages(u) = ask(&r.chain(), &usages(&r, None)).await.unwrap() else {
        panic!()
    };
    let defs: Vec<_> = u
        .definitions
        .iter()
        .map(|l| (l.path.as_str(), l.line, l.col, l.kind))
        .collect();
    assert_eq!(defs, vec![("a.rs", 1, 3, Some(SymbolKind::Function))]);
    let refq = r
        .log
        .lock()
        .iter()
        .find(|(m, _)| m == "textDocument/references")
        .cloned()
        .unwrap();
    assert_eq!(
        refq.1["position"],
        json!({"line": 0, "character": 3}),
        "the name, not the item start"
    );
}

#[tokio::test]
async fn symbols_come_from_the_server() {
    let r = rig();
    let req = ProviderRequest::Symbols {
        version: PROTOCOL_VERSION,
        root: root(&r),
        query: "help".into(),
        limit: 1,
    };
    let Answer::Symbols(s) = ask(&r.chain(), &req).await.unwrap() else {
        panic!()
    };
    assert_eq!(
        (s.provider.as_str(), s.items.len(), s.truncated),
        ("fake-ls", 1, true)
    );
    assert_eq!(s.items[0].name, "helper");
}

#[tokio::test]
async fn other_languages_and_deps_go_to_the_built_in_provider_without_a_server() {
    let r = rig();
    let Answer::File(f) = ask(&r.chain(), &file(&r, "c.py")).await.unwrap() else {
        panic!()
    };
    assert_eq!((f.provider.as_str(), f.warning), ("native", None));
    let deps = ProviderRequest::Deps {
        version: PROTOCOL_VERSION,
        root: root(&r),
        path: "b.rs".into(),
    };
    let Answer::Deps(d) = ask(&r.chain(), &deps).await.unwrap() else {
        panic!()
    };
    assert_eq!(d.provider, "native");
    assert_eq!(r.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_changed_sandbox_mode_starts_the_server_again() {
    let r = rig();
    let cfg = LanguageServerConfig {
        command: vec!["/usr/bin/fake-ls".into()],
        languages: vec!["rust".into()],
        timeout_secs: 5,
        initialization_options: None,
    };
    let key = "p".to_string();
    let get = |mode| r.pool.get(&key, r.dir.path(), &cfg, mode, Duration::from_secs(5));
    get(None).await.unwrap();
    get(None).await.unwrap();
    assert_eq!(r.starts.load(Ordering::SeqCst), 1);
    get(Some(SandboxMode::Off)).await.unwrap();
    assert_eq!(r.starts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn one_server_serves_every_request_and_hears_changes() {
    let r = rig();
    let chain = r.chain();
    ask(&chain, &file(&r, "a.rs")).await.unwrap();
    ask(&chain, &file(&r, "a.rs")).await.unwrap();
    std::fs::write(r.dir.path().join("a.rs"), "fn helper() {}\n").unwrap();
    ask(&chain, &file(&r, "a.rs")).await.unwrap();
    r.pool.touch(&"p".to_string(), "b.rs");
    ask(&chain, &file(&r, "a.rs")).await.unwrap();
    assert_eq!(r.starts.load(Ordering::SeqCst), 1);
    let m = r.methods();
    let count = |x: &str| m.iter().filter(|y| *y == x).count();
    assert_eq!(
        (
            count("initialize"),
            count("initialized"),
            count("textDocument/didOpen"),
            count("textDocument/didChange")
        ),
        (1, 1, 1, 1)
    );
    let watched = r
        .log
        .lock()
        .iter()
        .find(|(m, _)| m == "workspace/didChangeWatchedFiles")
        .cloned()
        .unwrap();
    assert_eq!(watched.1["changes"][0]["type"], 2);
    assert_eq!(r.pool.running(), vec!["fake-ls".to_string()]);
    r.pool.forget(&"p".to_string());
    assert!(r.pool.running().is_empty());
}

#[tokio::test]
async fn a_server_that_cannot_start_falls_through_with_the_reason() {
    let dir = project();
    let pool: LspPool<String> = LspPool::default();
    let root = dir.path().to_path_buf();
    let native: Arc<dyn CodeProvider> = Arc::new(NativeProvider {
        indexes: Arc::new(Indexes::default()),
        key: "p".to_string(),
        root: root.clone(),
        list: Arc::new(FileIndex {
            paths: vec!["a.rs".into()],
            truncated: false,
        }),
    });
    let chain: Vec<Arc<dyn CodeProvider>> = vec![
        Arc::new(LspProvider {
            pool,
            key: "p".to_string(),
            root: root.clone(),
            config: LanguageServerConfig {
                command: vec!["/nonexistent/ostra-no-such-server".into()],
                languages: vec!["rust".into()],
                timeout_secs: 5,
                initialization_options: None,
            },
            base: native.clone(),
            sandbox: None,
        }),
        native,
    ];
    let req = ProviderRequest::File {
        version: PROTOCOL_VERSION,
        root: root.to_string_lossy().into_owned(),
        path: "a.rs".into(),
    };
    let Answer::File(f) = ask(&chain, &req).await.unwrap() else {
        panic!()
    };
    assert_eq!(f.provider, "native");
    let w = f.warning.unwrap();
    assert!(
        w.contains("`ostra-no-such-server` failed") && w.contains("Cannot start"),
        "{w}"
    );
}

/// `cargo test -p ostra-code --test lsp -- --ignored`; needs rust-analyzer on PATH.
#[tokio::test]
#[ignore]
async fn live_rust_analyzer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub mod util;\npub fn run() { util::helper(); }\n",
    )
    .unwrap();
    std::fs::write(root.join("src/util.rs"), "pub fn helper() {}\n").unwrap();
    live(
        root,
        "rust-analyzer",
        "rust",
        None,
        ("src/lib.rs", 2, 21),
        "src/util.rs",
    )
    .await;
}

/// Needs gopls and go on PATH.
#[tokio::test]
#[ignore]
async fn live_gopls() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("go.mod"), "module example.com/demo\n\ngo 1.21\n").unwrap();
    std::fs::write(
        root.join("main.go"),
        "package main\n\nfunc main() { helper() }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("util.go"),
        "package main\n\nconst limit = 3\n\nfunc helper() int { return limit }\n",
    )
    .unwrap();
    let options = json!({"semanticTokens": true});
    let f = live(
        root,
        "gopls",
        "go",
        Some(options),
        ("main.go", 3, 14),
        "util.go",
    )
    .await;
    // The tokenizer sees `limit` in `return limit` as a name; gopls knows it is a constant.
    let constant = f
        .classes
        .iter()
        .position(|c| *c == TokenClass::Constant)
        .unwrap() as u32;
    assert!(
        f.tokens.chunks(4).any(|t| t == [5, 27, 5, constant]),
        "{:?}",
        f.tokens
    );
}

/// Ask `server` for the usages of `helper` at `at` until it has loaded the project, then check
/// the definition is in `def_file` and the outline of `def_file` names it.
async fn live(
    root: &Path,
    server: &str,
    language: &str,
    options: Option<Value>,
    at: (&str, u32, u32),
    def_file: &str,
) -> ostra_core::code::CodeFile {
    let native: Arc<dyn CodeProvider> = Arc::new(NativeProvider {
        indexes: Arc::new(Indexes::default()),
        key: 0u8,
        root: root.to_path_buf(),
        list: Arc::new(FileIndex {
            paths: vec![at.0.into(), def_file.into()],
            truncated: false,
        }),
    });
    let lsp = LspProvider {
        pool: LspPool::default(),
        key: 0u8,
        root: root.to_path_buf(),
        config: LanguageServerConfig {
            command: vec![server.into()],
            languages: vec![language.into()],
            timeout_secs: 60,
            initialization_options: options,
        },
        base: native,
        sandbox: None,
    };
    let roots = root.to_string_lossy().into_owned();
    let req = ProviderRequest::Usages {
        version: PROTOCOL_VERSION,
        root: roots.clone(),
        symbol: "helper".into(),
        path: Some(at.0.into()),
        line: Some(at.1),
        col: Some(at.2),
        limit: 50,
    };
    // A server answers with nothing until it has loaded the project.
    let mut found = None;
    for _ in 0..60 {
        if let Ok(Some(Answer::Usages(u))) = lsp.answer(&req).await
            && !u.definitions.is_empty()
        {
            found = Some(u);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    let u = found.expect("the server found the definition");
    let def_line = std::fs::read_to_string(root.join(def_file))
        .unwrap()
        .lines()
        .position(|l| l.contains("helper"))
        .unwrap() as u32
        + 1;
    assert_eq!(
        (u.definitions[0].path.as_str(), u.definitions[0].line),
        (def_file, def_line)
    );
    assert_eq!(u.references.len(), 1, "{u:?}");
    let file = ProviderRequest::File {
        version: PROTOCOL_VERSION,
        root: roots,
        path: def_file.into(),
    };
    let Some(Answer::File(f)) = lsp.answer(&file).await.unwrap() else {
        panic!()
    };
    assert!(f.symbols.iter().any(|s| s.name == "helper"), "{f:?}");
    lsp.pool.forget(&0u8);
    f
}

#[tokio::test]
async fn navigation_asks_implementations_and_the_type_hierarchy() {
    let r = rig();
    let wait = Duration::from_secs(5);
    let lsp = r.lsp();
    let cursor = At {
        path: "a.rs",
        text: A,
        line: 1,
        col: 4,
        trigger: None,
        retrigger: false,
    };
    let down = lsp
        .navigate(&cursor, NavigateTarget::Implementations, wait)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(down.provider, "fake-ls");
    assert_eq!(down.symbol, "helper");
    let got: Vec<(&str, u32, u32, &str)> = down
        .locations
        .iter()
        .map(|l| (l.path.as_str(), l.line, l.col, l.name.as_str()))
        .collect();
    assert_eq!(got, [("b.rs", 2, 3, "x")]);
    assert_eq!(
        r.params("textDocument/implementation")[0]["position"],
        json!({"line": 0, "character": 4})
    );
    let up = lsp
        .navigate(&cursor, NavigateTarget::Supertypes, wait)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((up.locations[0].path.as_str(), up.locations[0].line), ("b.rs", 2));
    // The supertypes request carries the item the prepare step returned.
    assert_eq!(
        r.params("typeHierarchy/supertypes")[0]["item"]["name"],
        "helper"
    );
    let py = At { path: "c.py", ..cursor };
    assert!(
        lsp.navigate(&py, NavigateTarget::Supertypes, wait)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn definitions_outside_the_project_are_kept_and_read_back() {
    let r = rig();
    let dep = dep_dir(r.dir.path());
    std::fs::create_dir_all(&dep).unwrap();
    std::fs::write(dep.join("lib.rs"), "pub fn helper() {}\n").unwrap();
    let req = ProviderRequest::Usages {
        version: PROTOCOL_VERSION,
        root: root(&r),
        symbol: "helper".into(),
        path: Some("b.rs".into()),
        line: Some(2),
        col: Some(9),
        limit: 100,
    };
    let Answer::Usages(u) = ask(&r.chain(), &req).await.unwrap() else {
        panic!()
    };
    let lib_uri = url::Url::from_file_path(dep.join("lib.rs"))
        .unwrap()
        .to_string();
    let defs: Vec<_> = u
        .definitions
        .iter()
        .map(|l| {
            (
                l.uri.as_deref(),
                l.path.as_str(),
                l.line,
                l.preview.as_str(),
            )
        })
        .collect();
    let lib_path = dep.join("lib.rs").to_string_lossy().into_owned();
    assert_eq!(
        defs,
        vec![
            (
                Some(lib_uri.as_str()),
                lib_path.as_str(),
                1,
                "pub fn helper() {}"
            ),
            (Some(JAR_CLASS), "lib.jar › com/acme/Thing.class", 3, ""),
        ]
    );
    let lsp = r.lsp();
    assert_eq!(
        r.pool.external_server(&"p".to_string(), JAR_CLASS),
        Some(vec!["/usr/bin/fake-ls".to_string()])
    );
    assert_eq!(
        r.pool
            .external_server(&"p".to_string(), "file:///etc/passwd"),
        None
    );

    let class = lsp.external_file(JAR_CLASS).await.unwrap();
    assert_eq!(
        (class.name.as_str(), class.language.as_deref()),
        ("Thing.class", Some("java"))
    );
    assert_eq!(class.content, CLASS_TEXT);
    let lib = lsp.external_file(&lib_uri).await.unwrap();
    assert_eq!(
        (lib.content.as_str(), lib.language.as_deref()),
        ("pub fn helper() {}\n", Some("rust"))
    );

    // A name in the class resolves through the server, which gets the class text opened first.
    let inner = lsp
        .external_usages(JAR_CLASS, "helper", 3, Some(9), 100)
        .await
        .unwrap();
    let at: Vec<_> = inner
        .definitions
        .iter()
        .map(|l| (l.uri.as_deref(), l.path.as_str(), l.line))
        .collect();
    assert_eq!(at, vec![(None, "a.rs", 1)]);
    let opened = r.params("textDocument/didOpen");
    let doc = opened
        .iter()
        .find(|p| p["textDocument"]["uri"] == JAR_CLASS)
        .unwrap();
    assert_eq!(doc["textDocument"]["languageId"], "java");
    assert_eq!(doc["textDocument"]["text"], CLASS_TEXT);
    let def = r.params("textDocument/definition").pop().unwrap();
    assert_eq!(def["position"], json!({"line": 2, "character": 7}));
    let _ = std::fs::remove_dir_all(dep);
}

#[tokio::test]
#[ignore = "needs gopls on PATH"]
async fn live_gopls_into_the_standard_library() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("go.mod"), "module example.com/demo\n\ngo 1.21\n").unwrap();
    let main = "package main\n\nimport \"strings\"\n\nfunc main() { _ = strings.ToUpper(\"x\") }\n";
    std::fs::write(root.join("main.go"), main).unwrap();
    let lsp = LspProvider {
        pool: LspPool::default(),
        key: 0u8,
        root: root.to_path_buf(),
        config: LanguageServerConfig {
            command: vec!["gopls".into()],
            languages: vec!["go".into()],
            timeout_secs: 60,
            initialization_options: None,
        },
        base: Arc::new(ostra_code::provider::Unanswered),
        sandbox: None,
    };
    let req = ProviderRequest::Usages {
        version: PROTOCOL_VERSION,
        root: root.to_string_lossy().into_owned(),
        symbol: "ToUpper".into(),
        path: Some("main.go".into()),
        line: Some(5),
        col: Some(27),
        limit: 50,
    };
    let mut found = None;
    for _ in 0..60 {
        if let Ok(Some(Answer::Usages(u))) = lsp.answer(&req).await
            && !u.definitions.is_empty()
        {
            found = Some(u);
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let u = found.expect("gopls found the definition");
    let def = &u.definitions[0];
    let uri = def.uri.clone().expect("the definition is outside the project");
    assert!(def.path.ends_with("strings/strings.go"), "{def:?}");
    let file = lsp.external_file(&uri).await.unwrap();
    assert_eq!(file.language.as_deref(), Some("go"));
    let line = file.content.lines().nth(def.line as usize - 1).unwrap();
    assert!(line.contains("func ToUpper"), "{line}");
    // From the library's own definition, the project's call is a reference.
    let back = lsp
        .external_usages(&uri, "ToUpper", def.line, Some(def.col), 50)
        .await
        .unwrap();
    let refs: Vec<_> = back.references.iter().map(|r| (r.path.as_str(), r.line)).collect();
    assert_eq!(refs, vec![("main.go", 5)], "{back:?}");
    lsp.pool.forget(&0u8);
}
