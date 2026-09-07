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

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// One wheel notch, in the units `wl_pointer.axis` uses.
const WHEEL_STEP: f64 = 15.0;
/// XKB keycodes are evdev codes plus eight.
const EVDEV_OFFSET: u32 = 8;

pub struct Input {
    keyboard: ZwpVirtualKeyboardV1,
    pointer: ZwlrVirtualPointerV1,
    state: xkb::State,
    /// Keysym → the keycode (and level) that produces it, preferring level 0.
    keycodes: HashMap<u32, (u32, u32)>,
    pressed: HashSet<u32>,
    buttons: u8,
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

        let text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        let fd = rustix::fs::memfd_create("swayrx-keymap", rustix::fs::MemfdFlags::CLOEXEC).context("memfd for the keymap")?;
        let mut file = std::fs::File::from(fd);
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        std::io::Write::write_all(&mut file, b"\0")?;
        let keyboard = keyboards.create_virtual_keyboard(seat, qh, ());
        keyboard.keymap(1, file.as_fd(), (text.len() + 1) as u32);

        let pointer = pointers.create_virtual_pointer_with_output(Some(seat), Some(output), qh, ());
        Ok(Self { keyboard, pointer, state: xkb::State::new(&keymap), keycodes, pressed: HashSet::new(), buttons: 0, started: Instant::now() })
    }

    fn time(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    pub fn key(&mut self, keysym: u32, down: bool) {
        let Some(&(code, level)) = self.keycodes.get(&keysym) else {
            warn!("no key produces keysym {keysym:#x}; dropped");
            return;
        };
        if level > 0 {
            debug!("keysym {keysym:#x} is at level {level} of keycode {code}; sending the keycode as it is");
        }
        if down && !self.pressed.insert(code) || !down && !self.pressed.remove(&code) {
            // A repeat or a release of something not held: the compositor would
            // count the state anyway, so send it as it is.
        }
        self.send_key(code, down);
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

    /// A PointerEvent: position in framebuffer pixels and the RFB button mask.
    pub fn pointer(&mut self, buttons: u8, x: u16, y: u16, extent: (u16, u16)) {
        let time = self.time();
        self.pointer.motion_absolute(time, u32::from(x), u32::from(y), u32::from(extent.0.max(1)), u32::from(extent.1.max(1)));
        for (bit, code) in [(1u8, BTN_LEFT), (2, BTN_MIDDLE), (4, BTN_RIGHT)] {
            let now = buttons & bit != 0;
            if now != (self.buttons & bit != 0) {
                self.pointer.button(time, code, if now { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released });
            }
        }
        // Wheel "buttons" scroll on the press; the release carries nothing.
        for (bit, axis, direction) in [
            (8u8, wl_pointer::Axis::VerticalScroll, -1.0),
            (16, wl_pointer::Axis::VerticalScroll, 1.0),
            (32, wl_pointer::Axis::HorizontalScroll, -1.0),
            (64, wl_pointer::Axis::HorizontalScroll, 1.0),
        ] {
            if buttons & bit != 0 && self.buttons & bit == 0 {
                self.pointer.axis_source(wl_pointer::AxisSource::Wheel);
                self.pointer.axis_discrete(time, axis, WHEEL_STEP * direction, direction as i32);
            }
        }
        self.pointer.frame();
        self.buttons = buttons;
    }

    /// Let go of everything a departed client left held.
    pub fn release_all(&mut self) {
        let held: Vec<u32> = self.pressed.drain().collect();
        for code in held {
            self.send_key(code, false);
        }
        if self.buttons & 7 != 0 {
            let time = self.time();
            for (bit, code) in [(1u8, BTN_LEFT), (2, BTN_MIDDLE), (4, BTN_RIGHT)] {
                if self.buttons & bit != 0 {
                    self.pointer.button(time, code, wl_pointer::ButtonState::Released);
                }
            }
            self.pointer.frame();
        }
        self.buttons = 0;
    }
}

wayland_client::delegate_noop!(Compositor: ignore ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(Compositor: ignore ZwpVirtualKeyboardV1);
wayland_client::delegate_noop!(Compositor: ignore ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(Compositor: ignore ZwlrVirtualPointerV1);
