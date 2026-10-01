//! REST and WebSocket shapes shared with the browser. Exported to `web/src/api/gen/` by ts-rs
//! (`cargo test -p ostra-core export_bindings`).
//!
//! WebSocket `/ws` carries JSON text frames of [`ClientMsg`] and [`ServerMsg`], plus binary PTY
//! frames: one byte holding the execution id length `n`, then `n` bytes of execution id, then raw
//! terminal bytes. The browser sends terminal input as [`ClientMsg::TermInput`].

use crate::agent::{AgentName, Capability};
use crate::config::{
    ExecutorRouting, PermissionMode, PermissionRules, ProjectProfile, ResolvedRoute, Routing,
    ValidationIssue, WorkspaceSettings,
};
use crate::event::{
    AnswerSource, ContextDelivery, ContextFile, ExecPurpose, GateAnswer, GatePayload, JudgeKind,
    SessionEvent, SessionKind, SessionOptions, UploadedFile,
};
use crate::exec::{ExecutionDelta, ExecutionStatus, Usage};
use crate::executor::{ExecStream, ExecutorKind, HarnessKind};
use crate::ids::{DecisionId, ExecutionId, GateId, SessionId, WorkspaceId};
use crate::model::{Effort, Tier};
use crate::pipeline::{Category, Lane, PhaseInfo, StageKind};
use crate::policy::ToolCall;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use ts_rs::TS;

// ---------------------------------------------------------------------------------------------
// Workspaces and projects
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceSummary {
    pub id: WorkspaceId,
    pub name: String,
    #[ts(type = "string")]
    pub root: PathBuf,
    pub projects: u32,
    pub active_sessions: u32,
    /// False when the directory or its `workspace.toml` is gone.
    pub available: bool,
}

/// `POST /api/workspaces` and `POST /api/workspaces/validate`. Only `name` and `root` are needed;
/// the rest is what the setup wizard collects. Enum values arrive as strings so an unknown one
/// becomes a validation issue on its field instead of a parse error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateWorkspace {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    #[ts(type = "string")]
    pub root: PathBuf,
    #[serde(default)]
    #[ts(optional)]
    pub projects: Option<Vec<ImportProject>>,
    #[serde(default)]
    #[ts(optional)]
    pub permissions: Option<CreatePermissions>,
    #[serde(default)]
    #[ts(optional)]
    pub yolo: Option<CreateYolo>,
    #[serde(default)]
    #[ts(as = "Option<RoutingPreset>", optional)]
    pub routing_preset: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub notifications: Option<CreateNotifications>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceDetail {
    pub id: WorkspaceId,
    #[ts(type = "string")]
    pub root: PathBuf,
    pub settings: WorkspaceSettings,
    pub projects: Vec<ProjectView>,
    pub harnesses: Vec<HarnessStatus>,
    pub providers: Vec<ProviderStatus>,
    /// Problems with the current settings. Saving refuses settings with problems.
    pub validation: Vec<ValidationIssue>,
    /// Changes Ostra can make on its own for some of those problems (`POST .../settings/fix`).
    pub fixes: Vec<SettingsFix>,
    /// Every agent's definition from `agent.toml`, with its routes under the current settings.
    pub agents: Vec<AgentInfo>,
    /// Stacks with a seed reference, the values a project's `stack` offers besides detection.
    pub stacks: Vec<String>,
    /// Permission rules from `[permissions]` in the global config, merged under the workspace's.
    pub global_permissions: PermissionRules,
    /// Folder files whose commands changed outside Ostra and wait for approval. Ostra starts
    /// none of their programs until the user approves.
    pub pending_commands: Vec<PendingCommands>,
    /// Whether this workspace's agent commands run sandboxed: the global `[sandbox]` config with
    /// the workspace's own mode in place of the global one.
    pub sandbox: SandboxStatus,
    /// The network choice and hosts from `[sandbox]` in the global config, which the workspace's
    /// own choice replaces and its hosts add to.
    pub global_sandbox: GlobalSandbox,
    /// `tool_enforcement` from the global config, which the workspace's own value replaces.
    pub global_tool_enforcement: crate::config::ToolEnforcement,
}

/// A settings change that fixes the validation issue at `path` by setting it to `value`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SettingsFix {
    pub path: String,
    pub value: String,
    /// What the fix does, for a button.
    pub label: String,
}

/// The parts of the global `[sandbox]` config a workspace's settings build on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GlobalSandbox {
    pub network: crate::config::SandboxNetwork,
    pub allowed_hosts: Vec<String>,
}

/// The command-bearing settings of one folder file, as the user approves them together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingCommands {
    /// The project whose `.ostra/project.toml` this is; null for the workspace's
    /// `.ostra/workspace.toml`.
    pub project: Option<String>,
    #[ts(type = "string")]
    pub file: PathBuf,
    /// Sent back with the approval, so a file that changed again is not approved unseen.
    pub hash: String,
    pub items: Vec<PendingCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PendingKind {
    McpServer,
    LanguageServer,
    CodeProvider,
    FormatCommand,
    AllowRule,
    /// A project folder outside the workspace folder, which agents may then write.
    ProjectOutside,
}

/// One program or rule a folder file asks for. Values of `env` and `headers` are never sent,
/// because they can hold secrets; `variables` names the server environment they read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingCommand {
    pub kind: PendingKind,
    /// The MCP server name, the project key, or the rule.
    pub name: String,
    pub enabled: bool,
    /// The command line, shell-quoted.
    pub command: Option<String>,
    pub url: Option<String>,
    pub env: Vec<String>,
    pub headers: Vec<String>,
    pub variables: Vec<String>,
    /// The folder a `projectOutside` entry points at.
    pub path: Option<PathBuf>,
}

/// Approve the commands of one folder file as the browser showed them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApproveCommands {
    #[serde(default)]
    #[ts(optional)]
    pub project: Option<String>,
    pub hash: String,
}

/// One agent's definition and where it runs under the current settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentInfo {
    pub name: AgentName,
    /// Sentence-case display name, for example `Code reviewer`.
    pub label: String,
    pub description: String,
    /// The tier `Agent default` means.
    pub default_tier: Tier,
    /// Reasoning effort per executor tier table (`native`, `claude`, `codex`, `grok`, `agy`).
    pub effort: BTreeMap<String, Effort>,
    /// The effort `Agent default` means on the executor the agent is routed to.
    pub default_effort: Effort,
    pub capabilities: Vec<Capability>,
    pub timeout_secs: u64,
    /// The agent's route under the current settings, for work with no phase file. Null when it
    /// does not resolve; `validation` then names the problem.
    pub resolved: Option<ResolvedRoute>,
    /// What `Agent default` resolves to on the executor the agent is routed to.
    pub default_route: Option<ResolvedRoute>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InitStatus {
    Initialized,
    NotInitialized,
    /// An init session is running.
    Initializing,
    /// The folder no longer exists.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectView {
    pub key: String,
    #[ts(type = "string")]
    pub path: PathBuf,
    pub init_status: InitStatus,
    /// `.ultracode/` holds a complete Ultracode bootstrap that could be migrated.
    pub ultracode_bootstrap: bool,
    pub is_git: bool,
    /// The checked-out branch from `.git/HEAD`; a detached HEAD shows its short commit id. Null
    /// when the project is not a git repository or HEAD cannot be read.
    pub git_branch: Option<String>,
    pub stack: Option<String>,
    pub profile: Option<ProjectProfile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportProject {
    #[ts(type = "string")]
    pub path: PathBuf,
    pub key: String,
    pub stack: Option<String>,
}

/// `POST /api/workspaces/{ws}/clone`: clone a git repository and import it. Only `url`
/// and `key` are needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CloneProject {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    #[ts(optional)]
    pub stack: Option<String>,
    /// Where the checkout goes. Absent means `<workspace root>/<key>`. It must not exist or be an
    /// empty folder.
    #[serde(default)]
    #[ts(type = "string | null", optional)]
    pub path: Option<PathBuf>,
    /// The branch to check out. Absent means the remote's default branch.
    #[serde(default)]
    #[ts(optional)]
    pub branch: Option<String>,
    /// A saved git credential id. Absent means the saved credential that matches the URL, else
    /// the machine's own git and SSH setup.
    #[serde(default)]
    #[ts(optional)]
    pub credential: Option<String>,
}

/// `POST /api/workspaces/{ws}/projects/{key}/pull`: what a fast-forward pull did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitPullResult {
    pub branch: Option<String>,
    /// False when the branch was already up to date.
    pub updated: bool,
    /// Short commit ids before and after the pull.
    pub before: Option<String>,
    pub after: Option<String>,
    /// The last lines git printed.
    pub output: String,
}

/// One changed path in the Git dock, project-relative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitChange {
    pub path: String,
    pub mark: GitMark,
    /// A rename's or copy's source path, when it is inside the project.
    pub orig_path: Option<String>,
}

/// `GET /api/workspaces/{ws}/projects/{key}/git`: the project's branch and its changes, split the
/// way `git status` splits them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitRepoStatus {
    pub is_git: bool,
    /// None when HEAD is detached.
    pub branch: Option<String>,
    /// Short id of HEAD; None before the first commit.
    pub head: Option<String>,
    /// The upstream branch, such as `origin/main`.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Changes in the index.
    pub staged: Vec<GitChange>,
    /// Work tree changes not in the index, untracked files included.
    pub unstaged: Vec<GitChange>,
    /// Unmerged paths.
    pub conflicted: Vec<GitChange>,
    /// Staged paths outside the project. A commit takes the whole index, so it includes them.
    pub staged_elsewhere: u32,
    /// A list reached its cap.
    pub truncated: bool,
    /// Set when a session works in the project, so the dock disables every change to git.
    pub busy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitBranch {
    /// `main`, or `origin/main` for a remote branch.
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: Option<String>,
    /// Short id of the branch tip.
    pub commit: String,
    /// The tip's subject line.
    pub subject: String,
}

/// `POST .../git/stage` and `.../git/unstage`. An empty list means every change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitPaths {
    #[serde(default)]
    pub paths: Vec<String>,
}

/// `POST .../git/commit`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitCommitRequest {
    pub message: String,
}

/// `POST .../git/checkout`: switch to `branch`, or create it from `start` (HEAD when absent) and
/// switch to it. A remote branch name checks out a local branch that tracks it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitCheckoutRequest {
    pub branch: String,
    #[serde(default)]
    pub create: bool,
    #[serde(default)]
    pub start: Option<String>,
}

/// What a git command from the Git dock did: git's last lines, and the status after it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitOpResult {
    pub output: String,
    pub status: GitRepoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GitCredentialKind {
    /// A username and a personal access token, for `https://` remotes.
    Https,
    /// An unencrypted private key, for `ssh://` and `git@host:path` remotes.
    Ssh,
}

/// A git credential saved in the registry. The secret is reported only as present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitCredentialView {
    pub id: String,
    pub label: String,
    /// A host, optionally with a path prefix: `github.com` or `github.com/acme`. The longest match
    /// against a remote's host and path wins.
    pub host: String,
    pub kind: GitCredentialKind,
    pub username: Option<String>,
    pub has_secret: bool,
}

/// `POST /api/git/credentials` and `PATCH /api/git/credentials/{id}`. On a patch an absent field
/// keeps its value; `secret` is write-only.
#[derive(Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct GitCredentialEdit {
    #[serde(default)]
    #[ts(optional)]
    pub label: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub host: Option<String>,
    #[serde(default)]
    #[ts(as = "Option<GitCredentialKind>", optional)]
    pub kind: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub username: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub secret: Option<String>,
}

impl std::fmt::Debug for GitCredentialEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitCredentialEdit")
            .field("label", &self.label)
            .field("host", &self.host)
            .field("kind", &self.kind)
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HarnessStatus {
    pub harness: HarnessKind,
    pub command: String,
    pub installed: bool,
    pub version: Option<String>,
    /// `None` when Ostra cannot tell.
    pub logged_in: Option<bool>,
}

/// What a setup terminal runs for one harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HarnessSetupAction {
    /// The vendor's official installer script.
    Install,
    /// The CLI's own interactive login.
    Login,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HarnessSetupRequest {
    pub action: HarnessSetupAction,
}

/// A terminal on this machine running an install or login. Stream it on `term:<terminal>` and
/// type into it with `term_input`, like a harness execution. When it exits, the server checks
/// the harnesses again and sends `harness_status` on `home`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HarnessSetupTerminal {
    pub terminal: String,
    /// The command line it runs, for display.
    pub command: String,
}

/// `POST /api/fs/mkdir`: create a folder and any missing parents, like `mkdir -p`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsMkdir {
    /// Absolute, or starting with `~`.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderStatus {
    pub name: String,
    pub has_key: bool,
    /// Where the key came from: `env:NAME`, `saved`, `keychain`, `registered`, or `none`.
    pub source: String,
    /// The base URL requests go to. `None` means the provider's own.
    pub base_url: Option<String>,
    /// Where the base URL came from: `config`, `env:NAME`, `saved`, or `default`.
    pub base_url_source: String,
    /// What was saved from the browser. Secrets are reported only as present.
    pub saved: SavedProviderView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct SavedProviderView {
    pub base_url: Option<String>,
    pub has_api_key: bool,
    pub has_auth_token: bool,
}

/// `PATCH /api/providers/{name}`: an absent field keeps its saved value and an empty string
/// clears it. Environment variables still take precedence over what is saved here.
#[derive(Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct ProviderCredentialsEdit {
    #[serde(default)]
    #[ts(optional)]
    pub base_url: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub api_key: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub auth_token: Option<String>,
}

impl std::fmt::Debug for ProviderCredentialsEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderCredentialsEdit")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

/// `GET /api/environment`: what this machine offers, checked before any workspace exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnvironmentStatus {
    pub providers: Vec<ProviderStatus>,
    pub harnesses: Vec<HarnessStatus>,
    /// Stacks with a seed reference, the values a project's `stack` offers besides detection.
    pub stacks: Vec<String>,
    pub sandbox: SandboxStatus,
    pub shell: ShellStatus,
}

/// The shell the Bash tool runs: `bash` on Unix, Git for Windows' `bash.exe` on Windows, where the
/// `bash` on `PATH` is often the WSL launcher.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShellStatus {
    pub available: bool,
    /// The program the Bash tool starts.
    #[ts(optional)]
    pub path: Option<String>,
    /// What to install when it is missing.
    #[ts(optional)]
    pub message: Option<String>,
}

impl ShellStatus {
    pub fn check() -> Self {
        match crate::shells::bash() {
            Ok(p) => ShellStatus {
                available: true,
                path: Some(p.display().to_string()),
                message: None,
            },
            Err(e) => ShellStatus {
                available: false,
                path: None,
                message: Some(e),
            },
        }
    }
}

/// Whether agent commands run inside the sandbox on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SandboxStatus {
    pub mode: crate::config::SandboxMode,
    /// The effective default mode on this OS when none is set: `required` on Linux and macOS,
    /// `auto` on Windows, which has no backend yet. The console shows this instead of assuming
    /// `required` (WINDOWS_HANDOVER 1.1).
    pub default_mode: crate::config::SandboxMode,
    /// A sandbox backend works here.
    pub available: bool,
    /// `bubblewrap` on Linux or `seatbelt` on macOS, when available.
    #[ts(optional)]
    pub backend: Option<String>,
    /// Executions start sandboxed under the current mode.
    pub active: bool,
    /// What to do when the sandbox is wanted but missing, or required but unavailable.
    #[ts(optional)]
    pub message: Option<String>,
    /// What the working sandbox cannot enforce on this OS.
    #[ts(optional)]
    pub gaps: Option<String>,
    /// Decoy credential files work here: always under bubblewrap, and under Seatbelt on an
    /// admin account, because macOS shows the system log's sandbox reports to admins only.
    pub decoys: bool,
    /// The decoys every agent sandbox gets, as `~/` paths. A workspace adds to them.
    pub builtin_decoys: Vec<String>,
    /// The hosts every command reaches under `allowlist`: package registries, source hosts, and
    /// the harness CLIs' model APIs. A workspace adds to them.
    pub builtin_hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreatePermissions {
    #[serde(default)]
    #[ts(as = "Option<PermissionMode>", optional)]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateYolo {
    #[serde(default)]
    #[ts(optional)]
    pub default: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateNotifications {
    #[serde(default)]
    #[ts(optional)]
    pub push: Option<bool>,
}

/// Where implementers run, as the setup wizard offers it. Every other agent stays native.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RoutingPreset {
    /// Every agent on the native loop.
    #[default]
    Native,
    /// `implementer` and `write-test` on `harness:codex`.
    Codex,
    /// `implementer` and `write-test` on `harness:claude`.
    Claude,
}

impl RoutingPreset {
    /// Agents a harness preset moves off the native loop.
    pub const HARNESS_AGENTS: [&'static str; 2] = ["implementer", "write-test"];

    pub fn harness(self) -> Option<HarnessKind> {
        match self {
            RoutingPreset::Native => None,
            RoutingPreset::Codex => Some(HarnessKind::Codex),
            RoutingPreset::Claude => Some(HarnessKind::Claude),
        }
    }

    /// Replace the executor routes with this preset's. Model routes stay: tiers resolve per executor.
    pub fn apply(self, routing: &mut Routing) {
        routing.executor = ExecutorRouting::default();
        if let Some(h) = self.harness() {
            for agent in Self::HARNESS_AGENTS {
                routing
                    .executor
                    .by_agent
                    .insert(agent.to_string(), ExecutorKind::Harness(h));
            }
        }
    }
}

/// `GET /api/onboarding`: whether the first-run setup was finished on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OnboardingState {
    pub onboarded_at: Option<DateTime<Utc>>,
    /// Registered workspaces. Zero with no `onboarded_at` means first run.
    pub workspaces: u32,
}

/// The largest UI state document the server stores, in bytes of JSON.
pub const UI_STATE_MAX_BYTES: usize = 64 * 1024;

/// `GET/PATCH /api/workspaces/:ws/ui`: the console layout, so another browser restores it.
/// PATCH merges top-level fields; unknown fields are dropped.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct WorkspaceUiState {
    /// Open editor tabs in order.
    pub tabs: Vec<UiTab>,
    /// Id of the focused tab.
    pub active: Option<String>,
    /// Left dock tab: `sessions` or `files`.
    pub left_tab: Option<String>,
    /// Project key the Files tab shows.
    pub files_project: Option<String>,
    /// Null means the UI's default.
    pub sidebar_open: Option<bool>,
    /// The quick-question dock.
    pub dock_open: bool,
    /// `light` or `dark`. Null follows the system.
    pub theme: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct UiTab {
    /// The UI's resource id, for example `session:<id>` or `file:<project>:<path>`.
    pub id: String,
    /// The user pinned it: it sits before unpinned tabs, is never the preview tab, and bulk closes keep it.
    pub pinned: bool,
    /// The single preview tab that the next preview open replaces.
    pub preview: bool,
}

// ---------------------------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateSession {
    pub request: String,
    #[serde(default)]
    pub options: SessionOptions,
    /// Projects the user pinned. Empty lets the Classify judge choose.
    #[serde(default)]
    pub projects: Vec<String>,
    /// Files attached as context, each inside a workspace project.
    #[serde(default)]
    pub files: Vec<ContextFile>,
    /// Staged uploads (`UploadRef::id`) to keep in the session.
    #[serde(default)]
    pub uploads: Vec<String>,
    /// Rule B6: an existing documentation book for the session's docs. Absent names the book
    /// after the documented projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub docs_book: Option<String>,
}

/// A file uploaded to the workspace's staging area, waiting for a session or an addition to
/// claim it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UploadRef {
    pub id: String,
    pub name: String,
    #[ts(type = "number")]
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionStatus {
    Running,
    /// Waiting on at least one gate.
    Waiting,
    Completed,
    Failed,
    /// Nothing is running and no gate is open, but the session is not done.
    Stalled,
    /// The user paused it, or containment signals did (Rule P3). Nothing starts until the user
    /// continues it.
    Paused,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionSummary {
    pub id: SessionId,
    pub workspace: WorkspaceId,
    pub kind: SessionKind,
    pub request: String,
    pub category: Option<Category>,
    pub status: SessionStatus,
    /// Current lane and a short stage label (the hub's inferred stage).
    pub lane: Lane,
    pub stage_label: String,
    pub yolo: bool,
    pub open_gates: u32,
    pub projects: Vec<String>,
    pub cost_usd: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// A 2 to 5 word label from the Classify judge, or `Initialize <project>` for an init
    /// session. `None` until the request is classified.
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum StageStatus {
    Pending,
    Running,
    Waiting,
    Done,
    Skipped,
    Failed,
    Blocked,
}

/// One card on the session board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StageCard {
    pub stage: StageKind,
    pub lane: Lane,
    pub label: String,
    pub status: StageStatus,
    pub project: Option<String>,
    pub phase: Option<u32>,
    pub executions: Vec<ExecutionId>,
    pub gate: Option<GateId>,
    /// One-line status detail, for example `FAIL, 2 findings` or `pass 2 of 3`.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PhaseStatus {
    Queued,
    Implementing,
    Reviewing,
    Passed,
    Blocked,
    /// Removed from the queue because a phase it depends on failed (Rule D9).
    Removed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PhaseView {
    pub info: PhaseInfo,
    pub status: PhaseStatus,
    pub review_iterations: u32,
    /// `none`, `queued`, `running`, `passed`, `blocked`, or `skipped`.
    pub tests: String,
    pub security_block: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionView {
    pub id: ExecutionId,
    pub session: Option<SessionId>,
    pub agent: AgentName,
    pub purpose: Option<ExecPurpose>,
    pub stage: Option<StageKind>,
    pub project: String,
    pub executor: ExecutorKind,
    pub model: String,
    pub status: ExecutionStatus,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub usage: Usage,
    #[ts(type = "string | null")]
    pub report_path: Option<PathBuf>,
    pub native_session_id: Option<String>,
    pub spawn_block: String,
    pub error: Option<String>,
    /// A harness execution whose session can be reopened.
    pub can_resume: bool,
    /// Rule U1: a running task the session can do without, so the user may skip it.
    pub can_skip: bool,
    /// Rule U2: a running or paused execution, so the user may send it a correction.
    pub can_steer: bool,
    /// Rule U2: the correction a paused run reads when the session continues; still withdrawable.
    pub queued_steer: Option<String>,
    /// A live PTY exists for this execution.
    pub has_terminal: bool,
    /// `<agent>:<project key>`, the key of this execution's entry in `SessionDetail::execution_groups`.
    pub group: String,
    /// The run within its group, for example `Phase 2`, `Phase 1 · fix pass`, or `Spec · pass 2`.
    pub run_label: String,
    /// The live view this execution streams, taken from its executor.
    pub stream: ExecStream,
    /// One line about the last tool call or status message, at most 120 characters.
    pub summary: Option<String>,
    /// A stored terminal transcript exists, so the Terminal tab can replay the run.
    pub has_transcript: bool,
    /// The latest open gate whose payload names this execution, for example the permission ask
    /// that holds its tool call.
    pub pending_gate: Option<PendingGate>,
    /// The project folder the execution works in, so tool paths can be shown relative to it.
    /// Null for a side-panel answer, which has no session.
    #[ts(type = "string | null")]
    pub repo_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingGate {
    pub id: GateId,
    /// The gate payload's kind, for example `permission` or `execution_failed`.
    pub kind: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GateView {
    pub id: GateId,
    pub session: SessionId,
    pub title: String,
    pub explanation: String,
    pub payload: GatePayload,
    pub answer: Option<GateAnswer>,
    pub source: Option<AnswerSource>,
    pub reason: Option<String>,
    pub opened_at: DateTime<Utc>,
    pub answered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionView {
    pub id: DecisionId,
    pub judge: JudgeKind,
    pub subject: Option<String>,
    pub input_summary: String,
    #[ts(type = "unknown")]
    pub output: serde_json::Value,
    pub reason: String,
    pub overridden: bool,
    /// False once work that depends on the decision has started.
    pub can_override: bool,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactRef {
    #[ts(type = "string")]
    pub path: PathBuf,
    /// `research`, `spec`, `plan`, `phase`, `report`, `ledger`, `completion`.
    pub kind: String,
    pub label: String,
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionDetail {
    pub summary: SessionSummary,
    pub stages: Vec<StageCard>,
    pub phases: Vec<PhaseView>,
    pub executions: Vec<ExecutionView>,
    pub gates: Vec<GateView>,
    pub decisions: Vec<DecisionView>,
    pub artifacts: Vec<ArtifactRef>,
    /// Markdown of the completion report, once written.
    pub completion: Option<String>,
    #[ts(type = "string")]
    pub session_root: PathBuf,
    /// Executions grouped by agent and project, ordered by their first start.
    pub execution_groups: Vec<ExecutionGroupView>,
    /// Every fact-check pass over the spec and the plan, oldest first.
    pub fact_checks: Vec<FactCheckView>,
    /// Files attached to the request when the session started.
    pub files: Vec<ContextFile>,
    /// Files uploaded with the request.
    pub uploads: Vec<UploadedFile>,
    /// Context the user added after the start, oldest first.
    pub additions: Vec<ContextAddition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ContextAddition {
    pub text: String,
    pub files: Vec<ContextFile>,
    pub uploads: Vec<UploadedFile>,
    pub delivery: ContextDelivery,
    pub at: DateTime<Utc>,
    /// Rule C2: queued behind running executions, so the user may still withdraw it.
    pub queued: bool,
    /// The user withdrew it before any step read it.
    pub withdrawn: bool,
}

/// One fact-check pass, for showing its findings on the document it checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FactCheckView {
    pub execution: ExecutionId,
    /// `spec` or `plan`.
    pub target: String,
    /// The artifact version the pass checked.
    pub version: u32,
    /// True when it checked the artifact as it stands now.
    pub current: bool,
    /// `None` while the pass runs.
    pub verdict: Option<crate::submit::Verdict>,
    pub findings: Vec<crate::submit::FactCheckFinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AnswerGate {
    pub answer: GateAnswer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OverrideDecision {
    #[ts(type = "unknown")]
    pub output: serde_json::Value,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetYolo {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AmendRequest {
    pub text: String,
    #[serde(default)]
    pub files: Vec<ContextFile>,
    /// Staged uploads (`UploadRef::id`) to keep in the session.
    #[serde(default)]
    pub uploads: Vec<String>,
    #[serde(default)]
    pub delivery: ContextDelivery,
}

/// Rule U2: a correction for one execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SteerRequest {
    pub text: String,
}

/// One persisted Activity item of an execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActivityItem {
    pub seq: i64,
    pub at: DateTime<Utc>,
    pub delta: ExecutionDelta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Artifact {
    #[ts(type = "string")]
    pub path: PathBuf,
    pub content: String,
    /// The markdown outline, in document order.
    pub headings: Vec<Heading>,
    /// The typed document the markdown was rendered from, for research, spec, plan, and phase files.
    pub document: Option<crate::doc::DocumentView>,
    /// Not UTF-8 text, or larger than the preview limit: `content` is empty and only a download is
    /// offered.
    pub binary: bool,
    #[ts(type = "number")]
    pub size: u64,
}

/// One markdown heading of an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Heading {
    /// GitHub-style anchor slug, unique within the document (`reqs`, `reqs-1`).
    pub id: String,
    /// 1 to 6.
    pub level: u8,
    pub title: String,
}

/// The runs of one agent on one project within a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionGroupView {
    /// `<agent>:<project key>`.
    pub group: String,
    pub agent: AgentName,
    pub project: String,
    /// `running` while any run is running, else the status of the latest run.
    pub status: ExecutionStatus,
    pub cost_usd: f64,
    /// Oldest first.
    pub executions: Vec<ExecutionId>,
}

// ---------------------------------------------------------------------------------------------
// Navigation: the Sessions tree, search, and workspace activity
// ---------------------------------------------------------------------------------------------

/// `GET /api/workspaces/:ws/tree`: every session of the workspace, most recently updated first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceTree {
    pub sessions: Vec<TreeSession>,
}

/// One session node of the Sessions tree. `tree_patch` messages carry a whole node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TreeSession {
    pub id: SessionId,
    pub title: Option<String>,
    pub request: String,
    pub kind: SessionKind,
    pub status: SessionStatus,
    pub open_gates: u32,
    pub cost_usd: f64,
    pub updated_at: DateTime<Utc>,
    /// In order of each group's first start.
    pub groups: Vec<TreeGroup>,
    pub artifacts: Vec<ArtifactRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TreeGroup {
    /// `<agent>:<project key>`.
    pub group: String,
    pub agent: AgentName,
    pub project: String,
    /// `running` while any run is running, else the status of the latest run.
    pub status: ExecutionStatus,
    pub cost_usd: f64,
    /// Oldest first.
    pub runs: Vec<TreeRun>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TreeRun {
    pub id: ExecutionId,
    pub run_label: String,
    pub status: ExecutionStatus,
    pub stream: ExecStream,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SearchKind {
    Session,
    Execution,
    Artifact,
    File,
    Project,
    Lesson,
    Setting,
}

/// `GET /api/workspaces/:ws/search?q=&limit=`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchResults {
    /// Best match first.
    pub items: Vec<SearchHit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchHit {
    pub kind: SearchKind,
    /// The UI resource id: `session:<id>`, `exec:<id>`, `artifact:<absolute path>`,
    /// `file:<project key>:<relative path>`, `project:<key>`, `lesson:<project key>:<lesson id>`,
    /// or `setting:<dotted key>`.
    pub id: String,
    pub label: String,
    pub hint: Option<String>,
    /// Higher is better; comparable across kinds within one response.
    pub score: f64,
}

/// `GET /api/workspaces/:ws/activity`: counts for the status bar and the workspace menu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceActivity {
    /// Oldest start first. Includes side-panel answers, which have no session.
    pub running: Vec<RunningExecution>,
    /// Oldest first.
    pub open_gates: Vec<OpenGateRef>,
    /// Execution spend since local midnight on the server.
    pub spend_today_usd: f64,
    /// Execution spend since local midnight six days ago: today and the six days before it.
    pub spend_week_usd: f64,
    /// Start of the `spend_today_usd` window.
    pub today_since: DateTime<Utc>,
    /// Start of the `spend_week_usd` window. Pass it as `since` to the cost report for the same week.
    pub week_since: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RunningExecution {
    pub id: ExecutionId,
    pub session: Option<SessionId>,
    pub agent: AgentName,
    pub project: String,
    pub run_label: String,
    pub stream: ExecStream,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OpenGateRef {
    pub id: GateId,
    pub session: SessionId,
    pub session_title: Option<String>,
    pub title: String,
    /// The gate payload's kind, for example `plan_approval` or `permission`.
    pub kind: String,
    pub opened_at: DateTime<Utc>,
}

/// Group key of an execution: `<agent>:<project key>`.
pub fn execution_group(agent: AgentName, project: &str) -> String {
    format!("{agent}:{project}")
}

/// Longest [`ExecutionView::summary`], in characters.
pub const SUMMARY_CHARS: usize = 120;

/// `text` on one line with whitespace collapsed, cut to [`SUMMARY_CHARS`] with an ellipsis.
pub fn summary_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= SUMMARY_CHARS {
        return line;
    }
    let mut cut: String = line.chars().take(SUMMARY_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// The summary line of a tool call: its tool and main argument, for example `Edit src/lib.rs` or
/// `Bash npm test`. Paths under `root` are shown relative to it.
/// Drops the `root` prefix from a tool-call path so the activity line reads relatively. On Windows
/// the compare folds case, separators, and the `\\?\` prefix, since the resolved path and the root
/// can differ in all three (WINDOWS_HANDOVER 1.4).
fn strip_root(arg: &str, root: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        let root = crate::paths::strip_verbatim(root).display().to_string();
        let mut out = arg.to_string();
        for sep in ["\\", "/"] {
            let prefix = format!("{}{sep}", root.replace(['\\', '/'], sep));
            // ASCII lowercasing keeps byte offsets, so matches index the original text.
            while let Some(at) = out.to_ascii_lowercase().find(&prefix.to_ascii_lowercase()) {
                out.replace_range(at..at + prefix.len(), "");
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        let prefix = format!("{}/", root.display());
        if prefix.len() > 1 {
            arg.replace(&prefix, "")
        } else {
            arg.to_string()
        }
    }
}

pub fn tool_summary(call: &ToolCall, root: Option<&std::path::Path>) -> String {
    const KEYS: [&str; 8] = [
        "file_path",
        "command",
        "pattern",
        "url",
        "path",
        "query",
        "skill",
        "notebook_path",
    ];
    let Some(arg) = KEYS.iter().find_map(|k| call.str_field(k)) else {
        return summary_line(&call.tool);
    };
    let arg = match root {
        Some(r) => strip_root(arg, r),
        None => arg.to_string(),
    };
    summary_line(&format!("{} {arg}", call.tool))
}

// ---------------------------------------------------------------------------------------------
// Memory, cost, side panel, push
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Lesson {
    pub id: i64,
    pub area: String,
    pub lesson: String,
    pub source: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LessonEdit {
    pub id: Option<i64>,
    pub area: String,
    pub lesson: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CostRow {
    /// Grouping value: a session id, stage, agent, or executor.
    pub key: String,
    pub executions: u32,
    pub usage: Usage,
    /// The metric Ultracode's bench tracks: cache reads divided by tool calls.
    pub cache_reads_per_tool_call: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CostReport {
    /// Only executions that started at or after this instant count. Null means all time.
    pub since: Option<DateTime<Utc>>,
    pub by_session: Vec<CostRow>,
    pub by_stage: Vec<CostRow>,
    pub by_agent: Vec<CostRow>,
    pub by_executor: Vec<CostRow>,
    pub total: CostRow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AskQuestion {
    pub question: String,
    /// When asked from a session, its artifacts join the context.
    pub session: Option<SessionId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AskStarted {
    /// Stream the answer on `execution:<id>`.
    pub execution: ExecutionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushSubscription {
    pub endpoint: String,
    pub keys: PushKeys,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushKeys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ServerInfo {
    pub version: String,
    /// VAPID public key (URL-safe base64) for push subscriptions.
    pub vapid_public_key: String,
}

/// Directory listing for the folder picker (`GET /api/fs/list?path=`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsListing {
    #[ts(type = "string")]
    pub path: PathBuf,
    #[ts(type = "string | null")]
    pub parent: Option<PathBuf>,
    pub entries: Vec<FsEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    /// The directory holds `.git`.
    pub is_git: bool,
    /// The directory holds `.ostra/INVENTORY.md`.
    pub is_ostra_project: bool,
}

/// Type-to-browse folder listing (`GET /api/fs`). A missing or unreadable `path` is not an error:
/// `exists` or `readable` is false, `entries` is empty, and `nearest` names the deepest existing
/// ancestor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsBrowse {
    /// The requested folder with `~` expanded; canonical when it exists.
    #[ts(type = "string")]
    pub path: PathBuf,
    #[ts(type = "string | null")]
    pub parent: Option<PathBuf>,
    /// The user's home folder, so the client can show and expand `~`.
    #[ts(type = "string")]
    pub home: PathBuf,
    pub exists: bool,
    pub readable: bool,
    /// `path` itself when it is a readable folder, else its deepest existing ancestor.
    #[ts(type = "string")]
    pub nearest: PathBuf,
    /// Folders only. Filtered by `prefix` (starts-with matches first, then contains), capped by `limit`.
    pub entries: Vec<FsEntry>,
    /// More folders matched than `limit` allowed.
    pub truncated: bool,
    /// `path` itself holds `.git`.
    pub is_git: bool,
    /// `path` itself holds `.ostra/INVENTORY.md`.
    pub is_ostra_project: bool,
}

/// One changed file of a review loop, for the ledger's diff view
/// (`GET /api/sessions/:id/diff?project=&phase=`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffFile {
    /// Project-relative.
    pub path: String,
    /// The file at HEAD; empty when it is new.
    pub original: String,
    /// The file in the working tree; empty when it was deleted.
    pub modified: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuthExchange {
    pub token: String,
}

/// A browser signed in to this server, from `GET /api/auth/sessions`. The id names the sign-in
/// for revoking; it is not the cookie.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SignInSession {
    pub id: String,
    pub created: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub expires: DateTime<Utc>,
    /// The browser's `User-Agent` at sign-in, when known.
    pub user_agent: Option<String>,
    /// The address the sign-in came from, when known.
    pub ip: Option<String>,
    /// The sign-in of the browser making this request.
    pub current: bool,
}

/// How many sign-ins a revoke request removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RevokedSignIns {
    pub revoked: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiError {
    pub error: String,
    #[serde(default)]
    pub issues: Vec<ValidationIssue>,
}

// ---------------------------------------------------------------------------------------------
// Project files (read-only)
// ---------------------------------------------------------------------------------------------

/// A file's state in git, from `git status --porcelain=v2`: modified (also type changes and
/// conflicts), added, deleted, renamed or copied, or untracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum GitMark {
    #[serde(rename = "M")]
    Modified,
    #[serde(rename = "A")]
    Added,
    #[serde(rename = "D")]
    Deleted,
    #[serde(rename = "R")]
    Renamed,
    #[serde(rename = "?")]
    Untracked,
}

/// The session execution that last changed a file, from the work pass's `changed_files` or,
/// while it runs, from its Edit and Write tool results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChangedBy {
    pub session: SessionId,
    pub execution: ExecutionId,
    pub agent: AgentName,
    pub phase: Option<u32>,
    /// The change came from the closing test loop (`Phase: N-tests`).
    pub tests: bool,
    /// The engine's stage step ran `git add` for this change and it succeeded.
    pub staged: bool,
    /// The execution is still running.
    pub running: bool,
    pub at: DateTime<Utc>,
}

/// One entry of `GET /api/workspaces/:ws/projects/:key/tree`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectTreeEntry {
    pub name: String,
    /// Project-relative, `/`-separated.
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    /// Bytes; 0 for folders.
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
    /// Matched by `.gitignore`, `.git/info/exclude`, or the global excludes file.
    pub ignored: bool,
    /// Files only. Folders carry `has_changes` instead, except an untracked folder, which is `?`.
    pub git: Option<GitMark>,
    /// The index holds a change for this file.
    pub staged: bool,
    /// A file with a git mark, or a folder holding one.
    pub has_changes: bool,
    pub changed_by: Option<ChangedBy>,
    /// A workspace artifact the user hid from every agent (HANDOVER 6.5). False for project files.
    #[serde(default)]
    pub hidden_from_agents: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectTree {
    pub project: String,
    /// The listed folder, project-relative; empty for the project root.
    pub path: String,
    /// Depth-first: each folder is followed by its entries when `depth` reaches them.
    pub entries: Vec<ProjectTreeEntry>,
    pub is_git: bool,
    /// The entry cap was reached.
    pub truncated: bool,
}

/// `GET /api/workspaces/:ws/artifacts`: one workspace artifact (HANDOVER 6.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceArtifact {
    /// `/`-separated path inside the artifacts folder. Tag it as `@_artifacts/<path>`.
    pub path: String,
    #[ts(type = "number")]
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
    /// Hidden from every agent, tag picker, and instruction (Rule W2).
    pub hidden: bool,
}

/// `GET /api/workspaces/:ws/artifacts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceArtifacts {
    /// Absolute path of the visible artifacts folder, which agents read.
    #[ts(type = "string")]
    pub dir: PathBuf,
    pub artifacts: Vec<WorkspaceArtifact>,
    /// Why deleting is refused right now, such as a session that has not ended (Rule W4).
    pub delete_blocked: Option<String>,
}

/// `POST /api/workspaces/:ws/artifacts/move`: move an artifact file or folder to a new path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MoveArtifact {
    pub from: String,
    /// The full new path, such as `guides/style.md` for `style.md` dropped on `guides`.
    pub to: String,
}

/// `POST /api/workspaces/:ws/artifacts/hidden`: hide or unhide one artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetArtifactHidden {
    pub path: String,
    pub hidden: bool,
}

/// `GET /api/workspaces/:ws/projects/:key/file?path=`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectFile {
    pub path: String,
    /// UTF-8 text (invalid sequences replaced), or null for a binary file.
    pub content: Option<String>,
    pub binary: bool,
    pub size: u64,
    /// The text was cut at the size cap.
    pub truncated: bool,
    pub modified: Option<DateTime<Utc>>,
    pub git: Option<GitMark>,
    pub staged: bool,
    pub changed_by: Option<ChangedBy>,
    /// SHA-256 of the bytes on disk, hex. Send it back as `base_hash` when saving. Null when the
    /// file cannot be edited in the browser: binary, cut at the size cap, or not valid UTF-8.
    pub hash: Option<String>,
    /// Why a save is refused right now, such as a running execution writing this file.
    pub read_only: Option<String>,
}

/// `PUT /api/workspaces/:ws/projects/:key/file`: write one project file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SaveProjectFile {
    pub path: String,
    pub content: String,
    /// The `hash` the edit started from. Null creates a file that must not exist yet. A file
    /// that changed since answers 409 with an issue on `base_hash`.
    pub base_hash: Option<String>,
}

/// `POST /api/workspaces/:ws/projects/:key/mkdir`: create a folder and its missing parents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateProjectFolder {
    pub path: String,
}

/// Every non-ignored file of a project, for "Find a file" and search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileIndex {
    /// Project-relative, `/`-separated, sorted.
    pub paths: Vec<String>,
    /// The path cap was reached.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffLine {
    #[serde(rename = "type")]
    pub kind: DiffLineKind,
    /// The line without its `+`, `-`, or space prefix and without the newline.
    pub text: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Text after the closing `@@`, usually the enclosing function.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// `GET /api/workspaces/:ws/projects/:key/diff?path=&base=HEAD`: the working tree against `base`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileDiff {
    pub path: String,
    pub base: String,
    pub hunks: Vec<DiffHunk>,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
    /// The diff was cut at the size cap.
    pub truncated: bool,
    pub git: Option<GitMark>,
    pub changed_by: Option<ChangedBy>,
}

/// One row of `GET /api/workspaces/:ws/projects/:key/changes`: a file a session changed that
/// still differs from HEAD (every attributed file when the project is not a git repository).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectChange {
    pub path: String,
    pub git: Option<GitMark>,
    pub staged: bool,
    pub added: u32,
    pub removed: u32,
    pub changed_by: ChangedBy,
}

// ---------------------------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------------------------

/// Channels: `session:<id>`, `execution:<id>`, `term:<execution>`, `workspace:<id>`, `home`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ClientMsg {
    Subscribe {
        channel: String,
    },
    Unsubscribe {
        channel: String,
    },
    TermInput {
        execution: ExecutionId,
        data: String,
    },
    TermResize {
        execution: ExecutionId,
        cols: u16,
        rows: u16,
    },
    /// Completions at `line`/`col` of the unsaved `text` of project file `path`. Answered with
    /// `code_completion` carrying the same `id`. A newer request from the same socket cancels
    /// this one. Lines are 1-based; columns are 0-based UTF-16 code units.
    CodeComplete {
        id: u32,
        workspace: WorkspaceId,
        key: String,
        path: String,
        text: String,
        line: u32,
        col: u32,
        /// The typed character that opened the list, when one did.
        #[serde(default)]
        trigger: Option<String>,
        /// The previous list was incomplete and the word under the cursor grew.
        #[serde(default)]
        retrigger: bool,
    },
    /// Signature help at `line`/`col`, answered with `code_signature_help`. Same rules as
    /// `code_complete`.
    CodeSignature {
        id: u32,
        workspace: WorkspaceId,
        key: String,
        path: String,
        text: String,
        line: u32,
        col: u32,
        #[serde(default)]
        trigger: Option<String>,
        /// Signature help is already showing.
        #[serde(default)]
        retrigger: bool,
    },
    /// The supertypes or implementations of the name at `line`/`col`, answered with
    /// `code_navigation`. Same rules as `code_complete`.
    CodeNavigate {
        id: u32,
        workspace: WorkspaceId,
        key: String,
        path: String,
        text: String,
        line: u32,
        col: u32,
        target: crate::code::NavigateTarget,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ServerMsg {
    /// An engine event appended to a session.
    SessionEvent {
        session: SessionId,
        seq: i64,
        at: DateTime<Utc>,
        event: SessionEvent,
    },
    /// Streamed activity of one execution.
    ExecutionDelta {
        execution: ExecutionId,
        seq: i64,
        at: DateTime<Utc>,
        delta: ExecutionDelta,
    },
    ExecutionStatus {
        execution: ExecutionId,
        status: ExecutionStatus,
    },
    SessionUpdated {
        summary: SessionSummary,
    },
    WorkspaceUpdated {
        workspace: WorkspaceId,
    },
    HarnessStatus {
        statuses: Vec<HarnessStatus>,
    },
    Subscribed {
        channel: String,
    },
    Error {
        message: String,
    },
    /// Files an execution wrote in a project, on `workspace:<id>`. Paths are project-relative;
    /// changes are coalesced over a short window.
    ProjectFsChanged {
        workspace: WorkspaceId,
        key: String,
        paths: Vec<String>,
    },
    /// A line of git progress for a clone into project `key`, on `workspace:<id>`. At most four
    /// per second per clone.
    GitProgress {
        workspace: WorkspaceId,
        key: String,
        line: String,
    },
    /// A session node of the Sessions tree changed or appeared, on `workspace:<id>`. Replace the
    /// node with the same id. At most four per second per session.
    TreePatch {
        workspace: WorkspaceId,
        session: TreeSession,
    },
    /// The answer to `code_complete`. `result` is null when nothing answered; `error` says why
    /// the request could not be asked.
    CodeCompletion {
        id: u32,
        result: Option<crate::code::CodeCompletion>,
        error: Option<String>,
    },
    /// The answer to `code_signature`. `result` is null outside a call.
    CodeSignatureHelp {
        id: u32,
        result: Option<crate::code::CodeSignatureHelp>,
        error: Option<String>,
    },
    /// The answer to `code_navigate`. `result` is null when neither the code index nor a
    /// language server knows the name at the cursor.
    CodeNavigation {
        id: u32,
        result: Option<crate::code::CodeNavigation>,
        error: Option<String>,
    },
    /// The workspace's running executions, open gates, or spend changed, on `workspace:<id>`. At
    /// most two per second.
    Activity {
        workspace: WorkspaceId,
        activity: WorkspaceActivity,
    },
}

// ---------------------------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------------------------

/// Where a skill sits in a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SkillOrigin {
    /// Under `.agents/skills/` or the older `.ostra/skills/`, the directories every executor loads skills from.
    Ostra,
    /// In a harness's own directory, such as `.claude/skills/`. Executions do not load it until it is adopted.
    Harness,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillView {
    pub name: String,
    /// The `description` from the SKILL.md frontmatter.
    pub description: Option<String>,
    /// Relative to the project root, for example `.agents/skills/convention/SKILL.md`.
    pub path: String,
    pub origin: SkillOrigin,
    /// The `project.toml` entry. Only registered skills reach the repo brief.
    pub entry: Option<crate::config::SkillEntry>,
    /// False when `project.toml` names a SKILL.md that is not on disk.
    pub exists: bool,
}

/// `GET /api/workspaces/:ws/skills`: every project's skills.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectSkills {
    pub project: String,
    #[ts(type = "string")]
    pub path: PathBuf,
    /// Set when the skills cannot be read or changed, such as a missing directory or a running init.
    pub blocked: Option<String>,
    pub skills: Vec<SkillView>,
}

/// `GET /api/workspaces/:ws/projects/:key/skills/:name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillDoc {
    pub skill: SkillView,
    pub content: String,
}

/// `PUT /api/workspaces/:ws/projects/:key/skills/:name`: write the skill's SKILL.md, under
/// `.agents/skills/<name>/` unless it already lives under `.ostra/skills/<name>/`, and register it in
/// `project.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillSave {
    /// `convention`, `creation`, `test`, or `other`.
    pub kind: String,
    pub component_type: Option<String>,
    pub content: String,
}

/// `POST /api/workspaces/:ws/projects/:key/skills/:name/adopt`: copy a harness skill directory into
/// `.agents/skills/<name>/` and register it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillAdopt {
    /// The harness skill's SKILL.md, relative to the project root.
    pub from: String,
    pub kind: String,
    pub component_type: Option<String>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn export_bindings() {
        // ts-rs writes every `#[ts(export)]` type when its generated tests run; this test exists so
        // `cargo test -p ostra-core export_bindings` is a stable command to regenerate them.
    }

    use super::*;
    use crate::config::{GlobalConfig, RouteQuery, resolve_executor, resolve_route};
    use crate::model::{Complexity, Tier};

    #[test]
    fn routing_presets_move_only_implementers() {
        let mut ws = WorkspaceSettings::seeded("x");
        ws.routing
            .executor
            .by_agent
            .insert("plan".into(), ExecutorKind::Harness(HarnessKind::Grok));
        RoutingPreset::Codex.apply(&mut ws.routing);
        let codex = ExecutorKind::Harness(HarnessKind::Codex);
        for c in Complexity::ALL {
            assert_eq!(resolve_executor(&ws, "implementer", Some(c)), codex);
            assert_eq!(resolve_executor(&ws, "write-test", Some(c)), codex);
        }
        assert_eq!(resolve_executor(&ws, "plan", None), ExecutorKind::Native);
        assert_eq!(
            resolve_executor(&ws, "code-reviewer", None),
            ExecutorKind::Native
        );
        let route = resolve_route(
            &GlobalConfig::default(),
            &ws,
            RouteQuery::new("implementer", Tier::Balanced),
        )
        .unwrap();
        assert_eq!(route.model, "gpt-5.6-luna");

        RoutingPreset::Claude.apply(&mut ws.routing);
        assert_eq!(
            resolve_executor(&ws, "implementer", None),
            ExecutorKind::Harness(HarnessKind::Claude)
        );
        assert_eq!(ws.routing.executor.by_agent.len(), 2);

        RoutingPreset::Native.apply(&mut ws.routing);
        assert!(ws.routing.executor.by_agent.is_empty());
        assert_eq!(
            ws.routing.model,
            WorkspaceSettings::seeded("x").routing.model
        );
    }

    #[test]
    fn agent_labels_and_gate_executions() {
        assert_eq!(AgentName::CodeReviewer.label(), "Code reviewer");
        assert_eq!(AgentName::Explore.label(), "Explore");
        let x = ExecutionId::from("x_1");
        let failed = GatePayload::ExecutionFailed {
            execution: x.clone(),
            agent: AgentName::Plan,
            project: "a".into(),
            error: "e".into(),
        };
        assert_eq!(failed.execution(), Some(&x));
        assert_eq!(
            GatePayload::BudgetReached {
                spent_usd: 1.0,
                budget_usd: 1.0
            }
            .execution(),
            None
        );
    }

    #[test]
    fn create_body_accepts_the_old_shape() {
        let body: CreateWorkspace =
            serde_json::from_str(r#"{"name": "a", "root": "/tmp/a"}"#).unwrap();
        assert_eq!(body.projects, None);
        assert_eq!(body.routing_preset, None);
        let full: CreateWorkspace = serde_json::from_str(
            r#"{"name": "a", "root": "/tmp/a", "projects": [{"path": "/x", "key": "x"}], "permissions": {"mode": "plan"},
               "yolo": {"default": true}, "routing_preset": "codex", "notifications": {"push": false}}"#,
        )
        .unwrap();
        assert_eq!(full.projects.unwrap()[0].stack, None);
        assert_eq!(full.permissions.unwrap().mode.as_deref(), Some("plan"));
    }

    #[test]
    fn summary_lines() {
        let edit = ToolCall::new(
            "Edit",
            serde_json::json!({"file_path": "/code/app/src/orders/state.rs", "old_string": "a"}),
        );
        assert_eq!(
            tool_summary(&edit, Some(std::path::Path::new("/code/app"))),
            "Edit src/orders/state.rs"
        );
        assert_eq!(
            tool_summary(&edit, None),
            "Edit /code/app/src/orders/state.rs"
        );
        let bash = ToolCall::new("Bash", serde_json::json!({"command": "npm run\n  test"}));
        assert_eq!(tool_summary(&bash, None), "Bash npm run test");
        assert_eq!(
            tool_summary(&ToolCall::new("TodoWrite", serde_json::json!({})), None),
            "TodoWrite"
        );
        let long = summary_line(&"x".repeat(500));
        assert_eq!(long.chars().count(), SUMMARY_CHARS);
        assert!(long.ends_with('…'));
    }
}

// ---------------------------------------------------------------------------------------------
// External MCP servers
// ---------------------------------------------------------------------------------------------

/// `GET /api/workspaces/:id/mcp`: one row per `mcp_servers` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpServerStatus {
    pub name: String,
    /// `stdio` or `http`.
    pub transport: String,
    pub state: McpConnState,
    /// Why the server is not connected, or what to do about it.
    #[ts(optional)]
    pub message: Option<String>,
    /// The server's own name and version from `initialize`.
    #[ts(optional)]
    pub server_info: Option<String>,
    /// Whether Ostra holds OAuth tokens for this server. `None` when it has never asked for them.
    #[ts(optional)]
    pub signed_in: Option<bool>,
    pub tools: Vec<McpToolInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum McpConnState {
    Disabled,
    /// Not connected yet. Ostra connects when an execution or this page needs it.
    Idle,
    Connected,
    /// The server wants a sign-in: `POST /api/workspaces/:id/mcp/:name/login`.
    NeedsAuth,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpToolInfo {
    /// The tool's name on its server, as `disabled_tools` lists it.
    pub name: String,
    /// The name agents, the Activity view, and permission rules use: `mcp__<server>__<tool>`.
    pub canonical: String,
    pub description: String,
    pub enabled: bool,
    /// The server marks the tool as read-only (`readOnlyHint`).
    pub read_only: bool,
}

/// `POST /api/workspaces/:id/mcp/:name/login`: open this URL to sign in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpLogin {
    pub authorization_url: String,
}
