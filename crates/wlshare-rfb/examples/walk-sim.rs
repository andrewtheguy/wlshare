//! The walk on a modelled link: captures played through the encoder and the
//! walk as a session plays them, one frame in flight, through the zDesk
//! gateway to a browser on a link of a given rate and round trip.
//!
//! ```text
//! walk-sim CAPTURE [CAPTURE...] [--link MBPS[,MBPS@SECS...]] [--rtt MS]
//!          [--hop MS] [--paint MS] [--jitter MS] [--ceiling Q] [--threads N]
//!          [--held] [--encode MS] [--timeline FILE]
//! ```
//!
//! `--link` is the browser's link in Mbit/s, 10 by default, and the rate it
//! changes to from each `@` second on; `--rtt` its round trip, 40 ms; `--hop`
//! the round trip between the session and the gateway, 1 ms; `--paint` how
//! long the browser takes to paint a frame once it has it, 5 ms, and
//! `--jitter` up to how much more, at random. `--ceiling` is the dial the
//! walk starts at and never passes, 90; `--held` holds it there, as a client
//! that lists `WLSD`. `--timeline` writes every frame as CSV.
//!
//! Time is the simulation's own: an encode takes what it took on this
//! machine, or `--encode` for a run that is the same every time; a frame
//! crosses the link at its rate after any frame before it; and the desktop is
//! the capture's frames at the times they were captured, so a session that
//! falls behind skips the frames it had no time for and carries their damage
//! in the next. Several captures play one after the other in one session.
//! What it says of a run is how long each change of the desktop took to be
//! painted, the frames a second, the dial the motion was coded at, the link
//! it used and how often the dial moved.

use std::fs::File;
use std::io::{BufReader, Read, Write as _};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wlshare_rfb::capture::Reader;
use wlshare_rfb::vp9::{Chroma, Rect, Vp9Encoder};
use wlshare_vp9::walk::QualityWalk;

type Error = Box<dyn std::error::Error>;

struct Desk {
    at: Duration,
    size: (u16, u16),
    keyframe: bool,
    changed: Option<Vec<Rect>>,
    pixels: Vec<u8>,
}

/// The captures, one after the other, each's times after the one before.
struct Source {
    paths: Vec<String>,
    reader: Option<Reader<BufReader<Box<dyn Read>>>>,
    offset: Duration,
    last: Duration,
    peeked: Option<Desk>,
}

impl Source {
    fn open(path: &str) -> Result<Reader<BufReader<Box<dyn Read>>>, Error> {
        let input: Box<dyn Read> = if path.ends_with(".zst") {
            let mut child = Command::new("zstd").arg("-dc").arg(path).stdout(Stdio::piped()).spawn()?;
            Box::new(child.stdout.take().unwrap())
        } else {
            Box::new(File::open(path)?)
        };
        Ok(Reader::new(BufReader::with_capacity(1 << 20, input))?)
    }

    fn peek(&mut self) -> Result<Option<&Desk>, Error> {
        while self.peeked.is_none() {
            if self.reader.is_none() {
                if self.paths.is_empty() {
                    return Ok(None);
                }
                let path = self.paths.remove(0);
                self.reader = Some(Self::open(&path)?);
                // A frame interval after the last capture's end.
                self.offset = self.last + Duration::from_millis(33);
            }
            let reader = self.reader.as_mut().unwrap();
            match reader.next()? {
                Some(f) => {
                    let at = self.offset + f.at;
                    self.last = at;
                    // A capture after the first starts on its own picture: the
                    // whole of it changed, though no keyframe is asked.
                    let first = f.index == 0;
                    self.peeked = Some(Desk {
                        at,
                        size: f.size,
                        keyframe: f.keyframe && !(first && self.offset > Duration::ZERO),
                        changed: if first { None } else { f.changed.map(<[Rect]>::to_vec) },
                        pixels: f.pixels.to_vec(),
                    });
                }
                None => self.reader = None,
            }
        }
        Ok(self.peeked.as_ref())
    }

    fn next_at(&mut self) -> Result<Option<Duration>, Error> {
        Ok(self.peek()?.map(|d| d.at))
    }

    fn take(&mut self) -> Desk {
        self.peeked.take().unwrap()
    }
}

/// A link of a rate that changes at given times, in bytes a second.
struct Link {
    rates: Vec<(Duration, f64)>,
    one_way: Duration,
    /// When the link is next idle.
    free: Duration,
}

impl Link {
    fn rate_at(&self, t: Duration) -> (f64, Option<Duration>) {
        let i = self.rates.iter().rposition(|(from, _)| *from <= t).unwrap_or(0);
        (self.rates[i].1, self.rates.get(i + 1).map(|(from, _)| *from))
    }

    /// When `bytes` written at `at` have all left the bottleneck.
    fn send(&mut self, at: Duration, bytes: usize) -> Duration {
        let mut t = at.max(self.free);
        let mut left = bytes as f64;
        loop {
            let (rate, until) = self.rate_at(t);
            let need = Duration::from_secs_f64(left / rate);
            match until {
                Some(until) if t + need > until => {
                    left -= rate * (until - t).as_secs_f64();
                    t = until;
                }
                _ => {
                    t += need;
                    break;
                }
            }
        }
        self.free = t;
        t
    }
}

/// The zDesk gateway between the session and the browser, as it passes a
/// `WLSV` stream (zDesk's `VideoSink::fence_hold`): a frame is handed to the
/// browser's link as it arrives, and its fence is echoed at once when nothing
/// handed before it is still unpainted, and otherwise once the oldest such
/// has been owed past the link's ping round trip, held 500 ms at most.
struct Gateway {
    link: Link,
    /// The round trip between the session and the gateway.
    hop: Duration,
    /// Frames handed over and not yet painted: when each was, and when its
    /// paint is acknowledged.
    owed: std::collections::VecDeque<(Duration, Duration)>,
    /// When the last echo went, which the next cannot overtake.
    echoed: Duration,
}

impl Gateway {
    /// A frame of `bytes` the session wrote at `sent`: when the browser has
    /// painted it, and when its fence's echo is back at the session.
    fn frame(&mut self, sent: Duration, bytes: usize, paint: Duration) -> (Duration, Duration) {
        let handed = sent + self.hop / 2;
        let painted = self.link.send(handed, bytes) + self.link.one_way + paint;
        while self.owed.front().is_some_and(|(_, acked)| *acked <= handed) {
            self.owed.pop_front();
        }
        let hold = self.owed.front().map_or(Duration::ZERO, |(at, _)| (handed - *at).saturating_sub(self.link.one_way * 2));
        self.owed.push_back((handed, painted + self.link.one_way));
        self.echoed = (handed + hold.min(Duration::from_millis(500))).max(self.echoed);
        (painted, self.echoed + self.hop / 2)
    }
}

fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// A flag's time in milliseconds, which is zero or more.
fn millis(flag: &str, value: String) -> Result<f64, Error> {
    let ms: f64 = value.parse()?;
    if !(ms.is_finite() && ms >= 0.0) {
        return Err(format!("{flag} {value}: a time of zero or more").into());
    }
    Ok(ms)
}

fn main() -> Result<(), Error> {
    let mut paths = Vec::new();
    let mut rates = vec![(Duration::ZERO, 10e6 / 8.0)];
    let (mut hop, mut held, mut encode_ms) = (1.0, false, None::<f64>);
    let (mut rtt, mut paint, mut jitter, mut ceiling, mut threads) = (40.0, 5.0, 0.0, 90u8, wlshare_rfb::vp9::encoder_threads());
    let mut timeline = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} takes a value"));
        match arg.as_str() {
            "--link" => {
                rates = value()?
                    .split(',')
                    .map(|step| {
                        let (rate, from) = step.split_once('@').unwrap_or((step, "0"));
                        let (rate, from) = (rate.parse::<f64>()?, from.parse::<f64>()?);
                        if !(rate.is_finite() && rate > 0.0 && from.is_finite() && from >= 0.0) {
                            return Err(format!("--link step {step}: a rate above zero and a time of zero or more").into());
                        }
                        Ok((Duration::from_secs_f64(from), rate * 1e6 / 8.0))
                    })
                    .collect::<Result<_, Error>>()?
            }
            "--rtt" => rtt = millis(&arg, value()?)?,
            "--hop" => hop = millis(&arg, value()?)?,
            "--held" => held = true,
            "--encode" => encode_ms = Some(millis(&arg, value()?)?),
            "--paint" => paint = millis(&arg, value()?)?,
            "--jitter" => jitter = millis(&arg, value()?)?,
            "--ceiling" => ceiling = value()?.parse()?,
            "--threads" => threads = value()?.parse()?,
            "--timeline" => timeline = Some(std::io::BufWriter::new(File::create(value()?)?)),
            _ if arg.starts_with("--") => return Err(format!("unknown flag {arg}").into()),
            _ => paths.push(arg),
        }
    }
    if paths.is_empty() {
        return Err("walk-sim takes a capture to play".into());
    }
    let name = paths.iter().map(|p| p.rsplit('/').next().unwrap().trim_end_matches(".wlcap.zst").to_string()).collect::<Vec<_>>().join("+");
    let link_name = rates.iter().map(|(from, rate)| format!("{}@{}", rate * 8.0 / 1e6, from.as_secs_f64())).collect::<Vec<_>>().join(",");
    let mut source = Source { paths, reader: None, offset: Duration::ZERO, last: Duration::ZERO, peeked: None };
    let one_way = Duration::from_secs_f64(rtt / 2e3);
    let link = Link { rates, one_way, free: Duration::ZERO };
    let mut gateway = Gateway { link, hop: Duration::from_secs_f64(hop / 1e3), owed: Default::default(), echoed: Duration::ZERO };
    let paint = Duration::from_secs_f64(paint / 1e3);
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut jitter = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        Duration::from_secs_f64((seed >> 11) as f64 / (1u64 << 53) as f64 * jitter / 1e3)
    };

    // A 60 Hz capture, the daemon's default.
    let base = Instant::now();
    let mut walk = QualityWalk::new(ceiling, Duration::from_micros(16_667), !held);
    let mut encoder: Option<Vp9Encoder> = None;
    let mut now = Duration::ZERO;
    // The desktop as it stands and what changed since the client's last frame.
    let mut desk: Option<Desk> = None;
    let mut dirty = false;
    let mut damage: Option<Vec<Rect>> = Some(Vec::new());
    let mut asked_key = false;
    let mut frame_sent: Option<Duration> = None;
    let mut out = Vec::new();
    let mut quality_was = walk.quality();
    let (mut qualities, mut deliveries) = (Vec::new(), Vec::new());
    // When each change of the desktop the client has not seen yet was made,
    // and how long each one took to be painted.
    let (mut unshown, mut shown) = (Vec::new(), Vec::new());
    let (mut frames, mut settles, mut keyframes, mut bytes, mut moves, mut downs) = (0u32, 0u32, 0u32, 0usize, 0u32, 0u32);
    let mut motion_frames = 0u32;
    if let Some(t) = &mut timeline {
        writeln!(t, "t_ms,kind,quality,interval_ms,bytes,encode_ms,delivery_ms,latency_ms")?;
    }

    loop {
        // Everything the desktop did up to now.
        while source.next_at()?.is_some_and(|at| at <= now) {
            let d = source.take();
            dirty = true;
            asked_key |= d.keyframe;
            damage = match (damage, &d.changed) {
                (Some(mut all), Some(rects)) if desk.as_ref().is_some_and(|desk| desk.size == d.size) => {
                    all.extend_from_slice(rects);
                    Some(all)
                }
                _ => None,
            };
            unshown.push(d.at);
            desk = Some(d);
        }
        let earliest = match frame_sent {
            Some(sent) if walk.slowed() => (sent + walk.interval()).max(now),
            _ => now,
        };
        let settle_at = walk.settle_at().map(|at| at.duration_since(base));
        let next = source.next_at()?;
        let settling = if dirty {
            if earliest > now {
                now = earliest;
                continue;
            }
            false
        } else {
            match (settle_at, next) {
                (Some(at), next) if next.is_none_or(|next| at <= next) => {
                    let at = at.max(earliest);
                    if next.is_some_and(|next| next <= at) {
                        now = next.unwrap();
                        continue;
                    }
                    now = at;
                    walk.settle(base + now);
                    true
                }
                (_, Some(next)) => {
                    now = next;
                    continue;
                }
                (_, None) => break,
            }
        };

        let d = desk.as_ref().unwrap();
        let (w, h) = d.size;
        if encoder.as_ref().is_none_or(|e| e.size() != d.size) {
            encoder = Some(Vp9Encoder::with_threads(w, h, walk.quality(), Chroma::Yuv444, threads)?);
            asked_key = true;
            damage = None;
        }
        let encoder = encoder.as_mut().unwrap();
        let keyframe = asked_key;
        out.clear();
        let started = Instant::now();
        let produced = if settling {
            encoder.settle(walk.ceiling(), keyframe, &mut out)?
        } else {
            encoder.read(&d.pixels, usize::from(w) * 4, if keyframe { None } else { damage.as_deref() })?;
            encoder.encode(keyframe, &mut out)?
        };
        let encode = encode_ms.map_or_else(|| started.elapsed(), |ms| Duration::from_secs_f64(ms / 1e3));
        dirty = false;
        damage = Some(Vec::new());
        asked_key = false;
        if !produced {
            continue;
        }
        let sent = now + encode;
        let coded_at = if settling { walk.ceiling() } else { encoder.quality() };
        let size = out.len() + 30;
        let (painted, back) = gateway.frame(sent, size, paint + jitter());
        if keyframe {
            walk.keyframe(base + sent);
        }
        walk.sent(encoder.coarsest(), base + sent);
        frame_sent = Some(sent);
        let delivery = back - sent;
        let verdict = !keyframe && !settling;
        let latency = painted.saturating_sub(d.at);
        shown.extend(unshown.drain(..).map(|at: Duration| (painted - at).as_secs_f64() * 1e3));
        frames += 1;
        bytes += size;
        keyframes += u32::from(keyframe);
        settles += u32::from(settling);
        if verdict {
            motion_frames += 1;
            qualities.push(f64::from(coded_at));
            deliveries.push(delivery.as_secs_f64() * 1e3);
        }
        if let Some(t) = &mut timeline {
            let kind = if keyframe { "key" } else if settling { "settle" } else { "frame" };
            writeln!(
                t,
                "{:.1},{kind},{coded_at},{:.0},{size},{:.1},{:.1},{:.1}",
                sent.as_secs_f64() * 1e3,
                walk.interval().as_secs_f64() * 1e3,
                encode.as_secs_f64() * 1e3,
                delivery.as_secs_f64() * 1e3,
                latency.as_secs_f64() * 1e3,
            )?;
        }
        // The fence comes back; the next frame waits for it.
        now = back;
        let moved = walk.fenced(delivery, verdict, base + now);
        walk.delivered(base + now);
        if let Some(pace) = moved {
            encoder.set_quality(pace.quality);
            if pace.quality != quality_was {
                moves += 1;
                downs += u32::from(pace.quality < quality_was);
                quality_was = pace.quality;
            }
        }
    }
    let span = now.as_secs_f64();
    println!(
        "{name} link={link_name}Mb rtt={rtt}ms | {frames} frames ({motion_frames} motion, {keyframes} key, {settles} settle) {:.1}/s | motion q mean={:.1} p10={:.0} | shown after mean={:.0} p50={:.0} p95={:.0} max={:.0} ms | delivery p50={:.0} p95={:.0} | {:.2} Mb/s | {moves} moves, {downs} down",
        f64::from(frames) / span,
        qualities.iter().sum::<f64>() / qualities.len().max(1) as f64,
        pct(&qualities, 0.1),
        shown.iter().sum::<f64>() / shown.len().max(1) as f64,
        pct(&shown, 0.5),
        pct(&shown, 0.95),
        pct(&shown, 1.0),
        pct(&deliveries, 0.5),
        pct(&deliveries, 0.95),
        bytes as f64 * 8.0 / span / 1e6,
    );
    Ok(())
}
