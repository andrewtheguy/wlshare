# Setting up the encoding agent

Experimental. A wlshare daemon on a machine with no GPU codes its H.264 with
libx264, on its processor. The encoding agent, `wlshare-enc-agent`, runs on
another machine that has a hardware H.264 encoder and lends it to that
daemon: the daemon sends it what changed of each picture, uncompressed, and
is sent the coded frame back.
[`architecture.md`](architecture.md#the-encoding-agent) says how it works and
why; this is how to set it up.

## What it is for

Two machines with a fast link of their own between them:

- a virtual machine or container with no GPU, and the host it runs on, or a
  sibling of it there, that has the host's GPU;
- or two wired machines on a LAN faster than gigabit. A video filling a
  1920×1080 screen is about 750 Mbit/s of pictures at 30 frames a second.

It is not for Wi-Fi, a link between sites, or the internet. The pictures
cross the link uncompressed and in the clear: the key the two machines share
proves each to the other and encrypts nothing.

Two things to know before choosing it:

- **No fallback.** A daemon set to an agent has no other H.264 encoder. With
  the agent away, a session that needs H.264 ends, and the next one tries
  the agent again.
- **One daemon an agent.** An agent codes for one daemon at a time. Give each
  daemon an agent of its own.

## The agent's machine

It needs a GPU with a VA-API H.264 encoder and that GPU's driver: Intel's
(`intel-media-va-driver`, or `intel-media-va-driver-non-free` for some
chips) or Mesa's for AMD (`mesa-va-drivers`). `vainfo` says what the machine
has:

```sh
sudo apt install vainfo
vainfo --display drm --device /dev/dri/renderD128 | grep H264
```

A line with `VAEntrypointEncSlice` or `VAEntrypointEncSliceLP` beside an
H.264 profile is an encoder the agent can use. In a container, the host's
render node (`/dev/dri/renderD128`) has to be passed in, and the account the
agent runs as needs to open it: the `render` group, which the package's unit
joins.

### Install

Each [release](https://github.com/andrewtheguy/wlshare/releases) has the
agent's package beside the daemon's, for Debian trixie and Ubuntu 26.04
(resolute), on amd64 and arm64: `wlshare-enc-agent-<suite>-<arch>.deb`. It
depends on libva alone and brings nothing of the daemon's.

```sh
sudo apt install ./wlshare-enc-agent-trixie-amd64.deb
```

Both machines run the same release of wlshare: an agent and a daemon of
different versions of the link refuse each other.

### Configure and start

The package's unit reads two values from `/etc/default/wlshare-enc-agent`:

```sh
# An address of this machine, and a port of your choosing.
LISTEN=192.168.1.20:5990
# Where the daemon connects from.
ALLOW=192.168.1.10/32
```

`ALLOW` is required, as IPv4 or IPv6 CIDR blocks with commas between them.
One address is a `/32`, or a `/128` for IPv6. The agent closes a connection
from anywhere else without answering. Name the daemon's own address, not its
network: every address allowed may try the key, though one with the wrong key
gets no answer either.

```sh
sudo systemctl enable --now wlshare-enc-agent
journalctl -u wlshare-enc-agent
```

A working agent logs the encoder it found and where it listens:

```
generated a key into /var/lib/wlshare-enc-agent/enc_agent.key: the daemon's [enc_agent] table needs a copy of the file
hardware H.264: /dev/dri/renderD128 through VA-API, its shader encoder
encoding agent on 192.168.1.20:5990, for a daemon at 192.168.1.10/32: experimental, the pictures in the clear
```

If a firewall runs on the machine, open the port to the daemon's address.

### Without the package

The agent is one binary that links libva and nothing else of wlshare's. On a
machine with Rust and `libva-dev`:

```sh
cargo build --release -p wlshare-enc-agent
target/release/wlshare-enc-agent --listen 192.168.1.20:5990 --allow 192.168.1.10/32
```

Run this way it keeps its key in `$XDG_CONFIG_HOME/wlshare/enc_agent.key`,
or the file `--key-file` names, and needs `libva2`, `libva-drm2` and the
driver on the machine it runs on. `--allow` is repeated, or given commas, for
more than one block.

## The daemon's machine

Copy the agent's key to it. The agent generated the key on its first start;
under the package's unit it is `/var/lib/wlshare-enc-agent/enc_agent.key`,
readable by root. The daemon looks for `enc_agent.key` beside its
configuration file:

```sh
# on the agent's machine
sudo cat /var/lib/wlshare-enc-agent/enc_agent.key
# on the daemon's, as the account wlshare runs as
install -m 600 /dev/stdin ~/.config/wlshare/enc_agent.key   # paste, then Ctrl-D
```

Then name the agent in the daemon's configuration, `config.toml`:

```toml
[enc_agent]
address = "192.168.1.20:5990"
# key_file = "/home/me/.config/wlshare/enc_agent.key"   # default: beside this file
```

`hardware_h264` has to be on, which it is by default: with it off the table
is not used. The address is the agent's `LISTEN`. A host name works too, and
is taken at the first address it has, so one with both an IPv6 and an IPv4
address is better written as the address `ALLOW` was written for.

Restart the daemon (`systemctl --user restart wlshare`). Its log says which
it found:

```
hardware H.264: the agent at 192.168.1.20:5990, experimental
```

or, with the agent not answering, a warning that says why, and that sessions
needing H.264 fail until it does. The daemon starts either way.

## When it does not work

What the logs say, the agent's (`journalctl -u wlshare-enc-agent`) and the
daemon's:

| The log says | What it means |
| --- | --- |
| agent: `this machine has no hardware H.264 encoder to lend` | No VA-API H.264 encoder was found, or the account cannot open the render node. Check `vainfo` and the driver; `RUST_LOG=debug` in the agent's environment says what was tried. |
| agent: `closed unanswered, being from nowhere this agent allows` | The connection came from an address `ALLOW` does not hold. The line starts with the address the agent saw, which is the one to allow. |
| agent: `its key is not this agent's`, or daemon: `its key is not this daemon's` | The two key files differ. Copy the agent's again. |
| agent: `closed unanswered, the agent being the daemon at …'s` | Another daemon has a stream open. An agent is one daemon's at a time. |
| daemon: `connecting: Connection refused`, or a timeout | Nothing listens at `address`: the agent is not running, `LISTEN` is another address or port, or a firewall is in the way. |
| daemon: `not done within 5s` | The agent stopped answering mid-stream, or the link is too slow for the pictures. The session ends. |

The agent's unit does not start again by itself after refusing its command
line, which is what an unset or mistyped `LISTEN` or `ALLOW` is:
`journalctl -u wlshare-enc-agent` has the reason, and `systemctl restart
wlshare-enc-agent` tries again once the file is fixed.
