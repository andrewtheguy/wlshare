# Setting up the encoding agent

Experimental. A wlshare daemon on a machine with no GPU codes its H.264 with
libx264, on its processor. The encoding agent, `wlshare-enc-agent`, runs on
another machine that has a hardware H.264 encoder and lends it to that
daemon: the daemon sends it what changed of each picture, uncompressed, and
is sent the coded frame back. It is for a wired link that is very fast, as
a virtual machine has to the host it runs on.
[`architecture.md`](https://github.com/andrewtheguy/wlshare/blob/main/docs/architecture.md#the-encoding-agent)
says how it works and why; this is how to set it up.

## What it is for

Two machines with a wired link between them that is very fast:

- **A virtual machine and its host**, which is the case it was made for. The
  guest has no GPU and runs wlshare; the host has the GPU and runs the
  agent. The pictures cross a bridge inside the one machine, which no
  network card's speed holds back.
  [A virtual machine and its host](#a-virtual-machine-and-its-host) is that
  setup from start to finish. A container is the same as a guest, and a
  guest or container beside it that has the host's GPU passed in does for
  the host. A QEMU guest can also share memory with the agent and not send
  it the pictures at all:
  [Shared memory with the guest](#shared-memory-with-the-guest).
- **Two wired machines on a LAN faster than gigabit**, 2.5 Gbit/s or more.

How fast depends on what moves. Only what changed of a picture is sent, so a
still desktop sends nothing and a typed letter a few kilobytes; a video
filling the screen is the whole picture every frame. At 1920×1080 that is
3 MB a frame, about 750 Mbit/s at 30 frames a second, and at 3840×2160 four
times that, 3 Gbit/s. A link that cannot carry it has the frame rate fall,
and a frame that takes five seconds ends the session.

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
different versions of the link refuse each other. Installing the package
starts nothing. Upgrading it restarts an agent that is running, which ends
the streams it was coding and the sessions they were for; removing it stops
the agent, and purging it (`apt purge`) removes the key as well.

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
is not used.

`address` is the agent's own address and the port it listens on, what its
`LISTEN` says: with `LISTEN=0.0.0.0:5990`, an address of the agent's machine
and port 5990. It is not the address in `ALLOW`, which is this machine's, the
one the daemon connects from. A host name of the agent's works too, and is
taken at the first address it has. Where that name has both an IPv6 and an
IPv4 address, write the agent's address itself, of the family `LISTEN` is:
the daemon then connects from its own address of that family, which is the
one `ALLOW` has to hold.

Restart the daemon (`systemctl --user restart wlshare`). Its log says which
it found:

```
hardware H.264: the agent at 192.168.1.20:5990, experimental
```

or, with the agent not answering, a warning that says why, and that sessions
needing H.264 fail until it does. The daemon starts either way.

## A virtual machine and its host

The setup the agent was made for, start to finish: wlshare in a guest with
no GPU, and the agent on the host, which has one. The two reach each other
over the bridge the hypervisor gave the guest, so both addresses are that
bridge's: the host's address on it is the agent's `LISTEN`, and the guest's
is its `ALLOW`. Here the host is `192.168.1.20` on the bridge and the guest
`192.168.1.10`.

1. **Give the guest an address that stays**, by a DHCP reservation or a
   static address, since the agent's `ALLOW` names it.

2. **On the host**, check the encoder (`vainfo`, as
   [above](#the-agents-machine)), then install the agent and tell it the two
   addresses:

   ```sh
   sudo apt install ./wlshare-enc-agent-trixie-amd64.deb
   sudoedit /etc/default/wlshare-enc-agent
   ```

   ```sh
   LISTEN=192.168.1.20:5990
   ALLOW=192.168.1.10/32
   ```

   ```sh
   sudo systemctl enable --now wlshare-enc-agent
   sudo cat /var/lib/wlshare-enc-agent/enc_agent.key
   ```

   The last line prints the key the guest needs. Listening on the bridge's
   address, and not on `0.0.0.0`, keeps the port off the host's other
   networks. If the host runs a firewall, open port 5990 on the bridge to
   the guest's address.

3. **In the guest**, as the account wlshare runs as, save the key and name
   the agent in `~/.config/wlshare/config.toml`:

   ```sh
   install -m 600 /dev/stdin ~/.config/wlshare/enc_agent.key   # paste, then Ctrl-D
   ```

   ```toml
   [enc_agent]
   address = "192.168.1.20:5990"
   ```

   ```sh
   systemctl --user restart wlshare
   journalctl --user -u wlshare | grep 'hardware H.264'
   # hardware H.264: the agent at 192.168.1.20:5990, experimental
   ```

Both run the same release of wlshare, so upgrade the two together.

If the bridge comes up after the agent starts, the agent cannot listen on
its address yet and exits. systemd starts it again every five seconds, so
it is listening shortly after the bridge is up, with nothing to do by hand.

A guest that reaches the host through another address than the one in
`ALLOW` is closed unanswered, and the agent's log names the address it saw:
see below.

## Shared memory with the guest

Where the agent is on the host of wlshare's virtual machine, or in a
container on that host, the two can share memory, and the pictures are not
sent at all: wlshare converts what changed of each one straight into
memory the agent maps too, and the network carries the rest, which is
little. It takes a little over a third off the processor time a frame
costs the host, the conversion of each picture included, and a frame comes
back sooner
([measured](https://github.com/andrewtheguy/wlshare/blob/main/docs/video-performance.md#the-encoding-agents-link)).
Everything above still holds: the agent listens, allows the guest's
address, and proves the key, and an agent anywhere else has only the
network.

The memory is QEMU's `ivshmem-plain` device: a PCI device in the guest whose
memory is a file on the host. What it costs:

- **The guest boots without Secure Boot.** Under it, the guest's kernel
  refuses a process the device's memory.
- **The file is the access control.** Whoever can open the file on the
  host, or the device in the guest, reads the desktop's pictures and can
  write into them. The key covers the network alone.

The memory is laid out by the agent, a picture a stream: about 3.1 MB a
1920×1080 picture and 12.4 MB a 3840×2160 one. A stream it has no room for
is sent over the network as before, so 64 MiB is plenty for a session or
two.

1. **On the host, the file.** It has to be readable and writable by the
   agent's account and no other, and to stay the same file while the agent
   runs. With plain QEMU it is the device's backing file:

   ```sh
   -object memory-backend-file,id=wlshare,share=on,mem-path=/dev/shm/wlshare,size=64M
   -device ivshmem-plain,memdev=wlshare
   ```

   **On Proxmox**, `qm set <vmid> --ivshmem size=64,name=wlshare` adds the
   device, backed by `/dev/shm/pve-shm-wlshare`. Proxmox removes that file
   each time the guest stops and QEMU makes it again, root's, as it starts,
   and Proxmox refuses a container a bind mount from `/dev`. So make the
   file somewhere else, at boot, and have a hookscript point Proxmox's name
   at it before each start:

   ```sh
   # /etc/tmpfiles.d/ivshmem.conf: the agent's account owns it. For an
   # unprivileged container that is its uid on the host, 100000 more.
   d /run/ivshmem-wlshare 0750 101000 101000 -
   f /run/ivshmem-wlshare/wlshare 0600 101000 101000 -
   ```

   ```sh
   #!/bin/sh
   # /var/lib/vz/snippets/ivshmem-wlshare.sh
   set -eu
   if [ "$2" = pre-start ]; then
       ln -sfn /run/ivshmem-wlshare/wlshare /dev/shm/pve-shm-wlshare
   fi
   ```

   ```sh
   systemd-tmpfiles --create /etc/tmpfiles.d/ivshmem.conf
   chmod 755 /var/lib/vz/snippets/ivshmem-wlshare.sh
   qm set <vmid> --ivshmem size=64,name=wlshare --hookscript local:snippets/ivshmem-wlshare.sh
   # the agent in a container: the file's directory, bound into it
   pct set <ctid> --mp0 /run/ivshmem-wlshare,mp=/mnt/ivshmem
   ```

   QEMU opens the file through the link and sizes it, and Proxmox's removal
   at a stop takes the link and leaves the file. Stop the guest and start it
   again (a reboot from inside it does not add a device), and restart the
   container for its mount.

2. **In the guest**, turn Secure Boot off in its firmware setup (on
   Proxmox's OVMF: Esc as it boots, then Device Manager, Secure Boot
   Configuration, untick Attempt Secure Boot, F10);
   `cat /sys/kernel/security/lockdown` says `[none]` once it is. Find the
   device, and give its memory to the account wlshare runs as, here `me`,
   with a udev rule:

   ```sh
   lspci -D -d 1af4:1110
   # 0000:00:14.0 RAM memory: Red Hat, Inc. QEMU Inter-VM shared memory device (rev 01)
   echo 'ACTION!="remove", SUBSYSTEM=="pci", ATTR{vendor}=="0x1af4", ATTR{device}=="0x1110", RUN+="/usr/bin/chown me /sys%p/resource2 /sys%p/resource2_wc"' |
       sudo tee /etc/udev/rules.d/70-ivshmem.rules
   sudo udevadm trigger --action=change --subsystem-match=pci --attr-match=vendor=0x1af4
   ```

   Then name the device's memory in the `[enc_agent]` table:

   ```toml
   [enc_agent]
   address = "192.168.1.20:5990"
   shared_memory = "/sys/bus/pci/devices/0000:00:14.0/resource2_wc"
   ```

3. **On the agent's machine**, name the file in
   `/etc/default/wlshare-enc-agent`, and let the unit's account open it: the
   unit's own account is made up as it starts and cannot own the file, and
   it sees the rest of the system read-only. A drop-in has it run as the
   file's owner, here `me`, with the file's directory writable:

   ```sh
   # /etc/default/wlshare-enc-agent
   SHARED_MEMORY=/mnt/ivshmem/wlshare
   ```

   ```sh
   sudo systemctl edit wlshare-enc-agent
   ```

   ```ini
   [Service]
   User=me
   ReadWritePaths=/mnt/ivshmem
   ```

   Run by hand, the agent takes `--shared-memory /mnt/ivshmem/wlshare`.

Restart the agent and wlshare. The agent's log names the memory, and each
stream where its picture went:

```
the daemon's pictures in 64 MiB of memory shared with it, /mnt/ivshmem/wlshare
10.22.38.224:58102: a 1920x1080 stream, its picture in shared memory at 0
```

and wlshare's, as it starts:

```
the encoding agent's pictures in 64 MiB of memory shared with it, /sys/bus/pci/devices/0000:00:14.0/resource2_wc
```

To change the size later: the file keeps the length QEMU gave it until the
host reboots, which makes it again, empty, for QEMU to size, and both ends
map it at the length they found as they started. So stop the guest and the
agent, give the file the new length, then start the guest with the new size
and start the agent:

```sh
qm shutdown <vmid>                      # and stop the agent: systemctl stop wlshare-enc-agent
truncate -s 128M /run/ivshmem-wlshare/wlshare
qm set <vmid> --ivshmem size=128,name=wlshare
qm start <vmid>                         # then start the agent
```

`truncate` keeps the file's owner and mode. The agent is stopped first
because the file is cut or grown under its mapping; and a file left at
another length than the device's is one wlshare and the agent do not share
all of: the daemon's log says the two are not the same memory.

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
| agent or daemon: `the shared memory`, then `Permission denied` | The account cannot open the file, or the device's memory in the guest: see [Shared memory with the guest](#shared-memory-with-the-guest). `Read-only file system` is the agent's unit without `ReadWritePaths`. |
| daemon: `the daemon shares memory with this agent and the agent none with it` | wlshare has `shared_memory` and the agent no `SHARED_MEMORY`. |
| daemon: `the two are not the same memory`, or `the two ends map other memory` | The guest's device and the agent's file are not the same memory: another file, or a guest started before the file was made. |
| daemon: `Operation not permitted` mapping the device | The guest booted with Secure Boot on. |
| agent: `over the connection: no room for its picture in shared memory` | The memory is full of other streams' pictures: this stream's cross the network, which works, more slowly. A larger `size` makes room: see [Shared memory with the guest](#shared-memory-with-the-guest) for changing it. |

The agent's unit does not start again by itself after refusing its command
line, which is what an unset or mistyped `LISTEN` or `ALLOW` is:
`journalctl -u wlshare-enc-agent` has the reason, and `systemctl restart
wlshare-enc-agent` tries again once the file is fixed.
