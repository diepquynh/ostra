//! The code navigation tools. The index and the dependency graph live in the server; a
//! [`CodeNav`] handed in through [`crate::ToolEnvConfig`] answers each call as text.

use crate::{ToolEnv, ToolOutput};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Answers code navigation calls for the project that holds `repo_root`. `Err` is a tool error.
#[async_trait::async_trait]
pub trait CodeNav: Send + Sync {
    async fn call(&self, repo_root: &Path, tool: &str, input: &Value) -> Result<String, String>;
}

/// Rule WD1: the work dir that holds the call's `path`, else the main one. A relative path that
/// starts with another work dir's folder name or project key goes to that work dir.
fn route(env: &ToolEnv, input: &Value) -> (PathBuf, Value) {
    let cfg = env.config();
    let main = cfg.repo_root.clone();
    let Some(raw) = input.get("path").and_then(Value::as_str).map(str::trim) else {
        return (main, input.clone());
    };
    if Path::new(raw).is_absolute() {
        let target = env.resolve(raw);
        let root = cfg
            .work_roots()
            .into_iter()
            .filter(|r| target.starts_with(r))
            .max_by_key(|r| r.components().count())
            .map(Path::to_path_buf)
            .unwrap_or(main);
        return (root, input.clone());
    }
    let rel = raw.trim_start_matches("./");
    if main.join(rel).exists() {
        return (main, input.clone());
    }
    for w in cfg.other_work_dirs() {
        let dir = w.path.file_name().and_then(|d| d.to_str());
        for prefix in [Some(w.project.as_str()), dir].into_iter().flatten() {
            if let Some(rest) = rel.strip_prefix(prefix).and_then(|r| r.strip_prefix('/')) {
                let mut input = input.clone();
                input["path"] = Value::String(rest.to_string());
                return (w.path.clone(), input);
            }
        }
    }
    (main, input.clone())
}

pub async fn run(env: &ToolEnv, tool: &str, input: &Value) -> ToolOutput {
    let Some(nav) = env.config().code.clone() else {
        return ToolOutput::err(
            "Code navigation is not available in this run. Use Grep, Glob, and Read instead.",
        );
    };
    let (root, input) = route(env, input);
    match nav.call(&root, tool, &input).await {
        Ok(text) => ToolOutput::ok(text),
        Err(e) => ToolOutput::err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{env_with_api, run as run_tool};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Seen(Mutex<Vec<(PathBuf, Value)>>);

    #[async_trait::async_trait]
    impl CodeNav for Seen {
        async fn call(&self, root: &Path, _tool: &str, input: &Value) -> Result<String, String> {
            self.0
                .lock()
                .unwrap()
                .push((root.to_path_buf(), input.clone()));
            Ok("ok".into())
        }
    }

    #[tokio::test]
    async fn code_calls_go_to_the_work_dir_that_holds_the_path() {
        let d = tempfile::tempdir().unwrap();
        let base = env_with_api(d.path());
        let seen = Arc::new(Seen::default());
        let mut cfg = base.config().clone();
        cfg.code = Some(seen.clone());
        let env = ToolEnv::new(cfg);
        let (web, api) = (
            env.config().repo_root.clone(),
            env.config().work_dirs[1].path.clone(),
        );
        std::fs::create_dir_all(web.join("src")).unwrap();
        std::fs::write(web.join("src/app.ts"), "").unwrap();
        let abs = api.join("src/routes.rs").to_string_lossy().into_owned();
        for path in [
            abs.as_str(),
            "api/src/routes.rs",
            "src/app.ts",
            "src/none.rs",
        ] {
            run_tool(&env, "CodeOutline", json!({"path": path})).await;
        }
        run_tool(&env, "CodeCallers", json!({"symbol": "main"})).await;
        let seen = seen.0.lock().unwrap();
        let roots: Vec<&PathBuf> = seen.iter().map(|(r, _)| r).collect();
        assert_eq!(roots, vec![&api, &api, &web, &web, &web]);
        assert_eq!(seen[1].1["path"], "src/routes.rs");
    }
}
