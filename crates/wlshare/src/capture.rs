//! Screen capture through wlr-screencopy into a shared-memory buffer, and from
//! there into the framebuffer.
//!
//! One frame is in flight at a time. `copy_with_damage` makes the compositor
//! answer only when something changed since the frame before, so an idle desktop
//! costs nothing and a busy one is paced by [`crate::config::Config::max_fps`].
//! Capture runs only while a client is connected.
//!
//! The exception is a framebuffer holding no pixels yet -- freshly made, resized,
//! or switched to another output. There is no frame before for damage to be
//! measured against, so the frame that fills it is asked for outright and taken
//! whole; waiting for damage there would hold a blank screen for as long as the
//! output happened to be still.

use std::os::fd::{AsFd, OwnedFd};
use std::time::{Duration, Instant};

use calloop::RegistrationToken;
use calloop::timer::{TimeoutAction, Timer};
use log::{debug, error, info, warn};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

use crate::compositor::Compositor;
use crate::framebuffer::{BGRX, FrameLayout, Rect, ResizeOrigin};

/// A `wl_shm` buffer the compositor copies a frame into.
struct ShmBuffer {
    _fd: OwnedFd,
    pool: WlShmPool,
    buffer: WlBuffer,
    map: memmap2::MmapMut,
    width: u32,
    height: u32,
    stride: u32,
    format: wl_shm::Format,
}

impl ShmBuffer {
    fn new(shm: &WlShm, qh: &QueueHandle<Compositor>, width: u32, height: u32, stride: u32, format: wl_shm::Format) -> anyhow::Result<Self> {
        let size = stride as usize * height as usize;
        let fd = rustix::fs::memfd_create("wlshare-frame", rustix::fs::MemfdFlags::CLOEXEC)?;
        rustix::fs::ftruncate(&fd, size as u64)?;
        // SAFETY: the mapping covers a memfd this process owns, sized just above;
        // the compositor writes it only between `copy` and `ready`, when this
        // side does not read it.
        let map = unsafe { memmap2::MmapMut::map_mut(&fd)? };
        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format, qh, ());
        Ok(Self { _fd: fd, pool, buffer, map, width, height, stride, format })
    }

    fn matches(&self, width: u32, height: u32, stride: u32, format: wl_shm::Format) -> bool {
        (self.width, self.height, self.stride, self.format) == (width, height, stride, format)
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
    }
}

#[derive(Default)]
pub struct Capture {
    pub manager: Option<ZwlrScreencopyManagerV1>,
    frame: Option<ZwlrScreencopyFrameV1>,
    buffer: Option<ShmBuffer>,
    /// The frame the compositor announced, before its buffer is ready.
    announced: Option<(u32, u32, u32, wl_shm::Format)>,
    damage: Vec<Rect>,
    /// How the frame in flight differs from the framebuffer's own layout, from
    /// the format announced for it and its y-invert flag.
    layout: FrameLayout,
    last_ready: Option<Instant>,
    timer: Option<RegistrationToken>,
    failures: u32,
}

impl Compositor {
    /// Begin capturing if a client wants frames and nothing is in flight.
    pub fn start_capture(&mut self) {
        if self.client.is_none() || self.capture.frame.is_some() {
            return;
        }
        let Some(manager) = &self.capture.manager else { return };
        let Some(output) = self.outputs.selected() else { return };
        self.capture.damage.clear();
        self.capture.layout = FrameLayout::default();
        self.capture.announced = None;
        // Keep the compositor's pointer out of the framebuffer. wlroots 0.19
        // gives the headless backend a cursor plane, so a screencopy without
        // overlay_cursor can finally leave it behind. The RFB session sends a
        // separate standard Cursor pseudo-rectangle, so the client moves the
        // pointer without waiting for a captured frame.
        let frame = manager.capture_output(0, &output.output, &self.qh, ());
        self.capture.frame = Some(frame);
    }

    /// Stop after the frame in flight, if any: nobody is watching.
    pub fn stop_capture(&mut self) {
        if let Some(token) = self.capture.timer.take() {
            self.handle.remove(token);
        }
        if let Some(frame) = self.capture.frame.take() {
            frame.destroy();
        }
        self.capture.buffer = None;
        self.capture.last_ready = None;
    }

    /// Capture again after `delay`, replacing any timer already set.
    fn capture_after(&mut self, delay: Duration) {
        if let Some(token) = self.capture.timer.take() {
            self.handle.remove(token);
        }
        let timer = if delay.is_zero() { Timer::immediate() } else { Timer::from_duration(delay) };
        match self.handle.insert_source(timer, |_, _, state: &mut Compositor| {
            state.capture.timer = None;
            state.start_capture();
            TimeoutAction::Drop
        }) {
            Ok(token) => self.capture.timer = Some(token),
            Err(e) => error!("cannot schedule a capture: {e}"),
        }
    }

    /// The delay that keeps captures under the configured frame rate.
    fn pace(&self) -> Duration {
        let interval = Duration::from_secs_f64(1.0 / f64::from(self.max_fps));
        match self.capture.last_ready {
            Some(at) => interval.saturating_sub(at.elapsed()),
            None => Duration::ZERO,
        }
    }

    fn frame_ready(&mut self) {
        let Some(buffer) = &self.capture.buffer else { return };
        let (width, height) = (buffer.width as u16, buffer.height as u16);
        let origin = match self.pending_resize {
            Some((client, w, h)) if (w, h) == (width, height) => {
                self.pending_resize = None;
                ResizeOrigin::Client(client)
            }
            _ => ResizeOrigin::Server,
        };
        let damage = std::mem::take(&mut self.capture.damage);
        let generation = {
            let mut fb = self.shared().framebuffer.lock().unwrap();
            if (fb.width, fb.height) != (width, height) {
                info!("the framebuffer is now {width}x{height} ({origin:?})");
                fb.resize(width, height, origin);
            }
            let damage = damage_for(damage, fb.painted, width, height);
            fb.apply(&buffer.map, buffer.stride as usize, &damage, self.capture.layout);
            fb.generation
        };
        self.shared().frame_changed(generation);
        self.capture.last_ready = Some(Instant::now());
        self.capture.failures = 0;
    }
}

/// What of a captured frame to copy in: the rectangles the compositor reported,
/// or the whole frame.
///
/// Damage is measured against the frame before, so it means nothing until there
/// has been one. A framebuffer that holds no pixels at its current size --
/// `painted` false, which is every framebuffer just made, just resized, or just
/// pointed at another output -- takes the frame whole; so does one whose frame
/// reported no damage at all, where everything may have changed.
fn damage_for(reported: Vec<Rect>, painted: bool, width: u16, height: u16) -> Vec<Rect> {
    if painted && !reported.is_empty() {
        return reported;
    }
    vec![Rect::whole(width, height)]
}

/// Where the framebuffer's `B, G, R, X` bytes sit in a pixel of `format`, or
/// `None` when this server cannot read it at all.
///
/// These are the eight 32-bit orders at eight bits a channel, which is every
/// format wlroots' screencopy can offer for one: GLES2 and Vulkan report
/// XRGB8888, ARGB8888, XBGR8888 or ABGR8888, and pixman additionally reports
/// the four with the unused byte first. Everything past this table -- 10-bit,
/// 16-bit, 565, 5551, and the packed 24-bit orders -- is a conversion rather
/// than a rearrangement, and none is reachable from a compositor an ordinary
/// desktop runs; see docs/architecture.md.
///
/// The DRM names read most significant byte first, so each one is its own
/// memory order reversed on a little-endian machine, which is the only kind
/// wl_shm describes.
fn channel_bytes(format: wl_shm::Format) -> Option<[u8; 4]> {
    match format {
        // In memory: B, G, R, X -- the framebuffer's own order.
        wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888 => Some(BGRX),
        // R, G, B, X
        wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888 => Some([2, 1, 0, 3]),
        // X, B, G, R
        wl_shm::Format::Rgbx8888 | wl_shm::Format::Rgba8888 => Some([1, 2, 3, 0]),
        // X, R, G, B
        wl_shm::Format::Bgrx8888 | wl_shm::Format::Bgra8888 => Some([3, 2, 1, 0]),
        _ => None,
    }
}

/// How much this server would rather have `format`: a straight copy over a
/// rearrangement over nothing it can use.
fn rank(format: wl_shm::Format) -> u8 {
    match channel_bytes(format) {
        Some(BGRX) => 2,
        Some(_) => 1,
        None => 0,
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for Compositor {
    fn event(state: &mut Self, frame: &ZwlrScreencopyFrameV1, event: zwlr_screencopy_frame_v1::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if state.capture.frame.as_ref() != Some(frame) {
            // A frame destroyed while its events were in flight.
            return;
        }
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer { format, width, height, stride } => {
                let WEnum::Value(format) = format else { return };
                // The compositor lists every format it can copy into, in no
                // order this server may rely on. Keep the best one seen so far,
                // and the first of them regardless -- an unusable format still
                // has to reach the error below, which names it.
                let better = state.capture.announced.is_none_or(|(.., current)| rank(format) > rank(current));
                if better {
                    state.capture.announced = Some((width, height, stride, format));
                }
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => {
                let Some((width, height, stride, format)) = state.capture.announced else {
                    warn!("the compositor offered no buffer format");
                    state.capture_failed(frame);
                    return;
                };
                let Some(bytes) = channel_bytes(format) else {
                    error!("the compositor offers frames only as {format:?}; this server needs a 32-bit format at eight bits a channel: XRGB8888, XBGR8888, RGBX8888, BGRX8888 or one of their alpha spellings");
                    state.capture_failed(frame);
                    return;
                };
                state.capture.layout.bytes = bytes;
                if !state.capture.buffer.as_ref().is_some_and(|b| b.matches(width, height, stride, format)) {
                    match ShmBuffer::new(&state.shm, qh, width, height, stride, format) {
                        Ok(b) => {
                            debug!("frame buffer {width}x{height}, stride {stride}, {format:?}");
                            state.capture.buffer = Some(b);
                        }
                        Err(e) => {
                            error!("cannot allocate a {width}x{height} frame buffer: {e}");
                            state.capture_failed(frame);
                            return;
                        }
                    }
                }
                // `copy_with_damage` waits for the output to change, which is
                // what keeps an idle desktop free -- but a blank framebuffer has
                // nothing to show in the meantime, and an output nobody is
                // touching can stay unchanged for minutes. So the frame that
                // fills a blank one is asked for outright.
                let painted = state.shared().framebuffer.lock().unwrap().painted;
                let buffer = state.capture.buffer.as_ref().unwrap();
                if painted {
                    frame.copy_with_damage(&buffer.buffer);
                } else {
                    debug!("asking for a whole frame: the framebuffer holds no pixels yet");
                    frame.copy(&buffer.buffer);
                }
            }
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                state.capture.layout.flipped = flags.into_result().is_ok_and(|f| f.contains(zwlr_screencopy_frame_v1::Flags::YInvert));
            }
            zwlr_screencopy_frame_v1::Event::Damage { x, y, width, height } => {
                state.capture.damage.push(Rect {
                    x: x.min(u32::from(u16::MAX)) as u16,
                    y: y.min(u32::from(u16::MAX)) as u16,
                    width: width.min(u32::from(u16::MAX)) as u16,
                    height: height.min(u32::from(u16::MAX)) as u16,
                });
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                frame.destroy();
                state.capture.frame = None;
                state.frame_ready();
                let delay = state.pace();
                state.capture_after(delay);
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                state.capture_failed(frame);
            }
            _ => {}
        }
    }
}

impl Compositor {
    fn capture_failed(&mut self, frame: &ZwlrScreencopyFrameV1) {
        frame.destroy();
        self.capture.frame = None;
        self.capture.failures += 1;
        // An output being reconfigured fails a frame or two; give up loudly only
        // when it keeps failing.
        let delay = Duration::from_millis(100 * u64::from(self.capture.failures.min(20)));
        if self.capture.failures == 20 {
            error!("screen capture keeps failing; retrying every {delay:?}");
        } else {
            debug!("screen capture failed; retrying in {delay:?}");
        }
        self.capture_after(delay);
    }
}

wayland_client::delegate_noop!(Compositor: ignore ZwlrScreencopyManagerV1);
wayland_client::delegate_noop!(Compositor: ignore WlShm);
wayland_client::delegate_noop!(Compositor: ignore WlShmPool);
wayland_client::delegate_noop!(Compositor: ignore WlBuffer);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_framebuffer_with_no_pixels_yet_takes_the_whole_frame() {
        let spot = vec![Rect { x: 4, y: 4, width: 8, height: 8 }];
        let whole = vec![Rect::whole(1280, 800)];
        // The frame that fills a blank framebuffer -- after a resize, or after a
        // switch to another output, where the size may not have changed at all.
        assert_eq!(damage_for(spot.clone(), false, 1280, 800), whole);
        // Once there are pixels to keep, only what the compositor reported is
        // copied over them.
        assert_eq!(damage_for(spot.clone(), true, 1280, 800), spot);
        // A frame that reported nothing says nothing about what is still good.
        assert_eq!(damage_for(Vec::new(), true, 1280, 800), whole);
    }
}
