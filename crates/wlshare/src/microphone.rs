//! The client's microphone, lent to the desktop: a PipeWire `Audio/Source` node
//! the desktop's applications record from like any other microphone, fed from
//! the PCM the client sends over the microphone extension
//! ([`wlshare_rfb::microphone`]).
//!
//! The camera's twin ([`crate::camera`]): a node in the session's own graph, so
//! it needs no kernel module and no privilege, and it exists from the client's
//! plug to its unplug or its leaving. It offers one format, [`FORMAT`], which the
//! client is told in every start; PipeWire converts it for an application that
//! records in another.
//!
//! Whether samples flow is the desktop's decision. The stream goes to
//! *streaming* when an application links to the node and back when the last one
//! leaves; each transition is a [`Signal`] the session sends on, so the client
//! captures only while something is recording.
//!
//! Unlike the camera the node is not its own driver. Audio has a clock, the
//! graph's, and a source that runs to its own would drift against every sink the
//! recording application also plays into; so the graph asks for a quantum of
//! frames when it wants one, and the node answers from what the client has sent.
//! The client's samples arrive at the network's pace rather than the graph's, so
//! they wait in a [`Jitter`] buffer between the two: it holds back the first
//! [`PREFILL_MS`] before it plays, so one late sample is not a click, answers
//! silence while it is empty, and keeps no more than [`MAX_BUFFERED_MS`], so a
//! client that sent a burst after a stall is heard live rather than late.
//!
//! Like the audio capture ([`crate::audio`]), each microphone is a thread running
//! PipeWire's loop, and the process callback runs on that loop rather than on the
//! graph's real-time thread — `RT_PROCESS` is deliberately not set, because it
//! takes the buffer's mutex, which the session's task holds while it pushes.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context as _;
use log::{debug, info, warn};
use pipewire as pw;
use pw::spa;
use tokio::sync::mpsc;
use wlshare_rfb::microphone::MicrophoneFormat;

use crate::shared::ClientId;

/// What the node offers and every start names: mono at Opus's own rate, which is
/// what a browser's speech encoder decodes to, so the remotex gateway resamples
/// nothing.
pub const FORMAT: MicrophoneFormat = MicrophoneFormat { channels: 1, frequency: 48_000 };
/// How long PipeWire may take to take the node before a plug is refused.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// Audio held back before the node plays, after a start or after running dry.
const PREFILL_MS: usize = 60;
/// The most audio kept waiting for the graph; the oldest goes past it.
const MAX_BUFFERED_MS: usize = 200;
/// The quantum the node asks the graph for: the twenty milliseconds a client
/// sends at a time.
const LATENCY_MS: u32 = 20;

/// A decision of the desktop's, for the session to send the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// An application is recording: send samples in [`FORMAT`].
    Start,
    /// The last application stopped: send none.
    Stop,
}

/// One plugged microphone: the thread running its node, the buffer between the
/// client and the graph, and the desktop's decisions.
pub struct Microphone {
    quit: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
    jitter: Arc<Mutex<Jitter>>,
    signals: mpsc::UnboundedReceiver<Signal>,
}

impl Microphone {
    /// Make the node for `client`'s microphone. Blocks until PipeWire has taken
    /// it or refused it, so it belongs on a blocking thread.
    pub fn plug(client: ClientId) -> anyhow::Result<Self> {
        let jitter = Arc::new(Mutex::new(Jitter::new(FORMAT)));
        let (quit, quit_rx) = pw::channel::channel();
        let (signal_tx, signals) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread_jitter = jitter.clone();
        let thread = std::thread::Builder::new()
            .name(format!("microphone-{}", client.0))
            .spawn(move || run(client, thread_jitter, quit_rx, signal_tx, ready_tx))
            .context("spawning the microphone thread")?;
        let mut microphone = Self { quit, thread: Some(thread), jitter, signals };
        match ready_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => Ok(microphone),
            Ok(Err(e)) => {
                // The thread has already returned.
                microphone.thread.take().map(JoinHandle::join);
                Err(e)
            }
            Err(_) => {
                // Stuck coming up: ask it to quit and let it go rather than
                // wait for a loop that may never run.
                let _ = microphone.quit.send(());
                microphone.thread.take();
                anyhow::bail!("PipeWire did not take the microphone within {START_TIMEOUT:?}")
            }
        }
    }

    /// Queue the client's PCM, whole frames of [`FORMAT`], for the graph; dropped
    /// while nothing records.
    pub fn sample(&self, pcm: &[u8]) {
        self.jitter.lock().unwrap().push(pcm);
    }

    /// The desktop's next decision. Never resolves while there is none.
    pub async fn signal(&mut self) -> Signal {
        match self.signals.recv().await {
            Some(signal) => signal,
            // The thread is gone, and with it every decision it would have made.
            None => std::future::pending().await,
        }
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let dropped = self.jitter.lock().unwrap().dropped;
        if dropped > 0 {
            debug!("microphone: {dropped} byte(s) of audio arrived faster than the desktop recorded them and were dropped");
        }
    }
}

/// The client's PCM on its way to the graph, in whole frames.
struct Jitter {
    queue: VecDeque<u8>,
    /// Bytes held back before playing: [`PREFILL_MS`].
    prefill: usize,
    /// Bytes kept at most: [`MAX_BUFFERED_MS`].
    ceiling: usize,
    /// An application is recording; samples outside that are nobody's.
    streaming: bool,
    /// The prefill has arrived, and the node plays until the queue runs dry.
    playing: bool,
    /// Bytes pushed out past the ceiling.
    dropped: u64,
}

impl Jitter {
    fn new(format: MicrophoneFormat) -> Self {
        let frame_bytes = format.frame_bytes();
        let bytes_for = |ms: usize| format.frequency as usize * ms / 1000 * frame_bytes;
        Self {
            queue: VecDeque::new(),
            prefill: bytes_for(PREFILL_MS),
            ceiling: bytes_for(MAX_BUFFERED_MS),
            streaming: false,
            playing: false,
            dropped: 0,
        }
    }

    /// A recording started or stopped: either way, nothing queued belongs to what
    /// follows.
    fn set_streaming(&mut self, streaming: bool) {
        self.streaming = streaming;
        self.queue.clear();
        self.playing = false;
    }

    fn push(&mut self, pcm: &[u8]) {
        if !self.streaming {
            return;
        }
        self.queue.extend(pcm);
        let over = self.queue.len().saturating_sub(self.ceiling);
        if over > 0 {
            // Both lengths are whole frames, so what goes is too.
            self.queue.drain(..over);
            self.dropped += over as u64;
        }
    }

    /// Fill `out`, whole frames, with what is queued — silence before the prefill
    /// has arrived, and after the queue runs dry.
    fn fill(&mut self, out: &mut [u8]) {
        if !self.playing && self.queue.len() >= self.prefill {
            self.playing = true;
        }
        let taken = if self.playing { self.queue.len().min(out.len()) } else { 0 };
        for (slot, byte) in out.iter_mut().zip(self.queue.drain(..taken)) {
            *slot = byte;
        }
        out[taken..].fill(0);
        if self.playing && taken < out.len() {
            self.playing = false;
        }
    }
}

/// The microphone thread: PipeWire's loop, from the node's creation to quit.
/// `ready` is told once, when the stream is connected or when that failed.
fn run(
    client: ClientId,
    jitter: Arc<Mutex<Jitter>>,
    quit: pw::channel::Receiver<()>,
    signals: mpsc::UnboundedSender<Signal>,
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

    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::MEDIA_CLASS => "Audio/Source",
        *pw::keys::APP_NAME => "wlshare",
        // Named for what it is, so it is not mistaken for a microphone of the
        // host's own in an application's list of them.
        *pw::keys::NODE_DESCRIPTION => "wlshare remote microphone",
    };
    props.insert(*pw::keys::NODE_NAME, format!("wlshare-microphone-{}", client.0));
    let frames = FORMAT.frequency * LATENCY_MS / 1000;
    props.insert(*pw::keys::NODE_LATENCY, format!("{frames}/{}", FORMAT.frequency));
    let stream = up!(pw::stream::StreamBox::new(&core, "wlshare microphone", props), "creating the microphone stream");

    let state_jitter = jitter.clone();
    let frame_bytes = FORMAT.frame_bytes();
    let _listener = up!(
        stream
            .add_local_listener::<()>()
            .state_changed(move |_, _, old, new| {
                if let pw::stream::StreamState::Error(e) = &new {
                    warn!("client {}: microphone: {e}", client.0);
                } else {
                    debug!("client {}: microphone {old:?} -> {new:?}", client.0);
                }
                let streaming = matches!(new, pw::stream::StreamState::Streaming);
                let mut jitter = state_jitter.lock().unwrap();
                if streaming == jitter.streaming {
                    return;
                }
                jitter.set_streaming(streaming);
                drop(jitter);
                if streaming {
                    info!("client {}: an application is recording from the microphone", client.0);
                } else {
                    info!("client {}: nothing records from the microphone any more", client.0);
                }
                let _ = signals.send(if streaming { Signal::Start } else { Signal::Stop });
            })
            // On this microphone's own loop thread, not the graph's real-time one.
            .process(move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                let requested = usize::try_from(buffer.requested()).unwrap_or(usize::MAX);
                let Some(data) = buffer.datas_mut().first_mut() else { return };
                let written = match data.data() {
                    Some(bytes) => {
                        let room = bytes.len() / frame_bytes;
                        // The graph says how many frames this cycle wants when it
                        // knows; otherwise the buffer is filled.
                        let frames = if requested == 0 { room } else { requested.min(room) };
                        let n = frames * frame_bytes;
                        jitter.lock().unwrap().fill(&mut bytes[..n]);
                        n
                    }
                    None => 0,
                };
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.size_mut() = u32::try_from(written).unwrap_or(u32::MAX);
                *chunk.stride_mut() = frame_bytes as i32;
            })
            .register(),
        "listening to the microphone stream"
    );

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::S16LE);
    info.set_rate(FORMAT.frequency);
    info.set_channels(u32::from(FORMAT.channels));
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let pod = up!(
        spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(object)),
        "serializing the microphone format"
    )
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&pod).expect("a serialized pod")];
    up!(
        stream.connect(
            spa::utils::Direction::Output,
            None,
            // Not AUTOCONNECT: a source is linked to by whatever records from it,
            // and linked on its own it would play the client's voice into the
            // default sink. No RT_PROCESS — see the module doc.
            pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        ),
        "connecting the microphone stream"
    );

    let loop_ = mainloop.clone();
    let _quit = quit.attach(mainloop.loop_(), move |()| loop_.quit());
    info!(
        "client {}: plugged a microphone, mono at {} Hz, as PipeWire node wlshare-microphone-{}",
        client.0, FORMAT.frequency, client.0
    );
    let _ = ready.send(Ok(()));
    mainloop.run();
    info!("client {}: the microphone is unplugged", client.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ms` milliseconds of [`FORMAT`], every frame `value`: 96 bytes a millisecond.
    fn frames(ms: usize, value: i16) -> Vec<u8> {
        value.to_le_bytes().repeat(48 * ms)
    }

    fn streaming() -> Jitter {
        let mut jitter = Jitter::new(FORMAT);
        jitter.set_streaming(true);
        jitter
    }

    #[test]
    fn nothing_is_kept_while_nothing_records() {
        let mut jitter = Jitter::new(FORMAT);
        jitter.push(&frames(100, 7));
        let mut out = vec![1; 960];
        jitter.fill(&mut out);
        assert!(out.iter().all(|&b| b == 0));
    }

    /// Silence until the prefill has arrived, then the samples in order.
    #[test]
    fn the_prefill_is_held_back_and_then_played() {
        let mut jitter = streaming();
        jitter.push(&frames(40, 5));
        let mut out = vec![1; 960];
        jitter.fill(&mut out);
        assert!(out.iter().all(|&b| b == 0), "40 ms is short of the prefill");
        jitter.push(&frames(20, 6));
        jitter.fill(&mut out);
        assert_eq!(out, frames(10, 5));
        assert_eq!(jitter.queue.len(), 50 * 96);
    }

    /// A queue that runs dry pads the cycle with silence and waits for a prefill
    /// again, rather than playing each sample the moment it lands.
    #[test]
    fn running_dry_pads_with_silence_and_prefills_again() {
        let mut jitter = streaming();
        jitter.push(&frames(60, 9));
        let mut out = vec![1; 70 * 96];
        jitter.fill(&mut out);
        assert_eq!(&out[..60 * 96], &frames(60, 9)[..]);
        assert!(out[60 * 96..].iter().all(|&b| b == 0));
        jitter.push(&frames(20, 3));
        let mut out = vec![1; 960];
        jitter.fill(&mut out);
        assert!(out.iter().all(|&b| b == 0), "one sample after running dry is not a prefill");
    }

    /// A burst past the ceiling keeps the newest audio.
    #[test]
    fn a_burst_keeps_the_newest_audio() {
        let mut jitter = streaming();
        jitter.push(&frames(150, 1));
        jitter.push(&frames(100, 2));
        assert_eq!(jitter.queue.len(), MAX_BUFFERED_MS * 96);
        assert_eq!(jitter.dropped, 50 * 96);
        let mut out = vec![0; 100 * 96];
        jitter.fill(&mut out);
        assert_eq!(out, frames(100, 1));
        jitter.fill(&mut out);
        assert_eq!(out, frames(100, 2));
    }

    /// A stop drops what the last recording left, and the next start prefills
    /// afresh.
    #[test]
    fn a_new_recording_starts_empty() {
        let mut jitter = streaming();
        jitter.push(&frames(100, 4));
        jitter.set_streaming(false);
        jitter.set_streaming(true);
        assert!(jitter.queue.is_empty());
        assert!(!jitter.playing);
    }
}
