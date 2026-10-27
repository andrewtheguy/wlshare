//! The walk over the tiles on a modelled link: captures played through the
//! tiles encoder ([`crate::rfb::tiles`]) and the walk as a session plays
//! them, one update in flight, through the zDesk gateway to a browser on a
//! link of a given rate and round trip — `walk-sim`'s model
//! (`crates/wlshare-vp9/examples/sim`), with the tiles picture in place of
//! `WLSV`.
//!
//! ```text
//! tiles-walk-sim CAPTURE [CAPTURE...] [--link MBPS[,MBPS@SECS...]] [--rtt MS]
//!                [--hop MS] [--paint MS] [--jitter MS] [--ceiling Q]
//!                [--held] [--libx264] [--encode MS] [--timeline FILE]
//! ```
//!
//! An example of the daemon's crate, since its regions are the daemon's H.264
//! ([`h264`]), so it runs where the daemon builds; `--libx264` takes libx264
//! where the machine has a VA-API encoder too. The flags are `walk-sim`'s:
//! `--link` the browser's link in Mbit/s, 10 by default, and the rate it
//! changes to from each `@` second on; `--rtt` its round trip, 40 ms; `--hop`
//! the round trip between the session and the gateway, 1 ms; `--paint` how
//! long the browser takes to paint an update once it has it and the one before
//! is painted, 5 ms, and `--jitter` up to how much more, at random.
//! `--ceiling` is the dial the walk starts at and never passes, 90; `--held`
//! holds it there, as a client that lists `WLSD` beside the tiles, every still
//! exact. `--timeline` writes every update as a JSON object a line: `t_ms`,
//! `kind` (`key` for one that crosses the link as a keyframe does, `tick` for
//! one of the tick alone, what the desktop stopping owes, or `frame`), `quality`,
//! `interval_ms`, `bytes`, `encode_ms`, `delivery_ms` and `latency_ms`.
//!
//! Time is the simulation's own, as `walk-sim`'s is: an encode, the read of
//! what changed with it, takes what it took on this machine, or `--encode`;
//! the desktop is the capture's frames at the times they were captured, so a
//! session that falls behind skips the frames it had no time for and reads
//! their damage with the next; and what the encoder owes once the desktop
//! stops goes out on its tick, as an update that waits for the fence and the
//! walk's interval as any other. The gateway passes each rectangle as a record
//! of its own, its body taking its share of the budget. Each update is the
//! walk's to judge as the session judges it: a keyframe or the tick's alone is
//! no verdict, and the first of the tick's settles the walk where it stands.
//! What it says of a run, one JSON object on stdout, is `walk-sim`'s, `picture` `tiles`, with `ticks` in place of its
//! `settles`, and `keyframes` the updates that crossed the link as one.

use std::fs::File;
use std::io::Write as _;
use std::time::{Duration, Instant};

use wlshare_proto::tiles::{TileHeader, TilesPicture};
use wlshare_vp9::walk::QualityWalk;

use crate::rfb::tiles::TilesEncoder;
use crate::rfb::vp9::Rect;
use sim::{Desk, Error, Gateway, Source, json, millis, pct};

// The daemon's own modules, of which the simulation uses the encoders and
// nothing more, the decoder the H.264 encoder's tests read it back with, and
// `walk-sim`'s model of the link.
#[cfg(test)]
#[path = "../src/decode.rs"]
mod decode;
#[allow(dead_code)]
#[path = "../src/h264.rs"]
mod h264;
#[allow(unused)]
#[path = "../src/rfb/mod.rs"]
mod rfb;
#[path = "../../wlshare-vp9/examples/sim/mod.rs"]
mod sim;

/// What the session writes around the encoder's rectangles: the update's
/// header and its fence.
const FRAMING: usize = 4 + 13;

/// An RFB rectangle's header, ahead of each tiles body.
const RECT_HEADER: usize = 12;

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
            TileHeader::Png { len } | TileHeader::Start { len, .. } | TileHeader::Frame { len, .. } | TileHeader::Webp { len } => len,
            TileHeader::End { .. } | TileHeader::Copy { .. } => 0,
        };
        let bytes = RECT_HEADER + used + body;
        records.push((bytes + if at == 0 { FRAMING } else { 0 }, body));
        at += bytes;
    }
    Ok(records)
}

fn main() -> Result<(), Error> {
    let mut paths = Vec::new();
    let mut rates = vec![(Duration::ZERO, 10e6 / 8.0)];
    let (mut hop, mut held, mut libx264, mut encode_ms) = (1.0, false, false, None::<f64>);
    let (mut rtt, mut paint, mut jitter, mut ceiling) = (40.0, 5.0, 0.0, 90u8);
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
            "--ceiling" => ceiling = value()?.parse::<u8>().ok().filter(|q| (1..=100).contains(q)).ok_or("--ceiling takes 1 to 100")?,
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

    // A 60 Hz capture, the daemon's default.
    let base = Instant::now();
    let picture = TilesPicture { quality: ceiling, adaptive: !held };
    let mut walk = QualityWalk::new(ceiling, Duration::from_micros(16_667), !held);
    // Where the walk stands, as the session hands it the encoder.
    let pace = |walk: &QualityWalk| if held { (ceiling, false) } else { (walk.quality(), walk.slowed()) };
    let mut encoder: Option<TilesEncoder> = None;
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
        if encoder.as_ref().is_none_or(|e| e.size() != d.size) {
            let mut made = TilesEncoder::new(w, h, picture, Box::new(coders.coder()?))?;
            let (quality, coarse) = pace(&walk);
            made.set_pace(quality, coarse);
            encoder = Some(made);
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
        let encode = encode_ms.map_or_else(|| started.elapsed(), |ms| Duration::from_secs_f64(ms / 1e3));
        dirty = false;
        damage = Some(Vec::new());
        whole = false;
        // An encode that owes nothing sends nothing, and the request waits.
        if update.rects == 0 {
            continue;
        }
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
        if let Some(t) = &mut timeline {
            let kind = if update.keyframe { "key" } else if update.tick { "tick" } else { "frame" };
            writeln!(
                t,
                "{{\"t_ms\":{:.1},\"kind\":\"{kind}\",\"quality\":{coded_at},\"interval_ms\":{:.0},\"bytes\":{size},\"encode_ms\":{:.1},\"delivery_ms\":{:.1},\"latency_ms\":{:.1}}}",
                sent.as_secs_f64() * 1e3,
                walk.interval().as_secs_f64() * 1e3,
                encode.as_secs_f64() * 1e3,
                delivery.as_secs_f64() * 1e3,
                latency.as_secs_f64() * 1e3,
            )?;
        }
        // The fence comes back; the next update waits for it.
        now = back;
        let moved = walk.fenced(delivery, verdict, base + now);
        walk.delivered(base + now);
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
    println!(
        "{{\"capture\":{},\"picture\":\"tiles\",\"link\":{link_json},\"rtt_ms\":{rtt},\"ceiling\":{ceiling},\"held\":{held},\"frames\":{frames},\"motion_frames\":{motion_frames},\"keyframes\":{keyframes},\"ticks\":{ticks},\"fps\":{},\"motion_q_mean\":{},\"motion_q_p10\":{},\"shown_mean_ms\":{},\"shown_p50_ms\":{},\"shown_p95_ms\":{},\"shown_max_ms\":{},\"delivery_p50_ms\":{},\"delivery_p95_ms\":{},\"mbps\":{},\"moves\":{moves},\"downs\":{downs}}}",
        json(&name),
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
