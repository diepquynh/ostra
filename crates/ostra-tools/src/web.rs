use crate::text::truncate_end;
use crate::{ToolEnv, ToolOutput, required, str_arg};
use serde_json::Value;

const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 100_000;

pub async fn fetch(env: &ToolEnv, input: &Value) -> ToolOutput {
    let raw = match required(input, "url") {
        Ok(u) => u,
        Err(e) => return e,
    };
    let url = match reqwest::Url::parse(raw.trim()) {
        Ok(u) => u,
        Err(e) => return ToolOutput::err(format!("Invalid URL `{raw}`: {e}")),
    };
    if !matches!(url.scheme(), "http" | "https") {
        return ToolOutput::err(format!(
            "Only http and https URLs can be fetched, not `{}`.",
            url.scheme()
        ));
    }
    let resp = match env
        .http
        .get(url.clone())
        .header(
            "accept",
            "text/html, text/markdown, text/plain, application/json;q=0.9, */*;q=0.5",
        )
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return ToolOutput::err(format!("Fetching {url} failed: {e}")),
    };
    let status = resp.status();
    let final_url = resp.url().clone();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let body = match read_capped(resp).await {
        Ok(b) => b,
        Err(e) => return ToolOutput::err(format!("Reading {url} failed: {e}")),
    };
    let text = match to_text(&content_type, &body) {
        Ok(t) => t,
        Err(e) => return ToolOutput::err(e),
    };
    let text = truncate_end(&text, MAX_TEXT_CHARS);
    let mut out = String::new();
    if let Some(prompt) = str_arg(input, "prompt").filter(|p| !p.trim().is_empty()) {
        out.push_str(&format!("Prompt: {prompt}\n\n"));
    }
    if final_url != url {
        out.push_str(&format!("Redirected to: {final_url}\n"));
    }
    out.push_str(&format!(
        "Content of {final_url} (HTTP {}):\n\n{text}",
        status.as_u16()
    ));
    if status.is_success() {
        ToolOutput::ok(out)
    } else {
        ToolOutput::err(out)
    }
}

async fn read_capped(mut resp: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY_BYTES {
            body.truncate(MAX_BODY_BYTES);
            break;
        }
    }
    Ok(body)
}

fn to_text(content_type: &str, body: &[u8]) -> Result<String, String> {
    let text = String::from_utf8_lossy(body);
    let sniff_html = || {
        let head: String = text
            .chars()
            .take(512)
            .collect::<String>()
            .to_ascii_lowercase();
        head.contains("<html") || head.contains("<!doctype html")
    };
    if content_type.contains("html") || (content_type.is_empty() && sniff_html()) {
        return htmd::convert(&text)
            .map_err(|e| format!("Converting HTML to markdown failed: {e}"));
    }
    let textual = content_type.is_empty()
        || content_type.starts_with("text/")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("javascript")
        || content_type.contains("yaml")
        || content_type.contains("markdown");
    if textual && !body[..body.len().min(8192)].contains(&0) {
        return Ok(text.into_owned());
    }
    Err(format!(
        "The response is `{content_type}`, which is not text, so it cannot be shown."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{env_in, run};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve_once(response: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf).await;
            s.write_all(response.as_bytes()).await.unwrap();
            s.shutdown().await.unwrap();
        });
        format!("http://{addr}/page")
    }

    #[tokio::test]
    async fn converts_html_to_markdown() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let url = serve_once("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h1>Title</h1><p>Some <b>bold</b> text</p></body></html>").await;
        let out = run(
            &env,
            "WebFetch",
            json!({"url": url, "prompt": "What is the title?"}),
        )
        .await;
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Prompt: What is the title?"));
        assert!(out.text.contains("# Title"), "{}", out.text);
        assert!(out.text.contains("**bold**"));
    }

    #[tokio::test]
    async fn refuses_other_schemes_and_binary() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let out = run(&env, "WebFetch", json!({"url": "file:///etc/passwd"})).await;
        assert!(out.is_error && out.text.contains("http and https"));
        assert!(to_text("image/png", &[0x89, 0x50, 0]).is_err());
        assert_eq!(
            to_text("application/json", b"{\"a\":1}").unwrap(),
            "{\"a\":1}"
        );
    }

    #[tokio::test]
    async fn reports_http_errors() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let url = serve_once(
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nnope",
        )
        .await;
        let out = run(&env, "WebFetch", json!({"url": url})).await;
        assert!(out.is_error && out.text.contains("HTTP 404"));
    }
}
