//! `wlshare-enc-agent`: lend this machine's hardware H.264 encoder to a
//! wlshare daemon on a machine that has none. Experimental.
//!
//! The agent looks for its encoder once, at startup, as a daemon does
//! ([`vaapi::find`]), and with none does not start: an agent is a GPU, not
//! another processor to run libx264 on. Then it listens, and every
//! connection that proves the pre-shared key is one stream ([`serve`]), for
//! one daemon at a time, whose streams share a VA display. The key is a file, generated
//! on a first start, which the daemon's `[enc_agent]` table needs a copy of.

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use log::info;
use wlshare_core::agent::{link, serve};
use wlshare_core::h264::{Backend, vaapi};

#[derive(Parser, Debug)]
#[command(
    name = "wlshare-enc-agent",
    version,
    about = "Lends this machine's hardware H.264 encoder to a wlshare daemon on another (experimental)",
    long_about = "Lends this machine's hardware H.264 encoder, through VA-API, to a wlshare daemon on a machine that has none. The daemon names this one in its [enc_agent] table and holds a copy of the key file. One daemon at a time: while one has a stream open, nobody is let in from any other address. Experimental: the pictures cross the network in the clear, so this is for a trusted LAN, or inside a tunnel."
)]
struct Args {
    /// The address to listen on for daemons.
    #[arg(long)]
    listen: SocketAddr,
    /// The pre-shared key, generated on first start when the file is missing
    /// (default: $XDG_CONFIG_HOME/wlshare/enc_agent.key).
    #[arg(long, value_name = "FILE")]
    key_file: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let version = format!("{} (link {})", env!("CARGO_PKG_VERSION"), link::VERSION);
    let args = <Args as clap::FromArgMatches>::from_arg_matches(
        &<Args as clap::CommandFactory>::command()
            .version(&*version.leak())
            .get_matches(),
    )
    .unwrap_or_else(|e| e.exit());
    let key = key(&args.key_file.unwrap_or_else(default_key_file))?;

    let (device, low_power) = vaapi::find(vaapi::moves_its_quantizer).context(
        "this machine has no hardware H.264 encoder to lend (RUST_LOG=debug says what was tried)",
    )?;
    let listener =
        TcpListener::bind(args.listen).with_context(|| format!("listening on {}", args.listen))?;
    info!(
        "encoding agent on {}: experimental, the pictures in the clear",
        args.listen
    );
    // A display for the daemon the agent is coding for, opened as it takes
    // its first stream and closed with its last: its streams share it, as
    // a session's share theirs, and none waits on the driver being loaded
    // again.
    serve::serve(
        listener,
        key.into(),
        Arc::new(move || {
            let display = Arc::new(vaapi::Display::open(&device)?);
            Ok(Arc::new(move |coded| {
                let stream = vaapi::Stream::open(display.clone(), low_power, coded)?;
                Ok(Box::new(stream) as Box<dyn Backend>)
            }) as Arc<serve::Streams>)
        }),
    );
    Ok(())
}

/// The key file where none is named: beside where the daemon's configuration
/// is, were this machine to have one.
fn default_key_file() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("wlshare").join("enc_agent.key")
}

/// The pre-shared key: the file's, or on a first start one generated and
/// written there, readable by this account alone, for the operator to copy
/// to the daemon.
fn key(path: &Path) -> anyhow::Result<Vec<u8>> {
    if !path.exists() {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let key: String = link::nonce()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .and_then(|mut file| writeln!(file, "{key}"))
            .with_context(|| format!("writing {}", path.display()))?;
        info!(
            "generated a key into {}: the daemon's [enc_agent] table needs a copy of the file",
            path.display()
        );
    }
    link::read_key(path)
}
