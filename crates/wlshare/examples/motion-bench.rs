//! The tiles encoding ([`crate::rfb::tiles`]) measured against the VP9
//! encoding on captures ([`wlshare_vp9::capture`]): every frame coded both
//! ways, and what each cost the encoder and the link.
//!
//! ```text
//! motion-bench [--threads N] [--quality Q] [--walk-at Q] [--libx264]
//!              [--wlsv-chroma 444|420 | --tiles-only] [--frames FILE] CAPTURE...
//! motion-bench check [--threads N] [--quality Q] [--libx264] CAPTURE...
//! ```
//!
//! An example of the daemon's crate, since its regions are the daemon's H.264
//! ([`h264`], and [`decode`] to read them back), so it runs where the daemon
//! builds. A capture ending in `.zst` is read through `zstd -dc`, which must
//! then be on the path.
//!
//! **`WLSV`** is coded as `vp9cap streams` codes it: one stream per size, each
//! frame its rectangles at its dial, a keyframe where one was asked, 4:4:4
//! unless `--wlsv-chroma` says otherwise. A stream the walk left coarse at the end is settled once at the
//! capture's highest dial, as a session settles a quiet desktop.
//! `--tiles-only` codes none, for a change to the tiles measured against the
//! `WLSV` a run before measured.
//!
//! **Tiles** is the encoder a session runs, one per size: each frame read with
//! its rectangles at the time it was captured and encoded, a keyframe asked
//! being a picture the client asks for whole ([`TilesEncoder::forget`]), and
//! between the frames, and after the last until nothing is owed (60 s at
//! most), the encode its tick is owed ([`TilesEncoder::tick_at`]), nothing
//! changing. Its dial is the capture's unless `--quality` names one, held
//! there, the picture exact as a list that holds the dial asks; `--walk-at`
//! codes the picture a walked list asks for, the walk held at its dial, the
//! ceiling or below it, every still a WebP at it, owed again at the ceiling
//! where it is below, as a link that is behind is
//! sent them ([`TilesEncoder::set_pace`]). Its
//! regions are the daemon's H.264 encoder, found as the daemon finds it
//! ([`h264::Encoder::find`]): the hardware's where VA-API opens one, libx264
//! otherwise or with `--libx264`, each tiles picture on a coder of its own as
//! each session's is.
//!
//! **Cost** is the process's CPU time over each coder's own work, both coders
//! in one pass over the capture: reading it, `zstd`, and the decoders that
//! check the result all run outside. `WLSV`'s encoder has `--threads` (4);
//! libx264's region streams have the daemon's count, the machine's less two
//! at most 8, and are coded one after another with the PNGs, so the CPU time
//! is the measure and not the wall clock. The hardware's cost the processor
//! the conversion and the upload, which is what is counted of them. As every
//! measurement of the encoder, run it on an idle machine and alone.
//!
//! **Checked**: both pictures are decoded as a page would — `WLSV` by libvpx;
//! the tiles' update parsed by the client's parser, a PNG by the `png` crate
//! and each region by a libavcodec software H.264 decoder of its own, its
//! picture turned back from BT.601 at studio swing by `yuv` — and compared with the
//! capture's: the PSNR over B, G and R, and for the tiles how many pixels
//! differ, which once settled is none.
//!
//! The output is JSON lines, a JSON object a line, one per capture per coder,
//! which `scripts/motion-tables.py` makes tables of: `capture`, `coder`
//! (`wlsv` or `tiles`), `chroma` (`WLSV`'s, `444` or `420`; `null` for the
//! tiles), `quality` (the ceiling), `walk_at` (`--walk-at`, or `null` for a
//! held picture), `frames`, `seconds`; its CPU time all of it, `cpu_ms`, the
//! tiles' ticks and tail included, and the time a frame took the frames'
//! alone, `frame_ms` and `p95_ms`; `bytes`, and of them `png`, `webp` and
//! `video` (the regions' H.264, or `WLSV`'s VP9), the tiles' `copies`, which
//! carry no payload, `streams` started and
//! `keyframes`; what the page held while the capture ran, `held_mse`, the mean
//! squared error over B, G and R averaged over every frame, and `exact_frames`,
//! the frames it held exactly; where it ended, `end_off` pixels off and
//! `end_mse`; and the tail, `settled`, `tail_s`, `tail_cpu_ms`, and its longest
//! tick and its largest, `tail_tick_ms` and `tail_tick_bytes`. `--frames`
//! writes one object per frame to FILE: `capture`, `frame`, `ms`, `changed`,
//! `wlsv_us`, `wlsv_bytes` (`null` with `--tiles-only`), `tiles_us`,
//! `tiles_bytes`, `png`, `webp`, `h264`, `streams` (live), `copies`, `tick_us` and
//! `tick_bytes`, a tick's what the ticks since the frame before cost.
//!
//! **`check`** holds the tiles to what a picture that stops owes, on any
//! capture and on the `still` ones above all, which are for this and no
//! measurement: two whole pictures say nothing of a coder's cost. The capture
//! is coded and the tick run on until nothing is owed; then 5 s more, nothing
//! changing, and 10 redraws of the whole picture with the pixels it already
//! has, as a compositor repaints what it shows, must send nothing, and the
//! page must hold the picture exactly. Each capture is a JSON object:
//! `capture`, `settled`, `settled_s`, `quiet_bytes`, `redraw_bytes`, `end_off`,
//! `end_mse` and `pass`; a capture that fails makes the exit 1.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cpu_time::ProcessTime;
use wlshare_proto::tiles::TilesPicture;
use wlshare_vp9::Decoder;
use wlshare_vp9::capture::Reader;
use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix};

use crate::rfb::client::{self, RectBody, ServerMsg, TileBody};
use crate::rfb::msg;
use crate::rfb::tiles::TilesEncoder;
use crate::rfb::vp9::{Chroma, Rect, Vp9Encoder};

// The daemon's own modules, of which the bench uses the encoders, the
// decoder and the client's half of the wire, and nothing more.
#[allow(dead_code)]
#[path = "../src/decode.rs"]
mod decode;
#[allow(dead_code)]
#[path = "../src/h264.rs"]
mod h264;
#[allow(unused)]
#[path = "../src/rfb/mod.rs"]
mod rfb;

type Error = Box<dyn std::error::Error>;

const USAGE: &str = "usage: motion-bench [--threads N] [--quality Q] [--walk-at Q] [--libx264] [--wlsv-chroma 444|420 | --tiles-only] [--frames FILE] CAPTURE...
       motion-bench check [--threads N] [--quality Q] [--libx264] CAPTURE...";

/// How long the tick runs on after a capture's last frame, at most.
const TAIL: Duration = Duration::from_secs(60);

struct Args {
    captures: Vec<PathBuf>,
    threads: usize,
    quality: Option<u8>,
    /// The tiles walked, their walk held at this dial, the ceiling or below.
    walk_at: Option<u8>,
    /// libx264 for the regions, the hardware not looked for.
    libx264: bool,
    wlsv_chroma: Chroma,
    /// The tiles alone, no `WLSV` coded.
    tiles_only: bool,
    frames: Option<PathBuf>,
    /// `check` rather than the bench.
    check: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut parsed = Args { captures: Vec::new(), threads: 4, quality: None, walk_at: None, libx264: false, wlsv_chroma: Chroma::Yuv444, tiles_only: false, frames: None, check: false };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} takes a value"));
        match arg.as_str() {
            "--threads" => parsed.threads = value()?.parse().ok().filter(|&n| n > 0).ok_or("--threads takes a count of at least 1")?,
            "--quality" => parsed.quality = Some(value()?.parse().ok().filter(|q| (1..=100).contains(q)).ok_or("--quality takes 1 to 100")?),
            "--walk-at" => parsed.walk_at = Some(value()?.parse().ok().filter(|q| (1..=100).contains(q)).ok_or("--walk-at takes 1 to 100")?),
            "--libx264" => parsed.libx264 = true,
            "--wlsv-chroma" => {
                parsed.wlsv_chroma = match value()?.as_str() {
                    "444" => Chroma::Yuv444,
                    "420" => Chroma::Yuv420,
                    other => return Err(format!("{other}: a chroma is 444 or 420")),
                }
            }
            "--tiles-only" => parsed.tiles_only = true,
            "--frames" => parsed.frames = Some(PathBuf::from(value()?)),
            flag if flag.starts_with("--") => return Err(format!("{flag}: not a flag")),
            "check" if parsed.captures.is_empty() && !parsed.check => parsed.check = true,
            _ => parsed.captures.push(PathBuf::from(arg)),
        }
    }
    if parsed.captures.is_empty() {
        return Err("a capture to read".into());
    }
    if parsed.check && (parsed.frames.is_some() || parsed.wlsv_chroma != Chroma::Yuv444 || parsed.tiles_only) {
        return Err("check codes no WLSV and writes no frames".into());
    }
    if parsed.check && parsed.walk_at.is_some() {
        return Err("check holds the dial: a walked picture ends lossy".into());
    }
    if parsed.tiles_only && parsed.wlsv_chroma != Chroma::Yuv444 {
        return Err("--tiles-only codes no WLSV".into());
    }
    Ok(parsed)
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("motion-bench: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    // libavcodec's own log at warnings, as the daemon sets it: what it says at
    // its default level is libx264's summary of every region's stream.
    // SAFETY: a store of the level, before any thread uses libavcodec.
    unsafe { ffmpeg_sys_next::av_log_set_level(ffmpeg_sys_next::AV_LOG_WARNING) };
    let encoder = match h264::Encoder::find(!args.libx264) {
        Ok(encoder) => encoder,
        Err(e) => {
            eprintln!("motion-bench: {e:#}");
            std::process::exit(1);
        }
    };
    eprintln!("motion-bench: the regions are H.264 by {}", encoder.name());
    if args.check {
        let mut failed = false;
        for capture in &args.captures {
            match check(capture, &args, &encoder) {
                Ok(passed) => failed |= !passed,
                Err(e) => {
                    eprintln!("motion-bench: {}: {e}", capture.display());
                    failed = true;
                }
            }
        }
        std::process::exit(i32::from(failed));
    }
    let mut frames = match args.frames.as_deref().map(File::create).transpose() {
        Ok(file) => file.map(BufWriter::new),
        Err(e) => {
            eprintln!("motion-bench: the frames: {e}");
            std::process::exit(1);
        }
    };
    let mut failed = false;
    for capture in &args.captures {
        if let Err(e) = bench(capture, &args, &encoder, frames.as_mut()) {
            eprintln!("motion-bench: {}: {e}", capture.display());
            failed = true;
        }
    }
    if let Some(frames) = &mut frames
        && let Err(e) = frames.flush()
    {
        eprintln!("motion-bench: the frames: {e}");
        failed = true;
    }
    if failed {
        std::process::exit(1);
    }
}

// ── The capture in ───────────────────────────────────────────────────────────

/// A capture open for reading, with the `zstd` it is read through, if any.
struct Source {
    reader: Reader<Box<dyn Read>>,
    zstd: Option<Child>,
}

impl Source {
    fn open(path: &Path) -> Result<Self, Error> {
        let (input, zstd): (Box<dyn Read>, _) = if path.extension().is_some_and(|e| e == "zst") {
            let file = File::open(path)?;
            let mut child = Command::new("zstd").arg("-dc").stdin(file).stdout(Stdio::piped()).spawn().map_err(|e| format!("running zstd: {e}"))?;
            (Box::new(child.stdout.take().expect("piped")), Some(child))
        } else {
            (Box::new(File::open(path)?), None)
        };
        Ok(Self { reader: Reader::new(Box::new(BufReader::with_capacity(1 << 20, input)) as Box<dyn Read>)?, zstd })
    }

    /// After the capture's end was read: whether `zstd` read all of its own.
    fn finish(mut self) -> Result<(), Error> {
        if let Some(mut child) = self.zstd.take()
            && !child.wait()?.success()
        {
            return Err("zstd could not decompress it".into());
        }
        Ok(())
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        if let Some(child) = &mut self.zstd {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ── The tiles, and the page that checks what they send ───────────────────────

/// What a coder sent, by kind: each payload's bytes, as a link carries them
/// less the rectangles' headers.
#[derive(Default)]
struct Sent {
    png: u64,
    webp: u64,
    /// The frames of the streams: the tiles' regions' H.264, or `WLSV`'s VP9.
    video: u64,
    starts: u64,
    /// The tiles' copies, which carry no payload.
    copies: u64,
    /// The tiles drawn from the page's cache, which carry no payload either.
    cached: u64,
}

impl Sent {
    fn bytes(&self) -> u64 {
        self.png + self.webp + self.video
    }

    fn add(&mut self, other: &Sent) {
        self.png += other.png;
        self.webp += other.webp;
        self.video += other.video;
        self.starts += other.starts;
        self.copies += other.copies;
        self.cached += other.cached;
    }
}

/// What a page draws of the tiles encoding.
struct Page {
    stride: usize,
    canvas: Vec<u8>,
    decoders: HashMap<u8, decode::Decoder>,
    /// The cache of tiles, each slot a tile's `B, G, R, A` rows.
    slots: HashMap<u16, Vec<u8>>,
    /// A decoded region's planes, and its picture as `B, G, R, A`, or a
    /// copy's source.
    planes: Vec<u8>,
    scratch: Vec<u8>,
}

impl Page {
    fn new((w, h): (u16, u16)) -> Self {
        let stride = usize::from(w) * 4;
        Self { stride, canvas: vec![0; stride * usize::from(h)], decoders: HashMap::new(), slots: HashMap::new(), planes: Vec::new(), scratch: Vec::new() }
    }

    /// One update of `count` rectangles, parsed as a client parses it and drawn.
    fn draw(&mut self, count: u16, rects: &[u8]) -> Result<Sent, Error> {
        let mut sent = Sent::default();
        if count == 0 {
            return Ok(sent);
        }
        let mut wire = msg::update_header(count).to_vec();
        wire.extend_from_slice(rects);
        let Some((ServerMsg::Update(rects), used)) = client::parse(&wire)? else { return Err("an update cut short".into()) };
        if used != wire.len() {
            return Err("bytes past the update's rectangles".into());
        }
        for rect in rects {
            let RectBody::Tiles(body) = rect.body else { return Err("a rectangle of another encoding".into()) };
            let (x, y, w, h) = (usize::from(rect.x), usize::from(rect.y), usize::from(rect.width), usize::from(rect.height));
            match body {
                TileBody::Png(png) => {
                    sent.png += png.len() as u64;
                    let mut decoder = png::Decoder::new(std::io::Cursor::new(&png));
                    decoder.set_transformations(png::Transformations::EXPAND);
                    let mut reader = decoder.read_info()?;
                    let mut rgb = vec![0; reader.output_buffer_size().ok_or("a PNG too large")?];
                    let info = reader.next_frame(&mut rgb)?;
                    if (info.width as usize, info.height as usize, info.color_type) != (w, h, png::ColorType::Rgb) {
                        return Err(format!("a PNG of {}x{} {:?} for {w}x{h}", info.width, info.height, info.color_type).into());
                    }
                    self.paste((x, y, w, h), &rgb);
                }
                TileBody::Webp(webp) => {
                    sent.webp += webp.len() as u64;
                    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(&webp))?;
                    if decoder.dimensions() != (w as u32, h as u32) || decoder.has_alpha() {
                        return Err(format!("a WebP of {:?} with alpha {} for {w}x{h}", decoder.dimensions(), decoder.has_alpha()).into());
                    }
                    let mut rgb = vec![0; decoder.output_buffer_size().ok_or("a WebP too large")?];
                    decoder.read_image(&mut rgb)?;
                    self.paste((x, y, w, h), &rgb);
                }
                TileBody::Start { stream, frame } => {
                    sent.video += frame.len() as u64;
                    sent.starts += 1;
                    if self.decoders.insert(stream, decode::Decoder::new()?).is_some() {
                        return Err(format!("stream {stream} started while live").into());
                    }
                    self.region(stream, (x, y, w, h), &frame)?;
                }
                TileBody::Frame { stream, frame } => {
                    sent.video += frame.len() as u64;
                    self.region(stream, (x, y, w, h), &frame)?;
                }
                TileBody::End { stream } => {
                    self.decoders.remove(&stream).ok_or_else(|| format!("stream {stream} ended while not live"))?;
                }
                TileBody::Copy { x: from_x, y: from_y } => {
                    sent.copies += 1;
                    let (from_x, from_y) = (usize::from(from_x), usize::from(from_y));
                    if (from_x + w) * 4 > self.stride || (from_y + h) * self.stride > self.canvas.len() {
                        return Err(format!("a copy from {from_x},{from_y} of {w}x{h}, past the desktop").into());
                    }
                    // Copied out first, so a source the rectangle overlaps is read whole.
                    self.scratch.clear();
                    for row in from_y..from_y + h {
                        self.scratch.extend_from_slice(&self.canvas[row * self.stride + from_x * 4..][..w * 4]);
                    }
                    for (row, from) in (y..y + h).zip(self.scratch.chunks_exact(w * 4)) {
                        self.canvas[row * self.stride + x * 4..][..w * 4].copy_from_slice(from);
                    }
                }
                TileBody::Store { slot } => {
                    let tile = usize::from(wlshare_proto::tiles::TILE);
                    if (w, h) != (tile, tile) || x % tile != 0 || y % tile != 0 {
                        return Err(format!("a store of {w}x{h}+{x}+{y}, not one tile on the grid").into());
                    }
                    let kept: Vec<u8> = (y..y + h).flat_map(|row| self.canvas[row * self.stride + x * 4..][..w * 4].to_vec()).collect();
                    self.slots.insert(slot, kept);
                }
                TileBody::Cached { slot } => {
                    sent.cached += 1;
                    let tile = usize::from(wlshare_proto::tiles::TILE);
                    if (w, h) != (tile, tile) {
                        return Err(format!("a cached draw of {w}x{h}, not one tile").into());
                    }
                    let kept = self.slots.get(&slot).ok_or_else(|| format!("slot {slot} drawn from before it was stored"))?;
                    for (row, from) in (y..y + h).zip(kept.chunks_exact(w * 4)) {
                        self.canvas[row * self.stride + x * 4..][..w * 4].copy_from_slice(from);
                    }
                }
            }
        }
        Ok(sent)
    }

    /// Packed RGB drawn at the rectangle.
    fn paste(&mut self, (x, y, w, h): (usize, usize, usize, usize), rgb: &[u8]) {
        let mut pixels = rgb.as_chunks::<3>().0.iter();
        for row in y..y + h {
            let at = row * self.stride + x * 4;
            for px in self.canvas[at..at + w * 4].as_chunks_mut::<4>().0 {
                let c = pixels.next().expect("a pixel for every one");
                px[..3].copy_from_slice(&[c[2], c[1], c[0]]);
            }
        }
    }

    fn region(&mut self, stream: u8, (x, y, w, h): (usize, usize, usize, usize), frame: &[u8]) -> Result<(), Error> {
        let decoder = self.decoders.get_mut(&stream).ok_or_else(|| format!("a frame of stream {stream}, which is not live"))?;
        let picture = decoder.decode(frame, &mut self.planes)?.ok_or_else(|| format!("a frame of stream {stream} decoded to no picture"))?;
        // The picture is the region's size rounded up to even sides, cut to
        // the region from its top-left.
        let (pw, ph) = (picture.width, picture.height);
        if (pw, ph) != (w.next_multiple_of(2), h.next_multiple_of(2)) {
            return Err(format!("a region of {w}x{h} decoded as {pw}x{ph}").into());
        }
        let (cw, ch) = picture.chroma();
        let (luma, chroma) = self.planes.split_at(pw * ph);
        let (u, v) = chroma.split_at(cw * ch);
        let planar = YuvPlanarImage { y_plane: luma, y_stride: pw as u32, u_plane: u, u_stride: cw as u32, v_plane: v, v_stride: cw as u32, width: pw as u32, height: ph as u32 };
        self.scratch.resize(pw * ph * 4, 0);
        yuv::yuv420_to_bgra(&planar, &mut self.scratch, (pw * 4) as u32, YuvRange::Limited, YuvStandardMatrix::Bt601)?;
        for (row, from) in (y..y + h).zip(self.scratch.chunks_exact(pw * 4)) {
            let at = row * self.stride + x * 4;
            for (to, from) in self.canvas[at..at + w * 4].as_chunks_mut::<4>().0.iter_mut().zip(from.as_chunks::<4>().0) {
                to[..3].copy_from_slice(&from[..3]);
            }
        }
        Ok(())
    }
}

/// The tiles of one size: the encoder, the page it draws, and the update it
/// appends to.
struct Tiles {
    encoder: TilesEncoder,
    page: Page,
    out: Vec<u8>,
}

impl Tiles {
    /// The tiles of a desktop of `size` as `picture` asks, its regions on a
    /// coder of `encoder`'s opened for it, as a session opens one.
    fn new(size: (u16, u16), picture: TilesPicture, encoder: &h264::Encoder) -> Result<Self, Error> {
        Ok(Self { encoder: TilesEncoder::new(size.0, size.1, picture, Box::new(encoder.coder()?))?, page: Page::new(size), out: Vec::new() })
    }

    /// Encode what is owed at `now` and draw it: the CPU time of the encode,
    /// and what it sent.
    fn encode(&mut self, now: Duration) -> Result<(Duration, Sent), Error> {
        self.out.clear();
        let cpu = ProcessTime::now();
        let update = self.encoder.encode(now, &mut self.out)?;
        let cost = cpu.elapsed();
        Ok((cost, self.page.draw(update.rects, &self.out)?))
    }

    /// One captured frame read and encoded, its read's CPU time counted too:
    /// `whole` for a picture the client asks for whole, `quality` where the
    /// walk stands.
    fn frame(&mut self, pixels: &[u8], changed: Option<&[Rect]>, whole: bool, quality: u8, now: Duration) -> Result<(Duration, Sent), Error> {
        let stride = usize::from(self.encoder.size().0) * 4;
        let cpu = ProcessTime::now();
        if whole {
            self.encoder.forget();
        }
        self.encoder.set_pace(quality, false);
        self.encoder.read(pixels, stride, changed, now)?;
        let read = cpu.elapsed();
        let (cost, sent) = self.encode(now)?;
        Ok((read + cost, sent))
    }

    /// Every tick owed by `until`.
    fn ticks(&mut self, until: Duration) -> Result<(Duration, Sent), Error> {
        let (mut cpu, mut all) = (Duration::ZERO, Sent::default());
        while let Some(at) = self.encoder.tick_at().filter(|at| *at <= until) {
            let (cost, sent) = self.encode(at)?;
            cpu += cost;
            all.add(&sent);
        }
        Ok((cpu, all))
    }
}

/// How many pixels of two `B, G, R, X` pictures differ in B, G or R, and the
/// mean squared error over those three.
fn compare(a: &[u8], b: &[u8]) -> (u64, f64) {
    let (mut off, mut squared) = (0u64, 0u64);
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let mut differs = false;
        for c in 0..3 {
            let d = i64::from(pa[c]) - i64::from(pb[c]);
            squared += (d * d) as u64;
            differs |= d != 0;
        }
        off += u64::from(differs);
    }
    (off, squared as f64 / (a.len() / 4 * 3) as f64)
}

/// A string as JSON.
fn json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── WLSV ─────────────────────────────────────────────────────────────────────

/// `WLSV` at one size, and its decoder.
struct Wlsv {
    encoder: Vp9Encoder,
    decoder: Decoder,
    started: bool,
    picture: Vec<u8>,
    out: Vec<u8>,
}

impl Wlsv {
    /// The encoder and what making it cost, as a region stream's making is
    /// the tiles' cost.
    fn new((w, h): (u16, u16), quality: u8, chroma: Chroma, threads: usize) -> Result<(Self, Duration), Error> {
        let decoder = Decoder::new(2)?;
        let cpu = ProcessTime::now();
        let encoder = Vp9Encoder::with_threads(w, h, quality, chroma, threads)?;
        let made = cpu.elapsed();
        Ok((Self { encoder, decoder, started: false, picture: vec![0; usize::from(w) * 4 * usize::from(h)], out: Vec::new() }, made))
    }

    /// Decode what the last encode appended, if anything.
    fn check(&mut self) -> Result<(), Error> {
        if self.out.len() > 5 {
            let (w, _) = self.encoder.size();
            self.decoder.decode(&self.out[5..])?.write_bgrx(&mut self.picture, usize::from(w) * 4)?;
        }
        Ok(())
    }
}

// ── The bench ────────────────────────────────────────────────────────────────

/// What one coder cost over a capture.
#[derive(Default)]
struct Tally {
    frames: u64,
    cpu: Duration,
    per_frame: Vec<Duration>,
    sent: Sent,
    keyframes: u64,
    tail_cpu: Duration,
    tail: Duration,
    /// The tail's longest tick and its largest, in bytes.
    tail_tick: Duration,
    tail_tick_bytes: u64,
    settled: bool,
    /// The page against the source after each frame: the squared error
    /// summed, and the frames it held exactly.
    mse: f64,
    exact: u64,
}

impl Tally {
    /// What the page held after a frame.
    fn held(&mut self, page: &[u8], source: &[u8]) {
        let (off, mse) = compare(page, source);
        self.mse += mse;
        self.exact += u64::from(off == 0);
    }
}

/// One size of a capture, coded both ways, or as the tiles alone.
struct Run {
    size: (u16, u16),
    wlsv: Option<Wlsv>,
    tiles: Tiles,
}

fn bench(path: &Path, args: &Args, encoder: &h264::Encoder, mut frames: Option<&mut BufWriter<File>>) -> Result<(), Error> {
    let name = capture_name(path);
    let mut source = Source::open(path)?;
    let (mut wlsv, mut tiles) = (Tally::default(), Tally::default());
    let mut run: Option<Run> = None;
    let (mut first_at, mut last_at, mut ceiling) = (None, Duration::ZERO, 0u8);
    let mut picture = Vec::new();
    while let Some(frame) = source.reader.next()? {
        let quality = args.quality.unwrap_or(frame.quality);
        ceiling = ceiling.max(quality);
        let now = frame.at;
        first_at.get_or_insert(now);
        last_at = now;
        let stride = usize::from(frame.size.0) * 4;
        let mut setup = Duration::ZERO;
        let fresh = run.as_ref().is_none_or(|r| r.size != frame.size);
        if fresh {
            let coder = if args.tiles_only {
                None
            } else {
                let (coder, cost) = Wlsv::new(frame.size, quality, args.wlsv_chroma, args.threads)?;
                setup = cost;
                Some(coder)
            };
            run = Some(Run { size: frame.size, wlsv: coder, tiles: Tiles::new(frame.size, TilesPicture { quality, adaptive: args.walk_at.is_some() }, encoder)? });
        }
        let run = run.as_mut().expect("just made");

        // The ticks owed before this frame.
        let (tick_cpu, tick_sent) = run.tiles.ticks(now)?;
        tiles.cpu += tick_cpu;
        tiles.sent.add(&tick_sent);

        // WLSV: what it cost and sent.
        let mut wlsv_spent = None;
        if let Some(w) = run.wlsv.as_mut() {
            let cpu = ProcessTime::now();
            if w.encoder.quality() != quality {
                w.encoder.set_quality(quality);
            }
            w.out.clear();
            w.encoder.read(frame.pixels, stride, if w.started { frame.changed } else { None })?;
            let made = w.encoder.encode(!w.started || frame.keyframe, &mut w.out)?;
            let wlsv_cpu = cpu.elapsed() + setup;
            w.started = true;
            wlsv.frames += 1;
            wlsv.cpu += wlsv_cpu;
            wlsv.per_frame.push(wlsv_cpu);
            let wlsv_bytes = if made { w.out.len() as u64 - 5 } else { 0 };
            wlsv.sent.video += wlsv_bytes;
            if made && wlshare_vp9::frame_header(&w.out[5..]).is_some_and(|header| header.keyframe) {
                wlsv.keyframes += 1;
            }
            w.check()?;
            wlsv.held(&w.picture, frame.pixels);
            wlsv_spent = Some((wlsv_cpu, wlsv_bytes));
        }

        // The tiles.
        let (tiles_cpu, sent) = run.tiles.frame(frame.pixels, frame.changed, frame.keyframe && !fresh, args.walk_at.unwrap_or(quality), now)?;
        tiles.frames += 1;
        tiles.cpu += tiles_cpu;
        tiles.per_frame.push(tiles_cpu);
        tiles.sent.add(&sent);
        tiles.held(&run.tiles.page.canvas, frame.pixels);
        picture.clear();
        picture.extend_from_slice(frame.pixels);

        if let Some(frames) = frames.as_deref_mut() {
            writeln!(
                frames,
                "{{\"capture\":{},\"frame\":{},\"ms\":{},\"changed\":{},\"wlsv_us\":{},\"wlsv_bytes\":{},\"tiles_us\":{},\"tiles_bytes\":{},\"png\":{},\"webp\":{},\"h264\":{},\"streams\":{},\"copies\":{},\"cached\":{},\"tick_us\":{},\"tick_bytes\":{}}}",
                json(&name),
                frame.index,
                (now - first_at.expect("set")).as_millis(),
                frame.changed_pixels(),
                wlsv_spent.map_or_else(|| "null".to_owned(), |(cpu, _)| cpu.as_micros().to_string()),
                wlsv_spent.map_or_else(|| "null".to_owned(), |(_, bytes)| bytes.to_string()),
                tiles_cpu.as_micros(),
                sent.bytes(),
                sent.png,
                sent.webp,
                sent.video,
                run.tiles.page.decoders.len(),
                sent.copies,
                sent.cached,
                tick_cpu.as_micros(),
                tick_sent.bytes(),
            )?;
        }
    }
    source.finish()?;
    let mut run = run.ok_or("the capture holds no frames")?;

    // WLSV's settle, where the walk left it coarse.
    if let Some(w) = run.wlsv.as_mut()
        && w.encoder.coarsest() < ceiling
    {
        let cpu = ProcessTime::now();
        w.out.clear();
        let made = w.encoder.settle(ceiling, false, &mut w.out)?;
        wlsv.tail_cpu = cpu.elapsed();
        wlsv.cpu += wlsv.tail_cpu;
        if made {
            wlsv.sent.video += w.out.len() as u64 - 5;
        }
        w.check()?;
    }

    // The tiles' tail: the tick on, nothing changing, until nothing is owed.
    let mut ended = last_at;
    while let Some(at) = run.tiles.encoder.tick_at().filter(|at| *at <= last_at + TAIL) {
        let (cost, sent) = run.tiles.encode(at)?;
        tiles.tail_cpu += cost;
        tiles.tail_tick = tiles.tail_tick.max(cost);
        tiles.tail_tick_bytes = tiles.tail_tick_bytes.max(sent.bytes());
        tiles.sent.add(&sent);
        ended = at;
    }
    tiles.cpu += tiles.tail_cpu;
    tiles.settled = run.tiles.encoder.tick_at().is_none();
    tiles.tail = ended.saturating_sub(last_at);

    let seconds = last_at.saturating_sub(first_at.unwrap_or_default()).as_secs_f64();
    wlsv.settled = true;
    let chroma = match args.wlsv_chroma {
        Chroma::Yuv444 => "\"444\"",
        Chroma::Yuv420 => "\"420\"",
    };
    if let Some(w) = &run.wlsv {
        row(&name, "wlsv", chroma, args, ceiling, &wlsv, seconds, compare(&w.picture, &picture));
    }
    row(&name, "tiles", "null", args, ceiling, &tiles, seconds, compare(&run.tiles.page.canvas, &picture));
    Ok(())
}

/// A capture's name, without its extensions.
fn capture_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().trim_end_matches(".zst").trim_end_matches(".wlcap").to_owned())
}

// ── check ────────────────────────────────────────────────────────────────────

/// How long a settled picture is watched.
const QUIET: Duration = Duration::from_secs(5);
/// How many redraws of a settled picture are fed it, and how far apart.
const REDRAWS: u32 = 10;
const REDRAW_EVERY: Duration = Duration::from_millis(100);

/// Whether the tiles leave a capture's picture be once it is settled: see
/// the module's `check`.
fn check(path: &Path, args: &Args, encoder: &h264::Encoder) -> Result<bool, Error> {
    let name = capture_name(path);
    let mut source = Source::open(path)?;
    let mut run: Option<((u16, u16), Tiles)> = None;
    let mut last_at = Duration::ZERO;
    let mut picture = Vec::new();
    while let Some(frame) = source.reader.next()? {
        let now = frame.at;
        last_at = now;
        let quality = args.quality.unwrap_or(frame.quality);
        let fresh = run.as_ref().is_none_or(|(size, _)| *size != frame.size);
        if fresh {
            run = Some((frame.size, Tiles::new(frame.size, TilesPicture { quality, adaptive: false }, encoder)?));
        }
        let (_, tiles) = run.as_mut().expect("just made");
        tiles.ticks(now)?;
        tiles.frame(frame.pixels, frame.changed, frame.keyframe && !fresh, quality, now)?;
        picture.clear();
        picture.extend_from_slice(frame.pixels);
    }
    source.finish()?;
    let (size, mut tiles) = run.ok_or("the capture holds no frames")?;

    // Settled: the tick on until nothing is owed.
    let mut now = last_at;
    while let Some(at) = tiles.encoder.tick_at().filter(|at| *at <= last_at + TAIL) {
        tiles.encode(at)?;
        now = at;
    }
    let settled = tiles.encoder.tick_at().is_none();
    let settled_in = now.saturating_sub(last_at);

    // Quiet: nothing changes, and nothing may be sent, by a tick or by an
    // encode at the window's end, which a settled encoder owes no tick for.
    now += QUIET;
    let quiet = tiles.ticks(now)?.1.bytes() + tiles.encode(now)?.1.bytes();

    // Redrawn: the whole picture again, every pixel as it was.
    let whole = [Rect { x: 0, y: 0, width: size.0, height: size.1 }];
    let quality = tiles.encoder.quality();
    let mut redrawn = 0;
    for _ in 0..REDRAWS {
        now += REDRAW_EVERY;
        redrawn += tiles.ticks(now)?.1.bytes();
        redrawn += tiles.frame(&picture, Some(&whole), false, quality, now)?.1.bytes();
    }
    redrawn += tiles.ticks(now + QUIET)?.1.bytes();

    let (off, mse) = compare(&tiles.page.canvas, &picture);
    let passed = settled && quiet == 0 && redrawn == 0 && off == 0;
    let settled_s = if settled { settled_in.as_secs_f64().to_string() } else { "null".to_owned() };
    println!(
        "{{\"capture\":{},\"settled\":{settled},\"settled_s\":{settled_s},\"quiet_bytes\":{quiet},\"redraw_bytes\":{redrawn},\"end_off\":{off},\"end_mse\":{mse},\"pass\":{passed}}}",
        json(&name)
    );
    Ok(passed)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// One coder's line: `chroma` is already JSON, and `end` what the page
/// ended off by, its pixels and its mean squared error.
#[allow(clippy::too_many_arguments)]
fn row(name: &str, coder: &str, chroma: &str, args: &Args, ceiling: u8, tally: &Tally, seconds: f64, (end_off, end_mse): (u64, f64)) {
    let mut per_frame = tally.per_frame.clone();
    per_frame.sort_unstable();
    let p95 = per_frame.get((per_frame.len() * 95 / 100).min(per_frame.len().saturating_sub(1))).copied().unwrap_or_default();
    let mean = if tally.frames == 0 { 0.0 } else { ms(per_frame.iter().sum()) / tally.frames as f64 };
    let held_mse = if tally.frames == 0 { 0.0 } else { tally.mse / tally.frames as f64 };
    let walk_at = args.walk_at.map_or_else(|| "null".to_owned(), |q| q.to_string());
    println!(
        "{{\"capture\":{},\"coder\":{},\"chroma\":{chroma},\"quality\":{ceiling},\"walk_at\":{walk_at},\"frames\":{},\"seconds\":{seconds},\"cpu_ms\":{},\"frame_ms\":{mean},\"p95_ms\":{},\"bytes\":{},\"png\":{},\"webp\":{},\"video\":{},\"copies\":{},\"cached\":{},\"streams\":{},\"keyframes\":{},\"held_mse\":{held_mse},\"exact_frames\":{},\"end_off\":{end_off},\"end_mse\":{end_mse},\"settled\":{},\"tail_s\":{},\"tail_cpu_ms\":{},\"tail_tick_ms\":{},\"tail_tick_bytes\":{}}}",
        json(name),
        json(coder),
        tally.frames,
        ms(tally.cpu),
        ms(p95),
        tally.sent.bytes(),
        tally.sent.png,
        tally.sent.webp,
        tally.sent.video,
        tally.sent.copies,
        tally.sent.cached,
        tally.sent.starts,
        tally.keyframes,
        tally.exact,
        tally.settled,
        tally.tail.as_secs_f64(),
        ms(tally.tail_cpu),
        ms(tally.tail_tick),
        tally.tail_tick_bytes,
    );
}
