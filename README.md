# swayrx

A VNC server for a sway desktop, built for the
[remotex](https://github.com/andrewtheguy/remotex) gateway. It captures one
output through wlr-screencopy, serves it over RFB 3.8 with ZRLE as its one pixel
encoding, injects input through the virtual keyboard and pointer protocols,
shares the clipboard through wlr-data-control — and tells the client what pixel
density the framebuffer is drawn at, which standard RFB cannot, so a `scale 2`
output is shown sharp at 2x and a client's own density becomes the output's.

Any VNC client that decodes ZRLE can connect. The density extension is asked for
by the client and stays silent otherwise; remotex asks for it under
`subtype = "swayrx"`. See [`docs/architecture.md`](docs/architecture.md) for
how it works and what it deliberately leaves out.

## Running

swayrx runs inside the sway session it captures, as the user who owns it:

```sh
WAYLAND_DISPLAY=wayland-1 swayrx --config ~/.config/swayrx/config.toml
```

or as the systemd user unit the package installs, started with the session:

```sh
systemctl --user enable --now swayrx.service
```

Configuration is one TOML file, `$XDG_CONFIG_HOME/swayrx/config.toml` by
default; every key has a default and [`packaging/config.example.toml`](packaging/config.example.toml)
lists them.

Who may connect is two independent settings, offered together the way macOS
Screen Sharing offers an account login beside its VNC password. Nothing set:
anyone who reaches the port is in. `password_file`: VncAuth with the server's
own password, for a client that knows the password and nothing about the
account; the login is checked and the session is in the clear, so `listen`
stays on loopback or behind a VPN or an SSH tunnel. `[pam]`: RSA-AES, RealVNC's
security type that TigerVNC and remotex speak — the client sends the username
and password of the account swayrx runs as, PAM checks them under the service
`swayrx` (the package installs `/etc/pam.d/swayrx`), and everything after the key
exchange is encrypted. No other account is accepted, since the desktop behind
the port is that one user's. With both set the server lists both and the client
picks by what it holds; remotex's `subtype = "swayrx"` target always brings the
account, and its plain `vnc` target the password.
The server's RSA key is generated on first start into `rsa_key_file` and its
fingerprint logged, so it can be compared with the one the client shows.
Because the password PAM verifies is the account's, the stack can pass it on —
a `pam_exec ... expose_authtok` line there is how a headless session gets its
keyring unlocked at VNC login.

A custom keyboard layout — for a modifier remap the session's only keyboard has
to carry — goes under `[xkb]`, with `XKB_CONFIG_EXTRA_PATH` in the unit's
environment naming the directory that holds it. Each key must keep its keysym:
the server resolves the client's keysyms through the same keymap it uploads.

## Building

The workspace has two crates: `swayrx-rfb`, the protocol, which builds and tests
anywhere, and `swayrx`, the daemon, which needs libwayland and libxkbcommon and
only runs under a Wayland compositor. A bare `cargo test` covers the protocol
crate; build the daemon with `cargo build --release -p swayrx` on a Linux host
with `libwayland-dev`, `libxkbcommon-dev`, `libpam0g-dev` and `pkg-config`.

Packages for Debian trixie on amd64 and arm64 are built in Docker by
`scripts/build-debs.sh`, into `dist/<arch>/swayrx-trixie-<arch>.deb`. The
**Release swayrx** workflow builds the same and publishes them as the GitHub
release `v<version>`, the version being the workspace's in `Cargo.toml`; bump it
before running the workflow. The package version is the crate's, and the
distribution is in the file name only.
