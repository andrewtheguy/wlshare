//! swayrx: a VNC server for a sway desktop that tells the remotex gateway what
//! pixel density its framebuffer is drawn at.
//!
//! Two halves. One thread owns the Wayland connection and everything on it —
//! capture, outputs, input, clipboard — and runs a calloop that also polls a
//! command channel ([`compositor`]). The tokio runtime accepts clients and runs
//! one task per connection ([`session`]). They share the framebuffer and a few
//! channels ([`shared`]), and every protocol byte comes from the `swayrx-rfb`
//! crate.

mod capture;
mod clipboard;
mod compositor;
mod config;
mod framebuffer;
mod input;
mod outputs;
mod session;
mod shared;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use log::{error, info};

#[derive(Parser, Debug)]
#[command(name = "swayrx", version, about = "A VNC server for a sway desktop, with pixel density on the wire")]
struct Args {
    /// The configuration file (default: $XDG_CONFIG_HOME/swayrx/config.toml).
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Listen on this address instead of the configured one.
    #[arg(long)]
    listen: Option<std::net::SocketAddr>,
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let path = args.config.unwrap_or_else(config::default_path);
    let mut config = if path.exists() {
        config::Config::load(&path)?
    } else {
        info!("no configuration at {}; using defaults", path.display());
        toml::from_str("").context("default configuration")?
    };
    if let Some(listen) = args.listen {
        config.listen = listen;
    }
    let password = config.password()?;
    if password.is_none() {
        info!("no password_file: accepting clients without authentication");
    }

    // The compositor thread comes up first and hands back what it learned about
    // the output, so ServerInit can name a size before the first frame.
    let (compositor, shared) = compositor::start(&config)?;

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let result = runtime.block_on(serve(config, password, shared.clone(), compositor));
    if let Err(e) = &result {
        error!("{e:#}");
    }
    result
}

async fn serve(
    config: config::Config,
    password: Option<String>,
    shared: Arc<shared::Shared>,
    mut compositor: compositor::Handle,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.listen).await.with_context(|| format!("listening on {}", config.listen))?;
    info!("listening on {}", config.listen);
    let session_config = Arc::new(session::SessionConfig { password, name: config.name.clone(), resize: config.resize });
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (socket, peer) = accepted.context("accepting a client")?;
                let id = shared.next_client();
                info!("client {} connected from {peer}", id.0);
                let shared = shared.clone();
                let session_config = session_config.clone();
                tokio::spawn(async move {
                    if let Err(e) = session::run(id, socket, shared.clone(), session_config).await {
                        info!("client {}: {e:#}", id.0);
                    }
                    shared.command(shared::Command::ClientLeft(id));
                    info!("client {} disconnected", id.0);
                });
            }
            result = compositor.exited() => {
                return match result {
                    Ok(()) => Err(anyhow::anyhow!("the compositor connection closed")),
                    Err(e) => Err(e),
                };
            }
            _ = tokio::signal::ctrl_c() => {
                info!("interrupted");
                return Ok(());
            }
        }
    }
}
