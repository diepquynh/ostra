//! Machine-level harness setup: detecting installed CLIs and their login state, and the one
//! global integration Antigravity needs.

use crate::launch::agy_hook_command;
use crate::protocol::{HookEvent, MCP_SERVER_NAME};
use ostra_core::agent::Capability;
use ostra_core::api::HarnessStatus;
use ostra_core::config::GlobalConfig;
use ostra_core::exec::ExecutionSpec;
use ostra_core::{AgentName, HarnessKind};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

async fn run(cmd: &str, args: &[&str]) -> Option<(i32, String)> {
    let child = tokio::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(15), child)
        .await
        .ok()?
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some((out.status.code().unwrap_or(-1), text))
}

/// Installed, version, and logged-in state of one harness CLI.
pub async fn harness_status(
    global: &GlobalConfig,
    harness: HarnessKind,
    home: &Path,
) -> HarnessStatus {
    let command = global.harness_command(harness);
    let installed = which::which(&command).is_ok();
    let mut status = HarnessStatus {
        harness,
        command: command.clone(),
        installed,
        version: None,
        logged_in: None,
    };
    if !installed {
        return status;
    }
    status.version = run(&command, &["--version"]).await.and_then(|(_, t)| {
        t.lines()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
    });
    status.logged_in = match harness {
        HarnessKind::Claude => run(&command, &["auth", "status"])
            .await
            .and_then(|(_, t)| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get("loggedIn").and_then(Value::as_bool)),
        HarnessKind::Codex => run(&command, &["login", "status"])
            .await
            .map(|(code, t)| code == 0 && t.contains("Logged in")),
        HarnessKind::Grok => Some(
            crate::launch::grok_home_real(home)
                .join("auth.json")
                .exists(),
        ),
        HarnessKind::Agy => Some(
            home.join(".gemini/antigravity-cli/antigravity-oauth-token")
                .exists()
                || home.join(".gemini/oauth_creds.json").exists(),
        ),
    };
    status
}

pub async fn all_harness_status(global: &GlobalConfig, home: &Path) -> Vec<HarnessStatus> {
    let mut out = vec![];
    for h in HarnessKind::ALL {
        out.push(harness_status(global, h, home).await);
    }
    out
}

/// The command a user runs in the Terminal view to log a harness in.
pub fn login_command(global: &GlobalConfig, harness: HarnessKind) -> (String, Vec<String>) {
    let cmd = global.harness_command(harness);
    let args = match harness {
        HarnessKind::Claude => vec!["auth".into(), "login".into()],
        HarnessKind::Codex => vec!["login".into()],
        HarnessKind::Grok => vec!["login".into()],
        HarnessKind::Agy => vec![],
    };
    (cmd, args)
}

// ---------------------------------------------------------------------------------------------
// Antigravity global integration
// ---------------------------------------------------------------------------------------------

fn agy_config_dir(home: &Path) -> PathBuf {
    home.join(".gemini").join("config")
}

pub fn agy_plugin_dir(home: &Path) -> PathBuf {
    agy_config_dir(home).join("plugins").join("ostra")
}

pub fn agy_agent_name(agent: AgentName) -> String {
    format!("ostra-{}", agent.as_str())
}

pub fn agy_agent_file(home: &Path, agent: AgentName) -> PathBuf {
    agy_plugin_dir(home)
        .join("agents")
        .join(format!("{}.md", agy_agent_name(agent)))
}

/// Agent definition Antigravity loads for `--agent ostra-<name>`: the rendered system prompt as
/// its instructions, inheriting the session's MCP registry so the `ostra` tools are reachable.
pub fn agy_agent_markdown(spec: &ExecutionSpec) -> String {
    let desc = format!("Ostra {} agent. Started by Ostra only.", spec.agent);
    let tools: String = agy_tools(&spec.capabilities)
        .iter()
        .map(|t| format!("    - {t}\n"))
        .collect();
    format!(
        "---\nname: {}\ndescription: {}\nmodel: {}\neffort: {}\ntools:\n{tools}timeout: {}\nsubagent: false\nhidden: true\ninheritMcp: true\n---\n\n{}\n",
        agy_agent_name(spec.agent),
        yaml_scalar(&desc),
        yaml_scalar(&spec.route.model),
        spec.effort
            .as_str()
            .replace("xhigh", "high")
            .replace("max", "high"),
        spec.timeout_secs,
        spec.system_prompt
    )
}

/// Antigravity's native tools for Ostra capabilities. An agent file with no `tools:` list gets no
/// native tools at all, and one unknown name makes the whole session fail to start (measured on
/// agy 1.2.8, where `command_status` is unknown), so only the names in Ultracode's verified tool
/// mapping are listed.
pub fn agy_tools(caps: &[Capability]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = vec![];
    let mut add = |names: &[&'static str]| {
        for n in names {
            if !out.contains(n) {
                out.push(n);
            }
        }
    };
    for c in caps {
        match c {
            Capability::Read | Capability::Skill => add(&["view_file"]),
            Capability::Write => add(&["write_to_file"]),
            Capability::Edit => add(&["replace_file_content"]),
            Capability::Shell => add(&["run_command"]),
            Capability::SearchText => add(&["grep_search"]),
            Capability::Glob => add(&["find_by_name"]),
            Capability::WebSearch => add(&["search_web"]),
            Capability::WebFetch => add(&["read_url_content"]),
            Capability::Report | Capability::Document | Capability::Memory | Capability::MemoryRecall => {}
        }
    }
    out
}

fn yaml_scalar(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// The hooks file of the Ostra plugin. Every command is inert unless `$OSTRA_EXECUTION` is set.
pub fn agy_hooks_json(ostra_binary: &Path) -> Value {
    let tool = |e: HookEvent| json!([{"matcher": ".*", "hooks": [{"type": "command", "command": agy_hook_command(ostra_binary, e)}]}]);
    let plain =
        |e: HookEvent| json!([{"type": "command", "command": agy_hook_command(ostra_binary, e)}]);
    json!({
        "ostra": {
            "PreToolUse": tool(HookEvent::PreToolUse),
            "PostToolUse": tool(HookEvent::PostToolUse),
            "PreInvocation": plain(HookEvent::PreInvocation),
            "PostInvocation": plain(HookEvent::PostInvocation),
        }
    })
}

/// Install or refresh the Antigravity integration: the `ostra` plugin (hooks and agent files),
/// its enable flag, and the `ostra` MCP server registered through `agy mcp add`, which is the
/// one registration Antigravity honors. Idempotent.
pub async fn ensure_agy_integration(
    global: &GlobalConfig,
    ostra_binary: &Path,
    home: &Path,
) -> Result<(), String> {
    let dir = agy_plugin_dir(home);
    std::fs::create_dir_all(dir.join("agents"))
        .map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let plugin = json!({
        "name": "ostra",
        "displayName": "Ostra",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Routes tool calls of Ostra-started sessions through Ostra's guards. Inert outside Ostra.",
    });
    let write = |p: PathBuf, v: &Value| {
        std::fs::write(&p, serde_json::to_string_pretty(v).unwrap_or_default())
            .map_err(|e| format!("writing {}: {e}", p.display()))
    };
    write(dir.join("plugin.json"), &plugin)?;
    write(dir.join("hooks.json"), &agy_hooks_json(ostra_binary))?;

    let config_path = agy_config_dir(home).join("config.json");
    let mut config: Value = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    if !config.is_object() {
        config = json!({});
    }
    let plugins = config
        .as_object_mut()
        .expect("object")
        .entry("plugins")
        .or_insert_with(|| json!({}));
    if let Some(p) = plugins.as_object_mut() {
        p.insert("ostra".into(), json!({"enabled": true}));
    }
    write(config_path, &config)?;

    let command = global.harness_command(HarnessKind::Agy);
    let listed = run(&command, &["mcp", "list"])
        .await
        .map(|(_, t)| t)
        .unwrap_or_default();
    let bin = ostra_binary.to_string_lossy().to_string();
    let registered = listed
        .lines()
        .any(|l| l.split_whitespace().next() == Some(MCP_SERVER_NAME) && l.contains(&bin));
    if !registered {
        match run(
            &command,
            &["mcp", "add", MCP_SERVER_NAME, &bin, "mcp-stdio"],
        )
        .await
        {
            Some((0, _)) => {}
            Some((code, text)) => {
                return Err(format!("`agy mcp add` exited {code}: {}", text.trim()));
            }
            None => return Err("`agy mcp add` did not finish".into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_are_env_gated_commands() {
        let v = agy_hooks_json(Path::new("/opt/ostra"));
        let cmd = v["ostra"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(cmd, "/opt/ostra hook --harness agy --event pre_tool_use");
        assert!(!cmd.contains("--execution"));
    }

    #[test]
    fn agent_markdown_front_matter() {
        let tmp = tempfile::tempdir().unwrap();
        let spec =
            crate::launch::tests::spec(HarnessKind::Agy, "gemini-3.8-flash-high", tmp.path());
        let md = agy_agent_markdown(&spec);
        assert!(md.starts_with("---\nname: ostra-implementer\n"));
        assert!(md.contains("inheritMcp: true"));
        assert!(
            md.contains("tools:\n    - view_file\n    - replace_file_content\n    - run_command\n")
        );
        assert!(md.contains("    - run_command\n"));
        assert!(md.trim_end().ends_with("Say \"hi\"."));
    }
}
