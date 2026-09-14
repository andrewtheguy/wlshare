//! The client's camera, lent to the desktop: a PipeWire `Video/Source` node the
//! desktop's applications open like any other camera, fed from the H.264 the
//! client sends over the camera extension ([`wlshare_rfb::camera`]).
//!
//! wlshare runs inside the user's session, where the video graph is, so the
//! camera needs no kernel module and no privilege: it is a node in the same
//! PipeWire graph a browser's camera portal, OBS or GStreamer's `pipewiresrc`
//! already enumerate. It exists from the client's plug to its unplug or its
//! leaving, and offers exactly one format — planar 4:2:0 at the geometry and rate
//! the client plugged with — because that is what one decoder behind it makes.
//! An application that wants anything else converts, or does not open it.
//!
//! Whether frames flow is the desktop's decision. The stream goes to *streaming*
//! when an application links to the node and back when the last one leaves; each
//! transition is a [`Signal`] the session sends on, so the client encodes only
//! while something is watching. The node is its own driver: nothing in the graph
//! has a clock that knows when the next camera frame is due, so each decoded
//! picture triggers the graph cycle that delivers it.
//!
//! Like the audio capture ([`crate::audio`]), each camera is a thread running
//! PipeWire's loop, and the stream's callbacks run on that loop rather than on
//! the graph's real-time thread: decoding and copying a picture has no business
//! there. Samples reach the thread through a queue a few deep. A session whose
//! samples arrive faster than the thread decodes them drops them rather than
//! falling behind the camera, and a dropped sample is a gap H.264 cannot decode
//! across, so everything after it is dropped too until a keyframe comes — which
//! is asked of the client once per gap. The same goes for a unit the decoder
//! refuses, and for the start of every stream.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context as _;
use log::{debug, info, warn};
use pipewire as pw;
use pw::spa;
use tokio::sync::mpsc;
use wlshare_rfb::camera::CameraFormat;

use crate::decode::{Decoder, Picture};
use crate::shared::ClientId;

/// How long PipeWire may take to take the node before a plug is refused.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// Samples waiting for the camera thread. Half a second at the rates a client
/// sends; a thread further behind than that is showing a picture already late.
const QUEUE_DEPTH: usize = 8;

/// A decision of the desktop's, for the session to send the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// An application opened the camera: send samples, from a keyframe.
    Start,
    /// The last application closed it: send none.
    Stop,
    /// Samples were lost or refused: the next one must be a keyframe.
    Keyframe,
}

/// One plugged camera: the thread running its node, and the session's end of
/// the traffic in each direction.
pub struct Camera {
    /// The format plugged, which is the one the node offers.
    pub format: CameraFormat,
    commands: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
    signals: mpsc::UnboundedReceiver<Signal>,
    /// The session's own keyframe requests, for the samples it drops before
    /// they reach the thread.
    signal_tx: mpsc::UnboundedSender<Signal>,
    /// Samples sent to the thread and not yet taken by it.
    queued: Arc<AtomicUsize>,
    /// Whether samples are being dropped until a keyframe.
    gap: bool,
}

enum Command {
    Sample { unit: Vec<u8>, keyframe: bool },
    Quit,
}

impl Camera {
    /// Make the node for `client`'s camera in `format`. Blocks until PipeWire
    /// has taken it or refused it, so it belongs on a blocking thread.
    pub fn plug(client: ClientId, format: CameraFormat) -> anyhow::Result<Self> {
        let (commands, commands_rx) = pw::channel::channel();
        let (signal_tx, signals) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (thread_signals, thread_queued) = (signal_tx.clone(), queued.clone());
        let thread = std::thread::Builder::new()
            .name(format!("camera-{}", client.0))
            .spawn(move || run(client, format, commands_rx, thread_signals, thread_queued, ready_tx))
            .context("spawning the camera thread")?;
        let mut camera = Self { format, commands, thread: Some(thread), signals, signal_tx, queued, gap: false };
        match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(camera),
            Ok(Err(e)) => {
                // The thread has already returned.
                camera.thread.take().map(JoinHandle::join);
                Err(e)
            }
            Err(_) => {
                // Stuck coming up: ask it to quit and let it go rather than
                // wait for a loop that may never run.
                let _ = camera.commands.send(Command::Quit);
                camera.thread.take();
                anyhow::bail!("PipeWire did not take the camera within {START_TIMEOUT:?}")
            }
        }
    }

    /// Hand one access unit to the camera thread, or drop it: a full queue
    /// opens a gap, reported once, and only a keyframe that finds room closes it.
    pub fn sample(&mut self, unit: Vec<u8>, keyframe: bool) {
        if !keyframe && self.gap {
            return;
        }
        if self.queued.load(Ordering::Relaxed) >= QUEUE_DEPTH {
            // A keyframe lost to the queue is owed again, or the gap never closes.
            if keyframe || !self.gap {
                let _ = self.signal_tx.send(Signal::Keyframe);
            }
            self.gap = true;
            return;
        }
        self.gap = false;
        self.queued.fetch_add(1, Ordering::Relaxed);
        if self.commands.send(Command::Sample { unit, keyframe }).is_err() {
            self.queued.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// The desktop's next decision. Never resolves while there is none.
    pub async fn signal(&mut self) -> Signal {
        match self.signals.recv().await {
            Some(signal) => signal,
            // The camera holds a sender itself, so the channel cannot close.
            None => std::future::pending().await,
        }
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the loop's callbacks share: the decoder, the latest picture, and where
/// the stream stands.
struct Feed {
    client: ClientId,
    decoder: Decoder,
    /// The picture the next graph cycle delivers, planes back to back.
    picture: Vec<u8>,
    /// What the decoder writes into, swapped with `picture` when it is whole.
    scratch: Vec<u8>,
    expected: Picture,
    streaming: bool,
    /// Units are skipped until a keyframe decodes.
    keyframe_owed: bool,
    /// A picture of the wrong size was reported already.
    size_reported: bool,
    signals: mpsc::UnboundedSender<Signal>,
}

impl Feed {
    /// Decode one unit. Returns whether a picture is ready for the graph.
    fn take(&mut self, unit: &[u8], keyframe: bool) -> bool {
        if self.keyframe_owed && !keyframe {
            return false;
        }
        match self.decoder.decode(unit, &mut self.scratch) {
            Ok(None) => false,
            Ok(Some(picture)) if picture != self.expected => {
                if !self.size_reported {
                    warn!(
                        "client {}: the camera sent {}x{} pictures after plugging {}x{}; dropping them",
                        self.client.0, picture.width, picture.height, self.expected.width, self.expected.height
                    );
                    self.size_reported = true;
                }
                false
            }
            Ok(Some(_)) => {
                self.keyframe_owed = false;
                std::mem::swap(&mut self.picture, &mut self.scratch);
                self.streaming
            }
            Err(e) => {
                debug!("client {}: camera: {e:#}; asking for a keyframe", self.client.0);
                self.keyframe_owed = true;
                let _ = self.signals.send(Signal::Keyframe);
                false
            }
        }
    }
}

/// The camera thread: PipeWire's loop, from the node's creation to quit.
/// `ready` is told once, when the stream is connected or when that failed.
fn run(
    client: ClientId,
    format: CameraFormat,
    commands: pw::channel::Receiver<Command>,
    signals: mpsc::UnboundedSender<Signal>,
    queued: Arc<AtomicUsize>,
    ready: std::sync::mpsc::Sender<anyhow::Result<()>>,
) {
    macro_rules! up {
        ($e:expr, $what:expr) => {
            match $e {
                Ok(v) => v,
                Err(e) => {
                    let _ = ready.send(Err(anyhow::Error::from(e).context($what)));
                    return;
                }
            }
        };
    }
    let expected = Picture { width: usize::from(format.width), height: usize::from(format.height) };
    let feed = Rc::new(RefCell::new(Feed {
        client,
        decoder: up!(Decoder::new(), "opening the camera's decoder"),
        picture: Vec::new(),
        scratch: Vec::new(),
        expected,
        streaming: false,
        keyframe_owed: true,
        size_reported: false,
        signals: signals.clone(),
    }));

    pw::init();
    let mainloop = up!(pw::main_loop::MainLoopRc::new(None), "creating PipeWire's loop");
    let context = up!(pw::context::ContextRc::new(&mainloop, None), "creating PipeWire's context");
    let core = up!(context.connect_rc(None), "connecting to PipeWire");

    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Camera",
        *pw::keys::MEDIA_CLASS => "Video/Source",
        *pw::keys::APP_NAME => "wlshare",
        // Named for what it is, so it is not mistaken for a camera of the host's
        // own in an application's list of them.
        *pw::keys::NODE_DESCRIPTION => "wlshare remote camera",
    };
    props.insert(*pw::keys::NODE_NAME, format!("wlshare-camera-{}", client.0));
    let stream = up!(pw::stream::StreamRc::new(core, "wlshare camera", props), "creating the camera stream");

    let state_feed = feed.clone();
    let process_feed = feed.clone();
    let picture_bytes = expected.bytes();
    let _listener = up!(
        stream
            .add_local_listener::<()>()
            .state_changed(move |_, _, old, new| {
                if let pw::stream::StreamState::Error(e) = &new {
                    warn!("client {}: camera: {e}", client.0);
                } else {
                    debug!("client {}: camera {old:?} -> {new:?}", client.0);
                }
                let streaming = matches!(new, pw::stream::StreamState::Streaming);
                let mut feed = state_feed.borrow_mut();
                if streaming == feed.streaming {
                    return;
                }
                feed.streaming = streaming;
                if streaming {
                    // A stream opens on a keyframe, which the client is about to
                    // send; nothing decoded before it belongs to this stream.
                    feed.keyframe_owed = true;
                    info!("client {}: an application opened the camera", client.0);
                } else {
                    info!("client {}: the camera is no longer open", client.0);
                }
                let _ = feed.signals.send(if streaming { Signal::Start } else { Signal::Stop });
            })
            .param_changed(move |stream, _, id, param| {
                if id != spa::param::ParamType::Format.as_raw() || param.is_none() {
                    return;
                }
                // The format is the one offered, so the buffers are always this size.
                let pod = match serialize(buffers_param(picture_bytes, usize::from(format.width))) {
                    Ok(pod) => pod,
                    Err(e) => return warn!("client {}: serializing the camera's buffers: {e:?}", client.0),
                };
                let mut params = [spa::pod::Pod::from_bytes(&pod).expect("a serialized pod")];
                if let Err(e) = stream.update_params(&mut params) {
                    warn!("client {}: the camera's buffers were refused: {e}", client.0);
                }
            })
            .process(move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                let feed = process_feed.borrow();
                let Some(data) = buffer.datas_mut().first_mut() else { return };
                let written = match data.data() {
                    Some(bytes) => {
                        let n = feed.picture.len().min(bytes.len());
                        bytes[..n].copy_from_slice(&feed.picture[..n]);
                        n
                    }
                    None => 0,
                };
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.size_mut() = written as u32;
                *chunk.stride_mut() = i32::from(format.width);
            })
            .register(),
        "listening to the camera stream"
    );

    let command_stream = stream.clone();
    let loop_ = mainloop.clone();
    let _commands = commands.attach(mainloop.loop_(), move |command| match command {
        Command::Quit => loop_.quit(),
        Command::Sample { unit, keyframe } => {
            queued.fetch_sub(1, Ordering::Relaxed);
            // The borrow ends before the trigger: a driving stream may run its
            // process callback, which reads the picture, before returning.
            let deliver = feed.borrow_mut().take(&unit, keyframe);
            if deliver && let Err(e) = command_stream.trigger_process() {
                debug!("client {}: the camera's graph cycle was not started: {e}", client.0);
            }
        }
    });

    let pod = up!(serialize(format_param(format)), "serializing the camera format");
    let mut params = [spa::pod::Pod::from_bytes(&pod).expect("a serialized pod")];
    up!(
        stream.connect(
            spa::utils::Direction::Output,
            None,
            // A driver, because only this node knows when a frame is due; no
            // RT_PROCESS, because its callbacks decode and copy.
            pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        ),
        "connecting the camera stream"
    );
    info!(
        "client {}: plugged a {}x{} camera at {}/{} frames a second as PipeWire node wlshare-camera-{}",
        client.0, format.width, format.height, format.fps_numerator, format.fps_denominator, client.0
    );
    let _ = ready.send(Ok(()));
    mainloop.run();
    info!("client {}: the camera is unplugged", client.0);
}

fn serialize(value: spa::pod::Value) -> Result<Vec<u8>, spa::pod::serialize::GenError> {
    Ok(spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &value)?.0.into_inner())
}

/// The one format the node offers: I420 at the plugged geometry and rate.
fn format_param(format: CameraFormat) -> spa::pod::Value {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    spa::pod::Value::Object(spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(FormatProperties::VideoFormat, Id, spa::param::video::VideoFormat::I420),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Rectangle,
            spa::utils::Rectangle { width: u32::from(format.width), height: u32::from(format.height) }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Fraction,
            spa::utils::Fraction { num: format.fps_numerator, denom: format.fps_denominator }
        ),
    ))
}

/// The buffers the node fills: one block holding a whole picture, in memory
/// PipeWire allocates and this thread maps.
fn buffers_param(bytes: usize, stride: usize) -> spa::pod::Value {
    use spa::pod::{ChoiceValue, Property, PropertyFlags, Value};
    use spa::utils::{Choice, ChoiceEnum, ChoiceFlags};
    let property = |key, value| Property { key, flags: PropertyFlags::empty(), value };
    let memory = (1 << spa::sys::SPA_DATA_MemPtr) | (1 << spa::sys::SPA_DATA_MemFd);
    Value::Object(spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            property(
                spa::sys::SPA_PARAM_BUFFERS_buffers,
                Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Range { default: 4, min: 1, max: 8 }))),
            ),
            property(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            property(spa::sys::SPA_PARAM_BUFFERS_size, Value::Int(bytes as i32)),
            property(spa::sys::SPA_PARAM_BUFFERS_stride, Value::Int(stride as i32)),
            property(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Flags { default: memory, flags: Vec::new() }))),
            ),
        ],
    })
}
