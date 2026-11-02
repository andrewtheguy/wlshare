//! The compositor's outputs: which one is shared, how big it is in pixels, and
//! the exact scale it is drawn at.
//!
//! Two protocols describe an output. `wl_output` gives its name, pixel mode and
//! an integer scale (wlroots reports the ceiling of a fractional one).
//! wlr-output-management gives the exact scale as the compositor has it, and is
//! the only way to *change* anything: a custom mode for a client's resize, a
//! mode and a scale together for a client's density. Heads are matched to
//! outputs by name.
//!
//! Only a headless output is ever reconfigured, the way wayvnc has it: a real
//! monitor's mode belongs to the person sitting at it.
//!
//! Which output is shared is the configuration's to begin with and the client's
//! from there: a client that speaks the outputs extension is sent the list and
//! may name another one ([`wlshare_proto::outputs`], [`Outputs::entries`]). The
//! configured output need not be there: another is shared while it is not, and
//! the desktop moves to it when it appears ([`Outputs::wanted`]).

use std::collections::HashMap;

use log::{debug, info, warn};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

use wlshare_proto::outputs::OutputEntry;

use crate::compositor::Compositor;
use crate::input::{Placement, Rect};
use crate::shared::{ClientId, Event, FIRST};

pub struct OutputInfo {
    pub output: WlOutput,
    pub global: u32,
    pub name: Option<String>,
    /// The current mode, in pixels, before the transform.
    pub mode: (i32, i32),
    pub transform: wl_output::Transform,
    pub wl_scale: i32,
    /// Its first `wl_output.done` with a box has come: before it, it is not an
    /// output a client may share, nor one the pointer can be aimed on.
    pub done: bool,
    /// Its xdg-output, which says where the compositor lays it out.
    pub xdg: ZxdgOutputV1,
    /// Where it is in the layout and how big, in the layout's units, as
    /// xdg-output has it: wlroots' own box for it, as of the last
    /// `wl_output.done`, which is what makes its events one change.
    pub logical: Option<Rect>,
    /// xdg-output's position and size as they arrive, until the `done`.
    pub pending_position: Option<(i32, i32)>,
    pub pending_size: Option<(i32, i32)>,
}

impl OutputInfo {
    /// The `wl_output.done`: what xdg-output said since the last takes effect,
    /// together, over the box as it was.
    fn settle_logical(&mut self) {
        let (position, size) = (self.pending_position.take(), self.pending_size.take());
        let was = self.logical;
        if let (Some((x, y)), Some((width, height))) = (position.or(was.map(|r| (r.x, r.y))), size.or(was.map(|r| (r.width, r.height)))) {
            self.logical = Some(Rect { x, y, width, height });
        }
    }

    /// The framebuffer size a capture of this output has.
    pub fn size(&self) -> (u16, u16) {
        let (w, h) = self.mode;
        let (w, h) = match self.transform {
            wl_output::Transform::_90 | wl_output::Transform::_270 | wl_output::Transform::Flipped90 | wl_output::Transform::Flipped270 => (h, w),
            _ => (w, h),
        };
        (w.clamp(0, i32::from(u16::MAX)) as u16, h.clamp(0, i32::from(u16::MAX)) as u16)
    }

    pub fn is_headless(&self) -> bool {
        self.name.as_deref().is_some_and(|n| n.starts_with("HEADLESS-"))
    }
}

pub struct Head {
    pub head: ZwlrOutputHeadV1,
    pub name: Option<String>,
    pub enabled: bool,
    pub scale: Option<f64>,
    pub current_mode: Option<ObjectId>,
    scale_changed: bool,
    /// The scale the daemon's last configuration that succeeded set this head
    /// to. A head whose scale is still within the 1/256 the protocol carries
    /// of it, and whose box is the one it lays the pixels out as, is at a
    /// scale that is the daemon's own where the compositor holds what it is
    /// set ([`held_as_set`], [`Placement::exact`]). Gone with a scale the
    /// compositor announces for the head while no configuration of the
    /// daemon's that sets its scale is under way ([`Head::settings`]):
    /// another client's, or a reload's, which may read the same to 1/256 and
    /// lay out the same box and not be the same, as a configured 1.7 does
    /// beside the daemon's 1.69921875 on 1920 pixels.
    set: Option<f64>,
    /// How many configurations of the daemon's that set this head's scale are
    /// under way, their changes possibly yet to arrive: a scale the head
    /// announces while there is none is somebody else's ([`Head::set`]).
    settings: u32,
}

#[derive(Default)]
pub struct ModeInfo {
    pub size: (i32, i32),
    pub refresh: i32,
}

/// What a configuration was for, so its outcome can be reported — and what it
/// asked, so one the compositor cancelled can be asked again.
///
/// A configuration is made against the layout the compositor last announced,
/// by serial, and one applied after the layout changed — an output added or
/// taken away, which a client's virtual displays do ([`crate::sway`]), or
/// configured by someone else — is `cancelled` rather than applied. The
/// request still stands, so it is made once more against the new layout, once:
/// `retry` marks that one, and a second cancellation is a refusal.
#[derive(Debug, Clone, Copy)]
pub enum ConfigKind {
    /// A SetDesktopSize: the mode alone.
    Resize { client: ClientId, width: u16, height: u16, retry: bool },
    /// A density declaration: the mode and the scale together, whichever differ.
    /// `id` tells its events from a declaration's before it.
    Declare { id: u64, client: ClientId, width: u16, height: u16, scale: f64, retry: bool },
}

/// A configuration out: what it was for, and the head whose scale it set and
/// the scale, when it set one. Its success makes that scale the head's own
/// ([`Head::set`]) whatever its client has done since: chosen another output,
/// left, or been replaced on its desk.
pub struct Configuration {
    kind: ConfigKind,
    scaled: Option<(ZwlrOutputHeadV1, f64)>,
}

impl ConfigKind {
    /// Whether this configuration is already the one asked again.
    fn retried(&self) -> bool {
        match self {
            ConfigKind::Resize { retry, .. } | ConfigKind::Declare { retry, .. } => *retry,
        }
    }
}

/// A `wl_display.sync` after declaration `id`'s configuration succeeded: when it
/// comes back with that declaration still unsettled, no head scale change
/// arrived — the compositor changed the mode alone, or nothing — and the
/// declaration still needs its answer.
pub struct ScaleSettle {
    id: u64,
    /// The head whose scale the configuration set, counted in its
    /// [`Head::settings`] until this comes back.
    scaled: Option<ZwlrOutputHeadV1>,
}

#[derive(Default)]
pub struct Outputs {
    pub outputs: Vec<OutputInfo>,
    pub heads: Vec<Head>,
    pub modes: HashMap<ObjectId, ModeInfo>,
    pub manager: Option<ZwlrOutputManagerV1>,
    pub serial: u32,
    /// The name of each desk's shared output, once chosen.
    pub selected: [Option<String>; 2],
    /// The output the configuration names. The desktop is on it whenever the
    /// compositor has it and the client has not named another: it is taken at
    /// the start when it is there, and each time it appears.
    pub wanted: Option<String>,
    /// The declaration whose configuration is out and not settled yet, and the
    /// desk it is for. One at a time, whichever desk asked: its settling is
    /// told apart from nothing else, and a configuration names every head.
    pub declaring: Option<(u64, usize)>,
    /// The id the next declaration takes.
    pub next_declaration: u64,
}

impl Outputs {
    pub fn selected(&self, desk: usize) -> Option<&OutputInfo> {
        let name = self.selected[desk].as_deref()?;
        self.outputs.iter().find(|o| o.name.as_deref() == Some(name))
    }

    pub fn selected_head(&self, desk: usize) -> Option<&Head> {
        let name = self.selected[desk].as_deref()?;
        self.head(name)
    }

    fn head(&self, name: &str) -> Option<&Head> {
        self.heads.iter().find(|h| h.name.as_deref() == Some(name))
    }

    /// The desks sharing the output of this name.
    pub fn desks_on(&self, name: Option<&str>) -> Vec<usize> {
        (0..self.selected.len()).filter(|&desk| name.is_some() && self.selected[desk].as_deref() == name).collect()
    }

    /// The exact scale of a desk's shared output: the head's, or `wl_output`'s.
    pub fn scale(&self, desk: usize) -> f64 {
        self.selected(desk).map_or(1.0, |o| self.scale_of(o))
    }

    /// An output as a pointer's motion meets it ([`Placement`]), once
    /// xdg-output has said where it is laid out, with the whole layout's box.
    pub fn placement_of(&self, output: &OutputInfo) -> Option<Placement> {
        let (logical, size, scale) = (output.logical?, output.size(), self.scale_of(output));
        let exact = self.set_scale(output).is_some_and(held_as_set) && lays_out(size, scale) == (logical.width, logical.height);
        Some(Placement { logical, size, transform: output.transform, scale, exact, layout: self.layout()? })
    }

    /// The scale the daemon set `output` to, while its head's is still within
    /// the 1/256 the protocol carries of it ([`Head::set`]).
    pub fn set_scale(&self, output: &OutputInfo) -> Option<f64> {
        let head = output.name.as_deref().and_then(|name| self.head(name))?;
        head.scale.zip(head.set).filter(|(scale, set)| (scale - set).abs() < 1.0 / 256.0).map(|(_, set)| set)
    }

    /// The layout's box: the union of every output's, as wlroots has it
    /// (`wlr_output_layout_get_box`), or `None` before any output has one.
    pub fn layout(&self) -> Option<Rect> {
        self.outputs.iter().filter_map(|o| o.logical).reduce(|a, b| {
            let (x, y) = (a.x.min(b.x), a.y.min(b.y));
            Rect { x, y, width: (a.x + a.width).max(b.x + b.width) - x, height: (a.y + a.height).max(b.y + b.height) - y }
        })
    }

    /// The scale `output` is drawn at: its head's, or `wl_output`'s.
    pub fn scale_of(&self, output: &OutputInfo) -> f64 {
        self.head_scale(output).unwrap_or(f64::from(output.wl_scale.max(1)))
    }

    /// The exact scale wlr-output-management gives `output`'s head, if it has
    /// one.
    fn head_scale(&self, output: &OutputInfo) -> Option<f64> {
        self.head(output.name.as_deref()?)?.scale
    }

    /// A desk's shared output's size in pixels as the head reports it, falling
    /// back to `wl_output`'s mode.
    pub fn size(&self, desk: usize) -> (u16, u16) {
        self.selected(desk).map_or((0, 0), |o| self.size_of(o))
    }

    /// The framebuffer size a capture of `output` has, as its head reports the
    /// mode, falling back to `wl_output`'s.
    pub fn size_of(&self, output: &OutputInfo) -> (u16, u16) {
        let mode = output
            .name
            .as_deref()
            .and_then(|name| self.head(name))
            .and_then(|h| h.current_mode.as_ref())
            .and_then(|m| self.modes.get(m));
        let Some(mode) = mode else { return output.size() };
        let (w, h) = mode.size;
        let (w, h) = match output.transform {
            wl_output::Transform::_90 | wl_output::Transform::_270 | wl_output::Transform::Flipped90 | wl_output::Transform::Flipped270 => (h, w),
            _ => (w, h),
        };
        (w.clamp(0, i32::from(u16::MAX)) as u16, h.clamp(0, i32::from(u16::MAX)) as u16)
    }

    /// A desk's shared output's id, or 0 while there is none: the `wl_output`
    /// global, which is unique for as long as the output exists and is what a
    /// client names in a `SelectOutput`.
    pub fn active_id(&self, desk: usize) -> u32 {
        self.selected(desk).map_or(0, |o| o.global)
    }

    /// The outputs a client may choose between, by name so the order a menu
    /// shows them in does not depend on the order the compositor announced
    /// them. An output whose name or mode has not arrived yet is not in the
    /// list: it cannot be labelled, and capturing it would produce nothing.
    pub fn entries(&self) -> Vec<OutputEntry> {
        let mut entries: Vec<OutputEntry> = self
            .outputs
            .iter()
            .filter(|o| self.listable(o))
            .map(|o| {
                let (width, height) = self.size_of(o);
                OutputEntry {
                    id: o.global,
                    name: o.name.clone().expect("listable"),
                    width,
                    height,
                    scale: self.scale_of(o),
                    headless: o.is_headless(),
                }
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    /// Whether an output can be shared at all: its properties have arrived, it
    /// has a name to be labelled by, and a capture of it would produce pixels.
    /// The one rule behind both [`Outputs::entries`] and [`Outputs::selectable`],
    /// so what a client is offered and what it may ask for cannot drift apart.
    fn listable(&self, output: &OutputInfo) -> bool {
        let (width, height) = self.size_of(output);
        output.done && output.name.is_some() && width > 0 && height > 0
    }

    /// The output with this id, while the compositor has one a client may share.
    /// An id that names an output still arriving, or one left without a mode, is
    /// no more selectable than an id the compositor never had: sharing it would
    /// put a desktop of no size in front of the client and give the capture
    /// nothing to read.
    pub fn selectable(&self, id: u32) -> Option<&OutputInfo> {
        self.outputs.iter().find(|o| o.global == id).filter(|o| self.listable(o))
    }

    /// Whether this is the output the configuration names.
    pub fn is_wanted(&self, name: Option<&str>) -> bool {
        name.is_some() && self.wanted.as_deref() == name
    }

    /// Pick the shared output: the configured name, or the first one — also
    /// when the compositor has none of that name, which is an output not
    /// enabled yet rather than a mistake to stop on.
    pub fn select(&mut self, wanted: Option<&str>) -> anyhow::Result<()> {
        self.wanted = wanted.map(str::to_owned);
        let named = wanted.and_then(|name| self.outputs.iter().find(|o| o.name.as_deref() == Some(name)));
        if let (Some(name), None) = (wanted, named) {
            let have: Vec<_> = self.outputs.iter().filter_map(|o| o.name.clone()).collect();
            info!("no output named {name} yet; the compositor has {have:?}, and the first is shared until it appears");
        }
        let chosen = match named {
            Some(output) => output,
            // The first with a name: one without cannot be shared, and being
            // listed first should not keep the daemon off one that can.
            None => self
                .outputs
                .iter()
                .find(|o| o.name.is_some())
                .or(self.outputs.first())
                .ok_or_else(|| anyhow::anyhow!("the compositor has no outputs"))?,
        };
        let name = chosen.name.clone().ok_or_else(|| anyhow::anyhow!("the output has no name; wl_output version 4 is required"))?;
        info!(
            "sharing output {name}: {}x{} pixels at scale {:.2}{}",
            chosen.size().0,
            chosen.size().1,
            self.heads.iter().find(|h| h.name.as_deref() == Some(&name)).and_then(|h| h.scale).unwrap_or(f64::from(chosen.wl_scale)),
            if chosen.is_headless() { ", headless" } else { "" }
        );
        self.selected[crate::shared::FIRST] = Some(name);
        Ok(())
    }

    /// Apply a configuration that changes a desk's shared output's mode, when
    /// `size` is given, and its scale, when `scale` is, leaving every other
    /// property of every head as the compositor has it. `false` when nothing was
    /// sent.
    pub fn configure(
        &mut self,
        qh: &QueueHandle<Compositor>,
        desk: usize,
        size: Option<(u16, u16)>,
        scale: Option<f64>,
        kind: ConfigKind,
    ) -> bool {
        let Some(manager) = &self.manager else {
            info!("wlr-output-management is not available; not reconfiguring the output");
            return false;
        };
        let Some(selected) = self.selected(desk) else { return false };
        if !selected.is_headless() {
            info!("not reconfiguring {}: not a headless output", selected.name.as_deref().unwrap_or("?"));
            return false;
        }
        let name = selected.name.clone();
        let refresh = self
            .selected_head(desk)
            .and_then(|h| h.current_mode.as_ref())
            .and_then(|m| self.modes.get(m))
            .map_or(0, |m| m.refresh);

        let scaled = scale.zip(self.selected_head(desk)).map(|(s, h)| (h.head.clone(), s));
        let scaled_head = scaled.as_ref().map(|(head, _)| head.clone());
        let config = manager.create_configuration(self.serial, qh, Configuration { kind, scaled });
        if let ConfigKind::Declare { id, .. } = kind {
            self.declaring = Some((id, desk));
        }
        for head in &self.heads {
            if !head.enabled {
                config.disable_head(&head.head);
                continue;
            }
            let ch = config.enable_head(&head.head, qh, ());
            if head.name == name {
                if let Some((w, h)) = size {
                    debug!("asking for a {w}x{h} mode at {refresh} mHz");
                    ch.set_custom_mode(i32::from(w), i32::from(h), refresh);
                    // Rotation makes no sense on a headless output.
                    ch.set_transform(wl_output::Transform::Normal);
                }
                if let Some(s) = scale {
                    debug!("asking for scale {s:.2}");
                    ch.set_scale(s);
                }
            }
        }
        config.apply();
        if let Some(head) = self.heads.iter_mut().find(|h| Some(&h.head) == scaled_head.as_ref()) {
            head.settings += 1;
        }
        true
    }

    /// Whether `configure` can reconfigure the output `desk` shares: the
    /// compositor offers wlr-output-management and the output is headless.
    pub fn configurable(&self, desk: usize) -> bool {
        self.manager.is_some() && self.selected(desk).is_some_and(OutputInfo::is_headless)
    }

    /// A configuration of the daemon's that set `head`'s scale succeeded: the
    /// scale is the daemon's own ([`Head::set`]).
    fn scale_set(&mut self, head: &ZwlrOutputHeadV1, scale: f64) {
        if let Some(head) = self.heads.iter_mut().find(|h| h.head == *head) {
            head.set = Some(scale);
        }
    }

    /// A configuration of the daemon's that set `head`'s scale is over, its
    /// changes arrived.
    fn scale_setting_over(&mut self, head: &ZwlrOutputHeadV1) {
        if let Some(head) = self.heads.iter_mut().find(|h| h.head == *head) {
            head.settings = head.settings.saturating_sub(1);
        }
    }
}

/// The box `pixels` are laid out as at `scale`, as wlroots has it
/// (`wlr_output_effective_resolution`): the pixels over the scale, truncated.
fn lays_out(pixels: (u16, u16), scale: f64) -> (i32, i32) {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    ((f64::from(pixels.0) / scale).trunc() as i32, (f64::from(pixels.1) / scale).trunc() as i32)
}

/// Whether a compositor set to `scale` holds it as it is: a multiple of an
/// eighth. The protocol carries a scale to 1/256, and Sway rounds what it is
/// set to the 1/120 fractional-scale-v1 carries (`round(scale * 120) / 120`
/// in its `output.c`), so 1.7 asked is 1.69921875 on the wire and 1.7 applied,
/// where labwc applies the 1.69921875; what both hold is what is whole in
/// both, every eighth, which the densities a browser declares mostly are.
fn held_as_set(scale: f64) -> bool {
    (scale * 8.0).fract() == 0.0
}

#[cfg(test)]
mod tests {
    use super::{held_as_set, lays_out};

    #[test]
    fn a_scale_is_held_as_set_where_it_is_whole_in_both_the_wires_and_sways_steps() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 2.75, 3.0] {
            assert!(held_as_set(scale), "{scale}");
        }
        for scale in [1.1, 1.2, 1.3, 1.7, 1.69921875, 1.09765625] {
            assert!(!held_as_set(scale), "{scale}");
        }
    }

    #[test]
    fn an_output_is_laid_out_as_its_pixels_over_its_scale_truncated() {
        assert_eq!(lays_out((1920, 1080), 1.7), (1129, 635));
        assert_eq!(lays_out((1920, 1080), 1.69921875), (1129, 635));
        assert_eq!(lays_out((1280, 720), 1.1), (1163, 654));
        assert_eq!(lays_out((1280, 720), 1.09765625), (1166, 655));
        assert_eq!(lays_out((3456, 1802), 2.75), (1256, 655));
        assert_eq!(lays_out((1920, 1080), 0.0), (1920, 1080));
    }
}

impl Dispatch<WlOutput, ()> for Compositor {
    fn event(state: &mut Self, output: &WlOutput, event: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some(info) = state.outputs.outputs.iter_mut().find(|o| o.output == *output) else { return };
        match event {
            wl_output::Event::Geometry { transform: WEnum::Value(t), .. } => info.transform = t,
            wl_output::Event::Mode { flags, width, height, .. } => {
                if flags.into_result().is_ok_and(|f| f.contains(wl_output::Mode::Current)) {
                    info.mode = (width, height);
                }
            }
            wl_output::Event::Scale { factor } => info.wl_scale = factor,
            wl_output::Event::Name { name } => info.name = Some(name),
            wl_output::Event::Done => {
                info.settle_logical();
                // The first `done` comes before xdg-output's, which follows
                // `get_xdg_output`: the output is here once it has its box.
                let appeared = info.logical.is_some() && !std::mem::replace(&mut info.done, true);
                let name = info.name.clone();
                for desk in state.outputs.desks_on(name.as_deref()) {
                    state.geometry_changed(desk);
                }
                // Moved before the list is published, never after: a session
                // waiting for the configured output reads it listed and not
                // shared as a client's choice to be elsewhere.
                if state.outputs.selected[FIRST].is_none() {
                    // Nothing is shared, and this output has just become
                    // something that can be: the desktop starts again on it.
                    state.adopt_output();
                } else if appeared && state.outputs.is_wanted(name.as_deref()) && state.outputs.selected[FIRST] != name {
                    // The configured output is here, and the desktop was on
                    // another only for want of it. On its appearing and not on
                    // every `done`: a client that names another afterwards
                    // stays where it asked to be.
                    state.adopt_output();
                }
                state.outputs_changed();
                // Its box, mode or transform may have changed, and with its
                // box the layout's.
                state.reaim();
                // A made output is here: the next step towards the client's
                // virtual displays, if there is one.
                state.reconcile_virtual_displays();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, u32> for Compositor {
    fn event(state: &mut Self, _: &ZxdgOutputV1, event: zxdg_output_v1::Event, global: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(info) = state.outputs.outputs.iter_mut().find(|o| o.global == *global) else { return };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => info.pending_position = Some((x, y)),
            zxdg_output_v1::Event::LogicalSize { width, height } => info.pending_size = Some((width, height)),
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Compositor: ignore ZxdgOutputManagerV1);

impl Dispatch<ZwlrOutputManagerV1, ()> for Compositor {
    fn event(state: &mut Self, _: &ZwlrOutputManagerV1, event: zwlr_output_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state.outputs.heads.push(Head { head, name: None, enabled: false, scale: None, current_mode: None, scale_changed: false, set: None, settings: 0 });
            }
            zwlr_output_manager_v1::Event::Done { serial } => {
                state.outputs.serial = serial;
                let mut changed = Vec::new();
                for head in &mut state.outputs.heads {
                    if std::mem::take(&mut head.scale_changed) {
                        changed.push(head.name.clone());
                    }
                }
                for name in changed {
                    for desk in state.outputs.desks_on(name.as_deref()) {
                        // The report this sends answers the declaration out
                        // for that desk, if any.
                        let settled = state.outputs.declaring.take_if(|(_, declaring)| *declaring == desk).is_some();
                        state.geometry_changed(desk);
                        if settled {
                            state.declaration_settled();
                        }
                    }
                }
                // A head's scale or mode is in every entry's label, not only the
                // shared one's.
                state.outputs_changed();
                // And in where a pixel is.
                state.reaim();
            }
            zwlr_output_manager_v1::Event::Finished => warn!("the compositor withdrew wlr-output-management"),
            _ => {}
        }
    }

    event_created_child!(Compositor, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for Compositor {
    fn event(state: &mut Self, head: &ZwlrOutputHeadV1, event: zwlr_output_head_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwlr_output_head_v1::Event::Mode { mode } = &event {
            state.outputs.modes.insert(mode.id(), ModeInfo::default());
        }
        let Some(h) = state.outputs.heads.iter_mut().find(|h| h.head == *head) else { return };
        match event {
            zwlr_output_head_v1::Event::Name { name } => h.name = Some(name),
            zwlr_output_head_v1::Event::Enabled { enabled } => h.enabled = enabled != 0,
            zwlr_output_head_v1::Event::Scale { scale } => {
                if h.scale != Some(scale) {
                    h.scale_changed = true;
                }
                h.scale = Some(scale);
                // Announced with no setting of the daemon's under way: set by
                // somebody else, whatever it reads ([`Head::set`]).
                if h.settings == 0 {
                    h.set = None;
                }
            }
            zwlr_output_head_v1::Event::CurrentMode { mode } => h.current_mode = Some(mode.id()),
            zwlr_output_head_v1::Event::Finished => {
                let name = h.name.clone();
                state.outputs.heads.retain(|h| h.head != *head);
                if head.version() >= 3 {
                    head.release();
                }
                debug!("head {} finished", name.unwrap_or_default());
            }
            _ => {}
        }
    }

    event_created_child!(Compositor, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for Compositor {
    fn event(state: &mut Self, mode: &ZwlrOutputModeV1, event: zwlr_output_mode_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_output_mode_v1::Event::Size { width, height } => {
                state.outputs.modes.entry(mode.id()).or_default().size = (width, height);
            }
            zwlr_output_mode_v1::Event::Refresh { refresh } => {
                state.outputs.modes.entry(mode.id()).or_default().refresh = refresh;
            }
            zwlr_output_mode_v1::Event::Finished => {
                state.outputs.modes.remove(&mode.id());
                if mode.version() >= 3 {
                    mode.release();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, Configuration> for Compositor {
    fn event(
        state: &mut Self,
        config: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        configuration: &Configuration,
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let kind = &configuration.kind;
        let scaled = configuration.scaled.as_ref().map(|(head, _)| head.clone());
        match event {
            zwlr_output_configuration_v1::Event::Succeeded => {
                debug!("output configuration succeeded ({kind:?})");
                if let Some((head, scale)) = &configuration.scaled {
                    state.outputs.scale_set(head, *scale);
                    // Sway sends the head's changes before `succeeded`, and a
                    // scale set again as it was sends none: the scale is only
                    // now known as the daemon's, which makes it exact.
                    state.reaim();
                }
                if let ConfigKind::Declare { id, .. } = *kind {
                    // `succeeded` says nothing about whether a head changed: the
                    // changes and their `done` follow when there are any. Sway, the
                    // measured compositor, sends them first when it commits on the
                    // spot. One round trip later, whatever the compositor was going
                    // to send has arrived, and a scale set is over.
                    if scaled.is_some() || state.outputs.declaring.is_some_and(|(declaring, _)| declaring == id) {
                        conn.display().sync(qh, ScaleSettle { id, scaled });
                    }
                }
            }
            zwlr_output_configuration_v1::Event::Cancelled if !kind.retried() => {
                // The layout changed under it: the `done` with the new serial
                // is already read, so the same request is made again against it.
                info!("the compositor cancelled an output configuration whose layout changed under it; asking again ({kind:?})");
                if let Some(head) = &scaled {
                    state.outputs.scale_setting_over(head);
                }
                match *kind {
                    ConfigKind::Resize { client, width, height, .. } => {
                        if let Some(desk) = state.desk_of(client) {
                            state.desks[desk].pending_resize = None;
                            state.resize(desk, client, width, height, true);
                        }
                    }
                    ConfigKind::Declare { id, client, width, height, scale, .. } => {
                        if let Some((_, desk)) = state.outputs.declaring.take_if(|(declaring, _)| *declaring == id) {
                            state.desks[desk].pending_resize = None;
                            if state.desks[desk].client == Some(client) {
                                state.declare(desk, client, width, height, scale, true);
                            }
                            // A newer declaration waiting follows the one asked
                            // again, or runs now when there is none to follow.
                            state.declaration_settled();
                        }
                    }
                }
            }
            zwlr_output_configuration_v1::Event::Failed | zwlr_output_configuration_v1::Event::Cancelled => {
                warn!("the compositor refused an output configuration ({kind:?})");
                if let Some(head) = &scaled {
                    state.outputs.scale_setting_over(head);
                }
                match *kind {
                    ConfigKind::Resize { client, .. } => {
                        if let Some(desk) = state.desk_of(client) {
                            state.desks[desk].pending_resize = None;
                        }
                        state.shared().emit(Event::ResizeRefused { client, status: crate::rfb::msg::EDS_STATUS_INVALID_LAYOUT });
                    }
                    ConfigKind::Declare { id, .. } => {
                        if let Some((_, desk)) = state.outputs.declaring.take_if(|(declaring, _)| *declaring == id) {
                            state.desks[desk].pending_resize = None;
                            state.answer_geometry(desk, None);
                            state.declaration_settled();
                        }
                    }
                }
            }
            _ => {}
        }
        config.destroy();
    }
}

impl Dispatch<WlCallback, ScaleSettle> for Compositor {
    fn event(state: &mut Self, _: &WlCallback, event: wl_callback::Event, settle: &ScaleSettle, _: &Connection, _: &QueueHandle<Self>) {
        let wl_callback::Event::Done { .. } = event else { return };
        if let Some(head) = &settle.scaled {
            state.outputs.scale_setting_over(head);
        }
        if let Some((_, desk)) = state.outputs.declaring.take_if(|(declaring, _)| *declaring == settle.id) {
            info!("the compositor applied the declaration's configuration without changing the scale; reporting the output as it is");
            state.answer_geometry(desk, None);
            state.declaration_settled();
        }
    }
}

wayland_client::delegate_noop!(Compositor: ignore ZwlrOutputConfigurationHeadV1);
