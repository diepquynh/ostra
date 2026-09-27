//! Programs Ostra starts for a project or workspace, outside any agent execution.

use crate::backend::{Decision, decide};
use crate::members::Members;
use crate::profile::Profile;
use ostra_core::config::{self, GlobalConfig, WorkspaceSandbox};
use ostra_core::paths;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// How Ostra starts a program for a project or workspace: under the sandbox when the config and
/// this machine give one, without the credential variables Ostra holds, and with git settings
/// that keep a planted `.git/config` from starting programs.
#[derive(Debug)]
pub struct HostCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env_remove: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Kills what the program leaves running once dropped. Hold it as long as the program runs.
    pub members: Option<Members>,
}

/// [`HostCommand`] for `program args` started in `cwd`, writing only under `roots`. The global
/// config is read fresh, and `ws` holds the workspace's own sandbox settings. `Err` when the mode
/// is `required` and no sandbox is available.
pub fn host_command(
    program: &str,
    args: &[String],
    cwd: &Path,
    roots: &[&Path],
    ws: &WorkspaceSandbox,
) -> Result<HostCommand, String> {
    let mut global: GlobalConfig =
        config::load_toml(&paths::global_config_path()).unwrap_or_default();
    global.sandbox = global.sandbox.for_workspace(ws);
    let mut env_remove: Vec<String> = config::credential_env_names(&global, &[])
        .into_iter()
        .collect();
    env_remove.extend(
        std::env::vars_os()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .filter(|k| k.starts_with("OSTRA_")),
    );
    let mut env = ostra_core::git::agent_env();
    let backend = match decide(&global.sandbox)? {
        Decision::Unsandboxed(_) => {
            return Ok(HostCommand {
                program: program.to_string(),
                args: args.to_vec(),
                env_remove,
                env,
                members: None,
            });
        }
        Decision::Sandboxed(b) => b,
    };
    let found = find_program(program, cwd).ok_or_else(|| {
        format!("Cannot start `{program}`: no such program in the folder or on PATH.")
    })?;
    let home = paths::home().unwrap_or_else(|| "/".into());
    let sc = Profile::for_program(roots, &global.sandbox, &home).command(
        &backend,
        cwd,
        found.as_os_str(),
        args.iter().map(OsString::from),
    )?;
    let text = |s: &OsStr| s.to_string_lossy().into_owned();
    env_remove.extend(sc.env_remove);
    env.extend(sc.env);
    Ok(HostCommand {
        program: text(sc.program.as_os_str()),
        args: sc.args.iter().map(|a| text(a)).collect(),
        env_remove,
        env,
        members: sc.members,
    })
}

/// `program` as the host would start it from `cwd`: a path with a `/`, else the first match on
/// `PATH`. Checked before the sandbox starts, so a missing program reads as missing.
fn find_program(program: &str, cwd: &Path) -> Option<PathBuf> {
    if program.contains('/') {
        let p = cwd.join(program);
        return p.is_file().then_some(p);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}
