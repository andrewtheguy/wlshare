# wlshare

A VNC server for wlroots-based Wayland compositors, built for the
[remotex](https://github.com/andrewtheguy/remotex) gateway. It captures one
output through wlr-screencopy, serves it over RFB 3.8 with ZRLE as its one pixel
encoding, injects input through the virtual keyboard and pointer protocols,
shares the clipboard through wlr-data-control, carries the desktop's sound from
PipeWire over the connection itself — and tells the client what pixel density the
framebuffer is drawn at, which standard RFB cannot, so a `scale 2` output is
shown sharp at 2x and a client's own density becomes the output's.

It is compositor-independent within that protocol surface: any wlroots-based
compositor exposing the required protocols is the same kind of peer.
`wlr-screencopy` version 2 or later is required. Output resizing and rescaling
additionally need `wlr-output-management`; input and clipboard use the virtual
keyboard, virtual pointer, and `wlr-data-control` protocols when the compositor
offers them.

Any VNC client that decodes ZRLE can connect. The density extension is asked for
by the client and stays silent otherwise; remotex asks for it on every plain
`vnc` target. Audio is the QEMU Audio extension `rfbproto` registers, which
QEMU, gtk-vnc and remotex already speak: a client that lists its pseudo-encoding
is offered the default sink's monitor, and one that does not hears nothing.
`audio = false` turns the offer off. One client is on the desktop at a time: a
connection that finishes the handshake takes it from whoever holds it, the way
Windows Remote Desktop does, and the RFB shared flag changes nothing. See
[`docs/architecture.md`](docs/architecture.md) for how it works and what it
deliberately leaves out.

## Running

wlshare runs inside the Wayland session it captures, as the user who owns it:

```sh
WAYLAND_DISPLAY=wayland-1 wlshare --config ~/.config/wlshare/config.toml
```

or as the systemd user unit the package installs, started with the graphical
session after its environment contains `WAYLAND_DISPLAY`:

```sh
systemctl --user enable --now wlshare.service
```

Configuration is one TOML file, `$XDG_CONFIG_HOME/wlshare/config.toml` by
default; every key has a default and [`packaging/config.example.toml`](packaging/config.example.toml)
lists them.

Who may connect is one setting, and there are three answers. Nothing set: anyone
who reaches the port is in, and the session is in the clear, so `listen` stays on
loopback or behind a VPN or an SSH tunnel. `[pam]`: RSA-AES, RealVNC's security
type that TigerVNC and remotex speak — the client sends the username and
password of the account wlshare runs as, PAM checks them under the service
`wlshare` (the package installs `/etc/pam.d/wlshare`), and everything after the
key exchange is encrypted. No other account is accepted, since the desktop
behind the port is that one user's. `[password]`: the same RSA-AES, asking for a
password alone, checked against an Argon2 hash in the configuration file —
for a host where no system account's password should be the way in. Print the
hash with

```sh
wlshare hash-password
```

and paste it into the table; the password itself is never written down.

Classic VncAuth is deliberately not offered in any of the three: it names
nobody, truncates the password to eight characters, proves only knowledge of a
machine's secret, and leaves the session in the clear.
The server's RSA key is generated on first start into `rsa_key_file` and its
fingerprint logged, so it can be compared with the one the client shows.
Under `[pam]`, because the password PAM verifies is the account's, the stack can
pass it on — a `pam_exec ... expose_authtok` line there is how a headless
session gets its keyring unlocked at VNC login.

A custom keyboard layout — for a modifier remap the session's only keyboard has
to carry — goes under `[xkb]`, with `XKB_CONFIG_EXTRA_PATH` in the unit's
environment naming the directory that holds it. Each key must keep its keysym:
the server resolves the client's keysyms through the same keymap it uploads.

## Building

The workspace has two crates: `wlshare-rfb`, the protocol, which builds and tests
anywhere, and `wlshare`, the daemon, which needs libwayland, libxkbcommon and
libpipewire and only runs under a wlroots-based Wayland compositor. A bare
`cargo test` covers the protocol crate; build the daemon with
`cargo build --release -p wlshare` on a Linux host with `libwayland-dev`,
`libxkbcommon-dev`, `libpam0g-dev`, `libpipewire-0.3-dev`, `libspa-0.2-dev`,
`libclang-dev` and `pkg-config`. libclang links nothing: PipeWire's `-sys`
crates generate their bindings with bindgen, which loads it at build time.

Packages for Debian trixie on amd64 and arm64 are built in Docker by
`scripts/build-debs.sh`, into `dist/<arch>/wlshare-trixie-<arch>.deb`. The
**Release wlshare** workflow builds the same and publishes them as the GitHub
release `v<version>`, the version being the workspace's in `Cargo.toml`; bump it
before running the workflow. The package version is the crate's, and the
distribution is in the file name only.
