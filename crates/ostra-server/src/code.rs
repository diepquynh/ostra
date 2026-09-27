//! Code navigation for the Files view: display tokens, outline, usages, dependencies, and symbol
//! search. The work lives in `ostra-code`. This module picks each project's providers (its own
//! `code_provider` program, then its language servers, then the built-in index) and feeds write
//! touches to the index and the running servers.

use crate::api::ApiErr;
use crate::app::{App, Pushed};
use crate::code_watch::{self, Batch, OnBatch, ProjectWatch};
use crate::files::{self, Files, ProjectId};
use crate::workspace::WorkspaceRt;
use axum::http::StatusCode;
use ostra_code::hint::{self, At};
use ostra_code::provider::{self, Answer, CodeProvider, CommandProvider, NativeProvider};
use ostra_code::{Indexes, LspPool, LspProvider, MAX_FILE_BYTES};
use ostra_core::api::{FileIndex, ServerMsg};
use ostra_core::code::{
    CodeCompletion, CodeExternalFile, CodeNavigation, CodeSignatureHelp, CodeUsages,
    NavigateTarget, PROTOCOL_VERSION, ProviderRequest,
};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

pub const DEFAULT_USAGES: u32 = 500;
pub const MAX_USAGES: u32 = 2_000;
pub const DEFAULT_SYMBOLS: u32 = 50;
pub const MAX_SYMBOLS: u32 = 200;
const MAX_SYMBOL_LEN: usize = 256;

/// A request before the project root is known.
pub enum Ask {
    File {
        path: String,
    },
    Usages {
        symbol: String,
        path: Option<String>,
        line: Option<u32>,
        col: Option<u32>,
        limit: Option<u32>,
    },
    Deps {
        path: String,
    },
    Symbols {
        query: String,
        limit: Option<u32>,
    },
}

#[derive(Default)]
pub struct Code {
    indexes: Arc<Indexes<ProjectId>>,
    servers: LspPool<ProjectId>,
    watches: Mutex<HashMap<ProjectId, ProjectWatch>>,
}

impl Code {
    /// Watch the project's folders from now on, so outside edits reach the index. Replaces a
    /// watch on an old root.
    pub fn ensure_watch(&self, app: &Arc<App>, id: &ProjectId, root: &Path, list: &FileIndex) {
        let mut watches = self.watches.lock();
        if watches.get(id).is_some_and(|w| w.root == root) {
            return;
        }
        let weak = Arc::downgrade(app);
        let key = id.clone();
        let on_batch: OnBatch = Arc::new(move |b: Batch| {
            let Some(app) = weak.upgrade() else { return };
            on_outside_change(&app, &key, b);
        });
        match code_watch::start(root.to_path_buf(), &list.paths, on_batch) {
            Some(w) => {
                watches.insert(id.clone(), w);
            }
            None => tracing::warn!(
                "cannot watch {}; outside edits reach the code index within 30 s",
                root.display()
            ),
        }
    }

    /// A file changed on disk; the next query re-reads it.
    pub fn touch(&self, id: &ProjectId, rel: &str) {
        self.indexes.touch(id, rel);
        self.servers.touch(id, rel);
    }

    pub async fn ask(
        &self,
        files: &Files,
        w: &WorkspaceRt,
        key: &str,
        ask: Ask,
    ) -> Result<Answer, ApiErr> {
        let root = files::project_root(w, key)?;
        let rel = |raw: &str| -> Result<String, ApiErr> {
            let c = files::contain(&root, raw)?;
            if c.rel.is_empty() || c.real.is_dir() {
                return Err(ApiErr::new(
                    StatusCode::BAD_REQUEST,
                    "Name a file with path.",
                ));
            }
            Ok(c.rel)
        };
        let r = root.to_string_lossy().into_owned();
        let version = PROTOCOL_VERSION;
        let req = match ask {
            Ask::File { path } => ProviderRequest::File {
                version,
                root: r,
                path: rel(&path)?,
            },
            Ask::Deps { path } => ProviderRequest::Deps {
                version,
                root: r,
                path: rel(&path)?,
            },
            Ask::Usages {
                symbol,
                path,
                line,
                col,
                limit,
            } => {
                let symbol = symbol.trim().to_string();
                if symbol.is_empty()
                    || symbol.len() > MAX_SYMBOL_LEN
                    || symbol.chars().any(|c| c.is_whitespace() || c.is_control())
                {
                    return Err(ApiErr::new(
                        StatusCode::BAD_REQUEST,
                        "Name one symbol with symbol, such as `parse_config`.",
                    ));
                }
                let path = path
                    .filter(|p| !p.is_empty())
                    .map(|p| rel(&p))
                    .transpose()?;
                ProviderRequest::Usages {
                    version,
                    root: r,
                    symbol,
                    path,
                    line,
                    col,
                    limit: limit.unwrap_or(DEFAULT_USAGES).clamp(1, MAX_USAGES),
                }
            }
            Ask::Symbols { query, limit } => ProviderRequest::Symbols {
                version,
                root: r,
                query: query.trim().chars().take(MAX_SYMBOL_LEN).collect(),
                limit: limit.unwrap_or(DEFAULT_SYMBOLS).clamp(1, MAX_SYMBOLS),
            },
        };
        let list = files.index(w, key).await?;
        let settings = w.effective_settings();
        let sandbox = settings.sandbox();
        let project = settings.projects.into_iter().find(|p| p.key == key);
        let id: ProjectId = (w.id.clone(), key.to_string());
        let native: Arc<dyn CodeProvider> = Arc::new(NativeProvider {
            indexes: self.indexes.clone(),
            key: id.clone(),
            root: root.clone(),
            list,
        });
        let mut chain: Vec<Arc<dyn CodeProvider>> = vec![];
        // Read per request, so saved provider settings apply to the next one.
        if let Some(cp) = project.as_ref().and_then(|p| p.code_provider.clone()) {
            chain.push(Arc::new(CommandProvider {
                command: cp.command,
                root: root.clone(),
                timeout: Duration::from_secs(cp.timeout_secs.into()),
                sandbox: sandbox.clone(),
            }));
        }
        for config in project.map(|p| p.language_servers).unwrap_or_default() {
            chain.push(Arc::new(LspProvider {
                pool: self.servers.clone(),
                key: id.clone(),
                root: root.clone(),
                config,
                base: native.clone(),
                sandbox: sandbox.clone(),
            }));
        }
        chain.push(native);
        provider::ask(&chain, &req)
            .await
            .map_err(|e| ApiErr::new(StatusCode::BAD_REQUEST, e))
    }
}

/// A cursor in the unsaved text of a project file, from the editor.
pub struct HintAsk {
    pub path: String,
    pub text: String,
    pub line: u32,
    pub col: u32,
    pub trigger: Option<String>,
    pub retrigger: bool,
}

impl HintAsk {
    fn at(&self) -> At<'_> {
        At {
            path: &self.path,
            text: &self.text,
            line: self.line,
            col: self.col,
            trigger: self.trigger.as_deref(),
            retrigger: self.retrigger,
        }
    }
}

/// How long a hint waits for a language server that is starting. The server keeps starting, and
/// the code index answers completions meanwhile.
const HINT_WAIT: Duration = Duration::from_secs(3);

/// A project's language servers for one hint request, and what the index fallback needs.
struct HintRig {
    id: ProjectId,
    root: PathBuf,
    list: Arc<FileIndex>,
    servers: Vec<LspProvider<ProjectId>>,
}

impl Code {
    async fn hint_rig(
        &self,
        app: &Arc<App>,
        w: &WorkspaceRt,
        key: &str,
        ask: &mut HintAsk,
    ) -> Result<HintRig, ApiErr> {
        let bad = |m: &str| ApiErr::new(StatusCode::BAD_REQUEST, m);
        let root = files::project_root(w, key)?;
        let c = files::contain(&root, &ask.path)?;
        if c.rel.is_empty() || c.real.is_dir() {
            return Err(bad("Name a file with path."));
        }
        ask.path = c.rel;
        if ask.text.len() as u64 > MAX_FILE_BYTES {
            return Err(bad(
                "The file is larger than 1 MB, the size Ostra sends to a language server.",
            ));
        }
        if ask.line == 0 || ask.line as usize > ask.text.split('\n').count() {
            return Err(bad("The cursor line is outside the text."));
        }
        if ask.trigger.as_ref().is_some_and(|t| t.chars().count() != 1) {
            return Err(bad("Send one typed character as trigger."));
        }
        let list = app.files.index(w, key).await?;
        let id: ProjectId = (w.id.clone(), key.to_string());
        self.ensure_watch(app, &id, &root, &list);
        let base: Arc<dyn CodeProvider> = Arc::new(NativeProvider {
            indexes: self.indexes.clone(),
            key: id.clone(),
            root: root.clone(),
            list: list.clone(),
        });
        let settings = w.effective_settings();
        let sandbox = settings.sandbox();
        let project = settings.projects.into_iter().find(|p| p.key == key);
        let servers = project
            .map(|p| p.language_servers)
            .unwrap_or_default()
            .into_iter()
            .map(|config| LspProvider {
                pool: self.servers.clone(),
                key: id.clone(),
                root: root.clone(),
                config,
                base: base.clone(),
                sandbox: sandbox.clone(),
            })
            .collect();
        Ok(HintRig {
            id,
            root,
            list,
            servers,
        })
    }

    /// Completions from the first language server that answers for the file, else from the
    /// names the code index defines.
    pub async fn complete(
        &self,
        app: &Arc<App>,
        w: &WorkspaceRt,
        key: &str,
        mut ask: HintAsk,
    ) -> Result<CodeCompletion, ApiErr> {
        let rig = self.hint_rig(app, w, key, &mut ask).await?;
        let mut problems = vec![];
        for s in &rig.servers {
            match s.complete(&ask.at(), HINT_WAIT).await {
                Ok(Some(c)) => return Ok(c),
                Ok(None) => {}
                Err(e) => problems.push(format!(
                    "`{}` did not complete, so the code index did: {e}",
                    s.name()
                )),
            }
        }
        let indexes = self.indexes.clone();
        let HintRig { id, root, list, .. } = rig;
        let mut c = tokio::task::spawn_blocking(move || {
            indexes.with(&id, &root, &list, |ix| hint::from_index(ix, &ask.at()))
        })
        .await
        .map_err(|e| {
            ApiErr::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("The code index failed: {e}"),
            )
        })?;
        c.warning = (!problems.is_empty()).then(|| problems.join(" "));
        Ok(c)
    }

    /// Signature help from the first language server that answers for the file. The code index
    /// has none to give.
    pub async fn signature(
        &self,
        app: &Arc<App>,
        w: &WorkspaceRt,
        key: &str,
        mut ask: HintAsk,
    ) -> Result<Option<CodeSignatureHelp>, ApiErr> {
        let rig = self.hint_rig(app, w, key, &mut ask).await?;
        let mut problems = vec![];
        for s in &rig.servers {
            match s.signature(&ask.at(), HINT_WAIT).await {
                Ok(Some(h)) => return Ok(Some(h)),
                Ok(None) => {}
                Err(e) => problems.push(format!("`{}`: {e}", s.name())),
            }
        }
        match problems.is_empty() {
            true => Ok(None),
            false => Err(ApiErr::new(StatusCode::BAD_GATEWAY, problems.join(" "))),
        }
    }
}

impl Code {
    /// Supertypes or implementations of the name at the cursor from the code index, which is
    /// fast and holds every language it parses. Language servers answer when the index does
    /// not hold the file or does not know the name; with neither, nothing answers.
    pub async fn navigate(
        &self,
        app: &Arc<App>,
        w: &WorkspaceRt,
        key: &str,
        mut ask: HintAsk,
        target: NavigateTarget,
    ) -> Result<Option<CodeNavigation>, ApiErr> {
        let rig = self.hint_rig(app, w, key, &mut ask).await?;
        let indexes = self.indexes.clone();
        let HintRig {
            id,
            root,
            list,
            servers,
        } = rig;
        let (ask, found) = tokio::task::spawn_blocking(move || {
            let found = indexes.with(&id, &root, &list, |ix| {
                hint::navigate_index(ix, &ask.at(), target)
            });
            (ask, found)
        })
        .await
        .map_err(|e| {
            ApiErr::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("The code index failed: {e}"),
            )
        })?;
        if found.is_some() {
            return Ok(found);
        }
        let mut problems = vec![];
        for s in &servers {
            match s.navigate(&ask.at(), target, HINT_WAIT).await {
                Ok(Some(n)) => return Ok(Some(n)),
                Ok(None) => {}
                Err(e) => problems.push(format!("`{}`: {e}", s.name())),
            }
        }
        match problems.is_empty() {
            true => Ok(None),
            false => Err(ApiErr::new(StatusCode::BAD_GATEWAY, problems.join(" "))),
        }
    }
}

/// Outside URIs a request may name.
const MAX_URI_LEN: usize = 4096;

impl Code {
    /// The language server of project `key` that answered with outside URI `uri`. Only such URIs
    /// are read, so the browser cannot name an arbitrary file.
    fn external_server(
        &self,
        w: &WorkspaceRt,
        key: &str,
        uri: &str,
    ) -> Result<LspProvider<ProjectId>, ApiErr> {
        if uri.is_empty() || uri.len() > MAX_URI_LEN {
            return Err(ApiErr::new(
                StatusCode::BAD_REQUEST,
                "Name the dependency file with uri.",
            ));
        }
        let root = files::project_root(w, key)?;
        let id: ProjectId = (w.id.clone(), key.to_string());
        let command = self.servers.external_server(&id, uri).ok_or_else(|| {
            ApiErr::new(
                StatusCode::NOT_FOUND,
                "Ostra does not know this dependency file, because no language server of this project has pointed at it since the Ostra server started. Open it again from the code that uses it.",
            )
        })?;
        let settings = w.effective_settings();
        let sandbox = settings.sandbox();
        let project = settings.projects.into_iter().find(|p| p.key == key);
        let config = project
            .into_iter()
            .flat_map(|p| p.language_servers)
            .find(|c| c.command == command)
            .ok_or_else(|| {
                ApiErr::new(
                    StatusCode::NOT_FOUND,
                    "The language server that found this dependency file is no longer in the project's settings.",
                )
            })?;
        let base: Arc<dyn CodeProvider> = Arc::new(provider::Unanswered);
        Ok(LspProvider {
            pool: self.servers.clone(),
            key: id,
            root,
            config,
            base,
            sandbox,
        })
    }

    /// A read-only file outside the project that a language server pointed at.
    pub async fn external_file(
        &self,
        w: &WorkspaceRt,
        key: &str,
        uri: &str,
    ) -> Result<CodeExternalFile, ApiErr> {
        let s = self.external_server(w, key, uri)?;
        s.external_file(uri)
            .await
            .map_err(|e| ApiErr::new(StatusCode::BAD_GATEWAY, e))
    }

    /// Where the name at `line` and `col` of an outside file is defined, and where the project
    /// uses it, from the language server that pointed at the file.
    pub async fn external_usages(
        &self,
        w: &WorkspaceRt,
        key: &str,
        uri: &str,
        symbol: &str,
        (line, col): (u32, Option<u32>),
        limit: Option<u32>,
    ) -> Result<CodeUsages, ApiErr> {
        if symbol.is_empty()
            || symbol.len() > MAX_SYMBOL_LEN
            || symbol.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(ApiErr::new(
                StatusCode::BAD_REQUEST,
                "Name one symbol with symbol, such as `parse_config`.",
            ));
        }
        let s = self.external_server(w, key, uri)?;
        let limit = limit.unwrap_or(DEFAULT_USAGES).clamp(1, MAX_USAGES) as usize;
        s.external_usages(uri, symbol, line, col, limit)
            .await
            .map_err(|e| ApiErr::new(StatusCode::BAD_GATEWAY, e))
    }
}

/// Which view of the dependency graph to answer, before the project root is known.
pub enum GraphAsk {
    Packages,
    Package(String),
    File {
        path: String,
        depth: u32,
    },
    Symbol {
        path: String,
        symbol: String,
        line: Option<u32>,
        depth: u32,
    },
}

pub const MAX_GRAPH_DEPTH: u32 = 3;

impl Code {
    /// One view of the project's dependency graph, from the built-in index.
    pub async fn graph(
        &self,
        app: &Arc<App>,
        w: &WorkspaceRt,
        key: &str,
        ask: GraphAsk,
    ) -> Result<ostra_core::code::CodeGraph, ApiErr> {
        let root = files::project_root(w, key)?;
        let ask = match ask {
            GraphAsk::File { path, depth } => {
                let c = files::contain(&root, &path)?;
                if c.rel.is_empty() || c.real.is_dir() {
                    return Err(ApiErr::new(
                        StatusCode::BAD_REQUEST,
                        "Name a file with path.",
                    ));
                }
                GraphAsk::File {
                    path: c.rel,
                    depth: depth.clamp(1, MAX_GRAPH_DEPTH),
                }
            }
            GraphAsk::Symbol {
                path,
                symbol,
                line,
                depth,
            } => {
                let c = files::contain(&root, &path)?;
                if c.rel.is_empty() || c.real.is_dir() {
                    return Err(ApiErr::new(
                        StatusCode::BAD_REQUEST,
                        "Name the file that defines the symbol with path.",
                    ));
                }
                GraphAsk::Symbol {
                    path: c.rel,
                    symbol,
                    line,
                    depth: depth.clamp(1, MAX_GRAPH_DEPTH),
                }
            }
            other => other,
        };
        let list = app.files.index(w, key).await?;
        let id: ProjectId = (w.id.clone(), key.to_string());
        self.ensure_watch(app, &id, &root, &list);
        let indexes = self.indexes.clone();
        let answer = tokio::task::spawn_blocking(move || {
            indexes.with(&id, &root, &list, |ix| match ask {
                GraphAsk::Packages => Some(ix.packages_view()),
                GraphAsk::Package(unit) => ix.package_view(&unit),
                GraphAsk::File { path, depth } => ix.file_view(&path, depth),
                GraphAsk::Symbol {
                    path,
                    symbol,
                    line,
                    depth,
                } => ix.symbol_view(&path, &symbol, line, depth),
            })
        })
        .await
        .map_err(|e| {
            ApiErr::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("The code index failed: {e}"),
            )
        })?;
        answer.ok_or_else(|| {
            ApiErr::new(
                StatusCode::NOT_FOUND,
                "That package, file, or symbol is not in the code index. The index leaves out ignored files, files over 1 MB, and languages it does not parse.",
            )
        })
    }

    /// Drop the project's index and file list and build them again from disk.
    pub async fn reindex(
        &self,
        app: &Arc<App>,
        w: &WorkspaceRt,
        key: &str,
    ) -> Result<ostra_core::code::CodeReindex, ApiErr> {
        let root = files::project_root(w, key)?;
        let id: ProjectId = (w.id.clone(), key.to_string());
        app.files.invalidate(&id.0, &id.1);
        let list = app.files.index(w, key).await?;
        self.ensure_watch(app, &id, &root, &list);
        let indexes = self.indexes.clone();
        tokio::task::spawn_blocking(move || {
            let started = std::time::Instant::now();
            indexes.forget(&id);
            let (files, truncated) =
                indexes.with(&id, &root, &list, |ix| (ix.files(), ix.truncated));
            ostra_core::code::CodeReindex {
                indexed_files: files as u32,
                millis: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
                truncated,
            }
        })
        .await
        .map_err(|e| {
            ApiErr::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("The code index failed: {e}"),
            )
        })
    }
}

/// Paths a browser is told about per change; more reload the whole listing.
const MAX_PUSHED_PATHS: usize = 200;

fn on_outside_change(app: &App, id: &ProjectId, b: Batch) {
    if b.structural {
        app.files.invalidate(&id.0, &id.1);
    }
    for rel in &b.changed {
        app.code.touch(id, rel);
    }
    let paths = if b.changed.len() > MAX_PUSHED_PATHS {
        vec![]
    } else {
        b.changed.into_iter().collect()
    };
    let _ = app.push.send(Pushed {
        channels: vec![format!("workspace:{}", id.0)],
        msg: ServerMsg::ProjectFsChanged {
            workspace: id.0.clone(),
            key: id.1.clone(),
            paths,
        },
    });
}

/// The code navigation tools' view of the server. Executors are built before the app, so the app
/// is bound afterwards.
#[derive(Default)]
pub struct CodeTools {
    app: OnceLock<Weak<App>>,
}

impl CodeTools {
    pub fn bind(&self, app: &Arc<App>) {
        let _ = self.app.set(Arc::downgrade(app));
    }
}

/// The workspace project whose folder holds `dir`, the innermost when projects nest.
fn project_holding(app: &App, dir: &Path) -> Option<(Arc<WorkspaceRt>, String, PathBuf)> {
    let workspaces: Vec<Arc<WorkspaceRt>> = app.workspaces.read().values().cloned().collect();
    let mut best: Option<(Arc<WorkspaceRt>, String, PathBuf)> = None;
    for w in workspaces {
        for p in w.effective_settings().projects {
            let Ok(root) = ostra_core::paths::canonical(&p.path) else {
                continue;
            };
            let longer = best
                .as_ref()
                .is_none_or(|b| b.2.as_os_str().len() < root.as_os_str().len());
            if ostra_core::paths::is_inside(&root, dir) && longer {
                best = Some((w.clone(), p.key.clone(), root));
            }
        }
    }
    best
}

#[async_trait::async_trait]
impl ostra_tools::CodeNav for CodeTools {
    async fn call(&self, repo_root: &Path, tool: &str, input: &Value) -> Result<String, String> {
        let app = self
            .app
            .get()
            .and_then(Weak::upgrade)
            .ok_or("The Ostra server is shutting down.")?;
        let dir = ostra_core::paths::canonical(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
        let (w, key, root) = project_holding(&app, &dir).ok_or_else(|| {
            format!(
                "{} is not inside a project of an open workspace, so it has no code index.",
                dir.display()
            )
        })?;
        let list = app
            .files
            .index(&w, &key)
            .await
            .map_err(|e| e.message().to_string())?;
        let id: ProjectId = (w.id.clone(), key);
        app.code.ensure_watch(&app, &id, &root, &list);
        let indexes = app.code.indexes.clone();
        let (tool, input) = (tool.to_string(), input.clone());
        tokio::task::spawn_blocking(move || {
            indexes.with(&id, &root, &list, |ix| {
                ostra_code::tools::run(ix, &tool, &input)
            })
        })
        .await
        .map_err(|e| format!("The code index failed: {e}"))?
    }
}
