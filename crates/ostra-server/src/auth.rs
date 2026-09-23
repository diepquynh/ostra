//! Browser auth (HANDOVER 15): a one-time token in the URL fragment, exchanged once for an
//! `HttpOnly`, `SameSite=Strict` cookie. Host and Origin checks block DNS rebinding and foreign
//! pages. Tokens and cookies are stored hashed in the registry, so a restart keeps sign-ins and
//! `ostra url` can mint a token for a running server.

use ostra_core::paths;
use ostra_store::RegistryDb;
use parking_lot::RwLock;
use rand::RngExt;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const COOKIE: &str = "ostra_session";
const TOKEN_TTL_SECS: u64 = 15 * 60;

pub struct Auth {
    registry: RegistryDb,
    port: u16,
    /// Exact `Host` header values the browser may send.
    hosts: Vec<String>,
    /// Host used in printed sign-in URLs.
    url_host: String,
    cookies: RwLock<HashSet<String>>,
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
}

/// Interfaces that carry container or VM bridges rather than the machine's own address.
fn is_bridge(name: &str) -> bool {
    ["docker", "br-", "veth", "cni", "flannel", "virbr", "vnet", "tailscale", "lo"].iter().any(|p| name.starts_with(p))
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
    ip.is_loopback() || if_addrs::get_if_addrs().unwrap_or_default().iter().any(|i| i.ip() == ip)
}

fn hostname() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .ok()
        .map(|h| h.trim().to_lowercase())
        .filter(|h| !h.is_empty())
}

impl Access {
    pub fn loopback(port: u16) -> Self {
        Access { port, bind: std::net::IpAddr::from([127, 0, 0, 1]), extra_hosts: vec![], dev: false }
    }

    pub fn is_remote(&self) -> bool {
        !self.bind.is_loopback()
    }

    /// Non-loopback addresses of the machine's own interfaces, IPv4 first.
    pub fn lan_addresses(&self) -> Vec<std::net::IpAddr> {
        if !self.bind.is_unspecified() {
            return if self.bind.is_loopback() { vec![] } else { vec![self.bind] };
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
        let mut v = vec![format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")];
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
            let has_port = h.rsplit_once(':').is_some_and(|(_, p)| p.chars().all(|c| c.is_ascii_digit())) && !h.ends_with(']');
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

fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn random_hex() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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
    registry.kv_set(&format!("auth:token:{}", hash(&token)), expires.to_string().as_bytes())?;
    Ok(token)
}

impl Auth {
    pub fn new(registry: RegistryDb, access: &Access) -> Self {
        Auth {
            registry,
            port: access.port,
            hosts: access.hosts(),
            url_host: access.url_host(),
            cookies: RwLock::new(HashSet::new()),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn sign_in_url(&self) -> anyhow::Result<String> {
        Ok(format!("http://{}/#token={}", self.url_host, mint_token(&self.registry)?))
    }

    /// Exchange a one-time token for a cookie value. A used token stays recorded as used, so a
    /// second attempt can say so rather than look like an unknown token.
    pub fn exchange(&self, token: &str) -> Result<String, ExchangeError> {
        let key = format!("auth:token:{}", hash(token.trim()));
        let stored = self.registry.kv_get(&key).ok().flatten().ok_or(ExchangeError::Unknown)?;
        let stored = String::from_utf8(stored).map_err(|_| ExchangeError::Unknown)?;
        if let Some(at) = stored.strip_prefix("used:") {
            let ago = now().saturating_sub(at.parse().unwrap_or(0));
            return Err(ExchangeError::Used { seconds_ago: ago });
        }
        let _ = self.registry.kv_set(&key, format!("used:{}", now()).as_bytes());
        let expires: u64 = stored.parse().map_err(|_| ExchangeError::Unknown)?;
        if expires < now() {
            return Err(ExchangeError::Expired { seconds_ago: now() - expires });
        }
        let cookie = random_hex();
        let h = hash(&cookie);
        let _ = self.registry.kv_set(&format!("auth:cookie:{h}"), now().to_string().as_bytes());
        self.cookies.write().insert(h);
        Ok(cookie)
    }

    pub fn check_cookie(&self, value: &str) -> bool {
        let h = hash(value);
        if self.cookies.read().contains(&h) {
            return true;
        }
        let known = self.registry.kv_get(&format!("auth:cookie:{h}")).ok().flatten().is_some();
        if known {
            self.cookies.write().insert(h);
        }
        known
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
                let rest = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://"));
                rest.is_some_and(|r| self.hosts.iter().any(|h| h == r))
            }
        }
    }

    pub fn cookie_header(value: &str) -> String {
        format!("{COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000")
    }
}

pub fn cookie_value(header: Option<&str>) -> Option<String> {
    header?.split(';').map(str::trim).find_map(|kv| kv.strip_prefix(&format!("{COOKIE}=")).map(String::from))
}

fn server_file() -> std::path::PathBuf {
    paths::data_dir().join("server.json")
}

pub fn write_server_file(auth: &Auth) -> anyhow::Result<()> {
    std::fs::create_dir_all(paths::data_dir())?;
    let body = serde_json::json!({"port": auth.port, "url_host": auth.url_host, "pid": std::process::id()});
    std::fs::write(server_file(), body.to_string())?;
    Ok(())
}

/// A server recorded its pid in `server.json` and that process is alive.
pub fn server_running() -> bool {
    let Ok(text) = std::fs::read_to_string(server_file()) else { return false };
    let pid = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.get("pid").and_then(|p| p.as_u64()));
    pid.is_some_and(|p| std::path::Path::new(&format!("/proc/{p}")).exists())
}

pub fn remove_server_file() {
    let _ = std::fs::remove_file(server_file());
}

/// `ostra url`: a fresh sign-in URL for the server that is running now.
pub fn request_url() -> anyhow::Result<String> {
    let text = std::fs::read_to_string(server_file()).map_err(|_| anyhow::anyhow!("No Ostra server is running. Start one with `ostra`."))?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let port = v.get("port").and_then(|p| p.as_u64()).ok_or_else(|| anyhow::anyhow!("server.json has no port"))?;
    let host = v.get("url_host").and_then(|h| h.as_str()).map(String::from).unwrap_or_else(|| format!("127.0.0.1:{port}"));
    let registry = RegistryDb::open(&paths::registry_db_path())?;
    Ok(format!("http://{host}/#token={}", mint_token(&registry)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_one_time_and_cookie_sticks() {
        let reg = RegistryDb::open_in_memory().unwrap();
        let auth = Auth::new(reg.clone(), &Access::loopback(7878));
        let token = mint_token(&reg).unwrap();
        let cookie = auth.exchange(&token).unwrap();
        assert!(matches!(auth.exchange(&token), Err(ExchangeError::Used { .. })));
        assert_eq!(auth.exchange("nope"), Err(ExchangeError::Unknown));
        assert!(auth.check_cookie(&cookie));
        assert!(!auth.check_cookie("nope"));
        let fresh = Auth::new(reg, &Access::loopback(7878));
        assert!(fresh.check_cookie(&cookie), "sign-ins survive a restart");
    }

    #[test]
    fn host_and_origin() {
        let auth = Auth::new(RegistryDb::open_in_memory().unwrap(), &Access::loopback(7878));
        assert!(auth.allowed_host(Some("127.0.0.1:7878")));
        assert!(auth.allowed_host(Some("localhost:7878")));
        assert!(!auth.allowed_host(Some("evil.example:7878")));
        assert!(!auth.allowed_host(None));
        assert!(auth.allowed_origin(None));
        assert!(auth.allowed_origin(Some("http://localhost:7878")));
        assert!(!auth.allowed_origin(Some("http://evil.example")));
        assert_eq!(cookie_value(Some("a=b; ostra_session=xyz")), Some("xyz".into()));
    }

    #[test]
    fn remote_access_hosts() {
        let access = Access {
            port: 7878,
            bind: "0.0.0.0".parse().unwrap(),
            extra_hosts: vec!["ostra.example.com".into(), "box.lan:9000".into()],
            dev: false,
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
