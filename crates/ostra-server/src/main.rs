use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ostra",
    version,
    about = "Ostra: a local workspace that runs the engineering pipeline"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(clap::Args, Clone, Default)]
struct ServeArgs {
    /// Port on 127.0.0.1. Defaults to the config's `server.port`, else 7878.
    #[arg(long)]
    port: Option<u16>,
    /// Do not open the browser.
    #[arg(long)]
    no_open: bool,
    /// Also accept the Vite dev server's host (localhost:5173) for frontend work.
    #[arg(long)]
    dev: bool,
    /// Address to listen on: 127.0.0.1 (the default), 0.0.0.0 or :: for every interface, or one
    /// interface's address. Overrides the config's `server.bind`.
    #[arg(long)]
    bind: Option<String>,
    /// An extra host name the browser may use, such as a reverse proxy's domain. Repeatable.
    #[arg(long = "allow-host")]
    allow_host: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the server (the default).
    Serve(ServeArgs),
    /// Hook bridge for harness executions. Harnesses call this; you do not.
    /// Arguments: `--execution <id> --harness <name> --event <event>`.
    Hook {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// MCP stdio shim for harness executions. Harnesses call this; you do not.
    McpStdio {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Print the config path, writing the default config if none exists.
    Config,
    /// Print a fresh sign-in URL for a running server.
    Url,
    /// List the browsers signed in to Ostra.
    Signins,
    /// Sign out every browser. Open pages disconnect within 30 seconds.
    Signout,
    /// Stop a session while the server is not running, so the next start does not recover and
    /// re-run its executions. With the server running, stop it from its session board instead.
    Stop {
        /// Session id, for example s_01a0cdab436570b0b522e10ce263a904.
        session: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // SAFETY: no other thread exists yet; the runtime starts below.
    unsafe { ostra_server::env::extend_path() };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    match cli.command {
        Some(Command::Hook { args }) => {
            let code = runtime.block_on(ostra_server::bridge::hook_cli(args));
            std::process::exit(code);
        }
        Some(Command::McpStdio { args }) => {
            let code = runtime.block_on(ostra_server::bridge::mcp_cli(args));
            std::process::exit(code);
        }
        Some(Command::Config) => {
            let path = ostra_server::app::ensure_global_config()?;
            println!("{}", path.display());
            Ok(())
        }
        Some(Command::Stop { session }) => {
            let n = ostra_server::app::stop_offline(&session)?;
            println!("Stopped {session}; {n} running executions were cancelled.");
            Ok(())
        }
        Some(Command::Signins) => {
            let registry = ostra_store::RegistryDb::open(&ostra_core::paths::registry_db_path())?;
            let list = ostra_server::auth::list_sign_ins(&registry)?;
            if list.is_empty() {
                println!("No browser is signed in.");
            }
            let date = |s: u64| {
                chrono::DateTime::from_timestamp(s as i64, 0)
                    .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
                    .unwrap_or_default()
            };
            for s in list {
                println!(
                    "{}  signed in {}  expires {}",
                    s.id,
                    date(s.created),
                    date(s.expires)
                );
            }
            Ok(())
        }
        Some(Command::Signout) => {
            let registry = ostra_store::RegistryDb::open(&ostra_core::paths::registry_db_path())?;
            let n = ostra_server::auth::revoke_sign_ins(&registry)?;
            println!("Signed out {n} browsers. Run `ostra url` to sign in again.");
            Ok(())
        }
        Some(Command::Url) => {
            let url = ostra_server::auth::request_url()?;
            println!("{url}");
            Ok(())
        }
        Some(Command::Serve(args)) => runtime.block_on(serve(args)),
        None => runtime.block_on(serve(cli.serve)),
    }
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_env("OSTRA_LOG").unwrap_or_else(|_| "info".into())
    };
    let dir = ostra_core::paths::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    // Also keep a log file, so problems in a browser elsewhere can be read back later.
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("server.log"))
    {
        Ok(file) => tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_target(false)
            .with_ansi(false)
            .with_writer(std::io::stdout.and(std::sync::Mutex::new(file)))
            .init(),
        Err(_) => tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_target(false)
            .init(),
    }
    let opts = ostra_server::app::ServeOptions {
        port: args.port,
        open_browser: !args.no_open,
        dev: args.dev,
        bind: args.bind,
        allow_hosts: args.allow_host,
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ostra")),
    };
    ostra_server::app::run(opts).await
}
