//! Native tool implementations. Names and input shapes follow Claude Code's tools, because the
//! agent prompts are tuned to them. The policy layer runs before [`execute`]; nothing here checks
//! permissions or guards.

mod bash;
mod defs;
mod fs;
mod misc;
mod search;
mod text;
mod web;

use ostra_core::AgentName;
use ostra_core::policy::ToolCall;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub use defs::{ToolDefinition, definitions, submit_tool_definition, wants_web_search};

/// Resolves an embedded skill name (for example `meta-author`) to its path and content.
pub type SkillResolver = Arc<dyn Fn(&str) -> Option<(PathBuf, String)> + Send + Sync>;

/// Receives live output chunks of a running tool: `(call_id, chunk)`.
pub type LiveOutput = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Clone)]
pub struct ToolEnvConfig {
    pub agent: AgentName,
    pub repo_root: PathBuf,
    pub session_dir: PathBuf,
    pub report_file: Option<PathBuf>,
    pub memory_db: PathBuf,
    /// Recorded as a lesson's `source` when the model gives none, for example `implementer x_123`.
    pub memory_source: String,
    pub skill_resolver: SkillResolver,
}

/// Per-execution tool state: the persistent shell working directory, the files read so far, and
/// an HTTP client.
pub struct ToolEnv {
    config: ToolEnvConfig,
    cwd: Mutex<PathBuf>,
    read_files: Mutex<HashSet<PathBuf>>,
    http: reqwest::Client,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// Unified diff of a Write or Edit, for the Activity view.
    pub diff: Option<String>,
}

impl ToolOutput {
    pub(crate) fn ok(text: impl Into<String>) -> Self {
        ToolOutput { text: text.into(), ..Default::default() }
    }

    pub(crate) fn err(text: impl Into<String>) -> Self {
        ToolOutput { text: text.into(), is_error: true, ..Default::default() }
    }
}

impl ToolEnv {
    pub fn new(config: ToolEnvConfig) -> Self {
        let cwd = config.repo_root.clone();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .timeout(std::time::Duration::from_secs(20))
            .user_agent(concat!("ostra/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        ToolEnv { config, cwd: Mutex::new(cwd), read_files: Mutex::new(HashSet::new()), http }
    }

    pub fn config(&self) -> &ToolEnvConfig {
        &self.config
    }

    /// The shell's current working directory.
    pub fn cwd(&self) -> PathBuf {
        self.cwd.lock().clone()
    }

    pub(crate) fn set_cwd(&self, dir: PathBuf) {
        *self.cwd.lock() = dir;
    }

    /// Resolve a model-supplied path against the shell's working directory.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let expanded = expand_home(path);
        ostra_core::paths::resolve(&self.cwd(), &expanded)
    }

    pub fn has_read(&self, path: &Path) -> bool {
        self.read_files.lock().contains(path)
    }

    pub fn mark_read(&self, path: &Path) {
        self.read_files.lock().insert(path.to_path_buf());
    }
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

/// Run one tool call. Unknown tools return an error output.
pub async fn execute(
    env: &ToolEnv,
    call_id: &str,
    call: &ToolCall,
    live: Option<LiveOutput>,
    cancel: CancellationToken,
) -> ToolOutput {
    let started = Instant::now();
    let input = &call.input;
    let fut = async {
        match call.tool.as_str() {
            "Read" => fs::read(env, input).await,
            "Write" => fs::write(env, input).await,
            "Edit" => fs::edit(env, input).await,
            "Bash" => bash::run(env, call_id, input, live.clone(), cancel.clone()).await,
            "Grep" => search::grep(env, input).await,
            "Glob" => search::glob(env, input).await,
            "Skill" => misc::skill(env, input).await,
            "WebFetch" => web::fetch(env, input).await,
            "Report" => misc::report(env, input).await,
            "Memory" => misc::memory(env, input).await,
            "MemoryRecall" => misc::memory_recall(env, input).await,
            "WebSearch" => ToolOutput::err(
                "WebSearch runs on the model provider's side and has no local implementation.",
            ),
            other => ToolOutput::err(format!("Unknown tool `{other}`.")),
        }
    };
    let mut out = if call.tool == "Bash" {
        fut.await
    } else {
        tokio::select! {
            out = fut => out,
            _ = cancel.cancelled() => ToolOutput::err("Cancelled."),
        }
    };
    out.duration_ms = started.elapsed().as_millis() as u64;
    out
}

pub(crate) fn str_arg<'a>(input: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(|v| v.as_str())
}

pub(crate) fn required<'a>(input: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolOutput> {
    str_arg(input, key).ok_or_else(|| ToolOutput::err(format!("Missing required parameter `{key}`.")))
}

pub(crate) fn u64_arg(input: &serde_json::Value, key: &str) -> Option<u64> {
    input.get(key).and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)))
}

pub(crate) fn bool_arg(input: &serde_json::Value, key: &str) -> Option<bool> {
    input.get(key).and_then(|v| v.as_bool())
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub fn env_in(dir: &Path) -> ToolEnv {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo = std::fs::canonicalize(&repo).unwrap();
        let session = dir.join("session");
        std::fs::create_dir_all(&session).unwrap();
        let session = std::fs::canonicalize(&session).unwrap();
        ToolEnv::new(ToolEnvConfig {
            agent: AgentName::Implementer,
            report_file: Some(session.join("ostra-implementer-phase-1.md")),
            memory_db: session.join("memory/knowledge.sqlite3"),
            memory_source: "implementer x_test".into(),
            skill_resolver: Arc::new(|name: &str| {
                (name == "meta-author").then(|| (PathBuf::from("/assets/skills/meta-author/SKILL.md"), "# Meta".into()))
            }),
            repo_root: repo,
            session_dir: session,
        })
    }

    pub async fn run(env: &ToolEnv, tool: &str, input: serde_json::Value) -> ToolOutput {
        execute(env, "c1", &ToolCall::new(tool, input), None, CancellationToken::new()).await
    }
}
