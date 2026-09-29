//! A one-shot HTTP server that plays recorded responses in order and captures requests.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct Recorded {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

impl Recorded {
    pub fn sse(body: &str) -> Self {
        Recorded {
            status: 200,
            headers: vec![("content-type", "text/event-stream".into())],
            body: body.to_string(),
        }
    }

    pub fn json(body: &serde_json::Value) -> Self {
        Recorded::error(200, &body.to_string())
    }

    pub fn error(status: u16, body: &str) -> Self {
        Recorded {
            status,
            headers: vec![("content-type", "application/json".into())],
            body: body.to_string(),
        }
    }

    pub fn with_header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_string()));
        self
    }
}

#[derive(Debug, Clone)]
pub struct Captured {
    pub head: String,
    pub body: serde_json::Value,
}

impl Captured {
    pub fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    }
}

pub async fn serve(responses: Vec<Recorded>) -> (String, Arc<Mutex<Vec<Captured>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(vec![]));
    let cap = captured.clone();
    tokio::spawn(async move {
        for r in responses {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![];
            let mut tmp = [0u8; 8192];
            let (head, body) = loop {
                let n = sock.read(&mut tmp).await.unwrap();
                buf.extend_from_slice(&tmp[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(split) = text.find("\r\n\r\n") {
                    let head = text[..split].to_string();
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if buf.len() >= split + 4 + len {
                        break (
                            head,
                            String::from_utf8_lossy(&buf[split + 4..split + 4 + len]).to_string(),
                        );
                    }
                }
                if n == 0 {
                    break (text, String::new());
                }
            };
            cap.lock().unwrap().push(Captured {
                head,
                body: serde_json::from_str(&body).unwrap_or_default(),
            });
            let mut out = format!(
                "HTTP/1.1 {} X\r\nconnection: close\r\ncontent-length: {}\r\n",
                r.status,
                r.body.len()
            );
            for (k, v) in &r.headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            out.push_str(&r.body);
            sock.write_all(out.as_bytes()).await.unwrap();
            sock.shutdown().await.ok();
        }
    });
    (format!("http://{addr}"), captured)
}

pub fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
