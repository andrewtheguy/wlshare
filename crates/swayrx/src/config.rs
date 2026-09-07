//! The configuration file: what the server listens on, who may connect, which
//! output it shares and how the virtual keyboard is laid out.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Where to accept clients. The session is not encrypted, so this is a
    /// loopback or a VPN address unless something in front of it encrypts.
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    /// A file holding the VncAuth password; absent means no authentication.
    /// Only the first eight bytes count, as VncAuth has it.
    pub password_file: Option<PathBuf>,
    /// The output to capture, by name (`swaymsg -t get_outputs`); absent means
    /// the first one the compositor lists.
    pub output: Option<String>,
    /// Whether clients may resize the output and set its scale. Only a headless
    /// output is ever reconfigured.
    #[serde(default = "default_true")]
    pub resize: bool,
    /// The most frames captured per second.
    #[serde(default = "default_max_fps")]
    pub max_fps: u32,
    /// The desktop name in ServerInit.
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default)]
    pub xkb: Xkb,
}

/// The keymap the virtual keyboard uploads and the server resolves keysyms
/// through — RMLVO names, as `xkbcommon` takes them. Empty means the library's
/// default; `XKB_CONFIG_EXTRA_PATH` in the environment adds a search directory
/// for a custom layout.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Xkb {
    #[serde(default)]
    pub rules: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub layout: String,
    #[serde(default)]
    pub variant: String,
    #[serde(default)]
    pub options: String,
}

fn default_listen() -> SocketAddr {
    "127.0.0.1:5900".parse().unwrap()
}

fn default_true() -> bool {
    true
}

fn default_max_fps() -> u32 {
    60
}

fn default_name() -> String {
    "sway".to_owned()
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Self = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        anyhow::ensure!(config.max_fps > 0, "max_fps must be at least 1");
        Ok(config)
    }

    /// The VncAuth password, trimmed of a trailing newline, or `None` for a
    /// server without authentication.
    pub fn password(&self) -> anyhow::Result<Option<String>> {
        let Some(path) = &self.password_file else { return Ok(None) };
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let password = text.trim_end_matches(['\r', '\n']).to_owned();
        anyhow::ensure!(!password.is_empty(), "{} is empty", path.display());
        Ok(Some(password))
    }
}

/// The default configuration path: `$XDG_CONFIG_HOME/swayrx/config.toml`.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("swayrx").join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_an_empty_file() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.listen, default_listen());
        assert!(c.resize);
        assert_eq!(c.max_fps, 60);
        assert_eq!(c.name, "sway");
        assert!(c.xkb.layout.is_empty());
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert!(toml::from_str::<Config>("port = 5900").is_err());
    }
}
