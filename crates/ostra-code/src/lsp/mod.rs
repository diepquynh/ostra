//! Language Server Protocol providers. A project's `language_servers` entries each name a server
//! and the languages it answers for. The pool starts a server on the first request for one of
//! them, keeps it running while it is used, and stops it after `IDLE` without requests. File
//! answers overlay the server's semantic tokens and outline on the built-in answer, so imports and
//! the colors the server leaves out still come from the tokenizer.

pub mod convert;
pub mod external;
pub mod rpc;

use crate::hint::{self, At};
use crate::provider::{Answer, CodeProvider};
use crate::{MAX_FILE_BYTES, lang, preview};
use async_trait::async_trait;
use convert::{Legend, Range};
use ostra_core::code::{
    CodeCompletion, CodeExternalFile, CodeLocation, CodeNavigation, CodeSignatureHelp, CodeSymbols,
    CodeUsages, NavigateTarget, ProviderRequest,
};
use ostra_core::config::LanguageServerConfig;
use parking_lot::Mutex;
use rpc::{Client, Transport};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::OnceCell;
use url::Url;

/// A server with no requests for this long is stopped.
pub const IDLE: Duration = Duration::from_secs(10 * 60);
/// Running servers across all projects. Starting one more stops the least recently used.
pub const MAX_SERVERS: usize = 8;
/// Documents one server keeps open. Opening one more closes the least recently used.
const MAX_OPEN: usize = 32;
/// A server that failed to start is not started again for this long.
const RETRY: Duration = Duration::from_secs(30);
/// `initialize` may take longer than a request, because some servers read the project first.
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_DEFINITIONS: usize = 50;
const SWEEP: Duration = Duration::from_secs(60);
/// Outside URIs the pool remembers, across projects. Remembering one more forgets the oldest.
const MAX_EXTERNALS: usize = 20_000;

/// Opens a connection to a server: `command` run in `root`.
pub type Connector = Arc<dyn Fn(&[String], &Path) -> std::io::Result<Transport> + Send + Sync>;

pub fn spawn_process(command: &[String], root: &Path) -> std::io::Result<Transport> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| std::io::Error::other("The command is empty."))?;
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    Ok(Transport {
        read: Box::new(child.stdout.take().expect("piped")),
        write: Box::new(child.stdin.take().expect("piped")),
        stderr: Some(Box::new(child.stderr.take().expect("piped"))),
        child: Some(child),
    })
}

struct Doc {
    version: i32,
    hash: u64,
    used: u64,
}

pub struct Server {
    pub name: String,
    client: Arc<Client>,
    root: PathBuf,
    /// The canonical root, because servers report canonical paths.
    real_root: PathBuf,
    caps: Value,
    legend: Option<Legend>,
    docs: tokio::sync::Mutex<(HashMap<String, Doc>, u64)>,
}

impl Server {
    fn uri(&self, rel: &str) -> String {
        Url::from_file_path(self.root.join(rel))
            .map(String::from)
            .unwrap_or_default()
    }

    /// The project-relative path of `uri`, or None outside the project.
    fn rel(&self, uri: &str) -> Option<String> {
        let p = Url::parse(uri).ok()?.to_file_path().ok()?;
        let r = p
            .strip_prefix(&self.root)
            .or_else(|_| p.strip_prefix(&self.real_root))
            .ok()?;
        let s = r.to_str()?.replace('\\', "/");
        (!s.is_empty() && !s.split('/').any(|c| c == "..")).then_some(s)
    }

    fn has(&self, cap: &str) -> bool {
        self.caps
            .get(cap)
            .is_some_and(|v| !v.is_null() && v != &Value::Bool(false))
    }

    /// Open `rel` with `text`, or send the new text when it changed since the last request.
    async fn sync(&self, rel: &str, lang: &str, text: &str) {
        self.sync_doc(&self.uri(rel), &convert::language_id(lang, rel), text)
            .await;
    }

    async fn sync_doc(&self, uri: &str, language_id: &str, text: &str) {
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        let hash = h.finish();
        let mut guard = self.docs.lock().await;
        let (docs, clock) = &mut *guard;
        *clock += 1;
        if let Some(d) = docs.get_mut(uri) {
            d.used = *clock;
            if d.hash != hash {
                d.hash = hash;
                d.version += 1;
                self.client.notify(
                    "textDocument/didChange",
                    json!({"textDocument": {"uri": uri, "version": d.version},
                           "contentChanges": [{"text": text}]}),
                );
            }
            return;
        }
        if docs.len() >= MAX_OPEN
            && let Some(old) = docs
                .iter()
                .min_by_key(|(_, d)| d.used)
                .map(|(k, _)| k.clone())
        {
            docs.remove(&old);
            self.client.notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": old}}),
            );
        }
        self.client.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": language_id,
                   "version": 1, "text": text}}),
        );
        docs.insert(
            uri.to_string(),
            Doc {
                version: 1,
                hash,
                used: *clock,
            },
        );
    }
}

async fn start(
    connect: Connector,
    command: Vec<String>,
    root: PathBuf,
    options: Option<Value>,
) -> Result<Arc<Server>, String> {
    let name = program_name(&command);
    let t = connect(&command, &root).map_err(|e| format!("Cannot start `{name}`: {e}."))?;
    let root_uri = Url::from_directory_path(&root)
        .map(String::from)
        .map_err(|_| format!("{} is not an absolute folder.", root.display()))?;
    let folder_name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let folders = json!([{"uri": root_uri, "name": folder_name}]);
    let client = Client::start(&name, t, folders.clone());
    let kinds: Vec<u32> = (1..=26).collect();
    let params = json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "ostra", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": root_uri,
        "rootPath": root.to_string_lossy(),
        "workspaceFolders": folders,
        "initializationOptions": options,
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "window": {"workDoneProgress": true},
            "workspace": {
                "workspaceFolders": true,
                "symbol": {"symbolKind": {"valueSet": kinds}},
                "didChangeWatchedFiles": {"dynamicRegistration": false},
            },
            "textDocument": {
                "synchronization": {"dynamicRegistration": false},
                "definition": {"linkSupport": true},
                "implementation": {"linkSupport": true},
                "typeHierarchy": {},
                "references": {},
                "completion": {
                    "completionItem": {
                        "snippetSupport": true,
                        "commitCharactersSupport": true,
                        "documentationFormat": ["markdown", "plaintext"],
                        "deprecatedSupport": true,
                        "preselectSupport": true,
                        "tagSupport": {"valueSet": [1]},
                        "insertReplaceSupport": true,
                    },
                    "completionItemKind": {"valueSet": (1..=25).collect::<Vec<u32>>()},
                    "contextSupport": true,
                },
                "signatureHelp": {
                    "signatureInformation": {
                        "documentationFormat": ["markdown", "plaintext"],
                        "parameterInformation": {"labelOffsetSupport": true},
                        "activeParameterSupport": true,
                    },
                    "contextSupport": true,
                },
                "documentSymbol": {
                    "hierarchicalDocumentSymbolSupport": true,
                    "symbolKind": {"valueSet": kinds},
                },
                "semanticTokens": {
                    "requests": {"full": true},
                    "tokenTypes": [
                        "namespace", "type", "class", "enum", "interface", "struct",
                        "typeParameter", "parameter", "variable", "property", "enumMember",
                        "event", "function", "method", "macro", "keyword", "modifier", "comment",
                        "string", "number", "regexp", "operator", "decorator",
                    ],
                    "tokenModifiers": [
                        "declaration", "definition", "readonly", "static", "deprecated",
                        "abstract", "async", "modification", "documentation", "defaultLibrary",
                    ],
                    "formats": ["relative"],
                    "multilineTokenSupport": false,
                    "overlappingTokenSupport": false,
                    "augmentsSyntaxTokens": true,
                },
            },
            "experimental": {"serverStatusNotification": true},
        },
    });
    let init = client.request("initialize", params, INIT_TIMEOUT).await;
    let init = match init {
        Ok(v) => v,
        Err(e) => {
            client.close().await;
            return Err(e);
        }
    };
    client.notify("initialized", json!({}));
    let caps = init.get("capabilities").cloned().unwrap_or(Value::Null);
    let encoding = caps["positionEncoding"].as_str().unwrap_or("utf-16");
    if encoding != "utf-16" {
        client.close().await;
        return Err(format!(
            "`{name}` counts columns in {encoding}. Ostra needs a server that counts them in utf-16."
        ));
    }
    let real_root = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    tracing::info!(server = %name, root = %root.display(), "language server started");
    Ok(Arc::new(Server {
        name,
        client,
        legend: Legend::from_caps(&caps),
        caps,
        root,
        real_root,
        docs: tokio::sync::Mutex::new((HashMap::new(), 0)),
    }))
}

fn program_name(command: &[String]) -> String {
    command
        .first()
        .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
        .unwrap_or_default()
}

type Started = Result<Arc<Server>, String>;

struct Entry {
    cell: Arc<OnceCell<Started>>,
    begun: Instant,
    used: Mutex<Instant>,
}

impl Entry {
    fn running(&self) -> Option<Arc<Server>> {
        match self.cell.get() {
            Some(Ok(s)) => Some(s.clone()),
            _ => None,
        }
    }

    /// A new request should start the server again.
    fn stale(&self) -> bool {
        match self.cell.get() {
            None => false,
            Some(Ok(s)) => s.client.dead().is_some() && self.begun.elapsed() >= RETRY,
            Some(Err(_)) => self.begun.elapsed() >= RETRY,
        }
    }
}

/// A project and a server command.
type ServerId<K> = (K, Vec<String>);

/// Outside URIs servers answered with, and the command of the server that did, so only those
/// are read back.
struct Externals<K> {
    by: HashMap<(K, String), Vec<String>>,
    order: VecDeque<(K, String)>,
}

struct PoolInner<K> {
    entries: Mutex<HashMap<ServerId<K>, Arc<Entry>>>,
    connect: Connector,
    sweeping: Mutex<bool>,
    externals: Mutex<Externals<K>>,
}

/// Running language servers, keyed by project and command.
pub struct LspPool<K> {
    inner: Arc<PoolInner<K>>,
}

impl<K> Clone for LspPool<K> {
    fn clone(&self) -> Self {
        LspPool {
            inner: self.inner.clone(),
        }
    }
}

impl<K: Hash + Eq + Clone + Send + Sync + 'static> Default for LspPool<K> {
    fn default() -> Self {
        Self::with_connector(Arc::new(spawn_process))
    }
}

impl<K: Hash + Eq + Clone + Send + Sync + 'static> LspPool<K> {
    pub fn with_connector(connect: Connector) -> Self {
        LspPool {
            inner: Arc::new(PoolInner {
                entries: Mutex::new(HashMap::new()),
                connect,
                sweeping: Mutex::new(false),
                externals: Mutex::new(Externals {
                    by: HashMap::new(),
                    order: VecDeque::new(),
                }),
            }),
        }
    }

    /// The running server for `key` and `cfg`, started when needed, waiting at most `wait`.
    pub async fn get(
        &self,
        key: &K,
        root: &Path,
        cfg: &LanguageServerConfig,
        wait: Duration,
    ) -> Result<Arc<Server>, String> {
        self.sweep_later();
        let id = (key.clone(), cfg.command.clone());
        let (entry, evicted) = {
            let mut map = self.inner.entries.lock();
            let mut evicted = vec![];
            if map.get(&id).is_none_or(|e| e.stale()) {
                if let Some(old) = map.remove(&id) {
                    evicted.push(old);
                }
                while map.len() >= MAX_SERVERS {
                    let lru = map
                        .iter()
                        .min_by_key(|(_, e)| *e.used.lock())
                        .map(|(k, _)| k.clone())
                        .expect("non-empty");
                    evicted.extend(map.remove(&lru));
                }
                map.insert(
                    id.clone(),
                    Arc::new(Entry {
                        cell: Arc::new(OnceCell::new()),
                        begun: Instant::now(),
                        used: Mutex::new(Instant::now()),
                    }),
                );
            }
            (map[&id].clone(), evicted)
        };
        for e in evicted {
            stop(e);
        }
        *entry.used.lock() = Instant::now();
        let cell = entry.cell.clone();
        let (connect, command, root, options) = (
            self.inner.connect.clone(),
            cfg.command.clone(),
            root.to_path_buf(),
            external::with_extensions(&cfg.languages, cfg.initialization_options.clone()),
        );
        // Spawned so that a caller that stops waiting leaves the server starting for the next one.
        let task = tokio::spawn(async move {
            cell.get_or_init(|| start(connect, command, root, options))
                .await
                .clone()
        });
        let name = program_name(&cfg.command);
        match tokio::time::timeout(wait, task).await {
            Ok(Ok(started)) => started,
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err(format!(
                "`{name}` is still starting. Ask again in a moment."
            )),
        }
    }

    /// Remember that the server `command` of `key` answered with outside URI `uri`.
    fn remember(&self, key: &K, uri: &str, command: &[String]) {
        let mut x = self.inner.externals.lock();
        let id = (key.clone(), uri.to_string());
        if x.by.insert(id.clone(), command.to_vec()).is_some() {
            return;
        }
        x.order.push_back(id);
        while x.order.len() > MAX_EXTERNALS {
            if let Some(old) = x.order.pop_front() {
                x.by.remove(&old);
            }
        }
    }

    /// The command of the server that answered with outside URI `uri` for `key`, or None when no
    /// server of this process did.
    pub fn external_server(&self, key: &K, uri: &str) -> Option<Vec<String>> {
        self.inner
            .externals
            .lock()
            .by
            .get(&(key.clone(), uri.to_string()))
            .cloned()
    }

    /// A project file changed on disk. Running servers of that project hear about it.
    pub fn touch(&self, key: &K, rel: &str) {
        let running: Vec<Arc<Server>> = self
            .inner
            .entries
            .lock()
            .iter()
            .filter(|((k, _), _)| k == key)
            .filter_map(|(_, e)| e.running())
            .collect();
        for s in running {
            // LSP FileChangeType: 2 changed, 3 deleted.
            let kind = if s.root.join(rel).exists() { 2 } else { 3 };
            s.client.notify(
                "workspace/didChangeWatchedFiles",
                json!({"changes": [{"uri": s.uri(rel), "type": kind}]}),
            );
        }
    }

    /// Stop every server of `key`.
    pub fn forget(&self, key: &K) {
        let gone: Vec<Arc<Entry>> = {
            let mut map = self.inner.entries.lock();
            let ids: Vec<_> = map.keys().filter(|(k, _)| k == key).cloned().collect();
            ids.iter().filter_map(|id| map.remove(id)).collect()
        };
        gone.into_iter().for_each(stop);
    }

    /// Servers running now, by program name.
    pub fn running(&self) -> Vec<String> {
        self.inner
            .entries
            .lock()
            .values()
            .filter_map(|e| e.running())
            .filter(|s| s.client.dead().is_none())
            .map(|s| s.name.clone())
            .collect()
    }

    fn sweep_later(&self) {
        let mut sweeping = self.inner.sweeping.lock();
        if *sweeping {
            return;
        }
        *sweeping = true;
        let weak: Weak<PoolInner<K>> = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SWEEP).await;
                let Some(inner) = weak.upgrade() else { return };
                let idle: Vec<Arc<Entry>> = {
                    let mut map = inner.entries.lock();
                    let ids: Vec<_> = map
                        .iter()
                        .filter(|(_, e)| e.used.lock().elapsed() >= IDLE || e.stale())
                        .map(|(k, _)| k.clone())
                        .collect();
                    ids.iter().filter_map(|id| map.remove(id)).collect()
                };
                idle.into_iter().for_each(stop);
            }
        });
    }
}

fn stop(e: Arc<Entry>) {
    if let Some(s) = e.running() {
        tokio::spawn(async move {
            tracing::info!(server = %s.name, "language server stopped");
            s.client.close().await;
        });
    }
}

/// One `language_servers` entry of one project. File requests start from `base`, the built-in
/// answer, and replace what the server knows better.
pub struct LspProvider<K> {
    pub pool: LspPool<K>,
    pub key: K,
    pub root: PathBuf,
    pub config: LanguageServerConfig,
    pub base: Arc<dyn CodeProvider>,
}

/// Text of project files read while building one answer.
#[derive(Default)]
struct Texts(HashMap<String, Option<Arc<str>>>);

impl Texts {
    fn get(&mut self, root: &Path, rel: &str) -> Option<Arc<str>> {
        self.0
            .entry(rel.to_string())
            .or_insert_with(|| read_text(&root.join(rel)).map(Arc::from))
            .clone()
    }

    /// An outside file by absolute path, which cannot collide with a project-relative one.
    fn abs(&mut self, p: &Path) -> Option<Arc<str>> {
        self.0
            .entry(p.to_string_lossy().into_owned())
            .or_insert_with(|| read_text(p).map(Arc::from))
            .clone()
    }
}

fn read_text(p: &Path) -> Option<String> {
    let meta = std::fs::metadata(p).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    String::from_utf8(std::fs::read(p).ok()?).ok()
}

/// Whether the string array `list` holds `c`.
fn listed(list: &Value, c: &str) -> bool {
    list.as_array()
        .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(c)))
}

fn line_of(text: &str, line1: u32) -> Option<(usize, &str)> {
    let mut start = 0;
    for (i, l) in text.split('\n').enumerate() {
        if i + 1 == line1 as usize {
            return Some((start, l.strip_suffix('\r').unwrap_or(l)));
        }
        start += l.len() + 1;
    }
    None
}

impl<K: Hash + Eq + Clone + Send + Sync + 'static> LspProvider<K> {
    fn timeout(&self) -> Duration {
        Duration::from_secs(self.config.timeout_secs.into())
    }

    fn language_of(&self, path: &str) -> Option<&'static str> {
        let l = lang::for_path(path)?.id;
        self.config.languages.iter().any(|x| x == l).then_some(l)
    }

    async fn server(&self) -> Result<Arc<Server>, String> {
        self.pool
            .get(&self.key, &self.root, &self.config, self.timeout())
            .await
    }

    async fn open(&self, s: &Server, path: &str, lang: &str) -> Result<Arc<str>, String> {
        let text = read_text(&self.root.join(path))
            .ok_or_else(|| format!("Cannot read {path} as UTF-8 text under 1 MB."))?;
        s.sync(path, lang, &text).await;
        Ok(Arc::from(text))
    }

    fn busy_note(&self, s: &Server) -> Option<String> {
        s.client.busy().then(|| {
            format!(
                "`{}` is still loading the project, so this answer may be incomplete.",
                s.name
            )
        })
    }

    fn location(
        &self,
        s: &Server,
        texts: &mut Texts,
        uri: &str,
        r: Range,
        symbol: &str,
    ) -> Option<CodeLocation> {
        let (path, text, outside) = match s.rel(uri) {
            Some(rel) => {
                let text = texts.get(&self.root, &rel);
                (rel, text, None)
            }
            None => {
                let d = external::describe(uri)?;
                self.pool.remember(&self.key, uri, &self.config.command);
                let text = external::file_path(uri).and_then(|p| texts.abs(&p));
                (d.path, text, Some(uri.to_string()))
            }
        };
        let line = text.as_deref().and_then(|t| line_of(t, r.start_line + 1));
        let (name, preview_line) = match line {
            Some((at, l)) => {
                let a = convert::byte_at(l, r.start_col);
                let b = if r.end_line == r.start_line {
                    convert::byte_at(l, r.end_col).max(a)
                } else {
                    l.len()
                };
                let text = text.as_deref().expect("line came from it");
                (l[a..b].to_string(), preview(text, at + a))
            }
            None => (String::new(), String::new()),
        };
        let name = if name.is_empty() || name.contains(char::is_whitespace) {
            symbol.to_string()
        } else {
            name
        };
        Some(CodeLocation {
            len: convert::utf16_len(&name),
            name,
            path,
            uri: outside,
            line: r.start_line + 1,
            col: r.start_col,
            preview: preview_line,
            kind: None,
            container: None,
            via: None,
        })
    }

    async fn file(&self, req: &ProviderRequest, path: &str) -> Result<Option<Answer>, String> {
        let Some(lang) = self.language_of(path) else {
            return Ok(None);
        };
        let Some(Answer::File(mut file)) = self.base.answer(req).await? else {
            return Ok(None);
        };
        if file.warning.is_some() {
            // The built-in provider already cut this file, so the server would get the same cut.
            return Ok(Some(Answer::File(file)));
        }
        let s = self.server().await?;
        self.open(&s, path, lang).await?;
        let doc = json!({"textDocument": {"uri": s.uri(path)}});
        let t = self.timeout();
        let tokens = async {
            match &s.legend {
                Some(l) => Some(
                    s.client
                        .request("textDocument/semanticTokens/full", doc.clone(), t)
                        .await
                        .map(|v| {
                            let data: Vec<u32> = v["data"]
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|n| n.as_u64())
                                        .map(|n| n as u32)
                                        .collect()
                                })
                                .unwrap_or_default();
                            convert::decode_tokens(&data, l)
                        }),
                ),
                None => None,
            }
        };
        let symbols = async {
            if s.has("documentSymbolProvider") {
                Some(
                    s.client
                        .request("textDocument/documentSymbol", doc.clone(), t)
                        .await
                        .map(|v| convert::document_symbols(&v)),
                )
            } else {
                None
            }
        };
        let (tokens, symbols) = tokio::join!(tokens, symbols);
        let mut problems = vec![];
        let mut used = false;
        match tokens {
            Some(Ok(tk)) => {
                convert::overlay(&mut file, &tk);
                used = true;
            }
            Some(Err(e)) => problems.push(format!("Colors come from the built-in tokenizer: {e}")),
            None => {}
        }
        match symbols {
            Some(Ok(sy)) => {
                if !sy.is_empty() {
                    file.symbols = sy;
                }
                used = true;
            }
            Some(Err(e)) => problems.push(format!(
                "The outline comes from the built-in tokenizer: {e}"
            )),
            None => {}
        }
        if !used {
            return match problems.is_empty() {
                true => Ok(None),
                false => Err(problems.join(" ")),
            };
        }
        problems.extend(self.busy_note(&s));
        file.warning = (!problems.is_empty()).then(|| problems.join(" "));
        Ok(Some(Answer::File(file)))
    }

    /// Definitions of `symbol` found through `workspace/symbol`, as (uri, name range, kind, container).
    async fn find_definitions(
        &self,
        s: &Server,
        symbol: &str,
    ) -> Result<Vec<(String, Range, Option<Value>, Option<String>)>, String> {
        if !s.has("workspaceSymbolProvider") {
            return Ok(vec![]);
        }
        let v = s
            .client
            .request("workspace/symbol", json!({"query": symbol}), self.timeout())
            .await?;
        let mut texts = Texts::default();
        let mut out = vec![];
        for item in v.as_array().into_iter().flatten() {
            if item["name"].as_str() != Some(symbol) {
                continue;
            }
            let Some((uri, r)) = convert::locations(&item["location"]).into_iter().next() else {
                continue;
            };
            let Some(rel) = s.rel(&uri) else { continue };
            // A workspace symbol's range may cover the whole item; point at the name.
            let r = texts
                .get(&self.root, &rel)
                .and_then(|t| {
                    let (_, l) = line_of(&t, r.start_line + 1)?;
                    let c = convert::find_name(l, symbol, r.start_col)?;
                    Some(Range {
                        start_line: r.start_line,
                        start_col: c,
                        end_line: r.start_line,
                        end_col: c + convert::utf16_len(symbol),
                    })
                })
                .unwrap_or(r);
            out.push((
                uri,
                r,
                item.get("kind").cloned(),
                item["containerName"].as_str().map(str::to_string),
            ));
            if out.len() == MAX_DEFINITIONS {
                break;
            }
        }
        Ok(out)
    }

    async fn usages(
        &self,
        symbol: &str,
        path: Option<&str>,
        line: Option<u32>,
        col: Option<u32>,
        limit: usize,
    ) -> Result<Option<Answer>, String> {
        let lang = match path {
            Some(p) => match self.language_of(p) {
                Some(l) => Some(l),
                None => return Ok(None),
            },
            None => None,
        };
        let s = self.server().await?;
        let t = self.timeout();
        let mut texts = Texts::default();
        // The name at the asked position, when the request names one and the text there matches.
        let mut at: Option<(String, u32, u32)> = None;
        if let (Some(p), Some(lang), Some(line)) = (path, lang, line) {
            let text = self.open(&s, p, lang).await?;
            if let Some((_, l)) = line_of(&text, line)
                && let Some(c) = convert::find_name(l, symbol, col.unwrap_or(0))
            {
                at = Some((p.to_string(), line - 1, c));
            }
        }
        let mut defs: Vec<CodeLocation> = vec![];
        let from = match at {
            Some((p, l, c)) => {
                let position = json!({"textDocument": {"uri": s.uri(&p)}, "position": {"line": l, "character": c}});
                if s.has("definitionProvider") {
                    let v = s
                        .client
                        .request("textDocument/definition", position.clone(), t)
                        .await?;
                    for (uri, r) in convert::locations(&v) {
                        defs.extend(self.location(&s, &mut texts, &uri, r, symbol));
                    }
                }
                Some(position)
            }
            None => {
                let found = self.find_definitions(&s, symbol).await?;
                for (uri, r, kind, container) in &found {
                    if let Some(mut loc) = self.location(&s, &mut texts, uri, *r, symbol) {
                        loc.kind = kind
                            .as_ref()
                            .and_then(Value::as_u64)
                            .and_then(convert::symbol_kind);
                        loc.container = container.clone().filter(|c| !c.is_empty());
                        defs.push(loc);
                    }
                }
                match found.first() {
                    Some((uri, r, _, _)) => {
                        if let (Some(rel), Some(l)) = (s.rel(uri), lang::for_path(uri))
                            && let Some(text) = texts.get(&self.root, &rel)
                        {
                            s.sync(&rel, l.id, &text).await;
                        }
                        Some(
                            json!({"textDocument": {"uri": uri}, "position": {"line": r.start_line, "character": r.start_col}}),
                        )
                    }
                    None => None,
                }
            }
        };
        self.finish_usages(&s, texts, from, defs, symbol, (path, limit))
            .await
    }

    /// References from `from`, merged with `defs` into one answer. References outside the
    /// project are left out, because a library's own uses would crowd out the project's.
    async fn finish_usages(
        &self,
        s: &Server,
        mut texts: Texts,
        from: Option<Value>,
        mut defs: Vec<CodeLocation>,
        symbol: &str,
        (path, limit): (Option<&str>, usize),
    ) -> Result<Option<Answer>, String> {
        let t = self.timeout();
        let mut refs: Vec<CodeLocation> = vec![];
        if let Some(mut position) = from
            && s.has("referencesProvider")
        {
            position["context"] = json!({"includeDeclaration": false});
            let v = s
                .client
                .request("textDocument/references", position, t)
                .await?;
            for (uri, r) in convert::locations(&v) {
                if s.rel(&uri).is_some() {
                    refs.extend(self.location(s, &mut texts, &uri, r, symbol));
                }
            }
        }
        if defs.is_empty() && refs.is_empty() {
            return Ok(None);
        }
        let same = |a: &CodeLocation, b: &CodeLocation| {
            (&a.path, a.line, a.col) == (&b.path, b.line, b.col)
        };
        refs.retain(|r| !defs.iter().any(|d| same(d, r)));
        refs.sort_by(|a, b| {
            let first = |l: &CodeLocation| Some(l.path.as_str()) != path;
            (first(a), &a.path, a.line, a.col).cmp(&(first(b), &b.path, b.line, b.col))
        });
        refs.dedup_by(|a, b| same(a, b));
        let truncated = defs.len() + refs.len() > limit;
        defs.truncate(limit);
        refs.truncate(limit.saturating_sub(defs.len()));
        Ok(Some(Answer::Usages(CodeUsages {
            symbol: symbol.to_string(),
            provider: String::new(),
            definitions: defs,
            references: refs,
            truncated,
            warning: self.busy_note(s),
        })))
    }

    /// A file outside the project that this server answered with. The caller checks that it did.
    pub async fn external_file(&self, uri: &str) -> Result<CodeExternalFile, String> {
        let d = external::describe(uri).ok_or("Ostra cannot read this kind of location.")?;
        let content = match external::file_path(uri) {
            Some(p) => read_text(&p)
                .ok_or_else(|| format!("Cannot read {} as UTF-8 text under 1 MB.", d.path))?,
            None => {
                let s = self.server().await?;
                let v = s
                    .client
                    .request(
                        "java/classFileContents",
                        json!({"uri": uri}),
                        self.timeout(),
                    )
                    .await?;
                match v.as_str() {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => {
                        return Err(format!(
                            "`{}` has no source or decompiled text for {}.",
                            s.name, d.name
                        ));
                    }
                }
            }
        };
        if content.len() as u64 > MAX_FILE_BYTES {
            return Err(format!(
                "{} is larger than 1 MB, the size Ostra reads.",
                d.name
            ));
        }
        Ok(CodeExternalFile {
            uri: uri.to_string(),
            name: d.name,
            path: d.path,
            language: d.language.map(str::to_string),
            content,
            provider: program_name(&self.config.command),
        })
    }

    /// Definitions and project references of `symbol` at `line` (1-based) and `col` of outside
    /// file `uri`, which this server answered with.
    pub async fn external_usages(
        &self,
        uri: &str,
        symbol: &str,
        line: u32,
        col: Option<u32>,
        limit: usize,
    ) -> Result<CodeUsages, String> {
        let file = self.external_file(uri).await?;
        let s = self.server().await?;
        let lang = file.language.as_deref().unwrap_or("plaintext");
        s.sync_doc(uri, &convert::language_id(lang, &file.name), &file.content)
            .await;
        let c = line_of(&file.content, line)
            .and_then(|(_, l)| convert::find_name(l, symbol, col.unwrap_or(0)))
            .ok_or_else(|| format!("{symbol} is not on line {line} of {}.", file.name))?;
        let position =
            json!({"textDocument": {"uri": uri}, "position": {"line": line - 1, "character": c}});
        let mut texts = Texts::default();
        let mut defs = vec![];
        if s.has("definitionProvider") {
            let v = s
                .client
                .request("textDocument/definition", position.clone(), self.timeout())
                .await?;
            for (u, r) in convert::locations(&v) {
                defs.extend(self.location(&s, &mut texts, &u, r, symbol));
            }
        }
        let answer = self
            .finish_usages(&s, texts, Some(position), defs, symbol, (None, limit))
            .await?;
        Ok(match answer {
            Some(Answer::Usages(mut u)) => {
                u.provider = s.name.clone();
                u
            }
            _ => CodeUsages {
                symbol: symbol.to_string(),
                provider: s.name.clone(),
                definitions: vec![],
                references: vec![],
                truncated: false,
                warning: self.busy_note(&s),
            },
        })
    }

    /// Completions at `at` from the server, or None when it does not answer for this file. Waits
    /// at most `wait` for a server that is starting.
    pub async fn complete(
        &self,
        at: &At<'_>,
        wait: Duration,
    ) -> Result<Option<CodeCompletion>, String> {
        let Some(lang) = self.language_of(at.path) else {
            return Ok(None);
        };
        let s = self
            .pool
            .get(&self.key, &self.root, &self.config, wait)
            .await?;
        if !s.has("completionProvider") {
            return Ok(None);
        }
        let empty = || CodeCompletion {
            provider: s.name.clone(),
            items: vec![],
            incomplete: false,
            warning: None,
        };
        // LSP CompletionTriggerKind: 1 invoked, 2 trigger character, 3 incomplete list retrigger.
        let context = match at.trigger {
            Some(c) if listed(&s.caps["completionProvider"]["triggerCharacters"], c) => {
                json!({"triggerKind": 2, "triggerCharacter": c})
            }
            // The browser opens the list on characters of every language; this server has no
            // completions after this one.
            Some(_) => return Ok(Some(empty())),
            None if at.retrigger => json!({"triggerKind": 3}),
            None => json!({"triggerKind": 1}),
        };
        s.sync(at.path, lang, at.text).await;
        let v = s
            .client
            .request(
                "textDocument/completion",
                json!({"textDocument": {"uri": s.uri(at.path)},
                       "position": {"line": at.line - 1, "character": at.col},
                       "context": context}),
                self.timeout(),
            )
            .await?;
        let mut c = convert::completion(&v);
        c.provider = s.name.clone();
        c.warning = self.busy_note(&s);
        Ok(Some(c))
    }

    /// Signature help at `at` from the server. None when the server does not answer for this
    /// file or the cursor is not inside a call.
    pub async fn signature(
        &self,
        at: &At<'_>,
        wait: Duration,
    ) -> Result<Option<CodeSignatureHelp>, String> {
        let Some(lang) = self.language_of(at.path) else {
            return Ok(None);
        };
        let s = self
            .pool
            .get(&self.key, &self.root, &self.config, wait)
            .await?;
        let caps = &s.caps["signatureHelpProvider"];
        if !s.has("signatureHelpProvider") {
            return Ok(None);
        }
        // LSP SignatureHelpTriggerKind: 1 invoked, 2 trigger character, 3 content change.
        let kind = match at.trigger {
            Some(c)
                if listed(&caps["triggerCharacters"], c)
                    || (at.retrigger && listed(&caps["retriggerCharacters"], c)) =>
            {
                2
            }
            _ if at.retrigger => 3,
            Some(_) => return Ok(None),
            None => 1,
        };
        let mut context = json!({"triggerKind": kind, "isRetrigger": at.retrigger});
        if kind == 2 {
            context["triggerCharacter"] = json!(at.trigger);
        }
        s.sync(at.path, lang, at.text).await;
        let v = s
            .client
            .request(
                "textDocument/signatureHelp",
                json!({"textDocument": {"uri": s.uri(at.path)},
                       "position": {"line": at.line - 1, "character": at.col},
                       "context": context}),
                self.timeout(),
            )
            .await?;
        Ok(convert::signature_help(&v).map(|mut h| {
            h.provider = s.name.clone();
            h
        }))
    }

    /// Supertypes through the type hierarchy, or implementations through
    /// `textDocument/implementation`. None when the server does not answer for this file or
    /// lacks the request.
    pub async fn navigate(
        &self,
        at: &At<'_>,
        target: NavigateTarget,
        wait: Duration,
    ) -> Result<Option<CodeNavigation>, String> {
        let Some(lang) = self.language_of(at.path) else {
            return Ok(None);
        };
        let s = self
            .pool
            .get(&self.key, &self.root, &self.config, wait)
            .await?;
        let needs = match target {
            NavigateTarget::Supertypes => "typeHierarchyProvider",
            NavigateTarget::Implementations => "implementationProvider",
        };
        if !s.has(needs) {
            return Ok(None);
        }
        s.sync(at.path, lang, at.text).await;
        let t = self.timeout();
        let position = json!({"textDocument": {"uri": s.uri(at.path)},
                              "position": {"line": at.line - 1, "character": at.col}});
        let found: Vec<(String, Range)> = match target {
            NavigateTarget::Implementations => convert::locations(
                &s.client
                    .request("textDocument/implementation", position, t)
                    .await?,
            ),
            NavigateTarget::Supertypes => {
                let items = s
                    .client
                    .request("textDocument/prepareTypeHierarchy", position, t)
                    .await?;
                match items.as_array().and_then(|a| a.first()) {
                    Some(item) => convert::hierarchy_items(
                        &s.client
                            .request("typeHierarchy/supertypes", json!({"item": item}), t)
                            .await?,
                    ),
                    None => vec![],
                }
            }
        };
        let symbol = hint::word_at(at.text, at.line, at.col);
        let mut texts = Texts::default();
        let mut locations: Vec<CodeLocation> = found
            .into_iter()
            .filter_map(|(uri, r)| self.location(&s, &mut texts, &uri, r, symbol))
            .collect();
        let truncated = locations.len() > hint::MAX_NAVIGATION;
        locations.truncate(hint::MAX_NAVIGATION);
        Ok(Some(CodeNavigation {
            provider: s.name.clone(),
            symbol: symbol.to_string(),
            locations,
            truncated,
        }))
    }

    async fn symbols(&self, query: &str, limit: usize) -> Result<Option<Answer>, String> {
        let s = self.server().await?;
        if !s.has("workspaceSymbolProvider") {
            return Ok(None);
        }
        let v = s
            .client
            .request("workspace/symbol", json!({"query": query}), self.timeout())
            .await?;
        let mut texts = Texts::default();
        let mut items = vec![];
        let all = v.as_array().map(Vec::as_slice).unwrap_or_default();
        for item in all {
            let Some(name) = item["name"].as_str() else {
                continue;
            };
            let Some((uri, r)) = convert::locations(&item["location"]).into_iter().next() else {
                continue;
            };
            if let Some(mut loc) = self.location(&s, &mut texts, &uri, r, name) {
                loc.name = name.to_string();
                loc.len = convert::utf16_len(name);
                loc.kind = item["kind"].as_u64().and_then(convert::symbol_kind);
                loc.container = item["containerName"]
                    .as_str()
                    .filter(|c| !c.is_empty())
                    .map(str::to_string);
                items.push(loc);
            }
            if items.len() > limit {
                break;
            }
        }
        if items.is_empty() {
            return Ok(None);
        }
        let truncated = items.len() > limit;
        items.truncate(limit);
        Ok(Some(Answer::Symbols(CodeSymbols {
            provider: String::new(),
            items,
            truncated,
            warning: self.busy_note(&s),
        })))
    }
}

#[async_trait]
impl<K: Hash + Eq + Clone + Send + Sync + 'static> CodeProvider for LspProvider<K> {
    fn name(&self) -> String {
        program_name(&self.config.command)
    }

    async fn answer(&self, req: &ProviderRequest) -> Result<Option<Answer>, String> {
        match req {
            ProviderRequest::File { path, .. } => self.file(req, path).await,
            ProviderRequest::Usages {
                symbol,
                path,
                line,
                col,
                limit,
                ..
            } => {
                self.usages(symbol, path.as_deref(), *line, *col, *limit as usize)
                    .await
            }
            ProviderRequest::Symbols { query, limit, .. } => {
                self.symbols(query, *limit as usize).await
            }
            ProviderRequest::Deps { .. } => Ok(None),
        }
    }
}
