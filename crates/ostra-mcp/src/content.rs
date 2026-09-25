//! `tools/call` results as the text a tool result carries.

use serde_json::Value;

/// Longest tool result passed on, in bytes. The rest is cut with a note.
pub const MAX_RESULT: usize = 100 * 1024;

/// The result's text, `Err` when the server flags it with `isError`.
pub fn result_text(result: &Value) -> Result<String, String> {
    let mut parts: Vec<String> = vec![];
    for item in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let s = |k: &str| item.get(k).and_then(Value::as_str);
        let part = match s("type").unwrap_or("") {
            "text" => s("text").unwrap_or_default().to_string(),
            kind @ ("image" | "audio") => format!(
                "[{kind} {}, {} bytes, not shown because tool results are text]",
                s("mimeType").unwrap_or("of unknown type"),
                s("data").map_or(0, |d| d.len() * 3 / 4)
            ),
            "resource" => {
                let r = item.get("resource").cloned().unwrap_or(Value::Null);
                match r.get("text").and_then(Value::as_str) {
                    Some(t) => t.to_string(),
                    None => format!(
                        "[binary resource {}]",
                        r.get("uri").and_then(Value::as_str).unwrap_or("")
                    ),
                }
            }
            "resource_link" => match s("name").filter(|n| !n.is_empty()) {
                Some(name) => format!("[resource {} {name}]", s("uri").unwrap_or("")),
                None => format!("[resource {}]", s("uri").unwrap_or("")),
            },
            _ => item.to_string(),
        };
        parts.push(part);
    }
    if parts.iter().all(|p| p.trim().is_empty())
        && let Some(structured) = result.get("structuredContent")
    {
        parts = vec![serde_json::to_string_pretty(structured).unwrap_or_default()];
    }
    let text = cap(parts.join("\n"));
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        Err(if text.trim().is_empty() {
            "The tool reported an error without a message.".into()
        } else {
            text
        })
    } else {
        Ok(text)
    }
}

fn cap(mut s: String) -> String {
    if s.len() <= MAX_RESULT {
        return s;
    }
    let mut end = MAX_RESULT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let cut = s.len() - end;
    s.truncate(end);
    s.push_str(&format!("\n... ({cut} more bytes cut)"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn content_kinds() {
        let r = json!({"content": [
            {"type": "text", "text": "hello"},
            {"type": "image", "mimeType": "image/png", "data": "AAAA"},
            {"type": "resource", "resource": {"uri": "file:///a", "text": "body"}},
            {"type": "resource_link", "uri": "file:///b", "name": "b"},
        ]});
        assert_eq!(
            result_text(&r).unwrap(),
            "hello\n[image image/png, 3 bytes, not shown because tool results are text]\nbody\n[resource file:///b b]"
        );
        let r = json!({"content": [], "structuredContent": {"n": 1}});
        assert_eq!(result_text(&r).unwrap(), "{\n  \"n\": 1\n}");
        let r = json!({"content": [{"type": "text", "text": "boom"}], "isError": true});
        assert_eq!(result_text(&r).unwrap_err(), "boom");
        let r = json!({"content": [{"type": "text", "text": "x".repeat(MAX_RESULT + 5)}]});
        assert!(result_text(&r).unwrap().ends_with("(5 more bytes cut)"));
    }
}
