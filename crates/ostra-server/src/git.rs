//! Clone and pull projects with git, and the git credentials saved from the browser. Credentials
//! live in the registry database next to the provider keys, reach git only through the process
//! environment and one-off `-c` options, and never land in a repository's `.git/config`.

use crate::app::{App, Pushed};
use ostra_core::api::SessionStatus;
use ostra_core::api::{
    CloneProject, GitCredentialEdit, GitCredentialKind, GitCredentialView, GitPullResult,
    ImportProject, ServerMsg,
};
use ostra_core::config::ValidationIssue;
use ostra_core::event::SessionKind;
use ostra_core::ids::SessionId;
use ostra_core::paths;
use ostra_store::{RegistryDb, StoreError};
use ostra_workspace::WorkspaceRt;
use ostra_workspace::projects::key_and_stack;
use ostra_workspace::settings::field_issue;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

const PREFIX: &str = "git_credential:";
const CLONE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const PULL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const QUICK_TIMEOUT: Duration = Duration::from_secs(30);
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
const TAIL_LINES: usize = 12;
/// GitHub and GitLab accept any user name with a personal access token.
const DEFAULT_TOKEN_USER: &str = "x-access-token";

// ---------------------------------------------------------------------------------------------
// Saved credentials
// ---------------------------------------------------------------------------------------------

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedGitCredential {
    pub label: String,
    pub host: String,
    pub kind: GitCredentialKind,
    pub username: Option<String>,
    pub secret: String,
}

impl std::fmt::Debug for SavedGitCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedGitCredential")
            .field("label", &self.label)
            .field("host", &self.host)
            .field("kind", &self.kind)
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

impl SavedGitCredential {
    fn view(&self, id: &str) -> GitCredentialView {
        GitCredentialView {
            id: id.to_string(),
            label: self.label.clone(),
            host: self.host.clone(),
            kind: self.kind,
            username: self.username.clone(),
            has_secret: !self.secret.is_empty(),
        }
    }
}

pub fn load_all(registry: &RegistryDb) -> Result<Vec<(String, SavedGitCredential)>, StoreError> {
    let mut out = vec![];
    for (key, bytes) in registry.secret_scan(PREFIX)? {
        match serde_json::from_slice(&bytes) {
            Ok(saved) => out.push((key[PREFIX.len()..].to_string(), saved)),
            Err(e) => tracing::warn!("ignoring unreadable git credential under {key}: {e}"),
        }
    }
    Ok(out)
}

pub fn views(registry: &RegistryDb) -> Result<Vec<GitCredentialView>, StoreError> {
    Ok(load_all(registry)?
        .iter()
        .map(|(id, c)| c.view(id))
        .collect())
}

fn load(registry: &RegistryDb, id: &str) -> Result<Option<SavedGitCredential>, StoreError> {
    Ok(load_all(registry)?
        .into_iter()
        .find(|(i, _)| i == id)
        .map(|(_, c)| c))
}

fn store(registry: &RegistryDb, id: &str, saved: &SavedGitCredential) -> Result<(), StoreError> {
    registry.secret_set(
        &format!("{PREFIX}{id}"),
        &serde_json::to_vec(saved).expect("credential serializes"),
    )
}

pub fn delete(registry: &RegistryDb, id: &str) -> Result<bool, StoreError> {
    registry.kv_delete(&format!("{PREFIX}{id}"))
}

pub enum EditError {
    Invalid(Vec<ValidationIssue>),
    NotFound,
    Store(StoreError),
}

impl From<StoreError> for EditError {
    fn from(e: StoreError) -> Self {
        EditError::Store(e)
    }
}

/// Save a new credential, or apply an edit to the saved one with `id`.
pub fn save(
    registry: &RegistryDb,
    id: Option<&str>,
    edit: &GitCredentialEdit,
) -> Result<String, EditError> {
    let current = match id {
        Some(id) => Some(load(registry, id)?.ok_or(EditError::NotFound)?),
        None => None,
    };
    let saved = apply(current, edit).map_err(EditError::Invalid)?;
    let id = id
        .map(str::to_string)
        .unwrap_or_else(|| format!("gc_{}", uuid::Uuid::now_v7().simple()));
    store(registry, &id, &saved)?;
    Ok(id)
}

/// `github.com/acme/` or `https://GitHub.com/acme` becomes `github.com/acme`.
fn normalize_host(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(raw)
        .trim_matches('/');
    let raw = raw.rsplit_once('@').map(|(_, h)| h).unwrap_or(raw);
    match raw.split_once('/') {
        Some((host, path)) => format!("{}/{}", host.to_ascii_lowercase(), path),
        None => raw.to_ascii_lowercase(),
    }
}

fn host_ok(host: &str) -> bool {
    let (name, path) = host.split_once('/').unwrap_or((host, ""));
    let (name, port) = name.split_once(':').unwrap_or((name, ""));
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && port.chars().all(|c| c.is_ascii_digit())
        && path.split('/').all(|seg| {
            path.is_empty()
                || (!seg.is_empty()
                    && seg
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._~-".contains(c)))
        })
}

fn apply(
    current: Option<SavedGitCredential>,
    edit: &GitCredentialEdit,
) -> Result<SavedGitCredential, Vec<ValidationIssue>> {
    let mut issues = vec![];
    let kind = match edit.kind.as_deref().map(str::trim) {
        Some("https") => Some(GitCredentialKind::Https),
        Some("ssh") => Some(GitCredentialKind::Ssh),
        Some(other) => {
            issues.push(field_issue(
                "kind",
                format!("`{other}` is not a credential type. Choose https or ssh."),
            ));
            None
        }
        None => current.as_ref().map(|c| c.kind),
    };
    let host = edit
        .host
        .as_deref()
        .map(normalize_host)
        .or_else(|| current.as_ref().map(|c| c.host.clone()))
        .unwrap_or_default();
    if host.is_empty() {
        issues.push(field_issue(
            "host",
            "Enter the host the credential is for, for example github.com.".into(),
        ));
    } else if !host_ok(&host) {
        issues.push(field_issue("host", format!("`{host}` is not a host. Enter a host name with an optional path prefix, for example github.com or github.com/acme.")));
    }
    let username = match &edit.username {
        Some(u) => Some(u.trim()).filter(|u| !u.is_empty()).map(str::to_string),
        None => current.as_ref().and_then(|c| c.username.clone()),
    };
    if username
        .as_deref()
        .is_some_and(|u| u.chars().any(|c| c.is_whitespace() || c.is_control()))
    {
        issues.push(field_issue(
            "username",
            "Enter the user name without spaces.".into(),
        ));
    }
    let secret = match edit.secret.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => current
            .as_ref()
            .map(|c| c.secret.clone())
            .unwrap_or_default(),
    };
    let fresh_secret = edit.secret.as_deref().is_some_and(|s| !s.trim().is_empty());
    match kind {
        None if !issues.iter().any(|i| i.path == "kind") => issues.push(field_issue(
            "kind",
            "Choose a credential type: https or ssh.".into(),
        )),
        _ if secret.is_empty() => issues.push(field_issue(
            "secret",
            "Paste the token or the private key.".into(),
        )),
        Some(GitCredentialKind::Https)
            if secret.chars().any(|c| c.is_whitespace() || c.is_control()) =>
        {
            issues.push(field_issue(
                "secret",
                "Paste the token without spaces or line breaks.".into(),
            ))
        }
        Some(GitCredentialKind::Ssh) if !secret.contains("PRIVATE KEY-----") => {
            issues.push(field_issue("secret", "Paste the whole private key, from its -----BEGIN line to its -----END line. A public key does not work.".into()))
        }
        Some(GitCredentialKind::Ssh) if fresh_secret && secret.contains("ENCRYPTED") => {
            issues.push(field_issue("secret", "Paste a key without a passphrase, because Ostra runs git with no terminal to ask for one.".into()))
        }
        _ => {}
    }
    let label = edit
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .or_else(|| current.as_ref().map(|c| c.label.clone()))
        .unwrap_or_else(|| host.clone());
    match kind {
        Some(kind) if issues.is_empty() => Ok(SavedGitCredential {
            label,
            host,
            kind,
            username,
            secret,
        }),
        _ => Err(issues),
    }
}

// ---------------------------------------------------------------------------------------------
// Remotes
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Remote {
    pub ssh: bool,
    /// `http://`, where a saved token would cross the network unencrypted.
    pub plain_http: bool,
    /// Host, `:port` when one is given, and the repository path without `.git`.
    pub key: String,
}

/// A remote URL Ostra agrees to clone: `https://`, `http://`, `ssh://`, or `user@host:path`.
/// Local paths, `file://`, and `ext::` are refused, because they read or run things on this
/// machine.
pub fn parse_remote(raw: &str) -> Result<Remote, String> {
    let raw = raw.trim();
    const USE: &str = "Use an https:// or ssh:// URL, or git@host:owner/repo.git.";
    if raw.is_empty() {
        return Err(format!("Enter the repository URL. {USE}"));
    }
    if raw.starts_with('-') || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("`{raw}` is not a repository URL. {USE}"));
    }
    let tidy = |host: &str, path: &str| {
        let path = path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        format!("{}/{}", host.to_ascii_lowercase(), path)
    };
    if raw.contains("://") {
        let url = url::Url::parse(raw).map_err(|_| format!("`{raw}` is not a URL. {USE}"))?;
        let ssh = match url.scheme() {
            "https" | "http" => false,
            "ssh" => true,
            other => return Err(format!("Ostra does not clone {other}:// URLs. {USE}")),
        };
        let Some(host) = url.host_str() else {
            return Err(format!("`{raw}` names no host. {USE}"));
        };
        if !ssh && url.password().is_some() {
            return Err("Remove the user name and token from the URL and save them as a git credential, because git keeps the URL in the project's .git/config.".into());
        }
        let host = match url.port() {
            Some(p) => format!("{host}:{p}"),
            None => host.to_string(),
        };
        return Ok(Remote {
            ssh,
            // Loopback traffic never crosses the network.
            plain_http: url.scheme() == "http"
                && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
            key: tidy(&host, url.path()),
        });
    }
    // scp-like syntax; git reads a colon before any slash this way.
    if let Some((left, path)) = raw.split_once(':')
        && !left.contains('/')
        && !path.is_empty()
        && !raw.contains("::")
    {
        let host = left.rsplit_once('@').map(|(_, h)| h).unwrap_or(left);
        if !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        {
            return Ok(Remote {
                ssh: true,
                plain_http: false,
                key: tidy(host, path),
            });
        }
    }
    Err(format!("`{raw}` is not a remote repository URL. {USE}"))
}

/// A credential host without a port matches the host on any port.
fn matches(remote: &Remote, c: &SavedGitCredential) -> bool {
    let (host, path) = remote.key.split_once('/').unwrap_or((&remote.key, ""));
    let c_name = c.host.split('/').next().unwrap_or_default();
    let key = match host.split_once(':') {
        Some((name, _)) if !c_name.contains(':') => format!("{name}/{path}"),
        _ => remote.key.clone(),
    };
    !remote.plain_http
        && (remote.ssh == (c.kind == GitCredentialKind::Ssh))
        && (key == c.host || key.starts_with(&format!("{}/", c.host)))
}

/// The saved credential for a remote: the matching one with the longest host and path prefix.
pub fn credential_for(
    creds: &[(String, SavedGitCredential)],
    remote: &Remote,
) -> Option<SavedGitCredential> {
    creds
        .iter()
        .filter(|(_, c)| matches(remote, c))
        .max_by_key(|(_, c)| c.host.len())
        .map(|(_, c)| c.clone())
}

// ---------------------------------------------------------------------------------------------
// Running git
// ---------------------------------------------------------------------------------------------

/// How one git process authenticates. Holds the private key file until the process ends.
pub struct GitAuth {
    args: Vec<String>,
    envs: Vec<(String, String)>,
    secret: Option<String>,
    _dir: Option<tempfile::TempDir>,
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

impl GitAuth {
    pub fn new(cred: Option<&SavedGitCredential>) -> std::io::Result<Self> {
        // Git has no terminal to ask on, so every prompt fails instead of waiting forever.
        let mut envs = vec![("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())];
        // Git for Windows' Credential Manager asks in a window, not the terminal, so a server
        // with no one at the desktop would wait on it forever. Stored credentials still answer.
        if cfg!(windows) {
            envs.push(("GCM_INTERACTIVE".to_string(), "never".to_string()));
        }
        let mut args = vec![];
        let mut dir = None;
        let ssh_base = "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new";
        match cred {
            Some(c) if c.kind == GitCredentialKind::Https => {
                // The empty helper drops the user's own helpers, so git does not copy the token
                // into them after a successful fetch.
                args.push("-c".into());
                args.push("credential.helper=".into());
                args.push("-c".into());
                // Answers only for the credential's own host, so a remote rewritten by the
                // repository's config (`url.*.insteadOf`) is never handed the token.
                args.push(
                    r#"credential.helper=!f() { test "$1" = get || exit 0; h=; while IFS= read -r l; do case "$l" in host=*) h="${l#host=}";; esac; done; test "$h" = "$OSTRA_GIT_HOST" || test "${h%%:*}" = "$OSTRA_GIT_HOST" || exit 0; echo "username=$OSTRA_GIT_USERNAME"; echo "password=$OSTRA_GIT_PASSWORD"; }; f"#.into(),
                );
                // The token goes straight to the remote over verified TLS, whatever proxy or
                // certificate settings the repository's config names.
                for pin in ["http.proxy=", "http.sslVerify=true"] {
                    args.push("-c".into());
                    args.push(pin.into());
                }
                envs.push((
                    "OSTRA_GIT_HOST".into(),
                    c.host
                        .split('/')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                ));
                envs.push((
                    "OSTRA_GIT_USERNAME".into(),
                    c.username
                        .clone()
                        .unwrap_or_else(|| DEFAULT_TOKEN_USER.into()),
                ));
                envs.push(("OSTRA_GIT_PASSWORD".into(), c.secret.clone()));
            }
            Some(c) => {
                // In the owner-only data dir, which the sandbox hides, not in the shared temp dir.
                let private = ostra_core::paths::ensure_data_dir()?.join("tmp");
                std::fs::create_dir_all(&private)?;
                let tmp = tempfile::Builder::new()
                    .prefix("ostra-git-")
                    .tempdir_in(&private)?;
                let key = tmp.path().join("id");
                write_private(&key, &format!("{}\n", c.secret.trim_end()))?;
                envs.push((
                    "GIT_SSH_COMMAND".into(),
                    format!(
                        "{ssh_base} -o IdentitiesOnly=yes -i {}",
                        shell_quote(&key.to_string_lossy())
                    ),
                ));
                dir = Some(tmp);
            }
            None if std::env::var_os("GIT_SSH_COMMAND").is_none() => {
                envs.push(("GIT_SSH_COMMAND".into(), ssh_base.into()));
            }
            None => {}
        }
        Ok(GitAuth {
            args,
            envs,
            secret: cred.map(|c| c.secret.clone()),
            _dir: dir,
        })
    }

    fn redact(&self, text: String) -> String {
        match &self.secret {
            Some(s) if s.len() >= 4 && text.contains(s.as_str()) => text.replace(s.as_str(), "***"),
            _ => text,
        }
    }
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(text.as_bytes())
}

/// Splits git's output into lines. `\r` ends a progress update that the next one overwrites, so
/// it reaches the progress callback but not the kept tail.
struct Lines {
    partial: Vec<u8>,
    tail: VecDeque<String>,
}

impl Lines {
    fn new() -> Self {
        Lines {
            partial: vec![],
            tail: VecDeque::new(),
        }
    }

    fn feed(&mut self, bytes: &[u8], on_line: &mut impl FnMut(&str)) {
        for &b in bytes {
            if b == b'\n' || b == b'\r' {
                let line = String::from_utf8_lossy(&self.partial).trim().to_string();
                self.partial.clear();
                if line.is_empty() {
                    continue;
                }
                on_line(&line);
                if b == b'\n' {
                    self.keep(line);
                }
            } else {
                self.partial.push(b);
            }
        }
    }

    fn keep(&mut self, line: String) {
        if self.tail.len() == TAIL_LINES {
            self.tail.pop_front();
        }
        self.tail.push_back(line);
    }

    fn finish(mut self) -> String {
        let rest = String::from_utf8_lossy(&self.partial).trim().to_string();
        if !rest.is_empty() {
            self.keep(rest);
        }
        Vec::from(self.tail).join("\n")
    }
}

/// Run git and return its last output lines. The error is the text to show: git's own last lines,
/// or why git could not run.
pub async fn run_git(
    cwd: Option<&Path>,
    auth: &GitAuth,
    args: &[&str],
    timeout: Duration,
    mut on_line: impl FnMut(&str),
) -> Result<String, String> {
    if let Some(dir) = cwd {
        // A planted `commondir` would hand this command another repository's config, whose
        // credential helper or SSH command a push or pull would run.
        for p in ostra_core::sandbox::repair_git_dirs(&ostra_core::sandbox::git_repos(&[dir])) {
            tracing::warn!("removed a planted {} before running git", p.display());
        }
    }
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(ostra_core::git::NO_EXEC);
    if let Some(dir) = cwd {
        cmd.args(ostra_core::git::submodule_filter_overrides(dir).await)
            .arg("-C")
            .arg(dir);
    }
    cmd.args(&auth.args)
        .args(args)
        .envs(auth.envs.iter().map(|(k, v)| (k, v)))
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            "Install git on the machine that runs Ostra, because cloning and pulling run the git command.".to_string()
        }
        _ => format!("Could not start git: {e}"),
    })?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let mut lines = Lines::new();
    let work = async {
        let mut out_buf = [0u8; 4096];
        let mut err_buf = [0u8; 4096];
        let (mut out_open, mut err_open) = (true, true);
        while out_open || err_open {
            tokio::select! {
                n = stdout.read(&mut out_buf), if out_open => match n {
                    Ok(0) | Err(_) => out_open = false,
                    Ok(n) => lines.feed(&out_buf[..n], &mut on_line),
                },
                n = stderr.read(&mut err_buf), if err_open => match n {
                    Ok(0) | Err(_) => err_open = false,
                    Ok(n) => lines.feed(&err_buf[..n], &mut on_line),
                },
            }
        }
        child.wait().await
    };
    let status = match tokio::time::timeout(timeout, work).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(format!("git failed: {e}")),
        Err(_) => {
            return Err(format!(
                "git did not finish within {} minutes and was stopped.",
                timeout.as_secs() / 60
            ));
        }
    };
    let text = auth.redact(lines.finish());
    if status.success() {
        Ok(text)
    } else if text.is_empty() {
        Err(format!("git exited with {status}."))
    } else {
        Err(text)
    }
}

pub(crate) async fn git_quiet(dir: &Path, args: &[&str]) -> Option<String> {
    let auth = GitAuth::new(None).ok()?;
    run_git(Some(dir), &auth, args, QUICK_TIMEOUT, |_| {})
        .await
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------------------------
// Clone
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct ClonePlan {
    pub url: String,
    pub key: String,
    pub stack: Option<String>,
    pub dest: PathBuf,
    /// The destination existed as an empty folder, so a failure empties it instead of removing it.
    pub dest_existed: bool,
    pub branch: Option<String>,
    pub credential: Option<SavedGitCredential>,
}

pub(crate) fn branch_ok(b: &str) -> bool {
    !b.is_empty()
        && !b.starts_with('-')
        && !b.contains("..")
        && !b
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
}

/// Check a clone request, and every problem with it on its field: `url`, `key`, `stack`, `path`,
/// `branch`, or `credential`.
pub fn clone_plan(
    w: &WorkspaceRt,
    registry: &RegistryDb,
    req: &CloneProject,
) -> Result<ClonePlan, Vec<ValidationIssue>> {
    let settings = w.settings();
    let mut issues = vec![];
    let url = req.url.trim().to_string();
    let remote = parse_remote(&url)
        .map_err(|m| issues.push(field_issue("url", m)))
        .ok();
    let (key, stack) = key_and_stack(&settings, &req.key, req.stack.as_deref(), &mut issues);
    let branch = req
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string);
    if let Some(b) = branch.as_deref().filter(|b| !branch_ok(b)) {
        issues.push(field_issue(
            "branch",
            format!("`{b}` is not a branch name."),
        ));
    }
    let creds = load_all(registry).unwrap_or_default();
    let credential = match req.credential.as_deref().filter(|c| !c.is_empty()) {
        Some(id) => match creds.iter().find(|(i, _)| i == id) {
            None => {
                issues.push(field_issue(
                    "credential",
                    "That git credential no longer exists. Choose another one.".into(),
                ));
                None
            }
            Some((_, c)) => {
                if let Some(r) = &remote
                    && r.ssh != (c.kind == GitCredentialKind::Ssh)
                {
                    let want = if r.ssh { "an ssh" } else { "an https" };
                    issues.push(field_issue(
                        "credential",
                        format!("This URL needs {want} credential. Choose another one, or change the URL."),
                    ));
                }
                Some(c.clone())
            }
        },
        None => remote.as_ref().and_then(|r| credential_for(&creds, r)),
    };
    let dest = match &req.path {
        Some(p) if !p.as_os_str().is_empty() => {
            if p.is_absolute() {
                Some(paths::normalize(p))
            } else {
                issues.push(field_issue("path", "Use an absolute path.".into()));
                None
            }
        }
        _ => ostra_core::slug::is_project_key(&key).then(|| w.root.join(&key)),
    };
    let mut dest_existed = false;
    if let Some(d) = &dest {
        if d.exists() {
            dest_existed = true;
            let empty = d.is_dir()
                && std::fs::read_dir(d).is_ok_and(|mut entries| entries.next().is_none());
            if !empty {
                issues.push(field_issue("path", format!("{} already exists and is not empty. Choose another folder, or import it with Add project.", d.display())));
            }
        }
        if paths::is_inside(&w.root.join(paths::RUNTIME_DIR), d) {
            issues.push(field_issue(
                "path",
                "A project cannot live inside the workspace's .ostra directory.".into(),
            ));
        }
        if let Some(other) = settings
            .projects
            .iter()
            .find(|o| paths::is_inside(&o.path, d) || paths::is_inside(d, &o.path))
        {
            issues.push(field_issue(
                "path",
                format!(
                    "{} overlaps project `{}`. Choose another folder.",
                    d.display(),
                    other.key
                ),
            ));
        }
    }
    match dest {
        Some(dest) if issues.is_empty() => Ok(ClonePlan {
            url,
            key,
            stack,
            dest,
            dest_existed,
            branch,
            credential,
        }),
        _ => Err(issues),
    }
}

pub enum GitError {
    /// Another clone or pull already holds the key or folder.
    Busy(String),
    /// Git ran and failed, or could not run.
    Git(String),
    /// The request cannot run as asked; the text says what to change.
    Invalid(String),
    /// The checkout exists but could not be imported.
    Import(ostra_workspace::CreateError),
}

/// Keys and folders with a clone, pull, or other git command in progress, released when it ends.
pub(crate) struct Claim<'a> {
    w: &'a WorkspaceRt,
    key: String,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.w.cloning.lock().retain(|(k, _)| k != &self.key);
    }
}

pub(crate) fn claim<'a>(w: &'a WorkspaceRt, key: &str, dest: &Path) -> Result<Claim<'a>, GitError> {
    let work = w.work.try_read().ok().filter(|deleted| !**deleted);
    if work.is_none() {
        return Err(GitError::Busy(ostra_workspace::STARTING.into()));
    }
    let mut busy = w.cloning.lock();
    if let Some((k, _)) = busy
        .iter()
        .find(|(k, d)| k == key || paths::is_inside(d, dest) || paths::is_inside(dest, d))
    {
        return Err(GitError::Busy(format!(
            "Git is already running in `{k}`. Wait for it to finish, then try again."
        )));
    }
    busy.push((key.to_string(), dest.to_path_buf()));
    Ok(Claim {
        w,
        key: key.to_string(),
    })
}

fn clear_failed(dest: &Path, existed: bool) {
    if !existed {
        let _ = std::fs::remove_dir_all(dest);
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dest) {
        for e in entries.flatten() {
            let p = e.path();
            let _ = if p.is_dir() && !p.is_symlink() {
                std::fs::remove_dir_all(&p)
            } else {
                std::fs::remove_file(&p)
            };
        }
    }
}

/// Clone the repository, streaming git's progress to `workspace:<id>`, then import the checkout.
pub async fn clone(app: &App, w: &WorkspaceRt, plan: ClonePlan) -> Result<(), GitError> {
    let _claim = claim(w, &plan.key, &plan.dest)?;
    let auth = GitAuth::new(plan.credential.as_ref())
        .map_err(|e| GitError::Git(format!("Could not prepare the SSH key: {e}")))?;
    let dest = plan.dest.to_string_lossy().to_string();
    let mut args = vec!["clone", "--progress"];
    if let Some(b) = &plan.branch {
        args.extend(["--branch", b]);
    }
    args.extend(["--", &plan.url, &dest]);
    let channel = format!("workspace:{}", w.id);
    let mut last = Instant::now() - PROGRESS_EVERY;
    let result = run_git(None, &auth, &args, CLONE_TIMEOUT, |line| {
        if last.elapsed() >= PROGRESS_EVERY {
            last = Instant::now();
            let _ = app.push.send(Pushed {
                channels: vec![channel.clone()],
                msg: ServerMsg::GitProgress {
                    workspace: w.id.clone(),
                    key: plan.key.clone(),
                    line: auth.redact(line.to_string()),
                },
            });
        }
    })
    .await;
    if let Err(message) = result {
        clear_failed(&plan.dest, plan.dest_existed);
        return Err(GitError::Git(message));
    }
    w.import_project(&ImportProject {
        path: plan.dest.clone(),
        key: plan.key.clone(),
        stack: plan.stack.clone(),
    })
    .map_err(GitError::Import)?;
    workspace_updated(app, w);
    Ok(())
}

pub(crate) fn workspace_updated(app: &App, w: &WorkspaceRt) {
    let _ = app.push.send(Pushed {
        channels: vec![format!("workspace:{}", w.id), "home".into()],
        msg: ServerMsg::WorkspaceUpdated {
            workspace: w.id.clone(),
        },
    });
}

// ---------------------------------------------------------------------------------------------
// Pull
// ---------------------------------------------------------------------------------------------

/// The session working in project `key`, which every git command that changes the checkout waits
/// for.
pub fn session_in(w: &WorkspaceRt, key: &str) -> Result<Option<SessionId>, StoreError> {
    Ok(w.db.list_sessions()?.into_iter().find_map(|s| {
        (matches!(
            s.status,
            SessionStatus::Running | SessionStatus::Waiting | SessionStatus::Paused
        ) && (s.projects.iter().any(|p| p == key)
            || matches!(&s.kind, SessionKind::Init { project } if project == key)))
        .then_some(s.id)
    }))
}

/// Authentication for talking to `remote`: the saved credential that matches its fetch URL, or
/// its push URL when `push`.
pub(crate) async fn remote_auth(
    app: &App,
    dir: &Path,
    remote: &str,
    push: bool,
) -> Result<GitAuth, GitError> {
    let mut args = vec!["remote", "get-url"];
    if push {
        args.push("--push");
    }
    args.extend(["--", remote]);
    let url = git_quiet(dir, &args).await;
    let credential = match url.as_deref().map(parse_remote) {
        Some(Ok(r)) => credential_for(&load_all(&app.shared.registry).unwrap_or_default(), &r),
        _ => None,
    };
    GitAuth::new(credential.as_ref())
        .map_err(|e| GitError::Git(format!("Could not prepare the SSH key: {e}")))
}

/// Fast-forward the project's current branch from its upstream, with the saved credential that
/// matches the upstream's URL.
pub async fn pull(
    app: &App,
    w: &WorkspaceRt,
    key: &str,
    dir: &Path,
) -> Result<GitPullResult, GitError> {
    let _claim = claim(w, key, dir)?;
    let branch = git_quiet(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).await;
    let Some(branch) = branch else {
        return Err(GitError::Git(
            "The project has no branch checked out. Check out a branch, then pull again.".into(),
        ));
    };
    let remote = git_quiet(
        dir,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )
    .await
    .unwrap_or_else(|| "origin".into());
    let auth = remote_auth(app, dir, &remote, false).await?;
    let before = git_quiet(dir, &["rev-parse", "--short", "HEAD"]).await;
    let output = run_git(
        Some(dir),
        &auth,
        &["pull", "--ff-only"],
        PULL_TIMEOUT,
        |_| {},
    )
    .await
    .map_err(GitError::Git)?;
    let after = git_quiet(dir, &["rev-parse", "--short", "HEAD"]).await;
    crate::files::announce_git(app, &w.id, key);
    workspace_updated(app, w);
    Ok(GitPullResult {
        branch: Some(branch),
        updated: before != after,
        before,
        after,
        output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(host: &str, kind: GitCredentialKind) -> (String, SavedGitCredential) {
        (
            host.into(),
            SavedGitCredential {
                label: host.into(),
                host: host.into(),
                kind,
                username: None,
                secret: "s3cret-token".into(),
            },
        )
    }

    #[test]
    fn remotes_parse_to_host_and_path() {
        let r = parse_remote("https://GitHub.com/acme/shop.git").unwrap();
        assert_eq!(
            r,
            Remote {
                ssh: false,
                plain_http: false,
                key: "github.com/acme/shop".into()
            }
        );
        let r = parse_remote("git@github.com:acme/shop.git").unwrap();
        assert_eq!(
            r,
            Remote {
                ssh: true,
                plain_http: false,
                key: "github.com/acme/shop".into()
            }
        );
        let plain = parse_remote("http://github.com/acme/shop").unwrap();
        assert!(plain.plain_http);
        let https = parse_remote("https://github.com/acme/shop").unwrap();
        let cred = SavedGitCredential {
            label: "gh".into(),
            host: "github.com".into(),
            kind: GitCredentialKind::Https,
            username: None,
            secret: "t".into(),
        };
        assert!(matches(&https, &cred));
        assert!(
            !matches(&plain, &cred),
            "a token never goes over plain http"
        );
        let r = parse_remote("ssh://git@git.example:2222/team/app").unwrap();
        assert_eq!(r.key, "git.example:2222/team/app");
        assert!(r.ssh);
    }

    #[test]
    fn local_and_unsafe_remotes_are_refused() {
        for bad in [
            "",
            "/srv/repo",
            "./repo",
            "file:///srv/repo",
            "ext::sh -c touch% /tmp/x",
            "-uhttps://x",
            "https://user:token@github.com/a/b",
        ] {
            assert!(parse_remote(bad).is_err(), "{bad} should be refused");
        }
        assert!(parse_remote("https://user@github.com/a/b").is_ok());
    }

    #[test]
    fn the_longest_matching_prefix_wins() {
        let creds = vec![
            cred("github.com", GitCredentialKind::Https),
            cred("github.com/acme", GitCredentialKind::Https),
            cred("github.com", GitCredentialKind::Ssh),
        ];
        let pick = |url: &str| {
            credential_for(&creds, &parse_remote(url).unwrap()).map(|c| (c.host, c.kind))
        };
        assert_eq!(
            pick("https://github.com/acme/shop"),
            Some(("github.com/acme".into(), GitCredentialKind::Https))
        );
        assert_eq!(
            pick("https://github.com/acmeish/shop"),
            Some(("github.com".into(), GitCredentialKind::Https))
        );
        assert_eq!(
            pick("git@github.com:acme/shop.git"),
            Some(("github.com".into(), GitCredentialKind::Ssh))
        );
        assert_eq!(pick("https://gitlab.com/acme/shop"), None);
        assert_eq!(
            pick("https://github.com:8443/acme/shop").map(|c| c.0),
            Some("github.com/acme".into())
        );
        let pinned = vec![cred("git.example:8443", GitCredentialKind::Https)];
        let hit = |url: &str| credential_for(&pinned, &parse_remote(url).unwrap()).is_some();
        assert!(hit("https://git.example:8443/a/b"));
        assert!(!hit("https://git.example/a/b"));
    }

    #[test]
    fn edits_validate_and_keep_the_secret() {
        let edit = GitCredentialEdit {
            host: Some("https://GitHub.com/acme/".into()),
            kind: Some("https".into()),
            secret: Some(" ghp_abc ".into()),
            ..Default::default()
        };
        let saved = apply(None, &edit).unwrap();
        assert_eq!(saved.host, "github.com/acme");
        assert_eq!(saved.label, "github.com/acme");
        assert_eq!(saved.secret, "ghp_abc");
        let renamed = apply(
            Some(saved.clone()),
            &GitCredentialEdit {
                label: Some("Work".into()),
                secret: Some("".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(renamed.label, "Work");
        assert_eq!(renamed.secret, "ghp_abc");

        let bad = GitCredentialEdit {
            host: Some("bad host".into()),
            kind: Some("ssh".into()),
            secret: Some("ssh-ed25519 AAAA me@host".into()),
            username: Some("a b".into()),
            ..Default::default()
        };
        let paths: Vec<_> = apply(None, &bad)
            .unwrap_err()
            .into_iter()
            .map(|i| i.path)
            .collect();
        assert_eq!(paths, ["host", "username", "secret"]);
        let missing = apply(None, &GitCredentialEdit::default()).unwrap_err();
        let paths: Vec<_> = missing.into_iter().map(|i| i.path).collect();
        assert_eq!(paths, ["host", "kind"]);
    }

    #[test]
    fn credentials_round_trip_without_exposing_the_secret() {
        let reg = RegistryDb::open_in_memory().unwrap();
        let edit = GitCredentialEdit {
            host: Some("github.com".into()),
            kind: Some("https".into()),
            secret: Some("ghp_abc".into()),
            ..Default::default()
        };
        let id = save(&reg, None, &edit).ok().unwrap();
        let v = views(&reg).unwrap();
        assert_eq!(v.len(), 1);
        assert!(v[0].has_secret);
        assert!(!serde_json::to_string(&v).unwrap().contains("ghp_abc"));
        assert!(!format!("{:?}", load(&reg, &id).unwrap()).contains("ghp_abc"));
        assert!(matches!(
            save(&reg, Some("gc_missing"), &edit),
            Err(EditError::NotFound)
        ));
        assert!(delete(&reg, &id).unwrap());
        assert!(views(&reg).unwrap().is_empty());
    }

    #[test]
    fn progress_lines_reach_the_callback_but_not_the_tail() {
        let mut lines = Lines::new();
        let mut seen = vec![];
        lines.feed(
            b"Cloning into 'x'...\nReceiving objects:  50%\rReceiving objects: 100%, done.\nfatal: nope",
            &mut |l: &str| seen.push(l.to_string()),
        );
        assert_eq!(seen.len(), 3);
        assert_eq!(
            lines.finish(),
            "Cloning into 'x'...\nReceiving objects: 100%, done.\nfatal: nope"
        );
    }

    #[test]
    fn the_ssh_key_file_is_private_and_quoted() {
        let (_, mut c) = cred("github.com", GitCredentialKind::Ssh);
        c.secret =
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----".into();
        let auth = GitAuth::new(Some(&c)).unwrap();
        let cmd = &auth
            .envs
            .iter()
            .find(|(k, _)| k == "GIT_SSH_COMMAND")
            .unwrap()
            .1;
        let path = cmd.rsplit_once(" -i ").unwrap().1.trim_matches('\'');
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.ends_with("-----\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(auth.args.is_empty());
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
    }

    #[test]
    fn the_credential_helper_answers_only_its_own_host() {
        let cred = SavedGitCredential {
            label: "gh".into(),
            host: "github.com/acme".into(),
            kind: GitCredentialKind::Https,
            username: Some("u".into()),
            secret: "tok".into(),
        };
        let auth = GitAuth::new(Some(&cred)).unwrap();
        let helper = auth
            .args
            .iter()
            .find_map(|a| a.strip_prefix("credential.helper=!"))
            .unwrap()
            .to_string();
        let ask = |input: &str| {
            use std::io::Write;
            let mut child = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("{helper} get"))
                .envs(auth.envs.iter().map(|(k, v)| (k, v)))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
        };
        assert!(ask("protocol=https\nhost=github.com\n\n").contains("password=tok"));
        assert!(ask("protocol=https\nhost=github.com:443\n\n").contains("password=tok"));
        assert_eq!(ask("protocol=https\nhost=evil.example\n\n"), "");
    }

    #[tokio::test]
    async fn run_git_clones_and_fast_forwards() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        let work = tmp.path().join("work");
        std::fs::create_dir(&origin).unwrap();
        git(&origin, &["init", "-q"]);
        std::fs::write(origin.join("a.txt"), "1").unwrap();
        git(&origin, &["add", "."]);
        git(&origin, &["commit", "-qm", "one"]);

        let auth = GitAuth::new(None).unwrap();
        let (src, dst) = (origin.to_string_lossy(), work.to_string_lossy());
        let mut progress = 0;
        run_git(
            None,
            &auth,
            &["clone", "--progress", "--", &src, &dst],
            CLONE_TIMEOUT,
            |_| progress += 1,
        )
        .await
        .unwrap();
        assert!(progress > 0);
        assert!(work.join("a.txt").exists());

        std::fs::write(origin.join("b.txt"), "2").unwrap();
        git(&origin, &["add", "."]);
        git(&origin, &["commit", "-qm", "two"]);
        run_git(
            Some(&work),
            &auth,
            &["pull", "--ff-only"],
            PULL_TIMEOUT,
            |_| {},
        )
        .await
        .unwrap();
        assert!(work.join("b.txt").exists());

        let err = run_git(
            Some(&work),
            &auth,
            &["no-such-command"],
            QUICK_TIMEOUT,
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(err.contains("no-such-command"), "{err}");
    }
}
