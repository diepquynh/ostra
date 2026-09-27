//! Native tool implementations. Names and input shapes follow Claude Code's tools, because the
//! agent prompts are tuned to them. The policy layer runs before [`execute`]; nothing here checks
//! permissions or guards.

mod bash;
mod code;
mod defs;
mod doc;
mod fs;
mod mcp;
mod misc;
mod search;
mod text;
mod web;
#[cfg(windows)]
mod winshell;

use ostra_core::AgentName;
use ostra_core::policy::ToolCall;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub use code::CodeNav;
pub use defs::{
    ToolDefinition, definitions, document_tool_definition, submit_tool_definition, wants_web_search,
};
pub use mcp::{McpConnector, McpOpened, McpTools};
pub use web::webfetch_hosts;

/// Resolves an embedded skill name (for example `meta-author`) to its path and content.
pub type SkillResolver = Arc<dyn Fn(&str) -> Option<(PathBuf, String)> + Send + Sync>;

/// Receives live output chunks of a running tool: `(call_id, chunk)`.
pub type LiveOutput = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Clone)]
pub struct ToolEnvConfig {
    pub agent: AgentName,
    pub repo_root: PathBuf,
    /// Empty outside a workspace. Custom skills among its artifacts load by name.
    pub workspace_root: PathBuf,
    pub session_dir: PathBuf,
    pub report_file: Option<PathBuf>,
    pub memory_db: PathBuf,
    /// Recorded as a lesson's `source` when the model gives none, for example `implementer x_123`.
    pub memory_source: String,
    pub skill_resolver: SkillResolver,
    /// Serves the code navigation tools; `None` where no index is wired in.
    pub code: Option<Arc<dyn CodeNav>>,
    /// The workspace's MCP server tools for this execution.
    pub mcp: Option<Arc<dyn McpTools>>,
}

/// Per-execution tool state: the persistent shell working directory, the files read so far, and
/// an HTTP client.
pub struct ToolEnv {
    config: ToolEnvConfig,
    cwd: Mutex<PathBuf>,
    read_files: Mutex<HashSet<PathBuf>>,
    http: reqwest::Client,
    /// Hosts a `WebFetch(domain:...)` allow rule names exactly, which may resolve to private
    /// addresses.
    private_hosts: Arc<Vec<String>>,
    /// Variables removed from every child process: provider credentials and Ostra's own.
    scrub_env: Vec<String>,
    /// The backend and profile Bash runs under; `None` runs it unsandboxed.
    sandbox: Option<(ostra_sandbox::Backend, ostra_sandbox::Profile)>,
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
        ToolOutput {
            text: text.into(),
            ..Default::default()
        }
    }

    pub(crate) fn err(text: impl Into<String>) -> Self {
        ToolOutput {
            text: text.into(),
            is_error: true,
            ..Default::default()
        }
    }
}

impl ToolEnv {
    pub fn new(config: ToolEnvConfig) -> Self {
        let cwd = config.repo_root.clone();
        let private_hosts = Arc::new(Vec::new());
        ToolEnv {
            config,
            cwd: Mutex::new(cwd),
            read_files: Mutex::new(HashSet::new()),
            http: web::client(private_hosts.clone()),
            private_hosts,
            scrub_env: bash::configured_secret_vars(),
            sandbox: None,
        }
    }

    /// Adds variables removed from every child process, such as the workspace's MCP secrets.
    pub fn with_scrub_env(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.scrub_env.extend(names);
        self.scrub_env.sort();
        self.scrub_env.dedup();
        self
    }

    /// Runs every Bash command under `backend` with this profile.
    pub fn with_sandbox(
        mut self,
        backend: ostra_sandbox::Backend,
        profile: ostra_sandbox::Profile,
    ) -> Self {
        self.sandbox = Some((backend, profile));
        self
    }

    /// A path as the sandboxed shell names it, as the host names the same file. Bubblewrap binds
    /// the scratch dir at `/tmp`; Seatbelt remaps nothing.
    pub fn sandbox_to_host(&self, inside: &Path) -> PathBuf {
        match &self.sandbox {
            Some((backend, profile)) if backend.has_mount_namespace() => profile.to_host(inside),
            _ => inside.to_path_buf(),
        }
    }

    /// Lets WebFetch reach private and loopback addresses for these exact hosts, taken from
    /// `WebFetch(domain:<host>)` allow rules.
    pub fn with_private_hosts(mut self, hosts: Vec<String>) -> Self {
        let hosts: Vec<String> = hosts.into_iter().map(|h| h.to_ascii_lowercase()).collect();
        self.private_hosts = Arc::new(hosts);
        self.http = web::client(self.private_hosts.clone());
        self
    }

    /// The call as the tool will run it: relative paths made absolute against the shell's working
    /// directory, Bash's `cwd` set to that directory, and a WebFetch URL in its parsed form. Check
    /// the policy on this call and run this call, so both see the same target.
    pub fn canonical_call(&self, call: &ToolCall) -> ToolCall {
        let Some(map) = call.input.as_object() else {
            return call.clone();
        };
        let mut map = map.clone();
        let mut absolute = |key: &str| {
            if let Some(raw) = map.get(key).and_then(|v| v.as_str())
                && !raw.trim().is_empty()
            {
                let abs = self.resolve(raw).display().to_string();
                map.insert(key.into(), serde_json::Value::String(abs));
            }
        };
        match call.tool.as_str() {
            "Read" | "Write" | "Edit" => absolute("file_path"),
            "Grep" | "Glob" | "Skill" | "Document" => absolute("path"),
            "Bash" | "PowerShell" | "Cmd" => {
                map.insert(
                    "cwd".into(),
                    serde_json::Value::String(self.cwd().display().to_string()),
                );
            }
            "WebFetch" => {
                if let Some(url) = map
                    .get("url")
                    .and_then(|v| v.as_str())
                    .and_then(|u| reqwest::Url::parse(u.trim()).ok())
                {
                    map.insert("url".into(), serde_json::Value::String(url.to_string()));
                }
            }
            _ => return call.clone(),
        }
        ToolCall::new(call.tool.clone(), serde_json::Value::Object(map))
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
    /// Under bubblewrap, `/tmp` is the execution's scratch dir, so in-process tools map it the
    /// same way.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let expanded = expand_home(path);
        self.sandbox_to_host(&ostra_core::paths::resolve(&self.cwd(), &expanded))
    }

    pub fn has_read(&self, path: &Path) -> bool {
        self.read_files.lock().contains(&ostra_core::paths::fold(path))
    }

    pub fn mark_read(&self, path: &Path) {
        // Key on the folded form so a verbatim path (`\\?\…`, e.g. the declared report file) and a
        // resolved path match on Windows despite prefix, case, and separator differences.
        self.read_files.lock().insert(ostra_core::paths::fold(path));
    }
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = ostra_core::paths::home()
    {
        return home.join(rest);
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
    let mut input = call.input.clone();
    if let Some(schema) = defs::input_schema(&call.tool, env.config().agent) {
        ostra_core::args::coerce_json_strings(&mut input, &schema);
    }
    let input = &input;
    let fut = async {
        match call.tool.as_str() {
            "Read" => fs::read(env, input).await,
            "Write" => fs::write(env, input).await,
            "Edit" => fs::edit(env, input).await,
            "Bash" => bash::run(env, call_id, input, live.clone(), cancel.clone()).await,
            #[cfg(windows)]
            "PowerShell" => {
                winshell::run(env, call_id, input, winshell::Shell::PowerShell, live.clone(), cancel.clone()).await
            }
            #[cfg(windows)]
            "Cmd" => {
                winshell::run(env, call_id, input, winshell::Shell::Cmd, live.clone(), cancel.clone()).await
            }
            "Grep" => search::grep(env, input).await,
            "Glob" => search::glob(env, input).await,
            "Skill" => misc::skill(env, input).await,
            "WebFetch" => web::fetch(env, input).await,
            "Report" => misc::report(env, input).await,
            "Document" => doc::document(env, input).await,
            "Memory" => misc::memory(env, input).await,
            "MemoryRecall" => misc::memory_recall(env, input).await,
            t if ostra_core::agent::is_code_tool(t) => code::run(env, t, input).await,
            t if ostra_core::mcp::is_gateway_tool(t) => {
                mcp::run(env, t, input, cancel.clone()).await
            }
            "WebSearch" => ToolOutput::err(
                "WebSearch runs on the model provider's side and has no local implementation.",
            ),
            other => ToolOutput::err(format!("Unknown tool `{other}`.")),
        }
    };
    // The shell tools drain their own output and honor cancellation internally, so they are not
    // wrapped in the cancel select that would drop that work.
    let self_cancelling = matches!(call.tool.as_str(), "Bash" | "PowerShell" | "Cmd");
    let mut out = if self_cancelling {
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
    str_arg(input, key)
        .ok_or_else(|| ToolOutput::err(format!("Missing required parameter `{key}`.")))
}

pub(crate) fn u64_arg(input: &serde_json::Value, key: &str) -> Option<u64> {
    input
        .get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)))
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
        let repo = ostra_core::paths::canonical(&repo).unwrap();
        let session = dir.join("session");
        std::fs::create_dir_all(&session).unwrap();
        let session = ostra_core::paths::canonical(&session).unwrap();
        ToolEnv::new(ToolEnvConfig {
            agent: AgentName::Implementer,
            report_file: Some(session.join("ostra-implementer-phase-1.md")),
            memory_db: session.join("memory/knowledge.sqlite3"),
            memory_source: "implementer x_test".into(),
            skill_resolver: Arc::new(|name: &str| {
                (name == "meta-author").then(|| {
                    (
                        PathBuf::from("/assets/skills/meta-author/SKILL.md"),
                        "# Meta".into(),
                    )
                })
            }),
            repo_root: repo,
            workspace_root: dir.to_path_buf(),
            session_dir: session,
            code: None,
            mcp: None,
        })
    }

    pub async fn run(env: &ToolEnv, tool: &str, input: serde_json::Value) -> ToolOutput {
        execute(
            env,
            "c1",
            &ToolCall::new(tool, input),
            None,
            CancellationToken::new(),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testutil::env_in;

    #[test]
    fn canonical_call_uses_the_shell_cwd() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let outside = ostra_core::paths::canonical(d.path()).unwrap().join("session");
        env.set_cwd(outside.clone());
        let write = env.canonical_call(&ToolCall::new(
            "Write",
            serde_json::json!({"file_path": "victim.toml", "content": "x"}),
        ));
        assert_eq!(
            write.input["file_path"],
            ostra_core::paths::strip_verbatim(&outside.join("victim.toml"))
                .display()
                .to_string()
        );
        let bash = env.canonical_call(&ToolCall::new(
            "Bash",
            serde_json::json!({"command": "ls", "cwd": "/"}),
        ));
        assert_eq!(bash.input["cwd"], outside.display().to_string());
        let grep = env.canonical_call(&ToolCall::new("Grep", serde_json::json!({"pattern": "x"})));
        assert!(grep.input.get("path").is_none());
    }

    #[test]
    fn canonical_webfetch_url_is_the_parsed_form() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let call = env.canonical_call(&ToolCall::new(
            "WebFetch",
            serde_json::json!({"url": "https://evil.example\\@docs.rs/x"}),
        ));
        let url = call.input["url"].as_str().unwrap();
        assert_eq!(
            reqwest::Url::parse(url).unwrap().host_str(),
            Some("evil.example")
        );
        assert!(url.starts_with("https://evil.example/"), "{url}");
    }
}
