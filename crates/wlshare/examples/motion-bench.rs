//! The tiles encoding ([`crate::rfb::tiles`]) measured against the VP9
//! encoding on captures ([`wlshare_vp9::capture`]): every frame coded both
//! ways, and what each cost the encoder and the link.
//!
//! ```text
//! motion-bench [--threads N] [--quality Q] [--walk-at Q] [--libx264]
//!              [--wlsv-chroma 444|420] [--csv FILE] CAPTURE...
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
//! The output is a Markdown table, a row per capture per coder: its CPU time
//! all of it, the tiles' ticks and tail included, and the time a frame took
//! the frames' alone; what the page held while the capture ran, as the PSNR
//! of the mean squared error over every frame and the share of frames it held
//! exactly; and where it ended. `--csv` adds one line per frame:
//! `capture,frame,ms,changed,wlsv_us,wlsv_bytes,tiles_us,tiles_bytes,png_bytes,
//! webp_bytes,h264_bytes,streams,tick_us,tick_bytes`, a tick's columns what the ticks
//! since the frame before cost.
//!
//! **`check`** holds the tiles to what a picture that stops owes, on any
//! capture and on the `still` ones above all, which are for this and no
//! measurement: two whole pictures say nothing of a coder's cost. The capture
//! is coded and the tick run on until nothing is owed; then 5 s more, nothing
//! changing, and 10 redraws of the whole picture with the pixels it already
//! has, as a compositor repaints what it shows, must send nothing, and the
//! page must hold the picture exactly. A capture that fails says what was
//! sent, and the exit is 1.

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

const USAGE: &str = "usage: motion-bench [--threads N] [--quality Q] [--walk-at Q] [--libx264] [--wlsv-chroma 444|420] [--csv FILE] CAPTURE...
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
    csv: Option<PathBuf>,
    /// `check` rather than the bench.
    check: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut parsed = Args { captures: Vec::new(), threads: 4, quality: None, walk_at: None, libx264: false, wlsv_chroma: Chroma::Yuv444, csv: None, check: false };
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
            "--csv" => parsed.csv = Some(PathBuf::from(value()?)),
            flag if flag.starts_with("--") => return Err(format!("{flag}: not a flag")),
            "check" if parsed.captures.is_empty() && !parsed.check => parsed.check = true,
            _ => parsed.captures.push(PathBuf::from(arg)),
        }
    }
    if parsed.captures.is_empty() {
        return Err("a capture to read".into());
    }
    if parsed.check && (parsed.csv.is_some() || parsed.wlsv_chroma != Chroma::Yuv444) {
        return Err("check codes no WLSV and writes no CSV".into());
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
        println!("| capture | settled in | 5 s quiet | 10 redraws | ends at | |");
        println!("|---|---:|---:|---:|---|---|");
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
    let mut csv = match args.csv.as_deref().map(File::create).transpose() {
        Ok(file) => file.map(BufWriter::new),
        Err(e) => {
            eprintln!("motion-bench: the CSV: {e}");
            std::process::exit(1);
        }
    };
    if let Some(csv) = &mut csv
        && let Err(e) = writeln!(csv, "capture,frame,ms,changed,wlsv_us,wlsv_bytes,tiles_us,tiles_bytes,png_bytes,webp_bytes,h264_bytes,streams,tick_us,tick_bytes")
    {
        eprintln!("motion-bench: the CSV: {e}");
        std::process::exit(1);
    }
    println!("| capture | coder | frames | seconds | CPU ms | ms/frame | p95 ms/frame | MB | Mbit/s | while it ran | frames exact | ends at | detail |");
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|");
    let mut failed = false;
    for capture in &args.captures {
        if let Err(e) = bench(capture, &args, &encoder, csv.as_mut()) {
            eprintln!("motion-bench: {}: {e}", capture.display());
            failed = true;
        }
    }
    if let Some(csv) = &mut csv
        && let Err(e) = csv.flush()
    {
        eprintln!("motion-bench: the CSV: {e}");
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
    }
}

/// What a page draws of the tiles encoding.
struct Page {
    stride: usize,
    canvas: Vec<u8>,
    decoders: HashMap<u8, decode::Decoder>,
    /// A decoded region's planes, and its picture as `B, G, R, A`.
    planes: Vec<u8>,
    scratch: Vec<u8>,
}

impl Page {
    fn new((w, h): (u16, u16)) -> Self {
        let stride = usize::from(w) * 4;
        Self { stride, canvas: vec![0; stride * usize::from(h)], decoders: HashMap::new(), planes: Vec::new(), scratch: Vec::new() }
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

/// A mean squared error as PSNR, "exact" for none.
fn db(mse: f64) -> String {
    if mse == 0.0 { "exact".to_owned() } else { format!("{:.1} dB", 10.0 * (255.0f64 * 255.0 / mse).log10()) }
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

/// One size of a capture, coded both ways.
struct Run {
    size: (u16, u16),
    wlsv: Wlsv,
    tiles: Tiles,
}

fn bench(path: &Path, args: &Args, encoder: &h264::Encoder, mut csv: Option<&mut BufWriter<File>>) -> Result<(), Error> {
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
            let (coder, cost) = Wlsv::new(frame.size, quality, args.wlsv_chroma, args.threads)?;
            setup = cost;
            run = Some(Run { size: frame.size, wlsv: coder, tiles: Tiles::new(frame.size, TilesPicture { quality, adaptive: args.walk_at.is_some() }, encoder)? });
        }
        let run = run.as_mut().expect("just made");

        // The ticks owed before this frame.
        let (tick_cpu, tick_sent) = run.tiles.ticks(now)?;
        tiles.cpu += tick_cpu;
        tiles.sent.add(&tick_sent);

        // WLSV.
        let cpu = ProcessTime::now();
        let w = &mut run.wlsv;
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

        // The tiles.
        let (tiles_cpu, sent) = run.tiles.frame(frame.pixels, frame.changed, frame.keyframe && !fresh, args.walk_at.unwrap_or(quality), now)?;
        tiles.frames += 1;
        tiles.cpu += tiles_cpu;
        tiles.per_frame.push(tiles_cpu);
        tiles.sent.add(&sent);
        wlsv.held(&run.wlsv.picture, frame.pixels);
        tiles.held(&run.tiles.page.canvas, frame.pixels);
        picture.clear();
        picture.extend_from_slice(frame.pixels);

        if let Some(csv) = csv.as_deref_mut() {
            writeln!(
                csv,
                "{name},{},{},{},{},{wlsv_bytes},{},{},{},{},{},{},{},{}",
                frame.index,
                (now - first_at.expect("set")).as_millis(),
                frame.changed_pixels(),
                wlsv_cpu.as_micros(),
                tiles_cpu.as_micros(),
                sent.bytes(),
                sent.png,
                sent.webp,
                sent.video,
                run.tiles.page.decoders.len(),
                tick_cpu.as_micros(),
                tick_sent.bytes(),
            )?;
        }
    }
    source.finish()?;
    let mut run = run.ok_or("the capture holds no frames")?;

    // WLSV's settle, where the walk left it coarse.
    if run.wlsv.encoder.coarsest() < ceiling {
        let cpu = ProcessTime::now();
        run.wlsv.out.clear();
        let made = run.wlsv.encoder.settle(ceiling, false, &mut run.wlsv.out)?;
        wlsv.tail_cpu = cpu.elapsed();
        wlsv.cpu += wlsv.tail_cpu;
        if made {
            wlsv.sent.video += run.wlsv.out.len() as u64 - 5;
        }
        run.wlsv.check()?;
    }

    // The tiles' tail: the tick on, nothing changing, until nothing is owed.
    let mut ended = last_at;
    while let Some(at) = run.tiles.encoder.tick_at().filter(|at| *at <= last_at + TAIL) {
        let (cost, sent) = run.tiles.encode(at)?;
        tiles.tail_cpu += cost;
        tiles.sent.add(&sent);
        ended = at;
    }
    tiles.cpu += tiles.tail_cpu;
    tiles.settled = run.tiles.encoder.tick_at().is_none();
    tiles.tail = ended.saturating_sub(last_at);

    let seconds = last_at.saturating_sub(first_at.unwrap_or_default()).as_secs_f64();
    let (_, wlsv_mse) = compare(&run.wlsv.picture, &picture);
    let (off, tiles_mse) = compare(&run.tiles.page.canvas, &picture);
    let wlsv_detail = format!("{} keyframes{}", wlsv.keyframes, if wlsv.tail_cpu > Duration::ZERO { format!(", settled at {ceiling} for {:.1} ms", ms(wlsv.tail_cpu)) } else { String::new() });
    let wlsv_name = format!(
        "WLSV {}",
        match args.wlsv_chroma {
            Chroma::Yuv444 => "4:4:4",
            Chroma::Yuv420 => "4:2:0",
        }
    );
    row(&name, &wlsv_name, &wlsv, seconds, &db(wlsv_mse), &wlsv_detail);
    let ends = if off == 0 { "exact".to_owned() } else { format!("{off} px off, {}", db(tiles_mse)) };
    let tiles_detail = format!(
        "PNG {:.2} MB, WebP {:.2} MB, H.264 {:.2} MB in {} streams; {} in {:.2} s after, {:.1} ms",
        mb(tiles.sent.png),
        mb(tiles.sent.webp),
        mb(tiles.sent.video),

        tiles.sent.starts,
        if tiles.settled { "settled" } else { "NOT settled" },
        tiles.tail.as_secs_f64(),
        ms(tiles.tail_cpu),
    );
    row(&name, "tiles", &tiles, seconds, &ends, &tiles_detail);
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
    let ends = if off == 0 { "exact".to_owned() } else { format!("{off} px off, {}", db(mse)) };
    let settle = if settled { format!("{:.2} s", settled_in.as_secs_f64()) } else { "never".to_owned() };
    println!("| {name} | {settle} | {quiet} B | {redrawn} B | {ends} | {} |", if passed { "pass" } else { "**FAIL**" });
    Ok(passed)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1e6
}

fn row(name: &str, coder: &str, tally: &Tally, seconds: f64, ends: &str, detail: &str) {
    let mut per_frame = tally.per_frame.clone();
    per_frame.sort_unstable();
    let p95 = per_frame.get((per_frame.len() * 95 / 100).min(per_frame.len().saturating_sub(1))).copied().unwrap_or_default();
    let mean = if tally.frames == 0 { 0.0 } else { ms(per_frame.iter().sum()) / tally.frames as f64 };
    let bytes = tally.sent.bytes();
    let rate = if seconds > 0.0 { format!("{:.1}", bytes as f64 * 8.0 / seconds / 1e6) } else { "–".to_owned() };
    let held = if tally.frames == 0 { "–".to_owned() } else { db(tally.mse / tally.frames as f64) };
    let exact = if tally.frames == 0 { 0.0 } else { tally.exact as f64 * 100.0 / tally.frames as f64 };
    println!("| {name} | {coder} | {} | {seconds:.1} | {:.0} | {mean:.2} | {:.2} | {:.2} | {rate} | {held} | {exact:.0}% | {ends} | {detail} |", tally.frames, ms(tally.cpu), ms(p95), mb(bytes));
}
