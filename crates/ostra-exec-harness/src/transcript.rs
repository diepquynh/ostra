//! Best-effort reads of harness session files: the final assistant message and token usage.
//! [`TranscriptReader`] reads a file incrementally, so a live session can be followed while it is
//! written; a partial last line waits for the next read.

use ostra_core::HarnessKind;
use ostra_core::exec::Usage;
use ostra_core::pricing;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptFacts {
    pub last_message: Option<String>,
    pub usage: Usage,
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

/// What one harness's lines add up to so far.
#[derive(Debug)]
enum Tally {
    /// Claude Code writes one line per content block, each repeating the message's usage, so
    /// usage counts once per message id. Each message is priced with its own model, because a
    /// session can switch models.
    Claude {
        messages: Vec<Usage>,
        by_id: HashMap<String, usize>,
        last_message: Option<String>,
    },
    /// Codex `token_count` events carry running totals in which `input_tokens` includes cached
    /// input and `output_tokens` includes reasoning, so each increase is normalized and priced
    /// with the current turn's model.
    Codex {
        model: String,
        seen: Usage,
        usage: Usage,
        last_message: Option<String>,
    },
    /// Each Grok Build `turn_completed` update carries that turn's usage, with `inputTokens`
    /// including cached input, and the turn's cost in `costUsdTicks` (10^10 per dollar), which
    /// Grok computes itself.
    Grok { usage: Usage },
    /// Antigravity records no token counts; the last planner response is the final message.
    Agy { last_message: Option<String> },
}

impl Tally {
    fn new(harness: HarnessKind) -> Tally {
        match harness {
            HarnessKind::Claude => Tally::Claude { messages: vec![], by_id: HashMap::new(), last_message: None },
            HarnessKind::Codex => Tally::Codex { model: String::new(), seen: Usage::default(), usage: Usage::default(), last_message: None },
            HarnessKind::Grok => Tally::Grok { usage: Usage::default() },
            HarnessKind::Agy => Tally::Agy { last_message: None },
        }
    }

    fn feed(&mut self, v: &Value) {
        match self {
            Tally::Claude { messages, by_id, last_message } => {
                if v.get("type").and_then(Value::as_str) != Some("assistant") {
                    return;
                }
                let Some(msg) = v.get("message") else { return };
                if let Some(t) = msg.get("content").and_then(text_blocks).filter(|t| !t.trim().is_empty()) {
                    *last_message = Some(t);
                }
                let Some(u) = msg.get("usage") else { return };
                let n = |p: &str| u.pointer(p).and_then(Value::as_u64).unwrap_or(0);
                let mut usage = Usage {
                    input_tokens: n("/input_tokens"),
                    output_tokens: n("/output_tokens"),
                    cache_read_tokens: n("/cache_read_input_tokens"),
                    cache_write_tokens: n("/cache_creation_input_tokens"),
                    cache_write_1h_tokens: n("/cache_creation/ephemeral_1h_input_tokens"),
                    ..Default::default()
                };
                let model = msg.get("model").and_then(Value::as_str).unwrap_or_default();
                usage.cost_usd = pricing::cost(model, &usage, n("/server_tool_use/web_search_requests"));
                match msg.get("id").and_then(Value::as_str) {
                    Some(id) => match by_id.get(id) {
                        Some(&i) => messages[i] = usage,
                        None => {
                            by_id.insert(id.to_string(), messages.len());
                            messages.push(usage);
                        }
                    },
                    None => messages.push(usage),
                }
            }
            Tally::Codex { model, seen, usage, last_message } => {
                let payload = v.get("payload").unwrap_or(&Value::Null);
                match (v.get("type").and_then(Value::as_str), payload.get("type").and_then(Value::as_str)) {
                    (Some("turn_context"), _) => {
                        if let Some(m) = payload.get("model").and_then(Value::as_str) {
                            *model = m.to_string();
                        }
                    }
                    (Some("response_item"), Some("message")) if payload.get("role").and_then(Value::as_str) == Some("assistant") => {
                        if let Some(t) = payload.get("content").and_then(text_blocks).filter(|t| !t.trim().is_empty()) {
                            *last_message = Some(t);
                        }
                    }
                    (Some("event_msg"), Some("token_count")) => {
                        let Some(total) = payload.pointer("/info/total_token_usage") else { return };
                        let n = |k: &str| total.get(k).and_then(Value::as_u64).unwrap_or(0);
                        let (cached, written) = (n("cached_input_tokens"), n("cache_write_input_tokens"));
                        let now = Usage {
                            input_tokens: n("input_tokens").saturating_sub(cached).saturating_sub(written),
                            output_tokens: n("output_tokens"),
                            cache_read_tokens: cached,
                            cache_write_tokens: written,
                            ..Default::default()
                        };
                        let mut delta = Usage {
                            input_tokens: now.input_tokens.saturating_sub(seen.input_tokens),
                            output_tokens: now.output_tokens.saturating_sub(seen.output_tokens),
                            cache_read_tokens: now.cache_read_tokens.saturating_sub(seen.cache_read_tokens),
                            cache_write_tokens: now.cache_write_tokens.saturating_sub(seen.cache_write_tokens),
                            ..Default::default()
                        };
                        delta.cost_usd = pricing::cost(model, &delta, 0);
                        usage.add(&delta);
                        *seen = now;
                    }
                    _ => {}
                }
            }
            Tally::Grok { usage } => {
                let Some(update) = v.pointer("/params/update") else { return };
                if update.get("sessionUpdate").and_then(Value::as_str) != Some("turn_completed") {
                    return;
                }
                let Some(u) = update.get("usage") else { return };
                let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                let (cached, written) = (n("cachedReadTokens"), n("cacheCreationTokens"));
                usage.add(&Usage {
                    input_tokens: n("inputTokens").saturating_sub(cached).saturating_sub(written),
                    output_tokens: n("outputTokens"),
                    cache_read_tokens: cached,
                    cache_write_tokens: written,
                    cost_usd: n("costUsdTicks") as f64 / 1e10,
                    ..Default::default()
                });
            }
            Tally::Agy { last_message } => {
                if v.get("type").and_then(Value::as_str) == Some("PLANNER_RESPONSE")
                    && let Some(t) = v.get("content").and_then(Value::as_str).filter(|t| !t.trim().is_empty())
                {
                    *last_message = Some(t.to_string());
                }
            }
        }
    }

    fn facts(&self) -> TranscriptFacts {
        match self {
            Tally::Claude { messages, last_message, .. } => {
                let mut usage = Usage::default();
                messages.iter().for_each(|m| usage.add(m));
                TranscriptFacts { last_message: last_message.clone(), usage }
            }
            Tally::Codex { usage, last_message, .. } => TranscriptFacts { last_message: last_message.clone(), usage: *usage },
            Tally::Grok { usage } => TranscriptFacts { last_message: None, usage: *usage },
            Tally::Agy { last_message } => TranscriptFacts { last_message: last_message.clone(), usage: Usage::default() },
        }
    }
}

/// Follows one harness session file. Each [`poll`](Self::poll) reads only the bytes appended
/// since the last one.
#[derive(Debug)]
pub struct TranscriptReader {
    harness: HarnessKind,
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    tally: Tally,
}

impl TranscriptReader {
    pub fn new(harness: HarnessKind, path: PathBuf) -> Self {
        TranscriptReader { harness, path, offset: 0, partial: vec![], tally: Tally::new(harness) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read what was appended. Returns whether any line was added.
    pub fn poll(&mut self) -> bool {
        let Ok(mut file) = std::fs::File::open(&self.path) else { return false };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            // The file was replaced or truncated: start over.
            *self = TranscriptReader::new(self.harness, std::mem::take(&mut self.path));
        }
        if len == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let mut buf = vec![];
        let Ok(n) = file.read_to_end(&mut buf) else { return false };
        self.offset += n as u64;
        self.partial.extend_from_slice(&buf);
        let mut added = false;
        let mut start = 0;
        while let Some(end) = self.partial[start..].iter().position(|b| *b == b'\n') {
            added |= self.feed_line(start, start + end);
            start += end + 1;
        }
        self.partial.drain(..start);
        // A last line without its newline is complete once it parses: a JSON object ends at its
        // closing brace, so a line still being written does not.
        if self.feed_line(0, self.partial.len()) {
            self.partial.clear();
            added = true;
        }
        added
    }

    fn feed_line(&mut self, start: usize, end: usize) -> bool {
        match serde_json::from_slice::<Value>(self.partial[start..end].trim_ascii()) {
            Ok(v) => {
                self.tally.feed(&v);
                true
            }
            Err(_) => false,
        }
    }

    pub fn facts(&self) -> TranscriptFacts {
        self.tally.facts()
    }
}

fn read_all(harness: HarnessKind, path: &Path) -> TranscriptFacts {
    let mut r = TranscriptReader::new(harness, path.to_path_buf());
    r.poll();
    r.facts()
}

/// Claude Code `~/.claude/projects/<dir>/<session>.jsonl`.
pub fn claude_facts(path: &Path) -> TranscriptFacts {
    read_all(HarnessKind::Claude, path)
}

/// Codex rollout `~/.codex/sessions/YYYY/MM/DD/rollout-*-<session>.jsonl`.
pub fn codex_facts(path: &Path) -> TranscriptFacts {
    read_all(HarnessKind::Codex, path)
}

/// Grok Build `~/.grok/sessions/<cwd>/<session>/updates.jsonl`.
pub fn grok_facts(path: &Path) -> TranscriptFacts {
    read_all(HarnessKind::Grok, path)
}

/// Antigravity `brain/<conversation>/.system_generated/logs/transcript_full.jsonl`.
pub fn agy_facts(path: &Path) -> TranscriptFacts {
    read_all(HarnessKind::Agy, path)
}

/// Grok Build's `updates.jsonl` for a session: next to the transcript its hooks name, or found
/// by session id under `~/.grok/sessions/<cwd>/`.
pub fn find_grok_updates(home: &Path, transcript: Option<&Path>, session_id: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = transcript.and_then(Path::parent).map(|d| d.join("updates.jsonl")).filter(|p| p.exists()) {
        return Some(p);
    }
    let root = std::env::var_os("GROK_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".grok"))
        .join("sessions");
    let session_id = session_id?;
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|d| d.path().join(session_id).join("updates.jsonl"))
        .find(|p| p.exists())
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

/// The session file that carries a harness's usage, once it exists.
pub fn locate(harness: HarnessKind, transcript: Option<&Path>, session_id: Option<&str>, home: &Path) -> Option<PathBuf> {
    match harness {
        HarnessKind::Grok => find_grok_updates(home, transcript, session_id),
        _ => transcript.map(Path::to_path_buf).filter(|p| p.exists()).or_else(|| match (harness, session_id) {
            (HarnessKind::Claude, Some(s)) => find_claude_transcript(home, s),
            (HarnessKind::Codex, Some(s)) => find_codex_rollout(home, s),
            _ => None,
        }),
    }
}

pub fn facts(harness: HarnessKind, transcript: Option<&Path>, session_id: Option<&str>, home: &Path) -> TranscriptFacts {
    locate(harness, transcript, session_id, home).map(|p| read_all(harness, &p)).unwrap_or_default()
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
        assert_eq!(f.usage.input_tokens, 10);
    }

    #[test]
    fn reader_follows_appends_and_waits_for_a_partial_line() {
        ostra_core::pricing::install_test_prices();
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.jsonl");
        let line = |id: &str, out: u64| format!(r#"{{"type":"assistant","message":{{"id":"{id}","model":"claude-sonnet-5","content":[],"usage":{{"input_tokens":0,"output_tokens":{out}}}}}}}"#);
        std::fs::write(&p, format!("{}\n", line("a", 1_000_000))).unwrap();
        let mut r = TranscriptReader::new(HarnessKind::Claude, p.clone());
        assert!(r.poll());
        assert!(!r.poll(), "nothing new");
        let second = line("b", 2_000_000);
        let (head, rest) = second.split_at(30);
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(head.as_bytes()).unwrap();
        assert!(!r.poll(), "a partial line waits");
        f.write_all(format!("{rest}\n").as_bytes()).unwrap();
        assert!(r.poll());
        assert_eq!(r.facts().usage.output_tokens, 3_000_000);
        assert!((r.facts().usage.cost_usd - 30.0).abs() < 1e-9);
        std::fs::write(&p, format!("{}\n", line("c", 5))).unwrap();
        assert!(r.poll());
        assert_eq!(r.facts().usage.output_tokens, 5, "a replaced file starts over");
    }

    #[test]
    fn grok_sums_turns_and_takes_its_own_cost() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".grok/sessions/%2Frepo/01a0-s");
        std::fs::create_dir_all(&dir).unwrap();
        let turn = |input: u64, cached: u64, ticks: u64| {
            format!(r#"{{"method":"session/update","params":{{"sessionId":"01a0-s","update":{{"sessionUpdate":"turn_completed","usage":{{"inputTokens":{input},"outputTokens":10,"cachedReadTokens":{cached},"cacheCreationTokens":0,"costUsdTicks":{ticks}}}}}}}}}"#)
        };
        let lines = [r#"{"method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk"}}}"#.to_string(), turn(100, 60, 5_000_000_000), turn(50, 50, 2_500_000_000)];
        std::fs::write(dir.join("updates.jsonl"), lines.join("\n")).unwrap();
        let _g = crate::launch::tests::EnvGuard::unset("GROK_HOME");
        let f = facts(HarnessKind::Grok, Some(&dir.join("chat_history.jsonl")), None, tmp.path());
        assert_eq!((f.usage.input_tokens, f.usage.cache_read_tokens, f.usage.output_tokens), (40, 110, 20));
        assert!((f.usage.cost_usd - 0.75).abs() < 1e-9);
        let by_id = facts(HarnessKind::Grok, None, Some("01a0-s"), tmp.path());
        assert_eq!(by_id.usage, f.usage);
    }

    #[test]
    fn claude_usage_counts_once_per_message_and_prices_cache_ttls() {
        ostra_core::pricing::install_test_prices();
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.jsonl");
        let usage = r#""usage":{"input_tokens":1000000,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":3000000,"cache_creation":{"ephemeral_5m_input_tokens":2000000,"ephemeral_1h_input_tokens":1000000}}"#;
        let line = |block: &str| format!(r#"{{"type":"assistant","message":{{"id":"msg_1","model":"claude-opus-5","content":[{block}],{usage}}}}}"#);
        std::fs::write(&p, [line(r#"{"type":"thinking","thinking":""}"#), line(r#"{"type":"text","text":"hi"}"#)].join("\n")).unwrap();
        let f = claude_facts(&p);
        assert_eq!((f.usage.input_tokens, f.usage.cache_write_tokens, f.usage.cache_write_1h_tokens), (1_000_000, 3_000_000, 1_000_000));
        // Opus 5: $5 input, $6.25 per 5-minute write, $10 per 1-hour write, per million.
        assert!((f.usage.cost_usd - (5.0 + 2.0 * 6.25 + 10.0)).abs() < 1e-9, "{}", f.usage.cost_usd);
    }

    #[test]
    fn codex_prices_each_increase_of_the_running_total() {
        ostra_core::pricing::install_test_prices();
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("r.jsonl");
        let count = |input: u64, cached: u64, output: u64| {
            format!(r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":1}}}}}}}}"#)
        };
        let lines = [
            r#"{"type":"turn_context","payload":{"model":"gpt-5.6-terra"}}"#.to_string(),
            count(100_000, 0, 0),
            count(100_000, 0, 0),
            count(250_000, 50_000, 50_000),
        ];
        std::fs::write(&p, lines.join("\n")).unwrap();
        let f = codex_facts(&p);
        assert_eq!((f.usage.input_tokens, f.usage.cache_read_tokens, f.usage.output_tokens), (200_000, 50_000, 50_000));
        // Terra below its 272k context tier: $2 input, $0.2 cached, $12 output, per million.
        assert!((f.usage.cost_usd - (0.4 + 0.01 + 0.6)).abs() < 1e-9, "{}", f.usage.cost_usd);
    }
}
