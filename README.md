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
lists them. The session is not encrypted, so `listen` is loopback unless a VPN or
an SSH tunnel is in front. Set `password_file` for VncAuth; without it, anyone
who can reach the port is in.

A custom keyboard layout — for a modifier remap the session's only keyboard has
to carry — goes under `[xkb]`, with `XKB_CONFIG_EXTRA_PATH` in the unit's
environment naming the directory that holds it. Each key must keep its keysym:
the server resolves the client's keysyms through the same keymap it uploads.

## Building

The workspace has two crates: `swayrx-rfb`, the protocol, which builds and tests
anywhere, and `swayrx`, the daemon, which needs libwayland and libxkbcommon and
only runs under a Wayland compositor. A bare `cargo test` covers the protocol
crate; build the daemon with `cargo build --release -p swayrx` on a Linux host
with `libwayland-dev`, `libxkbcommon-dev` and `pkg-config`.

Packages for Debian trixie on amd64 and arm64 are built in Docker by
`scripts/build-debs.sh` and released by the **Build and release packages**
workflow, tagged `trixie-<YYYYMMDD>-<N>`.
