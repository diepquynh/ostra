use crate::text::{floor_boundary, unified_diff};
use crate::{ToolEnv, ToolOutput, bool_arg, required, u64_arg};
use serde_json::Value;
use std::path::Path;

const DEFAULT_LINES: usize = 2000;
const MAX_LINE_CHARS: usize = 2000;
const MAX_READ_BYTES: u64 = 50 * 1024 * 1024;

fn looks_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8192)];
    head.contains(&0) || std::str::from_utf8(head).is_err_and(|e| e.error_len().is_some())
}

fn number_lines(lines: &[&str], first: usize) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let shown = if line.len() > MAX_LINE_CHARS {
            format!("{}... [line truncated]", &line[..floor_boundary(line, MAX_LINE_CHARS)])
        } else {
            line.to_string()
        };
        out.push_str(&format!("{:>6}\t{}\n", first + i, shown));
    }
    out
}

pub async fn read(env: &ToolEnv, input: &Value) -> ToolOutput {
    let raw = match required(input, "file_path") {
        Ok(p) => p,
        Err(e) => return e,
    };
    let path = env.resolve(raw);
    let meta = match tokio::fs::metadata(&path).await {
        Ok(m) => m,
        Err(_) => return ToolOutput::err(format!("File does not exist: {}", path.display())),
    };
    if meta.is_dir() {
        return ToolOutput::err(format!(
            "{} is a directory. Use Glob to list files, or Bash with `ls`.",
            path.display()
        ));
    }
    if meta.len() > MAX_READ_BYTES {
        return ToolOutput::err(format!(
            "{} is {} bytes, over the 50 MB read limit. Use Bash with `head`, `tail`, or `sed -n` to read part of it.",
            path.display(),
            meta.len()
        ));
    }
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) => return ToolOutput::err(format!("Cannot read {}: {e}", path.display())),
    };
    if looks_binary(&bytes) {
        env.mark_read(&path);
        return ToolOutput::ok(format!(
            "{} is a binary file ({} bytes) and cannot be shown as text.",
            path.display(),
            bytes.len()
        ));
    }
    env.mark_read(&path);
    let text = String::from_utf8_lossy(&bytes);
    if text.is_empty() {
        return ToolOutput::ok(format!("(The file {} exists and is empty.)", path.display()));
    }
    let lines: Vec<&str> = text.split_inclusive('\n').map(|l| l.strip_suffix('\n').unwrap_or(l)).collect();
    let total = lines.len();
    let offset = u64_arg(input, "offset").map(|o| o.max(1) as usize).unwrap_or(1);
    let limit = u64_arg(input, "limit").map(|l| l.max(1) as usize).unwrap_or(DEFAULT_LINES);
    if offset > total {
        return ToolOutput::ok(format!(
            "(The file has {total} lines; offset {offset} is past the end.)"
        ));
    }
    let end = (offset - 1 + limit).min(total);
    let mut out = number_lines(&lines[offset - 1..end], offset);
    if end < total || offset > 1 {
        out.push_str(&format!(
            "\n(Showing lines {offset} to {end} of {total}. Use offset and limit to read other lines.)\n"
        ));
    }
    ToolOutput::ok(out)
}

pub async fn write(env: &ToolEnv, input: &Value) -> ToolOutput {
    let raw = match required(input, "file_path") {
        Ok(p) => p,
        Err(e) => return e,
    };
    let content = match required(input, "content") {
        Ok(c) => c,
        Err(e) => return e,
    };
    let path = env.resolve(raw);
    if path.is_dir() {
        return ToolOutput::err(format!("{} is a directory.", path.display()));
    }
    let old = if path.exists() {
        if !env.has_read(&path) {
            return ToolOutput::err(format!(
                "{} already exists and has not been read in this run. Read it first, then write it, so you do not overwrite content you have not seen.",
                path.display()
            ));
        }
        Some(tokio::fs::read_to_string(&path).await.unwrap_or_default())
    } else {
        None
    };
    if let Err(e) = write_file(&path, content).await {
        return ToolOutput::err(e);
    }
    env.mark_read(&path);
    let shown = path.display().to_string();
    let diff = unified_diff(&shown, old.as_deref().unwrap_or(""), content);
    let text = match old {
        Some(_) => format!("The file {shown} has been updated."),
        None => format!("File created successfully at: {shown}"),
    };
    ToolOutput { diff, ..ToolOutput::ok(text) }
}

pub(crate) async fn write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    tokio::fs::write(path, content).await.map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

fn snippet(content: &str, byte_pos: usize, inserted_lines: usize) -> String {
    let line_idx = content[..byte_pos].matches('\n').count();
    let lines: Vec<&str> = content.lines().collect();
    let start = line_idx.saturating_sub(4);
    let end = (line_idx + inserted_lines + 4).min(lines.len());
    number_lines(&lines[start..end], start + 1)
}

pub async fn edit(env: &ToolEnv, input: &Value) -> ToolOutput {
    let raw = match required(input, "file_path") {
        Ok(p) => p,
        Err(e) => return e,
    };
    let old_string = match required(input, "old_string") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let new_string = match required(input, "new_string") {
        Ok(s) => s,
        Err(e) => return e,
    };
    let replace_all = bool_arg(input, "replace_all").unwrap_or(false);
    let path = env.resolve(raw);
    if old_string == new_string {
        return ToolOutput::err("old_string and new_string are identical, so there is nothing to change.");
    }
    if !path.exists() {
        if old_string.is_empty() {
            if let Err(e) = write_file(&path, new_string).await {
                return ToolOutput::err(e);
            }
            env.mark_read(&path);
            let shown = path.display().to_string();
            return ToolOutput {
                diff: unified_diff(&shown, "", new_string),
                ..ToolOutput::ok(format!("File created successfully at: {shown}"))
            };
        }
        return ToolOutput::err(format!("File does not exist: {}", path.display()));
    }
    if !env.has_read(&path) {
        return ToolOutput::err(format!(
            "{} has not been read in this run. Read it first, then edit it.",
            path.display()
        ));
    }
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) => return ToolOutput::err(format!("Cannot read {}: {e}", path.display())),
    };
    if old_string.is_empty() {
        return ToolOutput::err("old_string is empty but the file already exists. Name the exact text to replace.");
    }
    let count = content.matches(old_string).count();
    if count == 0 {
        return ToolOutput::err(format!(
            "old_string was not found in {}. It must match the file exactly, including whitespace and indentation. Read the file again to copy the current text.",
            path.display()
        ));
    }
    if count > 1 && !replace_all {
        return ToolOutput::err(format!(
            "old_string matches {count} places in {}. Add surrounding lines so it matches exactly one, or set replace_all to change every match.",
            path.display()
        ));
    }
    let first = content.find(old_string).unwrap_or(0);
    let updated = if replace_all {
        content.replace(old_string, new_string)
    } else {
        content.replacen(old_string, new_string, 1)
    };
    if let Err(e) = write_file(&path, &updated).await {
        return ToolOutput::err(e);
    }
    let shown = path.display().to_string();
    let text = if replace_all && count > 1 {
        format!("The file {shown} has been updated. All {count} matches were replaced.")
    } else {
        format!(
            "The file {shown} has been updated. Result around the change:\n{}",
            snippet(&updated, first, new_string.matches('\n').count() + 1)
        )
    };
    ToolOutput { diff: unified_diff(&shown, &content, &updated), ..ToolOutput::ok(text) }
}

#[cfg(test)]
mod tests {
    use crate::testutil::{env_in, run};
    use serde_json::json;

    #[tokio::test]
    async fn read_numbers_lines_and_pages() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let f = env.config().repo_root.join("a.txt");
        std::fs::write(&f, "one\ntwo\nthree\n").unwrap();
        let out = run(&env, "Read", json!({"file_path": f})).await;
        assert!(!out.is_error);
        assert_eq!(out.text, "     1\tone\n     2\ttwo\n     3\tthree\n");
        let out = run(&env, "Read", json!({"file_path": "a.txt", "offset": 2, "limit": 1})).await;
        assert!(out.text.starts_with("     2\ttwo\n"));
        assert!(out.text.contains("Showing lines 2 to 2 of 3"));
        let out = run(&env, "Read", json!({"file_path": "missing.txt"})).await;
        assert!(out.is_error);
        let out = run(&env, "Read", json!({"file_path": "."})).await;
        assert!(out.is_error && out.text.contains("directory"));
    }

    #[tokio::test]
    async fn read_truncates_long_lines_and_detects_binary() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let root = env.config().repo_root.clone();
        std::fs::write(root.join("long.txt"), "x".repeat(5000)).unwrap();
        let out = run(&env, "Read", json!({"file_path": "long.txt"})).await;
        assert!(out.text.contains("[line truncated]"));
        assert!(out.text.len() < 2100);
        std::fs::write(root.join("bin"), [0u8, 1, 2, 3]).unwrap();
        let out = run(&env, "Read", json!({"file_path": "bin"})).await;
        assert!(out.text.contains("binary"));
        std::fs::write(root.join("empty"), "").unwrap();
        let out = run(&env, "Read", json!({"file_path": "empty"})).await;
        assert!(out.text.contains("empty"));
    }

    #[tokio::test]
    async fn write_requires_prior_read_of_existing_file() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let out = run(&env, "Write", json!({"file_path": "new/dir/f.txt", "content": "hi\n"})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("created"));
        assert!(out.diff.is_some());
        let existing = env.config().repo_root.join("e.txt");
        std::fs::write(&existing, "old").unwrap();
        let out = run(&env, "Write", json!({"file_path": "e.txt", "content": "new"})).await;
        assert!(out.is_error);
        run(&env, "Read", json!({"file_path": "e.txt"})).await;
        let out = run(&env, "Write", json!({"file_path": "e.txt", "content": "new"})).await;
        assert!(!out.is_error);
        assert_eq!(std::fs::read_to_string(existing).unwrap(), "new");
    }

    #[tokio::test]
    async fn edit_requires_unique_match() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let f = env.config().repo_root.join("code.rs");
        std::fs::write(&f, "let a = 1;\nlet b = 1;\nlet c = 2;\n").unwrap();
        let edit = |old: &str, new: &str, all: bool| json!({"file_path": "code.rs", "old_string": old, "new_string": new, "replace_all": all});
        let out = run(&env, "Edit", edit("= 2", "= 3", false)).await;
        assert!(out.is_error && out.text.contains("not been read"));
        run(&env, "Read", json!({"file_path": "code.rs"})).await;
        let out = run(&env, "Edit", edit("= 1", "= 9", false)).await;
        assert!(out.is_error && out.text.contains("matches 2 places"));
        let out = run(&env, "Edit", edit("missing", "x", false)).await;
        assert!(out.is_error && out.text.contains("not found"));
        let out = run(&env, "Edit", edit("= 2", "= 2", false)).await;
        assert!(out.is_error && out.text.contains("identical"));
        let out = run(&env, "Edit", edit("let c = 2;", "let c = 3;", false)).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("let c = 3;"));
        assert!(out.diff.as_deref().unwrap().contains("+let c = 3;"));
        let out = run(&env, "Edit", edit("= 1", "= 9", true)).await;
        assert!(!out.is_error);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "let a = 9;\nlet b = 9;\nlet c = 3;\n");
    }

    #[tokio::test]
    async fn edit_with_empty_old_string_creates_new_file() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let out = run(&env, "Edit", json!({"file_path": "n.txt", "old_string": "", "new_string": "x"})).await;
        assert!(!out.is_error);
        assert!(env.config().repo_root.join("n.txt").exists());
    }
}
