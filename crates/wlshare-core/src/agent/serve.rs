//! The agent's end of the link ([`crate::agent::link`]): connections accepted, each
//! one stream, coded by the encoder its machine has. Experimental.
//!
//! Each connection has a thread of its own, the picture as the daemon last
//! sent it, and an encoder over it, which the agent's binary opens on its
//! own VA display on the device: what a session of a daemon on this machine
//! would have. A connection that fails its handshake is closed without a
//! word, and one whose stream fails is told why and closed, which fails the
//! daemon's stream and its session.

use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use log::{info, warn};

use crate::agent::link::{
    FRAME, IO, KEYFRAME, MAX_SIDE, Mac as _, NONE, OK, PROOF, REFINE, Role, Span, failed, hello,
    nonce, proof, read_hello,
};
use crate::h264::{Backend, Planes};

/// How long a daemon that has gone without a word is waited on before its
/// stream is closed: the connection is idle for as long as the desktop is
/// still, so only the keepalive tells. Half of it before the first probe,
/// and the rest [`PROBES`] probes unanswered, the count said and not left
/// to the system's, which on Linux is nine. And as long for what was sent
/// to be acknowledged, since a connection with a frame in flight is not
/// idle and is not probed: there it is the retransmissions that tell, which
/// left to the system go on for a quarter of an hour.
const KEEPALIVE: Duration = Duration::from_secs(30);

/// The keepalive probes that go unanswered before a daemon is taken for gone.
const PROBES: u32 = 3;

/// The most connections an agent has at once, each a thread, a socket and,
/// past its handshake, an encoder on the GPU: a session's video is one stream
/// and its tiles' regions a few, so this is several sessions' of several
/// daemons, and a bound on what whoever reaches the port can make it hold
/// without the key.
pub const STREAMS: usize = 32;

/// How much of a picture is read from the connection at a time.
const READ_AHEAD: usize = 256 * 1024;

/// What opens an encoder over a coded picture of even sides, for one stream.
pub type Open = dyn Fn((usize, usize)) -> anyhow::Result<Box<dyn Backend>> + Send + Sync;

/// Accept connections on `listener` for as long as it stands, each a stream
/// for whoever holds `key`, coded on a thread of its own by the encoder
/// `open` makes it: [`STREAMS`] of them at once, and a connection past that
/// closed unanswered.
pub fn serve(listener: TcpListener, key: Arc<[u8]>, open: Arc<Open>) {
    accept(listener, key, open, STREAMS);
}

/// [`serve`], with at most `most` connections at once.
fn accept(listener: TcpListener, key: Arc<[u8]>, open: Arc<Open>, most: usize) {
    // A connection's place among the `most` is a clone of this, held by its
    // thread and so given back however the thread ends, a handshake that
    // fails and a thread that never started included: the places taken are
    // the clones there are.
    let places = Arc::new(());
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                warn!("accepting a connection: {e}");
                continue;
            }
        };
        let peer = stream
            .peer_addr()
            .map_or_else(|_| "a daemon".to_owned(), |a| a.to_string());
        // Counted before there is a thread or a buffer for it. Only this
        // loop takes a place, so none is taken between the count and the
        // clone.
        if Arc::strong_count(&places) > most {
            warn!("{peer}: closed unanswered, with {most} connections open already");
            continue;
        }
        let (open, key, place) = (open.clone(), key.clone(), places.clone());
        let spawned = std::thread::Builder::new()
            .name("enc-agent".to_owned())
            .spawn(move || {
                let _place = place;
                match connection(&stream, &*open, &key, &peer) {
                    Ok(served) => info!("{peer}: its stream ended after {served}"),
                    Err(e) => warn!("{peer}: {e:#}"),
                }
            });
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

/// Have a daemon that is no longer there found out within [`KEEPALIVE`],
/// whether the connection stands idle or holds a frame it never acknowledged.
fn keepalive(stream: &TcpStream) -> std::io::Result<()> {
    let socket = socket2::SockRef::from(stream);
    socket.set_tcp_keepalive(
        &socket2::TcpKeepalive::new()
            .with_time(KEEPALIVE / 2)
            .with_interval(KEEPALIVE / 2 / PROBES)
            .with_retries(PROBES),
    )?;
    socket.set_tcp_user_timeout(Some(KEEPALIVE))
}

/// One connection: the handshake, the stream it asks for, and its frames
/// until the daemon closes it. What the stream came to.
fn connection(stream: &TcpStream, open: &Open, key: &[u8], peer: &str) -> anyhow::Result<Served> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(IO))?;
    stream.set_write_timeout(Some(IO))?;
    keepalive(stream)?;
    let mut from = BufReader::with_capacity(READ_AHEAD, stream);
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
    let mut asked = [0; PROOF + 4];
    from.read_exact(&mut asked)
        .context("reading its proof of the key")?;
    proof(key, Role::Daemon, &theirs, &ours)?
        .verify_slice(&asked[..PROOF])
        .map_err(|_| anyhow::anyhow!("its key is not this agent's"))?;
    let [width, height] =
        [PROOF, PROOF + 2].map(|at| usize::from(u16::from_be_bytes([asked[at], asked[at + 1]])));

    // The encoder before the planes, so a picture no encoder takes is
    // refused before it is made room for.
    let opened = (|| {
        anyhow::ensure!(
            width > 0
                && height > 0
                && width % 2 == 0
                && height % 2 == 0
                && width <= MAX_SIDE
                && height <= MAX_SIDE,
            "a {width}x{height} picture is not one an agent codes"
        );
        open((width, height))
    })();
    let mut backend = match opened {
        Ok(backend) => backend,
        Err(e) => {
            to.write_all(&failed(&format!("{e:#}")))?;
            return Err(e.context(format!("opening a {width}x{height} stream")));
        }
    };
    let mut planes = Planes::new(width as u16, height as u16);
    to.write_all(&[OK])?;
    info!("{peer}: a {width}x{height} stream");

    let mut served = Served::default();
    let mut reply = Vec::new();
    loop {
        // Between frames the daemon is owed nothing and may say nothing for
        // as long as its desktop stands still.
        stream.set_read_timeout(None)?;
        let mut tag = [0];
        match from.read(&mut tag) {
            Ok(0) => return Ok(served),
            Ok(_) => {}
            Err(e) => return Err(e).context("waiting for a frame"),
        }
        stream.set_read_timeout(Some(IO))?;
        reply.clear();
        reply.extend_from_slice(&[OK, 0, 0, 0, 0]);
        let coded = (|| {
            anyhow::ensure!(tag[0] == FRAME, "message {} is not a frame", tag[0]);
            let mut header = [0; 6];
            from.read_exact(&mut header).context("reading a frame")?;
            let [flags, qp, count @ ..] = header;
            anyhow::ensure!(qp <= 51, "{qp} is no H.264 quantizer");
            let stride = planes.stride;
            for _ in 0..u32::from_be_bytes(count) {
                let span = Span::read(&mut from, (width, height))?;
                for (plane, rows) in [
                    (&mut planes.y, span.y..span.y + span.h),
                    (&mut planes.uv, span.y / 2..(span.y + span.h) / 2),
                ] {
                    for row in rows {
                        from.read_exact(&mut plane[row * stride + span.x..][..span.w])
                            .context("reading a rectangle's rows")?;
                    }
                }
                planes.touch((span.x, span.y), (span.x + span.w, span.y + span.h));
                served.pictures += (span.w * span.h * 3 / 2) as u64;
            }
            planes.refine = flags & REFINE != 0;
            let began = std::time::Instant::now();
            let made = backend.frame(&planes, flags & KEYFRAME != 0, qp, &mut reply)?;
            served.coding += began.elapsed();
            if made {
                planes.coded_frame();
            }
            Ok(made)
        })();
        match coded {
            Ok(true) => {
                let length = (reply.len() - 5) as u32;
                reply[1..5].copy_from_slice(&length.to_be_bytes());
                to.write_all(&reply).context("sending a frame")?;
                served.frames += 1;
                served.stream += u64::from(length);
            }
            Ok(false) => to.write_all(&[NONE]).context("answering a frame")?,
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
    use crate::agent::link::{FAILED, MIN_KEY};

    /// The key the tests' agents and their daemons share.
    const KEY: [u8; MIN_KEY] = [0x5a; MIN_KEY];

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
            planes: &Planes,
            keyframe: bool,
            qp: u8,
            out: &mut Vec<u8>,
        ) -> anyhow::Result<bool> {
            echo(planes, keyframe, qp, out);
            Ok(true)
        }
    }

    impl Backend for Broken {
        fn frame(&mut self, _: &Planes, _: bool, _: u8, _: &mut Vec<u8>) -> anyhow::Result<bool> {
            anyhow::bail!("the encoder is unplugged")
        }
    }

    /// An agent on loopback for as long as the tests run, and where it
    /// listens: [`Echo`] for every picture but one 32 across, which gets
    /// [`Broken`], and one 48 across, which no encoder opens for.
    fn agent() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            serve(
                listener,
                KEY.into(),
                Arc::new(|(width, _)| match width {
                    32 => Ok(Box::new(Broken) as Box<dyn Backend>),
                    48 => anyhow::bail!("no encoder takes a picture 48 across"),
                    _ => Ok(Box::new(Echo) as Box<dyn Backend>),
                }),
            )
        });
        address
    }

    /// The agent's copy of a picture is the daemon's, sample for sample,
    /// through frames that change it here and there, frames that change
    /// nothing, and what each is asked: a keyframe, a refinement, a
    /// quantizer. A picture of odd sides, coded a pixel larger.
    #[test]
    fn the_agents_picture_is_the_daemons() {
        let agent = Agent::new(agent(), KEY.to_vec());
        let mut planes = Planes::new(149, 89);
        let (w, h) = planes.coded;
        let mut stream = Stream::open(&agent, planes.coded).unwrap();
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
            // Nothing every fourth frame, and up to three rectangles the rest.
            for _ in 0..(n % 4) {
                let (x, y) = (next() % (w / 2) * 2, next() % (h / 2) * 2);
                let (rw, rh) = (2 + next() % 30 * 2, 2 + next() % 20 * 2);
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
            assert!(stream.frame(&planes, keyframe, qp, &mut got).unwrap());
            echo(&planes, keyframe, qp, &mut expected);
            assert!(got == expected, "frame {n}: the agent's picture is another");
            planes.coded_frame();
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

        let gone = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .to_string();
        let nobody = Agent::new(gone.clone(), KEY.to_vec());
        let why = format!("{:#}", Stream::open(&nobody, (64, 64)).err().unwrap());
        assert!(why.contains(&gone), "{why}");
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
            .frame(&Planes::new(32, 32), false, 28, &mut out)
            .unwrap_err();
        assert!(
            format!("{why:#}").contains("the encoder is unplugged"),
            "{why:#}"
        );
        assert_eq!(out, [1, 2, 3], "nothing of a failed frame is kept");
    }

    /// A daemon gone without a word is found out within [`KEEPALIVE`], as
    /// the socket itself says it is set: the wait before the first probe and
    /// every probe after it, and the wait for what was sent to be
    /// acknowledged.
    #[test]
    fn a_daemon_gone_is_found_out_within_the_keepalive() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        keepalive(&stream).unwrap();
        let socket = socket2::SockRef::from(&stream);
        assert!(socket.keepalive().unwrap());
        assert_eq!(
            socket.tcp_keepalive_time().unwrap()
                + socket.tcp_keepalive_interval().unwrap()
                    * socket.tcp_keepalive_retries().unwrap(),
            KEEPALIVE
        );
        assert_eq!(socket.tcp_user_timeout().unwrap(), Some(KEEPALIVE));
    }

    /// An agent has so many connections at once and closes the next
    /// unanswered, which fails that stream; and a place is given back when
    /// its connection ends, a stream closed and a handshake failed alike.
    #[test]
    fn an_agent_has_so_many_connections_at_once() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            accept(
                listener,
                KEY.into(),
                Arc::new(|_| Ok(Box::new(Echo) as Box<dyn Backend>)),
                2,
            )
        });
        let agent = Agent::new(address.clone(), KEY.to_vec());
        // A stream, once the agent has taken back the place of whatever
        // closed before it: its thread ends when it reads the close.
        let open = || {
            let began = std::time::Instant::now();
            loop {
                match Stream::open(&agent, (64, 64)) {
                    Ok(stream) => return stream,
                    Err(e) => assert!(began.elapsed() < Duration::from_secs(10), "{e:#}"),
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        // Connections that fail their handshake, more of them than there
        // are places, leave every place there was.
        let stranger = Agent::new(address, vec![0x3c; MIN_KEY]);
        for _ in 0..4 {
            assert!(Stream::open(&stranger, (64, 64)).is_err());
        }
        let (first, _second) = (open(), open());
        assert!(
            Stream::open(&agent, (64, 64)).is_err(),
            "a third connection is closed unanswered"
        );
        drop(first);
        let _third = open();
    }

    /// What a daemon with the key may ask is still held to the picture: a
    /// picture of no size, odd sides or past the longest side is refused
    /// with a word, and a rectangle outside the picture fails the stream
    /// with one.
    #[test]
    fn an_agent_refuses_what_is_no_part_of_the_picture() {
        let address = agent();
        let open = |width: u16, height: u16| {
            let mut link = TcpStream::connect(&address).unwrap();
            let ours = nonce();
            link.write_all(&hello(&ours)).unwrap();
            let theirs = read_hello(&mut link).unwrap();
            let mut proved = [0; PROOF];
            link.read_exact(&mut proved).unwrap();
            proof(&KEY, Role::Agent, &ours, &theirs)
                .unwrap()
                .verify_slice(&proved)
                .expect("the agent's proof");
            link.write_all(
                &proof(&KEY, Role::Daemon, &ours, &theirs)
                    .unwrap()
                    .finalize()
                    .into_bytes(),
            )
            .unwrap();
            link.write_all(&[width.to_be_bytes(), height.to_be_bytes()].concat())
                .unwrap();
            let mut status = [0];
            link.read_exact(&mut status).unwrap();
            (link, status[0])
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
}
