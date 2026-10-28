//! The walk on a modelled link: captures played through the encoder and the
//! walk as a session plays them, one frame in flight, through the zDesk
//! gateway to a browser on a link of a given rate and round trip. The model of
//! the link and the gateway is `sim`, which the daemon's `tiles-walk-sim`
//! plays the tiles picture over.
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
//! long the browser takes to paint a frame once it has it and the one before
//! is painted, 5 ms, and `--jitter` up to how much more, at random.
//! `--ceiling` is the dial the walk starts at and never passes, 90; `--held`
//! holds it there, as a client that lists `WLSD`. `--timeline` writes every
//! frame as a JSON object a line: `t_ms`, `kind` (`key`, `settle` or `frame`),
//! `quality`, `interval_ms`, `bytes`, `encode_ms`, `delivery_ms` and
//! `latency_ms`.
//!
//! Time is the simulation's own: an encode takes what it took on this
//! machine, or `--encode` for a run that is the same every time; a frame
//! crosses the link at its rate after any frame before it, the gateway reading
//! it, and its fence, once the frame's share of its 512 KiB is free, the whole
//! of it at most, ahead of the browser; and the desktop is the capture's frames
//! at the times they were captured, so a session that falls behind skips the
//! frames it had no time for and carries their damage in the next. Several captures play one after the other in one session.
//! What it says of a run, one JSON object on stdout, is how long each change of
//! the desktop took to be painted (`shown_mean_ms`, `shown_p50_ms`,
//! `shown_p95_ms`, `shown_max_ms`), the frames a second (`fps`, of `frames`,
//! `motion_frames`, `keyframes` and `settles`), the dial the motion was coded
//! at (`motion_q_mean`, `motion_q_p10`), the link it used (`mbps`, and
//! `delivery_p50_ms`, `delivery_p95_ms`) and how often the dial moved (`moves`,
//! `downs`), beside what it was run with: `capture`, `picture` (`wlsv`), `link` (each rate in
//! Mbit/s and the second it starts at), `rtt_ms`, `ceiling` and `held`.

mod sim;

use std::fs::File;
use std::io::Write as _;
use std::time::{Duration, Instant};

use sim::{Desk, Error, Gateway, Source, json, millis, pct};
use wlshare_vp9::walk::QualityWalk;
use wlshare_vp9::{Chroma, Rect, Stream};

fn main() -> Result<(), Error> {
    let mut paths = Vec::new();
    let mut rates = vec![(Duration::ZERO, 10e6 / 8.0)];
    let (mut hop, mut held, mut encode_ms) = (1.0, false, None::<f64>);
    let (mut rtt, mut paint, mut jitter, mut ceiling, mut threads) = (40.0, 5.0, 0.0, 90u8, wlshare_vp9::encoder_threads());
    let mut timeline = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} takes a value"));
        match arg.as_str() {
            "--link" => rates = sim::link_steps(&value()?)?,
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
    let name = sim::run_name(&paths);
    let link_json = sim::rates_json(&rates);
    let mut source = Source::new(paths);
    let mut gateway = Gateway::new(rates, Duration::from_secs_f64(rtt / 1e3), Duration::from_secs_f64(hop / 1e3));
    let paint = Duration::from_secs_f64(paint / 1e3);
    let mut jitter = sim::jitter(jitter);

    // A 30 Hz capture, the daemon's default.
    let base = Instant::now();
    let mut walk = QualityWalk::new(ceiling, Duration::from_micros(33_333), !held);
    let mut encoder: Option<Stream> = None;
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
            encoder = Some(Stream::new(w, h, walk.quality(), Chroma::Yuv444, threads)?);
            asked_key = true;
            damage = None;
        }
        let encoder = encoder.as_mut().unwrap();
        let keyframe = asked_key;
        out.clear();
        let started = Instant::now();
        let coded = if settling {
            encoder.settle(walk.ceiling(), keyframe, &mut out)
        } else {
            encoder.read_bgrx(&d.pixels, usize::from(w) * 4, if keyframe { None } else { damage.as_deref() })?;
            encoder.encode(keyframe, &mut out)
        };
        // libvpx making no frame of a picture is the session's to skip, as it
        // skips it.
        let produced = match coded {
            Ok(_) => true,
            Err(wlshare_vp9::Error::NoFrame) => false,
            Err(e) => return Err(e.into()),
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
        // The frame, the five bytes of dial and length before it in its
        // rectangle, and the thirty this model counts for the framing around
        // that.
        let size = out.len() + 35;
        let (painted, back) = gateway.frame(sent, &[(size, out.len())], paint + jitter());
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
                "{{\"t_ms\":{:.1},\"kind\":\"{kind}\",\"quality\":{coded_at},\"interval_ms\":{:.0},\"bytes\":{size},\"encode_ms\":{:.1},\"delivery_ms\":{:.1},\"latency_ms\":{:.1}}}",
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
    // Through the last frame's paint, which its own fence can come back before.
    let span = now.max(gateway.painted).as_secs_f64();
    println!(
        "{{\"capture\":{},\"picture\":\"wlsv\",\"link\":{link_json},\"rtt_ms\":{rtt},\"ceiling\":{ceiling},\"held\":{held},\"frames\":{frames},\"motion_frames\":{motion_frames},\"keyframes\":{keyframes},\"settles\":{settles},\"fps\":{},\"motion_q_mean\":{},\"motion_q_p10\":{},\"shown_mean_ms\":{},\"shown_p50_ms\":{},\"shown_p95_ms\":{},\"shown_max_ms\":{},\"delivery_p50_ms\":{},\"delivery_p95_ms\":{},\"mbps\":{},\"moves\":{moves},\"downs\":{downs}}}",
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
