//! Input injection: a virtual keyboard and a virtual pointer on the compositor's
//! seat.
//!
//! RFB carries X11 keysyms. The compositor takes evdev keycodes under whatever
//! keymap the virtual keyboard uploaded, so the same keymap serves twice: it is
//! uploaded to the compositor, and searched here for a keycode that produces
//! each incoming keysym. That is why a remapped modifier has to be a *layout*
//! that keeps every key's keysym rather than an option that rewrites it — see
//! the configuration.
//!
//! A character keysym names a character, not a key: the client resolved its own
//! Shift and Caps Lock before sending `A`, and the keycode that produces `A`
//! here is the same one that produces `a`. So the state the compositor will see
//! is checked before every press, and Shift is pressed or let go around the key
//! when the keycode alone would type the other case. A keysym that names a key
//! rather than a character -- Tab, Return, an arrow -- is left alone: it goes out
//! on its keycode under whatever the client holds, so Shift+Tab stays Shift+Tab
//! even though the keycode's shifted level is ISO_Left_Tab, not Tab.
//!
//! The keyboard is the seat's, one for both desks, and what each desk's client
//! holds on it is kept apart: a client leaving or being superseded lets go of
//! its own keys and buttons, and of nothing the other holds. A key both hold
//! is down until the last of them lets go of it.
//!
//! Pointer positions arrive in framebuffer pixels and go to the compositor as
//! absolute positions against the framebuffer's extent, which the virtual
//! pointer maps onto the output it was made for. So each desk has a pointer of
//! its own, on its own output, while a client is on it.
//!
//! A compositor can drop that binding without a word: labwc and Sway map every
//! pointer back to the output its device names, none for a virtual one, when
//! they reload their configuration, and the positions then span the whole
//! layout. The desk's cursor session says where the cursor is on its output, in
//! the framebuffer's pixels, so what follows the pointer's own motions is held
//! to where they put it ([`Witness`]), and a pointer the cursor is found away
//! from after a doubt is made again against its output. Every output has a
//! session of its own for that alone ([`crate::cursor::Watch`]), so the cursor
//! turning up on another output counts as much as a position that bears
//! nothing out.

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::fd::AsFd;
use std::time::Instant;

use anyhow::Context as _;
use log::{debug, trace, warn};
use wayland_client::protocol::wl_output::{Transform, WlOutput};
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::QueueHandle;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};
use xkbcommon::xkb;

use crate::compositor::Compositor;
use crate::config::Xkb;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// One wheel notch, in the units `wl_pointer.axis` uses.
const WHEEL_STEP: f64 = 15.0;
/// The most motions a pointer remembers while the compositor has yet to say
/// where they put the cursor: a session reports at the output's next commit,
/// a frame behind at most, and a client sends far fewer in a frame than this.
const RECENT: usize = 64;
/// How far a reported position may be from the one sent, in framebuffer
/// pixels: the compositor rounds the cursor's place on the output.
pub const SLACK: i32 = 1;
/// XKB keycodes are evdev codes plus eight.
const EVDEV_OFFSET: u32 = 8;
const XK_SHIFT_L: u32 = 0xffe1;
const XK_SHIFT_R: u32 = 0xffe2;

/// What was done to Shift around a key so its keysym came out, undone with the key.
enum ShiftFix {
    /// Shift was pressed for the key.
    Pressed(u32),
    /// These held Shift keycodes were let go for the key.
    Released(Vec<u32>),
}

/// Whether `keysym` names a printable character -- one the client has cased --
/// rather than a key. libxkbcommon gives Tab, Return, BackSpace, Escape and
/// Delete control codepoints, so those count as keys here too.
fn is_character(keysym: u32) -> bool {
    xkb::Keysym::new(keysym).key_char().is_some_and(|c| !c.is_control())
}

/// The modifier state the compositor is told after every key: an `xkb_state`
/// fed each keycode's transitions and nothing else.
///
/// xkb counts the presses of a modifier key and holds the modifier until as many
/// releases have arrived, so a press of a key already down -- a held Control
/// auto-repeating in the browser -- would leave the modifier depressed after the
/// one release that follows, and every later key would be typed under it. Only
/// a keycode going down or coming up reaches the state, and only that reaches
/// the compositor: see [`Keys::send_key`].
struct Modifiers {
    state: xkb::State,
    /// The keycodes the compositor has been told are down.
    down: HashSet<u32>,
}

impl Modifiers {
    fn new(keymap: &xkb::Keymap) -> Self {
        Self { state: xkb::State::new(keymap), down: HashSet::new() }
    }

    /// Whether the keycode went down or came up: a press of one already down,
    /// or a release of one that is not, changes nothing.
    fn key(&mut self, code: u32, down: bool) -> bool {
        let changed = if down { self.down.insert(code) } else { self.down.remove(&code) };
        if changed {
            self.state.update_key(xkb::Keycode::new(code), if down { xkb::KeyDirection::Down } else { xkb::KeyDirection::Up });
        }
        changed
    }
}

/// One key transition for the compositor, with the modifier state that follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sent {
    code: u32,
    down: bool,
    depressed: u32,
    latched: u32,
    locked: u32,
    layout: u32,
}

/// The client's keysyms turned into the key transitions the compositor is sent.
struct Keys {
    modifiers: Modifiers,
    /// Keysym → the keycode (and level) that produces it, preferring level 0.
    keycodes: HashMap<u32, (u32, u32)>,
    /// The keycodes of the Shift keys, for pressing one and recognising any.
    shift_codes: Vec<u32>,
    /// The keycodes the clients hold between them.
    held: HashSet<u32>,
    /// The keycodes each desk's client holds.
    owners: [HashSet<u32>; 2],
    /// Keys pressed with Shift corrected around them.
    fixes: HashMap<u32, ShiftFix>,
    /// The transitions of the call under way.
    sent: Vec<Sent>,
}

pub struct Input {
    keyboard: ZwpVirtualKeyboardV1,
    /// Each desk's pointer, while it has an output to point at.
    pointers: [Option<Pointer>; 2],
    /// The desk whose pointer moved the seat's cursor last. The cursor is the
    /// seat's one, so only that desk's session reports what its pointer did.
    moved_last: Option<usize>,
    /// Counts the times another desk took to moving the cursor: each sends a
    /// round trip, and what the sessions report before it comes back is of the
    /// cursor as the other desk left it, which bears out a motion and doubts
    /// nothing.
    movers: u64,
    /// The round trip of the latest of them has come back.
    settled: bool,
    keys: Keys,
    started: Instant,
}

/// A virtual pointer on one output.
struct Pointer {
    pointer: ZwlrVirtualPointerV1,
    /// The client's RFB button mask.
    buttons: u8,
    /// The last motion: the position and the extent it was against.
    motion: Option<(u16, u16, (u16, u16))>,
    witness: Witness,
}

impl Pointer {
    fn new(pointer: ZwlrVirtualPointerV1) -> Self {
        Self { pointer, buttons: 0, motion: None, witness: Witness::default() }
    }
}

/// An output as a motion meets it: its size in its pixels, upright, which a
/// resize gives before the framebuffer follows, and its transform.
#[derive(Clone, Copy)]
pub struct Placement {
    pub size: (u16, u16),
    pub transform: Transform,
}

/// Where the compositor puts the cursor for a motion to (x, y) of `extent`, in
/// the pixels of the output as `placement` has it, upright, which its cursor
/// session reports in.
///
/// The framebuffer is the output's buffer, as screencopy gives it, so a
/// motion is in the output's untransformed orientation; wlroots reads a mapped
/// pointer's absolute motion that way and turns it by the output's transform
/// (`apply_output_transform` in `wlr_cursor.c`) before it places the cursor,
/// so the place is the motion turned. A position past the extent, which the
/// page sends for a drag past the tab's edge, is held at the far edge, where
/// wlroots clamps it once it is mapped; and the extent is spread over the
/// output as it is, which a resize makes another size until the framebuffer
/// follows.
fn landing(x: u16, y: u16, extent: (u16, u16), placement: Placement) -> (i32, i32) {
    let (w, h) = (u64::from(extent.0.max(1)), u64::from(extent.1.max(1)));
    let (x, y) = (u64::from(x).min(w), u64::from(y).min(h));
    // Each upright axis as a fraction of the motion's: numerator, denominator.
    let (across, down) = match placement.transform {
        Transform::_90 => ((h - y, h), (x, w)),
        Transform::_180 => ((w - x, w), (h - y, h)),
        Transform::_270 => ((y, h), (w - x, w)),
        Transform::Flipped => ((w - x, w), (y, h)),
        Transform::Flipped90 => ((y, h), (x, w)),
        Transform::Flipped180 => ((x, w), (h - y, h)),
        Transform::Flipped270 => ((h - y, h), (w - x, w)),
        _ => ((x, w), (y, h)),
    };
    // Rounded, as wlroots rounds the output cursor's place.
    let place = |(at, of): (u64, u64), size: u16| ((2 * at * u64::from(size.max(1)) + of) / (2 * of)) as i32;
    (place(across, placement.size.0), place(down, placement.size.1))
}

/// Where a pointer's motions put the cursor, held against where its desk's
/// cursor session says the cursor is.
///
/// A session reports a position at the output's next commit, and only when it
/// changed, so a report is of one of the motions sent lately or of something
/// else: a mouse at the desk, the cursor moved by the layout, or the pointer's
/// binding lost. One report that bears out none of them is a doubt, and so is
/// the cursor turning up on another output, which a binding lost does when it
/// puts it there. The cursor leaving the output is a doubt too, but only that:
/// an application hiding it does the same, and nothing turns up anywhere. A
/// report that bears nothing out, or the cursor on another output, after a
/// doubt and a motion sent since is the binding lost, since each of the others
/// leaves the next motion where it was put. A pointer made again for it waits
/// for a report to bear it out before it is doubted again, so a pointer the
/// compositor keeps elsewhere, by a configuration of its own, is made again
/// once and not on every motion.
#[derive(Default)]
struct Witness {
    /// The positions sent, oldest first, back to the oldest one a report may
    /// still be of.
    sent: VecDeque<(i32, i32)>,
    /// How many motions have been sent.
    motions: u64,
    /// The count of motions at the report that bore none out, until one does.
    doubt: Option<u64>,
    /// Made again for its reports, and not borne out since.
    remade: bool,
}

impl Witness {
    fn sent(&mut self, x: i32, y: i32) {
        if self.sent.len() == RECENT {
            self.sent.pop_front();
        }
        self.sent.push_back((x, y));
        self.motions += 1;
    }

    /// The session says the cursor left the output.
    fn left(&mut self) {
        if !self.sent.is_empty() && !self.remade && self.doubt.is_none() {
            self.doubt = Some(self.motions);
        }
    }

    /// Whether (x, y) is where one of the motions sent put the cursor.
    fn borne_out(&mut self, x: i32, y: i32) -> bool {
        let Some(i) = self.sent.iter().rposition(|&(sx, sy)| (sx - x).abs() <= SLACK && (sy - y).abs() <= SLACK) else { return false };
        // What came before it the compositor has done with.
        self.sent.drain(..i);
        self.doubt = None;
        self.remade = false;
        true
    }

    /// The cursor is where none of the motions sent put it: on this output
    /// elsewhere, or on another output altogether. Whether the binding is lost.
    fn against(&mut self) -> bool {
        if self.sent.is_empty() || self.remade {
            return false;
        }
        match self.doubt {
            Some(at) if self.motions > at => {
                self.doubt = None;
                true
            }
            Some(_) => false,
            None => {
                self.doubt = Some(self.motions);
                false
            }
        }
    }
}

impl Keys {
    fn new(keymap: &xkb::Keymap) -> Self {
        let mut keycodes = HashMap::new();
        let min = keymap.min_keycode().raw();
        let max = keymap.max_keycode().raw();
        for code in min..=max {
            let key = xkb::Keycode::new(code);
            for layout in 0..keymap.num_layouts_for_key(key) {
                for level in 0..keymap.num_levels_for_key(key, layout) {
                    for sym in keymap.key_get_syms_by_level(key, layout, level) {
                        let entry = keycodes.entry(sym.raw()).or_insert((code, level));
                        if level < entry.1 {
                            *entry = (code, level);
                        }
                    }
                }
            }
        }
        debug!("keymap resolves {} keysyms", keycodes.len());
        let shift_codes: Vec<u32> = [XK_SHIFT_L, XK_SHIFT_R].iter().filter_map(|sym| keycodes.get(sym).map(|&(code, _)| code)).collect();
        if shift_codes.is_empty() {
            warn!("the keymap has no Shift key: case cannot be corrected");
        }
        Self { modifiers: Modifiers::new(keymap), keycodes, shift_codes, held: HashSet::new(), owners: Default::default(), fixes: HashMap::new(), sent: Vec::new() }
    }

    /// A KeyEvent, as the transitions to send for it.
    fn key(&mut self, keysym: u32, down: bool) -> Vec<Sent> {
        let Some(&(code, level)) = self.keycodes.get(&keysym) else {
            warn!("no key produces keysym {keysym:#x}; dropped");
            return Vec::new();
        };
        if down {
            // A repeat sends nothing: see [`Keys::send_key`].
            if self.held.insert(code) {
                self.fix_shift(code, level, keysym);
                self.send_key(code, true);
            }
        } else {
            self.let_go(code);
        }
        std::mem::take(&mut self.sent)
    }

    fn let_go(&mut self, code: u32) {
        if self.held.remove(&code) {
            self.release(code);
        } else {
            // A release of something the client does not hold, which is still
            // a key to let go of when a correction left it down.
            self.send_key(code, false);
        }
    }

    /// A desk's KeyEvent. The keyboard is one for both desks, so a key goes up
    /// only once neither client holds it: a release from one, of a key the
    /// other holds, is the other's to send.
    fn desk_key(&mut self, desk: usize, keysym: u32, down: bool) -> Vec<Sent> {
        if let Some(&(code, _)) = self.keycodes.get(&keysym) {
            if down {
                self.owners[desk].insert(code);
            } else {
                self.owners[desk].remove(&code);
                if self.owned(code) {
                    return Vec::new();
                }
            }
        }
        self.key(keysym, down)
    }

    fn owned(&self, code: u32) -> bool {
        self.owners.iter().any(|held| held.contains(&code))
    }

    /// Let go of every key a desk's client left held, but for those the other
    /// desk's holds too.
    fn desk_left(&mut self, desk: usize) -> Vec<Sent> {
        for code in std::mem::take(&mut self.owners[desk]) {
            if !self.owned(code) {
                self.let_go(code);
            }
        }
        // With nobody holding a key, what a correction left down goes too.
        if self.owners.iter().all(HashSet::is_empty) {
            return self.release_all();
        }
        std::mem::take(&mut self.sent)
    }

    /// Press or let go of Shift so that `code` types `keysym`: the keycode
    /// that produces `A` at level 1 produces `a` at level 0, and the client has
    /// already decided which one it means.
    ///
    /// Only a character has a case to decide. Tab is level 0 of the key whose
    /// level 1 is ISO_Left_Tab, and a browser reports "Tab" for both, so a
    /// client sending Tab with Shift held would otherwise have its Shift let go
    /// and type a plain Tab. Such a keysym is sent on its keycode as it is.
    fn fix_shift(&mut self, code: u32, level: u32, keysym: u32) {
        if !is_character(keysym) {
            return;
        }
        let produced = self.modifiers.state.key_get_one_sym(xkb::Keycode::new(code)).raw();
        if produced == keysym {
            return;
        }
        let shift_down = self.modifiers.state.mod_name_is_active(xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_DEPRESSED);
        if level == 1 && !shift_down {
            if let Some(&shift) = self.shift_codes.first() {
                debug!("keysym {keysym:#x} needs Shift on keycode {code}; pressing it");
                self.send_key(shift, true);
                self.fixes.insert(code, ShiftFix::Pressed(shift));
            }
        } else if level == 0 && shift_down {
            let shifts: Vec<u32> = self.shift_codes.iter().copied().filter(|s| self.held.contains(s)).collect();
            if !shifts.is_empty() {
                debug!("keysym {keysym:#x} needs keycode {code} without Shift; letting go of it");
                for &shift in &shifts {
                    self.send_key(shift, false);
                }
                self.fixes.insert(code, ShiftFix::Released(shifts));
            }
        } else {
            debug!("keysym {keysym:#x} is at level {level} of keycode {code}, which now produces {produced:#x}; sending the keycode as it is");
        }
    }

    /// A key the client let go of, or lost by leaving.
    ///
    /// The Shift pressed for a key is the client's own once the client presses
    /// that Shift key itself, and is then left down for the client to let go of.
    /// A Shift key the client lets go of is one a key's fix no longer holds
    /// either, so it goes up there and then, and not a second time with the key.
    fn release(&mut self, code: u32) {
        if self.shift_codes.contains(&code) {
            self.fixes.retain(|_, fix| !matches!(fix, ShiftFix::Pressed(shift) if *shift == code));
        }
        self.send_key(code, false);
        match self.fixes.remove(&code) {
            Some(ShiftFix::Pressed(shift)) => {
                if !self.held.contains(&shift) {
                    self.send_key(shift, false);
                }
            }
            Some(ShiftFix::Released(shifts)) => {
                for shift in shifts {
                    if self.held.contains(&shift) {
                        self.send_key(shift, true);
                    }
                }
            }
            None => {}
        }
    }

    /// A keycode going down or coming up, and nothing else: a press of a key
    /// the compositor has down is not sent.
    ///
    /// A client repeats a held key as presses -- RFB has no other way to say
    /// it, and a browser repeats a held modifier like any key -- and a Wayland
    /// client repeats what it is given by itself, so the compositor needs none
    /// of them. Sway takes each for a key of its own: it keeps the keys it has
    /// been told are down in a table of 32, a held Shift fills that in a
    /// second, and the key pressed next is then left out of it -- no binding
    /// matches it, and its release is never passed to the application, which
    /// goes on repeating it.
    fn send_key(&mut self, code: u32, down: bool) {
        if !self.modifiers.key(code, down) {
            return;
        }
        let state = &self.modifiers.state;
        self.sent.push(Sent {
            code,
            down,
            depressed: state.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            latched: state.serialize_mods(xkb::STATE_MODS_LATCHED),
            locked: state.serialize_mods(xkb::STATE_MODS_LOCKED),
            layout: state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        });
    }

    /// Let go of every key the client left held.
    fn release_all(&mut self) -> Vec<Sent> {
        for code in std::mem::take(&mut self.held) {
            self.release(code);
        }
        std::mem::take(&mut self.sent)
    }
}

impl Input {
    pub fn new(
        qh: &QueueHandle<Compositor>,
        keyboards: &ZwpVirtualKeyboardManagerV1,
        seat: &WlSeat,
        xkb_config: &Xkb,
    ) -> anyhow::Result<Self> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let options = if xkb_config.options.is_empty() { None } else { Some(xkb_config.options.clone()) };
        let keymap = xkb::Keymap::new_from_names(
            &context,
            &xkb_config.rules,
            &xkb_config.model,
            &xkb_config.layout,
            &xkb_config.variant,
            options,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| anyhow::anyhow!("no keymap compiles from {xkb_config:?}"))?;

        let text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        let fd = rustix::fs::memfd_create("wlshare-keymap", rustix::fs::MemfdFlags::CLOEXEC).context("memfd for the keymap")?;
        let mut file = std::fs::File::from(fd);
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        std::io::Write::write_all(&mut file, b"\0")?;
        let keyboard = keyboards.create_virtual_keyboard(seat, qh, ());
        keyboard.keymap(1, file.as_fd(), (text.len() + 1) as u32);

        Ok(Self {
            keyboard,
            pointers: [None, None],
            moved_last: None,
            movers: 0,
            settled: true,
            keys: Keys::new(&keymap),
            started: Instant::now(),
        })
    }

    fn time(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    pub fn key(&mut self, desk: usize, keysym: u32, down: bool) {
        let sent = self.keys.desk_key(desk, keysym, down);
        self.send_keys(&sent);
    }

    fn send_keys(&mut self, sent: &[Sent]) {
        let time = self.time();
        for key in sent {
            self.keyboard.key(time, key.code - EVDEV_OFFSET, u32::from(key.down));
            self.keyboard.modifiers(key.depressed, key.latched, key.locked, key.layout);
        }
    }

    /// Press and release the buttons that changed, without a frame.
    fn set_buttons(pointer: &ZwlrVirtualPointerV1, time: u32, before: u8, after: u8) {
        for (bit, code) in [(1u8, BTN_LEFT), (2, BTN_MIDDLE), (4, BTN_RIGHT)] {
            let now = after & bit != 0;
            if now != (before & bit != 0) {
                pointer.button(time, code, if now { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released });
            }
        }
    }

    /// A PointerEvent: position in framebuffer pixels, upright, and the RFB
    /// button mask, onto the output as it is now. Where this desk takes to
    /// moving the cursor from another, the round trip to send, by its count
    /// ([`Self::settle`]).
    #[must_use]
    pub fn pointer(&mut self, desk: usize, buttons: u8, x: u16, y: u16, extent: (u16, u16), output: Placement) -> Option<u64> {
        let time = self.time();
        let Some(Pointer { pointer, buttons: held, motion, witness }) = &mut self.pointers[desk] else { return None };
        pointer.motion_absolute(time, u32::from(x), u32::from(y), u32::from(extent.0.max(1)), u32::from(extent.1.max(1)));
        *motion = Some((x, y, extent));
        let (landed_x, landed_y) = landing(x, y, extent, output);
        trace!("desk {desk}: motion to {x},{y} of {}x{}, landing at {landed_x},{landed_y}", extent.0, extent.1);
        witness.sent(landed_x, landed_y);
        let took_over = self.moved_last.is_some_and(|last| last != desk);
        self.moved_last = Some(desk);
        let previous = std::mem::replace(held, buttons);
        Self::set_buttons(pointer, time, previous & 7, buttons & 7);
        // Wheel "buttons" scroll on the press; the release carries nothing.
        for (bit, axis, direction) in [
            (8u8, wl_pointer::Axis::VerticalScroll, -1.0),
            (16, wl_pointer::Axis::VerticalScroll, 1.0),
            (32, wl_pointer::Axis::HorizontalScroll, -1.0),
            (64, wl_pointer::Axis::HorizontalScroll, 1.0),
        ] {
            if buttons & bit != 0 && previous & bit == 0 {
                pointer.axis_source(wl_pointer::AxisSource::Wheel);
                pointer.axis_discrete(time, axis, WHEEL_STEP * direction, direction as i32);
            }
        }
        pointer.frame();
        took_over.then(|| {
            self.movers += 1;
            self.settled = false;
            self.movers
        })
    }

    /// The round trip sent when a desk took to moving the cursor came back.
    pub fn settle(&mut self, mover: u64) {
        if mover == self.movers {
            self.settled = true;
        }
    }

    /// The desk whose motion moved the cursor last.
    pub fn moved_last(&self) -> Option<usize> {
        self.moved_last
    }

    /// A Scroll: a distance in the output's logical pixels, the units an axis
    /// is in, positive rightward and downward, where the pointer is. It goes to
    /// the compositor as a continuous source: a distance the applications
    /// spend as it is, the way they spend a touchpad's, rather than a count of
    /// notches.
    pub fn scroll(&mut self, desk: usize, dx: i16, dy: i16) {
        if dx == 0 && dy == 0 {
            return;
        }
        let time = self.time();
        let Some(Pointer { pointer, .. }) = &self.pointers[desk] else { return };
        pointer.axis_source(wl_pointer::AxisSource::Continuous);
        for (axis, pixels) in [(wl_pointer::Axis::VerticalScroll, dy), (wl_pointer::Axis::HorizontalScroll, dx)] {
            if pixels != 0 {
                pointer.axis(time, axis, f64::from(pixels));
            }
        }
        pointer.frame();
    }

    /// Point a desk's pointer at an output: the one its client is shown, at
    /// first or after it asked for another. `zwlr_virtual_pointer` takes an
    /// output when it is made and never again, so the one bound to the old
    /// output is destroyed and a new one made against the new one. Whatever the
    /// client holds is let go first — a press cannot outlive the pointer that
    /// made it.
    pub fn point(&mut self, desk: usize, qh: &QueueHandle<Compositor>, pointers: &ZwlrVirtualPointerManagerV1, seat: &WlSeat, output: &WlOutput) {
        self.unpoint(desk);
        let pointer = pointers.create_virtual_pointer_with_output(Some(seat), Some(output), qh, ());
        self.pointers[desk] = Some(Pointer::new(pointer));
    }

    /// Make a desk's pointer again against the output it is already on: the
    /// compositor has laid its outputs out anew. labwc then maps every pointer
    /// back to the output its device names, which for a virtual one is none, so
    /// the binding made with the pointer is gone and absolute positions span the
    /// whole layout — the right place only while it is this one output. The new
    /// pointer is made before the old one goes, so the seat never loses its
    /// pointer and the cursor session keeps working; the buttons held are let go
    /// with the pointer that pressed them, the keys stay down.
    pub fn repoint(&mut self, desk: usize, qh: &QueueHandle<Compositor>, pointers: &ZwlrVirtualPointerManagerV1, seat: &WlSeat, output: &WlOutput) {
        let Some(old) = self.pointers[desk].take() else { return };
        let held = old.buttons & 7;
        if held != 0 {
            Self::set_buttons(&old.pointer, self.time(), held, 0);
            old.pointer.frame();
        }
        let pointer = pointers.create_virtual_pointer_with_output(Some(seat), Some(output), qh, ());
        old.pointer.destroy();
        self.pointers[desk] = Some(Pointer { motion: old.motion, ..Pointer::new(pointer) });
    }

    /// A desk's cursor session reports the cursor at (x, y) on its output, in
    /// framebuffer pixels: whether that says the desk's pointer is no longer
    /// bound to it ([`Witness`]). What follows another desk's motion is that
    /// desk's pointer's doing, not this one's.
    pub fn contradicted(&mut self, desk: usize, x: i32, y: i32) -> bool {
        if self.moved_last != Some(desk) {
            return false;
        }
        let settled = self.settled;
        let Some(pointer) = &mut self.pointers[desk] else { return false };
        if pointer.witness.borne_out(x, y) || !settled {
            return false;
        }
        Self::doubt(pointer, || format!("the cursor is at {x},{y}, where no recent motion put it"))
    }

    /// The cursor turned up on another output than a desk's, after that desk's
    /// motion: whether its pointer is no longer bound to its output.
    pub fn elsewhere(&mut self, desk: usize) -> bool {
        if self.moved_last != Some(desk) || !self.settled {
            return false;
        }
        let Some(pointer) = &mut self.pointers[desk] else { return false };
        Self::doubt(pointer, || "the cursor is on another output".to_owned())
    }

    /// Hold the cursor being where no motion put it against a pointer, and
    /// say so when that is the first doubt.
    fn doubt(pointer: &mut Pointer, why: impl FnOnce() -> String) -> bool {
        let doubted = pointer.witness.doubt.is_some();
        let lost = pointer.witness.against();
        if !doubted && pointer.witness.doubt.is_some() {
            debug!("{}: doubting the pointer", why());
        }
        lost
    }

    /// A desk's cursor session says the cursor left its output: a doubt of
    /// its pointer, where its motion was the last ([`Witness`]).
    pub fn cursor_left(&mut self, desk: usize) {
        if self.moved_last == Some(desk)
            && self.settled
            && let Some(pointer) = &mut self.pointers[desk]
        {
            pointer.witness.left();
        }
    }

    /// Make a desk's pointer again against its output, as [`Self::repoint`]
    /// does, because its cursor session says the binding is gone, and send the
    /// last motion again so the cursor goes where the client put it. The
    /// buttons are let go with the old pointer, as there, and pressed again by
    /// the client's next event that holds them.
    pub fn rebind(&mut self, desk: usize, qh: &QueueHandle<Compositor>, pointers: &ZwlrVirtualPointerManagerV1, seat: &WlSeat, output: &WlOutput, placement: Placement) {
        self.repoint(desk, qh, pointers, seat, output);
        let Some(pointer) = &mut self.pointers[desk] else { return };
        pointer.witness.remade = true;
        if let Some((x, y, extent)) = pointer.motion {
            // The desk's own motion again: nobody else took to moving it.
            let _ = self.pointer(desk, 0, x, y, extent, placement);
        }
    }

    /// Take a desk's pointer away, with everything its client held.
    pub fn unpoint(&mut self, desk: usize) {
        self.release_all(desk);
        if let Some(Pointer { pointer, .. }) = self.pointers[desk].take() {
            pointer.destroy();
        }
    }

    /// Let go of everything a desk's client left held.
    pub fn release_all(&mut self, desk: usize) {
        let time = self.time();
        let sent = self.keys.desk_left(desk);
        self.send_keys(&sent);
        if let Some(Pointer { pointer, buttons, .. }) = &mut self.pointers[desk] {
            let before = std::mem::take(buttons) & 7;
            if before != 0 {
                Self::set_buttons(pointer, time, before, 0);
                pointer.frame();
            }
        }
    }
}

wayland_client::delegate_noop!(Compositor: ignore ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(Compositor: ignore ZwpVirtualKeyboardV1);
wayland_client::delegate_noop!(Compositor: ignore ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(Compositor: ignore ZwlrVirtualPointerV1);

#[cfg(test)]
mod tests {
    use xkbcommon::xkb;

    use wayland_client::protocol::wl_output::Transform;

    use super::{EVDEV_OFFSET, Keys, Modifiers, Placement, RECENT, Sent, Witness, is_character, landing};
    use crate::shared::{BESIDE, FIRST};

    /// The evdev codes of Left Shift, Left Control, Left Alt and Left Meta.
    const MODIFIER_KEYS: [u32; 4] = [42, 29, 56, 125];

    /// The keycodes of Left Shift, Tab and A, and Shift's bit in the modifier mask.
    const SHIFT: u32 = 42 + EVDEV_OFFSET;
    const TAB: u32 = 15 + EVDEV_OFFSET;
    const A: u32 = 30 + EVDEV_OFFSET;
    const SHIFT_MASK: u32 = 1;
    const XK_SHIFT_L: u32 = 0xffe1;
    const XK_TAB: u32 = 0xff09;
    const XK_UPPER_A: u32 = 0x41;

    fn keymap() -> xkb::Keymap {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        xkb::Keymap::new_from_names(&context, "", "", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS).expect("the us layout compiles")
    }

    fn modifiers() -> Modifiers {
        Modifiers::new(&keymap())
    }

    /// What reached the compositor: each keycode, its direction, and the
    /// depressed modifiers reported after it.
    fn wire(sent: Vec<Sent>) -> Vec<(u32, bool, u32)> {
        sent.into_iter().map(|key| (key.code, key.down, key.depressed)).collect()
    }

    fn upright(size: (u16, u16)) -> Placement {
        Placement { size, transform: Transform::Normal }
    }

    #[test]
    fn a_motion_lands_where_the_compositor_spreads_its_extent_over_the_output() {
        assert_eq!(landing(349, 16, (1920, 928), upright((1920, 928))), (349, 16));
        // The output resized to 1789x903, the framebuffer still the old size.
        assert_eq!(landing(349, 16, (1920, 928), upright((1789, 903))), (325, 16));
        assert_eq!(landing(321, 39, (1920, 928), upright((1789, 903))), (299, 38));
        // Past the edge, held at the far edge, where wlroots clamps it, and
        // spread over an output grown to twice the size.
        assert_eq!(landing(4000, 5000, (3456, 1802), upright((3456, 1802))), (3456, 1802));
        assert_eq!(landing(1280, 10, (1280, 720), upright((2560, 1440))), (2560, 20));
    }

    #[test]
    fn a_motion_on_a_turned_output_lands_turned() {
        // Measured on Sway with `transform 90`, which is wl_output's 270: the
        // 1280x720 framebuffer's motions, and where the cursor session put
        // them on the upright 720x1280 output.
        let turned = Placement { size: (720, 1280), transform: Transform::_270 };
        for (sent, reported) in [((100, 100), (100, 1180)), ((600, 1000), (720, 680)), ((50, 1200), (720, 1230)), ((700, 20), (20, 580)), ((360, 640), (640, 920))] {
            assert_eq!(landing(sent.0, sent.1, (1280, 720), turned), reported, "{sent:?}");
        }
        // Every transform puts the extent's corners on the output's corners.
        let all = [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ];
        for transform in all {
            let quarter = matches!(transform, Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270);
            let size = if quarter { (720, 1280) } else { (1280, 720) };
            let placement = Placement { size, transform };
            for corner in [(0, 0), (1280, 0), (0, 720), (1280, 720)] {
                let (x, y) = landing(corner.0, corner.1, (1280, 720), placement);
                assert!([0, i32::from(size.0)].contains(&x) && [0, i32::from(size.1)].contains(&y), "{transform:?} {corner:?}");
            }
        }
    }

    /// The session reports the cursor at (x, y): whether the binding is lost.
    fn report(witness: &mut Witness, x: i32, y: i32) -> bool {
        !witness.borne_out(x, y) && witness.against()
    }

    #[test]
    fn the_cursor_kept_on_another_output_is_the_binding_lost() {
        // Every motion the stretched pointer sends lands on the other output,
        // so this desk's session says nothing after its leave.
        let mut witness = Witness::default();
        witness.sent(100, 100);
        witness.left();
        witness.sent(200, 100);
        assert!(witness.against(), "the cursor turned up on the other output");
        let mut witness = Witness::default();
        witness.sent(100, 100);
        assert!(!witness.against(), "once is a doubt");
        witness.sent(200, 100);
        assert!(witness.against());
    }

    #[test]
    fn a_hidden_cursor_never_loses_the_binding() {
        // A leave, and then nothing anywhere, however many motions follow.
        let mut witness = Witness::default();
        for x in 0..10 {
            witness.sent(x * 10, 0);
            witness.left();
        }
        assert!(witness.doubt.is_some());
        assert!(!report(&mut witness, 90, 0));
        assert_eq!(witness.doubt, None);
    }

    #[test]
    fn a_report_of_any_motion_still_in_flight_bears_the_pointer_out() {
        let mut witness = Witness::default();
        for x in [10, 20, 30] {
            witness.sent(x, 5);
        }
        // The compositor reports at its commits, so of an older motion first.
        assert!(!report(&mut witness, 10, 5));
        assert!(!report(&mut witness, 31, 4), "within a pixel of the newest");
        // Done with what came before the newest, and still with the newest.
        assert_eq!(witness.sent, [(30, 5)]);
        assert!(!report(&mut witness, 30, 5));
    }

    #[test]
    fn the_binding_is_lost_at_the_second_report_that_bears_nothing_out_after_a_motion_between() {
        let mut witness = Witness::default();
        witness.sent(100, 100);
        assert!(!report(&mut witness, 200, 100), "one report is a doubt");
        assert!(!report(&mut witness, 201, 100), "the same doubt, with no motion since");
        witness.sent(960, 540);
        assert!(report(&mut witness, 640, 540));
    }

    #[test]
    fn a_leave_is_a_doubt_and_never_the_binding_lost() {
        // The stretched pointer puts the cursor on the other output first.
        let mut witness = Witness::default();
        witness.sent(100, 100);
        witness.left();
        witness.sent(960, 540);
        assert!(report(&mut witness, 640, 540));
        // An application hides the cursor, and shows it where it was sent.
        let mut witness = Witness::default();
        witness.sent(100, 100);
        witness.left();
        witness.sent(110, 100);
        witness.left();
        assert!(!report(&mut witness, 110, 100));
        assert_eq!(witness.doubt, None);
    }

    #[test]
    fn a_report_that_bears_a_motion_out_ends_the_doubt() {
        // A mouse at the desk moves the cursor once; the client's next motion
        // puts it back where it was sent.
        let mut witness = Witness::default();
        witness.sent(100, 100);
        assert!(!report(&mut witness, 700, 300));
        witness.sent(110, 100);
        assert!(!report(&mut witness, 110, 100));
        witness.sent(120, 100);
        assert!(!report(&mut witness, 800, 300), "a doubt anew, not the second");
    }

    #[test]
    fn nothing_is_held_against_a_pointer_that_has_sent_nothing() {
        let mut witness = Witness::default();
        for _ in 0..3 {
            assert!(!report(&mut witness, 5, 5));
        }
        assert_eq!(witness.doubt, None);
    }

    #[test]
    fn a_pointer_made_again_is_not_doubted_until_a_report_bears_it_out() {
        // The compositor keeps it elsewhere by a configuration of its own:
        // made again once, and not on every motion.
        let mut witness = Witness { remade: true, ..Witness::default() };
        for x in 0..5 {
            witness.sent(x * 10, 0);
            assert!(!report(&mut witness, x * 10 + 500, 0));
        }
        witness.sent(70, 0);
        assert!(!report(&mut witness, 70, 0));
        assert!(!witness.remade);
        witness.sent(80, 0);
        assert!(!report(&mut witness, 500, 0));
        witness.sent(90, 0);
        assert!(report(&mut witness, 500, 0));
    }

    #[test]
    fn a_pointer_remembers_its_recent_motions_alone() {
        let mut witness = Witness::default();
        for x in 0..RECENT as i32 + 10 {
            witness.sent(x * 3, 0);
        }
        assert_eq!(witness.sent.len(), RECENT);
        assert_eq!(witness.sent.front(), Some(&(30, 0)));
    }

    #[test]
    fn a_shift_the_client_takes_over_outlives_the_key_it_was_pressed_for() {
        let mut keys = Keys::new(&keymap());
        // `A` with no Shift held, as under Caps Lock: Shift is pressed for it.
        assert_eq!(wire(keys.key(XK_UPPER_A, true)), [(SHIFT, true, SHIFT_MASK), (A, true, SHIFT_MASK)]);
        // The client presses that Shift itself, and it repeats: it is down.
        for _ in 0..3 {
            assert_eq!(wire(keys.key(XK_SHIFT_L, true)), []);
        }
        // Letting go of `A` leaves the Shift the client now holds.
        assert_eq!(wire(keys.key(XK_UPPER_A, false)), [(A, false, SHIFT_MASK)]);
        // So the Tab that follows is Shift+Tab.
        assert_eq!(wire(keys.key(XK_TAB, true)), [(TAB, true, SHIFT_MASK)]);
        assert_eq!(wire(keys.key(XK_TAB, false)), [(TAB, false, SHIFT_MASK)]);
        assert_eq!(wire(keys.key(XK_SHIFT_L, false)), [(SHIFT, false, 0)]);
        assert_eq!(wire(keys.release_all()), []);
    }

    #[test]
    fn a_held_key_is_pressed_once_however_often_the_client_repeats_it() {
        let mut keys = Keys::new(&keymap());
        assert_eq!(wire(keys.key(XK_SHIFT_L, true)), [(SHIFT, true, SHIFT_MASK)]);
        // Sway's table of pressed keys holds 32.
        for _ in 0..40 {
            assert_eq!(wire(keys.key(XK_SHIFT_L, true)), []);
        }
        assert_eq!(wire(keys.key(XK_TAB, true)), [(TAB, true, SHIFT_MASK)]);
        for _ in 0..40 {
            assert_eq!(wire(keys.key(XK_TAB, true)), []);
        }
        assert_eq!(wire(keys.key(XK_TAB, false)), [(TAB, false, SHIFT_MASK)]);
        assert_eq!(wire(keys.key(XK_SHIFT_L, false)), [(SHIFT, false, 0)]);
        assert_eq!(wire(keys.release_all()), []);
    }

    #[test]
    fn a_shift_pressed_for_a_key_goes_with_the_key() {
        let mut keys = Keys::new(&keymap());
        assert_eq!(wire(keys.key(XK_UPPER_A, true)), [(SHIFT, true, SHIFT_MASK), (A, true, SHIFT_MASK)]);
        assert_eq!(wire(keys.key(XK_UPPER_A, false)), [(A, false, SHIFT_MASK), (SHIFT, false, 0)]);
        assert_eq!(wire(keys.key(XK_TAB, true)), [(TAB, true, 0)]);
    }

    #[test]
    fn a_shift_the_client_took_over_and_let_go_of_goes_up_once() {
        let mut keys = Keys::new(&keymap());
        keys.key(XK_UPPER_A, true);
        keys.key(XK_SHIFT_L, true);
        assert_eq!(wire(keys.key(XK_SHIFT_L, false)), [(SHIFT, false, 0)]);
        assert_eq!(wire(keys.key(XK_UPPER_A, false)), [(A, false, 0)]);
    }

    #[test]
    fn a_key_both_desks_hold_goes_up_with_the_last_of_them() {
        let mut keys = Keys::new(&keymap());
        assert_eq!(wire(keys.desk_key(FIRST, XK_TAB, true)), [(TAB, true, 0)]);
        // The second press of a key already down is a repeat.
        assert_eq!(wire(keys.desk_key(BESIDE, XK_TAB, true)), []);
        assert_eq!(wire(keys.desk_key(FIRST, XK_TAB, false)), []);
        assert_eq!(wire(keys.desk_key(BESIDE, XK_TAB, false)), [(TAB, false, 0)]);
    }

    #[test]
    fn a_release_from_a_desk_that_never_held_the_key_leaves_the_other_its_key() {
        let mut keys = Keys::new(&keymap());
        keys.desk_key(FIRST, XK_SHIFT_L, true);
        assert_eq!(wire(keys.desk_key(BESIDE, XK_SHIFT_L, false)), []);
        assert_eq!(wire(keys.desk_left(BESIDE)), []);
        // The Shift is still down for the client that holds it.
        assert_eq!(wire(keys.desk_key(FIRST, XK_TAB, true)), [(TAB, true, SHIFT_MASK)]);
    }

    #[test]
    fn a_desk_leaving_lets_go_of_what_only_it_held() {
        let mut keys = Keys::new(&keymap());
        keys.desk_key(FIRST, XK_SHIFT_L, true);
        keys.desk_key(BESIDE, XK_SHIFT_L, true);
        keys.desk_key(BESIDE, XK_TAB, true);
        assert_eq!(wire(keys.desk_left(BESIDE)), [(TAB, false, SHIFT_MASK)]);
        assert_eq!(wire(keys.desk_left(FIRST)), [(SHIFT, false, 0)]);
    }

    fn depressed(modifiers: &Modifiers) -> u32 {
        modifiers.state.serialize_mods(xkb::STATE_MODS_DEPRESSED)
    }

    #[test]
    fn a_repeated_modifier_is_let_go_by_one_release() {
        for code in MODIFIER_KEYS.map(|evdev| evdev + EVDEV_OFFSET) {
            let mut modifiers = modifiers();
            modifiers.key(code, true);
            let held = depressed(&modifiers);
            assert_ne!(held, 0, "keycode {code} is a modifier");
            for _ in 0..3 {
                modifiers.key(code, true);
                assert_eq!(depressed(&modifiers), held, "keycode {code}");
            }
            modifiers.key(code, false);
            assert_eq!(depressed(&modifiers), 0, "keycode {code}");
        }
    }

    #[test]
    fn a_release_of_a_key_not_down_changes_nothing() {
        for code in MODIFIER_KEYS.map(|evdev| evdev + EVDEV_OFFSET) {
            let mut modifiers = modifiers();
            modifiers.key(code, false);
            assert_eq!(depressed(&modifiers), 0, "keycode {code}");
            modifiers.key(code, true);
            modifiers.key(code, false);
            assert_eq!(depressed(&modifiers), 0, "keycode {code}");
        }
    }

    #[test]
    fn cased_characters_are_corrected() {
        for keysym in [0x61, 0x41, 0x31, 0x21, 0x20, 0xe9, 0x0100_20ac] {
            assert!(is_character(keysym), "{keysym:#x}");
        }
    }

    #[test]
    fn keys_are_sent_as_they_are() {
        // Tab, ISO_Left_Tab, Return, BackSpace, Escape, Delete, Left, Shift_L, F1
        for keysym in [0xff09, 0xfe20, 0xff0d, 0xff08, 0xff1b, 0xffff, 0xff51, 0xffe1, 0xffbe] {
            assert!(!is_character(keysym), "{keysym:#x}");
        }
    }
}
