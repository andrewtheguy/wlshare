//! The daemon's end of the link to an encoding agent ([`crate::agent::link`]): where
//! the agent is, and a stream on it as the encoder a picture is handed to.
//!
//! A stream is a connection: opened, each end's key proved to the other, and
//! asked for an encoder over the picture's size; then every frame is what
//! changed of the picture, sent, and the access unit, read back. A stream
//! dropped closes its connection, which ends the agent's encoder.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;

use crate::agent::link::{
    FAILED, FRAME, IO, KEYFRAME, MAX_SIDE, Mac as _, NONCE, NONE, OK, PROOF, REFINE, Role, Span,
    hello, nonce, proof, read_hello,
};
use crate::h264::{Backend, Planes};

/// How long a connection to the agent may take to open.
const CONNECT: Duration = Duration::from_secs(3);

/// What a read wrote into since the last frame: the macroblocks no frame has
/// coded as they stand, as rectangles of the coded picture, a row's run of
/// them joined to the same run of the row above.
fn changed(planes: &Planes) -> Vec<Span> {
    let (width, height) = planes.coded;
    let mut spans: Vec<Span> = Vec::new();
    // The rectangles that reach down to the row at hand, by where they are
    // in `spans`: the ones a run of this row can carry on.
    let mut reaching: Vec<usize> = Vec::new();
    for (row, still) in planes.still.chunks(planes.across).enumerate() {
        let top = row * 16;
        let bottom = (top + 16).min(height);
        let mut carried = Vec::new();
        let mut at = 0;
        while at < still.len() {
            if still[at] != 0 {
                at += 1;
                continue;
            }
            let run = still[at..].iter().take_while(|s| **s == 0).count();
            let (x, w) = (at * 16, ((at + run) * 16).min(width) - at * 16);
            at += run;
            match reaching
                .iter()
                .find(|&&n| spans[n].x == x && spans[n].w == w)
            {
                Some(&n) => {
                    spans[n].h = bottom - spans[n].y;
                    carried.push(n);
                }
                None => {
                    carried.push(spans.len());
                    spans.push(Span {
                        x,
                        y: top,
                        w,
                        h: bottom - top,
                    });
                }
            }
        }
        reaching = carried;
    }
    spans
}

/// An encoding agent as the daemon is configured with it: where it listens, and
/// the key it shares.
#[derive(Clone)]
pub struct Agent {
    address: String,
    key: Arc<[u8]>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is nobody's to read in a log.
        f.debug_struct("Agent")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// The agent at `address`, a host and port, sharing `key`.
    pub fn new(address: String, key: Vec<u8>) -> Self {
        Self {
            address,
            key: key.into(),
        }
    }

    /// Where the agent listens, as configured.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// A connection to the agent, each end's key proved to the other.
    fn connect(&self) -> anyhow::Result<(TcpStream, [u8; NONCE], [u8; NONCE])> {
        let mut refused = None;
        let mut link = None;
        for address in self
            .address
            .to_socket_addrs()
            .context("looking its address up")?
        {
            match TcpStream::connect_timeout(&address, CONNECT) {
                Ok(stream) => {
                    link = Some(stream);
                    break;
                }
                Err(e) => refused = Some(e),
            }
        }
        let mut link = match (link, refused) {
            (Some(link), _) => link,
            (None, Some(e)) => return Err(e).context("connecting"),
            (None, None) => anyhow::bail!("its name has no address"),
        };
        link.set_nodelay(true)?;
        link.set_read_timeout(Some(IO))?;
        link.set_write_timeout(Some(IO))?;
        let ours = nonce();
        link.write_all(&hello(&ours)).context("sending the hello")?;
        let theirs = read_hello(&mut link)?;
        let mut proved = [0; PROOF];
        link.read_exact(&mut proved)
            .context("reading its proof of the key")?;
        proof(&self.key, Role::Agent, &ours, &theirs)?
            .verify_slice(&proved)
            .map_err(|_| anyhow::anyhow!("its key is not this daemon's"))?;
        Ok((link, ours, theirs))
    }
}

/// One stream on an agent: the connection it is coded over.
pub struct Stream {
    link: TcpStream,
    /// A frame's request, kept for the next one's bytes.
    request: Vec<u8>,
    /// The most bytes an access unit of this picture is taken to be.
    most: usize,
}

impl Stream {
    /// A stream over a `width`×`height` picture, both even, on `agent`.
    pub fn open(agent: &Agent, (width, height): (usize, usize)) -> anyhow::Result<Self> {
        let opened = (|| {
            anyhow::ensure!(
                width <= MAX_SIDE && height <= MAX_SIDE,
                "a {width}x{height} picture is past the {MAX_SIDE} pixels a side an agent takes"
            );
            let (mut link, ours, theirs) = agent.connect()?;
            let mut open = [0; PROOF + 4];
            open[..PROOF].copy_from_slice(
                &proof(&agent.key, Role::Daemon, &ours, &theirs)?
                    .finalize()
                    .into_bytes(),
            );
            open[PROOF..PROOF + 2].copy_from_slice(&(width as u16).to_be_bytes());
            open[PROOF + 2..].copy_from_slice(&(height as u16).to_be_bytes());
            link.write_all(&open).context("asking for a stream")?;
            status(&mut link).context("opening a stream")?;
            Ok(Self {
                link,
                request: Vec::new(),
                // Raw 4:2:0 is 384 bytes a macroblock, and the agent's own
                // buffer is that and headroom ([`crate::h264::vaapi::Stream::open`]).
                most: width.div_ceil(16) * height.div_ceil(16) * 400 + 64 * 1024,
            })
        })();
        opened.with_context(|| format!("the encoding agent at {}", agent.address))
    }
}

/// Read the agent's status byte: `true` for [`OK`], `false` for [`NONE`], and
/// for [`FAILED`] the reason it gives, as the error.
fn status(link: &mut TcpStream) -> anyhow::Result<bool> {
    let mut status = [0];
    link.read_exact(&mut status)
        .context("waiting for its answer")?;
    match status[0] {
        OK => Ok(true),
        NONE => Ok(false),
        FAILED => {
            let mut length = [0; 2];
            link.read_exact(&mut length)?;
            let mut why = vec![0; usize::from(u16::from_be_bytes(length))];
            link.read_exact(&mut why)?;
            anyhow::bail!("{}", String::from_utf8_lossy(&why))
        }
        other => anyhow::bail!("it answered {other}, which is no status"),
    }
}

impl Backend for Stream {
    fn frame(
        &mut self,
        planes: &Planes,
        keyframe: bool,
        qp: u8,
        out: &mut Vec<u8>,
    ) -> anyhow::Result<bool> {
        let spans = changed(planes);
        let request = &mut self.request;
        request.clear();
        request.extend_from_slice(&[
            FRAME,
            if keyframe { KEYFRAME } else { 0 } | if planes.refine { REFINE } else { 0 },
            qp,
        ]);
        request.extend_from_slice(&(spans.len() as u32).to_be_bytes());
        for span in &spans {
            request.extend_from_slice(&span.header());
            for (plane, rows) in [
                (&planes.y, span.y..span.y + span.h),
                (&planes.uv, span.y / 2..(span.y + span.h) / 2),
            ] {
                for row in rows {
                    request.extend_from_slice(&plane[row * planes.stride + span.x..][..span.w]);
                }
            }
        }
        let coded = (|| {
            self.link
                .write_all(request)
                .context("sending it a picture")?;
            if !status(&mut self.link)? {
                return Ok(false);
            }
            let mut length = [0; 4];
            self.link.read_exact(&mut length)?;
            let length = u32::from_be_bytes(length) as usize;
            anyhow::ensure!(
                length <= self.most,
                "its frame of {length} bytes is more than the picture's own"
            );
            let start = out.len();
            out.resize(start + length, 0);
            if let Err(e) = self.link.read_exact(&mut out[start..]) {
                out.truncate(start);
                return Err(e).context("reading its frame");
            }
            Ok(true)
        })();
        coded.context("the encoding agent")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Planes whose macroblocks have all been coded as they stand.
    fn coded(width: u16, height: u16) -> Planes {
        let mut planes = Planes::new(width, height);
        planes.still.fill(1);
        planes
    }

    #[test]
    fn a_new_picture_is_sent_whole_in_one_rectangle() {
        let planes = Planes::new(100, 70);
        assert_eq!(
            changed(&planes),
            [Span {
                x: 0,
                y: 0,
                w: 100,
                h: 70
            }]
        );
        assert!(changed(&coded(100, 70)).is_empty());
    }

    /// Macroblocks touched are sent as whole macroblocks, clipped to the
    /// coded picture, and a run joins the same run above it and no other.
    #[test]
    fn what_changed_is_whole_macroblocks_joined_across_and_down() {
        let mut planes = coded(100, 70);
        // One sample in the second macroblock of the second row.
        planes.touch((20, 20), (22, 22));
        assert_eq!(
            changed(&planes),
            [Span {
                x: 16,
                y: 16,
                w: 16,
                h: 16
            }]
        );
        // A column two macroblocks wide down the right edge, and the bottom
        // row's first macroblock: the picture's last column and row are
        // partial macroblocks.
        let mut planes = coded(100, 70);
        planes.touch((70, 0), (100, 70));
        planes.touch((0, 66), (2, 70));
        assert_eq!(
            changed(&planes),
            [
                Span {
                    x: 64,
                    y: 0,
                    w: 36,
                    h: 70
                },
                Span {
                    x: 0,
                    y: 64,
                    w: 16,
                    h: 6
                }
            ]
        );
        // A run under one of another width starts a rectangle of its own,
        // and a rectangle a row did not carry on is not picked up again.
        let mut planes = coded(100, 70);
        planes.touch((0, 0), (32, 16));
        planes.touch((0, 16), (16, 32));
        planes.touch((0, 48), (32, 64));
        assert_eq!(
            changed(&planes),
            [
                Span {
                    x: 0,
                    y: 0,
                    w: 32,
                    h: 16
                },
                Span {
                    x: 0,
                    y: 16,
                    w: 16,
                    h: 16
                },
                Span {
                    x: 0,
                    y: 48,
                    w: 32,
                    h: 16
                }
            ]
        );
    }

    /// Every sample of what changed is in exactly one rectangle, however the
    /// macroblocks fall.
    #[test]
    fn the_rectangles_cover_what_changed_once() {
        let (w, h) = (150u16, 90u16);
        let mut state = 0x2545_F491u32;
        for _ in 0..200 {
            let mut planes = coded(w, h);
            for still in &mut planes.still {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *still = u8::from(state.is_multiple_of(3));
            }
            let mut covered = vec![0u8; planes.still.len()];
            for span in changed(&planes) {
                assert!(span.x % 16 == 0 && span.y % 16 == 0, "{span:?}");
                assert!(span.x + span.w <= 150 && span.y + span.h <= 90, "{span:?}");
                for row in span.y / 16..(span.y + span.h).div_ceil(16) {
                    for column in span.x / 16..(span.x + span.w).div_ceil(16) {
                        covered[row * planes.across + column] += 1;
                    }
                }
            }
            for (covered, still) in covered.iter().zip(&planes.still) {
                assert_eq!(*covered, u8::from(*still == 0));
            }
        }
    }
}
