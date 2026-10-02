use crate::{ToolEnv, ToolOutput, str_arg};
use ostra_core::doc::{self, DocKind, Document};
use serde_json::Value;
use std::fmt::Write;

pub async fn document(env: &ToolEnv, input: &Value) -> ToolOutput {
    let Some(kind) = DocKind::for_agent(env.config().agent) else {
        return ToolOutput::err(
            "Only explore, generate-spec, and plan write documents. Write your output where your prompt says.",
        );
    };
    let Some(raw) = str_arg(input, "path").filter(|p| !p.trim().is_empty()) else {
        return ToolOutput::err("Give `path`: the absolute path of the document's markdown file.");
    };
    let md = env.resolve(raw);
    let name = md
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !name.starts_with(kind.prefix())
        || !name.ends_with(".md")
        || (kind == DocKind::Plan && name.contains("-phase-"))
    {
        return ToolOutput::err(format!(
            "Name the {} `{}{{run-stamp}}-{{topic-slug}}.md`: `{name}` is not that shape, and later stages find the file by it.",
            kind.label(),
            kind.prefix()
        ));
    }
    let session = &env.config().session_dir;
    // Compare through `fold` so a resolved path (no `\\?\`, real case) matches the stored session
    // dir on Windows, where the two can differ in prefix, case, and separators.
    let same_dir = md.parent().is_some_and(|p| {
        ostra_core::paths::fold(p) == ostra_core::paths::fold(session)
    });
    if !same_dir {
        return ToolOutput::err(format!(
            "Write the {} directly inside {}, the session dir your prompt names, because the next stage reads it there.",
            kind.label(),
            session.display()
        ));
    }
    let remove: Vec<String> = match input.get("remove") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(ids)) => ids
            .iter()
            .filter_map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
            })
            .collect(),
        Some(_) => return ToolOutput::err("`remove` must be a list of ids."),
    };
    let (value, kept) = match (input.get("document"), input.get("update")) {
        (Some(d), None) if remove.is_empty() => (d.clone(), vec![]),
        (Some(_), _) => {
            return ToolOutput::err(
                "Send either `document` (the whole document) or `update` with `remove`, not both.",
            );
        }
        (None, update) => {
            let existing = doc::load(&md);
            if existing.is_none() && !remove.is_empty() {
                return ToolOutput::err(format!(
                    "There is no {} at {} yet, so there is nothing to remove.",
                    kind.label(),
                    md.display()
                ));
            }
            let empty = Value::Object(Default::default());
            match doc::apply_update(existing, update.unwrap_or(&empty), &remove) {
                Ok(m) => (m.value, m.kept),
                Err(e) => return ToolOutput::err(e.0),
            }
        }
    };
    let written = match doc::write(kind, &md, &value, Some(&env.config().repo_root)) {
        Ok(w) => w,
        Err(e) => {
            return ToolOutput::err(format!(
                "The {} does not match its schema, so nothing was written: {e}. Fix that field and call Document again.",
                kind.label()
            ));
        }
    };
    for f in &written.files {
        env.mark_read(f);
        env.mark_read(&doc::json_path(f));
    }
    ToolOutput::ok(summary(&written, &kept))
}

fn summary(w: &doc::Written, kept: &[String]) -> String {
    let mut out = String::new();
    let what = match &w.document {
        Document::Research(d) => format!(
            "research document: {} files, {} patterns, {} external facts, {} sources, {} open questions",
            d.files.len(),
            d.patterns.len(),
            d.external.len(),
            d.sources.len(),
            d.open_questions.len()
        ),
        Document::Spec(d) => format!(
            "spec: {} criteria, {} deliverables, {} requirements, {} acceptance criteria, {} evidence rows, {} open questions",
            d.criteria.len(),
            d.deliverables.len(),
            d.requirements.len(),
            d.requirements
                .iter()
                .map(|r| r.acceptance.len())
                .sum::<usize>(),
            d.evidence.len(),
            d.open_questions.len()
        ),
        Document::Plan(d) => format!(
            "plan: {} phases, {} steps, {} clarifying questions",
            d.phases.len(),
            d.phases.iter().map(|p| p.steps.len()).sum::<usize>(),
            d.clarifying_questions.len()
        ),
        Document::Phase(_) => "phase".into(),
    };
    let _ = writeln!(out, "Wrote the {what}.");
    let _ = writeln!(
        out,
        "Files (markdown rendered from the document, with a .json beside each):"
    );
    for f in &w.files {
        let _ = writeln!(out, "- {}", f.display());
    }
    if !kept.is_empty() {
        let _ = writeln!(
            out,
            "Kept, because this update did not send them: {}. Remove any that no longer applies with `remove`.",
            kept.join("; ")
        );
    }
    if w.issues.is_empty() {
        let _ = writeln!(out, "Ostra's checks found nothing to fix.");
    } else {
        let errors = w.errors();
        if errors > 0 {
            let _ = writeln!(
                out,
                "Fix the {errors} errors below with an `update` before you submit, because the submit call is refused while any remain:"
            );
        } else {
            let _ = writeln!(out, "Warnings to review:");
        }
        for i in &w.issues {
            let _ = writeln!(out, "{}", i.line());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::testutil::{env_in, run};
    use crate::{ToolEnv, ToolEnvConfig};
    use ostra_core::AgentName;
    use serde_json::json;

    fn env_for(dir: &std::path::Path, agent: AgentName) -> ToolEnv {
        let base = env_in(dir);
        let mut cfg: ToolEnvConfig = base.config().clone();
        cfg.agent = agent;
        cfg.report_file = None;
        ToolEnv::new(cfg)
    }

    fn research() -> serde_json::Value {
        json!({"title": "T", "date": "2026-07-28", "repo": "app", "scope": "s", "problem": "p"})
    }

    #[tokio::test]
    async fn writes_json_and_markdown_then_updates_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_for(dir.path(), AgentName::Explore);
        let md = env.config().session_dir.join("ostra-research-1-t.md");
        let out = run(
            &env,
            "Document",
            json!({"path": md, "document": research()}),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("nothing to fix"), "{}", out.text);
        assert!(
            std::fs::read_to_string(&md)
                .unwrap()
                .starts_with("# Research: T")
        );
        let q = json!({"id": "Q1", "question": "Which?", "tag": "Scope", "options": [{"label": "A", "description": "a"}], "recommended": 0});
        let out = run(
            &env,
            "Document",
            json!({"path": md, "update": {"open_questions": [q]}}),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("(Q1): Give the question 2 to 4 options"),
            "{}",
            out.text
        );
        let stored = ostra_core::doc::load(&md).unwrap();
        assert_eq!(stored["title"], "T");
        assert_eq!(stored["open_questions"][0]["id"], "Q1");
        let out = run(&env, "Document", json!({"path": md, "remove": ["Q1"]})).await;
        assert!(!out.is_error, "{}", out.text);
        assert_eq!(
            ostra_core::doc::load(&md).unwrap()["open_questions"],
            json!([])
        );
    }

    // Rules D2a and Hard 4: a research document is fingerprinted against the agent's repo, and a
    // file it names that the repo lacks is an error.
    #[tokio::test]
    async fn research_files_are_fingerprinted_and_checked_against_the_repo() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_for(dir.path(), AgentName::Explore);
        let repo = env.config().repo_root.clone();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/a.rs"), "fn a() {}").unwrap();
        let md = env.config().session_dir.join("ostra-research-1-t.md");
        let mut doc = research();
        doc["files"] = json!([
            {"path": "src/a.rs", "purpose": "p"},
            {"path": "src/b.rs", "purpose": "p"}
        ]);
        let out = run(&env, "Document", json!({"path": md, "document": doc})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("Fix the 1 errors") && out.text.contains("`src/b.rs` is not there"),
            "{}",
            out.text
        );
        let stored = ostra_core::doc::load(&md).unwrap();
        assert_eq!(stored["snapshot"]["root"], json!(repo.display().to_string()));
        assert_eq!(stored["snapshot"]["files"]["src/b.rs"], "missing");
        assert_eq!(
            stored["snapshot"]["files"]["src/a.rs"].as_str().unwrap().len(),
            16
        );
    }

    #[tokio::test]
    async fn a_document_sent_as_json_text_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_for(dir.path(), AgentName::Explore);
        let md = env.config().session_dir.join("ostra-research-1-t.md");
        let text = serde_json::to_string_pretty(&research()).unwrap();
        let out = run(&env, "Document", json!({"path": md, "document": text})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(md.exists());
    }

    #[tokio::test]
    async fn refuses_bad_paths_schema_errors_and_other_agents() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_for(dir.path(), AgentName::Explore);
        let session = env.config().session_dir.clone();
        let out = run(
            &env,
            "Document",
            json!({"path": session.join("notes.md"), "document": research()}),
        )
        .await;
        assert!(
            out.is_error && out.text.contains("ostra-research-"),
            "{}",
            out.text
        );
        let out = run(
            &env,
            "Document",
            json!({"path": session.join("sub/ostra-research-1.md"), "document": research()}),
        )
        .await;
        assert!(
            out.is_error && out.text.contains("session dir"),
            "{}",
            out.text
        );
        let mut bad = research();
        bad.as_object_mut().unwrap().remove("scope");
        let out = run(
            &env,
            "Document",
            json!({"path": session.join("ostra-research-1.md"), "document": bad}),
        )
        .await;
        assert!(
            out.is_error && out.text.contains("missing field `scope`"),
            "{}",
            out.text
        );
        assert!(!session.join("ostra-research-1.md").exists());
        let imp = env_for(dir.path(), AgentName::Implementer);
        let out = run(
            &imp,
            "Document",
            json!({"path": session.join("ostra-research-1.md"), "document": research()}),
        )
        .await;
        assert!(out.is_error, "{}", out.text);
    }
}
