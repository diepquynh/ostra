//! OAuth 2.1 sign-in for remote MCP servers, per the MCP authorization spec: protected resource
//! metadata (RFC 9728), authorization server metadata (RFC 8414, OpenID discovery), dynamic
//! client registration (RFC 7591), PKCE with S256, and the `resource` parameter (RFC 8707).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthServer {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Vec<String>,
}

/// What discovery found for one server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    /// The resource indicator to request tokens for.
    pub resource: String,
    pub server: AuthServer,
    /// Scopes to request when the settings name none.
    pub scopes: Vec<String>,
    /// The server publishes no OAuth metadata, so the endpoints are the 2025-03-26 defaults.
    #[serde(default)]
    pub guessed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCredentials {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

impl Tokens {
    /// Expired, or expiring within a minute.
    pub fn expired(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|t| t - 60 <= now)
    }
}

/// `resource_metadata` and `scope` of a `WWW-Authenticate: Bearer` challenge.
pub fn parse_challenge(header: &str) -> (Option<String>, Option<String>) {
    let param = |name: &str| -> Option<String> {
        let lower = header.to_ascii_lowercase();
        let mut from = 0;
        while let Some(i) = lower[from..].find(name) {
            let start = from + i;
            let boundary = start == 0
                || !lower.as_bytes()[start - 1].is_ascii_alphanumeric()
                    && lower.as_bytes()[start - 1] != b'_';
            let rest = header[start + name.len()..].trim_start();
            if boundary && let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim_start();
                return Some(match rest.strip_prefix('"') {
                    Some(q) => q.split('"').next().unwrap_or("").to_string(),
                    None => rest.split([',', ' ']).next().unwrap_or("").to_string(),
                });
            }
            from = start + name.len();
        }
        None
    };
    (param("resource_metadata"), param("scope"))
}

fn origin(u: &Url) -> String {
    let mut s = format!("{}://{}", u.scheme(), u.host_str().unwrap_or(""));
    if let Some(p) = u.port() {
        s.push_str(&format!(":{p}"));
    }
    s
}

fn is_loopback(u: &Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// An OAuth URL Ostra fetches or sends the browser to must be `https`, or plain `http` on a
/// loopback host or on the MCP server's own origin (which the user chose). Anything else, such
/// as a `javascript:` URL from a hostile server's metadata, is refused, because the browser would
/// run it on Ostra's origin.
pub fn check_endpoint(what: &str, url: &str, server_origin: Option<&str>) -> Result<(), String> {
    let u = Url::parse(url).map_err(|e| format!("the {what} `{url}` is not a URL: {e}"))?;
    let ok = match u.scheme() {
        "https" => u.host().is_some(),
        "http" => is_loopback(&u) || server_origin.is_some_and(|o| o == origin(&u)),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "refused the {what} `{url}`: OAuth endpoints must use https, because the sign-in link opens in the browser and carries codes and secrets"
        ))
    }
}

fn check_server(server: &AuthServer, server_origin: &str) -> Result<(), String> {
    let o = Some(server_origin);
    check_endpoint("authorization endpoint", &server.authorization_endpoint, o)?;
    check_endpoint("token endpoint", &server.token_endpoint, o)?;
    if let Some(r) = &server.registration_endpoint {
        check_endpoint("registration endpoint", r, o)?;
    }
    Ok(())
}

fn same_issuer(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

fn path_of(u: &Url) -> String {
    let p = u.path().trim_end_matches('/');
    if p.is_empty() {
        String::new()
    } else {
        p.to_string()
    }
}

async fn get_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let resp = http
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// Find the authorization server for `server_url`, starting from its 401 challenge if any.
pub async fn discover(
    http: &reqwest::Client,
    server_url: &str,
    challenge: Option<&str>,
) -> Result<Discovery, String> {
    let url = Url::parse(server_url).map_err(|e| format!("`{server_url}` is not a URL: {e}"))?;
    let (metadata_url, challenge_scope) = challenge.map(parse_challenge).unwrap_or((None, None));
    let base = origin(&url);
    let mut candidates = vec![];
    if let Some(m) = metadata_url {
        check_endpoint("resource metadata URL", &m, Some(&base))?;
        candidates.push(m);
    }
    let path = path_of(&url);
    if !path.is_empty() {
        candidates.push(format!("{base}/.well-known/oauth-protected-resource{path}"));
    }
    candidates.push(format!("{base}/.well-known/oauth-protected-resource"));
    let mut prm = None;
    for c in &candidates {
        if let Some(v) = get_json(http, c).await
            && v.get("authorization_servers").is_some()
        {
            prm = Some(v);
            break;
        }
    }
    let mut resource = server_url
        .split('#')
        .next()
        .unwrap_or(server_url)
        .to_string();
    let mut scopes: Vec<String> = challenge_scope
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    let issuer = match &prm {
        Some(p) => {
            if let Some(r) = p.get("resource").and_then(Value::as_str) {
                resource = r.to_string();
            }
            if scopes.is_empty() {
                scopes = strings(p.get("scopes_supported"));
            }
            p.get("authorization_servers")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .ok_or("the server's resource metadata names no authorization server")?
                .to_string()
        }
        // Servers from before RFC 9728 support host their own authorization server.
        None => base.clone(),
    };
    check_endpoint("authorization server", &issuer, Some(&base))?;
    let found = authorization_server(http, &issuer).await?;
    let guessed = found.is_none();
    let server = match found {
        Some(s) => s,
        None if prm.is_none() => AuthServer {
            issuer: base.clone(),
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            registration_endpoint: Some(format!("{base}/register")),
            scopes_supported: vec![],
            token_endpoint_auth_methods_supported: vec![],
        },
        None => {
            return Err(format!(
                "could not read the metadata of the authorization server {issuer}"
            ));
        }
    };
    check_server(&server, &base)?;
    if scopes.is_empty() {
        scopes = server.scopes_supported.clone();
    }
    Ok(Discovery {
        resource,
        server,
        scopes,
        guessed,
    })
}

/// The authorization server's metadata. `Err` when metadata names a different issuer, because
/// RFC 8414 section 3.3 makes the client refuse it: another server could otherwise hand out
/// endpoints for this one.
async fn authorization_server(
    http: &reqwest::Client,
    issuer: &str,
) -> Result<Option<AuthServer>, String> {
    let Ok(url) = Url::parse(issuer) else {
        return Ok(None);
    };
    let base = origin(&url);
    let path = path_of(&url);
    let candidates = if path.is_empty() {
        vec![
            format!("{base}/.well-known/oauth-authorization-server"),
            format!("{base}/.well-known/openid-configuration"),
        ]
    } else {
        vec![
            format!("{base}/.well-known/oauth-authorization-server{path}"),
            format!("{base}/.well-known/openid-configuration{path}"),
            format!("{base}{path}/.well-known/openid-configuration"),
        ]
    };
    for c in candidates {
        if let Some(v) = get_json(http, &c).await
            && let Ok(s) = serde_json::from_value::<AuthServer>(v)
        {
            if !same_issuer(&s.issuer, issuer) {
                return Err(format!(
                    "refused the metadata at {c}: it names issuer `{}` instead of `{issuer}`",
                    s.issuer
                ));
            }
            return Ok(Some(s));
        }
    }
    Ok(None)
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}

/// Register Ostra as a public client for `redirect_uri`.
pub async fn register(
    http: &reqwest::Client,
    server: &AuthServer,
    redirect_uri: &str,
) -> Result<ClientCredentials, String> {
    let endpoint = server.registration_endpoint.as_deref().ok_or(
        "the authorization server does not let clients register; set `oauth.client_id` for this server",
    )?;
    check_endpoint("registration endpoint", endpoint, Some(&issuer_origin(server)))?;
    let body = serde_json::json!({
        "client_name": "Ostra",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let resp = http
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("client registration failed: {e}"))?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(format!(
            "client registration was refused ({status}): {}",
            error_text(&v)
        ));
    }
    let client_id = v
        .get("client_id")
        .and_then(Value::as_str)
        .ok_or("client registration returned no client_id")?
        .to_string();
    Ok(ClientCredentials {
        client_id,
        client_secret: v
            .get("client_secret")
            .and_then(Value::as_str)
            .map(String::from),
    })
}

/// A PKCE verifier and its S256 challenge.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn pkce() -> Pkce {
    let verifier = random_token();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
    }
}

/// 256 random bits, URL-safe. Used for PKCE verifiers and `state`.
pub fn random_token() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    URL_SAFE_NO_PAD.encode(bytes)
}

pub struct AuthRequest<'a> {
    pub discovery: &'a Discovery,
    pub client: &'a ClientCredentials,
    pub redirect_uri: &'a str,
    pub scopes: &'a [String],
    pub state: &'a str,
    pub challenge: &'a str,
}

fn issuer_origin(server: &AuthServer) -> String {
    Url::parse(&server.issuer)
        .map(|u| origin(&u))
        .unwrap_or_default()
}

pub fn authorization_url(r: &AuthRequest<'_>) -> Result<String, String> {
    // Discovery records saved before endpoints were checked are checked again here.
    let server = &r.discovery.server;
    check_endpoint(
        "authorization endpoint",
        &server.authorization_endpoint,
        Some(&issuer_origin(server)),
    )?;
    let mut u = Url::parse(&server.authorization_endpoint)
        .map_err(|e| format!("the authorization endpoint is not a URL: {e}"))?;
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &r.client.client_id)
            .append_pair("redirect_uri", r.redirect_uri)
            .append_pair("state", r.state)
            .append_pair("code_challenge", r.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("resource", &r.discovery.resource);
        if !r.scopes.is_empty() {
            q.append_pair("scope", &r.scopes.join(" "));
        }
    }
    Ok(u.into())
}

/// Trade an authorization code for tokens.
pub async fn exchange_code(
    http: &reqwest::Client,
    discovery: &Discovery,
    client: &ClientCredentials,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    now: i64,
) -> Result<Tokens, String> {
    let form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("code_verifier", verifier.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("resource", discovery.resource.clone()),
    ];
    token_request(http, discovery, client, form, None, now).await
}

/// Use a refresh token. A server that issues no new refresh token keeps the old one valid.
pub async fn refresh(
    http: &reqwest::Client,
    discovery: &Discovery,
    client: &ClientCredentials,
    refresh_token: &str,
    now: i64,
) -> Result<Tokens, String> {
    let form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
        ("resource", discovery.resource.clone()),
    ];
    token_request(http, discovery, client, form, Some(refresh_token), now).await
}

async fn token_request(
    http: &reqwest::Client,
    discovery: &Discovery,
    client: &ClientCredentials,
    mut form: Vec<(&'static str, String)>,
    old_refresh: Option<&str>,
    now: i64,
) -> Result<Tokens, String> {
    let mut req = http
        .post(&discovery.server.token_endpoint)
        .header(reqwest::header::ACCEPT, "application/json");
    match &client.client_secret {
        Some(secret)
            if discovery
                .server
                .token_endpoint_auth_methods_supported
                .iter()
                .any(|m| m == "client_secret_basic")
                && !discovery
                    .server
                    .token_endpoint_auth_methods_supported
                    .iter()
                    .any(|m| m == "client_secret_post") =>
        {
            req = req.basic_auth(&client.client_id, Some(secret));
        }
        Some(secret) => {
            form.push(("client_id", client.client_id.clone()));
            form.push(("client_secret", secret.clone()));
        }
        None => form.push(("client_id", client.client_id.clone())),
    }
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form.iter().map(|(k, v)| (*k, v.as_str())))
        .finish();
    let resp = req
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await
        .map_err(|e| format!("the token request failed: {e}"))?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(format!(
            "the token request was refused ({status}): {}",
            error_text(&v)
        ));
    }
    let access_token = v
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or("the token response carried no access_token")?
        .to_string();
    Ok(Tokens {
        access_token,
        refresh_token: v
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| old_refresh.map(String::from)),
        expires_at: v.get("expires_in").and_then(Value::as_i64).map(|s| now + s),
    })
}

fn error_text(v: &Value) -> String {
    let e = v
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("no error code");
    match v.get("error_description").and_then(Value::as_str) {
        Some(d) => format!("{e}: {d}"),
        None => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenges() {
        let (m, s) = parse_challenge(
            r#"Bearer error="invalid_token", resource_metadata="https://x.example/.well-known/oauth-protected-resource", scope="read write""#,
        );
        assert_eq!(
            m.as_deref(),
            Some("https://x.example/.well-known/oauth-protected-resource")
        );
        assert_eq!(s.as_deref(), Some("read write"));
        assert_eq!(parse_challenge("Bearer realm=x"), (None, None));
    }

    #[test]
    fn pkce_is_s256() {
        let p = pkce();
        assert!(p.verifier.len() >= 43);
        assert_eq!(
            p.challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes()))
        );
    }
}
