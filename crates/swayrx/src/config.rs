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
    /// A file holding the VncAuth password; absent means no authentication
    /// unless `[pam]` is set. Only the first eight bytes count, as VncAuth has
    /// it, and the session stays in the clear.
    pub password_file: Option<PathBuf>,
    /// RSA-AES with the system login: the client names the account swayrx
    /// runs as and gives its password, PAM checks the two, and the session is
    /// encrypted. Exclusive with `password_file`.
    pub pam: Option<Pam>,
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

/// The `[pam]` table: RSA-AES security with the credentials checked by PAM.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Pam {
    /// The PAM service name, which is the stack under `/etc/pam.d`.
    #[serde(default = "default_pam_service")]
    pub service: String,
    /// The server's RSA key as PKCS#8 PEM, generated on first start when the
    /// file is missing. Absent means `rsa_key.pem` beside the configuration
    /// file.
    pub rsa_key_file: Option<PathBuf>,
}

fn default_pam_service() -> String {
    "swayrx".to_owned()
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
        anyhow::ensure!(
            config.password_file.is_none() || config.pam.is_none(),
            "{} sets both password_file and [pam]; a server offers VncAuth or RSA-AES, not both",
            path.display()
        );
        Ok(config)
    }

    /// Where the RSA-AES key lives when `[pam]` is set: the configured path, or
    /// `rsa_key.pem` beside the configuration file at `config_path`.
    pub fn rsa_key_file(&self, config_path: &Path) -> Option<PathBuf> {
        let pam = self.pam.as_ref()?;
        Some(pam.rsa_key_file.clone().unwrap_or_else(|| config_path.with_file_name("rsa_key.pem")))
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
    fn pam_defaults_its_service_and_key_beside_the_config() {
        let c: Config = toml::from_str("[pam]\n").unwrap();
        let pam = c.pam.as_ref().unwrap();
        assert_eq!(pam.service, "swayrx");
        assert_eq!(c.rsa_key_file(Path::new("/etc/x/config.toml")), Some(PathBuf::from("/etc/x/rsa_key.pem")));
        let c: Config = toml::from_str("[pam]\nservice = \"vnc\"\nrsa_key_file = \"/k.pem\"").unwrap();
        assert_eq!(c.pam.as_ref().unwrap().service, "vnc");
        assert_eq!(c.rsa_key_file(Path::new("/etc/x/config.toml")), Some(PathBuf::from("/k.pem")));
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.rsa_key_file(Path::new("/etc/x/config.toml")), None);
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert!(toml::from_str::<Config>("port = 5900").is_err());
    }
}
