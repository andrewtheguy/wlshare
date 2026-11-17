//! `wlshare-enc-agent`: lend this machine's hardware H.264 encoder to a
//! wlshare daemon on a machine that has none. Experimental.
//!
//! The agent looks for its encoder once, at startup, as a daemon does
//! ([`vaapi::find`]), and with none does not start: an agent is a GPU, not
//! another processor to run libx264 on. Then it listens, for connections
//! from where it was told its daemon is and nowhere else (`--allow`, which
//! has no default), and every one that proves the pre-shared key is one
//! stream ([`serve`]), for one daemon at a time, whose streams share a VA
//! display. The key is a file, generated on a first start, which the
//! daemon's `[enc_agent]` table needs a copy of.
//!
//! It is for two machines with a fast link of their own between them: a
//! virtual machine or container with no GPU and the host it runs on, or a
//! sibling of it there, that has the host's encoder; or a machine wired to a
//! LAN faster than gigabit and another on it.

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use log::info;
use wlshare_core::agent::{allow::Net, link, serve};
use wlshare_core::h264::{Backend, vaapi};

#[derive(Parser, Debug)]
#[command(
    name = "wlshare-enc-agent",
    version,
    about = "Lends this machine's hardware H.264 encoder to a wlshare daemon on another (experimental)",
    long_about = "Lends this machine's hardware H.264 encoder, through VA-API, to a wlshare daemon on a machine that has none. The daemon names this one in its [enc_agent] table and holds a copy of the key file. For two machines with a fast link of their own: a virtual machine or container and its host, or a sibling there, that has the host's GPU, or two wired machines on a LAN faster than gigabit. --allow says where the daemon is, and a connection from anywhere else is closed unanswered, as is one with another key: name the daemon's own address, since an agent that allows a whole network lets every machine on it try the key. One daemon at a time: while one has a stream open, nobody is let in from any other address. Experimental: the pictures cross the network in the clear, so this is for a trusted LAN, or inside a tunnel."
)]
struct Args {
    /// The address and port to listen on for daemons.
    #[arg(long, value_name = "ADDRESS:PORT", value_parser = listen)]
    listen: SocketAddr,
    /// Where the daemon connects from, as an IPv4 or IPv6 CIDR block: one
    /// address is a /32 or a /128. At least one, and more by repeating the
    /// option or with commas between them.
    #[arg(long, value_name = "CIDR", required = true, value_delimiter = ',')]
    allow: Vec<Net>,
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
        "encoding agent on {}, for a daemon at {}: experimental, the pictures in the clear",
        args.listen,
        args.allow
            .iter()
            .map(Net::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    // A display for the daemon the agent is coding for, opened as it takes
    // its first stream and closed with its last: its streams share it, as
    // a session's share theirs, and none waits on the driver being loaded
    // again.
    serve::serve(
        listener,
        key.into(),
        args.allow,
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

/// Where the agent listens: an address and a port a daemon can be told, so
/// not port 0, which is whichever port is free.
fn listen(written: &str) -> Result<SocketAddr, String> {
    let address: SocketAddr = written
        .parse()
        .map_err(|_| "it is no address:port, an IPv6 address in brackets".to_owned())?;
    if address.port() == 0 {
        return Err("port 0 is no port a daemon can be told: name the one to listen on".to_owned());
    }
    Ok(address)
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
        let key: String = link::nonce()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        new_file(path, |file| writeln!(file, "{key}"))
            .with_context(|| format!("writing {}", path.display()))?;
        info!(
            "generated a key into {}: the daemon's [enc_agent] table needs a copy of the file",
            path.display()
        );
    }
    link::read_key(path)
}

/// A file made at `path`, this account's alone to read, and filled by
/// `fill`. A file already there is an error and is left as it is; and one
/// that cannot be filled is removed again, being this call's own, so that no
/// later start reads half a key out of it.
fn new_file(
    path: &Path,
    fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    fill(&mut file).inspect_err(|_| {
        let _ = std::fs::remove_file(path);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key file that could not be written whole is not left for the next
    /// start to read; one written is this account's alone; and one already
    /// there is neither written over nor removed.
    #[test]
    fn a_key_file_is_written_whole_or_not_at_all() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("wlshare-enc-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("enc_agent.key");

        let failed = new_file(&path, |file| {
            file.write_all(b"half a k")?;
            Err(std::io::ErrorKind::StorageFull.into())
        });
        assert_eq!(failed.unwrap_err().kind(), std::io::ErrorKind::StorageFull);
        assert!(!path.exists(), "half a key is no key file");

        new_file(&path, |file| writeln!(file, "{}", "5a".repeat(32))).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let first = key(&path).unwrap();
        assert_eq!(first, "5a".repeat(32).into_bytes());

        let again = new_file(&path, |_| Err(std::io::ErrorKind::StorageFull.into()));
        assert_eq!(again.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(key(&path).unwrap(), first, "the key there is kept");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An agent listens where a daemon can be told it does: at an address
    /// and a port, and not at port 0.
    #[test]
    fn an_agent_listens_at_a_port_it_names() {
        for good in ["192.168.1.20:5990", "0.0.0.0:1", "[::]:5990"] {
            assert_eq!(listen(good), Ok(good.parse().unwrap()));
        }
        for bad in [
            "192.168.1.20:0",
            "[::]:0",
            "192.168.1.20",
            "gpu.lan:5990",
            "",
        ] {
            assert!(listen(bad).is_err(), "{bad:?}");
        }
    }
}
