//! The walk over the tiles on a modelled link: captures played through the
//! tiles encoder ([`crate::rfb::tiles`]) and the walk as a session plays
//! them, one update in flight, through the zDesk gateway to a browser on a
//! link of a given rate and round trip — `walk-sim`'s model
//! (`examples/sim`), with the tiles picture in place of `WLSV`.
//!
//! ```text
//! tiles-walk-sim CAPTURE [CAPTURE...] [--link MBPS[,MBPS@SECS...]] [--rtt MS]
//!                [--hop MS] [--paint MS] [--jitter MS]
//!                [--held] [--libx264] [--encode MS]
//!                [--timeline FILE]
//! ```
//!
//! Its regions are the daemon's H.264 ([`h264`]), as `walk-sim`'s stream is;
//! `--libx264` takes libx264 where the machine has a VA-API encoder too. The
//! flags are `walk-sim`'s:
//! `--link` the browser's link in Mbit/s, 10 by default, and the rate it
//! changes to from each `@` second on; `--rtt` its round trip, 40 ms; `--hop`
//! the round trip between the session and the gateway, 1 ms; `--paint` how
//! long the browser takes to paint an update once it has it and the one before
//! is painted, 5 ms, and `--jitter` up to how much more, at random.
//! The walk starts at wlshare's dial ([`rfb::QUALITY`]) and never passes it,
//! over every tile progressive, as a client that lists the tiles with no
//! `WLSD` is sent them, and the passes are counted by kind on stderr;
//! `--held` holds the dial there instead, as a client that lists `WLSD`
//! beside the tiles, every still exact and the regions streamed.
//! `--timeline` writes every update as a JSON object a line: `t_ms`,
//! `kind` (`key` for one that crosses the link as a keyframe does, `tick` for
//! one of the tick alone, what the desktop stopping owes, or `frame`), `quality`,
//! `interval_ms`, `bytes`, `stills` and `streams` (the bodies of its PNGs
//! and progressive tiles, and of its streams' frames), `encode_ms`, `cpu_ms` (the
//! processor time of that encode, on every thread, which `encoder_cpu_pct`
//! sums), `delivery_ms`,
//! `latency_ms` and, where its fence moved the dial, `moved_to`.
//!
//! Time is the simulation's own, as `walk-sim`'s is: an encode, the read of
//! what changed with it, takes what it took on this machine, or `--encode`;
//! the desktop is the capture's frames at the times they were captured, so a
//! session that falls behind skips the frames it had no time for and reads
//! their damage with the next; and what the encoder owes once the desktop
//! stops goes out on its tick, as an update that waits for the fence and the
//! walk's interval as any other. The capture is the daemon's default, 30 Hz.
//! The gateway passes each rectangle as a record
//! of its own, its body taking its share of the budget. Each update is the
//! walk's to judge as the session judges it: a keyframe or the tick's alone is
//! no verdict, and the first of the tick's settles the walk where it stands.
//! What it says of a run, one JSON object on stdout, is `walk-sim`'s, `picture`
//! `tiles`, with `ticks`, the updates of the tick alone, the dial the motion
//! was coded at (`motion_q_mean`, `motion_q_p10`) and how often it moved
//! (`moves`, `downs`), and `keyframes` the updates that crossed the link as
//! one.
//! Every update is drawn as the page draws it ([`Page`]), and `psnr_mean_db`
//! and `psnr_p10_db` are how close each update left the page to the desktop
//! it was coded from, `psnr_end_db` the page once nothing more is owed.
//! `encoder_cpu_pct` is the processor time the encoder took, on every thread
//! of its own, read from the process's clock around each read and encode, as
//! a share of one core over the session: what a daemon spends coding it,
//! allocating through the daemon's jemalloc.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write as _;
use std::time::{Duration, Instant};

use wlshare_proto::tiles::{TileHeader, TilesPicture};

use crate::rfb::client::{self, RectBody, ServerMsg, TileBody};
use crate::rfb::tiles::TilesEncoder;
use crate::rfb::Rect;
use crate::walk::QualityWalk;
use sim::{Desk, Error, Gateway, Source, json, millis, pct};

// The daemon's own modules, of which the simulation uses the encoders and
// nothing more, the decoder the H.264 encoder's tests read it back with, and
// `walk-sim`'s model of the link and the walk it plays over it.
#[path = "../src/decode.rs"]
mod decode;
#[allow(dead_code)]
#[path = "../src/h264.rs"]
mod h264;
#[allow(unused)]
#[path = "../src/rfb/mod.rs"]
mod rfb;
mod sim;
#[allow(dead_code)]
#[path = "../src/walk.rs"]
mod walk;
#[allow(dead_code)]
#[path = "../src/wlcap.rs"]
mod wlcap;

#[global_allocator]
static ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// The processor time the process has taken, on all of its threads.
fn process_cpu() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid `timespec` for the call to write.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    assert_eq!(rc, 0, "the process's processor clock");
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// What the session writes around the encoder's rectangles: the update's
/// header and its fence.
const FRAMING: usize = 4 + 13;

/// An RFB rectangle's header, ahead of each tiles body.
const RECT_HEADER: usize = 12;

/// The bodies of an update's stills, PNG and progressive, and of its streams' frames.
fn split(out: &[u8]) -> Result<(usize, usize), Error> {
    let (mut stills, mut streams, mut at) = (0, 0, 0);
    while at < out.len() {
        let Some((header, used)) = TileHeader::parse(&out[at + RECT_HEADER..])? else {
            return Err("a tiles rectangle cut short".into());
        };
        let body = match header {
            TileHeader::Png { len } | TileHeader::Progressive { len } => {
                stills += len;
                len
            }
            TileHeader::Start { len, .. } | TileHeader::Frame { len, .. } => {
                streams += len;
                len
            }
            TileHeader::End { .. } | TileHeader::Copy { .. } | TileHeader::Cached { .. } | TileHeader::Store { .. } => 0,
        };
        at += RECT_HEADER + used + body;
    }
    Ok((stills, streams))
}

/// The records the gateway passes of an update the encoder wrote as `out`:
/// each rectangle, its bytes on the link and those of its body, which a
/// rectangle takes of the gateway's budget, a COPY and an END none. The
/// update's framing goes with the first.
fn records(out: &[u8]) -> Result<Vec<(usize, usize)>, Error> {
    let mut records = Vec::new();
    let mut at = 0;
    while at < out.len() {
        let Some((header, used)) = TileHeader::parse(&out[at + RECT_HEADER..])? else {
            return Err("a tiles rectangle cut short".into());
        };
        let body = match header {
            TileHeader::Png { len } | TileHeader::Start { len, .. } | TileHeader::Frame { len, .. } | TileHeader::Progressive { len } => len,
            TileHeader::End { .. } | TileHeader::Copy { .. } | TileHeader::Cached { .. } | TileHeader::Store { .. } => 0,
        };
        let bytes = RECT_HEADER + used + body;
        records.push((bytes + if at == 0 { FRAMING } else { 0 }, body));
        at += bytes;
    }
    Ok(records)
}

/// A rectangle: left, top, width, height.
type Bounds = (u16, u16, u16, u16);

/// What the page draws of the updates: each PNG, each region's
/// H.264 by libavcodec, each copy and cached tile, and each progressive tile
/// as the encoder's model of zDesk's decoder shows it.
struct Page {
    w: usize,
    canvas: Vec<u8>,
    /// Each live stream's rectangle and decoder.
    streams: HashMap<u8, (Bounds, decode::Decoder)>,
    slots: HashMap<u16, Vec<u8>>,
    planes: Vec<u8>,
}

/// The progressive tiles sent, count and bytes, by pass: first passes at each
/// stage below full, whole, as differences, upgrades.
#[derive(Default)]
struct Passes {
    first: [(u64, u64); 3],
    whole: (u64, u64),
    differences: (u64, u64),
    upgrades: (u64, u64),
}

impl Passes {
    /// A tile block's pass, read from its bytes: a first pass's flags and
    /// quality byte, or an upgrade.
    fn add(&mut self, block: &[u8]) {
        let tally = |t: &mut (u64, u64)| {
            t.0 += 1;
            t.1 += block.len() as u64;
        };
        match (u16::from_le_bytes([block[0], block[1]]), block.get(13), block.get(14)) {
            (wlshare_proto::progressive::WBT_TILE_FIRST, Some(flags), _) if flags & 1 == 1 => tally(&mut self.differences),
            (wlshare_proto::progressive::WBT_TILE_FIRST, _, Some(0xFF)) => tally(&mut self.whole),
            (wlshare_proto::progressive::WBT_TILE_FIRST, _, Some(&stage)) => tally(&mut self.first[usize::from(stage).min(2)]),
            _ => tally(&mut self.upgrades),
        }
    }
}

impl Page {
    fn new(w: u16, h: u16) -> Self {
        Self { w: usize::from(w), canvas: vec![0; usize::from(w) * usize::from(h) * 4], streams: HashMap::new(), slots: HashMap::new(), planes: Vec::new() }
    }

    /// `B, G, R, X` rows `line` pixels apart drawn at the rectangle.
    fn paste(&mut self, (x, y, w, h): Bounds, bgrx: &[u8], line: usize) {
        let row_bytes = usize::from(w) * 4;
        for row in 0..usize::from(h) {
            let at = ((usize::from(y) + row) * self.w + usize::from(x)) * 4;
            self.canvas[at..at + row_bytes].copy_from_slice(&bgrx[row * line * 4..][..row_bytes]);
        }
    }

    fn rgb(rgb: &[u8]) -> Vec<u8> {
        rgb.as_chunks::<3>().0.iter().flat_map(|p| [p[2], p[1], p[0], 0]).collect()
    }

    fn draw(&mut self, count: u16, rects: &[u8], encoder: &mut TilesEncoder, passes: &mut Passes) -> Result<(), Error> {
        let mut wire = crate::rfb::msg::update_header(count).to_vec();
        wire.extend_from_slice(rects);
        let Some((ServerMsg::Update(rects), _)) = client::parse(&wire)? else {
            return Err("an update the client cannot read".into());
        };
        for rect in rects {
            let RectBody::Tiles(body) = rect.body else { return Err("a rectangle not of the tiles".into()) };
            let place = (rect.x, rect.y, rect.width, rect.height);
            let (w, h) = (usize::from(rect.width), usize::from(rect.height));
            match body {
                TileBody::Png(png) => {
                    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
                    decoder.set_transformations(png::Transformations::EXPAND);
                    let mut reader = decoder.read_info()?;
                    let mut buf = vec![0; reader.output_buffer_size().ok_or("a PNG too large")?];
                    let info = reader.next_frame(&mut buf)?;
                    let bgrx = Self::rgb(&buf[..info.buffer_size()]);
                    self.paste(place, &bgrx, w);
                }
                TileBody::Progressive(block) => {
                    let cell = wlshare_proto::progressive::tile_of(&block).ok_or("a progressive body that is no tile block")?;
                    // A tile something later in the update drew over is
                    // held no more; what drew over it is pasted in its turn.
                    if let Some(shown) = encoder.progressive_shown(cell) {
                        self.paste(place, &shown, 64);
                    }
                    passes.add(&block);
                }
                TileBody::Start { stream, frame } => {
                    self.streams.insert(stream, (place, decode::Decoder::new().map_err(|e| format!("{e:#}"))?));
                    self.region(stream, &frame)?;
                }
                TileBody::Frame { stream, frame } => self.region(stream, &frame)?,
                TileBody::End { stream } => {
                    self.streams.remove(&stream);
                }
                TileBody::Copy { x, y } => {
                    let line = w * 4;
                    let source: Vec<u8> = (0..h).flat_map(|row| {
                        let at = ((usize::from(y) + row) * self.w + usize::from(x)) * 4;
                        self.canvas[at..at + line].to_vec()
                    }).collect();
                    self.paste(place, &source, w);
                }
                TileBody::Store { slot } => {
                    let tile: Vec<u8> = (0..h).flat_map(|row| self.canvas[((usize::from(rect.y) + row) * self.w + usize::from(rect.x)) * 4..][..w * 4].to_vec()).collect();
                    self.slots.insert(slot, tile);
                }
                TileBody::Cached { slot } => {
                    let tile = self.slots.get(&slot).ok_or("a slot drawn before it was stored")?.clone();
                    self.paste(place, &tile, w);
                }
            }
        }
        Ok(())
    }

    /// A frame of a stream, decoded and drawn over its rectangle: BT.601 at
    /// studio swing, as the stream's VUI says.
    fn region(&mut self, stream: u8, frame: &[u8]) -> Result<(), Error> {
        let (place, decoder) = self.streams.get_mut(&stream).ok_or("a frame of no live stream")?;
        let place = *place;
        let Some(picture) = decoder.decode(frame, &mut self.planes).map_err(|e| format!("{e:#}"))? else { return Ok(()) };
        let (pw, ph) = (picture.width, picture.height);
        let (cw, ch) = picture.chroma();
        let (y, rest) = self.planes.split_at(pw * ph);
        let (u, v) = rest.split_at(cw * ch);
        let mut bgrx = vec![0u8; pw * ph * 4];
        for row in 0..ph {
            for col in 0..pw {
                let l = 1.164 * (f32::from(y[row * pw + col]) - 16.0);
                let (cb, cr) = (f32::from(u[row / 2 * cw + col / 2]) - 128.0, f32::from(v[row / 2 * cw + col / 2]) - 128.0);
                let at = (row * pw + col) * 4;
                bgrx[at] = (l + 2.017 * cb).round().clamp(0.0, 255.0) as u8;
                bgrx[at + 1] = (l - 0.392 * cb - 0.813 * cr).round().clamp(0.0, 255.0) as u8;
                bgrx[at + 2] = (l + 1.596 * cr).round().clamp(0.0, 255.0) as u8;
            }
        }
        self.paste(place, &bgrx, pw);
        Ok(())
    }

    /// The mean squared error over B, G and R from `pixels`.
    fn mse(&self, pixels: &[u8]) -> f64 {
        let sum: u64 = self.canvas.as_chunks::<4>().0.iter().zip(pixels.as_chunks::<4>().0).map(|(a, b)| (0..3).map(|i| u64::from(a[i].abs_diff(b[i])).pow(2)).sum::<u64>()).sum();
        sum as f64 / (self.canvas.len() / 4 * 3) as f64
    }
}

fn psnr(mse: f64) -> f64 {
    if mse == 0.0 { 99.0 } else { (10.0 * (255.0f64.powi(2) / mse).log10()).min(99.0) }
}

fn main() -> Result<(), Error> {
    let mut paths = Vec::new();
    let mut rates = vec![(Duration::ZERO, 10e6 / 8.0)];
    let (mut hop, mut held, mut libx264, mut encode_ms) = (1.0, false, false, None::<f64>);
    let (mut rtt, mut paint, mut jitter) = (40.0, 5.0, 0.0);
    let ceiling = rfb::QUALITY;
    let mut timeline = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} takes a value"));
        match arg.as_str() {
            "--link" => rates = sim::link_steps(&value()?)?,
            "--rtt" => rtt = millis(&arg, value()?)?,
            "--hop" => hop = millis(&arg, value()?)?,
            "--held" => held = true,
            "--libx264" => libx264 = true,
            "--encode" => encode_ms = Some(millis(&arg, value()?)?),
            "--paint" => paint = millis(&arg, value()?)?,
            "--jitter" => jitter = millis(&arg, value()?)?,
            "--timeline" => timeline = Some(std::io::BufWriter::new(File::create(value()?)?)),
            _ if arg.starts_with("--") => return Err(format!("unknown flag {arg}").into()),
            _ => paths.push(arg),
        }
    }
    if paths.is_empty() {
        return Err("tiles-walk-sim takes a capture to play".into());
    }
    // libavcodec's own log at warnings, as the daemon sets it: what it says at
    // its default level is libx264's summary of every region's stream.
    // SAFETY: a store of the level, before any thread uses libavcodec.
    unsafe { ffmpeg_sys_next::av_log_set_level(ffmpeg_sys_next::AV_LOG_WARNING) };
    let coders = h264::Encoder::find(!libx264).map_err(|e| format!("{e:#}"))?;
    eprintln!("tiles-walk-sim: the regions are H.264 by {}", coders.name());
    let name = sim::run_name(&paths);
    let link_json = sim::rates_json(&rates);
    let mut source = Source::new(paths);
    let mut gateway = Gateway::new(rates, Duration::from_secs_f64(rtt / 1e3), Duration::from_secs_f64(hop / 1e3));
    let paint = Duration::from_secs_f64(paint / 1e3);
    let mut jitter = sim::jitter(jitter);

    // A 30 Hz capture, the daemon's default.
    let base = Instant::now();
    let picture = TilesPicture { progressive: !held };
    let mut walk = QualityWalk::new(ceiling, Duration::from_micros(33_333), !held);
    // Where the walk stands, as the session hands it the encoder.
    let pace = |walk: &QualityWalk| if held { (ceiling, false) } else { (walk.quality(), walk.slowed()) };
    let mut encoder: Option<TilesEncoder> = None;
    let mut encoder_cpu = Duration::ZERO;
    let mut page: Option<Page> = None;
    let mut passes = Passes::default();
    let mut psnrs = Vec::new();
    let mut now = Duration::ZERO;
    // The desktop as it stands and what changed since the client's last update.
    let mut desk: Option<Desk> = None;
    let mut dirty = false;
    let mut damage: Option<Vec<Rect>> = Some(Vec::new());
    let mut whole = false;
    let mut update_sent: Option<Duration> = None;
    let mut out = Vec::new();
    let mut quality_was = walk.quality();
    let (mut qualities, mut deliveries) = (Vec::new(), Vec::new());
    // When each change of the desktop the client has not seen yet was made,
    // and how long each one took to be painted.
    let (mut unshown, mut shown) = (Vec::new(), Vec::new());
    let (mut frames, mut ticks, mut keyframes, mut bytes, mut moves, mut downs) = (0u32, 0u32, 0u32, 0usize, 0u32, 0u32);
    let mut motion_frames = 0u32;
    // The last update was the tick's alone, as the session keeps it.
    let mut quiet = false;

    loop {
        // Everything the desktop did up to now.
        while source.next_at()?.is_some_and(|at| at <= now) {
            let d = source.take();
            dirty = true;
            whole |= d.keyframe;
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
        // A slowed walk's interval holds every update, a tick's too.
        let earliest = match update_sent {
            Some(sent) if walk.slowed() => (sent + walk.interval()).max(now),
            _ => now,
        };
        let next = source.next_at()?;
        if !dirty {
            let tick_at = encoder.as_ref().and_then(TilesEncoder::tick_at).map(|at| at.max(earliest));
            match (tick_at, next) {
                (Some(at), next) if next.is_none_or(|next| at < next) => now = at,
                (_, Some(next)) => {
                    now = next;
                    continue;
                }
                (_, None) => break,
            }
        } else if earliest > now {
            now = earliest;
            continue;
        }

        let d = desk.as_ref().unwrap();
        let (w, h) = d.size;
        let started = Instant::now();
        let cpu_started = process_cpu();
        if encoder.as_ref().is_none_or(|e| e.size() != d.size) {
            let mut made = TilesEncoder::new(w, h, picture, Box::new(coders.coder()?))?;
            let (quality, coarse) = pace(&walk);
            made.set_pace(quality, coarse);
            encoder = Some(made);
            page = Some(Page::new(w, h));
            damage = None;
        } else if whole {
            // A picture the client asks for whole.
            encoder.as_mut().unwrap().forget();
            damage = None;
        }
        let encoder = encoder.as_mut().unwrap();
        out.clear();
        if dirty {
            encoder.read(&d.pixels, usize::from(w) * 4, damage.as_deref(), now)?;
        }
        let update = encoder.encode(now, &mut out)?;
        let cpu = process_cpu() - cpu_started;
        encoder_cpu += cpu;
        let encode = encode_ms.map_or_else(|| started.elapsed(), |ms| Duration::from_secs_f64(ms / 1e3));
        dirty = false;
        damage = Some(Vec::new());
        whole = false;
        // An encode that owes nothing sends nothing, and the request waits.
        if update.rects == 0 {
            continue;
        }
        let page = page.as_mut().unwrap();
        page.draw(update.rects, &out, encoder, &mut passes)?;
        psnrs.push(psnr(page.mse(&d.pixels)));
        let sent = now + encode;
        let coded_at = encoder.quality();
        let size = out.len() + FRAMING;
        let (painted, back) = gateway.frame(sent, &records(&out)?, paint + jitter());
        if update.keyframe {
            walk.keyframe(base + sent);
        }
        if update.tick && !quiet {
            walk.settle(base + sent);
        }
        quiet = update.tick;
        update_sent = Some(sent);
        let delivery = back - sent;
        let verdict = !update.keyframe && !update.tick;
        let latency = painted.saturating_sub(d.at);
        shown.extend(unshown.drain(..).map(|at: Duration| (painted - at).as_secs_f64() * 1e3));
        frames += 1;
        bytes += size;
        keyframes += u32::from(update.keyframe);
        ticks += u32::from(update.tick);
        if verdict {
            motion_frames += 1;
            qualities.push(f64::from(coded_at));
            deliveries.push(delivery.as_secs_f64() * 1e3);
        }
        let interval = walk.interval();
        // The fence comes back; the next update waits for it.
        now = back;
        let moved = walk.fenced(delivery, verdict, base + now);
        if let Some(t) = &mut timeline {
            let kind = if update.keyframe { "key" } else if update.tick { "tick" } else { "frame" };
            let (stills, streams) = split(&out)?;
            let moved_to = moved.map_or_else(String::new, |moved| format!(",\"moved_to\":{}", moved.quality));
            writeln!(
                t,
                "{{\"t_ms\":{:.1},\"kind\":\"{kind}\",\"quality\":{coded_at},\"interval_ms\":{:.0},\"bytes\":{size},\"stills\":{stills},\"streams\":{streams},\"encode_ms\":{:.1},\"cpu_ms\":{:.1},\"delivery_ms\":{:.1},\"latency_ms\":{:.1}{moved_to}}}",
                sent.as_secs_f64() * 1e3,
                interval.as_secs_f64() * 1e3,
                encode.as_secs_f64() * 1e3,
                cpu.as_secs_f64() * 1e3,
                delivery.as_secs_f64() * 1e3,
                latency.as_secs_f64() * 1e3,
            )?;
        }
        if let Some(moved) = moved {
            let (quality, coarse) = pace(&walk);
            encoder.set_pace(quality, coarse);
            if moved.quality != quality_was {
                moves += 1;
                downs += u32::from(moved.quality < quality_was);
                quality_was = moved.quality;
            }
        }
    }
    // Through the last update's paint, which its own fence can come back before.
    let span = now.max(gateway.painted).as_secs_f64();
    let end = match (&page, &desk) {
        (Some(page), Some(desk)) => psnr(page.mse(&desk.pixels)),
        _ => 0.0,
    };
    println!(
        "{{\"capture\":{},\"picture\":\"{}\",\"link\":{link_json},\"rtt_ms\":{rtt},\"ceiling\":{ceiling},\"held\":{held},\"frames\":{frames},\"motion_frames\":{motion_frames},\"keyframes\":{keyframes},\"ticks\":{ticks},\"fps\":{},\"motion_q_mean\":{},\"motion_q_p10\":{},\"shown_mean_ms\":{},\"shown_p50_ms\":{},\"shown_p95_ms\":{},\"shown_max_ms\":{},\"delivery_p50_ms\":{},\"delivery_p95_ms\":{},\"mbps\":{},\"moves\":{moves},\"downs\":{downs},\"psnr_mean_db\":{:.2},\"psnr_p10_db\":{:.2},\"psnr_end_db\":{end:.2},\"encoder_cpu_pct\":{:.1}}}",
        json(&name),
        if held { "tiles" } else { "progressive" },
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
        psnrs.iter().sum::<f64>() / psnrs.len().max(1) as f64,
        pct(&psnrs, 0.1),
        encoder_cpu.as_secs_f64() / span * 100.0,
    );
    if !held {
        let Passes { first: [f25, f50, f75], whole, differences, upgrades } = passes;
        eprintln!("progressive tiles, count and bytes: first at 25 {f25:?}, first at 50 {f50:?}, first at 75 {f75:?}, whole {whole:?}, differences {differences:?}, upgrades {upgrades:?}");
    }
    Ok(())
}
