//! The desktop's sound: a PipeWire capture of the default sink's monitor, one
//! per client that enabled audio, in the format that client asked for.
//!
//! wlshare runs inside the user's session, where the audio graph is, so it
//! reads what the desktop plays the same way it reads what the desktop draws:
//! from the compositor's side, with nothing configured on the client. The
//! stream is a `Stream/Input/Audio` node with `stream.capture.sink` set, which
//! is PipeWire's word for "the monitor of whatever the default sink is", so it
//! follows the default output when that changes. PipeWire converts the graph's
//! own rate and sample format into the client's, and is asked for buffers of
//! twenty milliseconds — one Opus packet's worth at the gateway.
//!
//! Per client rather than shared, because the format is the client's choice
//! and two clients may choose differently; a capture stream is cheap, and a
//! desktop with two listeners is rare. Nothing runs while no client has
//! enabled audio: the stream, its thread and its PipeWire connection exist
//! between an enable and the disable or disconnect that ends it.
//!
//! PipeWire's loop wants a thread of its own, so each capture is one, and the
//! process callback runs on that loop rather than on the graph's real-time
//! thread — `RT_PROCESS` is deliberately not set. The callback copies the
//! buffer into a queue, which allocates, takes a mutex and wakes a task, and
//! none of that is real-time safe: run on the data thread it could stall the
//! whole audio graph and give every application on the host an xrun. The
//! thread this capture already owns is the right place for it, and a
//! twenty-millisecond buffer has time to spare. A session that falls behind
//! loses the oldest buffers, never the newest, so what it does send is live.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context as _;
use log::{debug, info, warn};
use pipewire as pw;
use pw::spa;
use tokio::sync::Notify;
use wlshare_rfb::audio::{AudioFormat, SampleFormat};

use crate::shared::ClientId;

/// Buffers kept for a session slow to send them; the oldest goes first.
const QUEUE_DEPTH: usize = 16;
/// How long PipeWire may take to come up before an enable is refused.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// The buffer PipeWire is asked for, in milliseconds of the client's rate.
const BUFFER_MS: u32 = 20;

/// One client's capture: the thread running PipeWire's loop, and the buffers
/// it has produced.
pub struct Capture {
    quit: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
    queue: Arc<Queue>,
}

struct Queue {
    buffers: Mutex<VecDeque<Vec<u8>>>,
    /// Signalled when a buffer is queued; a permit is kept when nobody waits.
    ready: Notify,
    dropped: std::sync::atomic::AtomicU64,
}

impl Capture {
    /// Open a capture in `format` for `client`. Blocks until PipeWire has taken
    /// the stream or refused it, so it belongs on a blocking thread.
    pub fn start(client: ClientId, format: AudioFormat) -> anyhow::Result<Self> {
        let queue = Arc::new(Queue {
            buffers: Mutex::new(VecDeque::with_capacity(QUEUE_DEPTH)),
            ready: Notify::new(),
            dropped: std::sync::atomic::AtomicU64::new(0),
        });
        let (quit, quit_rx) = pw::channel::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread_queue = queue.clone();
        let thread = std::thread::Builder::new()
            .name(format!("audio-{}", client.0))
            .spawn(move || run(client, format, thread_queue, quit_rx, ready_tx))
            .context("spawning the audio thread")?;
        let mut capture = Self { quit, thread: Some(thread), queue };
        match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(capture),
            Ok(Err(e)) => {
                // The thread has already returned.
                capture.thread.take().map(JoinHandle::join);
                Err(e)
            }
            Err(_) => {
                // Stuck coming up: ask it to quit and let it go rather than
                // wait for a loop that may never run.
                let _ = capture.quit.send(());
                capture.thread.take();
                anyhow::bail!("PipeWire did not take the capture within {START_TIMEOUT:?}")
            }
        }
    }

    /// The oldest buffer not yet sent, if any.
    pub fn take(&self) -> Option<Vec<u8>> {
        self.queue.buffers.lock().unwrap().pop_front()
    }

    /// Resolves once a buffer has been queued since the last wait.
    pub async fn ready(&self) {
        self.queue.ready.notified().await;
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let dropped = self.queue.dropped.load(std::sync::atomic::Ordering::Relaxed);
        if dropped > 0 {
            debug!("audio: {dropped} buffer(s) were dropped for a session that fell behind");
        }
    }
}

fn spa_format(sample: SampleFormat) -> spa::param::audio::AudioFormat {
    use spa::param::audio::AudioFormat as F;
    match sample {
        SampleFormat::U8 => F::U8,
        SampleFormat::S8 => F::S8,
        SampleFormat::U16 => F::U16LE,
        SampleFormat::S16 => F::S16LE,
        SampleFormat::U32 => F::U32LE,
        SampleFormat::S32 => F::S32LE,
    }
}

/// The capture thread: PipeWire's loop, from connect to quit. `ready` is told
/// once, when the stream is connected or when that failed.
fn run(
    client: ClientId,
    format: AudioFormat,
    queue: Arc<Queue>,
    quit: pw::channel::Receiver<()>,
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
    pw::init();
    let mainloop = up!(pw::main_loop::MainLoopRc::new(None), "creating PipeWire's loop");
    let context = up!(pw::context::ContextRc::new(&mainloop, None), "creating PipeWire's context");
    let core = up!(context.connect_rc(None), "connecting to PipeWire");

    let frames = format.frequency * BUFFER_MS / 1000;
    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::STREAM_CAPTURE_SINK => "true",
        *pw::keys::APP_NAME => "wlshare",
    };
    props.insert(*pw::keys::NODE_NAME, format!("wlshare client {}", client.0));
    props.insert(*pw::keys::NODE_LATENCY, format!("{frames}/{}", format.frequency));
    props.insert(*pw::keys::NODE_RATE, format!("1/{}", format.frequency));
    let stream = up!(pw::stream::StreamBox::new(&core, "wlshare audio", props), "creating the capture stream");

    let frame_bytes = format.frame_bytes();
    let process_queue = queue.clone();
    let _listener = up!(
        stream
            .add_local_listener::<()>()
            .state_changed(move |_, _, old, new| match &new {
                pw::stream::StreamState::Error(e) => warn!("client {}: audio capture: {e}", client.0),
                _ => debug!("client {}: audio capture {old:?} -> {new:?}", client.0),
            })
            // On this capture's own loop thread, not the graph's real-time one.
            .process(move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                let datas = buffer.datas_mut();
                let Some(data) = datas.first_mut() else { return };
                let offset = data.chunk().offset() as usize;
                let size = data.chunk().size() as usize;
                let Some(bytes) = data.data() else { return };
                let start = offset.min(bytes.len());
                let end = (offset + size).min(bytes.len());
                // Whole frames only: a split frame would swap the channels of
                // everything after it.
                let whole = (end - start) / frame_bytes * frame_bytes;
                if whole == 0 {
                    return;
                }
                let samples = bytes[start..start + whole].to_vec();
                let mut buffers = process_queue.buffers.lock().unwrap();
                if buffers.len() >= QUEUE_DEPTH {
                    buffers.pop_front();
                    process_queue.dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                buffers.push_back(samples);
                drop(buffers);
                process_queue.ready.notify_one();
            })
            .register(),
        "listening to the capture stream"
    );

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa_format(format.sample));
    info.set_rate(format.frequency);
    info.set_channels(u32::from(format.channels));
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let pod = up!(
        spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(object)),
        "serializing the audio format"
    )
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&pod).expect("a serialized pod")];
    up!(
        stream.connect(
            spa::utils::Direction::Input,
            None,
            // No RT_PROCESS: the process callback allocates and locks, which
            // the graph's real-time thread must not do — see the module doc.
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        ),
        "connecting the capture stream"
    );

    let loop_ = mainloop.clone();
    let _quit = quit.attach(mainloop.loop_(), move |()| loop_.quit());
    info!(
        "client {}: capturing the default sink's monitor as {:?} x{} at {} Hz, {frames} frames a buffer",
        client.0, format.sample, format.channels, format.frequency
    );
    let _ = ready.send(Ok(()));
    mainloop.run();
    debug!("client {}: audio capture closed", client.0);
}
