//! The walk on a modelled link: captures played through the video encoder
//! ([`crate::rfb::video`]) and the walk as a session plays them, one frame in
//! flight, through the zDesk gateway to a browser on a link of a given rate
//! and round trip. The model of the link and the gateway is `sim`, which
//! `tiles-walk-sim` plays the tiles picture over.
//!
//! ```text
//! walk-sim CAPTURE [CAPTURE...] [--link MBPS[,MBPS@SECS...]] [--rtt MS]
//!          [--hop MS] [--paint MS] [--jitter MS] [--libx264] [--444]
//!          [--encode MS] [--timeline FILE] [--stream FILE]
//!          [--shot MS:FILE | end:FILE]...
//! ```
//!
//! The stream is the daemon's H.264 ([`h264`]): the hardware's where the
//! machine has a VA-API encoder, unless `--libx264` takes libx264; `--444`
//! codes it at 4:4:4, High 4:4:4 Predictive, which is libx264's on every
//! machine. `--link`
//! is the browser's link in Mbit/s, 10 by default, and the rate it changes to
//! from each `@` second on; `--rtt` its round trip, 40 ms; `--hop` the round
//! trip between the session and the gateway, 1 ms; `--paint` how long the
//! browser takes to paint a frame once it has it and the one before is
//! painted, 5 ms, and `--jitter` up to how much more, at random. The walk
//! ([`QualityWalk`]) moves each frame's quality from wlshare's dial
//! ([`crate::rfb::QUALITY`]) down, and the frames after it. A desktop that
//! has not changed for half a second since its last frame is sent its settle
//! frames ([`VideoEncoder::settle`]) as the session sends them, each once the
//! one before is fenced, none a verdict, the first settling the walk where it
//! stands. `--timeline` writes
//! every frame as a JSON object a line: `t_ms`, `kind` (`key`, `frame` or
//! `settle`),
//! `quality`, `interval_ms`, `bytes`, `encode_ms`, `cpu_ms` (the processor
//! time of that read and encode, on every thread), `delivery_ms`,
//! `latency_ms` and `psnr_db` (how close the frame left the page to the
//! desktop it was coded from).
//!
//! Time is the simulation's own: an encode, the read of what changed with
//! it, takes what it took on this machine, or `--encode` for a run that is the
//! same every time; a frame crosses the link at its rate after any frame
//! before it, the gateway reading it, and its fence, once the frame's share of
//! its 512 KiB is free, the whole of it at most, ahead of the browser; and the
//! desktop is the capture's frames at the times they were captured, so a
//! session that falls behind skips the frames it had no time for and carries
//! their damage in the next. Several captures play one after the other in one
//! session. The capture is the daemon's default, 30 Hz. What it says of a run,
//! one JSON object on stdout, is how long each change of the desktop took to
//! be painted (`shown_mean_ms`, `shown_p50_ms`, `shown_p95_ms`,
//! `shown_max_ms`), the frames a second (`fps`, of `frames`, `motion_frames`,
//! `keyframes` and `settles`, whose bytes are `settle_bytes`), the quality the motion was coded at (`motion_q_mean`,
//! `motion_q_p10`), the link it used (`mbps`, and `delivery_p50_ms`,
//! `delivery_p95_ms`), how often the walk moved, its quality or its frames
//! (`moves`, and `downs` of them toward coarser or fewer) and
//! the processor time the encoder took as a share of one core over the
//! session (`encoder_cpu_pct`), beside what it was run with: `capture`,
//! `picture` (`video`), `chroma` (`420` or `444`), `encoder`, `link` (each
//! rate in Mbit/s and the second it starts at), `rtt_ms` and `ceiling`.
//! Every frame is decoded as a page decodes it, by libavcodec's software
//! decoder ([`decode`]), its picture turned back from BT.601 at studio swing
//! by `yuv`, outside the time counted: `psnr_mean_db` and `psnr_p10_db` are
//! how close each frame left the page to the desktop it was coded from, over
//! B, G and R, as `tiles-walk-sim`'s are, and `psnr_end_db` the page as the
//! last frame left it, its settle frames among them.
//! `--stream` writes every frame's body as it was sent, the length word and
//! the access unit, one after another, for a decoder to be timed on.
//! `--shot MS:FILE` writes the page as the first frame sent `MS` into the
//! session or after left it, a PNG, and beside it the desktop that frame was
//! coded from (`-desk` before the extension); `end:FILE` the page and the
//! desktop when the session is over.

use std::fs::File;
use std::io::Write as _;
use std::time::{Duration, Instant};

use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix};

use crate::rfb::video::{SETTLE_AFTER, VideoEncoder};
use crate::rfb::{Chroma, Rect};
use crate::walk::QualityWalk;
use sim::{Desk, Error, Gateway, Shots, Source, json, millis, pct};

// The daemon's own modules, of which the simulation uses the encoder and the
// walk and nothing more, and the model of the link `tiles-walk-sim` shares.
#[path = "../src/decode.rs"]
#[allow(dead_code)]
mod decode;
#[allow(dead_code)]
#[path = "../src/h264/mod.rs"]
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
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid `timespec` for the call to write.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    assert_eq!(rc, 0, "the process's processor clock");
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// What a page holds of the stream: each frame decoded by libavcodec's
/// software decoder and turned back to `B, G, R, X`, cut to the desktop.
struct Page {
    decoder: decode::Decoder,
    chroma: Chroma,
    planes: Vec<u8>,
    scratch: Vec<u8>,
    size: (usize, usize),
    canvas: Vec<u8>,
}

impl Page {
    fn new(chroma: Chroma) -> Result<Self, Error> {
        Ok(Self {
            decoder: decode::Decoder::new()?,
            chroma,
            planes: Vec::new(),
            scratch: Vec::new(),
            size: (0, 0),
            canvas: Vec::new(),
        })
    }

    /// Decode `unit`, a frame of a `w`×`h` desktop, and draw it.
    fn draw(&mut self, unit: &[u8], (w, h): (usize, usize)) -> Result<(), Error> {
        let full = self.chroma == Chroma::Yuv444;
        let picture = if full {
            self.decoder.decode_444(unit, &mut self.planes)
        } else {
            self.decoder.decode(unit, &mut self.planes)
        }?
        .ok_or("a frame decoded to no picture")?;
        let (pw, ph) = (picture.width, picture.height);
        if (pw, ph) != (w.next_multiple_of(2), h.next_multiple_of(2)) {
            return Err(format!("a {w}x{h} desktop decoded as {pw}x{ph}").into());
        }
        let (cw, ch) = if full { (pw, ph) } else { picture.chroma() };
        let (luma, chroma) = self.planes.split_at(pw * ph);
        let (u, v) = chroma.split_at(cw * ch);
        let planar = YuvPlanarImage {
            y_plane: luma,
            y_stride: pw as u32,
            u_plane: u,
            u_stride: cw as u32,
            v_plane: v,
            v_stride: cw as u32,
            width: pw as u32,
            height: ph as u32,
        };
        self.scratch.resize(pw * ph * 4, 0);
        let back = if full {
            yuv::yuv444_to_bgra
        } else {
            yuv::yuv420_to_bgra
        };
        back(
            &planar,
            &mut self.scratch,
            (pw * 4) as u32,
            YuvRange::Limited,
            YuvStandardMatrix::Bt601,
        )?;
        self.size = (w, h);
        self.canvas.resize(w * h * 4, 0);
        for (to, from) in self
            .canvas
            .chunks_exact_mut(w * 4)
            .zip(self.scratch.chunks_exact(pw * 4))
        {
            to.copy_from_slice(&from[..w * 4]);
        }
        Ok(())
    }

    /// How close the page is to `pixels`, the desktop: the PSNR over B, G
    /// and R, 99 dB for the same picture.
    fn psnr(&self, pixels: &[u8]) -> f64 {
        let sum: u64 = self
            .canvas
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pixels.as_chunks::<4>().0)
            .map(|(a, b)| {
                (0..3)
                    .map(|i| u64::from(a[i].abs_diff(b[i])).pow(2))
                    .sum::<u64>()
            })
            .sum();
        if sum == 0 {
            return 99.0;
        }
        let mse = sum as f64 / (self.canvas.len() / 4 * 3) as f64;
        (10.0 * (255.0f64.powi(2) / mse).log10()).min(99.0)
    }
}

fn main() -> Result<(), Error> {
    let mut paths = Vec::new();
    let mut chroma = Chroma::Yuv420;
    let mut rates = vec![(Duration::ZERO, 10e6 / 8.0)];
    let (mut hop, mut libx264, mut encode_ms) = (1.0, false, None::<f64>);
    let (mut rtt, mut paint, mut jitter) = (40.0, 5.0, 0.0);
    let mut timeline = None;
    let mut shots = Shots::default();
    let mut stream = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} takes a value"));
        match arg.as_str() {
            "--link" => rates = sim::link_steps(&value()?)?,
            "--rtt" => rtt = millis(&arg, value()?)?,
            "--hop" => hop = millis(&arg, value()?)?,
            "--libx264" => libx264 = true,
            "--444" => chroma = Chroma::Yuv444,
            "--encode" => encode_ms = Some(millis(&arg, value()?)?),
            "--paint" => paint = millis(&arg, value()?)?,
            "--jitter" => jitter = millis(&arg, value()?)?,
            "--timeline" => timeline = Some(std::io::BufWriter::new(File::create(value()?)?)),
            "--stream" => stream = Some(std::io::BufWriter::new(File::create(value()?)?)),
            "--shot" => shots.add(&value()?)?,
            _ if arg.starts_with("--") => return Err(format!("unknown flag {arg}").into()),
            _ => paths.push(arg),
        }
    }
    if paths.is_empty() {
        return Err("walk-sim takes a capture to play".into());
    }
    let name = sim::run_name(&paths);
    let link_json = sim::rates_json(&rates);
    let h264 = h264::Encoder::find(!libx264).map_err(|e| format!("{e:#}"))?;
    let mut source = Source::new(paths);
    let mut gateway = Gateway::new(
        rates,
        Duration::from_secs_f64(rtt / 1e3),
        Duration::from_secs_f64(hop / 1e3),
    );
    let paint = Duration::from_secs_f64(paint / 1e3);
    let mut jitter = sim::jitter(jitter);

    // A 30 Hz capture, the daemon's default.
    let base = Instant::now();
    let mut walk = QualityWalk::new(rfb::QUALITY, Duration::from_micros(33_333), true);
    let mut encoder: Option<VideoEncoder> = None;
    let mut now = Duration::ZERO;
    // The desktop as it stands and what changed since the client's last frame.
    let mut desk: Option<Desk> = None;
    let mut dirty = false;
    let mut damage: Option<Vec<Rect>> = Some(Vec::new());
    let mut asked_key = false;
    let mut frame_sent: Option<Duration> = None;
    let mut out = Vec::new();
    let mut pace_was = walk.pace();
    let mut qualities = Vec::new();
    let mut deliveries = Vec::new();
    // When each change of the desktop the client has not seen yet was made,
    // and how long each one took to be painted.
    let (mut unshown, mut shown) = (Vec::new(), Vec::new());
    let (mut frames, mut motion_frames, mut keyframes, mut bytes, mut moves, mut downs) =
        (0u32, 0u32, 0u32, 0usize, 0u32, 0u32);
    // The last frame was a settle frame: the desktop is quiet, and the walk
    // was told so at the first.
    let mut quiet = false;
    let (mut settles, mut settle_bytes) = (0u32, 0usize);
    let mut cpu = Duration::ZERO;
    let mut page = Page::new(chroma)?;
    let mut psnrs = Vec::new();
    let mut end = 0.0;

    loop {
        // Everything the desktop did up to now.
        while source.next_at()?.is_some_and(|at| at <= now) {
            let d = source.take();
            dirty = true;
            asked_key |= d.keyframe;
            damage = match (damage, &d.changed) {
                (Some(mut all), Some(rects))
                    if desk.as_ref().is_some_and(|desk| desk.size == d.size) =>
                {
                    all.extend_from_slice(rects);
                    Some(all)
                }
                _ => None,
            };
            unshown.push(d.at);
            desk = Some(d);
        }
        let held = match frame_sent {
            Some(sent) if walk.slowed() => (sent + walk.interval()).max(now),
            _ => now,
        };
        // Nothing changed: the next thing is the desktop's next change, or a
        // settle frame where one is owed and due before it.
        let mut settling = false;
        if !dirty {
            let due = match (&encoder, frame_sent) {
                (Some(encoder), Some(sent)) if encoder.settle_owed() => Some(if quiet {
                    held
                } else {
                    held.max(sent + SETTLE_AFTER)
                }),
                _ => None,
            };
            match (source.next_at()?, due) {
                (Some(next), Some(due)) if due < next => {
                    now = due;
                    settling = true;
                }
                (Some(next), _) => {
                    now = next;
                    continue;
                }
                (None, Some(due)) => {
                    now = due;
                    settling = true;
                }
                (None, None) => break,
            }
        } else if held > now {
            now = held;
            continue;
        }

        let d = desk.as_ref().unwrap();
        let (w, h) = d.size;
        if encoder.as_ref().is_none_or(|e| e.size() != d.size) {
            let mut coder = h264.coder().map_err(|e| format!("{e:#}"))?;
            encoder = Some(VideoEncoder::new(&mut coder, w, h, chroma)?);
            asked_key = true;
            damage = None;
        }
        let encoder = encoder.as_mut().unwrap();
        let keyframe = asked_key;
        let quality = if settling {
            rfb::QUALITY_MAX
        } else {
            walk.quality()
        };
        out.clear();
        let (started, cpu_from) = (Instant::now(), process_cpu());
        let produced = if settling {
            encoder.settle(&mut out)?
        } else {
            encoder.read(
                &d.pixels,
                usize::from(w) * 4,
                if keyframe { None } else { damage.as_deref() },
            )?;
            // The encoder making no frame of a picture is the session's to
            // skip, as it skips it.
            encoder.encode(keyframe, quality, &mut out)?
        };
        let spent = process_cpu().saturating_sub(cpu_from);
        cpu += spent;
        let encode =
            encode_ms.map_or_else(|| started.elapsed(), |ms| Duration::from_secs_f64(ms / 1e3));
        dirty = false;
        damage = Some(Vec::new());
        asked_key = false;
        if !produced {
            continue;
        }
        page.draw(
            &out[wlshare_proto::video::FRAME_HEADER_LEN..],
            (usize::from(w), usize::from(h)),
        )?;
        let close = page.psnr(&d.pixels);
        psnrs.push(close);
        end = close;
        if let Some(stream) = &mut stream {
            stream.write_all(&out)?;
        }
        let sent = now + encode;
        shots.update(sent, &page.canvas, &d.pixels, page.size)?;
        // The rectangle's body, its length word included, and the thirty
        // bytes this model counts for the framing around it.
        let size = out.len() + 30;
        let (painted, back) = gateway.frame(sent, &[(size, out.len())], paint + jitter());
        if keyframe {
            walk.keyframe(base + sent);
        }
        // The desktop stopping settles the walk, at the first settle frame.
        if settling && !quiet {
            walk.settle(base + sent);
        }
        quiet = settling;
        frame_sent = Some(sent);
        let delivery = back - sent;
        let verdict = !keyframe && !settling;
        let latency = painted.saturating_sub(d.at);
        shown.extend(
            unshown
                .drain(..)
                .map(|at: Duration| (painted - at).as_secs_f64() * 1e3),
        );
        frames += 1;
        bytes += size;
        keyframes += u32::from(keyframe);
        if settling {
            settles += 1;
            settle_bytes += size;
        }
        if verdict {
            motion_frames += 1;
            deliveries.push(delivery.as_secs_f64() * 1e3);
            qualities.push(f64::from(quality));
        }
        if let Some(t) = &mut timeline {
            let kind = match (keyframe, settling) {
                (true, _) => "key",
                (false, true) => "settle",
                (false, false) => "frame",
            };
            writeln!(
                t,
                "{{\"t_ms\":{:.1},\"kind\":\"{kind}\",\"quality\":{quality},\"interval_ms\":{:.0},\"bytes\":{size},\"encode_ms\":{:.1},\"cpu_ms\":{:.1},\"delivery_ms\":{:.1},\"latency_ms\":{:.1},\"psnr_db\":{close:.2}}}",
                sent.as_secs_f64() * 1e3,
                walk.interval().as_secs_f64() * 1e3,
                encode.as_secs_f64() * 1e3,
                spent.as_secs_f64() * 1e3,
                delivery.as_secs_f64() * 1e3,
                latency.as_secs_f64() * 1e3,
            )?;
        }
        // The fence comes back; the next frame waits for it.
        now = back;
        if let Some(pace) = walk.fenced(delivery, verdict, base + now)
            && pace != pace_was
        {
            moves += 1;
            downs +=
                u32::from(pace.quality < pace_was.quality || pace.interval > pace_was.interval);
            pace_was = pace;
        }
    }
    if let Some(desk) = &desk {
        shots.end(&page.canvas, &desk.pixels, page.size)?;
    }
    // Through the last frame's paint, which its own fence can come back before.
    let span = now.max(gateway.painted).as_secs_f64();
    println!(
        "{{\"capture\":{},\"picture\":\"video\",\"chroma\":\"{}\",\"encoder\":{},\"link\":{link_json},\"rtt_ms\":{rtt},\"ceiling\":{},\"frames\":{frames},\"motion_frames\":{motion_frames},\"keyframes\":{keyframes},\"settles\":{settles},\"settle_bytes\":{settle_bytes},\"fps\":{},\"motion_q_mean\":{},\"motion_q_p10\":{},\"shown_mean_ms\":{},\"shown_p50_ms\":{},\"shown_p95_ms\":{},\"shown_max_ms\":{},\"delivery_p50_ms\":{},\"delivery_p95_ms\":{},\"mbps\":{},\"moves\":{moves},\"downs\":{downs},\"psnr_mean_db\":{:.2},\"psnr_p10_db\":{:.2},\"psnr_end_db\":{end:.2},\"encoder_cpu_pct\":{:.1}}}",
        json(&name),
        match chroma {
            Chroma::Yuv420 => "420",
            Chroma::Yuv444 => "444",
        },
        json(match chroma {
            Chroma::Yuv420 => h264.name(),
            Chroma::Yuv444 => "libx264",
        }),
        rfb::QUALITY,
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
        cpu.as_secs_f64() / span * 100.0,
    );
    Ok(())
}
