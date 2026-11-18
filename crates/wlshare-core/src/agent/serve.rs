//! The agent's end of the link ([`crate::agent::link`]): connections accepted, each
//! one stream, coded by the encoder its machine has. Experimental.
//!
//! Each connection has a thread of its own, the picture as the daemon last
//! sent it, and an encoder over it, which the agent's binary opens on the
//! VA display it opened for the daemon. A connection that fails its
//! handshake is closed without a word, and one whose stream fails is told
//! why and closed, which fails the daemon's stream and its session.
//!
//! An agent is for the one machine it lends its encoder to, over a link
//! with nothing else on it worth the name: the virtual machine beside it on
//! its host, or a wired machine on its LAN. So it is told where that daemon
//! is ([`Net`]), with no default, and a connection from anywhere else is
//! closed as it is accepted, before a thread or a byte. An agent whose
//! allowlist is wider than its daemon is one set up wrong, and what follows
//! is what such an agent still holds to, not what makes it right.
//!
//! An agent is one daemon's at a time: the daemon with a stream open, by
//! its address, and nobody else is let in until its last stream closes.
//!
//! What a connection from an allowed address gets before it proves the key
//! is little and counted apart: a thread, a socket, and the handshake's time
//! to prove it in, as a whole ([`HANDSHAKE_WITHIN`]). One that proves
//! another key, or none, is closed without a word, having got the agent's
//! hello and its proof and nothing of its encoder. Only a connection that
//! has proved the key is a stream, with a buffer for its picture, an
//! encoder, and a place among the streams ([`STREAMS`]).

use std::collections::VecDeque;
use std::io::ErrorKind::{ConnectionAborted, ConnectionReset};
use std::io::{BufReader, Read, Write};
use std::net::{IpAddr, Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::Context as _;
use log::{info, warn};

use crate::agent::allow::Net;
use crate::agent::link::{
    CODED_HEAD, FRAME_WITHIN, Frame, HANDSHAKE_WITHIN, MARK, MAX_SIDE, Mac as _, NO_FRAME, Role,
    SEEN, Span, Timed, coded_head, failed, hello, keepalive, nonce, opened, proof, read_hello,
    read_open_request,
};
use crate::agent::shared::Places;
use crate::h264::{Backend, Planes};

/// The most streams an agent codes at once for the daemon it has, each a
/// thread, a socket and an encoder on the GPU: a session's video is one
/// stream and its tiles' regions a few, so this is several sessions'. A
/// daemon that asks for one more is told there is none.
pub const STREAMS: usize = 32;

/// The most connections that have not yet proved the key, each a thread and
/// a socket, which is all that an allowed address can make an agent hold
/// without the key. They are no streams and take no stream's place, and one
/// more closes the oldest of them: a daemon's handshake is over in
/// milliseconds, so whatever has waited longest is the likeliest to be
/// nothing, and a port held open by a stranger still lets a daemon in. A
/// stranger that opens this many more inside a daemon's handshake closes
/// that too, and is at an address the allowlist was not to hold.
const UNPROVED: usize = 32;

/// How long the agent waits before it accepts again after an error that is
/// not one connection's: the machine's, out of descriptors or of memory,
/// which stands for a while and is not to be spun on or fill the log.
const ACCEPT_AGAIN: Duration = Duration::from_secs(1);

/// How much of a picture is read from the connection at a time.
const READ_AHEAD: usize = 256 * 1024;

/// What opens an encoder over a coded picture of even sides, for one stream
/// of the daemon it was made for.
pub type Streams = dyn Fn((usize, usize)) -> anyhow::Result<Box<dyn Backend>> + Send + Sync;

/// What makes a daemon its [`Streams`], when it becomes the agent's user:
/// whatever its streams share, which for the hardware is a VA display on the
/// device, opened once for the daemon and not for each stream, and closed
/// with its last.
pub type Open = dyn Fn() -> anyhow::Result<Arc<Streams>> + Send + Sync;

/// What an agent keeps to: so many streams, so many connections yet to
/// prove the key, and so long for one to prove it.
#[derive(Clone, Copy)]
struct Limits {
    streams: usize,
    unproved: usize,
    handshake: Duration,
}

/// The daemon an agent is coding for: where it is, what opens its streams,
/// and how many it has open.
struct User {
    address: IpAddr,
    streams: Arc<Streams>,
    coding: usize,
}

/// Who an agent lets in and what it holds for them, against its [`Limits`].
///
/// **From where it allows.** A connection from an address in none of the
/// agent's networks is closed as it is accepted, whoever has the agent or
/// nobody, and an agent with no network lets nobody in.
///
/// **One daemon at a time.** The daemon with a stream open is the agent's
/// user, known by its address, until its last stream closes. While it is, a
/// connection from any other address is closed as it is accepted, before a
/// thread or a handshake, so nobody elsewhere on the network comes between
/// the user and its streams, with the key or without; and its streams share
/// what was opened for it. An agent nobody is using lets any address it
/// allows try the key, and the first to prove it and ask for a stream is the
/// user.
struct Door {
    limits: Limits,
    allowed: Vec<Net>,
    /// The memory the agent shares with its daemon, a picture a stream.
    places: Option<Arc<Places>>,
    open: Arc<Open>,
    /// The connections yet to prove the key, the oldest first, each by its
    /// number and a handle of its socket to close it by.
    unproved: Mutex<VecDeque<(u64, TcpStream)>>,
    user: Mutex<Option<User>>,
}

/// A connection's place among the unproved, given back when it is dropped:
/// the key proved, the handshake failed, or the thread never started.
struct Unproved {
    door: Arc<Door>,
    number: u64,
}

/// One of the user's streams, counted until it is dropped; the user's last
/// leaves the agent nobody's.
struct Coding(Arc<Door>);

impl Door {
    fn user(&self) -> MutexGuard<'_, Option<User>> {
        self.user.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a daemon may connect from `address`: one of the agent's
    /// networks holds it.
    fn allows(&self, address: IpAddr) -> bool {
        self.allowed.iter().any(|net| net.contains(address))
    }

    /// The daemon elsewhere than `address` that the agent is coding for,
    /// where there is one: nothing from `address` is let in while it is.
    fn anothers(&self, address: IpAddr) -> Option<IpAddr> {
        self.user()
            .as_ref()
            .map(|user| user.address)
            .filter(|user| *user != address)
    }

    /// A place among the unproved for connection `number`, the oldest there
    /// closed for it where they are as many as may be.
    fn unproved(self: &Arc<Self>, number: u64, stream: &TcpStream) -> std::io::Result<Unproved> {
        let handle = stream.try_clone()?;
        let mut unproved = self.unproved.lock().unwrap_or_else(PoisonError::into_inner);
        if unproved.len() >= self.limits.unproved
            && let Some((_, oldest)) = unproved.pop_front()
        {
            warn!(
                "{}: closed with its handshake unfinished, for another's to begin",
                peer(&oldest)
            );
            // Under its thread, whose read ends with it.
            let _ = oldest.shutdown(Shutdown::Both);
        }
        unproved.push_back((number, handle));
        Ok(Unproved {
            door: self.clone(),
            number,
        })
    }

    /// A stream for the daemon at `address`, which has proved the key: one
    /// more of the user's, or the first of a daemon that becomes it, with
    /// what opens its streams. An error, for the daemon to be told, where
    /// the agent is another's or the user has as many as it may.
    fn coding(self: &Arc<Self>, address: IpAddr) -> anyhow::Result<(Coding, Arc<Streams>)> {
        let mut user = self.user();
        let streams = match &mut *user {
            Some(user) => {
                anyhow::ensure!(
                    user.address == address,
                    "the agent is coding for the daemon at {}, and for one daemon at a time",
                    user.address
                );
                anyhow::ensure!(
                    user.coding < self.limits.streams,
                    "the agent is coding {} streams, which is as many as it takes",
                    user.coding
                );
                user.coding += 1;
                user.streams.clone()
            }
            None => {
                let streams = (self.open)()?;
                info!("{address}: the agent is its daemon's");
                *user = Some(User {
                    address,
                    streams: streams.clone(),
                    coding: 1,
                });
                streams
            }
        };
        Ok((Coding(self.clone()), streams))
    }
}

impl Drop for Unproved {
    fn drop(&mut self) {
        self.door
            .unproved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(number, _)| *number != self.number);
    }
}

impl Drop for Coding {
    fn drop(&mut self) {
        let mut user = self.0.user();
        let Some(theirs) = user.as_mut() else {
            return;
        };
        theirs.coding -= 1;
        if theirs.coding == 0 {
            info!(
                "{}: its last stream closed, and the agent is nobody's",
                theirs.address
            );
            *user = None;
        }
    }
}

/// Who a connection is from, for the log.
fn peer(stream: &TcpStream) -> String {
    stream
        .peer_addr()
        .map_or_else(|_| "a daemon".to_owned(), |a| a.to_string())
}

/// Accept connections on `listener` for as long as it stands, from the
/// addresses `allowed` holds and no others, each a stream for whoever holds
/// `key`, coded on a thread of its own by an encoder of what `open` makes
/// its daemon: one daemon's at a time, and [`STREAMS`] of its at once.
/// With `places`, memory shared with the daemon, each stream that the
/// daemon shares it for has its picture there while there is room.
pub fn serve(
    listener: TcpListener,
    key: Arc<[u8]>,
    allowed: Vec<Net>,
    places: Option<Arc<Places>>,
    open: Arc<Open>,
) {
    accept(
        listener,
        key,
        allowed,
        places,
        open,
        Limits {
            streams: STREAMS,
            unproved: UNPROVED,
            handshake: HANDSHAKE_WITHIN,
        },
    );
}

/// [`serve`], within `limits`.
fn accept(
    listener: TcpListener,
    key: Arc<[u8]>,
    allowed: Vec<Net>,
    places: Option<Arc<Places>>,
    open: Arc<Open>,
    limits: Limits,
) {
    let door = Arc::new(Door {
        limits,
        allowed,
        places,
        open,
        unproved: Mutex::default(),
        user: Mutex::default(),
    });
    for (number, stream) in listener.incoming().enumerate() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                warn!("accepting a connection: {e}");
                // A connection gone before it was accepted is its own
                // error, and the next is there to take.
                if !matches!(e.kind(), ConnectionAborted | ConnectionReset) {
                    std::thread::sleep(ACCEPT_AGAIN);
                }
                continue;
            }
        };
        // From nowhere that can be told is from nobody that can be let in.
        let Ok(from) = stream.peer_addr() else {
            continue;
        };
        let (peer, address) = (from.to_string(), from.ip().to_canonical());
        if !door.allows(address) {
            warn!("{peer}: closed unanswered, being from nowhere this agent allows");
            continue;
        }
        if let Some(user) = door.anothers(address) {
            warn!("{peer}: closed unanswered, the agent being the daemon at {user}'s");
            continue;
        }
        // Its place before its thread, so that the threads are as many as
        // the places and those just closed.
        let unproved = match door.unproved(number as u64, &stream) {
            Ok(unproved) => unproved,
            Err(e) => {
                warn!("{peer}: no handle of its socket: {e}");
                continue;
            }
        };
        let key = key.clone();
        let spawned = std::thread::Builder::new()
            .name("enc-agent".to_owned())
            .spawn(
                move || match connection(&stream, &key, &peer, address, unproved) {
                    Ok(served) => info!("{peer}: its stream ended after {served}"),
                    Err(e) => warn!("{peer}: {e:#}"),
                },
            );
        if let Err(e) = spawned {
            warn!("no thread for a connection: {e}");
        }
    }
}

/// What a stream came to, for the log: the frames coded, the bytes of
/// picture they were sent as and of stream they were coded into, and how
/// long the encoder took over them.
#[derive(Default)]
struct Served {
    frames: u64,
    pictures: u64,
    stream: u64,
    coding: Duration,
}

impl std::fmt::Display for Served {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let frames = self.frames.max(1) as f64;
        write!(
            f,
            "{} frames: {:.1} MB of pictures in and {:.1} MB of stream out, {:.1} ms a frame in the encoder",
            self.frames,
            self.pictures as f64 / 1e6,
            self.stream as f64 / 1e6,
            self.coding.as_secs_f64() * 1e3 / frames
        )
    }
}

/// One connection: the handshake, the stream it asks for, and its frames
/// until the daemon closes it. What the stream came to.
fn connection(
    stream: &TcpStream,
    key: &[u8],
    peer: &str,
    address: IpAddr,
    unproved: Unproved,
) -> anyhow::Result<Served> {
    let door = unproved.door.clone();
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(FRAME_WITHIN))?;
    keepalive(stream)?;
    // The key is proved within the handshake's time as a whole, and until
    // it is the connection has no buffer to read a picture into.
    let mut from = Timed::new(stream);
    from.within(door.limits.handshake);
    let mut to = stream;

    let ours = nonce();
    to.write_all(&hello(&ours)).context("sending the hello")?;
    let theirs = read_hello(&mut from)?;
    to.write_all(
        &proof(key, Role::Agent, &theirs, &ours)?
            .finalize()
            .into_bytes(),
    )
    .context("sending the proof of the key")?;
    let (its, (width, height), shared) = read_open_request(&mut from)?;
    proof(key, Role::Daemon, &theirs, &ours)?
        .verify_slice(&its)
        .map_err(|_| anyhow::anyhow!("its key is not this agent's"))?;

    // A daemon's: its place among the unproved is another's to take, and it
    // is the agent's user, or one more stream of the user's, or told why
    // not. The encoder before the planes, so a picture no encoder takes is
    // refused before it is made room for; and the stream counted for as
    // long as its encoder is.
    drop(unproved);
    let made = (|| {
        anyhow::ensure!(
            width > 0
                && height > 0
                && width % 2 == 0
                && height % 2 == 0
                && width <= MAX_SIDE
                && height <= MAX_SIDE,
            "a {width}x{height} picture is not one an agent codes"
        );
        let places = match (shared, &door.places) {
            (0, _) => None,
            (_, None) => anyhow::bail!(
                "the daemon shares memory with this agent and the agent none with it: \
                 start the agent with --shared-memory, or take shared_memory out of the \
                 daemon's [enc_agent] table"
            ),
            (shared, Some(places)) => {
                let ours = places.region().len();
                anyhow::ensure!(
                    shared == ours as u64,
                    "the daemon shares {shared} bytes of memory and this agent {ours}: \
                     the two are not the same memory"
                );
                Some(places)
            }
        };
        let (coding, streams) = door.coding(address)?;
        let backend = streams((width, height))?;
        // A place while there is room, the stream's pictures over the
        // connection while there is none.
        let place = places.and_then(|places| places.take((width, height)));
        Ok((coding, backend, place))
    })();
    let (_coding, mut backend, place) = match made {
        Ok(opened) => opened,
        Err(e) => {
            to.write_all(&failed(&format!("{e:#}")))?;
            return Err(e.context(format!("opening a {width}x{height} stream")));
        }
    };
    to.write_all(&opened(place.as_ref().map(|place| place.at())))?;
    let (mut planes, shares) = match place {
        Some(place) => {
            // The daemon's mark, where the picture starts: the two ends map
            // the same memory, or are told they do not.
            let mut mark = [0; MARK];
            from.read_exact(&mut mark).context("reading its mark")?;
            if !place.holds(&mark) {
                let why = "the daemon's mark is not where its picture is in this agent's \
                           shared memory: the two ends map other memory";
                to.write_all(&failed(why))?;
                anyhow::bail!(why);
            }
            to.write_all(&SEEN)?;
            info!(
                "{peer}: a {width}x{height} stream, its picture in shared memory at {}",
                place.at()
            );
            (place.planes(), true)
        }
        None => {
            match shared {
                0 => info!("{peer}: a {width}x{height} stream"),
                _ => info!(
                    "{peer}: a {width}x{height} stream, over the connection: no room for its \
                     picture in shared memory"
                ),
            }
            (Planes::new(width as u16, height as u16), false)
        }
    };

    let mut from = BufReader::with_capacity(READ_AHEAD, from);
    let mut served = Served::default();
    let mut reply = Vec::new();
    loop {
        // Between frames the daemon is owed nothing and may say nothing for
        // as long as its desktop stands still; a frame begun is read whole
        // within its time.
        from.get_mut().at_leisure();
        let mut tag = [0];
        match from.read(&mut tag) {
            Ok(0) => return Ok(served),
            Ok(_) => {}
            Err(e) => return Err(e).context("waiting for a frame"),
        }
        from.get_mut().within(FRAME_WITHIN);
        reply.clear();
        // The answer's head, its length to come once the frame is coded
        // after it.
        reply.extend_from_slice(&coded_head(0));
        let coded: anyhow::Result<bool> = (|| {
            let frame = Frame::read(tag[0], &mut from)?;
            for _ in 0..frame.spans {
                let span = Span::read(&mut from, (width, height))?;
                // In shared memory the rows are where the daemon wrote them.
                if !shares {
                    span.read_rows(&mut from, &mut planes)?;
                }
                planes.touch((span.x, span.y), (span.x + span.w, span.y + span.h));
                served.pictures += (span.w * span.h * 3 / 2) as u64;
            }
            planes.refine = frame.refine;
            let began = std::time::Instant::now();
            let made = backend.frame(&mut planes, frame.keyframe, frame.qp, &mut reply)?;
            served.coding += began.elapsed();
            if made {
                planes.coded_frame();
            }
            Ok(made)
        })();
        match coded {
            Ok(true) => {
                let length = (reply.len() - CODED_HEAD) as u32;
                reply[..CODED_HEAD].copy_from_slice(&coded_head(length));
                to.write_all(&reply).context("sending a frame")?;
                served.frames += 1;
                served.stream += u64::from(length);
            }
            Ok(false) => to.write_all(&NO_FRAME).context("answering a frame")?,
            Err(e) => {
                // Told why, if it is still there to be told.
                let _ = to.write_all(&failed(&format!("{e:#}")));
                return Err(e.context(format!(
                    "frame {} of a {width}x{height} stream",
                    served.frames
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::client::{Agent, Stream};
    use crate::agent::link::{FAILED, FRAME, MIN_KEY, OK, PROOF};
    use crate::agent::shared::tests::Scratch;
    use crate::agent::shared::{Region, picture_bytes};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The key the tests' agents and their daemons share.
    const KEY: [u8; MIN_KEY] = [0x5a; MIN_KEY];

    /// Where the tests' daemons are: every address of loopback's.
    fn loopback() -> Vec<Net> {
        vec!["127.0.0.0/8".parse().unwrap()]
    }

    /// What [`Echo`] makes of a picture: the coded picture's rows, luma and
    /// then chroma, and what it was asked of them.
    fn echo(planes: &Planes, keyframe: bool, qp: u8, out: &mut Vec<u8>) {
        let (w, h) = planes.coded;
        for (plane, rows) in [(&planes.y, h), (&planes.uv, h / 2)] {
            for row in 0..rows {
                out.extend_from_slice(&plane[row * planes.stride..][..w]);
            }
        }
        out.extend_from_slice(&[u8::from(keyframe), u8::from(planes.refine), qp]);
    }

    /// An encoder whose frame is its picture, so that what an agent holds of
    /// one can be read; and one that fails every frame.
    struct Echo;
    struct Broken;

    impl Backend for Echo {
        fn frame(
            &mut self,
            planes: &mut Planes,
            keyframe: bool,
            qp: u8,
            out: &mut Vec<u8>,
        ) -> anyhow::Result<bool> {
            echo(planes, keyframe, qp, out);
            Ok(true)
        }
    }

    impl Backend for Broken {
        fn frame(
            &mut self,
            _: &mut Planes,
            _: bool,
            _: u8,
            _: &mut Vec<u8>,
        ) -> anyhow::Result<bool> {
            anyhow::bail!("the encoder is unplugged")
        }
    }

    /// An agent on loopback for as long as the tests run, and where it
    /// listens: [`Echo`] for every picture but one 32 across, which gets
    /// [`Broken`], and one 48 across, which no encoder opens for.
    fn agent() -> String {
        agent_sharing(None)
    }

    /// [`agent`], sharing the memory `places` are in.
    fn agent_sharing(places: Option<Arc<Places>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            serve(
                listener,
                KEY.into(),
                loopback(),
                places,
                Arc::new(|| {
                    Ok(Arc::new(|(width, _): (usize, usize)| match width {
                        32 => Ok(Box::new(Broken) as Box<dyn Backend>),
                        48 => anyhow::bail!("no encoder takes a picture 48 across"),
                        _ => Ok(Box::new(Echo) as Box<dyn Backend>),
                    }) as Arc<Streams>)
                }),
            )
        });
        address
    }

    /// The agent's copy of a picture is the daemon's, sample for sample,
    /// through frames that change it here and there, frames that change
    /// nothing, and what each is asked: a keyframe, a refinement, a
    /// quantizer. A picture of odd sides, coded a pixel larger, its rows
    /// sent one by one; one as wide as its planes' stride, whose whole
    /// rows are sent as one run of each plane; and a screen's, whose first
    /// picture is sent zero-copy where the account may pin it, and copied
    /// from then on, loopback having copied it ([`crate::agent::zerocopy`]).
    /// And each again in memory the two ends share, the rows written there.
    #[test]
    fn the_agents_picture_is_the_daemons() {
        for shared in [false, true] {
            the_agents_picture_is_the_daemons_at(149, 89, shared);
            let across = Planes::new(128, 70);
            assert_eq!(across.stride, across.coded.0);
            the_agents_picture_is_the_daemons_at(128, 70, shared);
            the_agents_picture_is_the_daemons_at(1920, 1080, shared);
        }
    }

    /// The two ends of memory shared in a file of `bytes`, each its own
    /// mapping of it, as each machine has: the daemon's region and the
    /// agent's places.
    fn shared(name: &str, bytes: usize) -> (Scratch, Arc<Region>, Arc<Places>) {
        let file = Scratch::new(name, bytes as u64);
        let daemon = Region::open(&file.0).unwrap();
        let agent = Places::new(Region::open(&file.0).unwrap());
        (file, daemon, agent)
    }

    fn the_agents_picture_is_the_daemons_at(width: u16, height: u16, sharing: bool) {
        let size = Planes::coded_size(width, height);
        let (_file, agent) = if sharing {
            let (file, region, places) = shared("picture", picture_bytes(size));
            let agent = Agent::new(agent_sharing(Some(places)), KEY.to_vec()).sharing(region);
            (Some(file), agent)
        } else {
            (None, Agent::new(agent(), KEY.to_vec()))
        };
        let mut planes = Planes::new(width, height);
        let (w, h) = planes.coded;
        let mut stream = Stream::open(&agent, planes.coded).unwrap();
        assert_eq!(stream.shares(), sharing);
        let mut state = 0x2545_F491u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as usize
        };
        for sample in planes.y.iter_mut().chain(&mut planes.uv) {
            *sample = next() as u8;
        }
        for n in 0..60 {
            // Nothing every fourth frame, and up to three rectangles the
            // rest, one in five of them the picture's whole width.
            for _ in 0..(n % 4) {
                let (x, y) = (next() % (w / 2) * 2, next() % (h / 2) * 2);
                let (rw, rh) = (2 + next() % 30 * 2, 2 + next() % 20 * 2);
                let (x, rw) = if next() % 5 == 0 { (0, w) } else { (x, rw) };
                let (right, bottom) = ((x + rw).min(w), (y + rh).min(h));
                for row in y..bottom {
                    for sample in &mut planes.y[row * planes.stride + x..][..right - x] {
                        *sample = next() as u8;
                    }
                }
                for row in y / 2..bottom / 2 {
                    for sample in &mut planes.uv[row * planes.stride + x..][..right - x] {
                        *sample = next() as u8;
                    }
                }
                planes.touch((x, y), (right, bottom));
            }
            planes.refine = n % 7 == 3;
            let (keyframe, qp) = (n % 11 == 5, (16 + n % 30) as u8);
            let (mut got, mut expected) = (Vec::new(), Vec::new());
            assert!(stream.frame(&mut planes, keyframe, qp, &mut got).unwrap());
            echo(&planes, keyframe, qp, &mut expected);
            assert!(got == expected, "frame {n}: the agent's picture is another");
            // Only a picture large enough goes zero-copy, and only it is
            // copied by loopback all the same; where its calls may be
            // refused, they may be copied from the first.
            if sharing {
            } else if w * h < 1 << 20 {
                assert!(stream.zero_copy(), "frame {n}");
            } else if crate::agent::zerocopy::unlimited() {
                assert!(!stream.zero_copy(), "frame {n}");
            }
            planes.coded_frame();
        }
    }

    /// A frame the agent never reads fails in its time, and the planes it
    /// was sent from are the caller's again at once, as they were, while
    /// the kernel still holds their pages: their buffers are kept until it
    /// lets go, which it does here only once the agent's end goes, and the
    /// planes are given copies.
    #[test]
    fn a_frame_never_read_fails_in_its_time_and_keeps_its_planes() {
        let listener =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        listener.set_recv_buffer_size(64 << 10).unwrap();
        listener
            .bind(
                &"127.0.0.1:0"
                    .parse::<std::net::SocketAddr>()
                    .unwrap()
                    .into(),
            )
            .unwrap();
        listener.listen(1).unwrap();
        let listener: TcpListener = listener.into();
        let address = listener.local_addr().unwrap();
        let (over, ended) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (link, _) = listener.accept().unwrap();
            let (mut to, mut from) = (&link, &link);
            let ours = nonce();
            to.write_all(&hello(&ours)).unwrap();
            let theirs = read_hello(&mut from).unwrap();
            let proved = proof(&KEY, Role::Agent, &theirs, &ours).unwrap();
            to.write_all(&proved.finalize().into_bytes()).unwrap();
            read_open_request(&mut from).unwrap();
            to.write_all(&opened(None)).unwrap();
            // Nothing more is read until the test says, and then the
            // connection goes with what it never read.
            let _ = ended.recv();
        });
        let agent = Agent::new(address.to_string(), KEY.to_vec());
        let mut planes = Planes::new(1920, 1080);
        for (n, sample) in planes.y.iter_mut().chain(&mut planes.uv).enumerate() {
            *sample = (n * 7 + n / 4093) as u8;
        }
        let before = (planes.y.clone(), planes.uv.clone());
        let at = planes.y.as_ptr();
        let mut stream = Stream::open(&agent, planes.coded).unwrap();
        stream.frames_within(Duration::from_millis(300));
        let (freed, was_freed) = std::sync::mpsc::channel();
        stream.hold_cleanup(freed);
        // The frame on a thread of its own, so that one that waits on its
        // pages fails the test rather than hanging it.
        let (returned, has_returned) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let why = stream.frame(&mut planes, true, 28, &mut Vec::new()).err();
            returned.send((why, planes, stream)).unwrap();
        });
        let (why, planes, _stream) = has_returned
            .recv_timeout(Duration::from_secs(2))
            .expect("the frame waited for the kernel to let go of its pages");
        let why = format!("{:#}", why.expect("a frame never read is no frame"));
        assert!(why.contains("not done within"), "{why}");
        assert!(
            planes.y == before.0 && planes.uv == before.1,
            "the planes are not as they were"
        );
        let handed = planes.y.as_ptr() != at;
        if crate::agent::zerocopy::unlimited() {
            assert!(handed, "the buffers the kernel holds were not kept");
        }
        if handed {
            // Kept for as long as the kernel holds them, past the frame.
            assert!(was_freed.recv_timeout(Duration::from_millis(200)).is_err());
            drop(over);
            was_freed
                .recv_timeout(Duration::from_secs(2))
                .expect("the buffers were never freed");
        }
    }

    /// A daemon whose key is not the agent's is told so before it sends a
    /// picture, the agent serving on; and one whose agent is not there is
    /// told where it looked.
    #[test]
    fn an_agent_of_another_key_or_none_fails_the_stream() {
        let address = agent();
        let stranger = Agent::new(address.clone(), vec![0x3c; MIN_KEY]);
        let why = format!("{:#}", Stream::open(&stranger, (64, 64)).err().unwrap());
        assert!(why.contains(&address) && why.contains("key"), "{why}");
        Stream::open(&Agent::new(address, KEY.to_vec()), (64, 64)).unwrap();

        // A port nobody listens on, held for as long as the test looks: one
        // bound and let go could be another test's agent's by then.
        let held =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        held.bind(&std::net::SocketAddr::from(([127, 0, 0, 1], 0)).into())
            .unwrap();
        let gone = held.local_addr().unwrap().as_socket().unwrap().to_string();
        let nobody = Agent::new(gone.clone(), KEY.to_vec());
        let why = format!("{:#}", Stream::open(&nobody, (64, 64)).err().unwrap());
        assert!(why.contains(&gone), "{why}");
    }

    /// A daemon looks its agent's name up again after a stream that could
    /// not be opened where the name was last found, and not before: an
    /// agent that has moved fails one stream and has the next.
    #[test]
    fn an_agent_that_moved_is_looked_up_again() {
        use std::net::SocketAddr;
        // Where the agent was: nobody listening, for as long as the test
        // looks.
        let held =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        held.bind(&SocketAddr::from(([127, 0, 0, 1], 0)).into())
            .unwrap();
        let was = held.local_addr().unwrap().as_socket().unwrap();
        let moved = Agent::last_found_at(agent(), was, KEY.to_vec());
        let why = format!("{:#}", Stream::open(&moved, (64, 64)).err().unwrap());
        assert!(why.contains("connecting"), "{why}");
        Stream::open(&moved, (64, 64)).unwrap();
    }

    /// An encoder that will not open, or fails a frame, fails the daemon's
    /// stream in the encoder's own words.
    #[test]
    fn the_agents_encoder_failing_fails_the_stream_with_its_reason() {
        let agent = Agent::new(agent(), KEY.to_vec());
        let why = format!("{:#}", Stream::open(&agent, (48, 32)).err().unwrap());
        assert!(
            why.contains("no encoder takes a picture 48 across"),
            "{why}"
        );

        let mut stream = Stream::open(&agent, (32, 32)).unwrap();
        let mut out = vec![1, 2, 3];
        let why = stream
            .frame(&mut Planes::new(32, 32), false, 28, &mut out)
            .unwrap_err();
        assert!(
            format!("{why:#}").contains("the encoder is unplugged"),
            "{why:#}"
        );
        assert_eq!(out, [1, 2, 3], "nothing of a failed frame is kept");
        // And no frame is sent on the stream after it.
        let why = stream
            .frame(&mut Planes::new(32, 32), false, 28, &mut out)
            .unwrap_err();
        assert!(
            format!("{why:#}").contains("failed a frame before"),
            "{why:#}"
        );
    }

    /// What makes a daemon its streams in these tests, coding by [`Echo`],
    /// and how many daemons it has made them for.
    fn echoes() -> (Arc<Open>, Arc<AtomicUsize>) {
        let made = Arc::new(AtomicUsize::new(0));
        let open: Arc<Open> = Arc::new({
            let made = made.clone();
            move || {
                made.fetch_add(1, Ordering::SeqCst);
                Ok(Arc::new(|_| Ok(Box::new(Echo) as Box<dyn Backend>)) as Arc<Streams>)
            }
        });
        (open, made)
    }

    /// An agent within `limits`, coding by [`Echo`], and where it listens.
    fn agent_within(limits: Limits) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            accept(listener, KEY.into(), loopback(), None, echoes().0, limits)
        });
        address
    }

    /// A stream of `agent`'s once the agent lets its daemon have one: what
    /// closed before it is the agent's thread's to notice, in its own time.
    fn in_time(agent: &Agent) -> Stream {
        let began = std::time::Instant::now();
        loop {
            match Stream::open(agent, (64, 64)) {
                Ok(stream) => return stream,
                Err(e) => assert!(began.elapsed() < Duration::from_secs(10), "{e:#}"),
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A connection to the agent on loopback's `port`, made from loopback's
    /// address `host`: a daemon at 127.0.0.1 and another at 127.0.0.2.
    fn from_host(host: u8, port: u16) -> TcpStream {
        use std::net::SocketAddr;
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        socket
            .bind(&SocketAddr::from(([127, 0, 0, host], 0)).into())
            .unwrap();
        socket
            .connect(&SocketAddr::from(([127, 0, 0, 1], port)).into())
            .unwrap();
        socket.into()
    }

    /// A daemon's handshake on `link`, by hand, asking for a stream over a
    /// `width`×`height` picture: the agent's status, or the error of a
    /// connection the agent closed on it.
    fn by_hand(mut link: &TcpStream, width: u16, height: u16) -> std::io::Result<u8> {
        let ours = nonce();
        link.write_all(&hello(&ours))?;
        let theirs = read_hello(&mut link).map_err(std::io::Error::other)?;
        let mut proved = [0; PROOF];
        link.read_exact(&mut proved)?;
        proof(&KEY, Role::Agent, &ours, &theirs)
            .unwrap()
            .verify_slice(&proved)
            .expect("the agent's proof");
        link.write_all(
            &proof(&KEY, Role::Daemon, &ours, &theirs)
                .unwrap()
                .finalize()
                .into_bytes(),
        )?;
        // No memory shared.
        link.write_all(&[&width.to_be_bytes()[..], &height.to_be_bytes(), &[0; 8]].concat())?;
        let mut status = [0];
        link.read_exact(&mut status)?;
        if status[0] == OK {
            // Where its picture is in shared memory: nowhere.
            let mut at = [0; 8];
            link.read_exact(&mut at)?;
            assert_eq!(u64::from_be_bytes(at), crate::agent::link::SOCKET);
        }
        Ok(status[0])
    }

    /// An agent is one daemon's at a time. With a stream of one open, a
    /// connection from another address is closed unanswered, whoever it is
    /// and whatever key it has; the daemon's own further streams are opened
    /// by what was made for its first. Its last stream closed, the agent is
    /// the next daemon's, and the first is the one kept out.
    #[test]
    fn an_agent_is_one_daemons_at_a_time() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (open, made) = echoes();
        let limits = Limits {
            streams: 8,
            unproved: 8,
            handshake: HANDSHAKE_WITHIN,
        };
        std::thread::spawn(move || accept(listener, KEY.into(), loopback(), None, open, limits));
        let one = Agent::new(format!("127.0.0.1:{port}"), KEY.to_vec());

        let streams = [in_time(&one), in_time(&one), in_time(&one)];
        assert_eq!(made.load(Ordering::SeqCst), 1, "made once for the daemon");
        for _ in 0..3 {
            let (said, took) = until_closed(&from_host(2, port));
            assert!(said.is_empty(), "not so much as a hello");
            assert!(took < Duration::from_secs(2), "{took:?}");
        }

        drop(streams);
        // The agent is nobody's once its threads have read the closes, and
        // then the daemon elsewhere is let in and is its user.
        let began = std::time::Instant::now();
        let _theirs = loop {
            let link = from_host(2, port);
            if let Ok(status) = by_hand(&link, 64, 64) {
                assert_eq!(status, OK);
                break link;
            }
            assert!(began.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(made.load(Ordering::SeqCst), 2, "and once for the next");
        assert!(Stream::open(&one, (64, 64)).is_err());
    }

    /// An agent lets a daemon in from where it allows and from nowhere
    /// else: a connection from another address is closed unanswered, with
    /// nobody using the agent and with the key in hand, and takes no
    /// handshake's place; and the daemon where the agent allows has its
    /// streams.
    #[test]
    fn an_agent_lets_in_whom_it_allows_and_nobody_else() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (open, made) = echoes();
        let limits = Limits {
            streams: 8,
            unproved: 2,
            handshake: Duration::from_secs(60),
        };
        let allowed = vec![
            "127.0.0.2/32".parse().unwrap(),
            "127.0.3.0/24".parse().unwrap(),
        ];
        std::thread::spawn(move || accept(listener, KEY.into(), allowed, None, open, limits));

        // A daemon the agent allows, at the hello and no further, which a
        // handshake begun elsewhere would close were it given a place.
        let waiting = from_host(2, port);
        read_hello(&mut &waiting).unwrap();
        for _ in 0..4 {
            let (said, took) = until_closed(&TcpStream::connect(("127.0.0.1", port)).unwrap());
            assert!(said.is_empty(), "not so much as a hello");
            assert!(took < Duration::from_secs(2), "{took:?}");
        }
        assert_eq!(by_hand(&from_host(2, port), 64, 64).unwrap(), OK);
        assert_eq!(made.load(Ordering::SeqCst), 1);
        waiting
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let waited = (&waiting).read(&mut [0]).unwrap_err();
        assert_eq!(waited.kind(), std::io::ErrorKind::WouldBlock, "still open");

        let door = |allowed: &[&str]| Door {
            limits,
            allowed: allowed.iter().map(|net| net.parse().unwrap()).collect(),
            places: None,
            open: echoes().0,
            unproved: Mutex::default(),
            user: Mutex::default(),
        };
        let two = door(&["127.0.0.2/32", "127.0.3.0/24", "fd00:1::/64"]);
        for (address, allowed) in [
            ("127.0.0.2", true),
            ("127.0.3.200", true),
            ("fd00:1::9", true),
            ("127.0.0.1", false),
            ("127.0.4.1", false),
            ("fd00:2::9", false),
        ] {
            assert_eq!(two.allows(address.parse().unwrap()), allowed, "{address}");
        }
        assert!(!door(&[]).allows("127.0.0.1".parse().unwrap()));
    }

    /// Two daemons that prove the key to an agent nobody is using are let
    /// in together, and the second to ask for a stream is told whose the
    /// agent is; as is the user, of one stream more than it may have.
    #[test]
    fn a_daemon_is_told_why_it_has_no_stream() {
        let door = Arc::new(Door {
            limits: Limits {
                streams: 2,
                unproved: 8,
                handshake: HANDSHAKE_WITHIN,
            },
            allowed: Vec::new(),
            places: None,
            open: echoes().0,
            unproved: Mutex::default(),
            user: Mutex::default(),
        });
        let [one, another] = [1, 2].map(|host| IpAddr::from([10, 0, 0, host]));
        let why =
            |opened: anyhow::Result<(Coding, Arc<Streams>)>| opened.err().unwrap().to_string();
        let first = door.coding(one).unwrap();
        assert!(why(door.coding(another)).contains("10.0.0.1"));
        assert_eq!(door.anothers(another), Some(one));
        assert_eq!(door.anothers(one), None);
        let second = door.coding(one).unwrap();
        assert!(why(door.coding(one)).contains("2 streams"));
        drop(first);
        assert!(why(door.coding(another)).contains("10.0.0.1"));
        drop(second);
        assert_eq!(door.anothers(another), None);
        let _theirs = door.coding(another).unwrap();
        assert!(why(door.coding(one)).contains("10.0.0.2"));
    }

    /// What is read of a connection until the agent closes it, and how long
    /// that took: a test that waits on an agent that never does fails.
    fn until_closed(mut link: &TcpStream) -> (Vec<u8>, Duration) {
        let began = std::time::Instant::now();
        link.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut said = Vec::new();
        // A connection closed under its bytes is reset, which is closed too.
        match link.read_to_end(&mut said) {
            Err(e) if e.kind() != std::io::ErrorKind::ConnectionReset => panic!("{e}"),
            _ => (said, began.elapsed()),
        }
    }

    /// An agent codes so many streams at once and tells the daemon that
    /// asks for one more that there is none; a stream closed gives its
    /// place to the next.
    #[test]
    fn an_agent_codes_so_many_streams_at_once() {
        let address = agent_within(Limits {
            streams: 2,
            unproved: 8,
            handshake: HANDSHAKE_WITHIN,
        });
        let agent = Agent::new(address, KEY.to_vec());
        let (first, _second) = (in_time(&agent), in_time(&agent));
        let why = format!("{:#}", Stream::open(&agent, (64, 64)).err().unwrap());
        assert!(why.contains("coding 2 streams"), "{why}");
        drop(first);
        let _third = in_time(&agent);
    }

    /// Connections that say nothing take no stream's place and keep no
    /// daemon out: with as many as an agent keeps of them, one more closes
    /// the oldest, and a daemon's stream opens through all of them.
    #[test]
    fn strangers_at_the_port_keep_no_daemon_out() {
        let address = agent_within(Limits {
            streams: 2,
            unproved: 3,
            handshake: Duration::from_secs(60),
        });
        let silent: Vec<TcpStream> = (0..5)
            .map(|_| {
                let link = TcpStream::connect(&address).unwrap();
                // Its hello read, so the agent has it among the unproved.
                read_hello(&mut &link).unwrap();
                link
            })
            .collect();
        // The first two were closed for the fourth and the fifth.
        for link in &silent[..2] {
            assert!(until_closed(link).0.is_empty());
        }
        let agent = Agent::new(address, KEY.to_vec());
        let _streams = [
            Stream::open(&agent, (64, 64)).unwrap(),
            Stream::open(&agent, (64, 64)).unwrap(),
        ];
        // The first stream's handshake took the third's place and gave it
        // back, which left the second's a place of its own: the last two
        // stand as they were, with nothing closed that did not have to be.
        assert!(until_closed(&silent[2]).0.is_empty());
        for mut link in &silent[3..] {
            link.set_read_timeout(Some(Duration::from_millis(100)))
                .unwrap();
            let waited = link.read(&mut [0]).unwrap_err();
            assert_eq!(waited.kind(), std::io::ErrorKind::WouldBlock, "still open");
        }
    }

    /// A connection that has not proved the key within the handshake's time
    /// is closed, however it spaces what it sends.
    #[test]
    fn a_handshake_is_done_in_its_time_or_not_at_all() {
        let address = agent_within(Limits {
            streams: 2,
            unproved: 8,
            handshake: Duration::from_millis(300),
        });
        let link = TcpStream::connect(&address).unwrap();
        read_hello(&mut &link).unwrap();
        // A hello a byte at a time, each well inside any timeout of a read.
        let dripped = hello(&nonce());
        let dripping = std::thread::spawn({
            let mut link = link.try_clone().unwrap();
            move || {
                for byte in dripped {
                    if link.write_all(&[byte]).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        });
        // The agent's proof, which it sends on reading a hello, never comes.
        let (said, took) = until_closed(&link);
        assert!(said.is_empty(), "{} bytes", said.len());
        assert!(took < Duration::from_secs(2), "{took:?}");
        link.shutdown(Shutdown::Both).ok();
        dripping.join().unwrap();
    }

    /// What a daemon with the key may ask is still held to the picture: a
    /// picture of no size, odd sides or past the longest side is refused
    /// with a word, and a rectangle outside the picture fails the stream
    /// with one.
    #[test]
    fn an_agent_refuses_what_is_no_part_of_the_picture() {
        let address = agent();
        let open = |width: u16, height: u16| {
            let link = TcpStream::connect(&address).unwrap();
            let status = by_hand(&link, width, height).unwrap();
            (link, status)
        };
        let why = |link: &mut TcpStream| {
            let mut rest = Vec::new();
            link.read_to_end(&mut rest).unwrap();
            assert_eq!(
                usize::from(u16::from_be_bytes([rest[0], rest[1]])),
                rest.len() - 2
            );
            String::from_utf8(rest.split_off(2)).unwrap()
        };

        for (width, height) in [(0, 64), (64, 63), (8194, 64)] {
            let (mut link, status) = open(width, height);
            assert_eq!(status, FAILED, "{width}x{height}");
            assert!(why(&mut link).contains("picture"));
        }

        let (mut link, status) = open(64, 48);
        assert_eq!(status, OK);
        // One rectangle, two rows below the picture's last.
        let mut frame = vec![FRAME, 0, 28, 0, 0, 0, 1];
        for n in [0u16, 34, 64, 16] {
            frame.extend_from_slice(&n.to_be_bytes());
        }
        link.write_all(&frame).unwrap();
        let mut status = [0];
        link.read_exact(&mut status).unwrap();
        assert_eq!(status[0], FAILED);
        let why = why(&mut link);
        assert!(why.contains("no part of a 64x48 picture"), "{why}");
    }

    /// A stream the shared memory has no room for, its places held, has its
    /// pictures over the connection, and the place a stream held is
    /// another's once it ends.
    #[test]
    fn a_stream_with_no_room_crosses_the_connection() {
        let size = (64, 64);
        let (_file, region, places) = shared("room", picture_bytes(size));
        let agent = Agent::new(agent_sharing(Some(places)), KEY.to_vec()).sharing(region);
        let first = Stream::open(&agent, size).unwrap();
        assert!(first.shares());
        let mut second = Stream::open(&agent, size).unwrap();
        assert!(!second.shares());
        let mut planes = Planes::new(64, 64);
        planes.y.fill(77);
        let mut got = Vec::new();
        assert!(second.frame(&mut planes, true, 30, &mut got).unwrap());
        let mut expected = Vec::new();
        echo(&planes, true, 30, &mut expected);
        assert!(got == expected);
        drop(first);
        // Its thread ends as the connection does, a moment after.
        let began = std::time::Instant::now();
        while !Stream::open(&agent, size).unwrap().shares() {
            assert!(
                began.elapsed() < Duration::from_secs(5),
                "the place is still held"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Ends that do not share the same memory are told so as the stream
    /// opens: a daemon sharing memory with an agent that shares none, two
    /// of other lengths, and two of one length that are other memory.
    #[test]
    fn ends_sharing_other_memory_are_told_so() {
        let size = (64, 64);
        let bytes = picture_bytes(size);
        let (_daemons, region, _) = shared("daemons", bytes);
        let why = |agent: Agent| format!("{:#}", Stream::open(&agent, size).err().unwrap());
        let none = Agent::new(agent(), KEY.to_vec()).sharing(region.clone());
        assert!(why(none).contains("--shared-memory"));
        let (_longer, _, longer) = shared("longer", 2 * bytes);
        let other = Agent::new(agent_sharing(Some(longer)), KEY.to_vec()).sharing(region.clone());
        assert!(why(other).contains("not the same memory"));
        let (_agents, _, agents) = shared("agents", bytes);
        let other = Agent::new(agent_sharing(Some(agents.clone())), KEY.to_vec()).sharing(region);
        assert!(why(other).contains("other memory"));
        // And a daemon that shares none with an agent that does sends its
        // pictures over the connection.
        let plain = Agent::new(agent_sharing(Some(agents)), KEY.to_vec());
        assert!(!Stream::open(&plain, size).unwrap().shares());
    }
}
