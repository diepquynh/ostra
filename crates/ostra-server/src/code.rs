//! Code navigation for the Files view: display tokens, outline, usages, dependencies, and symbol
//! search. The work lives in `ostra-code`. This module picks each project's providers (its own
//! `code_provider` program, then its language servers, then the built-in index) and feeds write
//! touches to the index and the running servers.

use crate::api::ApiErr;
use crate::files::{self, Files, ProjectId};
use crate::workspace::WorkspaceRt;
use axum::http::StatusCode;
use ostra_code::provider::{self, Answer, CodeProvider, CommandProvider, NativeProvider};
use ostra_code::{Indexes, LspPool, LspProvider};
use ostra_core::code::{PROTOCOL_VERSION, ProviderRequest};
use std::sync::Arc;
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
}

impl Code {
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
        let project = w.settings().projects.into_iter().find(|p| p.key == key);
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
            }));
        }
        for config in project.map(|p| p.language_servers).unwrap_or_default() {
            chain.push(Arc::new(LspProvider {
                pool: self.servers.clone(),
                key: id.clone(),
                root: root.clone(),
                config,
                base: native.clone(),
            }));
        }
        chain.push(native);
        provider::ask(&chain, &req)
            .await
            .map_err(|e| ApiErr::new(StatusCode::BAD_REQUEST, e))
    }
}
