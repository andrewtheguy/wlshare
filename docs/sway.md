# Sway

wlshare serves any wlroots-based compositor through wlroots' protocols, and
does the most with sway. The reason is how sway is built. Every command it
takes can also be sent while it runs, over its IPC socket, from `swaymsg` or a
program, in the same words as its configuration: making a headless output,
unplugging one, turning a monitor off, moving a workspace. So a feature the
mainstream Wayland protocols give no easy way to costs wlshare a small IPC client (`src/sway.rs`) and
nothing compositor-specific beyond it, and rearranging a session around a
client is a script's job rather than the daemon's. Its configuration is built
from the same parts: `output *` rules, `exec` lines and included drop-ins,
from which a session for wlshare is put together without touching the rest.

Every other wlroots compositor is served through the protocols the
[README](../README.md) lists — the picture, the cursor, input, the clipboard,
resizing a headless output — with the sound, the camera and the microphone
through PipeWire, and is the same kind of peer for all of them. What it does
not get is what sway's IPC gives.

How to set a desktop up — installing, a headless sway under the user manager,
who may connect, the keyboard — is the user guide's,
[Setting up wlshare](https://zdesk.dev/guides/wlshare). This page is what that
setup uses sway for, and the examples are drawn from the hosts wlshare runs
on.

## What sway's IPC gives

| | On sway | Elsewhere |
|-|---------|-----------|
| Virtual displays | A client's request for one or two is met by making and unplugging `HEADLESS-n` outputs through sway's IPC (`create_output`, `output NAME unplug`) | The outputs are the compositor's; every OutputList says the daemon makes none |
| A desktop at the console | The watcher's commands are `swaymsg` lines: disabling a monitor moves its workspaces onto the headless output the client is shown | `state_socket` and `wlshare watch` are the same; what the commands do is the compositor's own business |

The daemon finds sway through `SWAYSOCK` in its environment and checks at
startup that what answers there is sway; there is nothing to configure, and
without it the daemon makes no outputs and says so
([`architecture.md`](architecture.md#virtual-displays)). The IPC is `src/sway.rs`,
and it is the only place in the daemon that speaks to a compositor through
anything but Wayland.

## A headless session

The user guide's
[headless desktop](https://zdesk.dev/guides/wlshare#headless) runs sway as a
systemd user service on wlroots' headless backend, started at boot by
lingering, with `wlshare.service` part of `graphical-session.target`. These
parts of it are there for wlshare.

The environment sway hands the user manager names `SWAYSOCK`, or the daemon
started from it finds no sway to make outputs with:

```
# The end of sway's config.
exec systemctl --user import-environment \
    WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP XDG_SESSION_TYPE
exec systemctl --user start sway-session.target
```

sway's own unit stops `graphical-session.target` on its way out, not only
`sway-session.target`: the package's `wlshare.service` is part of the former,
and left behind it would restart against a compositor that is gone until its
unit's rate limit stopped it.

```
# ~/.config/systemd/user/sway-headless.service, [Service]
Environment=WLR_BACKENDS=headless
Environment=WLR_LIBINPUT_NO_DEVICES=1
ExecStart=/usr/bin/sway
ExecReload=/bin/sh -c 'exec /usr/bin/swaymsg -s "$${SWAYSOCK:-%t/sway-ipc.%U.$MAINPID.sock}" reload'
ExecStopPost=-/usr/bin/systemctl --user stop graphical-session.target
Restart=on-failure
```

`ExecReload` lets `systemctl --user reload sway-headless` re-read the
configuration and keep every window: a reload changes nothing for wlshare's
pointer, which is bound to no output.

The outputs: `HEADLESS-1` pinned at the origin, and every output given the
starting mode and the background by `output *`, since an output wlshare has sway
make for a second virtual display is `HEADLESS-2`, `HEADLESS-3` and on, counting
up for the life of the compositor, and cannot be named in advance. A made output
has no position and goes to the right of the pinned one.

```
output HEADLESS-1 {
    mode 1920x1080
    pos 0 0
}
output * {
    mode 1920x1080
    bg #2e3440 solid_color
}
```

The mode is a starting one: wlshare resizes and rescales a headless output to
the client's window and density, and a reload of sway's configuration applies
this mode again. No `scale` belongs here for the same reason.

wlshare's configuration names the output and nothing about sway:

```toml
listen = "0.0.0.0:5900"
output = "HEADLESS-1"

[pam]
```

## The modifier key

The session's only keyboard is the virtual one wlshare makes, and under the
virtual-keyboard protocol the keymap is the one wlshare uploads: sway uses it as
given and ignores an `input ... xkb_options` block for that device
([swaywm/sway#6436](https://github.com/swaywm/sway/issues/6436)). So `$mod` on a
key a VNC client can send — Right Alt rather than a Super key the client's own
desktop keeps, or a lock key, which latches on the client before anything is
sent — is an XKB layout of its own, named under `[xkb]` and found through
`XKB_CONFIG_EXTRA_PATH`, as the user guide's
[keyboard](https://zdesk.dev/guides/wlshare#keyboard) says. Each key keeps its
keysym, since wlshare looks the client's keysyms up in the same keymap, and only
gains the modifier's action:

```
// /etc/xkb/symbols/swayvnc
default partial modifier_keys
xkb_symbols "modkey" {
    include "us(basic)"

    key <RALT> {
        type[Group1] = "ONE_LEVEL",
        symbols[Group1] = [ Alt_R ],
        actions[Group1] = [ SetMods(modifiers=Mod3) ]
    };
    key <LVL3> {
        type[Group1] = "ONE_LEVEL",
        symbols[Group1] = [ ISO_Level3_Shift ],
        actions[Group1] = [ SetMods(modifiers=Mod3) ]
    };
    key <RCTL> {
        type[Group1] = "ONE_LEVEL",
        symbols[Group1] = [ Control_R ],
        actions[Group1] = [ SetMods(modifiers=Mod3) ]
    };

    modifier_map Mod3 { <RALT>, <LVL3>, <RCTL> };
};
```

```
# sway's config
set $mod Mod3
```

```toml
# wlshare's config
[xkb]
layout = "swayvnc"
```

Right Alt is listed under both keysyms a client may send for it, `Alt_R` and
AltGr's `ISO_Level3_Shift`, and Right Ctrl is there for a PC keyboard on a
Mac's client, whose right-hand keys send something else; `wev` in the session
shows what a client sends. A headed
session's own keyboard takes the same layout (`input type:keyboard { xkb_layout
swayvnc }`), so `$mod` is the same keys at the console.

## A headed session with a remote output

A sway on a real display hands a client a headless output of the client's size
and density, the display dark while the desktop is watched from elsewhere. The
user guide's [desktop at the console](https://zdesk.dev/guides/wlshare#at-the-console)
has the two-line watcher; a session whose workspaces must come back where they
were needs more than that, because sway cannot put them back here.

On a desk of real monitors sway sends a workspace back to its output when the
output returns, knowing the output by its make, model and serial. A virtual
machine's connectors and a headless output have no EDID, so every one of them
is `Unknown Unknown Unknown` and a workspace goes home to whichever sway lists
first. sway also drops an empty workspace whose output goes away and gives an
output that returns a new one under the lowest free number, which may be the
name of one that was elsewhere. The watcher's script therefore records where
each workspace was before it disables the display, and replays that after
enabling it:

```toml
# wlshare's config
output = "HEADLESS-1"
state_socket = "wlshare/state.sock"
output_wait_secs = 10
```

```
# sway's config: HEADLESS-1 made at start and on every reload, far from the
# display's outputs so the pointer cannot wander onto it from the console.
output Virtual-1 pos 0 0
output HEADLESS-1 pos -20000 0
exec_always ~/.local/bin/sway-remote-displays create
bindsym $mod+Shift+Home exec ~/.local/bin/sway-remote-displays free
```

```
# ~/.config/systemd/user/wlshare-watch.service
[Unit]
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=/usr/bin/wlshare watch \
    --held "%h/.local/bin/sway-remote-displays held" \
    --free "%h/.local/bin/sway-remote-displays free" \
    --free-after-secs 15
Restart=always
RestartSec=1

[Install]
WantedBy=graphical-session.target
```

The script, in outline (`swaymsg` and `jq`, error handling left out). Each
command must be safe to run on a desktop already as it would leave it, since
the watcher runs the one for where the desktop stands whenever it starts:

```sh
remote=HEADLESS-1
state=$XDG_RUNTIME_DIR/sway-remote-displays.json
displays() { swaymsg -t get_outputs | jq -r --arg r "$remote" '.[] | select(.name != $r) | .name'; }

case $1 in
create)  # make HEADLESS-1 if it is missing; off unless a client holds the desktop
    swaymsg -t get_outputs | jq -e --arg r "$remote" 'any(.[]; .name == $r)' >/dev/null \
        || swaymsg create_output
    [ -s "$state" ] || swaymsg output "$remote" disable ;;
held)    # remote output on first, so the waiting client starts there
    swaymsg output "$remote" enable
    [ -s "$state" ] || swaymsg -t get_workspaces \
        | jq -c --arg r "$remote" '[.[] | select(.output != $r) | {name, output, focused, visible}]' > "$state"
    for o in $(displays); do swaymsg output "$o" disable; done ;;
free)    # display back, made outputs unplugged, each workspace home
    for o in $(displays); do swaymsg output "$o" enable; done
    for o in $(displays | grep '^HEADLESS-'); do swaymsg output "$o" unplug; done
    jq -r '.[] | [.name, .output] | @tsv' "$state" | while IFS="$(printf '\t')" read -r ws out; do
        swaymsg "workspace --no-auto-back-and-forth \"$ws\"; move workspace to output \"$out\""
    done
    swaymsg output "$remote" disable
    rm -f "$state" ;;
esac
```

The full script also sends a workspace made while the client held the desktop
to the output that had the focus, shows on each output the workspace it showed,
keeps the focus where the client left it, puts the pointer on a visible output,
and never disables the remote output while it is the only one on. `free`
unplugs a second virtual display the client asked for, since it outlives the
client and the console has no use for it.

The watcher is a unit apart from wlshare's so that a wlshare that crashes with a
client on the desktop still gives the display back: its socket closes, which the
watcher reads as the desktop left, and the wait before `free` (15 seconds here)
is longer than the five before the unit restarts wlshare, so a client that
reconnects finds its windows where they were
([`architecture.md`](architecture.md#the-watcher)).

Where such a session locks, the locker is swaylock: it puts its surface on
every output, gives one to an output that appears while it is up, and takes the
keyboard wherever the focus is, so the console and the client can each unlock
across the outputs changing under it. A locker that draws its form on one output
leaves it on whichever had the lock's focus when the outputs moved.
