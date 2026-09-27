//! Code providers. The Files view asks a chain of providers and takes the first answer: a
//! project's own `code_provider` program when it has one, then the built-in index. A program
//! answers `null` for a request it leaves to the built-in provider. A failed or invalid answer
//! falls through too, and the answer that is shown carries the reason as its warning.

use crate::index::Indexes;
use crate::resolve::Resolver;
use crate::{MAX_FILE_BYTES, render};
use async_trait::async_trait;
use ostra_core::api::FileIndex;
use ostra_core::code::{
    CodeDeps, CodeFile, CodeLocation, CodeSymbols, CodeUsages, NATIVE_PROVIDER, ProviderRequest,
};
use std::hash::Hash;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    File(CodeFile),
    Usages(CodeUsages),
    Deps(CodeDeps),
    Symbols(CodeSymbols),
}

impl Answer {
    fn stamp(&mut self, provider: &str, warning: Option<String>) {
        let (p, w) = match self {
            Answer::File(a) => (&mut a.provider, &mut a.warning),
            Answer::Usages(a) => (&mut a.provider, &mut a.warning),
            Answer::Deps(a) => (&mut a.provider, &mut a.warning),
            Answer::Symbols(a) => (&mut a.provider, &mut a.warning),
        };
        *p = provider.to_string();
        if let Some(extra) = warning {
            *w = Some(match w.take() {
                Some(own) => format!("{extra} {own}"),
                None => extra,
            });
        }
    }
}

#[async_trait]
pub trait CodeProvider: Send + Sync {
    /// Shown in the browser beside every answer.
    fn name(&self) -> String;
    /// `Ok(None)` hands the request to the next provider.
    async fn answer(&self, req: &ProviderRequest) -> Result<Option<Answer>, String>;
}

/// Ask each provider in turn and return the first answer.
pub async fn ask(
    providers: &[Arc<dyn CodeProvider>],
    req: &ProviderRequest,
) -> Result<Answer, String> {
    let mut problems = vec![];
    for p in providers {
        match p.answer(req).await {
            Ok(Some(mut a)) => {
                let warning = (!problems.is_empty()).then(|| problems.join(" "));
                a.stamp(&p.name(), warning);
                return Ok(a);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(provider = %p.name(), op = req.op(), "code provider failed: {e}");
                problems.push(format!(
                    "The code provider `{}` failed, so the built-in index answered: {e}",
                    p.name()
                ));
            }
        }
    }
    Err(if problems.is_empty() {
        "No code provider answered this request.".into()
    } else {
        problems.join(" ")
    })
}

/// Answers nothing, for a language server asked only about files outside the project.
pub struct Unanswered;

#[async_trait]
impl CodeProvider for Unanswered {
    fn name(&self) -> String {
        String::new()
    }

    async fn answer(&self, _: &ProviderRequest) -> Result<Option<Answer>, String> {
        Ok(None)
    }
}

/// The built-in tokenizer and index.
pub struct NativeProvider<K> {
    pub indexes: Arc<Indexes<K>>,
    pub key: K,
    pub root: PathBuf,
    pub list: Arc<FileIndex>,
}

#[async_trait]
impl<K: Hash + Eq + Clone + Send + Sync + 'static> CodeProvider for NativeProvider<K> {
    fn name(&self) -> String {
        NATIVE_PROVIDER.to_string()
    }

    async fn answer(&self, req: &ProviderRequest) -> Result<Option<Answer>, String> {
        let (indexes, key, root, list) = (
            self.indexes.clone(),
            self.key.clone(),
            self.root.clone(),
            self.list.clone(),
        );
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let answer = match req {
                ProviderRequest::File { path, .. } => {
                    let resolver: Arc<Resolver> = indexes.resolver(&key, &root, &list);
                    let full = root.join(&path);
                    let meta = std::fs::metadata(&full).map_err(|e| format!("Cannot read {path}: {e}"))?;
                    if meta.len() > MAX_FILE_BYTES {
                        let mut f = render(&path, "", None);
                        f.warning = Some("The file is larger than the size Ostra tokenizes, so it shows without color.".into());
                        return Ok(Some(Answer::File(f)));
                    }
                    let bytes = std::fs::read(&full).map_err(|e| format!("Cannot read {path}: {e}"))?;
                    let src = String::from_utf8_lossy(&bytes);
                    Answer::File(render(&path, &src, Some(&resolver)))
                }
                ProviderRequest::Usages {
                    symbol, path, line, col, limit, ..
                } => Answer::Usages(indexes.with(&key, &root, &list, |ix| {
                    let mut u = match (path.as_deref(), line) {
                        (Some(p), Some(l)) => ix.usages_at(&symbol, p, l, col, limit as usize),
                        _ => ix.usages(&symbol, path.as_deref(), limit as usize),
                    };
                    if ix.truncated {
                        u.warning = Some(TRUNCATED.into());
                    }
                    u
                })),
                ProviderRequest::Deps { path, .. } => {
                    Answer::Deps(indexes.with(&key, &root, &list, |ix| ix.deps(&path)))
                }
                ProviderRequest::Symbols { query, limit, .. } => {
                    Answer::Symbols(indexes.with(&key, &root, &list, |ix| {
                        let mut s = ix.symbols(&query, limit as usize);
                        if ix.truncated {
                            s.warning = Some(TRUNCATED.into());
                        }
                        s
                    }))
                }
            };
            Ok(Some(answer))
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

const TRUNCATED: &str =
    "The project is larger than the size Ostra indexes, so some files are not searched.";

/// Output an external provider may write, and the stderr kept for its error message.
const MAX_OUTPUT: u64 = 64 * 1024 * 1024;
const MAX_STDERR: u64 = 1024 * 1024;
const MAX_TOKENS: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 20_000;

/// A project's own program, run once per request in the project folder. The request is one JSON
/// object on stdin; the answer is one JSON value on stdout.
pub struct CommandProvider {
    pub command: Vec<String>,
    pub root: PathBuf,
    pub timeout: Duration,
    /// The workspace's own sandbox settings.
    pub sandbox: ostra_core::config::WorkspaceSandbox,
}

impl CommandProvider {
    async fn run(&self, input: Vec<u8>) -> Result<Vec<u8>, String> {
        let (program, args) = self.command.split_first().ok_or("The command is empty.")?;
        let hc = ostra_core::sandbox::host_command(
            program,
            args,
            &self.root,
            &[&self.root],
            &self.sandbox,
        )?;
        let mut cmd = tokio::process::Command::new(&hc.program);
        for k in &hc.env_remove {
            cmd.env_remove(k);
        }
        let mut child = cmd
            .args(&hc.args)
            .envs(hc.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&self.root)
            .env(
                "OSTRA_CODE_PROTOCOL",
                ostra_core::code::PROTOCOL_VERSION.to_string(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Cannot start `{program}`: {e}."))?;
        let mut stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let work = async move {
            // A provider may answer without reading its input; a closed pipe is not an error.
            let _ = stdin.write_all(&input).await;
            drop(stdin);
            let mut out = vec![];
            let mut err = vec![];
            let mut stdout = stdout.take(MAX_OUTPUT + 1);
            let mut stderr = stderr.take(MAX_STDERR);
            let (o, _) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
            o.map_err(|e| e.to_string())?;
            let status = child.wait().await.map_err(|e| e.to_string())?;
            Ok::<_, String>((status, out, err))
        };
        let (status, out, err) = tokio::time::timeout(self.timeout, work)
            .await
            .map_err(|_| format!("It took longer than {} seconds.", self.timeout.as_secs()))??;
        if !status.success() {
            let why = String::from_utf8_lossy(&err);
            let last = why.trim().lines().last().unwrap_or("").trim();
            return Err(format!(
                "It exited with {status}{}{last}",
                if last.is_empty() { "." } else { ": " }
            ));
        }
        if out.len() as u64 > MAX_OUTPUT {
            return Err("Its answer is larger than 64 MB.".into());
        }
        Ok(out)
    }
}

#[async_trait]
impl CodeProvider for CommandProvider {
    fn name(&self) -> String {
        self.command
            .first()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            .unwrap_or_default()
    }

    async fn answer(&self, req: &ProviderRequest) -> Result<Option<Answer>, String> {
        let mut input = serde_json::to_vec(req).map_err(|e| e.to_string())?;
        input.push(b'\n');
        let out = self.run(input).await?;
        parse_answer(req, &out)
    }
}

fn bad_path(p: &str) -> bool {
    p.is_empty()
        || p.starts_with('/')
        || p.contains('\\')
        || p.contains('\0')
        || p.split('/').any(|s| s == ".." || s == ".")
}

fn check_locations(items: &[CodeLocation]) -> Result<(), String> {
    if items.len() > MAX_ITEMS {
        return Err(format!("It returned more than {MAX_ITEMS} locations."));
    }
    match items.iter().find(|l| bad_path(&l.path) || l.line == 0) {
        Some(l) => Err(format!(
            "It returned `{}` line {}. Use project-relative paths and 1-based lines.",
            l.path, l.line
        )),
        None => Ok(()),
    }
}

/// Read and check an external provider's stdout for request `req`.
pub fn parse_answer(req: &ProviderRequest, out: &[u8]) -> Result<Option<Answer>, String> {
    let text = std::str::from_utf8(out).map_err(|_| "Its answer is not UTF-8.".to_string())?;
    let text = text.trim();
    if text.is_empty() || text == "null" {
        return Ok(None);
    }
    let bad = |e: serde_json::Error| {
        format!(
            "Its answer to `{}` does not match the protocol: {e}.",
            req.op()
        )
    };
    let answer = match req {
        ProviderRequest::File { path, .. } => {
            let mut f: CodeFile = serde_json::from_str(text).map_err(bad)?;
            f.path = path.clone();
            if !f.tokens.len().is_multiple_of(4) || f.tokens.len() > MAX_TOKENS {
                return Err(
                    "Send tokens as groups of four numbers: line, column, length, class.".into(),
                );
            }
            if f.tokens
                .chunks(4)
                .any(|c| c[0] == 0 || c[3] as usize >= f.classes.len())
            {
                return Err(
                    "Every token needs a 1-based line and a class index inside classes.".into(),
                );
            }
            if f.symbols.len() > MAX_ITEMS || f.imports.len() > MAX_ITEMS {
                return Err(format!("Send at most {MAX_ITEMS} symbols and imports."));
            }
            if f.imports
                .iter()
                .any(|i| i.target.as_deref().is_some_and(bad_path))
            {
                return Err("Import targets must be project-relative paths.".into());
            }
            Answer::File(f)
        }
        ProviderRequest::Usages { symbol, .. } => {
            let mut u: CodeUsages = serde_json::from_str(text).map_err(bad)?;
            u.symbol = symbol.clone();
            for l in u.definitions.iter_mut().chain(u.references.iter_mut()) {
                if l.name.is_empty() {
                    l.name = symbol.clone();
                }
                // Ostra reads back only outside files a language server named.
                l.uri = None;
            }
            check_locations(&u.definitions)?;
            check_locations(&u.references)?;
            Answer::Usages(u)
        }
        ProviderRequest::Deps { path, .. } => {
            let mut d: CodeDeps = serde_json::from_str(text).map_err(bad)?;
            d.path = path.clone();
            if d.imports.len() > MAX_ITEMS || d.importers.len() > MAX_ITEMS {
                return Err(format!("Send at most {MAX_ITEMS} imports and importers."));
            }
            if d.importers.iter().any(|i| bad_path(&i.path))
                || d.imports
                    .iter()
                    .any(|i| i.target.as_deref().is_some_and(bad_path))
            {
                return Err("Dependency paths must be project-relative.".into());
            }
            Answer::Deps(d)
        }
        ProviderRequest::Symbols { .. } => {
            let mut s: CodeSymbols = serde_json::from_str(text).map_err(bad)?;
            s.items.iter_mut().for_each(|l| l.uri = None);
            check_locations(&s.items)?;
            Answer::Symbols(s)
        }
    };
    Ok(Some(answer))
}
