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
//! A keysym names a character, not a key: the client resolved its own Shift and
//! Caps Lock before sending `A`, and the keycode that produces `A` here is the
//! same one that produces `a`. So the state the compositor will see is checked
//! before every press, and Shift is pressed or let go around the key when the
//! keycode alone would type the other case.
//!
//! What each client holds is kept apart, so a client leaving lets go of its own
//! keys and buttons and nobody else's.
//!
//! Pointer positions arrive in framebuffer pixels and go to the compositor as
//! absolute positions against the framebuffer's extent, which the virtual
//! pointer maps onto the shared output.

use std::collections::{HashMap, HashSet};
use std::os::fd::AsFd;
use std::time::Instant;

use anyhow::Context as _;
use log::{debug, warn};
use wayland_client::protocol::wl_output::WlOutput;
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
use crate::shared::ClientId;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// One wheel notch, in the units `wl_pointer.axis` uses.
const WHEEL_STEP: f64 = 15.0;
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

pub struct Input {
    keyboard: ZwpVirtualKeyboardV1,
    pointer: ZwlrVirtualPointerV1,
    state: xkb::State,
    /// Keysym → the keycode (and level) that produces it, preferring level 0.
    keycodes: HashMap<u32, (u32, u32)>,
    /// The keycodes of the Shift keys, for pressing one and recognising any.
    shift_codes: Vec<u32>,
    /// Keys held, by the client holding them.
    held: HashMap<ClientId, HashSet<u32>>,
    /// Keys pressed with Shift corrected around them.
    fixes: HashMap<u32, ShiftFix>,
    /// Each client's RFB button mask; the compositor sees their union.
    buttons: HashMap<ClientId, u8>,
    started: Instant,
}

impl Input {
    pub fn new(
        qh: &QueueHandle<Compositor>,
        keyboards: &ZwpVirtualKeyboardManagerV1,
        pointers: &ZwlrVirtualPointerManagerV1,
        seat: &WlSeat,
        output: &WlOutput,
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

        let text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        let fd = rustix::fs::memfd_create("swayrx-keymap", rustix::fs::MemfdFlags::CLOEXEC).context("memfd for the keymap")?;
        let mut file = std::fs::File::from(fd);
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        std::io::Write::write_all(&mut file, b"\0")?;
        let keyboard = keyboards.create_virtual_keyboard(seat, qh, ());
        keyboard.keymap(1, file.as_fd(), (text.len() + 1) as u32);

        let pointer = pointers.create_virtual_pointer_with_output(Some(seat), Some(output), qh, ());
        Ok(Self {
            keyboard,
            pointer,
            state: xkb::State::new(&keymap),
            keycodes,
            shift_codes,
            held: HashMap::new(),
            fixes: HashMap::new(),
            buttons: HashMap::new(),
            started: Instant::now(),
        })
    }

    fn time(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    pub fn key(&mut self, client: ClientId, keysym: u32, down: bool) {
        let Some(&(code, level)) = self.keycodes.get(&keysym) else {
            warn!("no key produces keysym {keysym:#x}; dropped");
            return;
        };
        if down {
            if !self.held.entry(client).or_default().insert(code) {
                // A repeat: the state is as the first press left it.
                self.send_key(code, true);
                return;
            }
            self.fix_shift(code, level, keysym);
            self.send_key(code, true);
        } else {
            let was_held = self.held.get_mut(&client).is_some_and(|held| held.remove(&code));
            if !was_held {
                // A release of something not held: the compositor counts the
                // state anyway, so send it as it is.
                self.send_key(code, false);
                return;
            }
            self.release(code);
        }
    }

    /// Press or let go of Shift so that `code` types `keysym`: the keycode
    /// that produces `A` at level 1 produces `a` at level 0, and the client has
    /// already decided which one it means.
    fn fix_shift(&mut self, code: u32, level: u32, keysym: u32) {
        let produced = self.state.key_get_one_sym(xkb::Keycode::new(code)).raw();
        if produced == keysym {
            return;
        }
        let shift_down = self.state.mod_name_is_active(xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_DEPRESSED);
        if level == 1 && !shift_down {
            if let Some(&shift) = self.shift_codes.first() {
                debug!("keysym {keysym:#x} needs Shift on keycode {code}; pressing it");
                self.send_key(shift, true);
                self.fixes.insert(code, ShiftFix::Pressed(shift));
            }
        } else if level == 0 && shift_down {
            let shifts: Vec<u32> = self.shift_codes.iter().copied().filter(|s| self.held_by_anyone(*s)).collect();
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

    fn held_by_anyone(&self, code: u32) -> bool {
        self.held.values().any(|held| held.contains(&code))
    }

    /// A key one client let go of, or lost by leaving.
    fn release(&mut self, code: u32) {
        if self.held_by_anyone(code) {
            // Another client still holds it.
            return;
        }
        self.send_key(code, false);
        match self.fixes.remove(&code) {
            Some(ShiftFix::Pressed(shift)) => self.send_key(shift, false),
            Some(ShiftFix::Released(shifts)) => {
                for shift in shifts {
                    if self.held_by_anyone(shift) {
                        self.send_key(shift, true);
                    }
                }
            }
            None => {}
        }
    }

    fn send_key(&mut self, code: u32, down: bool) {
        let time = self.time();
        self.keyboard.key(time, code - EVDEV_OFFSET, u32::from(down));
        self.state.update_key(xkb::Keycode::new(code), if down { xkb::KeyDirection::Down } else { xkb::KeyDirection::Up });
        self.keyboard.modifiers(
            self.state.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            self.state.serialize_mods(xkb::STATE_MODS_LATCHED),
            self.state.serialize_mods(xkb::STATE_MODS_LOCKED),
            self.state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        );
    }

    /// The button mask the compositor sees: every client's, together.
    fn buttons_down(&self) -> u8 {
        self.buttons.values().fold(0, |all, mask| all | mask) & 7
    }

    /// Press and release the buttons whose union changed, without a frame.
    fn set_buttons(&mut self, before: u8, after: u8) {
        let time = self.time();
        for (bit, code) in [(1u8, BTN_LEFT), (2, BTN_MIDDLE), (4, BTN_RIGHT)] {
            let now = after & bit != 0;
            if now != (before & bit != 0) {
                self.pointer.button(time, code, if now { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released });
            }
        }
    }

    /// A PointerEvent: position in framebuffer pixels and the RFB button mask.
    pub fn pointer(&mut self, client: ClientId, buttons: u8, x: u16, y: u16, extent: (u16, u16)) {
        let time = self.time();
        self.pointer.motion_absolute(time, u32::from(x), u32::from(y), u32::from(extent.0.max(1)), u32::from(extent.1.max(1)));
        let before = self.buttons_down();
        let previous = self.buttons.insert(client, buttons).unwrap_or(0);
        let after = self.buttons_down();
        self.set_buttons(before, after);
        // Wheel "buttons" scroll on the press; the release carries nothing.
        for (bit, axis, direction) in [
            (8u8, wl_pointer::Axis::VerticalScroll, -1.0),
            (16, wl_pointer::Axis::VerticalScroll, 1.0),
            (32, wl_pointer::Axis::HorizontalScroll, -1.0),
            (64, wl_pointer::Axis::HorizontalScroll, 1.0),
        ] {
            if buttons & bit != 0 && previous & bit == 0 {
                self.pointer.axis_source(wl_pointer::AxisSource::Wheel);
                self.pointer.axis_discrete(time, axis, WHEEL_STEP * direction, direction as i32);
            }
        }
        self.pointer.frame();
    }

    /// Let go of everything a departed client left held — and nothing another
    /// client holds.
    pub fn release_client(&mut self, client: ClientId) {
        let held = self.held.remove(&client).unwrap_or_default();
        for code in held {
            self.release(code);
        }
        let before = self.buttons_down();
        self.buttons.remove(&client);
        let after = self.buttons_down();
        if after != before {
            self.set_buttons(before, after);
            self.pointer.frame();
        }
    }
}

wayland_client::delegate_noop!(Compositor: ignore ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(Compositor: ignore ZwpVirtualKeyboardV1);
wayland_client::delegate_noop!(Compositor: ignore ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(Compositor: ignore ZwlrVirtualPointerV1);
