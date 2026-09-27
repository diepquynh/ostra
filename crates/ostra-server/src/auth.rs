//! Browser auth (HANDOVER 15): a one-time token in the URL fragment, exchanged once for an
//! `HttpOnly`, `SameSite=Strict` cookie. Host and Origin checks block DNS rebinding and foreign
//! pages. Tokens and cookies are stored hashed in the registry, so a restart keeps sign-ins and
//! `ostra url` can mint a token for a running server. A sign-in lasts [`COOKIE_TTL_SECS`] and is
//! listed and revoked from the settings or with `ostra sessions`. A revoke from this server takes
//! effect at once; one from the CLI, another process, within [`RECHECK`].

use ostra_core::api::SignInSession;
use ostra_core::paths;
use ostra_store::RegistryDb;
use parking_lot::{Mutex, RwLock};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub const COOKIE: &str = "ostra_session";
const TOKEN_TTL_SECS: u64 = 15 * 60;
pub const COOKIE_TTL_SECS: u64 = 30 * 24 * 60 * 60;
/// How long a checked cookie is trusted before the registry is read again, which bounds how late
/// a revoke from the CLI lands.
pub const RECHECK: Duration = Duration::from_secs(5);
const COOKIE_KEY: &str = "auth:cookie:";
/// `last_seen` is written at most this often, so an active tab does not write on every request.
const SEEN_EVERY_SECS: u64 = 60;
/// Failed exchanges one address may make per [`FAILURE_WINDOW`] before it must wait.
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// A sign-in as stored under `auth:cookie:<sha256 of the cookie>`. Older servers stored only the
/// creation time as a number; [`parse_record`] reads both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    id: String,
    created: u64,
    last_seen: u64,
    #[serde(default)]
    user_agent: Option<String>,
    #[serde(default)]
    ip: Option<String>,
}

impl Record {
    fn expires(&self) -> u64 {
        self.created + COOKIE_TTL_SECS
    }
}

/// What the browser told the server when it signed in.
#[derive(Debug, Clone, Default)]
pub struct SignInMeta {
    pub user_agent: Option<String>,
    pub ip: Option<String>,
}

/// `(record, legacy)`: a legacy value holds only the creation time and needs rewriting.
fn parse_record(stored: &[u8]) -> Option<(Record, bool)> {
    if let Ok(r) = serde_json::from_slice::<Record>(stored) {
        return Some((r, false));
    }
    let created = std::str::from_utf8(stored)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some((
        Record {
            id: new_id(),
            created,
            last_seen: created,
            user_agent: None,
            ip: None,
        },
        true,
    ))
}

fn new_id() -> String {
    let bytes: [u8; 8] = rand::rng().random();
    format!("si_{}", hex::encode(bytes))
}

fn store_record(registry: &RegistryDb, cookie_hash: &str, r: &Record) -> anyhow::Result<()> {
    registry.kv_set(
        &format!("{COOKIE_KEY}{cookie_hash}"),
        serde_json::to_string(r)?.as_bytes(),
    )?;
    Ok(())
}

/// Every stored sign-in with its cookie hash. Legacy rows are rewritten with an id, and expired
/// ones are removed.
fn load_records(registry: &RegistryDb) -> anyhow::Result<Vec<(String, Record)>> {
    let mut out = vec![];
    for (key, value) in registry.kv_scan(COOKIE_KEY)? {
        let Some(h) = key.strip_prefix(COOKIE_KEY) else {
            continue;
        };
        let Some((record, legacy)) = parse_record(&value) else {
            registry.kv_delete(&key)?;
            continue;
        };
        if record.expires() <= now() {
            registry.kv_delete(&key)?;
            continue;
        }
        if legacy {
            store_record(registry, h, &record)?;
        }
        out.push((h.to_string(), record));
    }
    out.sort_by_key(|(_, r)| r.created);
    Ok(out)
}

fn to_view(r: &Record, current: bool) -> SignInSession {
    let at = |s: u64| chrono::DateTime::from_timestamp(s as i64, 0).unwrap_or_default();
    SignInSession {
        id: r.id.clone(),
        created: at(r.created),
        last_seen: at(r.last_seen),
        expires: at(r.expires()),
        user_agent: r.user_agent.clone(),
        ip: r.ip.clone(),
        current,
    }
}

pub struct Auth {
    registry: RegistryDb,
    port: u16,
    /// Exact `Host` header values the browser may send.
    hosts: Vec<String>,
    /// Host used in printed sign-in URLs.
    url_host: String,
    /// `127.0.0.1:port` and its aliases when a private name serves the browser instead: requests
    /// there are sent to the private name, because a cookie set on them reaches every local port.
    ip_hosts: Vec<String>,
    /// Cookie hash to (expiry, when the registry last confirmed it).
    cookies: RwLock<HashMap<String, (u64, Instant)>>,
    /// Cookie hashes of sign-ins revoked by this server, so open sockets close at once.
    revoked: broadcast::Sender<String>,
    /// Recent failed exchanges per address.
    failures: Mutex<HashMap<IpAddr, Vec<Instant>>>,
}

/// Where the server listens and which names reach it.
#[derive(Debug, Clone)]
pub struct Access {
    pub port: u16,
    pub bind: std::net::IpAddr,
    /// Extra names, for example a reverse proxy's domain. `name` or `name:port`.
    pub extra_hosts: Vec<String>,
    /// Also accept the Vite dev server's host.
    pub dev: bool,
    /// Serve a loopback-bound server at a private `ostra-….localhost` name.
    pub private_host: bool,
}

/// Interfaces that carry container or VM bridges rather than the machine's own address.
fn is_bridge(name: &str) -> bool {
    [
        "docker",
        "br-",
        "veth",
        "cni",
        "flannel",
        "virbr",
        "vnet",
        "tailscale",
        "lo",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

fn host_port(ip: std::net::IpAddr, port: u16) -> String {
    match ip {
        std::net::IpAddr::V4(v4) => format!("{v4}:{port}"),
        std::net::IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    }
}

/// True for loopback and for any address of this machine's own interfaces.
pub fn is_local_address(ip: std::net::IpAddr) -> bool {
    let ip = match ip {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    };
    ip.is_loopback()
        || if_addrs::get_if_addrs()
            .unwrap_or_default()
            .iter()
            .any(|i| i.ip() == ip)
}

/// The machine's host name, without a `.local` suffix (macOS often includes one).
fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `buf.len()` bytes into the buffer we own.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_lowercase();
    let name = name.strip_suffix(".local").unwrap_or(&name).to_string();
    (!name.is_empty()).then_some(name)
}

impl Access {
    pub fn loopback(port: u16) -> Self {
        Access {
            port,
            bind: std::net::IpAddr::from([127, 0, 0, 1]),
            extra_hosts: vec![],
            dev: false,
            private_host: false,
        }
    }

    pub fn is_remote(&self) -> bool {
        !self.bind.is_loopback()
    }

    /// Non-loopback addresses of the machine's own interfaces, IPv4 first.
    pub fn lan_addresses(&self) -> Vec<std::net::IpAddr> {
        if !self.bind.is_unspecified() {
            return if self.bind.is_loopback() {
                vec![]
            } else {
                vec![self.bind]
            };
        }
        let mut v: Vec<std::net::IpAddr> = if_addrs::get_if_addrs()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| !i.is_loopback() && !is_bridge(&i.name))
            .map(|i| i.ip())
            .filter(|ip| !matches!(ip, std::net::IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80))
            .collect();
        v.sort_by_key(|ip| ip.is_ipv6());
        v.dedup();
        v
    }

    /// Every `Host` header value that reaches this server.
    pub fn hosts(&self) -> Vec<String> {
        let port = self.port;
        let mut v = vec![
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ];
        if self.bind.is_unspecified() {
            // Bridge addresses count too: a container or VM on this machine may connect through one.
            for i in if_addrs::get_if_addrs().unwrap_or_default() {
                v.push(host_port(i.ip(), port));
            }
            if let Some(h) = hostname() {
                v.push(format!("{h}:{port}"));
                v.push(format!("{h}.local:{port}"));
            }
        } else if !self.bind.is_loopback() {
            v.push(host_port(self.bind, port));
        }
        for h in &self.extra_hosts {
            let h = h.trim().to_lowercase();
            if h.is_empty() {
                continue;
            }
            let has_port = h
                .rsplit_once(':')
                .is_some_and(|(_, p)| p.chars().all(|c| c.is_ascii_digit()))
                && !h.ends_with(']');
            if has_port {
                v.push(h);
            } else {
                // A reverse proxy on its default port sends the bare name.
                v.push(format!("{h}:{port}"));
                v.push(h);
            }
        }
        if self.dev {
            v.push("localhost:5173".into());
            v.push("127.0.0.1:5173".into());
        }
        v.iter_mut().for_each(|h| *h = h.to_lowercase());
        v.sort();
        v.dedup();
        v
    }

    /// Host for printed sign-in URLs: loopback when local, else the first LAN address.
    pub fn url_host(&self) -> String {
        match self.lan_addresses().first() {
            Some(ip) if self.is_remote() => host_port(*ip, self.port),
            _ => format!("127.0.0.1:{}", self.port),
        }
    }
}

/// A stable random `ostra-<hex>.localhost` name. Browsers resolve every `*.localhost` name to
/// this machine, and a cookie set on it is not sent to pages on `127.0.0.1` or other names.
fn private_name(registry: &RegistryDb) -> String {
    const KEY: &str = "server:private_host";
    if let Some(v) = registry.kv_get(KEY).ok().flatten()
        && let Ok(name) = String::from_utf8(v)
    {
        return name;
    }
    let bytes: [u8; 8] = rand::rng().random();
    let name = format!("ostra-{}.localhost", hex::encode(bytes));
    let _ = registry.kv_set(KEY, name.as_bytes());
    name
}

/// The registry key part for a cookie; the cookie itself is never stored.
pub fn cookie_hash(value: &str) -> String {
    hash(value)
}

fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn random_hex() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeError {
    Unknown,
    Used { seconds_ago: u64 },
    Expired { seconds_ago: u64 },
}

impl ExchangeError {
    pub fn message(&self) -> String {
        match self {
            ExchangeError::Unknown => {
                "This sign-in link was not issued by this server. Run `ostra url` on the machine running Ostra, as the same user.".into()
            }
            ExchangeError::Used { seconds_ago } => format!(
                "This sign-in link was already used {seconds_ago} s ago, so it cannot sign in again. Run `ostra url` for a new one."
            ),
            ExchangeError::Expired { .. } => {
                "This sign-in link expired; links last 15 minutes. Run `ostra url` for a new one.".into()
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ExchangeError::Unknown => "unknown",
            ExchangeError::Used { .. } => "used",
            ExchangeError::Expired { .. } => "expired",
        }
    }
}

pub fn mint_token(registry: &RegistryDb) -> anyhow::Result<String> {
    let token = random_hex();
    let expires = now() + TOKEN_TTL_SECS;
    registry.kv_set(
        &format!("auth:token:{}", hash(&token)),
        expires.to_string().as_bytes(),
    )?;
    Ok(token)
}

impl Auth {
    pub fn new(registry: RegistryDb, access: &Access) -> Self {
        if let Err(e) = load_records(&registry) {
            tracing::warn!("could not read the sign-ins: {e}");
        }
        let mut hosts = access.hosts();
        let mut url_host = access.url_host();
        let mut ip_hosts = vec![];
        if access.private_host && !access.is_remote() {
            let name = private_name(&registry);
            let port = access.port;
            ip_hosts = vec![
                format!("127.0.0.1:{port}"),
                format!("localhost:{port}"),
                format!("[::1]:{port}"),
            ];
            url_host = format!("{name}:{port}");
            hosts.push(url_host.clone());
        }
        Auth {
            registry,
            port: access.port,
            hosts,
            url_host,
            ip_hosts,
            cookies: RwLock::new(HashMap::new()),
            revoked: broadcast::channel(64).0,
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// For a request to `127.0.0.1:port` or an alias, the private host to send it to instead.
    pub fn private_redirect(&self, host: Option<&str>) -> Option<&str> {
        let host = host?;
        self.ip_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(host))
            .then_some(self.url_host.as_str())
    }

    /// Cookie hashes as this server revokes them.
    pub fn subscribe_revoked(&self) -> broadcast::Receiver<String> {
        self.revoked.subscribe()
    }

    /// Seconds `ip` must wait before another exchange, after [`MAX_FAILURES`] failures within
    /// [`FAILURE_WINDOW`]. Tokens are 256-bit, so this only stops a guessing loop from filling
    /// the log and the registry.
    /// Loopback peers are not limited: one-time tokens cannot be guessed, and counting their
    /// failures would let any local process lock the user out of signing in.
    pub fn exchange_wait(&self, ip: Option<IpAddr>) -> Option<u64> {
        let ip = ip.filter(|i| !i.is_loopback())?;
        let mut failures = self.failures.lock();
        let list = failures.entry(ip).or_default();
        list.retain(|t| t.elapsed() < FAILURE_WINDOW);
        (list.len() >= MAX_FAILURES).then(|| {
            let oldest = list.iter().min().copied().unwrap_or_else(Instant::now);
            FAILURE_WINDOW
                .saturating_sub(oldest.elapsed())
                .as_secs()
                .max(1)
        })
    }

    pub fn note_exchange(&self, ip: Option<IpAddr>, ok: bool) {
        let Some(ip) = ip.filter(|i| !i.is_loopback()) else {
            return;
        };
        let mut failures = self.failures.lock();
        if ok {
            failures.remove(&ip);
        } else {
            failures.entry(ip).or_default().push(Instant::now());
        }
    }

    /// Every live sign-in, oldest first, marking the one whose cookie is `current`.
    pub fn sessions(&self, current: Option<&str>) -> anyhow::Result<Vec<SignInSession>> {
        let mine = current.map(hash);
        Ok(load_records(&self.registry)?
            .iter()
            .map(|(h, r)| to_view(r, mine.as_deref() == Some(h.as_str())))
            .collect())
    }

    fn drop_sign_in(&self, cookie_hash: &str) -> anyhow::Result<()> {
        self.registry
            .kv_delete(&format!("{COOKIE_KEY}{cookie_hash}"))?;
        self.cookies.write().remove(cookie_hash);
        let _ = self.revoked.send(cookie_hash.to_string());
        Ok(())
    }

    /// Revoke the sign-in with this id. False when there is none.
    pub fn revoke(&self, id: &str) -> anyhow::Result<bool> {
        let found = load_records(&self.registry)?
            .into_iter()
            .find(|(_, r)| r.id == id);
        match found {
            Some((h, _)) => {
                self.drop_sign_in(&h)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Revoke every sign-in except the one whose cookie is `keep`.
    pub fn revoke_others(&self, keep: &str) -> anyhow::Result<usize> {
        let keep = hash(keep);
        let mut n = 0;
        for (h, _) in load_records(&self.registry)? {
            if h != keep {
                self.drop_sign_in(&h)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Revoke the sign-in this cookie belongs to.
    pub fn sign_out(&self, cookie: &str) -> anyhow::Result<()> {
        self.drop_sign_in(&hash(cookie))
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn sign_in_url(&self) -> anyhow::Result<String> {
        Ok(format!(
            "http://{}/#token={}",
            self.url_host,
            mint_token(&self.registry)?
        ))
    }

    /// Exchange a one-time token for a cookie value. A used token stays recorded as used, so a
    /// second attempt can say so rather than look like an unknown token.
    pub fn exchange(&self, token: &str, meta: &SignInMeta) -> Result<String, ExchangeError> {
        let key = format!("auth:token:{}", hash(token.trim()));
        let stored = self
            .registry
            .kv_get(&key)
            .ok()
            .flatten()
            .ok_or(ExchangeError::Unknown)?;
        let text = String::from_utf8(stored.clone()).map_err(|_| ExchangeError::Unknown)?;
        if let Some(at) = text.strip_prefix("used:") {
            let ago = now().saturating_sub(at.parse().unwrap_or(0));
            return Err(ExchangeError::Used { seconds_ago: ago });
        }
        let claimed = self
            .registry
            .kv_replace(&key, &stored, format!("used:{}", now()).as_bytes())
            .unwrap_or(false);
        if !claimed {
            return Err(ExchangeError::Used { seconds_ago: 0 });
        }
        let stored = text;
        let expires: u64 = stored.parse().map_err(|_| ExchangeError::Unknown)?;
        if expires < now() {
            return Err(ExchangeError::Expired {
                seconds_ago: now() - expires,
            });
        }
        let cookie = random_hex();
        let h = hash(&cookie);
        let created = now();
        let record = Record {
            id: new_id(),
            created,
            last_seen: created,
            user_agent: meta
                .user_agent
                .as_deref()
                .map(|u| u.chars().take(300).collect()),
            ip: meta.ip.clone(),
        };
        let _ = store_record(&self.registry, &h, &record);
        self.cookies
            .write()
            .insert(h, (created + COOKIE_TTL_SECS, Instant::now()));
        Ok(cookie)
    }

    /// A cookie is valid while its sign-in is in the registry and younger than the TTL.
    pub fn check_cookie(&self, value: &str) -> bool {
        let h = hash(value);
        if let Some((expires, checked)) = self.cookies.read().get(&h).copied()
            && checked.elapsed() < RECHECK
        {
            return expires > now();
        }
        let key = format!("{COOKIE_KEY}{h}");
        let raw = self.registry.kv_get(&key).ok().flatten();
        let record = raw.as_deref().and_then(parse_record);
        let expires = record.as_ref().map(|(r, _)| r.expires());
        match expires {
            Some(e) if e > now() => {
                if let (Some((mut r, legacy)), Some(raw)) = (record, raw.as_deref())
                    && (legacy || now().saturating_sub(r.last_seen) >= SEEN_EVERY_SECS)
                {
                    r.last_seen = now();
                    // Only over the row just read, so a revoke that lands meanwhile stays.
                    if let Ok(body) = serde_json::to_string(&r) {
                        let _ = self.registry.kv_replace(&key, raw, body.as_bytes());
                    }
                }
                self.cookies.write().insert(h, (e, Instant::now()));
                true
            }
            other => {
                if other.is_some() {
                    let _ = self.registry.kv_delete(&key);
                }
                self.cookies.write().remove(&h);
                false
            }
        }
    }

    /// A `Host` this server is not known by is refused, which blocks DNS rebinding.
    pub fn allowed_host(&self, host: Option<&str>) -> bool {
        host.is_some_and(|h| self.hosts.iter().any(|x| x.eq_ignore_ascii_case(h)))
    }

    /// A foreign `Origin` is refused on REST and on the WebSocket upgrade. `https` is accepted for
    /// a TLS reverse proxy in front of Ostra.
    pub fn allowed_origin(&self, origin: Option<&str>) -> bool {
        match origin {
            None => true,
            Some(o) => {
                let o = o.to_lowercase();
                let rest = o
                    .strip_prefix("http://")
                    .or_else(|| o.strip_prefix("https://"));
                rest.is_some_and(|r| self.hosts.iter().any(|h| h == r))
            }
        }
    }

    /// Expires the cookie in the browser that signs out.
    pub fn clear_cookie_header(secure: bool) -> String {
        let secure = if secure { "; Secure" } else { "" };
        format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure}")
    }

    /// `secure` when the browser reached Ostra over HTTPS, as through a TLS reverse proxy. The
    /// caller decides from the `Origin` header, which a browser sets itself; `X-Forwarded-Proto` is
    /// not read, because any client could send it.
    pub fn cookie_header(value: &str, secure: bool) -> String {
        let secure = if secure { "; Secure" } else { "" };
        format!(
            "{COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={COOKIE_TTL_SECS}{secure}"
        )
    }
}

/// Every live sign-in, oldest first, for `ostra sessions`.
pub fn list_sign_ins(registry: &RegistryDb) -> anyhow::Result<Vec<SignInSession>> {
    Ok(load_records(registry)?
        .iter()
        .map(|(_, r)| to_view(r, false))
        .collect())
}

/// Remove one sign-in by id, for `ostra sessions revoke`. False when there is none. A running
/// server refuses the cookie within [`RECHECK`].
pub fn revoke_sign_in(registry: &RegistryDb, id: &str) -> anyhow::Result<bool> {
    for (h, r) in load_records(registry)? {
        if r.id == id {
            registry.kv_delete(&format!("{COOKIE_KEY}{h}"))?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Remove every sign-in. Returns how many there were.
pub fn revoke_sign_ins(registry: &RegistryDb) -> anyhow::Result<usize> {
    let keys = registry.kv_scan(COOKIE_KEY)?;
    for (key, _) in &keys {
        registry.kv_delete(key)?;
    }
    Ok(keys.len())
}

pub fn cookie_value(header: Option<&str>) -> Option<String> {
    header?
        .split(';')
        .map(str::trim)
        .find_map(|kv| kv.strip_prefix(&format!("{COOKIE}=")).map(String::from))
}

fn server_file() -> std::path::PathBuf {
    paths::data_dir().join("server.json")
}

pub fn write_server_file(auth: &Auth) -> anyhow::Result<()> {
    paths::ensure_data_dir()?;
    let body = serde_json::json!({"port": auth.port, "url_host": auth.url_host, "pid": std::process::id()});
    std::fs::write(server_file(), body.to_string())?;
    Ok(())
}

/// A server recorded its pid in `server.json` and that process is alive.
pub fn server_running() -> bool {
    let Ok(text) = std::fs::read_to_string(server_file()) else {
        return false;
    };
    let pid = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("pid").and_then(|p| p.as_u64()));
    pid.and_then(|p| i32::try_from(p).ok())
        .is_some_and(process_alive)
}

/// `kill(pid, 0)` probes without signalling, on Linux and macOS alike. EPERM means the process
/// exists under another user.
fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence and permission check.
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

pub fn remove_server_file() {
    let _ = std::fs::remove_file(server_file());
}

/// `ostra url`: a fresh sign-in URL for the server that is running now.
pub fn request_url() -> anyhow::Result<String> {
    let text = std::fs::read_to_string(server_file())
        .map_err(|_| anyhow::anyhow!("No Ostra server is running. Start one with `ostra`."))?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let port = v
        .get("port")
        .and_then(|p| p.as_u64())
        .ok_or_else(|| anyhow::anyhow!("server.json has no port"))?;
    let host = v
        .get("url_host")
        .and_then(|h| h.as_str())
        .map(String::from)
        .unwrap_or_else(|| format!("127.0.0.1:{port}"));
    let registry = crate::app::open_registry()?;
    Ok(format!("http://{host}/#token={}", mint_token(&registry)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> SignInMeta {
        SignInMeta {
            user_agent: Some("TestBrowser/1".into()),
            ip: Some("127.0.0.1".into()),
        }
    }

    #[test]
    fn token_is_one_time_and_cookie_sticks() {
        let reg = RegistryDb::open_in_memory().unwrap();
        let auth = Auth::new(reg.clone(), &Access::loopback(7878));
        let token = mint_token(&reg).unwrap();
        let cookie = auth.exchange(&token, &meta()).unwrap();
        assert!(matches!(
            auth.exchange(&token, &meta()),
            Err(ExchangeError::Used { .. })
        ));
        assert_eq!(auth.exchange("nope", &meta()), Err(ExchangeError::Unknown));
        assert!(auth.check_cookie(&cookie));
        assert!(!auth.check_cookie("nope"));
        let fresh = Auth::new(reg, &Access::loopback(7878));
        assert!(fresh.check_cookie(&cookie), "sign-ins survive a restart");
    }

    #[test]
    fn sessions_are_listed_and_revoked_at_once() {
        let reg = RegistryDb::open_in_memory().unwrap();
        let auth = Auth::new(reg.clone(), &Access::loopback(7878));
        let a = auth.exchange(&mint_token(&reg).unwrap(), &meta()).unwrap();
        let b = auth.exchange(&mint_token(&reg).unwrap(), &meta()).unwrap();
        let list = auth.sessions(Some(&a)).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list.iter().filter(|s| s.current).count(), 1);
        assert_eq!(list[0].user_agent.as_deref(), Some("TestBrowser/1"));
        assert!(
            !list[0].id.contains(&cookie_hash(&a)[..8]),
            "the id is not the cookie"
        );

        let mut revoked = auth.subscribe_revoked();
        let b_id = list.iter().find(|s| !s.current).unwrap().id.clone();
        assert!(auth.check_cookie(&b));
        assert!(auth.revoke(&b_id).unwrap());
        assert!(
            !auth.check_cookie(&b),
            "a revoke on this server bypasses the cache"
        );
        assert_eq!(revoked.try_recv().unwrap(), cookie_hash(&b));
        assert!(!auth.revoke(&b_id).unwrap());

        let c = auth.exchange(&mint_token(&reg).unwrap(), &meta()).unwrap();
        assert_eq!(auth.revoke_others(&a).unwrap(), 1);
        assert!(auth.check_cookie(&a) && !auth.check_cookie(&c));
        auth.sign_out(&a).unwrap();
        assert!(!auth.check_cookie(&a));
        assert!(auth.sessions(None).unwrap().is_empty());
    }

    #[test]
    fn cli_revokes_reach_a_running_server_within_the_recheck() {
        let reg = RegistryDb::open_in_memory().unwrap();
        let auth = Auth::new(reg.clone(), &Access::loopback(7878));
        let cookie = auth.exchange(&mint_token(&reg).unwrap(), &meta()).unwrap();
        let id = list_sign_ins(&reg).unwrap()[0].id.clone();
        assert!(revoke_sign_in(&reg, &id).unwrap());
        assert!(!revoke_sign_in(&reg, &id).unwrap());
        assert!(
            auth.check_cookie(&cookie),
            "the cache holds for a short while"
        );
        auth.cookies
            .write()
            .values_mut()
            .for_each(|(_, checked)| *checked -= RECHECK);
        assert!(
            !auth.check_cookie(&cookie),
            "then the registry is read again"
        );
    }

    #[test]
    fn legacy_and_expired_sign_ins() {
        let reg = RegistryDb::open_in_memory().unwrap();
        let legacy = format!("{COOKIE_KEY}{}", hash("legacy"));
        reg.kv_set(&legacy, now().to_string().as_bytes()).unwrap();
        let old = format!("{COOKIE_KEY}{}", hash("old"));
        reg.kv_set(&old, (now() - COOKIE_TTL_SECS - 1).to_string().as_bytes())
            .unwrap();
        let auth = Auth::new(reg.clone(), &Access::loopback(7878));
        assert!(
            auth.check_cookie("legacy"),
            "an older server's sign-in still works"
        );
        assert!(
            !auth.check_cookie("old"),
            "a sign-in older than the TTL is refused"
        );
        assert_eq!(reg.kv_get(&old).unwrap(), None, "and removed");
        let list = list_sign_ins(&reg).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].id.starts_with("si_") && list[0].user_agent.is_none());
        let stored = reg.kv_get(&legacy).unwrap().unwrap();
        assert!(
            parse_record(&stored).is_some_and(|(_, l)| !l),
            "migrated to a record"
        );
        assert_eq!(revoke_sign_ins(&reg).unwrap(), 1);
    }

    #[test]
    fn failed_exchanges_are_limited_per_address() {
        let auth = Auth::new(
            RegistryDb::open_in_memory().unwrap(),
            &Access::loopback(7878),
        );
        let ip: Option<IpAddr> = Some("10.0.0.7".parse().unwrap());
        for _ in 0..MAX_FAILURES {
            assert_eq!(auth.exchange_wait(ip), None);
            auth.note_exchange(ip, false);
        }
        assert!(auth.exchange_wait(ip).is_some_and(|s| s <= 60));
        assert_eq!(auth.exchange_wait(Some("10.0.0.8".parse().unwrap())), None);
        auth.note_exchange(ip, true);
        assert_eq!(auth.exchange_wait(ip), None);
        let local: Option<IpAddr> = Some("127.0.0.1".parse().unwrap());
        for _ in 0..MAX_FAILURES * 2 {
            auth.note_exchange(local, false);
        }
        assert_eq!(
            auth.exchange_wait(local),
            None,
            "loopback cannot lock the user out"
        );
    }

    #[test]
    fn detects_live_processes_and_names_the_host() {
        assert!(process_alive(std::process::id() as i32));
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        assert!(!process_alive(pid), "a reaped child is gone");
        assert!(!process_alive(0) && !process_alive(-1));
        let h = hostname().expect("every test machine has a host name");
        assert!(!h.is_empty() && !h.ends_with(".local") && h == h.to_lowercase());
    }

    #[test]
    fn cookie_is_secure_over_https() {
        assert!(!Auth::cookie_header("v", false).contains("Secure"));
        assert!(Auth::cookie_header("v", true).ends_with("; Secure"));
        assert!(Auth::cookie_header("v", false).contains(&format!("Max-Age={COOKIE_TTL_SECS}")));
        assert!(Auth::clear_cookie_header(false).contains("Max-Age=0"));
    }

    #[test]
    fn host_and_origin() {
        let auth = Auth::new(
            RegistryDb::open_in_memory().unwrap(),
            &Access::loopback(7878),
        );
        assert!(auth.allowed_host(Some("127.0.0.1:7878")));
        assert!(auth.allowed_host(Some("localhost:7878")));
        assert!(!auth.allowed_host(Some("evil.example:7878")));
        assert!(!auth.allowed_host(None));
        assert!(auth.allowed_origin(None));
        assert!(auth.allowed_origin(Some("http://localhost:7878")));
        assert!(!auth.allowed_origin(Some("http://evil.example")));
        assert_eq!(
            cookie_value(Some("a=b; ostra_session=xyz")),
            Some("xyz".into())
        );
    }

    #[test]
    fn a_loopback_server_signs_in_at_a_private_name() {
        let registry = RegistryDb::open_in_memory().unwrap();
        let access = Access {
            private_host: true,
            ..Access::loopback(7878)
        };
        let auth = Auth::new(registry.clone(), &access);
        let url = auth.sign_in_url().unwrap();
        let host = url.trim_start_matches("http://").split('/').next().unwrap();
        assert!(
            host.starts_with("ostra-") && host.ends_with(".localhost:7878"),
            "{host}"
        );
        assert!(auth.allowed_host(Some(host)));
        assert_eq!(auth.private_redirect(Some("127.0.0.1:7878")), Some(host));
        assert_eq!(auth.private_redirect(Some("localhost:7878")), Some(host));
        assert_eq!(auth.private_redirect(Some(host)), None);
        let again = Auth::new(registry, &access);
        assert!(
            again.sign_in_url().unwrap().contains(host),
            "the name is stable"
        );
        let plain = Auth::new(
            RegistryDb::open_in_memory().unwrap(),
            &Access::loopback(7878),
        );
        assert_eq!(plain.private_redirect(Some("127.0.0.1:7878")), None);
    }

    #[test]
    fn remote_access_hosts() {
        let access = Access {
            port: 7878,
            bind: "0.0.0.0".parse().unwrap(),
            extra_hosts: vec!["ostra.example.com".into(), "box.lan:9000".into()],
            dev: false,
            private_host: true,
        };
        let auth = Auth::new(RegistryDb::open_in_memory().unwrap(), &access);
        assert!(auth.allowed_host(Some("127.0.0.1:7878")));
        assert!(auth.allowed_host(Some("ostra.example.com")));
        assert!(auth.allowed_host(Some("ostra.example.com:7878")));
        assert!(auth.allowed_host(Some("box.lan:9000")));
        assert!(!auth.allowed_host(Some("evil.example:7878")));
        assert!(auth.allowed_origin(Some("https://ostra.example.com")));
        assert!(!auth.allowed_origin(Some("https://evil.example")));
        for ip in access.lan_addresses() {
            assert!(auth.allowed_host(Some(&host_port(ip, 7878))), "{ip}");
        }
        assert!(is_local_address("127.0.0.1".parse().unwrap()));
        assert!(!is_local_address("203.0.113.9".parse().unwrap()));
        let local = Access::loopback(7878);
        assert!(!local.is_remote());
        assert_eq!(local.url_host(), "127.0.0.1:7878");
    }
}
