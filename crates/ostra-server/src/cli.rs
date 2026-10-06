//! The `ostra` command line.

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
enum SessionsAction {
    /// Revoke one sign-in by the id `ostra sessions` lists, or every one with --all.
    Revoke {
        /// Sign-in id, for example si_3f2a9c1d7e5b6a40.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        id: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum PluginAction {
    /// Register a plugin program in a workspace's `[[plugins]]`. Ostra starts it in the workspace
    /// folder and speaks JSON-RPC with it on stdio, once the folder file is approved.
    Add {
        /// The plugin's name, the same as the name in its manifest, for example `release-gate`.
        name: String,
        /// The program and its arguments, after `--`. A relative program path is relative to the
        /// workspace folder.
        #[arg(last = true, required = true)]
        command: Vec<String>,
        /// A folder inside the workspace. Defaults to the current folder.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// An environment variable for the program, as KEY=VALUE. Repeatable.
        #[arg(long = "env", value_parser = parse_env)]
        env: Vec<(String, String)>,
        /// Seconds one stage decision may take.
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        /// Register it without starting it.
        #[arg(long)]
        disabled: bool,
    },
}

fn parse_env(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((k, v)) if !k.is_empty() => Ok((k.to_string(), v.to_string())),
        _ => Err(format!("write it as KEY=VALUE, not `{s}`")),
    }
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
    /// List the browsers signed in to Ostra, or revoke their sign-ins. Works whether or not the
    /// server runs; a running server refuses a revoked browser within 5 seconds.
    Sessions {
        #[command(subcommand)]
        action: Option<SessionsAction>,
    },
    /// Sign out every browser, the same as `ostra sessions revoke --all`.
    Signout,
    /// Manage a workspace's plugin programs.
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Stop a session while the server is not running, so the next start does not recover and
    /// re-run its executions. With the server running, stop it from its session board instead.
    Stop {
        /// Session id, for example s_01a0cdab436570b0b522e10ce263a904.
        session: String,
    },
}

/// Run the `ostra` command line with the plugins built into this binary (HANDOVER 10.10). The
/// `ostra` binary passes none; a binary built with `ostra-sdk` passes its own.
pub fn main_with(plugins: ostra_sdk::Registry) -> anyhow::Result<()> {
    // Inside a sandbox, before a runtime or logging starts. Hidden from `--help` on purpose.
    let mut raw = std::env::args_os().skip(1);
    if raw.next().is_some_and(|a| a == "sandbox-init") {
        std::process::exit(ostra_sandbox::init::main(raw.collect()));
    }
    // Before anything reads the environment or starts a thread.
    ostra_sandbox::scrub_startup_env();
    if let Ok(exe) = std::env::current_exe() {
        ostra_sandbox::set_helper(exe);
    }
    let cli = Cli::parse();
    ostra_default_plugin::install();
    // SAFETY: no other thread exists yet; the runtime starts below.
    unsafe { crate::env::extend_path() };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    match cli.command {
        Some(Command::Hook { args }) => {
            let code = runtime.block_on(crate::bridge::hook_cli(args));
            std::process::exit(code);
        }
        Some(Command::McpStdio { args }) => {
            let code = runtime.block_on(crate::bridge::mcp_cli(args));
            std::process::exit(code);
        }
        Some(Command::Config) => {
            let path = crate::app::ensure_global_config()?;
            println!("{}", path.display());
            Ok(())
        }
        Some(Command::Plugin {
            action:
                PluginAction::Add {
                    name,
                    command,
                    workspace,
                    env,
                    timeout,
                    disabled,
                },
        }) => {
            let entry = ostra_core::plugin::PluginConfig {
                name: name.clone(),
                command,
                env: env.into_iter().collect(),
                enabled: !disabled,
                timeout_secs: timeout,
            };
            let added = crate::app::add_plugin(&workspace, entry)?;
            println!(
                "Registered plugin {name} in {}.",
                ostra_core::paths::workspace_toml(&added.workspace).display()
            );
            if !added.approved {
                println!(
                    "It starts once you approve the workspace's folder file in the console, because the file waits for approval."
                );
            } else if disabled {
                println!("It is disabled; set `enabled = true` in its entry to start it.");
            }
            Ok(())
        }
        Some(Command::Stop { session }) => {
            let n = crate::app::stop_offline(&session)?;
            println!("Stopped {session}; {n} running executions were cancelled.");
            Ok(())
        }
        Some(Command::Sessions { action: None }) => {
            let registry = crate::app::open_registry()?;
            let list = crate::auth::list_sign_ins(&registry)?;
            if list.is_empty() {
                println!("No browser is signed in.");
            }
            let date =
                |d: chrono::DateTime<chrono::Utc>| d.format("%Y-%m-%d %H:%M UTC").to_string();
            for s in list {
                println!(
                    "{}  signed in {}  last seen {}  from {}  {}",
                    s.id,
                    date(s.created),
                    date(s.last_seen),
                    s.ip.as_deref().unwrap_or("unknown address"),
                    s.user_agent.as_deref().unwrap_or("unknown browser"),
                );
            }
            Ok(())
        }
        Some(Command::Sessions {
            action: Some(SessionsAction::Revoke { id, all }),
        }) => {
            let registry = crate::app::open_registry()?;
            if all {
                let n = crate::auth::revoke_sign_ins(&registry)?;
                println!("Signed out {n} browsers. Run `ostra url` to sign in again.");
            } else {
                let id = id.unwrap_or_default();
                if !crate::auth::revoke_sign_in(&registry, &id)? {
                    anyhow::bail!(
                        "No sign-in {id}. Run `ostra sessions` to list the current ones."
                    );
                }
                println!("Revoked {id}. That browser is signed out within 5 seconds.");
            }
            Ok(())
        }
        Some(Command::Signout) => {
            let registry = crate::app::open_registry()?;
            let n = crate::auth::revoke_sign_ins(&registry)?;
            println!("Signed out {n} browsers. Run `ostra url` to sign in again.");
            Ok(())
        }
        Some(Command::Url) => {
            let url = crate::auth::request_url()?;
            println!("{url}");
            Ok(())
        }
        Some(Command::Serve(args)) => runtime.block_on(serve(args, plugins)),
        None => runtime.block_on(serve(cli.serve, plugins)),
    }
}

async fn serve(args: ServeArgs, plugins: ostra_sdk::Registry) -> anyhow::Result<()> {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_env("OSTRA_LOG").unwrap_or_else(|_| "info".into())
    };
    let dir = ostra_core::paths::ensure_data_dir()?;
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
    let opts = crate::app::ServeOptions {
        port: args.port,
        open_browser: !args.no_open,
        dev: args.dev,
        bind: args.bind,
        allow_hosts: args.allow_host,
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ostra")),
        plugins,
    };
    crate::app::run(opts).await
}
