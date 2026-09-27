//! The egress proxy: the one way out of a sandbox. On Linux each execution gets its own Unix
//! socket, bound only into its own sandbox, where a helper inside forwards the sandbox's loopback
//! to it. On macOS, where every sandbox shares the host's loopback, it gets its own port on
//! `127.0.0.1`, the one port its Seatbelt policy lets it connect to. The proxy sees the
//! destination host and port of every connection, lets through only what the policy allows, and
//! connects to the address it checked, never a second lookup.

use crate::HarnessKind;
use crate::config::{LoopbackAccess, SandboxConfig, SandboxNetwork};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio_util::sync::CancellationToken;

/// Package registries and source hosts that builds fetch from under `allowlist`.
pub const DEFAULT_ALLOWED_HOSTS: &[&str] = &[
    "index.crates.io",
    "static.crates.io",
    "static.rust-lang.org",
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "repo.yarnpkg.com",
    "pypi.org",
    "files.pythonhosted.org",
    "proxy.golang.org",
    "sum.golang.org",
    "crates.io",
    "github.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
    "raw.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "repo.maven.apache.org",
    "repo1.maven.org",
    "services.gradle.org",
    "plugins.gradle.org",
    "plugins-artifacts.gradle.org",
    "dl.google.com",
    "maven.google.com",
    "rubygems.org",
    "index.rubygems.org",
    "api.nuget.org",
];

/// The model API and sign-in hosts each harness CLI reaches, measured with one run each. A
/// harness gets its own under every network choice; `allowlist` lets every command reach all of
/// them, so a build or a test can call a model API too.
pub fn model_hosts(harness: HarnessKind) -> &'static [&'static str] {
    match harness {
        HarnessKind::Claude => &[
            "api.anthropic.com",
            "console.anthropic.com",
            "platform.claude.com",
            "claude.ai",
        ],
        HarnessKind::Codex => &["api.openai.com", "chatgpt.com", "auth.openai.com"],
        HarnessKind::Grok => &[
            "api.x.ai",
            "auth.x.ai",
            "accounts.x.ai",
            "grok.com",
            "cli-chat-proxy.grok.com",
        ],
        HarnessKind::Agy => &[
            "cloudcode-pa.googleapis.com",
            "daily-cloudcode-pa.googleapis.com",
            "generativelanguage.googleapis.com",
            "oauth2.googleapis.com",
            "www.googleapis.com",
            "lh3.googleusercontent.com",
        ],
    }
}

const MAX_HEAD: usize = 8 * 1024;
const MAX_CONNECTIONS: usize = 256;
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// One `allowed_hosts` entry: `host`, `*.domain`, an IPv4 address, or `[v6]`, each with an
/// optional `:port`. Without a port it allows 443 and 80.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRule {
    host: Pattern,
    port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pattern {
    Name(String),
    /// `*.domain`: every name below `domain`, not `domain` itself.
    Subdomains(String),
    Ip(IpAddr),
}

/// A destination as a request names it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Host {
    Name(String),
    Ip(IpAddr),
}

impl std::fmt::Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Host::Name(n) => f.write_str(n),
            Host::Ip(ip) => write!(f, "{ip}"),
        }
    }
}

impl HostRule {
    pub fn parse(text: &str) -> Option<HostRule> {
        let t = text.trim();
        if t.is_empty()
            || t.contains(|c: char| c.is_whitespace() || matches!(c, '/' | '@' | '?' | '#'))
        {
            return None;
        }
        if let Some(rest) = t.strip_prefix('[') {
            let (v6, after) = rest.split_once(']')?;
            let ip: Ipv6Addr = v6.parse().ok()?;
            let port = match after {
                "" => None,
                p => Some(parse_port(p.strip_prefix(':')?)?),
            };
            return Some(HostRule {
                host: Pattern::Ip(canonical(IpAddr::V6(ip))),
                port,
            });
        }
        let (host, port) = match t.split_once(':') {
            Some((h, p)) => (h, Some(parse_port(p)?)),
            None => (t, None),
        };
        let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return Some(HostRule {
                host: Pattern::Ip(IpAddr::V4(ip)),
                port,
            });
        }
        let host = match host.strip_prefix("*.") {
            Some(d) => valid_name(d).then(|| Pattern::Subdomains(d.to_string()))?,
            None => valid_name(&host).then_some(Pattern::Name(host))?,
        };
        Some(HostRule { host, port })
    }

    /// `localhost` or a loopback address. Clients skip the proxy for these, so such an entry
    /// works only as a forwarded port.
    pub fn is_loopback(&self) -> bool {
        match &self.host {
            Pattern::Ip(ip) => ip.is_loopback(),
            Pattern::Name(n) => n == "localhost",
            Pattern::Subdomains(_) => false,
        }
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    fn matches(&self, host: &Host, port: u16) -> bool {
        let port_ok = match self.port {
            Some(p) => p == port,
            None => port == 443 || port == 80,
        };
        port_ok
            && match (&self.host, host) {
                (Pattern::Name(n), Host::Name(h)) => n == h,
                (Pattern::Subdomains(d), Host::Name(h)) => h
                    .strip_suffix(d.as_str())
                    .and_then(|rest| rest.strip_suffix('.'))
                    .is_some_and(|rest| !rest.is_empty()),
                (Pattern::Ip(a), Host::Ip(b)) => a == b,
                _ => false,
            }
    }
}

fn parse_port(p: &str) -> Option<u16> {
    if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    p.parse::<u16>().ok().filter(|&p| p != 0)
}

fn valid_name(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

/// An IPv4-mapped IPv6 address as the IPv4 address it carries.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// Loopback, private, link-local (the cloud metadata address included), CGNAT, unique-local,
/// multicast, and reserved ranges.
pub fn is_private(ip: IpAddr) -> bool {
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

/// A name that points at this machine or the local network without a lookup: `localhost`,
/// local-only suffixes, and single labels, which resolve through the LAN's search domains.
fn local_name(h: &str) -> bool {
    h == "localhost"
        || !h.contains('.')
        || [".localhost", ".local", ".internal", ".lan", ".home.arpa"]
            .iter()
            .any(|s| h.ends_with(s))
}

/// What the proxy lets through for one execution.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    network: SandboxNetwork,
    /// Hosts the user chose, which may resolve anywhere: `allowed_hosts` and the base URLs a
    /// harness launch passes.
    trusted: Vec<HostRule>,
    /// Built-in hosts, which must resolve to public addresses.
    builtin: Vec<HostRule>,
    /// `[sandbox] upstream_proxy`, which public destinations are reached through.
    upstream: Option<(Host, u16)>,
    /// The workspace's loopback choice, which Seatbelt policies apply.
    loopback: LoopbackAccess,
    /// Loopback ports no command connects to, even when listed.
    blocked_ports: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    AnyAddress,
    PublicOnly,
    Refused,
}

impl Policy {
    pub fn new(cfg: &SandboxConfig) -> Policy {
        let parse = |xs: &mut dyn Iterator<Item = &str>| -> Vec<HostRule> {
            xs.filter_map(HostRule::parse).collect()
        };
        let user = || parse(&mut cfg.allowed_hosts.iter().map(String::as_str));
        let (trusted, builtin) = match cfg.network {
            SandboxNetwork::None | SandboxNetwork::Host => (vec![], vec![]),
            SandboxNetwork::Allowlist => (
                user(),
                parse(
                    &mut DEFAULT_ALLOWED_HOSTS
                        .iter()
                        .chain(HarnessKind::ALL.iter().flat_map(|h| model_hosts(*h)))
                        .copied(),
                ),
            ),
            SandboxNetwork::Public => (user(), vec![]),
        };
        Policy {
            network: cfg.network,
            trusted,
            builtin,
            upstream: cfg.upstream_proxy.as_deref().and_then(upstream_authority),
            loopback: cfg.loopback,
            blocked_ports: cfg.blocked_ports.clone(),
        }
    }

    /// Whether a command on the host's shared loopback (Seatbelt) may connect to every port but
    /// the blocked ones: the workspace's choice, under `allowlist` and `public` only, because
    /// `none` reaches nothing.
    pub fn shared_loopback(&self) -> bool {
        self.loopback == LoopbackAccess::Open
            && matches!(
                self.network,
                SandboxNetwork::Allowlist | SandboxNetwork::Public
            )
    }

    pub fn blocked_ports(&self) -> &[u16] {
        &self.blocked_ports
    }

    pub fn network(&self) -> SandboxNetwork {
        self.network
    }

    /// Hosts a harness CLI needs under every choice: its model API (`builtin`, public addresses
    /// only) and the hosts of base URLs its launch passes (`trusted`, because the user set them).
    pub fn allow(&mut self, builtin: Vec<HostRule>, trusted: Vec<HostRule>) {
        self.builtin.extend(builtin);
        self.trusted.extend(trusted);
    }

    /// Ports of listed loopback hosts (`127.0.0.1:8317`, `localhost:8317`). Inside a network
    /// namespace loopback is the sandbox's own, and clients skip the proxy for it, so these are
    /// forwarded to the host's loopback instead.
    pub fn loopback_ports(&self) -> Vec<u16> {
        let mut out: Vec<u16> = self
            .trusted
            .iter()
            .filter(|r| r.is_loopback())
            .filter_map(|r| r.port)
            .filter(|p| !self.blocked_ports.contains(p))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Whether some destination on `port` is reachable, such as an SSH server on 22.
    pub fn reaches_port(&self, port: u16) -> bool {
        matches!(self.network, SandboxNetwork::Public | SandboxNetwork::Host)
            || self
                .trusted
                .iter()
                .chain(&self.builtin)
                .any(|r| r.port == Some(port))
    }

    /// Whether the sandbox needs the proxy: a private namespace with somewhere to go.
    pub fn needs_proxy(&self) -> bool {
        match self.network {
            SandboxNetwork::Host => false,
            SandboxNetwork::None => !(self.trusted.is_empty() && self.builtin.is_empty()),
            SandboxNetwork::Allowlist | SandboxNetwork::Public => true,
        }
    }

    fn access(&self, host: &Host, port: u16) -> Access {
        if self.trusted.iter().any(|r| r.matches(host, port)) {
            Access::AnyAddress
        } else if self.network == SandboxNetwork::Public
            || self.builtin.iter().any(|r| r.matches(host, port))
        {
            Access::PublicOnly
        } else {
            Access::Refused
        }
    }

    /// Where to connect, or the refusal. With an upstream proxy, a trusted host on a private
    /// address is reached directly, because an internal mirror is rarely behind the corporate
    /// proxy, and everything else through the upstream, which also takes names that do not
    /// resolve here.
    async fn route(&self, host: &Host, port: u16) -> Result<Route, Refusal> {
        let Some(up) = &self.upstream else {
            return self.resolve(host, port).await.map(Route::Direct);
        };
        match self.resolve(host, port).await {
            Ok(addrs) if addrs.iter().any(|a| is_private(a.ip())) => Ok(Route::Direct(addrs)),
            Ok(_) => Ok(Route::Upstream(up.clone())),
            Err(r) if r.unresolved => Ok(Route::Upstream(up.clone())),
            Err(r) => Err(r),
        }
    }

    /// The addresses to connect to, in order, or the refusal.
    async fn resolve(&self, host: &Host, port: u16) -> Result<Vec<SocketAddr>, Refusal> {
        let access = self.access(host, port);
        let local = match host {
            Host::Ip(ip) => is_private(*ip),
            Host::Name(n) => local_name(n),
        };
        if access == Access::Refused {
            return Err(Refusal {
                reason: self.unlisted(host, port),
                local,
                unresolved: false,
            });
        }
        // Not looked up: a local-only name points at this machine or the LAN, and its lookup
        // goes out on the LAN (multicast DNS for `.local`, seconds before it fails on macOS).
        if access == Access::PublicOnly && matches!(host, Host::Name(_)) && local {
            return Err(Refusal {
                reason: local_refusal(host, port, "a local-only name"),
                local: true,
                unresolved: false,
            });
        }
        let addrs: Vec<SocketAddr> = match host {
            Host::Ip(ip) => vec![SocketAddr::new(*ip, port)],
            Host::Name(n) => match tokio::net::lookup_host((n.as_str(), port)).await {
                Ok(a) => a
                    .map(|a| SocketAddr::new(canonical(a.ip()), port))
                    .collect(),
                Err(e) => {
                    return Err(Refusal {
                        reason: format!("Check the host name `{n}`: it does not resolve ({e})."),
                        local,
                        // An upstream proxy may reach it, but not a local-only name the user did
                        // not list, which would reach the upstream's own network unchecked.
                        unresolved: access == Access::AnyAddress || !local,
                    });
                }
            },
        };
        if access == Access::AnyAddress {
            return Ok(addrs);
        }
        let public: Vec<SocketAddr> = addrs
            .iter()
            .copied()
            .filter(|a| !is_private(a.ip()))
            .collect();
        if public.is_empty() {
            let shown = addrs
                .first()
                .map(|a| a.ip().to_string())
                .unwrap_or_default();
            return Err(Refusal {
                reason: local_refusal(host, port, &shown),
                local: true,
                unresolved: false,
            });
        }
        Ok(public)
    }

    fn unlisted(&self, host: &Host, port: u16) -> String {
        let rule = rule_text(host, port);
        match self.network {
            SandboxNetwork::None => format!(
                "Set `[sandbox] network` in config.toml to `allowlist` and add `{rule}` to `allowed_hosts` to let sandboxed commands reach it, because the sandbox has no network under `none`."
            ),
            _ => format!(
                "Add `{rule}` to `[sandbox] allowed_hosts` in config.toml, or set `[sandbox] network = \"public\"`, to let sandboxed commands reach it, because the sandbox lets traffic through only to listed hosts."
            ),
        }
    }
}

/// Whether a request head carries `Proxy-Authorization: <want>`, compared in constant time.
fn authorized(head: &[u8], want: &str) -> bool {
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    text.split("\r\n").skip(1).any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        let value = value.trim().as_bytes();
        name.trim().eq_ignore_ascii_case("proxy-authorization")
            && value.len() == want.len()
            && value
                .iter()
                .zip(want.as_bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    })
}

/// The refusal of a local destination the user did not list; `what` names the address or name.
fn local_refusal(host: &Host, port: u16, what: &str) -> String {
    format!(
        "Add `{}` to `[sandbox] allowed_hosts` in config.toml if it is a service you run, because the sandbox refuses loopback, private, and link-local addresses ({what}) for hosts you did not list.",
        rule_text(host, port)
    )
}

/// The `allowed_hosts` entry that would allow `host:port`.
fn rule_text(host: &Host, port: u16) -> String {
    let h = match host {
        Host::Ip(IpAddr::V6(v6)) => format!("[{v6}]"),
        h => h.to_string(),
    };
    if port == 443 || port == 80 {
        h
    } else {
        format!("{h}:{port}")
    }
}

struct Refusal {
    reason: String,
    /// The destination is loopback, private, or link-local.
    local: bool,
    /// The name did not resolve here, which an upstream proxy may still reach.
    unresolved: bool,
}

enum Route {
    Direct(Vec<SocketAddr>),
    Upstream((Host, u16)),
}

/// `http://host:port` of `[sandbox] upstream_proxy`, as the host and port. `None` when it is
/// not in that form.
pub fn parse_upstream(url: &str) -> Option<(String, u16)> {
    upstream_authority(url).map(|(h, p)| (h.to_string(), p))
}

fn upstream_authority(url: &str) -> Option<(Host, u16)> {
    let rest = url.trim().strip_prefix("http://")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.contains(['/', '@', '?', '#']) {
        return None;
    }
    authority(rest, None).ok()
}

/// `host:port` as a request line writes it, brackets around IPv6.
fn target_text(host: &Host, port: u16) -> String {
    match host {
        Host::Ip(IpAddr::V6(v6)) => format!("[{v6}]:{port}"),
        h => format!("{h}:{port}"),
    }
}

/// Connects through the upstream proxy: a CONNECT tunnel to `host:port`, or, for plain HTTP
/// (`tunnel` false), a connection the absolute-form request is sent on. Returns the stream and
/// any bytes the destination sent after the upstream's answer.
async fn via_upstream(
    up: &(Host, u16),
    host: &Host,
    port: u16,
    tunnel: bool,
) -> Result<(TcpStream, Vec<u8>), String> {
    let addrs: Vec<SocketAddr> = match &up.0 {
        Host::Ip(ip) => vec![SocketAddr::new(*ip, up.1)],
        Host::Name(n) => tokio::net::lookup_host((n.as_str(), up.1))
            .await
            .map_err(|e| {
                format!("Check `[sandbox] upstream_proxy`: `{n}` does not resolve ({e}).")
            })?
            .collect(),
    };
    let mut stream = None;
    for a in addrs {
        if let Ok(Ok(s)) = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(a)).await {
            stream = Some(s);
            break;
        }
    }
    let mut s = stream.ok_or_else(|| {
        format!(
            "Check `[sandbox] upstream_proxy`: cannot connect to {}.",
            target_text(&up.0, up.1)
        )
    })?;
    if !tunnel {
        return Ok((s, vec![]));
    }
    let target = target_text(host, port);
    s.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let (head, rest) = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut s))
        .await
        .map_err(|_| "The upstream proxy did not answer.".to_string())??;
    let status = String::from_utf8_lossy(&head);
    let status = status.lines().next().unwrap_or_default();
    if status.split(' ').nth(1) != Some("200") {
        return Err(format!(
            "The upstream proxy answered `{status}` for {target}."
        ));
    }
    Ok((s, rest))
}

/// A plain HTTP head in origin form rewritten to the absolute form a proxy expects.
fn absolute_form(head: &str, host: &Host, port: u16) -> String {
    let (first, rest) = head.split_once("\r\n").unwrap_or((head, ""));
    let mut parts = first.splitn(3, ' ');
    let (method, path, version) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or("/"),
        parts.next().unwrap_or("HTTP/1.1"),
    );
    let auth = if port == 80 {
        match host {
            Host::Ip(IpAddr::V6(v6)) => format!("[{v6}]"),
            h => h.to_string(),
        }
    } else {
        target_text(host, port)
    };
    format!("{method} http://{auth}{path} {version}\r\n{rest}")
}

/// One connection the proxy allowed or refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub host: String,
    pub port: u16,
    pub allowed: bool,
    /// Why it was refused, with the setting that allows it first.
    pub reason: Option<String>,
    /// The destination is loopback, private, or link-local.
    pub local: bool,
}

/// Receives the first allowed connection and the first refusal per host and port, so a retry
/// loop reports once. Called on the proxy's runtime, so it must not block.
pub type OnDecision = Arc<dyn Fn(Decision) + Send + Sync>;

/// A listener on the egress runtime: a Unix socket, or a port on `127.0.0.1`. Dropping it stops
/// the listener, ends its open connections, and removes the socket.
#[derive(Debug)]
pub struct Listener {
    /// Empty for a loopback port.
    socket: PathBuf,
    port: Option<u16>,
    /// The proxy URL with its credential, for a loopback proxy.
    url: Option<String>,
    /// `None` for a socket another part of Ostra serves and removes.
    stop: Option<CancellationToken>,
}

impl Listener {
    /// The Unix socket, empty for a loopback port.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The port on `127.0.0.1`, for a loopback listener.
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// `http://ostra:<credential>@127.0.0.1:<port>`, for a loopback proxy.
    pub fn proxy_url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// A socket that something else serves, such as the server's hook bridge socket, to bind
    /// into a sandbox. Dropping it leaves the socket alone.
    pub fn served_elsewhere(socket: PathBuf) -> Listener {
        Listener {
            socket,
            port: None,
            url: None,
            stop: None,
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.cancel();
            if self.port.is_none() {
                let _ = std::fs::remove_file(&self.socket);
            }
        }
    }
}

/// A connection a listener accepted, from a Unix socket or a loopback port.
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
type Client = Box<dyn Stream>;

enum Incoming {
    Unix(UnixListener),
    Tcp(TcpListener),
}

impl Incoming {
    async fn accept(&self) -> std::io::Result<Client> {
        match self {
            Incoming::Unix(l) => l.accept().await.map(|(c, _)| Box::new(c) as Client),
            Incoming::Tcp(l) => l.accept().await.map(|(c, _)| Box::new(c) as Client),
        }
    }
}

/// Where a splice leads.
#[derive(Debug, Clone)]
pub enum Target {
    Tcp(SocketAddr),
    /// A Unix socket another part of Ostra serves, such as the hook bridge socket.
    Socket(PathBuf),
}

/// A fresh owner-only socket path in `dir`, which is created 0700.
pub fn socket_path(dir: &Path, prefix: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir.join(format!(
        "{prefix}{}.sock",
        &uuid::Uuid::new_v4().simple().to_string()[..16]
    )))
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ostra-egress")
            .enable_all()
            .build()
            .expect("the egress runtime starts")
    })
}

/// A fresh owner-only socket in `dir`, bound on the egress runtime.
fn bind(dir: &Path) -> std::io::Result<(PathBuf, UnixListener)> {
    let socket = socket_path(dir, "")?;
    let std_listener = std::os::unix::net::UnixListener::bind(&socket).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!(
                "Cannot create the socket {}: {e}. Keep the data dir path short, because a socket path is limited to 107 bytes.",
                socket.display()
            ),
        )
    })?;
    std_listener.set_nonblocking(true)?;
    let _guard = runtime().enter();
    Ok((socket, UnixListener::from_std(std_listener)?))
}

/// A fresh port on `127.0.0.1`, bound on the egress runtime. Only IPv4, because clients are
/// handed `http://127.0.0.1:<port>` and the Seatbelt rule names the port.
fn bind_loopback() -> std::io::Result<(u16, TcpListener)> {
    let std_listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    std_listener.set_nonblocking(true)?;
    let port = std_listener.local_addr()?.port();
    let _guard = runtime().enter();
    Ok((port, TcpListener::from_std(std_listener)?))
}

fn serve_proxy(
    incoming: Incoming,
    policy: Policy,
    on: OnDecision,
    auth: Option<String>,
) -> CancellationToken {
    let stop = CancellationToken::new();
    let shared = Arc::new(Shared {
        policy,
        on,
        seen: Mutex::new(HashSet::new()),
        auth,
    });
    runtime().spawn(accept_loop(incoming, stop.clone(), move |client| {
        let shared = shared.clone();
        async move { handle(client, &shared).await }
    }));
    stop
}

/// Starts the egress proxy for one execution on a new socket in `dir`.
pub fn proxy(policy: Policy, dir: &Path, on: OnDecision) -> std::io::Result<Listener> {
    let (socket, listener) = bind(dir)?;
    Ok(Listener {
        socket,
        port: None,
        url: None,
        stop: Some(serve_proxy(Incoming::Unix(listener), policy, on, None)),
    })
}

/// The user name in a loopback proxy's URL; the password is the execution's own credential.
const PROXY_USER: &str = "ostra";

/// Starts the egress proxy for one execution on a new port of `127.0.0.1`, for a sandbox that
/// shares the host's loopback. Every request must carry the credential in [`Listener::proxy_url`],
/// because a sandbox whose workspace opens the loopback reaches other executions' proxy ports too.
pub fn loopback_proxy(policy: Policy, on: OnDecision) -> std::io::Result<Listener> {
    use base64::Engine;
    let (port, listener) = bind_loopback()?;
    let secret = uuid::Uuid::new_v4().simple().to_string();
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{PROXY_USER}:{secret}"))
    );
    Ok(Listener {
        socket: PathBuf::new(),
        port: Some(port),
        url: Some(format!("http://{PROXY_USER}:{secret}@127.0.0.1:{port}")),
        stop: Some(serve_proxy(Incoming::Tcp(listener), policy, on, Some(auth))),
    })
}

fn serve_splice(incoming: Incoming, target: Target) -> CancellationToken {
    let stop = CancellationToken::new();
    runtime().spawn(accept_loop(incoming, stop.clone(), move |mut client| {
        let target = target.clone();
        async move {
            let mut up: Client = match target {
                Target::Tcp(a) => {
                    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(a)).await {
                        Ok(Ok(s)) => Box::new(s),
                        _ => return,
                    }
                }
                Target::Socket(p) => match UnixStream::connect(p).await {
                    Ok(s) => Box::new(s),
                    Err(_) => return,
                },
            };
            let _ = tokio::io::copy_bidirectional(&mut client, &mut up).await;
        }
    }));
    stop
}

/// A socket in `dir` whose every connection is spliced to `target`, such as Ostra's own
/// listener for a harness's hook bridge.
pub fn splice(dir: &Path, target: SocketAddr) -> std::io::Result<Listener> {
    let (socket, listener) = bind(dir)?;
    Ok(Listener {
        socket,
        port: None,
        url: None,
        stop: Some(serve_splice(Incoming::Unix(listener), Target::Tcp(target))),
    })
}

/// A new port on `127.0.0.1` whose every connection is spliced to `target`, so a sandbox that
/// shares the host's loopback reaches the hook bridge through a port its policy names.
pub fn loopback_splice(target: Target) -> std::io::Result<Listener> {
    let (port, listener) = bind_loopback()?;
    Ok(Listener {
        socket: PathBuf::new(),
        port: Some(port),
        url: None,
        stop: Some(serve_splice(Incoming::Tcp(listener), target)),
    })
}

async fn accept_loop<F, Fut>(listener: Incoming, stop: CancellationToken, serve: F)
where
    F: Fn(Client) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    loop {
        let accepted = tokio::select! {
            _ = stop.cancelled() => return,
            a = listener.accept() => a,
        };
        let client = match accepted {
            Ok(c) => c,
            Err(_) => {
                // Out of file descriptors, most likely; retrying at once would spin.
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let stop = stop.clone();
        let fut = serve(client);
        tokio::spawn(async move {
            let _permit = permit;
            tokio::select! {
                _ = stop.cancelled() => {}
                _ = fut => {}
            }
        });
    }
}

struct Shared {
    policy: Policy,
    on: OnDecision,
    seen: Mutex<HashSet<(String, u16, bool)>>,
    /// The `Proxy-Authorization` value every request must carry, for a loopback proxy.
    auth: Option<String>,
}

impl Shared {
    fn report(&self, d: Decision) {
        let first = self
            .seen
            .lock()
            .map(|mut s| s.insert((d.host.clone(), d.port, d.allowed)))
            .unwrap_or(false);
        if first {
            (self.on)(d);
        }
    }

    fn allowed(&self, host: &Host, port: u16) {
        self.report(Decision {
            host: host.to_string(),
            port,
            allowed: true,
            reason: None,
            local: false,
        });
    }

    fn refused(&self, host: &Host, port: u16, r: &Refusal) {
        self.report(Decision {
            host: host.to_string(),
            port,
            allowed: false,
            reason: Some(r.reason.clone()),
            local: r.local,
        });
    }
}

/// A parsed proxy request.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    host: Host,
    port: u16,
    /// For plain HTTP, the head to send upstream: origin form, `Connection: close`, no
    /// `Proxy-*` headers. `None` for `CONNECT`.
    upstream_head: Option<String>,
}

fn parse_request(head: &[u8]) -> Result<Request, String> {
    let text =
        std::str::from_utf8(head).map_err(|_| "The request head is not UTF-8.".to_string())?;
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("Send an HTTP/1.1 request line.".into());
    };
    if !version.starts_with("HTTP/1.") {
        return Err("Send HTTP/1.1, because the egress proxy speaks nothing else.".into());
    }
    if method == "CONNECT" {
        let (host, port) = authority(target, None)?;
        return Ok(Request {
            host,
            port,
            upstream_head: None,
        });
    }
    let rest = target
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("http://"))
        .map(|_| &target[7..])
        .ok_or("Use CONNECT for HTTPS, and an absolute `http://` URL for plain HTTP.")?;
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (auth, path) = rest.split_at(split);
    let (host, port) = authority(auth, Some(80))?;
    let path = match path {
        "" => "/".to_string(),
        p if p.starts_with('?') => format!("/{p}"),
        p => p.to_string(),
    };
    let mut out = format!("{method} {path} {version}\r\nHost: {auth}\r\n");
    for line in lines.filter(|l| !l.is_empty()) {
        if line.starts_with([' ', '\t']) {
            return Err("Folded header lines are not accepted.".into());
        }
        let name = line
            .split(':')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if name.starts_with("proxy-")
            || matches!(name.as_str(), "host" | "connection" | "keep-alive")
        {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    Ok(Request {
        host,
        port,
        upstream_head: Some(out),
    })
}

/// `host:port`, `[v6]:port`, or a bare host when `default_port` is given.
fn authority(text: &str, default_port: Option<u16>) -> Result<(Host, u16), String> {
    let bad = || format!("`{text}` is not a host and port.");
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (v6, after) = rest.split_once(']').ok_or_else(bad)?;
        let ip: Ipv6Addr = v6.parse().map_err(|_| bad())?;
        (Host::Ip(canonical(IpAddr::V6(ip))), after.strip_prefix(':'))
    } else {
        let (h, p) = match text.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (text, None),
        };
        let h = h.strip_suffix('.').unwrap_or(h).to_ascii_lowercase();
        let host = match h.parse::<Ipv4Addr>() {
            Ok(ip) => Host::Ip(IpAddr::V4(ip)),
            Err(_) if valid_name(&h) => Host::Name(h),
            Err(_) => return Err(bad()),
        };
        (host, p)
    };
    let port = match port {
        Some(p) => parse_port(p).ok_or_else(bad)?,
        None => default_port.ok_or_else(bad)?,
    };
    Ok((host, port))
}

/// Reads up to the end of the request head. Returns the head and whatever followed it.
async fn read_head<S: tokio::io::AsyncRead + Unpin>(
    s: &mut S,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        let n = s.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("The connection closed before the request head ended.".into());
        }
        let from = buf.len().saturating_sub(3);
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf[from..].windows(4).position(|w| w == b"\r\n\r\n") {
            let end = from + i + 4;
            if end > MAX_HEAD {
                break;
            }
            let rest = buf.split_off(end);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            break;
        }
    }
    Err(format!(
        "Keep the request head under {} KiB, because the egress proxy refuses longer ones.",
        MAX_HEAD / 1024
    ))
}

/// What the first bytes of a tunnel say about the TLS server name.
#[derive(Debug, PartialEq, Eq)]
enum Hello {
    /// Read more before deciding.
    More,
    /// Not a TLS handshake, or a ClientHello without a server name.
    NoName,
    Name(String),
    /// A handshake Ostra cannot read, or one larger than it reads.
    Invalid,
}

/// The TLS ClientHello's handshake bytes are capped, because a real one is a few KiB.
const MAX_HELLO: usize = 32 * 1024;

/// The server name of the ClientHello at the start of `buf`, reassembled across TLS records.
fn client_hello_name(buf: &[u8]) -> Hello {
    if buf.first().is_some_and(|&b| b != 0x16) {
        return Hello::NoName;
    }
    let mut hs = vec![];
    let mut at = 0;
    loop {
        let Some(head) = buf.get(at..at + 5) else {
            return Hello::More;
        };
        if head[0] != 0x16 {
            return Hello::Invalid;
        }
        let len = u16::from_be_bytes([head[3], head[4]]) as usize;
        // RFC 8446 5.1: handshake records are never empty and hold at most 2^14 bytes plus 256.
        if len == 0 || len > (1 << 14) + 256 {
            return Hello::Invalid;
        }
        let Some(body) = buf.get(at + 5..at + 5 + len) else {
            return Hello::More;
        };
        hs.extend_from_slice(body);
        at += 5 + len;
        if hs.len() >= 4 {
            if hs[0] != 1 {
                return Hello::Invalid;
            }
            let want = 4 + (u32::from_be_bytes([0, hs[1], hs[2], hs[3]]) as usize);
            if want > MAX_HELLO {
                return Hello::Invalid;
            }
            if hs.len() >= want {
                return hello_name(&hs[4..want])
                    .map_or(Hello::Invalid, |n| n.map_or(Hello::NoName, Hello::Name));
            }
        }
    }
}

/// The `server_name` extension of a ClientHello body. `None` when the body does not parse.
fn hello_name(b: &[u8]) -> Option<Option<String>> {
    let mut r = Reader(b);
    r.take(2 + 32)?;
    let sid = r.u8()? as usize;
    r.take(sid)?;
    let suites = r.u16()? as usize;
    r.take(suites)?;
    let comp = r.u8()? as usize;
    r.take(comp)?;
    if r.0.is_empty() {
        return Some(None);
    }
    let ext_len = r.u16()? as usize;
    let mut exts = Reader(r.take(ext_len)?);
    while !exts.0.is_empty() {
        let kind = exts.u16()?;
        let len = exts.u16()? as usize;
        let data = exts.take(len)?;
        if kind != 0 {
            continue;
        }
        let mut list = Reader(data);
        let list_len = list.u16()? as usize;
        let mut names = Reader(list.take(list_len)?);
        while !names.0.is_empty() {
            let name_type = names.u8()?;
            let n = names.u16()? as usize;
            let name = names.take(n)?;
            if name_type == 0 {
                let name = std::str::from_utf8(name).ok()?;
                return Some(Some(name.trim_end_matches('.').to_ascii_lowercase()));
            }
        }
        return Some(None);
    }
    Some(None)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
}

/// Reads the tunnel's first bytes into `buf` until [`client_hello_name`] can decide.
async fn read_hello(s: &mut Client, buf: &mut Vec<u8>) -> Hello {
    let mut chunk = [0u8; 4096];
    loop {
        match client_hello_name(buf) {
            Hello::More => {}
            decided => return decided,
        }
        match s.read(&mut chunk).await {
            Ok(0) | Err(_) => return Hello::NoName,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

async fn reply(s: &mut Client, status: &str, body: &str) {
    reply_with(s, status, "", body).await
}

/// [`reply`] with more header lines, each ending in CRLF.
async fn reply_with(s: &mut Client, status: &str, headers: &str, body: &str) {
    let msg = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}\n",
        body.len() + 1
    );
    let _ = s.write_all(msg.as_bytes()).await;
    let _ = s.shutdown().await;
}

async fn handle(mut client: Client, shared: &Shared) {
    let (head, rest) = match tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut client)).await {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => return reply(&mut client, "400 Bad Request", &e).await,
        Err(_) => return,
    };
    if let Some(want) = &shared.auth
        && !authorized(&head, want)
    {
        // git sends the credential only after this challenge.
        return reply_with(
            &mut client,
            "407 Proxy Authentication Required",
            "Proxy-Authenticate: Basic realm=\"ostra\"\r\n",
            "Use the proxy URL in `HTTPS_PROXY` with its credential, because this proxy serves one execution's sandbox only.",
        )
        .await;
    }
    let req = match parse_request(&head) {
        Ok(r) => r,
        Err(e) => return reply(&mut client, "400 Bad Request", &e).await,
    };
    let route = match shared.policy.route(&req.host, req.port).await {
        Ok(r) => r,
        Err(r) => {
            shared.refused(&req.host, req.port, &r);
            return reply(&mut client, "403 Forbidden", &r.reason).await;
        }
    };
    shared.allowed(&req.host, req.port);
    let tunnel = req.upstream_head.is_none();
    let (mut up, early, head) = match route {
        Route::Direct(addrs) => {
            let mut upstream = None;
            let mut last_err = String::from("no address");
            for a in addrs {
                match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(a)).await {
                    Ok(Ok(s)) => {
                        upstream = Some(s);
                        break;
                    }
                    Ok(Err(e)) => last_err = e.to_string(),
                    Err(_) => last_err = "timed out".into(),
                }
            }
            let Some(up) = upstream else {
                let msg = format!("Cannot connect to {}:{}: {last_err}.", req.host, req.port);
                return reply(&mut client, "502 Bad Gateway", &msg).await;
            };
            (up, vec![], req.upstream_head.clone())
        }
        Route::Upstream(proxy) => match via_upstream(&proxy, &req.host, req.port, tunnel).await {
            Ok((up, early)) => {
                let head = req
                    .upstream_head
                    .as_deref()
                    .map(|h| absolute_form(h, &req.host, req.port));
                (up, early, head)
            }
            Err(e) => return reply(&mut client, "502 Bad Gateway", &e).await,
        },
    };
    let mut rest = rest;
    let sent = match &head {
        None => client
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .is_ok(),
        Some(h) => up.write_all(h.as_bytes()).await.is_ok(),
    };
    if !sent || (!early.is_empty() && client.write_all(&early).await.is_err()) {
        return;
    }
    // An allowed name must also be the TLS server name, because a CDN that routes by SNI would
    // otherwise carry the tunnel to any site it serves.
    if tunnel
        && early.is_empty()
        && let Host::Name(host) = &req.host
    {
        // A TLS client speaks first. When the destination does (an SSH banner), it is not TLS.
        let mut first = [0u8; 4096];
        let hello = tokio::select! {
            h = tokio::time::timeout(HEAD_TIMEOUT, read_hello(&mut client, &mut rest)) => {
                h.unwrap_or(Hello::Invalid)
            }
            n = up.read(&mut first) => {
                match n {
                    Ok(n) if n > 0 => {
                        if client.write_all(&first[..n]).await.is_err() {
                            return;
                        }
                        Hello::NoName
                    }
                    _ => return,
                }
            }
        };
        let refused = match hello {
            Hello::Name(sni) if sni != *host => Some(format!(
                "Connect to `{sni}` by its own name, because the sandbox refuses a TLS server name that differs from the CONNECT host `{host}`."
            )),
            Hello::Invalid => Some(format!(
                "Send a standard TLS ClientHello to `{host}`, because the sandbox could not read the server name in this one."
            )),
            _ => None,
        };
        if let Some(reason) = refused {
            let r = Refusal {
                reason,
                local: false,
                unresolved: false,
            };
            shared.refused(&req.host, req.port, &r);
            return;
        }
    }
    if up.write_all(&rest).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut up).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(s: &str) -> HostRule {
        HostRule::parse(s).unwrap_or_else(|| panic!("{s} parses"))
    }

    fn name(s: &str) -> Host {
        Host::Name(s.into())
    }

    #[test]
    fn rules_parse_hosts_domains_addresses_and_ports() {
        assert_eq!(
            rule("Example.COM."),
            HostRule {
                host: Pattern::Name("example.com".into()),
                port: None
            }
        );
        assert_eq!(
            rule("*.corp.example:8443"),
            HostRule {
                host: Pattern::Subdomains("corp.example".into()),
                port: Some(8443)
            }
        );
        assert_eq!(
            rule("10.0.0.5:3128").host,
            Pattern::Ip("10.0.0.5".parse().unwrap())
        );
        assert_eq!(rule("[::1]:80").host, Pattern::Ip("::1".parse().unwrap()));
        assert_eq!(
            rule("[::ffff:10.0.0.1]").host,
            Pattern::Ip("10.0.0.1".parse().unwrap())
        );
        for bad in [
            "",
            "https://x.dev",
            "x.dev/path",
            "user@x.dev",
            "x.dev:0",
            "x.dev:+1",
            "x.dev:70000",
            "::1",
            "*.",
            "a.*.dev",
            "-a.dev",
            "a b",
            "[::1]x",
        ] {
            assert_eq!(HostRule::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn rules_match_ports_and_subdomains() {
        let r = rule("*.example.com");
        assert!(r.matches(&name("a.example.com"), 443));
        assert!(r.matches(&name("a.b.example.com"), 80));
        assert!(!r.matches(&name("example.com"), 443));
        assert!(!r.matches(&name("badexample.com"), 443));
        assert!(!r.matches(&name("a.example.com"), 8080));
        assert!(rule("example.com:8080").matches(&name("example.com"), 8080));
        assert!(!rule("example.com:8080").matches(&name("example.com"), 443));
        assert!(!rule("1.2.3.4").matches(&name("1.2.3.4.nip.io"), 443));
        assert!(rule("1.2.3.4").matches(&Host::Ip("1.2.3.4".parse().unwrap()), 443));
    }

    #[test]
    fn address_classes_include_mapped_and_metadata_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.0.2",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "fd00::1",
            "fe80::1",
            "64:ff9b::a00:1",
        ] {
            assert!(is_private(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "2606:4700:4700::1111", "::ffff:8.8.8.8"] {
            assert!(!is_private(ip.parse().unwrap()), "{ip}");
        }
    }

    fn policy(network: SandboxNetwork, allowed: &[&str]) -> Policy {
        Policy::new(&SandboxConfig {
            network,
            allowed_hosts: allowed.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        })
    }

    #[test]
    fn the_policy_follows_the_network_choice() {
        let allow = policy(SandboxNetwork::Allowlist, &["mirror.lan"]);
        assert_eq!(
            allow.access(&name("registry.npmjs.org"), 443),
            Access::PublicOnly
        );
        assert_eq!(allow.access(&name("mirror.lan"), 443), Access::AnyAddress);
        assert_eq!(allow.access(&name("evil.example"), 443), Access::Refused);
        // Every harness's model hosts are defaults too, for a test that calls a model API.
        for h in [
            "api.anthropic.com",
            "api.openai.com",
            "generativelanguage.googleapis.com",
            "crates.io",
        ] {
            assert_eq!(allow.access(&name(h), 443), Access::PublicOnly, "{h}");
        }
        let public = policy(SandboxNetwork::Public, &[]);
        assert_eq!(public.access(&name("evil.example"), 22), Access::PublicOnly);
        let mut none = policy(SandboxNetwork::None, &["mirror.lan"]);
        assert!(!none.needs_proxy());
        assert_eq!(none.access(&name("mirror.lan"), 443), Access::Refused);
        none.allow(vec![rule("api.anthropic.com")], vec![]);
        assert!(none.needs_proxy());
        assert_eq!(
            none.access(&name("api.anthropic.com"), 443),
            Access::PublicOnly
        );
        assert!(!policy(SandboxNetwork::Host, &[]).needs_proxy());
        let local = policy(
            SandboxNetwork::Allowlist,
            &[
                "127.0.0.1:8317",
                "localhost:8317",
                "[::1]:9000",
                "localhost",
                "10.0.0.2:80",
            ],
        );
        assert_eq!(local.loopback_ports(), vec![8317, 9000]);
    }

    #[test]
    fn refusals_name_the_setting_and_flag_local_destinations() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let allow = policy(SandboxNetwork::Allowlist, &[]);
        let r = rt
            .block_on(allow.resolve(&name("evil.example"), 443))
            .unwrap_err();
        assert!(
            r.reason
                .starts_with("Add `evil.example` to `[sandbox] allowed_hosts`")
        );
        assert!(!r.local);
        let r = rt
            .block_on(allow.resolve(&Host::Ip("169.254.169.254".parse().unwrap()), 80))
            .unwrap_err();
        assert!(r.local);
        let r = rt
            .block_on(allow.resolve(&name("localhost"), 8080))
            .unwrap_err();
        assert!(
            r.local && r.reason.contains("`localhost:8080`"),
            "{}",
            r.reason
        );
        // A built-in host that resolves to loopback is refused as local.
        let mut p = policy(SandboxNetwork::None, &[]);
        p.allow(vec![rule("localhost:9")], vec![]);
        let r = rt.block_on(p.resolve(&name("localhost"), 9)).unwrap_err();
        assert!(r.local && r.reason.contains("loopback"), "{}", r.reason);
        let public = policy(SandboxNetwork::Public, &[]);
        let r = rt
            .block_on(public.resolve(&Host::Ip("10.0.0.1".parse().unwrap()), 443))
            .unwrap_err();
        assert!(r.local);
    }

    #[test]
    fn plain_http_is_rewritten_to_origin_form() {
        let req = parse_request(
            b"GET http://Example.com:8080?q=1 HTTP/1.1\r\nHost: evil.example\r\nProxy-Authorization: x\r\nConnection: keep-alive\r\nAccept: */*\r\n\r\n",
        )
        .unwrap();
        assert_eq!((req.host, req.port), (name("example.com"), 8080));
        assert_eq!(
            req.upstream_head.unwrap(),
            "GET /?q=1 HTTP/1.1\r\nHost: Example.com:8080\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
        let c = parse_request(b"CONNECT [::1]:443 HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!((c.host, c.port), (Host::Ip("::1".parse().unwrap()), 443));
        for bad in [
            &b"GET https://x.dev/ HTTP/1.1\r\n\r\n"[..],
            b"GET /relative HTTP/1.1\r\n\r\n",
            b"CONNECT x.dev HTTP/1.1\r\n\r\n",
            b"CONNECT u@x.dev:443 HTTP/1.1\r\n\r\n",
            b"GET http://x.dev/ HTTP/2\r\n\r\n",
            b"GET http://x.dev/ HTTP/1.1\r\nA: b\r\n c\r\n\r\n",
        ] {
            assert!(
                parse_request(bad).is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    type Seen = Arc<Mutex<Vec<Decision>>>;

    fn start(p: Policy, dir: &Path) -> (Listener, Seen) {
        let seen: Seen = Arc::default();
        let s = seen.clone();
        let l = proxy(p, dir, Arc::new(move |d| s.lock().unwrap().push(d))).unwrap();
        (l, seen)
    }

    fn exchange(sock: &Path, send: &[u8]) -> String {
        use std::io::{Read, Write};
        let mut s = std::os::unix::net::UnixStream::connect(sock).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(send).unwrap();
        let mut out = vec![];
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    /// A one-shot upstream on loopback that records what it received and answers `answer`.
    fn upstream(answer: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut got = vec![];
            let mut buf = [0u8; 4096];
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                if got.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            s.write_all(answer.as_bytes()).unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });
        (port, t)
    }

    #[test]
    fn a_live_proxy_reaches_public_hosts_through_the_upstream_proxy() {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        // Names under `.invalid` never resolve, and fail at once. A fake corporate proxy: records each request head, refuses one host, tunnels "pong".
        let heads = std::thread::spawn(move || {
            let mut heads = vec![];
            for _ in 0..3 {
                let (mut s, _) = l.accept().unwrap();
                s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut got = vec![];
                let mut buf = [0u8; 1024];
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = s.read(&mut buf).unwrap();
                    got.extend_from_slice(&buf[..n]);
                }
                let head = String::from_utf8_lossy(&got).into_owned();
                let answer: &[u8] = if head.contains("blocked.invalid") {
                    b"HTTP/1.1 403 Forbidden\r\n\r\n"
                } else if head.starts_with("CONNECT") {
                    b"HTTP/1.1 200 OK\r\n\r\npong"
                } else {
                    b"HTTP/1.1 204 No Content\r\n\r\n"
                };
                s.write_all(answer).unwrap();
                heads.push(head);
            }
            heads
        });
        let d = tempfile::tempdir().unwrap();
        let p = Policy::new(&SandboxConfig {
            network: SandboxNetwork::Allowlist,
            allowed_hosts: vec!["pkg.invalid".into(), "blocked.invalid".into()],
            upstream_proxy: Some(format!("http://127.0.0.1:{port}")),
            ..Default::default()
        });
        let (l, _) = start(p, &d.path().join("egress"));
        let out = exchange(l.socket(), b"CONNECT pkg.invalid:443 HTTP/1.1\r\n\r\n");
        assert!(
            out.starts_with("HTTP/1.1 200 Connection established"),
            "{out}"
        );
        assert!(out.ends_with("pong"), "{out}");
        let out = exchange(
            l.socket(),
            b"GET http://pkg.invalid/a?b HTTP/1.1\r\nAccept: */*\r\n\r\n",
        );
        assert!(out.starts_with("HTTP/1.1 204"), "{out}");
        let out = exchange(l.socket(), b"CONNECT blocked.invalid:443 HTTP/1.1\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 502"), "{out}");
        assert!(out.contains("403 Forbidden"), "{out}");
        let out = exchange(l.socket(), b"CONNECT evil.example:443 HTTP/1.1\r\n\r\n");
        assert!(
            out.starts_with("HTTP/1.1 403"),
            "unlisted hosts never reach the upstream: {out}"
        );
        let heads = heads.join().unwrap();
        assert!(
            heads[0].starts_with("CONNECT pkg.invalid:443 HTTP/1.1\r\nHost: pkg.invalid:443"),
            "{}",
            heads[0]
        );
        assert!(
            heads[1].starts_with("GET http://pkg.invalid/a?b HTTP/1.1\r\nHost: pkg.invalid"),
            "{}",
            heads[1]
        );
        assert!(heads[1].contains("Connection: close"), "{}", heads[1]);
        assert!(
            heads[2].starts_with("CONNECT blocked.invalid:443"),
            "{}",
            heads[2]
        );
        let p = Policy::new(&SandboxConfig {
            network: SandboxNetwork::Public,
            upstream_proxy: Some(format!("http://127.0.0.1:{port}")),
            ..Default::default()
        });
        let (l2, _) = start(p, &d.path().join("egress"));
        let out = exchange(
            l2.socket(),
            b"CONNECT printer.invalid.local:443 HTTP/1.1\r\n\r\n",
        );
        assert!(
            out.starts_with("HTTP/1.1 403"),
            "a local-only name never reaches the upstream: {out}"
        );
        assert_eq!(
            parse_upstream("http://proxy.corp:3128/"),
            Some(("proxy.corp".into(), 3128))
        );
        assert_eq!(parse_upstream("http://u:p@proxy.corp:3128"), None);
        assert_eq!(parse_upstream("https://proxy.corp:3128"), None);
        assert_eq!(parse_upstream("http://proxy.corp"), None);
    }

    #[test]
    fn a_live_proxy_checks_the_tls_server_name_against_the_connect_host() {
        use std::io::Read;
        let d = tempfile::tempdir().unwrap();
        let (port, up) = upstream("pong");
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port2 = l.local_addr().unwrap().port();
        let refused_up = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut got = vec![];
            let _ = s.read_to_end(&mut got);
            got
        });
        let (l, seen) = start(
            policy(
                SandboxNetwork::Allowlist,
                &[&format!("localhost:{port}"), &format!("localhost:{port2}")],
            ),
            &d.path().join("egress"),
        );
        let mut send = format!("CONNECT localhost:{port} HTTP/1.1\r\n\r\n").into_bytes();
        send.extend(hello(Some("localhost")));
        let out = exchange(l.socket(), &send);
        assert!(out.ends_with("pong"), "{out}");
        assert!(up.join().unwrap().len() > 40, "the hello went upstream");

        let mut send = format!("CONNECT localhost:{port2} HTTP/1.1\r\n\r\n").into_bytes();
        send.extend(hello(Some("evil.example")));
        let out = exchange(l.socket(), &send);
        assert_eq!(out, "HTTP/1.1 200 Connection established\r\n\r\n");
        assert!(
            refused_up.join().unwrap().is_empty(),
            "nothing went upstream"
        );
        let seen = seen.lock().unwrap().clone();
        let refusal = seen
            .iter()
            .find(|d| !d.allowed)
            .expect("the mismatch is reported");
        assert!(
            refusal
                .reason
                .as_deref()
                .unwrap()
                .contains("`evil.example`")
        );

        // A destination that speaks first is not TLS, so its banner passes at once.
        let banner = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port3 = banner.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::Write;
            let (mut s, _) = banner.accept().unwrap();
            s.write_all(b"SSH-2.0-test\r\n").unwrap();
        });
        let (l3, _) = start(
            policy(SandboxNetwork::Allowlist, &[&format!("localhost:{port3}")]),
            &d.path().join("egress"),
        );
        let started = std::time::Instant::now();
        let out = exchange(
            l3.socket(),
            format!("CONNECT localhost:{port3} HTTP/1.1\r\n\r\n").as_bytes(),
        );
        assert!(out.ends_with("SSH-2.0-test\r\n"), "{out}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    /// A TLS 1.3 ClientHello record, with the server name when given.
    fn hello(sni: Option<&str>) -> Vec<u8> {
        let mut body = vec![3, 3];
        body.extend([7u8; 32]);
        body.extend([0, 0, 2, 0x13, 0x01, 1, 0]);
        let mut exts = vec![];
        if let Some(n) = sni {
            let n = n.as_bytes();
            let mut list = vec![0];
            list.extend((n.len() as u16).to_be_bytes());
            list.extend(n);
            let mut data = (list.len() as u16).to_be_bytes().to_vec();
            data.extend(list);
            exts.extend([0, 0]);
            exts.extend((data.len() as u16).to_be_bytes());
            exts.extend(data);
        }
        exts.extend([0, 43, 0, 3, 2, 3, 4]);
        body.extend((exts.len() as u16).to_be_bytes());
        body.extend(exts);
        let mut hs = vec![1];
        hs.extend(&(body.len() as u32).to_be_bytes()[1..]);
        hs.extend(body);
        let mut rec = vec![0x16, 3, 1];
        rec.extend((hs.len() as u16).to_be_bytes());
        rec.extend(hs);
        rec
    }

    #[test]
    fn the_client_hello_names_its_server() {
        let h = hello(Some("Registry.NPMJS.org."));
        assert_eq!(
            client_hello_name(&h),
            Hello::Name("registry.npmjs.org".into())
        );
        assert_eq!(client_hello_name(&h[..20]), Hello::More);
        assert_eq!(client_hello_name(&hello(None)), Hello::NoName);
        assert_eq!(client_hello_name(b"SSH-2.0-OpenSSH\r\n"), Hello::NoName);
        // The same handshake split over two records.
        let hs = &h[5..];
        let mut split = vec![0x16, 3, 1, 0, 10];
        split.extend(&hs[..10]);
        split.extend([0x16, 3, 1]);
        split.extend(((hs.len() - 10) as u16).to_be_bytes());
        split.extend(&hs[10..]);
        assert_eq!(
            client_hello_name(&split),
            Hello::Name("registry.npmjs.org".into())
        );
        assert_eq!(client_hello_name(&[0x16, 3, 1, 0, 0]), Hello::Invalid);
        let mut not_hello = h.clone();
        not_hello[5] = 2;
        assert_eq!(client_hello_name(&not_hello), Hello::Invalid);
        let mut cut = h.clone();
        cut.truncate(h.len() - 3);
        cut[3..5].copy_from_slice(&((h.len() - 8) as u16).to_be_bytes());
        cut[8] = (h.len() - 12) as u8;
        assert_eq!(
            client_hello_name(&cut),
            Hello::Invalid,
            "extensions overrun the body"
        );
    }

    #[test]
    fn a_live_proxy_allows_listed_hosts_and_refuses_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let (port, up) = upstream("pong");
        let allowed = format!("127.0.0.1:{port}");
        let (l, seen) = start(
            policy(SandboxNetwork::Allowlist, &[&allowed]),
            &d.path().join("egress"),
        );
        let out = exchange(
            l.socket(),
            format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\nping\r\n\r\n").as_bytes(),
        );
        assert!(out.starts_with("HTTP/1.1 200"), "{out}");
        assert!(out.ends_with("pong"), "{out}");
        assert_eq!(up.join().unwrap(), "ping\r\n\r\n");

        let out = exchange(l.socket(), b"CONNECT evil.example:443 HTTP/1.1\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 403"), "{out}");
        assert!(out.contains("allowed_hosts"), "{out}");
        // A retry is refused again but reported once.
        assert!(
            exchange(l.socket(), b"CONNECT evil.example:443 HTTP/1.1\r\n\r\n")
                .starts_with("HTTP/1.1 403")
        );
        let out = exchange(l.socket(), b"CONNECT 127.0.0.1:1 HTTP/1.1\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 403"), "{out}");
        let long = format!(
            "GET http://x.dev/ HTTP/1.1\r\nA: {}\r\n\r\n",
            "a".repeat(9000)
        );
        assert!(exchange(l.socket(), long.as_bytes()).starts_with("HTTP/1.1 400"));

        let (port2, up2) = upstream("HTTP/1.1 204 No Content\r\n\r\n");
        let (l2, _) = start(
            policy(SandboxNetwork::Allowlist, &[&format!("localhost:{port2}")]),
            &d.path().join("egress"),
        );
        let out = exchange(
            l2.socket(),
            format!("GET http://localhost:{port2}/a HTTP/1.1\r\nProxy-Authorization: x\r\n\r\n")
                .as_bytes(),
        );
        assert!(out.starts_with("HTTP/1.1 204"), "{out}");
        let got = up2.join().unwrap();
        assert!(
            got.starts_with("GET /a HTTP/1.1\r\nHost: localhost:"),
            "{got}"
        );
        assert!(
            got.contains("Connection: close") && !got.contains("Proxy-"),
            "{got}"
        );

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert!(seen[0].allowed && seen[0].host == "127.0.0.1");
        assert!(!seen[1].allowed && !seen[1].local);
        assert!(!seen[2].allowed && seen[2].local);
        let sock = l.socket().to_path_buf();
        drop(l);
        assert!(!sock.exists());
    }
}
