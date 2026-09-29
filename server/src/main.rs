//! Workbench — an AI-centric developer workspace.
//!
//! One Rust process serves the React UI, a REST/WebSocket API, the PTYs that
//! host Claude Code sessions, and an MCP endpoint those sessions use to drive
//! the workspace. See `docs/ARCHITECTURE.md`.

mod app;
mod auth;
mod config;
mod error;
mod events;
mod forge;
mod mcp;
mod projects;
mod secrets;
mod spa;
mod util;

// Feature slices. Each owns its module directory and its `web/src/features/<slice>/`.
mod apps;
mod atlassian;
mod db;
mod debug;
mod devcontainer;
mod files;
mod git;
mod github;
mod gitlab;
mod lsp;
mod platform;
mod terminals;
mod workspace;

use std::net::SocketAddr;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "workbench", version, about = "AI-centric developer workspace")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (the default).
    Serve {
        /// Address to bind, overriding `server.bind` in config.toml.
        #[arg(long)]
        bind: Option<String>,
        /// Open the UI in a browser window once the server is up.
        #[arg(long)]
        open: bool,
    },
    /// Open the UI of the running server in a browser window.
    Open,
    /// Print a login URL for the running server (it carries the master token: keep
    /// it to yourself; `open` uses a one-time code instead).
    Url,
    /// Git credential prompt helper (GIT_ASKPASS). Prints the answer to the prompt.
    Askpass { prompt: Option<String> },
    /// Non-interactive git editor for Workbench's interactive rebase
    /// (GIT_SEQUENCE_EDITOR / GIT_EDITOR): writes the prepared todo or message.
    #[command(name = "git-editor")]
    GitEditor { mode: String, dir: String, file: String },
    /// Claude Code status line helper: reads the status JSON on stdin, reports it
    /// to the server and prints a compact line.
    Statusline,
    // Linux: a systemd user service; Windows: a sign-in entry (platform::service).
    #[command(about = platform::service::ABOUT)]
    Service(platform::service::ServiceArgs),
}

fn main() -> anyhow::Result<()> {
    // Windows: git and ssh start this executable itself as their askpass program, with the
    // prompt as the only argument (`util::os::helper`).
    if let Some(prompt) = util::os::helper::askpass_prompt(is_command) {
        return git::cli_askpass(&prompt);
    }
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve { bind: None, open: false }) {
        Command::Serve { bind, open } => serve(bind, open),
        Command::Open => {
            let url = app::launch_url_of_running_server()?;
            util::open_in_browser(&url);
            Ok(())
        }
        Command::Url => {
            println!("{}", app::login_url_of_running_server()?);
            Ok(())
        }
        Command::Askpass { prompt } => git::cli_askpass(prompt.as_deref().unwrap_or("")),
        Command::GitEditor { mode, dir, file } => git::cli_git_editor(&mode, &dir, &file),
        Command::Statusline => terminals::cli_statusline(),
        Command::Service(args) => platform::service::cli(args),
    }
}

/// Whether `word`, a single argument, is a command line of Workbench's own (a subcommand,
/// `help`, an option) rather than a prompt.
fn is_command(word: &str) -> bool {
    use clap::CommandFactory;
    word.starts_with('-') || word == "help" || Cli::command().find_subcommand(word).is_some()
}

fn serve(bind: Option<String>, open: bool) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("WORKBENCH_LOG")
                .unwrap_or_else(|_| "workbench=info,tower_http=warn".into()),
        )
        .with_target(false)
        .init();

    // A Workbench started from inside a Claude Code session must not leak that
    // session's identity into the sessions it hosts.
    util::proc::scrub_own_env();

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async move {
        let paths = config::Paths::from_env()?;
        let cfg = config::GlobalConfig::load_or_init(&paths)?;
        let bind = bind.unwrap_or_else(|| cfg.server.bind.clone());
        let addr: SocketAddr = bind.parse().with_context(|| format!("bad bind address {bind:?}"))?;

        // [server.tls]: load the certificate before binding so a bad path fails fast.
        let tls = match &cfg.server.tls {
            Some(t) => Some(platform::tls::acceptor(t)?),
            None => None,
        };

        let listener = util::os::net::bind(addr)
            .await
            .with_context(|| format!("cannot bind {addr} (is another Workbench running?)"))?;
        let addr = listener.local_addr()?;
        // Hooks, MCP, askpass, the statusline helper and `workbench open` always use
        // http://127.0.0.1:<port> (`AppState::local_base_url`). When the bind address
        // does not cover it (just a Tailscale or LAN address, [::1], 127.0.0.2…), listen
        // there too, plain HTTP.
        let loopback = match app::loopback_listen_addr(addr) {
            Some(local) => Some(tokio::net::TcpListener::bind(local).await.with_context(|| {
                format!("cannot also listen on {local}, which local helpers (agent hooks, MCP, git askpass) use; free that port or bind 0.0.0.0")
            })?),
            None => None,
        };

        let state = app::AppState::new(paths, cfg, addr).await?;
        let router = app::build_router(state.clone());
        app::start_background(&state).await;
        app::write_runtime_file(&state)?;

        let url = state.login_url();
        let scheme = if tls.is_some() { "https (and http from this computer)" } else { "http" };
        tracing::info!("workbench listening on {addr} ({scheme})");
        // The login URL carries the master token: show it on an interactive terminal
        // only, never in a service log.
        if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            println!("Workbench: {url}");
        } else {
            println!("Workbench: {} (run `workbench open` to sign in)", state.local_base_url());
        }
        if open {
            // A one-time code, not the token: argv is readable by every local process.
            util::open_in_browser(&format!("{}{}", state.local_base_url(), state.auth.local_login_path()));
        }

        let stop = tokio_util::sync::CancellationToken::new();
        {
            let (state, stop) = (state.clone(), stop.clone());
            tokio::spawn(async move {
                util::os::proc::shutdown_signal(&state.paths.data_dir).await;
                tracing::info!("shutting down");
                app::shutdown(&state).await;
                stop.cancel();
            });
        }
        if let Some(local) = loopback {
            tracing::info!("also listening on {} (http, for local helpers)", local.local_addr()?);
            let (router, stop) = (router.clone(), stop.clone());
            tokio::spawn(async move {
                let served = axum::serve(local, router.into_make_service_with_connect_info::<SocketAddr>())
                    .with_graceful_shutdown(stop.cancelled_owned())
                    .await;
                if let Err(e) = served {
                    tracing::error!("loopback listener failed: {e}");
                }
            });
        }
        match tls {
            None => {
                axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                    .with_graceful_shutdown(stop.cancelled_owned())
                    .await?
            }
            Some(acceptor) => {
                use axum::serve::ListenerExt;
                // `tap_io` gives the listener axum's `ConnectInfo<SocketAddr>` support.
                let listener = platform::tls::TlsListener::new(listener, acceptor)?.tap_io(|_| {});
                axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                    .with_graceful_shutdown(stop.cancelled_owned())
                    .await?
            }
        }
        Ok(())
    })
}
