use crate::fs::write_file;
use crate::{ToolEnv, ToolOutput, required, str_arg, u64_arg};
use ostra_store::memory::{DEFAULT_RECALL_LIMIT, MemoryStore};
use serde_json::Value;
use std::path::PathBuf;

const MAX_RECALL_LIMIT: usize = 50;

fn skill_output(path: &std::path::Path, content: &str) -> ToolOutput {
    ToolOutput::ok(format!(
        "Skill loaded from {}. Follow its instructions for this task.\n\n{content}",
        path.display()
    ))
}

pub async fn skill(env: &ToolEnv, input: &Value) -> ToolOutput {
    if let Some(path) = str_arg(input, "path").filter(|p| !p.trim().is_empty()) {
        let path = env.resolve(path);
        return match tokio::fs::read_to_string(&path).await {
            Ok(content) => {
                env.mark_read(&path);
                skill_output(&path, &content)
            }
            Err(_) => ToolOutput::err(format!("No skill file at {}.", path.display())),
        };
    }
    let name = match str_arg(input, "name").or_else(|| str_arg(input, "skill")) {
        Some(n) if !n.trim().is_empty() => n.trim(),
        _ => return ToolOutput::err("Give the skill's `name`, or the `path` of its SKILL.md."),
    };
    let bare = name.trim_start_matches("ostra:");
    if bare.contains('/') || bare.contains("..") {
        return ToolOutput::err(format!(
            "`{name}` is not a skill name. Pass a SKILL.md location as `path` instead."
        ));
    }
    let local: PathBuf = ostra_core::paths::project_skills_dir(&env.config().repo_root).join(bare).join("SKILL.md");
    if let Ok(content) = tokio::fs::read_to_string(&local).await {
        env.mark_read(&local);
        return skill_output(&local, &content);
    }
    if let Some((path, content)) = (env.config().skill_resolver)(bare) {
        return skill_output(&path, &content);
    }
    ToolOutput::err(format!(
        "Unknown skill `{bare}`: there is no {}. Use the skill paths your repo brief lists; do not guess a path.",
        local.display()
    ))
}

pub async fn report(env: &ToolEnv, input: &Value) -> ToolOutput {
    let content = match required(input, "content") {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(path) = env.config().report_file.clone() else {
        return ToolOutput::err(
            "This run has no declared `Report file:`, so there is nothing for Report to write. Write your output where your prompt says.",
        );
    };
    if let Err(e) = write_file(&path, content).await {
        return ToolOutput::err(e);
    }
    env.mark_read(&path);
    ToolOutput::ok(format!("Report written to {} ({} bytes).", path.display(), content.len()))
}

pub async fn memory(env: &ToolEnv, input: &Value) -> ToolOutput {
    let area = match required(input, "area") {
        Ok(a) if !a.trim().is_empty() => a.trim().to_string(),
        Ok(_) => return ToolOutput::err("`area` is empty. Scope the lesson to a module, for example `orders::Service`."),
        Err(e) => return e,
    };
    let lesson = match required(input, "lesson") {
        Ok(l) if !l.trim().is_empty() => l.trim().to_string(),
        Ok(_) => return ToolOutput::err("`lesson` is empty."),
        Err(e) => return e,
    };
    let source = str_arg(input, "source")
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| env.config().memory_source.clone());
    let db = env.config().memory_db.clone();
    let result = tokio::task::spawn_blocking(move || {
        MemoryStore::open(&db).and_then(|s| s.record(&area, &lesson, &source)).map(|total| (area, total))
    })
    .await;
    match result {
        Ok(Ok((area, total))) => ToolOutput::ok(format!(
            "Recorded the lesson under `{area}`. The store now holds {total} lesson{}.",
            if total == 1 { "" } else { "s" }
        )),
        Ok(Err(e)) => ToolOutput::err(format!("Recording the lesson failed: {e}")),
        Err(e) => ToolOutput::err(format!("Recording the lesson failed: {e}")),
    }
}

pub async fn memory_recall(env: &ToolEnv, input: &Value) -> ToolOutput {
    let query = str_arg(input, "query").map(str::to_string);
    let area = str_arg(input, "area").map(str::to_string);
    let limit = u64_arg(input, "limit").map(|l| l as usize).unwrap_or(DEFAULT_RECALL_LIMIT).clamp(1, MAX_RECALL_LIMIT);
    let db = env.config().memory_db.clone();
    let result = tokio::task::spawn_blocking(move || {
        MemoryStore::open(&db).and_then(|s| s.recall(area.as_deref(), query.as_deref(), limit))
    })
    .await;
    let lessons = match result {
        Ok(Ok(l)) => l,
        Ok(Err(e)) => return ToolOutput::err(format!("Recall failed: {e}")),
        Err(e) => return ToolOutput::err(format!("Recall failed: {e}")),
    };
    if lessons.is_empty() {
        return ToolOutput::ok("No lessons recorded for this query.");
    }
    let mut out = format!("{} lesson{} recalled, most relevant first:\n", lessons.len(), if lessons.len() == 1 { "" } else { "s" });
    for (i, l) in lessons.iter().enumerate() {
        let date = l.created_at.get(..10).unwrap_or(&l.created_at);
        out.push_str(&format!("{}. [{}] {} (source: {}, {date})\n", i + 1, l.area, l.lesson, l.source));
    }
    ToolOutput::ok(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use crate::testutil::{env_in, run};
    use serde_json::json;

    #[tokio::test]
    async fn skill_by_name_path_and_resolver() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let dir = env.config().repo_root.join(".ostra/skills/convention");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "# Convention\nUse final.").unwrap();
        let out = run(&env, "Skill", json!({"name": "convention"})).await;
        assert!(!out.is_error && out.text.contains("Use final."), "{}", out.text);
        let out = run(&env, "Skill", json!({"path": ".ostra/skills/convention/SKILL.md"})).await;
        assert!(!out.is_error && out.text.contains("Skill loaded from"));
        let out = run(&env, "Skill", json!({"name": "meta-author"})).await;
        assert!(out.text.contains("# Meta"));
        let out = run(&env, "Skill", json!({"name": "nope"})).await;
        assert!(out.is_error);
        let out = run(&env, "Skill", json!({"name": "../etc"})).await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn report_writes_declared_path() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let out = run(&env, "Report", json!({"content": "# Report\n"})).await;
        assert!(!out.is_error, "{}", out.text);
        let path = env.config().report_file.clone().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Report\n");
        let out = run(&env, "Edit", json!({"file_path": path, "old_string": "Report", "new_string": "Change report"})).await;
        assert!(!out.is_error, "{}", out.text);
    }

    #[tokio::test]
    async fn report_without_declared_path_fails() {
        let d = tempfile::tempdir().unwrap();
        let mut cfg = env_in(d.path()).config().clone();
        cfg.report_file = None;
        let env = crate::ToolEnv::new(cfg);
        let out = run(&env, "Report", json!({"content": "x"})).await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn memory_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let out = run(&env, "MemoryRecall", json!({"query": "ownership"})).await;
        assert_eq!(out.text, "No lessons recorded for this query.");
        let out = run(&env, "Memory", json!({"area": "orders", "lesson": "cancel checks ownership first"})).await;
        assert!(!out.is_error && out.text.contains("1 lesson."), "{}", out.text);
        let out = run(&env, "MemoryRecall", json!({"query": "ownership", "area": "orders"})).await;
        assert!(out.text.contains("1. [orders] cancel checks ownership first (source: implementer x_test"), "{}", out.text);
        let out = run(&env, "Memory", json!({"area": " ", "lesson": "x"})).await;
        assert!(out.is_error);
    }
}
