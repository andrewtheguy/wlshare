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
//! absolute positions against the whole layout: each desk's virtual pointer is
//! made with no output, which wlroots spreads over the layout's box, and is
//! sent the place of the pixel on the desk's output as a fraction of that box
//! ([`aim`]). Bound to nothing, the pointer has no binding for the compositor
//! to lose: labwc and Sway map every pointer back to the output its device
//! names, none for a virtual one, when they reload their configuration, which
//! stretches a pointer made with an output over the layout and leaves one made
//! without as it was. Each desk has a pointer of its own, so what its client
//! holds is let go apart from the other's.

use std::collections::{HashMap, HashSet};
use std::os::fd::AsFd;
use std::time::Instant;

use anyhow::Context as _;
use log::{debug, trace, warn};
use wayland_client::protocol::wl_output::Transform;
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
/// The extent every absolute motion is against: the layout's box, in this
/// many parts an axis, which is finer than a millionth of a layout unit on
/// any layout there is.
const EXTENT: u32 = u32::MAX;
/// How far inside an output's far edges wlroots keeps a point it puts on the
/// output (`wlr_box_closest_point`): a motion is held that far in, so that it
/// is on the output and not on the neighbour that starts at its edge.
const EDGE: f64 = 1.0 / 256.0;
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
    /// Each desk's pointer, while a client is on an output.
    pointers: [Option<Pointer>; 2],
    /// The desk whose client last put the seat's one cursor where it is, which
    /// both desks' pointers move.
    cursor: Option<usize>,
    keys: Keys,
    started: Instant,
}

/// A desk's virtual pointer.
struct Pointer {
    pointer: ZwlrVirtualPointerV1,
    /// The client's RFB button mask.
    buttons: u8,
    /// What its last motion was aimed at, on the output it is on.
    aimed: Option<(u32, u32)>,
}

/// A box in the layout: its position and size in the layout's units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// An output as a motion meets it: its box in the layout, as xdg-output has
/// it from the compositor, which is the box wlroots lays it out as -- its
/// pixels over its scale, truncated (`wlr_output_effective_resolution`); its
/// size in its pixels, upright, which a resize gives before the framebuffer
/// follows; its transform; and the box of the whole layout, the union of
/// every output's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub logical: Rect,
    pub size: (u16, u16),
    pub transform: Transform,
    pub layout: Rect,
}

/// The absolute motion for a position (x, y) of `extent` on the output
/// `placement` describes: each axis as a numerator over [`EXTENT`] of the
/// layout's box, which wlroots spreads a pointer made with no output over
/// (`wlr_cursor_absolute_to_layout_coords`), or `None` for a layout or an
/// output of no size.
///
/// The framebuffer is the output's buffer, as screencopy gives it, so a motion
/// is in the output's untransformed orientation, and is turned by the output's
/// transform here the way wlroots turns a pointer's that is mapped to an
/// output (`apply_output_transform` in `wlr_cursor.c`), and leaves one's that
/// is not. A position past the extent, which the page sends for a drag past
/// the tab's edge, is held at the far edge, and the extent is spread over the
/// output as it is, which a resize makes another size until the framebuffer
/// follows. The output's pixels are spread over its box, as wlroots spreads a
/// pointer's mapped to the output, and held within it as wlroots holds a point
/// it puts on an output: [`EDGE`] short of the far edges, since a point on the
/// edge two outputs share is the next one's. At a whole scale that is the
/// pixel's own place; a fractional one, whose pixels run past the box they
/// truncate to, lands a motion short of its pixel by up to two, as the
/// pointer mapped to the output did. The numerator is the least whose place,
/// as wlroots computes it in the same arithmetic, is not short of the pixel's:
/// a client is sent the place to 1/256 of a unit and Chromium floors it, so
/// one a hair under the integer is a pixel back, where one a hair over is the
/// pixel itself; and it is moved a part back where that put the place outside
/// the box.
fn aim(x: u16, y: u16, extent: (u16, u16), placement: &Placement) -> Option<(u32, u32)> {
    let (w, h) = (u64::from(extent.0.max(1)), u64::from(extent.1.max(1)));
    let (x, y) = (u64::from(x).min(w), u64::from(y).min(h));
    // Each upright axis as a fraction of the output: numerator, denominator.
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
    let axis = |(at, of): (u64, u64), pixels: u16, (box_at, units): (i32, i32), (layout_at, layout_size): (i32, i32)| -> Option<u32> {
        if units <= 0 || layout_size <= 0 {
            return None;
        }
        let pixel = at as f64 * f64::from(pixels) / of as f64;
        let (box_at, far) = (f64::from(box_at), f64::from(units) - EDGE);
        let place = box_at + (pixel * f64::from(units) / f64::from(pixels.max(1))).clamp(0.0, far);
        let (layout_at, layout_size) = (f64::from(layout_at), f64::from(layout_size));
        // What wlroots makes of a numerator, in its own arithmetic.
        let lands = |n: u32| layout_size * (f64::from(n) / f64::from(EXTENT)) + layout_at;
        let mut n = ((place - layout_at) / layout_size * f64::from(EXTENT)).ceil().clamp(0.0, f64::from(EXTENT)) as u32;
        while n < EXTENT && lands(n) < place {
            n += 1;
        }
        while n > 0 && lands(n) > box_at + far {
            n -= 1;
        }
        Some(n)
    };
    let (logical, layout) = (placement.logical, placement.layout);
    Some((
        axis(across, placement.size.0, (logical.x, logical.width), (layout.x, layout.width))?,
        axis(down, placement.size.1, (logical.y, logical.height), (layout.y, layout.height))?,
    ))
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
            cursor: None,
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
    /// button mask, onto the desk's output as it is laid out now ([`aim`]).
    /// With nothing to aim at, before the layout is known, a press or a notch
    /// would land wherever the cursor is, so only the buttons let go go.
    pub fn pointer(&mut self, desk: usize, buttons: u8, x: u16, y: u16, extent: (u16, u16), placement: Option<&Placement>) {
        let time = self.time();
        let Some(Pointer { pointer, buttons: held, aimed }) = &mut self.pointers[desk] else { return };
        let Some((ax, ay)) = placement.and_then(|placement| aim(x, y, extent, placement)) else {
            debug!("desk {desk}: motion to {x},{y} with no layout to aim at: {placement:?}");
            let kept = *held & buttons;
            if kept & 7 != *held & 7 {
                Self::set_buttons(pointer, time, *held & 7, kept & 7);
                pointer.frame();
            }
            *held = kept;
            return;
        };
        trace!("desk {desk}: motion to {x},{y} of {}x{}, aimed at {ax},{ay} of {EXTENT}", extent.0, extent.1);
        pointer.motion_absolute(time, ax, ay, EXTENT, EXTENT);
        *aimed = Some((ax, ay));
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
        self.cursor = Some(desk);
    }

    /// A Scroll: a distance in the output's logical pixels, the units an axis
    /// is in, positive rightward and downward, where the pointer is. It goes to
    /// the compositor as a continuous source: a distance the applications
    /// spend as it is, the way they spend a touchpad's, rather than a count of
    /// notches. The cursor is the seat's, which the other desk may have moved
    /// since, so it is first put back where this desk's client last put it,
    /// and the scroll is dropped where it has put it nowhere on its output yet.
    pub fn scroll(&mut self, desk: usize, dx: i16, dy: i16) {
        if dx == 0 && dy == 0 {
            return;
        }
        let time = self.time();
        let Some(Pointer { pointer, aimed, .. }) = &self.pointers[desk] else { return };
        if self.cursor != Some(desk) {
            let Some((ax, ay)) = *aimed else {
                debug!("desk {desk}: a scroll before any motion on its output, dropped");
                return;
            };
            pointer.motion_absolute(time, ax, ay, EXTENT, EXTENT);
        }
        pointer.axis_source(wl_pointer::AxisSource::Continuous);
        for (axis, pixels) in [(wl_pointer::Axis::VerticalScroll, dy), (wl_pointer::Axis::HorizontalScroll, dx)] {
            if pixels != 0 {
                pointer.axis(time, axis, f64::from(pixels));
            }
        }
        pointer.frame();
        self.cursor = Some(desk);
    }

    /// Give a desk a pointer, if it has none, and let go of whatever its
    /// client holds: the client is on an output, at first or after it asked
    /// for another, and a press does not carry over to the new one. The
    /// pointer is made with no output, so the compositor binds it to none and
    /// has nothing to lose; its motions say where on the layout ([`aim`]).
    pub fn point(&mut self, desk: usize, qh: &QueueHandle<Compositor>, pointers: &ZwlrVirtualPointerManagerV1, seat: &WlSeat) {
        self.release_all(desk);
        self.leave_cursor(desk);
        match &mut self.pointers[desk] {
            Some(Pointer { aimed, .. }) => *aimed = None,
            None => {
                let pointer = pointers.create_virtual_pointer(Some(seat), qh, ());
                self.pointers[desk] = Some(Pointer { pointer, buttons: 0, aimed: None });
            }
        }
    }

    /// The desk's client no longer says where the cursor is.
    fn leave_cursor(&mut self, desk: usize) {
        if self.cursor == Some(desk) {
            self.cursor = None;
        }
    }

    /// Take a desk's pointer away, with everything its client held.
    pub fn unpoint(&mut self, desk: usize) {
        self.release_all(desk);
        self.leave_cursor(desk);
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

    use super::{EDGE, EVDEV_OFFSET, EXTENT, Keys, Modifiers, Placement, Rect, Sent, aim, is_character};
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

    /// Where wlroots puts a numerator over [`EXTENT`] of a layout axis at
    /// `at`, `size` units long: `wlr_cursor_absolute_to_layout_coords`.
    fn lands(n: u32, at: i32, size: i32) -> f64 {
        f64::from(size) * (f64::from(n) / f64::from(EXTENT)) + f64::from(at)
    }

    /// Where wlroots puts the cursor for the motion `aim` sends, in the
    /// layout's units.
    fn placed(x: u16, y: u16, extent: (u16, u16), placement: &Placement) -> (f64, f64) {
        let (ax, ay) = aim(x, y, extent, placement).expect("a layout to aim at");
        (lands(ax, placement.layout.x, placement.layout.width), lands(ay, placement.layout.y, placement.layout.height))
    }

    /// Within a hundred-thousandth of a unit, which a 2^32 extent is well under.
    fn near(got: (f64, f64), want: (f64, f64)) -> bool {
        (got.0 - want.0).abs() < 1e-5 && (got.1 - want.1).abs() < 1e-5
    }

    /// The box `size` pixels at `scale` are laid out as.
    fn laid_out(size: (u16, u16), scale: f64) -> (i32, i32) {
        ((f64::from(size.0) / scale).trunc() as i32, (f64::from(size.1) / scale).trunc() as i32)
    }

    /// An output of `size` pixels at `scale`, turned by `transform`, at the
    /// layout's origin, with a 1280x720 output at scale 1 to its right.
    fn beside(size: (u16, u16), scale: f64, transform: Transform) -> Placement {
        let (width, height) = laid_out(size, scale);
        Placement {
            logical: Rect { x: 0, y: 0, width, height },
            size,
            transform,
            layout: Rect { x: 0, y: 0, width: width + 1280, height: height.max(720) },
        }
    }

    fn upright(size: (u16, u16)) -> Placement {
        beside(size, 1.0, Transform::Normal)
    }

    #[test]
    fn a_motion_lands_on_its_pixel_with_the_extent_spread_over_the_output() {
        assert!(near(placed(349, 16, (1920, 928), &upright((1920, 928))), (349.0, 16.0)));
        assert!(near(placed(0, 0, (1920, 928), &upright((1920, 928))), (0.0, 0.0)));
        assert!(near(placed(1919, 927, (1920, 928), &upright((1920, 928))), (1919.0, 927.0)));
        // The output resized to 1789x903, the framebuffer still the old size.
        assert!(near(placed(349, 16, (1920, 928), &upright((1789, 903))), (349.0 * 1789.0 / 1920.0, 16.0 * 903.0 / 928.0)));
        // Past the edge, held at the far edge, inside it as wlroots keeps a
        // point, and spread over an output grown to twice the size.
        assert!(near(placed(4000, 5000, (3456, 1802), &upright((3456, 1802))), (3456.0 - EDGE, 1802.0 - EDGE)));
        assert!(near(placed(1280, 10, (1280, 720), &upright((2560, 1440))), (2560.0 - EDGE, 20.0)));
    }

    #[test]
    fn the_far_edge_is_the_outputs_own_and_not_the_neighbours() {
        let placement = upright((1280, 720));
        let (ax, _) = aim(1280, 360, (1280, 720), &placement).unwrap();
        let x = lands(ax, 0, 2560);
        assert!(x <= 1280.0 - EDGE && x > 1280.0 - EDGE - 1e-5, "{x}");
        // One part more would be past what wlroots keeps on the output.
        assert!(lands(ax + 1, 0, 2560) > 1280.0 - EDGE);
    }

    #[test]
    fn a_motion_on_the_second_output_is_aimed_past_the_first() {
        // A 1920x1080 output at the origin, the desk's 1280x720 to its right.
        let placement = Placement {
            logical: Rect { x: 1920, y: 0, width: 1280, height: 720 },
            size: (1280, 720),
            transform: Transform::Normal,
            layout: Rect { x: 0, y: 0, width: 3200, height: 1080 },
        };
        assert!(near(placed(0, 0, (1280, 720), &placement), (1920.0, 0.0)));
        assert!(near(placed(640, 360, (1280, 720), &placement), (2560.0, 360.0)));
        assert!(near(placed(1279, 719, (1280, 720), &placement), (3199.0, 719.0)));
        assert!(near(placed(1280, 720, (1280, 720), &placement), (3200.0 - EDGE, 720.0 - EDGE)));
        // The layout starting left of the origin, as an output placed there has it.
        let shifted = Placement { logical: Rect { x: -1280, y: -100, ..placement.logical }, layout: Rect { x: -1280, y: -100, ..placement.layout }, ..placement };
        assert!(near(placed(0, 0, (1280, 720), &shifted), (-1280.0, -100.0)));
        assert!(near(placed(640, 360, (1280, 720), &shifted), (-640.0, 260.0)));
    }

    #[test]
    fn a_fractional_scale_spreads_the_pixels_over_the_box_it_truncates_to() {
        // 3456 pixels at 2.75 are 1256 layout units, which are 3454 pixels:
        // the pixels are spread over the box, as a pointer mapped to the
        // output spreads them, so a motion lands short of its pixel by up to
        // two, and the far edge is held inside the box. 1802 are 655.
        let fractional = beside((3456, 1802), 2.75, Transform::Normal);
        assert_eq!((fractional.logical.width, fractional.logical.height), (1256, 655));
        let (x, y) = placed(2593, 901, (3456, 1802), &fractional);
        assert!(near((x, y), (2593.0 * 1256.0 / 3456.0, 901.0 * 655.0 / 1802.0)));
        assert_eq!(((x * 2.75).floor(), (y * 2.75).floor()), (2591.0, 900.0));
        assert!(near(placed(3456, 1802, (3456, 1802), &fractional), (1256.0 - EDGE, 655.0 - EDGE)));
        // A whole scale divides the pixels exactly, so every one is reached.
        let whole = beside((3456, 1802), 2.0, Transform::Normal);
        assert!(near(placed(2593, 901, (3456, 1802), &whole), (1296.5, 450.5)));
        assert!(near(placed(3455, 1801, (3456, 1802), &whole), (1727.5, 900.5)));
    }

    #[test]
    fn a_motion_on_a_turned_output_lands_turned() {
        // Measured on Sway with `transform 90`, which is wl_output's 270: the
        // 1280x720 framebuffer's motions, and where the cursor session put
        // them on the upright 720x1280 output.
        let turned = beside((720, 1280), 1.0, Transform::_270);
        for (sent, want) in [((100, 100), (100.0, 1180.0)), ((600, 1000), (720.0 - EDGE, 680.0)), ((50, 1200), (720.0 - EDGE, 1230.0)), ((700, 20), (20.0, 580.0)), ((360, 640), (640.0, 920.0))] {
            assert!(near(placed(sent.0, sent.1, (1280, 720), &turned), want), "{sent:?}");
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
            let placement = beside(size, 1.0, transform);
            for corner in [(0, 0), (1280, 0), (0, 720), (1280, 720)] {
                let (x, y) = placed(corner.0, corner.1, (1280, 720), &placement);
                let edge = |at: f64, size: u16| at.abs() < 1e-5 || (at - (f64::from(size) - EDGE)).abs() < 1e-5;
                assert!(edge(x, size.0) && edge(y, size.1), "{transform:?} {corner:?}: {x},{y}");
            }
        }
    }

    #[test]
    fn a_motion_is_never_short_of_its_pixel_as_wlroots_computes_it() {
        // A client is sent the place to 1/256 of a unit and may floor it, so
        // a hair under the integer would be a pixel back: every pixel of a
        // 1400x900 output beside a 1280x720 one lands at or over itself, and
        // under a millionth over.
        let placement = Placement { logical: Rect { x: 0, y: 0, width: 1400, height: 900 }, size: (1400, 900), transform: Transform::Normal, layout: Rect { x: 0, y: 0, width: 2680, height: 900 } };
        for x in 0..1400u16 {
            let (lx, ly) = placed(x, x % 900, (1400, 900), &placement);
            assert!(lx >= f64::from(x) && lx - f64::from(x) < 1e-6, "{x}: {lx}");
            assert!(ly >= f64::from(x % 900) && ly - f64::from(x % 900) < 1e-6, "{}: {ly}", x % 900);
        }
        // So is one on the output to the right, from the layout's origin.
        let beside = Placement { logical: Rect { x: 1400, y: 0, width: 1280, height: 720 }, size: (1280, 720), ..placement };
        for x in 0..1280u16 {
            let (lx, _) = placed(x, 0, (1280, 720), &beside);
            assert!(lx >= 1400.0 + f64::from(x) && lx - 1400.0 - f64::from(x) < 1e-6, "{x}: {lx}");
        }
    }

    #[test]
    fn a_layout_or_an_output_of_no_size_gives_nothing_to_aim_at() {
        let mut placement = upright((1280, 720));
        placement.layout.width = 0;
        assert_eq!(aim(640, 360, (1280, 720), &placement), None);
        let mut placement = upright((1280, 720));
        placement.logical.height = 0;
        assert_eq!(aim(640, 360, (1280, 720), &placement), None);
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
