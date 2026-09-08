//! The compositor's outputs: which one is shared, how big it is in pixels, and
//! the exact scale it is drawn at.
//!
//! Two protocols describe an output. `wl_output` gives its name, pixel mode and
//! an integer scale (wlroots reports the ceiling of a fractional one).
//! wlr-output-management gives the exact scale as the compositor has it, and is
//! the only way to *change* anything: a custom mode for a client's resize, a
//! scale for a client's density. Heads are matched to outputs by name.
//!
//! Only a headless output is ever reconfigured, the way wayvnc has it: a real
//! monitor's mode belongs to the person sitting at it.

use std::collections::HashMap;

use log::{debug, info, warn};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, event_created_child};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

use crate::compositor::Compositor;
use crate::shared::{ClientId, Event};

pub struct OutputInfo {
    pub output: WlOutput,
    pub global: u32,
    pub name: Option<String>,
    /// The current mode, in pixels, before the transform.
    pub mode: (i32, i32),
    pub transform: wl_output::Transform,
    pub wl_scale: i32,
    pub done: bool,
}

impl OutputInfo {
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
}

#[derive(Default)]
pub struct ModeInfo {
    pub size: (i32, i32),
    pub refresh: i32,
}

/// What a configuration was for, so its outcome can be reported.
#[derive(Debug, Clone, Copy)]
pub enum ConfigKind {
    Resize { client: ClientId },
    Scale,
}

/// A `wl_display.sync` after a scale configuration succeeded: when it comes back
/// with no head scale change seen, the compositor accepted the configuration and
/// left the scale as it was, and the declaration still needs its answer.
pub struct ScaleSettle;

#[derive(Default)]
pub struct Outputs {
    pub outputs: Vec<OutputInfo>,
    pub heads: Vec<Head>,
    pub modes: HashMap<ObjectId, ModeInfo>,
    pub manager: Option<ZwlrOutputManagerV1>,
    pub serial: u32,
    /// The name of the shared output, once chosen.
    pub selected: Option<String>,
    /// A scale configuration is out and the compositor has not settled it.
    pub scale_pending: bool,
    scale_changed_since_apply: bool,
}

impl Outputs {
    pub fn selected(&self) -> Option<&OutputInfo> {
        let name = self.selected.as_deref()?;
        self.outputs.iter().find(|o| o.name.as_deref() == Some(name))
    }

    pub fn selected_head(&self) -> Option<&Head> {
        let name = self.selected.as_deref()?;
        self.heads.iter().find(|h| h.name.as_deref() == Some(name))
    }

    /// The exact scale of the shared output: the head's, or `wl_output`'s.
    pub fn scale(&self) -> f64 {
        if let Some(s) = self.selected_head().and_then(|h| h.scale) {
            return s;
        }
        self.selected().map_or(1.0, |o| f64::from(o.wl_scale.max(1)))
    }

    /// The shared output's size in pixels as the head reports it, falling back
    /// to `wl_output`'s mode.
    pub fn size(&self) -> (u16, u16) {
        if let Some(mode) = self.selected_head().and_then(|h| h.current_mode.as_ref()).and_then(|m| self.modes.get(m)) {
            let (w, h) = mode.size;
            let transform = self.selected().map_or(wl_output::Transform::Normal, |o| o.transform);
            let (w, h) = match transform {
                wl_output::Transform::_90 | wl_output::Transform::_270 | wl_output::Transform::Flipped90 | wl_output::Transform::Flipped270 => (h, w),
                _ => (w, h),
            };
            return (w.clamp(0, i32::from(u16::MAX)) as u16, h.clamp(0, i32::from(u16::MAX)) as u16);
        }
        self.selected().map_or((0, 0), |o| o.size())
    }

    /// Pick the shared output: the configured name, or the first one.
    pub fn select(&mut self, wanted: Option<&str>) -> anyhow::Result<()> {
        let chosen = match wanted {
            Some(name) => self
                .outputs
                .iter()
                .find(|o| o.name.as_deref() == Some(name))
                .ok_or_else(|| {
                    let have: Vec<_> = self.outputs.iter().filter_map(|o| o.name.clone()).collect();
                    anyhow::anyhow!("no output named {name}; the compositor has {have:?}")
                })?,
            None => self.outputs.first().ok_or_else(|| anyhow::anyhow!("the compositor has no outputs"))?,
        };
        let name = chosen.name.clone().ok_or_else(|| anyhow::anyhow!("the output has no name; wl_output version 4 is required"))?;
        info!(
            "sharing output {name}: {}x{} pixels at scale {:.2}{}",
            chosen.size().0,
            chosen.size().1,
            self.heads.iter().find(|h| h.name.as_deref() == Some(&name)).and_then(|h| h.scale).unwrap_or(f64::from(chosen.wl_scale)),
            if chosen.is_headless() { ", headless" } else { "" }
        );
        self.selected = Some(name);
        Ok(())
    }

    /// Apply a configuration that changes the shared output's mode, when `size`
    /// is given, and its scale, when `scale` is, leaving every other property of
    /// every head as the compositor has it. `false` when nothing was sent.
    pub fn configure(
        &mut self,
        qh: &QueueHandle<Compositor>,
        size: Option<(u16, u16)>,
        scale: Option<f64>,
        kind: ConfigKind,
    ) -> bool {
        let Some(manager) = &self.manager else {
            info!("wlr-output-management is not available; not reconfiguring the output");
            return false;
        };
        let Some(selected) = self.selected() else { return false };
        if !selected.is_headless() {
            info!("not reconfiguring {}: not a headless output", selected.name.as_deref().unwrap_or("?"));
            return false;
        }
        let name = selected.name.clone();
        let refresh = self
            .selected_head()
            .and_then(|h| h.current_mode.as_ref())
            .and_then(|m| self.modes.get(m))
            .map_or(0, |m| m.refresh);

        let config = manager.create_configuration(self.serial, qh, kind);
        if matches!(kind, ConfigKind::Scale) {
            self.scale_pending = true;
            self.scale_changed_since_apply = false;
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
        true
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
                info.done = true;
                let selected = state.outputs.selected.as_deref() == info.name.as_deref();
                if selected {
                    state.geometry_changed();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Compositor {
    fn event(state: &mut Self, _: &ZwlrOutputManagerV1, event: zwlr_output_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state.outputs.heads.push(Head { head, name: None, enabled: false, scale: None, current_mode: None, scale_changed: false });
            }
            zwlr_output_manager_v1::Event::Done { serial } => {
                state.outputs.serial = serial;
                let selected = state.outputs.selected.clone();
                let mut changed = false;
                for head in &mut state.outputs.heads {
                    if head.scale_changed {
                        head.scale_changed = false;
                        if head.name == selected {
                            changed = true;
                        }
                    }
                }
                if changed {
                    if state.outputs.scale_pending {
                        state.outputs.scale_changed_since_apply = true;
                        state.outputs.scale_pending = false;
                    }
                    state.geometry_changed();
                }
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

impl Dispatch<ZwlrOutputConfigurationV1, ConfigKind> for Compositor {
    fn event(
        state: &mut Self,
        config: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        kind: &ConfigKind,
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_configuration_v1::Event::Succeeded => {
                debug!("output configuration succeeded ({kind:?})");
                if let ConfigKind::Scale = kind {
                    // `succeeded` says nothing about whether a head changed: the
                    // changes and their `done` follow when there are any. Sway, the
                    // measured compositor, sends them first when it commits on the
                    // spot. One round trip later, whatever the compositor was going
                    // to send has arrived.
                    if state.outputs.scale_pending && !state.outputs.scale_changed_since_apply {
                        conn.display().sync(qh, ScaleSettle);
                    }
                }
            }
            zwlr_output_configuration_v1::Event::Failed | zwlr_output_configuration_v1::Event::Cancelled => {
                warn!("the compositor refused an output configuration ({kind:?})");
                match kind {
                    ConfigKind::Resize { client } => {
                        state.pending_resize = None;
                        state.shared().emit(Event::ResizeRefused { client: *client, status: wlshare_rfb::msg::EDS_STATUS_INVALID_LAYOUT });
                    }
                    ConfigKind::Scale => {
                        state.outputs.scale_pending = false;
                        state.answer_geometry(None);
                    }
                }
            }
            _ => {}
        }
        config.destroy();
    }
}

impl Dispatch<WlCallback, ScaleSettle> for Compositor {
    fn event(state: &mut Self, _: &WlCallback, event: wl_callback::Event, _: &ScaleSettle, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = event
            && state.outputs.scale_pending
        {
            state.outputs.scale_pending = false;
            if !state.outputs.scale_changed_since_apply {
                info!("the compositor applied the scale configuration without changing the scale; reporting it as it is");
                state.answer_geometry(None);
            }
        }
    }
}

wayland_client::delegate_noop!(Compositor: ignore ZwlrOutputConfigurationHeadV1);
