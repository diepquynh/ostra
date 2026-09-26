use crate::text::truncate_end;
use crate::{ToolEnv, ToolOutput, required, str_arg};
use serde_json::Value;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 100_000;
const MAX_REDIRECTS: usize = 10;

/// The WebFetch client. Redirects are followed only within one host, because the policy checked
/// only the first URL; a redirect elsewhere comes back to the model as a new URL to fetch.
pub(crate) fn client(private_hosts: Arc<Vec<String>>) -> reqwest::Client {
    let redirect = reqwest::redirect::Policy::custom(|attempt| {
        let same_host = attempt.previous().last().is_some_and(|prev| {
            prev.host_str() == attempt.url().host_str()
                && prev.port_or_known_default() == attempt.url().port_or_known_default()
        });
        if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if same_host && matches!(attempt.url().scheme(), "http" | "https") {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });
    reqwest::Client::builder()
        .redirect(redirect)
        .dns_resolver(PublicResolver { private_hosts })
        // A proxy resolves the name itself, which would skip the resolver's address check.
        .no_proxy()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("ostra/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

/// Resolves names to public addresses only, so a name cannot point WebFetch at this machine or
/// the local network, including by changing its DNS answer between the check and the connect.
struct PublicResolver {
    private_hosts: Arc<Vec<String>>,
}

impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_ascii_lowercase();
        let allowed = self.private_hosts.contains(&host);
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| allowed || !is_private(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(
                    format!("{host} resolves only to private or loopback addresses").into(),
                );
            }
            let addrs: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(addrs)
        })
    }
}

/// Loopback, private, link-local, CGNAT, unique-local, multicast, and reserved ranges.
pub(crate) fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => private_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return private_v4(v4);
            }
            let s = v6.segments();
            // `::a.b.c.d` (IPv4-compatible) carries an IPv4 address too.
            if s[..6] == [0, 0, 0, 0, 0, 0] && (s[6] != 0 || s[7] > 1) {
                let [a, b] = s[6].to_be_bytes();
                let [c, d] = s[7].to_be_bytes();
                return private_v4(Ipv4Addr::new(a, b, c, d));
            }
            // Local-use NAT64 (`64:ff9b:1::/48`) translates to addresses the network chooses.
            if s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1 {
                return true;
            }
            let nat64 = s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0];
            if nat64 {
                let [a, b] = s[6].to_be_bytes();
                let [c, d] = s[7].to_be_bytes();
                return private_v4(Ipv4Addr::new(a, b, c, d));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xffc0) == 0xfec0
        }
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (b & 0xc0) == 64)
        || (a == 198 && (b & 0xfe) == 18)
        || a >= 240
}

/// A URL whose host is a private IP literal, which the resolver never sees.
fn private_literal(url: &reqwest::Url, private_hosts: &[String]) -> bool {
    let ip = match url.host() {
        Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip),
        Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip),
        _ => return false,
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    is_private(ip) && !private_hosts.iter().any(|h| h == &host || h == bare)
}

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
    if private_literal(&url, &env.private_hosts) {
        return private_refusal(&url);
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
        Err(e) if is_resolver_refusal(&e) => return private_refusal(&url),
        Err(e) => return ToolOutput::err(format!("Fetching {url} failed: {e}")),
    };
    let status = resp.status();
    let final_url = resp.url().clone();
    if status.is_redirection()
        && let Some(next) = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|loc| final_url.join(loc).ok())
    {
        return ToolOutput::ok(format!(
            "Fetch {next} with a new WebFetch call to follow it, because {final_url} redirected (HTTP {}) to another host and a redirect to another host is not followed.",
            status.as_u16()
        ));
    }
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

fn private_refusal(url: &reqwest::Url) -> ToolOutput {
    let host = url.host_str().unwrap_or_default();
    ToolOutput::err(format!(
        "Add the allow rule `WebFetch(domain:{host})` to fetch {url}, because it points at a private or loopback address, which WebFetch refuses by default."
    ))
}

fn is_resolver_refusal(e: &reqwest::Error) -> bool {
    let mut source: Option<&dyn std::error::Error> = Some(e);
    while let Some(err) = source {
        if err.to_string().contains("resolves only to private") {
            return true;
        }
        source = err.source();
    }
    false
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

/// Hosts that exact `WebFetch(domain:<host>)` allow rules name, which may resolve to local addresses.
pub fn webfetch_hosts(allow: &[String]) -> Vec<String> {
    allow
        .iter()
        .filter_map(|r| r.trim().strip_prefix("WebFetch(domain:")?.strip_suffix(')'))
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{env_in, run};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn local_env(d: &std::path::Path) -> crate::ToolEnv {
        env_in(d).with_private_hosts(vec!["127.0.0.1".into()])
    }

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
        let env = local_env(d.path());
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
        let env = local_env(d.path());
        let url = serve_once(
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nnope",
        )
        .await;
        let out = run(&env, "WebFetch", json!({"url": url})).await;
        assert!(out.is_error && out.text.contains("HTTP 404"));
    }

    #[tokio::test]
    async fn refuses_private_addresses_unless_a_rule_names_the_host() {
        let d = tempfile::tempdir().unwrap();
        let env = env_in(d.path());
        let url = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nsecret",
        )
        .await;
        for u in [
            url.clone(),
            "http://[::1]:9/".into(),
            "http://169.254.169.254/latest/meta-data/".into(),
            "http://[::ffff:127.0.0.1]:9/".into(),
            url.replace("127.0.0.1", "localhost"),
        ] {
            let out = run(&env, "WebFetch", json!({"url": u})).await;
            assert!(
                out.is_error && out.text.contains("private or loopback"),
                "{u}: {}",
                out.text
            );
        }
        let out = run(&local_env(d.path()), "WebFetch", json!({"url": url})).await;
        assert!(!out.is_error && out.text.contains("secret"), "{}", out.text);
    }

    #[tokio::test]
    async fn redirect_to_another_host_is_returned_not_followed() {
        let d = tempfile::tempdir().unwrap();
        let target = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\ninternal",
        )
        .await;
        let body: &'static str = Box::leak(
            format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .into_boxed_str(),
        );
        let start = serve_once(body).await.replace("127.0.0.1", "localhost");
        let env = env_in(d.path()).with_private_hosts(vec!["localhost".into()]);
        let out = run(&env, "WebFetch", json!({"url": start})).await;
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text
                .starts_with(&format!("Fetch {target} with a new WebFetch call")),
            "{}",
            out.text
        );
        assert!(!out.text.contains("internal"));
    }

    #[test]
    fn private_ranges() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "64:ff9b:1::1",
            "::127.0.0.1",
            "::a00:1",
        ] {
            assert!(is_private(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111", "100.128.0.1"] {
            assert!(!is_private(ip.parse().unwrap()), "{ip}");
        }
    }
}

