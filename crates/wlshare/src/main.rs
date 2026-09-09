//! wlshare: a VNC server for wlroots-based Wayland compositors that tells the
//! remotex gateway what pixel density its framebuffer is drawn at.
//!
//! Two halves. One thread owns the Wayland connection and everything on it —
//! capture, outputs, input, clipboard — and runs a calloop that also polls a
//! command channel ([`compositor`]). The tokio runtime accepts clients and runs
//! one task per connection ([`session`]). They share the framebuffer and a few
//! channels ([`shared`]), and every protocol byte comes from the `wlshare-rfb`
//! crate. A client that enables audio gets a PipeWire capture thread of its
//! own for as long as it listens ([`audio`]).

mod audio;
mod capture;
mod clipboard;
mod compositor;
mod config;
mod framebuffer;
mod input;
mod outputs;
mod pam;
mod session;
mod shared;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use log::{error, info};
use wlshare_rfb::rsa_aes::ServerKey;

use crate::session::{RsaAes, Security};

#[derive(Parser, Debug)]
#[command(name = "wlshare", version, about = "A VNC server for wlroots-based Wayland compositors, with pixel density on the wire")]
struct Args {
    /// The configuration file (default: $XDG_CONFIG_HOME/wlshare/config.toml).
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
    let security = security(&config, &path)?;

    // The compositor thread comes up first and hands back what it learned about
    // the output, so ServerInit can name a size before the first frame.
    let (compositor, shared) = compositor::start(&config)?;

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let result = runtime.block_on(serve(config, security, shared.clone(), compositor));
    if let Err(e) = &result {
        error!("{e:#}");
    }
    result
}

/// What the configuration says about who may connect, with the RSA-AES key
/// loaded — or generated and written, on a first start — when PAM is on.
fn security(config: &config::Config, config_path: &std::path::Path) -> anyhow::Result<Security> {
    let rsa_aes = match &config.pam {
        Some(pam) => Some(rsa_aes(pam, config.rsa_key_file(config_path).expect("[pam] is set"))?),
        None => None,
    };
    match &rsa_aes {
        None => info!("no [pam] table: accepting clients without authentication, in the clear"),
        Some(_) => info!("RSA-AES: a client needs the account's login"),
    }
    Ok(Security { rsa_aes })
}

/// The RSA-AES half of the security: the key from `key_path`, generated there
/// on first start, and the account whose login PAM will be asked about.
fn rsa_aes(pam: &config::Pam, key_path: std::path::PathBuf) -> anyhow::Result<RsaAes> {
    let key = match std::fs::read_to_string(&key_path) {
        Ok(pem) => ServerKey::from_pem(&pem).with_context(|| format!("{} is not a PKCS#8 PEM RSA key", key_path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!("generating a {}-bit RSA key into {}", wlshare_rfb::rsa_aes::SERVER_KEY_BITS, key_path.display());
            let key = ServerKey::generate().context("generating the RSA key")?;
            if let Some(dir) = key_path.parent() {
                std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            }
            write_private(&key_path, key.to_pem().context("encoding the RSA key")?.as_bytes())?;
            key
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", key_path.display())),
    };
    let account = pam::process_user()?;
    info!(
        "RSA-AES with PAM service {:?}: the login is {account:?}'s; server key {} bits, fingerprint {}",
        pam.service,
        key.bits(),
        key.fingerprint()
    );
    Ok(RsaAes { key: Arc::new(key), service: pam.service.clone(), account })
}

/// Write a file only its owner can read, created fresh.
fn write_private(path: &std::path::Path, contents: &[u8]) -> anyhow::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(contents).with_context(|| format!("writing {}", path.display()))
}

async fn serve(
    config: config::Config,
    security: Security,
    shared: Arc<shared::Shared>,
    mut compositor: compositor::Handle,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.listen).await.with_context(|| format!("listening on {}", config.listen))?;
    info!("listening on {}", config.listen);
    let session_config = Arc::new(session::SessionConfig { security, name: config.name.clone(), resize: config.resize, audio: config.audio });
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
