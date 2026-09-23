//! Best-effort reads of harness transcripts: the final assistant message and token usage. Every
//! reader tolerates a partial last line, because the file is written while the session is live.

use ostra_core::HarnessKind;
use ostra_core::exec::Usage;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptFacts {
    pub last_message: Option<String>,
    pub usage: Usage,
}

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str(l.trim()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn text_blocks(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let texts: Vec<&str> = items
                .iter()
                .filter(|b| {
                    matches!(
                        b.get("type").and_then(Value::as_str),
                        Some("text" | "output_text")
                    )
                })
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

/// Claude Code `~/.claude/projects/<dir>/<session>.jsonl`.
pub fn claude_facts(path: &Path) -> TranscriptFacts {
    let mut facts = TranscriptFacts::default();
    for v in lines(path) {
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message") else {
            continue;
        };
        if let Some(t) = msg
            .get("content")
            .and_then(text_blocks)
            .filter(|t| !t.trim().is_empty())
        {
            facts.last_message = Some(t);
        }
        if let Some(u) = msg.get("usage") {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            facts.usage.input_tokens += n("input_tokens");
            facts.usage.output_tokens += n("output_tokens");
            facts.usage.cache_read_tokens += n("cache_read_input_tokens");
            facts.usage.cache_write_tokens += n("cache_creation_input_tokens");
        }
    }
    facts
}

/// Codex rollout `~/.codex/sessions/YYYY/MM/DD/rollout-*-<session>.jsonl`.
pub fn codex_facts(path: &Path) -> TranscriptFacts {
    let mut facts = TranscriptFacts::default();
    for v in lines(path) {
        let payload = v.get("payload").unwrap_or(&Value::Null);
        match (
            v.get("type").and_then(Value::as_str),
            payload.get("type").and_then(Value::as_str),
        ) {
            (Some("response_item"), Some("message"))
                if payload.get("role").and_then(Value::as_str) == Some("assistant") =>
            {
                if let Some(t) = payload
                    .get("content")
                    .and_then(text_blocks)
                    .filter(|t| !t.trim().is_empty())
                {
                    facts.last_message = Some(t);
                }
            }
            (Some("event_msg"), Some("token_count")) => {
                if let Some(total) = payload.pointer("/info/total_token_usage") {
                    let n = |k: &str| total.get(k).and_then(Value::as_u64).unwrap_or(0);
                    facts.usage.input_tokens = n("input_tokens");
                    facts.usage.cache_read_tokens = n("cached_input_tokens");
                    facts.usage.output_tokens = n("output_tokens") + n("reasoning_output_tokens");
                }
            }
            _ => {}
        }
    }
    facts
}

/// Antigravity `brain/<conversation>/.system_generated/logs/transcript_full.jsonl`: the last
/// planner response with content is the final message. It records no token counts.
pub fn agy_facts(path: &Path) -> TranscriptFacts {
    let mut facts = TranscriptFacts::default();
    for v in lines(path) {
        if v.get("type").and_then(Value::as_str) == Some("PLANNER_RESPONSE")
            && let Some(t) = v
                .get("content")
                .and_then(Value::as_str)
                .filter(|t| !t.trim().is_empty())
        {
            facts.last_message = Some(t.to_string());
        }
    }
    facts
}

/// The Codex rollout file for a session id, searched newest day first.
pub fn find_codex_rollout(home: &Path, session_id: &str) -> Option<PathBuf> {
    let root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("sessions");
    let mut stack = vec![root];
    let mut found = None;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(session_id) && n.ends_with(".jsonl"))
            {
                found = Some(p);
            }
        }
    }
    found
}

/// Claude Code transcript for a session id under `~/.claude/projects/`.
pub fn find_claude_transcript(home: &Path, session_id: &str) -> Option<PathBuf> {
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"))
        .join("projects");
    let name = format!("{session_id}.jsonl");
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|d| d.path().join(&name))
        .find(|p| p.exists())
}

pub fn facts(
    harness: HarnessKind,
    transcript: Option<&Path>,
    session_id: Option<&str>,
    home: &Path,
) -> TranscriptFacts {
    let path = transcript
        .map(Path::to_path_buf)
        .or_else(|| match (harness, session_id) {
            (HarnessKind::Claude, Some(s)) => find_claude_transcript(home, s),
            (HarnessKind::Codex, Some(s)) => find_codex_rollout(home, s),
            _ => None,
        });
    match (harness, path) {
        (HarnessKind::Claude, Some(p)) => claude_facts(&p),
        (HarnessKind::Codex, Some(p)) => codex_facts(&p),
        (HarnessKind::Agy, Some(p)) => agy_facts(&p),
        _ => TranscriptFacts::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.jsonl");
        std::fs::write(
            &p,
            concat!(
                r#"{"type":"user","message":{"content":"go"}}"#, "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"first"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100}}}"#, "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read"}],"usage":{"input_tokens":2,"output_tokens":1}}}"#, "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]"#,
            ),
        )
        .unwrap();
        let f = claude_facts(&p);
        assert_eq!(f.last_message.as_deref(), Some("first"));
        assert_eq!(
            (
                f.usage.input_tokens,
                f.usage.output_tokens,
                f.usage.cache_read_tokens
            ),
            (12, 6, 100)
        );
    }

    #[test]
    fn codex_rollout() {
        let tmp = tempfile::tempdir().unwrap();
        let day = tmp.path().join(".codex/sessions/2026/09/22");
        std::fs::create_dir_all(&day).unwrap();
        let p = day.join("rollout-2026-09-22T10-00-00-019a0000-aaaa.jsonl");
        std::fs::write(
            &p,
            concat!(
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"all done"}]}}"#, "\n",
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"cached_input_tokens":40,"output_tokens":7}}}}"#, "\n",
            ),
        )
        .unwrap();
        let _g = crate::launch::tests::EnvGuard::unset("CODEX_HOME");
        let found = find_codex_rollout(tmp.path(), "019a0000-aaaa").unwrap();
        let f = codex_facts(&found);
        assert_eq!(f.last_message.as_deref(), Some("all done"));
        assert_eq!(f.usage.cache_read_tokens, 40);
    }
}
