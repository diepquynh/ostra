//! Process-wide state and startup.

use crate::auth::Auth;
use crate::env::EnvStatus;
use crate::files::Files;
use crate::workspace::WorkspaceRt;
use anyhow::Context;
use ostra_core::api::ServerMsg;
use ostra_core::config::{GlobalConfig, load_toml, save_toml};
use ostra_core::ids::WorkspaceId;
use ostra_core::paths;
use ostra_engine::EngineNotice;
use ostra_exec_native::NativeExecutor;
use ostra_notify::{Notifier, VapidKeys};
use ostra_providers::Providers;
use ostra_store::RegistryDb;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::broadcast;

pub const DEFAULT_PORT: u16 = 7878;

pub struct ServeOptions {
    pub port: Option<u16>,
    pub open_browser: bool,
    pub dev: bool,
    pub exe: PathBuf,
    /// Listen address; overrides the config's `server.bind`.
    pub bind: Option<String>,
    /// Extra host names, added to the config's `server.allowed_hosts`.
    pub allow_hosts: Vec<String>,
}

/// The listen address: the flag, else the config, else 127.0.0.1.
pub fn bind_addr(opts: &ServeOptions, global: &GlobalConfig) -> anyhow::Result<std::net::IpAddr> {
    let raw = opts
        .bind
        .clone()
        .or_else(|| global.server.bind.clone())
        .unwrap_or_else(|| "127.0.0.1".into());
    let raw = raw.trim().trim_start_matches('[').trim_end_matches(']');
    match raw {
        "localhost" => Ok(std::net::IpAddr::from([127, 0, 0, 1])),
        "all" | "*" => Ok(std::net::IpAddr::from([0, 0, 0, 0])),
        other => other.parse().map_err(|_| {
            anyhow::anyhow!(
                "`{other}` is not an IP address to listen on (for example 127.0.0.1 or 0.0.0.0)"
            )
        }),
    }
}

pub fn access(
    opts: &ServeOptions,
    global: &GlobalConfig,
    port: u16,
) -> anyhow::Result<crate::auth::Access> {
    let mut extra_hosts = global.server.allowed_hosts.clone();
    extra_hosts.extend(opts.allow_hosts.iter().cloned());
    Ok(crate::auth::Access {
        port,
        bind: bind_addr(opts, global)?,
        extra_hosts,
        dev: opts.dev,
        private_host: !global.server.use_ip_host,
    })
}

/// Everything shared by every workspace.
pub struct Shared {
    pub global_path: PathBuf,
    pub global_cache: RwLock<GlobalConfig>,
    pub registry: RegistryDb,
    pub providers: Arc<Providers>,
    pub native: Arc<NativeExecutor>,
    pub harness: crate::bridge::HarnessRuntime,
    pub notifier: Arc<Notifier>,
    /// Connections to each workspace's external MCP servers.
    pub mcp: Arc<crate::mcp::McpGateway>,
    pub env: RwLock<EnvStatus>,
    pub exe: PathBuf,
    pub port: u16,
}

impl Shared {
    /// Re-read on every call, so edits apply without a restart. A broken file keeps the last
    /// good copy.
    pub fn global(&self) -> GlobalConfig {
        match load_toml::<GlobalConfig>(&self.global_path) {
            Ok(g) if !ostra_core::config::validate_sandbox(&g.sandbox).is_empty() => {
                for i in ostra_core::config::validate_sandbox(&g.sandbox) {
                    tracing::warn!(
                        "{}: {}; using the last good global config",
                        i.path,
                        i.message
                    );
                }
                self.global_cache.read().clone()
            }
            Ok(g) => {
                *self.global_cache.write() = g.clone();
                g
            }
            Err(e) => {
                tracing::warn!("{e}; using the last good global config");
                self.global_cache.read().clone()
            }
        }
    }

    pub fn protected_paths(&self) -> Vec<PathBuf> {
        vec![
            self.exe.clone(),
            self.global_path.clone(),
            paths::data_dir(),
            ostra_agents::assets_dir(),
        ]
    }
}

/// A notice from one workspace's engine, tagged with the workspace.
#[derive(Clone, Debug)]
pub struct HubMsg {
    pub workspace: WorkspaceId,
    pub notice: EngineNotice,
}

/// A message the server itself originates for the browser, with the channels it goes to.
#[derive(Clone, Debug)]
pub struct Pushed {
    pub channels: Vec<String>,
    pub msg: ServerMsg,
}

pub struct App {
    pub shared: Arc<Shared>,
    pub workspaces: RwLock<HashMap<WorkspaceId, Arc<WorkspaceRt>>>,
    pub hub: broadcast::Sender<HubMsg>,
    pub auth: Auth,
    pub dev: bool,
    /// Project file trees, reads, diffs, the file name index, and write tracking.
    pub files: Arc<Files>,
    /// Display tokens, outline, usages, and dependencies for the Files view.
    pub code: crate::code::Code,
    /// The agents' code navigation tools, over the same indexes.
    pub code_tools: Arc<crate::code::CodeTools>,
    pub push: broadcast::Sender<Pushed>,
    /// The Sessions tree, search, and workspace activity.
    pub nav: Arc<crate::nav::Nav>,
}

#[derive(Debug)]
pub enum DeleteError {
    NotFound(String),
    Busy(String),
    Store(ostra_store::StoreError),
}

impl From<ostra_store::StoreError> for DeleteError {
    fn from(e: ostra_store::StoreError) -> Self {
        DeleteError::Store(e)
    }
}

/// Opens the registry with owner-only modes on the database and its SQLite side files, because it
/// holds provider keys, git credentials, and the push signing key.
pub fn open_registry() -> anyhow::Result<RegistryDb> {
    paths::ensure_data_dir()?;
    let db = paths::registry_db_path();
    let registry = RegistryDb::open(&db)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for suffix in ["", "-wal", "-shm"] {
            let mut p = db.clone().into_os_string();
            p.push(suffix);
            match std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                r => r?,
            }
        }
    }
    Ok(registry)
}

/// `ostra stop`: end a session while no server holds it.
pub fn stop_offline(session: &str) -> anyhow::Result<usize> {
    if crate::auth::server_running() {
        anyhow::bail!(
            "The Ostra server is running. Stop the session from its board (or POST /api/sessions/{session}/stop), or stop the server first."
        );
    }
    let registry = open_registry()?;
    let id = ostra_core::ids::SessionId::from(session);
    for w in registry.list_workspaces()? {
        let db_path = paths::workspace_db(&w.root);
        if !db_path.exists() {
            continue;
        }
        let db = ostra_store::WorkspaceDb::open(&db_path)?;
        if db.get_session(&id)?.is_some() {
            return ostra_engine::runner::stop_session_offline(&db, &id)
                .map_err(|e| anyhow::anyhow!("{e}"));
        }
    }
    anyhow::bail!("No registered workspace has a session {session}.")
}

pub fn ensure_global_config() -> anyhow::Result<PathBuf> {
    let path = paths::global_config_path();
    if !path.exists() {
        save_toml(&path, &GlobalConfig::default())
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(path)
}

fn vapid_keys(registry: &RegistryDb) -> anyhow::Result<VapidKeys> {
    if let Some(bytes) = registry.secret_get("vapid_private")?
        && let Ok(keys) = VapidKeys::from_private_bytes(&bytes)
    {
        return Ok(keys);
    }
    let keys = VapidKeys::generate();
    registry.secret_set("vapid_private", &keys.private_bytes())?;
    Ok(keys)
}

pub fn skill_resolver() -> ostra_tools::SkillResolver {
    Arc::new(|name: &str| {
        let content = ostra_agents::embedded_skill(name)?;
        Some((
            ostra_agents::assets_dir()
                .join("skills")
                .join(name)
                .join("SKILL.md"),
            content.to_string(),
        ))
    })
}

impl App {
    pub fn workspace(&self, id: &WorkspaceId) -> Option<Arc<WorkspaceRt>> {
        self.workspaces.read().get(id).cloned()
    }

    pub fn all_workspaces(&self) -> Vec<Arc<WorkspaceRt>> {
        self.workspaces.read().values().cloned().collect()
    }

    /// Unregister a workspace and delete `workspace.toml` and `workspace.db`. Project folders,
    /// session artifact folders, and per-project `.ostra/` files stay on disk.
    pub fn delete_workspace(&self, id: &WorkspaceId) -> Result<(), DeleteError> {
        let record = self
            .shared
            .registry
            .get_workspace(id)?
            .ok_or_else(|| DeleteError::NotFound(format!("No workspace {id}.")))?;
        {
            // Held across the check, so no request reaches the engine between the check and removal.
            let mut open = self.workspaces.write();
            if let Some(rt) = open.get(id) {
                if let Some(reason) = rt.busy()? {
                    return Err(DeleteError::Busy(reason));
                }
                open.remove(id);
            }
        }
        self.shared.registry.remove_workspace(id)?;
        crate::trust::forget(&self.shared.registry, &record.root);
        self.nav.forget(id);
        self.shared.mcp.forget(&record.root);
        let _ = self.push.send(Pushed {
            channels: vec![format!("workspace:{id}"), "home".into()],
            msg: ServerMsg::WorkspaceUpdated {
                workspace: id.clone(),
            },
        });
        let db = paths::workspace_db(&record.root);
        let mut files = vec![paths::workspace_toml(&record.root), db.clone()];
        files.extend(["-wal", "-shm"].map(|s| {
            let mut p = db.clone().into_os_string();
            p.push(s);
            PathBuf::from(p)
        }));
        for f in files {
            match std::fs::remove_file(&f) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("deleting {}: {e}", f.display()),
            }
        }
        Ok(())
    }

    /// Open a registered workspace, recover its sessions, and forward its engine's notices.
    pub fn attach(
        self: &Arc<Self>,
        id: WorkspaceId,
        root: &Path,
    ) -> anyhow::Result<Arc<WorkspaceRt>> {
        if let Some(w) = self.workspace(&id) {
            return Ok(w);
        }
        let rt = Arc::new(WorkspaceRt::open(self.shared.clone(), id.clone(), root)?);
        rt.engine
            .recover()
            .map_err(|e| anyhow::anyhow!("recovering {}: {e}", root.display()))?;
        let mut rx = rt.engine.subscribe();
        let hub = self.hub.clone();
        let files = self.files.clone();
        let nav = self.nav.clone();
        let wid = id.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(notice) => {
                        files.observe(&wid, &notice);
                        nav.observe(&wid, &notice);
                        let _ = hub.send(HubMsg {
                            workspace: wid.clone(),
                            notice,
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("hub lagged by {n} notices");
                        nav.lagged(&wid);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        self.workspaces.write().insert(id, rt.clone());
        Ok(rt)
    }
}

pub async fn build(opts: &ServeOptions, port: u16) -> anyhow::Result<Arc<App>> {
    let global_path = ensure_global_config()?;
    let global: GlobalConfig =
        load_toml(&global_path).with_context(|| format!("reading {}", global_path.display()))?;
    if let Some(i) = ostra_core::config::validate_sandbox(&global.sandbox).first() {
        anyhow::bail!("{}: {} ({})", i.path, i.message, global_path.display());
    }
    let sandbox = ostra_core::api::SandboxStatus::check(&global.sandbox);
    match &sandbox.message {
        None if sandbox.active => tracing::info!("agent commands run in the bubblewrap sandbox"),
        Some(m) => {
            tracing::warn!("{m}");
            // On stderr as well, because under `mode = "auto"` Ostra still starts and the log is
            // easy to miss.
            eprintln!(
                "\nWARNING: agent commands run WITHOUT a sandbox, with your user's full rights.\n{m}\n"
            );
        }
        None => tracing::warn!(
            "the sandbox is off (`[sandbox] mode = \"off\"`), so agent commands run with your user's full rights"
        ),
    }
    paths::ensure_data_dir()?;
    crate::prices::start(&paths::data_dir());
    let registry = crate::master_key::unlock(open_registry()?)?;
    let assets = paths::data_dir().join("assets");
    ostra_agents::set_assets_dir(assets.clone());
    ostra_agents::materialize_assets(&assets)
        .map_err(|e| anyhow::anyhow!("writing assets: {e}"))?;
    let providers = Arc::new(Providers::from_config(
        &global,
        &crate::credentials::load_all(&registry)?,
    ));
    let code_tools = Arc::new(crate::code::CodeTools::default());
    let code_nav: Arc<dyn ostra_tools::CodeNav> = code_tools.clone();
    let mcp = crate::mcp::McpGateway::new(registry.clone());
    let mcp_connector: Arc<dyn ostra_tools::McpConnector> =
        Arc::new(crate::mcp::Connector(mcp.clone()));
    let native = Arc::new(
        NativeExecutor::new(providers.clone(), skill_resolver(), Some(code_nav.clone()))
            .with_mcp(mcp_connector.clone()),
    );
    let notifier = Arc::new(Notifier::new(
        vapid_keys(&registry)?,
        "mailto:ostra@localhost".into(),
    ));
    let env = EnvStatus::detect(&global).await;
    let access = access(opts, &global, port)?;
    // Harness hooks call back on an address the server listens on.
    let callback = if access.bind.is_unspecified() || access.bind.is_loopback() {
        format!("127.0.0.1:{port}")
    } else {
        SocketAddr::new(access.bind, port).to_string()
    };
    let harness = crate::bridge::HarnessRuntime::new(opts.exe.clone(), callback);
    let (touches, touch_rx) = tokio::sync::mpsc::unbounded_channel();
    harness.set_write_sink(touches.clone());
    harness.set_code_nav(code_nav);
    harness.set_mcp(mcp_connector);
    let shared = Arc::new(Shared {
        global_path,
        global_cache: RwLock::new(global),
        registry,
        providers,
        native,
        harness,
        notifier,
        mcp,
        env: RwLock::new(env),
        exe: opts.exe.clone(),
        port,
    });
    let (hub, _) = broadcast::channel(8192);
    let (push, _) = broadcast::channel(1024);
    let app = Arc::new(App {
        auth: Auth::new(shared.registry.clone(), &access),
        shared,
        workspaces: RwLock::new(HashMap::new()),
        hub,
        dev: opts.dev,
        files: Arc::new(Files::new(touches)),
        code: Default::default(),
        code_tools: code_tools.clone(),
        push,
        nav: Default::default(),
    });
    code_tools.bind(&app);
    tokio::spawn(crate::files::run_touches(Arc::downgrade(&app), touch_rx));
    tokio::spawn(crate::nav::run(Arc::downgrade(&app)));
    for record in app.shared.registry.list_workspaces()? {
        if !paths::workspace_toml(&record.root).exists() {
            tracing::warn!(
                "workspace {} is missing at {}",
                record.name,
                record.root.display()
            );
            continue;
        }
        if let Err(e) = app.attach(record.id.clone(), &record.root) {
            tracing::error!("could not open workspace {}: {e:#}", record.name);
        }
    }
    Ok(app)
}

async fn bind(ip: std::net::IpAddr, port: Option<u16>) -> anyhow::Result<tokio::net::TcpListener> {
    let start = port.unwrap_or(DEFAULT_PORT);
    if port.is_some() {
        return Ok(tokio::net::TcpListener::bind(SocketAddr::new(ip, start)).await?);
    }
    for p in start..start + 20 {
        if let Ok(l) = tokio::net::TcpListener::bind(SocketAddr::new(ip, p)).await {
            return Ok(l);
        }
    }
    Ok(tokio::net::TcpListener::bind(SocketAddr::new(ip, 0)).await?)
}

pub async fn run(opts: ServeOptions) -> anyhow::Result<()> {
    let global = ensure_global_config()
        .ok()
        .and_then(|p| load_toml::<GlobalConfig>(&p).ok())
        .unwrap_or_default();
    let ip = bind_addr(&opts, &global)?;
    let listener = bind(ip, opts.port.or(global.server.port)).await?;
    let port = listener.local_addr()?.port();
    let app = build(&opts, port).await?;
    crate::auth::write_server_file(&app.auth)?;
    let url = app.auth.sign_in_url()?;
    println!(
        "Ostra is running on {}. Open this URL to sign in:\n\n  {url}\n",
        listener.local_addr()?
    );
    if !ip.is_loopback() {
        let lan = access(&opts, &global, port)?.lan_addresses();
        if lan.len() > 1 {
            let others: Vec<String> = lan.iter().skip(1).map(|a| a.to_string()).collect();
            println!(
                "The link works once, from any address this machine has ({}). Run `ostra url` for another.\n",
                others.join(", ")
            );
        }
        println!(
            "Ostra is reachable from your network over plain HTTP. Anyone who reaches this port with a sign-in link can run commands as you. Traffic is not encrypted, so anyone on the network path can read the sign-in cookie and what you type into harness terminals. Prefer an SSH tunnel or a TLS reverse proxy (add its host name with --allow-host). Push notifications need HTTPS or localhost. Run `ostra signout` to end every sign-in.\n"
        );
    }
    if opts.open_browser && open::that(&url).is_err() {
        println!("Could not open a browser; copy the URL above.");
    }
    let router = crate::api::router(app.clone());
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    crate::auth::remove_server_file();
    Ok(())
}
