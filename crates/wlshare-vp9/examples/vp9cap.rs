//! What is made of a capture ([`wlshare_vp9::capture`]): the file a daemon
//! started with `--capture-frames` wrote of what its VP9 encoder was handed,
//! played into the encoder again, as the workspace has it now, and written as the
//! streams a decoder is tested and timed on. At 4:4:4, always: the daemon
//! captures a 4:4:4 stream alone, and the streams are for the 4:4:4 work a
//! capture is made for.
//!
//! ```text
//! vp9cap info CAPTURE
//! vp9cap times CAPTURE
//! vp9cap frames CAPTURE [--size WxH] [--fps N]
//! vp9cap shape STREAM.ivf
//! vp9cap streams CAPTURE --dir DIR --name NAME [--least N] [--threads N]
//! vp9cap sample CAPTURE --dir DIR --name NAME --frames N [--busiest]
//!        [--threads N]
//! vp9cap trim CAPTURE --out CAPTURE (--from FRAME [--frames N]
//!        | --frames N [--busiest])
//! ```
//!
//! A capture ending in `.zst` is read through `zstd -dc`, and written
//! through `zstd`, which must then be on the path.
//!
//! `info` says what a capture holds: for each run of frames at one size, how
//! many there are, how many were whole pictures, how much of the picture the
//! rest said changed, and how long the run lasted.
//!
//! `times` lists every frame, `frame,ms,width,height,whole,keyframe,changed`:
//! when the session handed it over, counted from the capture's first, and the
//! pixels it said changed, so that a part of a scripted session found by its
//! time can be cut with `trim --from FRAME --frames N`.
//!
//! `frames` writes the capture's first run of `--size`, or without it its
//! longest run of a size (a second pass), to standard output as
//! raw `B, G, R, X` pictures, rows tight, at a constant `--fps` (30 by
//! default): at each tick the picture as the latest frame at or before it
//! left it, so the session's own timing survives, pauses and all. Its size,
//! `WxH`, and the rate go to standard error first, for the encoder it is
//! piped into: `ffmpeg -f rawvideo -pix_fmt bgr0 -s WxH -r N -i -`. Nothing
//! is coded, so what is watched is what the session was handed.
//!
//! `shape` says what `sample` names a stream for, of any IVF: its size, its
//! tile columns and whether it is loop filtered, as its first frame has them.
//!
//! `streams` codes every frame as the session did — its rectangles, its dial,
//! its keyframe where one was asked — into one stream per run of a size,
//! `DIR/NAME-WxH.ivf`, with `-2`, `-3` for a size the capture comes
//! back to. A run of fewer than `--least` frames is left
//! out: the second or two a session spent at another size before it was
//! resized into its own is in a capture for an encoder's sake, and is no
//! stream to test a decoder on. Beside each is `.ivf.csv`, `frame,ms,bytes,keyframe,quality`: the
//! time is when the session's encoder was handed the frame, counted from the
//! stream's first, and it is the IVF timestamp too.
//!
//! `sample` cuts one stream of exactly N frames from the capture's longest
//! run of a size ([`window`]): its first N, or with `--busiest` the N in a
//! row whose frames said the most pixels changed, the earliest such where
//! several tie. The cut starts with a keyframe of the picture as it stood, and codes the
//! frames after it as the session did. It is named for what a decoder's
//! threads make of it, read from the keyframe's own header and not from the
//! encoder's rule: `DIR/NAME-WxH-<tile columns>col-<lf|nolf>.ivf`. A run
//! shorter than N is an error.
//!
//! `trim` writes part of the capture as a capture of its own, its times
//! counted from its first frame and that frame a whole picture with a
//! keyframe asked, as a session's first is: from FRAME on, or only the N
//! frames from there with `--frames`, for a capture whose start is no part
//! of its scenario, such as the seconds it was still before it began; or,
//! with `--frames` alone, the N frames `sample` would cut of it, for an
//! encoder measured on a part of a capture as a decoder is on a sample. A
//! capture that ends short of the N is an error.
//!
//! Each command prints the path of every stream or capture it wrote, one a
//! line.
//!
//! The encoder is libvpx's, as `wlshare-vp9` sets it up, as a session's is.
//!
//! `--threads` is the encoder's, 4 by default: the tile columns a width is
//! coded in are capped by them, so a sample is the same stream on any
//! machine only at the same count.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use wlshare_vp9::capture::{Frame, Reader, Writer};
use wlshare_vp9::{Chroma, Stream};

type Error = Box<dyn std::error::Error>;

struct Args {
    capture: PathBuf,
    dir: PathBuf,
    name: String,
    frames: usize,
    least: u32,
    busiest: bool,
    threads: usize,
    from: u64,
    out: PathBuf,
    fps: u32,
    size: Option<(u16, u16)>,
}

const USAGE: &str = "usage: vp9cap info CAPTURE
       vp9cap times CAPTURE
       vp9cap frames CAPTURE [--size WxH] [--fps N]
       vp9cap shape STREAM.ivf
       vp9cap streams CAPTURE --dir DIR --name NAME [--least N] [--threads N]
       vp9cap sample CAPTURE --dir DIR --name NAME --frames N [--busiest] [--threads N]
       vp9cap trim CAPTURE --out CAPTURE (--from FRAME [--frames N] | --frames N [--busiest])";

fn parse_args() -> Result<(String, Args), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("a command")?;
    let capture = PathBuf::from(args.next().ok_or("a capture to read")?);
    let mut parsed = Args { capture, dir: PathBuf::new(), name: String::new(), frames: 0, least: 1, busiest: false, threads: 4, from: 0, out: PathBuf::new(), fps: 30, size: None };
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} takes a value"));
        match flag.as_str() {
            "--dir" => parsed.dir = PathBuf::from(value()?),
            "--name" => parsed.name = value()?,
            "--frames" => parsed.frames = value()?.parse().map_err(|_| "--frames takes a count")?,
            "--least" => parsed.least = value()?.parse().map_err(|_| "--least takes a count")?,
            "--busiest" => parsed.busiest = true,
            "--threads" => parsed.threads = value()?.parse().map_err(|_| "--threads takes a count")?,
            "--from" => parsed.from = value()?.parse().map_err(|_| "--from takes a frame")?,
            "--out" => parsed.out = PathBuf::from(value()?),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or("--size takes WxH")?;
                parsed.size = Some((w.parse().map_err(|_| "--size takes WxH")?, h.parse().map_err(|_| "--size takes WxH")?));
            }
            "--fps" => parsed.fps = value()?.parse().ok().filter(|&fps| fps > 0).ok_or("--fps takes a rate above 0")?,
            other => return Err(format!("{other}: not a flag")),
        }
    }
    match command.as_str() {
        "info" | "times" | "frames" | "shape" => {}
        "streams" | "sample" => {
            if parsed.dir.as_os_str().is_empty() || parsed.name.is_empty() {
                return Err("--dir and --name are needed".into());
            }
            if parsed.threads == 0 {
                return Err("--threads takes at least 1".into());
            }
            if (command == "sample") != (parsed.frames > 0) || (parsed.busiest && command != "sample") {
                return Err("--frames and --busiest are sample's and trim's, and --frames is needed by sample".into());
            }
        }
        "trim" => {
            if parsed.out.as_os_str().is_empty() || (parsed.from == 0 && parsed.frames == 0) {
                return Err("trim takes --out, and --from, a frame after the first, or --frames".into());
            }
            if parsed.busiest && (parsed.from > 0 || parsed.frames == 0) {
                return Err("--busiest takes --frames and no --from".into());
            }
        }
        other => return Err(format!("{other}: not a command")),
    }
    if command != "frames" && (parsed.fps != 30 || parsed.size.is_some()) {
        return Err("--fps and --size are frames'".into());
    }
    if command != "trim" && (parsed.from > 0 || !parsed.out.as_os_str().is_empty()) {
        return Err("--from and --out are trim's".into());
    }
    Ok((command, parsed))
}

fn main() {
    let (command, args) = match parse_args() {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("vp9cap: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let done = match command.as_str() {
        "info" => info(&args),
        "times" => times(&args),
        "frames" => frames(&args),
        "shape" => shape(&args),
        "streams" => streams(&args),
        "trim" => trim(&args),
        _ => sample(&args),
    };
    if let Err(e) = done {
        eprintln!("vp9cap: {}: {e}", args.capture.display());
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
            // Opened here so that a file that is not there is this program's to say.
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
    /// A capture left before its end: `zstd` has nobody to write to.
    fn drop(&mut self) {
        if let Some(child) = &mut self.zstd {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ── info ─────────────────────────────────────────────────────────────────────

/// What one frame of a capture is, without its pixels.
struct Seen {
    size: (u16, u16),
    micros: u64,
    quality: u8,
    keyframe: bool,
    whole: bool,
    changed: u64,
}

fn scan(path: &Path) -> Result<Vec<Seen>, Error> {
    let mut source = Source::open(path)?;
    let mut seen = Vec::new();
    while let Some(frame) = source.reader.next()? {
        seen.push(Seen {
            size: frame.size,
            micros: frame.at.as_micros() as u64,
            quality: frame.quality,
            keyframe: frame.keyframe,
            whole: frame.changed.is_none(),
            changed: frame.changed_pixels(),
        });
    }
    source.finish()?;
    Ok(seen)
}

/// The runs of frames at one size, as ranges of frame indices.
fn runs(seen: &[Seen]) -> Vec<std::ops::Range<usize>> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, frame) in seen.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if seen[run.start].size == frame.size => run.end = i + 1,
            _ => runs.push(i..i + 1),
        }
    }
    runs
}

fn info(args: &Args) -> Result<(), Error> {
    let seen = scan(&args.capture)?;
    println!("{} frames", seen.len());
    for run in runs(&seen) {
        let frames = &seen[run.clone()];
        let (width, height) = frames[0].size;
        let whole = frames.iter().filter(|f| f.whole).count();
        let keyframes = frames.iter().filter(|f| f.keyframe).count();
        let partial: Vec<&Seen> = frames.iter().filter(|f| !f.whole).collect();
        let share = if partial.is_empty() { 0.0 } else { partial.iter().map(|f| f.changed).sum::<u64>() as f64 * 100.0 / (partial.len() as f64 * f64::from(width) * f64::from(height)) };
        let seconds = (frames[frames.len() - 1].micros - frames[0].micros) as f64 / 1e6;
        let (low, high) = frames.iter().fold((u8::MAX, 0), |(low, high), f| (low.min(f.quality), high.max(f.quality)));
        println!(
            "{width}x{height}: frames {}..{}, {:.1} s, {whole} whole, {keyframes} keyframes asked, the other {} changed {share:.1}% of the picture each, quality {low}..{high}",
            run.start,
            run.end - 1,
            seconds,
            partial.len(),
        );
    }
    Ok(())
}

fn times(args: &Args) -> Result<(), Error> {
    let seen = scan(&args.capture)?;
    let start = seen.first().map_or(0, |f| f.micros);
    println!("frame,ms,width,height,whole,keyframe,changed");
    for (index, f) in seen.iter().enumerate() {
        let (width, height) = f.size;
        println!("{index},{:.1},{width},{height},{},{},{}", (f.micros - start) as f64 / 1e3, u8::from(f.whole), u8::from(f.keyframe), f.changed);
    }
    Ok(())
}

fn frames(args: &Args) -> Result<(), Error> {
    let size = match args.size {
        Some(size) => size,
        None => {
            let seen = scan(&args.capture)?;
            let run = runs(&seen).into_iter().max_by_key(|run| (run.len(), std::cmp::Reverse(run.start))).ok_or("the capture holds no frames")?;
            seen[run.start].size
        }
    };
    eprintln!("{}x{} {} fps", size.0, size.1, args.fps);
    let tick = Duration::from_secs(1) / args.fps;
    let mut out = BufWriter::with_capacity(1 << 22, std::io::stdout().lock());
    let mut source = Source::open(&args.capture)?;
    // The frame waiting for the next to say when it ends, and the tick due.
    let mut held: Option<(Duration, Vec<u8>)> = None;
    let mut next: Option<Duration> = None;
    let mut run_seen = false;
    while let Some(frame) = source.reader.next()? {
        if frame.size != size {
            // The first run of the size, and no other.
            if run_seen {
                break;
            }
            continue;
        }
        run_seen = true;
        if let Some((_, pixels)) = &held {
            let due = next.get_or_insert(held.as_ref().map_or(frame.at, |(at, _)| *at));
            while *due < frame.at {
                out.write_all(pixels)?;
                *due += tick;
            }
        }
        match &mut held {
            Some((at, pixels)) => {
                *at = frame.at;
                pixels.clear();
                pixels.extend_from_slice(frame.pixels);
            }
            None => held = Some((frame.at, frame.pixels.to_vec())),
        }
    }
    let (_, pixels) = held.ok_or_else(|| format!("the capture holds no frame of {}x{}", size.0, size.1))?;
    // The last frame lasts a tick.
    out.write_all(&pixels)?;
    out.flush()?;
    Ok(())
}

// ── The streams out ──────────────────────────────────────────────────────────

/// Where an IVF header keeps its frame count.
const FRAME_COUNT_AT: u64 = 24;

/// An IVF header for a VP9 stream of a `width`×`height` picture timed in
/// milliseconds, its frame count to be written when the frames are.
fn ivf_header(width: u16, height: u16) -> [u8; 32] {
    let mut header = [0; 32];
    header[..4].copy_from_slice(b"DKIF");
    header[6..8].copy_from_slice(&32u16.to_le_bytes());
    header[8..12].copy_from_slice(b"VP90");
    header[12..14].copy_from_slice(&width.to_le_bytes());
    header[14..16].copy_from_slice(&height.to_le_bytes());
    header[16..20].copy_from_slice(&1000u32.to_le_bytes());
    header[20..24].copy_from_slice(&1u32.to_le_bytes());
    header
}

/// One stream being coded from a capture's frames and written: the encoder,
/// the IVF and its CSV. Written under a name of its own making and given the
/// name it is to keep when it is whole, so a stream cut short is never taken
/// for one.
struct Output {
    stream: Stream,
    size: (u16, u16),
    ivf: BufWriter<File>,
    csv: BufWriter<File>,
    part: PathBuf,
    began: Option<u64>,
    frames: u32,
    /// The stream's first frame, which says how the rest are coded.
    first: Vec<u8>,
    unit: Vec<u8>,
}

impl Output {
    fn new(dir: &Path, size: (u16, u16), quality: u8, threads: usize) -> Result<Self, Error> {
        let part = dir.join(format!(".vp9cap-{}.part", std::process::id()));
        let mut ivf = BufWriter::new(File::create(&part)?);
        ivf.write_all(&ivf_header(size.0, size.1))?;
        let mut csv = BufWriter::new(File::create(part.with_extension("csv"))?);
        csv.write_all(b"frame,ms,bytes,keyframe,quality\n")?;
        Ok(Self { stream: Stream::new(size.0, size.1, quality, Chroma::Yuv444, threads)?, size, ivf, csv, part, began: None, frames: 0, first: Vec::new(), unit: Vec::new() })
    }

    /// Code `frame` as the session did, or as a keyframe of the whole picture
    /// where the stream starts with it.
    fn frame(&mut self, frame: &Frame<'_>) -> Result<(), Error> {
        let starting = self.began.is_none();
        if self.stream.quality() != frame.quality {
            self.stream.set_quality(frame.quality);
        }
        self.unit.clear();
        let changed = if starting { None } else { frame.changed };
        self.stream.read_bgrx(frame.pixels, usize::from(self.size.0) * 4, changed)?;
        // libvpx making none of a capture's frame is an error here, since
        // every frame after it would be one early.
        self.stream.encode(starting || frame.keyframe, &mut self.unit)?;
        let keyframe = wlshare_vp9::frame_header(&self.unit).is_some_and(|header| header.keyframe);
        let micros = frame.at.as_micros() as u64;
        let ms = (micros - *self.began.get_or_insert(micros)) / 1000;
        if starting {
            self.first.clone_from(&self.unit);
        }
        self.ivf.write_all(&(self.unit.len() as u32).to_le_bytes())?;
        self.ivf.write_all(&ms.to_le_bytes())?;
        self.ivf.write_all(&self.unit)?;
        writeln!(self.csv, "{},{ms},{},{},{}", self.frames, self.unit.len(), u8::from(keyframe), frame.quality)?;
        self.frames += 1;
        Ok(())
    }

    /// Write the frame count, and give the files the name `path` and its
    /// `.csv`, printed.
    fn finish(mut self, path: &Path) -> Result<(), Error> {
        self.ivf.seek(SeekFrom::Start(FRAME_COUNT_AT))?;
        self.ivf.write_all(&self.frames.to_le_bytes())?;
        self.ivf.flush()?;
        self.csv.flush()?;
        std::fs::rename(&self.part, path)?;
        let mut csv = path.as_os_str().to_owned();
        csv.push(".csv");
        std::fs::rename(self.part.with_extension("csv"), csv)?;
        println!("{}", path.display());
        Ok(())
    }

    /// Remove what a stream that is not to be kept has written.
    fn discard(self) {
        let _ = std::fs::remove_file(&self.part);
        let _ = std::fs::remove_file(self.part.with_extension("csv"));
    }
}

fn streams(args: &Args) -> Result<(), Error> {
    std::fs::create_dir_all(&args.dir)?;
    let mut source = Source::open(&args.capture)?;
    let mut output: Option<Output> = None;
    let mut written: Vec<(u16, u16)> = Vec::new();
    let path = |written: &mut Vec<(u16, u16)>, size: (u16, u16)| {
        written.push(size);
        let nth = written.iter().filter(|&&s| s == size).count();
        let again = if nth > 1 { format!("-{nth}") } else { String::new() };
        args.dir.join(format!("{}-{}x{}{again}.ivf", args.name, size.0, size.1))
    };
    let keep = |done: Output, written: &mut Vec<(u16, u16)>| {
        if done.frames < args.least.max(1) {
            done.discard();
            return Ok(());
        }
        let size = done.size;
        done.finish(&path(written, size))
    };
    while let Some(frame) = source.reader.next()? {
        if let Some(done) = output.take_if(|o| o.size != frame.size) {
            keep(done, &mut written)?;
        }
        let output = match &mut output {
            Some(output) => output,
            None => output.insert(Output::new(&args.dir, frame.size, frame.quality, args.threads)?),
        };
        output.frame(&frame)?;
    }
    source.finish()?;
    keep(output.ok_or("the capture holds no frames")?, &mut written)
}

/// Where the `frames` that `sample` cuts begin, in the capture's longest run
/// of a size: its start, or with `busiest` the start of the `frames` in a row
/// whose frames said the most pixels changed, the earliest where several tie.
fn window(seen: &[Seen], frames: usize, busiest: bool) -> Result<usize, Error> {
    let run = runs(seen).into_iter().max_by_key(|run| (run.len(), std::cmp::Reverse(run.start))).ok_or("the capture holds no frames")?;
    let (width, height) = seen[run.start].size;
    if run.len() < frames {
        return Err(format!("its longest run of a size is {} frames of {width}x{height}, short of the {frames} asked", run.len()).into());
    }
    let mut start = run.start;
    if busiest {
        let mut window: u64 = seen[start..start + frames].iter().map(|f| f.changed).sum();
        let mut most = window;
        for at in run.start + 1..=run.end - frames {
            window = window - seen[at - 1].changed + seen[at + frames - 1].changed;
            if window > most {
                (most, start) = (window, at);
            }
        }
    }
    Ok(start)
}

fn sample(args: &Args) -> Result<(), Error> {
    let seen = scan(&args.capture)?;
    let start = window(&seen, args.frames, args.busiest)?;

    std::fs::create_dir_all(&args.dir)?;
    let mut source = Source::open(&args.capture)?;
    let mut output: Option<Output> = None;
    while let Some(frame) = source.reader.next()? {
        if (frame.index as usize) < start {
            continue;
        }
        if frame.index as usize >= start + args.frames {
            break;
        }
        let output = match &mut output {
            Some(output) => output,
            None => output.insert(Output::new(&args.dir, frame.size, frame.quality, args.threads)?),
        };
        output.frame(&frame)?;
    }
    drop(source);
    let output = output.expect("the run is in the capture");
    let Some(shape) = keyframe_shape(&output.first) else {
        output.discard();
        return Err("the stream's first frame is not a keyframe this reads".into());
    };
    eprintln!("vp9cap: {}: frames {start}..{} of {}", args.name, start + args.frames - 1, seen.len());
    let path = args.dir.join(format!("{}-{}.ivf", args.name, shape.name()));
    output.finish(&path)
}

// ── trim ─────────────────────────────────────────────────────────────────────

fn trim(args: &Args) -> Result<(), Error> {
    // The frames kept, `start..end`.
    let (start, end) = match (args.from, args.frames as u64) {
        (from, 0) => (from, u64::MAX),
        (0, frames) => {
            let start = window(&scan(&args.capture)?, args.frames, args.busiest)? as u64;
            (start, start + frames)
        }
        (from, frames) => (from, from + frames),
    };
    let part = args.out.with_file_name(format!(".vp9cap-{}.part", std::process::id()));
    let file = File::create(&part)?;
    let (output, mut zstd): (Box<dyn std::io::Write>, _) = if args.out.extension().is_some_and(|e| e == "zst") {
        let mut child = Command::new("zstd").arg("-qc").stdin(Stdio::piped()).stdout(file).spawn().map_err(|e| format!("running zstd: {e}"))?;
        (Box::new(child.stdin.take().expect("piped")), Some(child))
    } else {
        (Box::new(file), None)
    };
    let written = (|| {
        let mut writer = Writer::new(BufWriter::with_capacity(1 << 20, output))?;
        let mut source = Source::open(&args.capture)?;
        let (mut began, mut kept, mut cut) = (None, 0u64, false);
        while let Some(frame) = source.reader.next()? {
            if frame.index < start {
                continue;
            }
            if frame.index >= end {
                cut = true;
                break;
            }
            kept += 1;
            let first = began.is_none();
            let at = frame.at - *began.get_or_insert(frame.at);
            let (keyframe, changed) = if first { (true, None) } else { (frame.keyframe, frame.changed) };
            writer.frame_at(at, frame.size, frame.quality, keyframe, changed, frame.pixels, usize::from(frame.size.0) * 4)?;
        }
        // Only a capture read to its end has zstd's exit to answer for.
        if !cut {
            source.finish()?;
        }
        if began.is_none() {
            return Err(format!("the capture ends before frame {start}").into());
        }
        if end != u64::MAX && start + kept < end {
            return Err(format!("the capture ends after frame {}, short of the {} asked from {start}", start + kept - 1, end - start).into());
        }
        writer.flush()?;
        eprintln!("vp9cap: trim: frames {start}..{}", start + kept - 1);
        Ok::<_, Error>(())
    })();
    if let Some(child) = &mut zstd {
        // The writer, and with it zstd's input, is closed by now.
        if !child.wait()?.success() && written.is_ok() {
            let _ = std::fs::remove_file(&part);
            return Err("zstd could not compress it".into());
        }
    }
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &args.out)?;
    println!("{}", args.out.display());
    Ok(())
}

fn shape(args: &Args) -> Result<(), Error> {
    let mut ivf = Vec::new();
    File::open(&args.capture)?.take(1 << 16).read_to_end(&mut ivf)?;
    if ivf.len() < 44 || &ivf[..4] != b"DKIF" || &ivf[8..12] != b"VP90" {
        return Err("not a VP9 stream in IVF".into());
    }
    let shape = keyframe_shape(&ivf[44..]).ok_or("the stream's first frame is not a keyframe this reads")?;
    println!("{}", shape.name());
    Ok(())
}

// ── What a keyframe says of its stream ───────────────────────────────────────

/// What a decoder's threads have to go on, as a keyframe's header has it.
struct Shape {
    width: u32,
    height: u32,
    /// The loop filter's level, 0 for none.
    loop_filter_level: u8,
    tile_columns_log2: u32,
}

impl Shape {
    /// `WxH-<tile columns>col-<lf|nolf>`.
    fn name(&self) -> String {
        let filter = if self.loop_filter_level > 0 { "lf" } else { "nolf" };
        format!("{}x{}-{}col-{filter}", self.width, self.height, 1u32 << self.tile_columns_log2)
    }
}

/// A frame's bits, most significant first; `None` past its end.
struct Bits<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn take(&mut self, count: u32) -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            let byte = *self.bytes.get(self.at / 8)?;
            value = value << 1 | u32::from(byte >> (7 - self.at % 8) & 1);
            self.at += 1;
        }
        Some(value)
    }

    fn flag(&mut self) -> Option<bool> {
        Some(self.take(1)? == 1)
    }

    /// A field that is there only behind a set bit.
    fn optional(&mut self, count: u32) -> Option<()> {
        if self.flag()? {
            self.take(count)?;
        }
        Some(())
    }
}

/// Read a keyframe's uncompressed header (VP9 bitstream §6.2) as far as its
/// tile columns, for the loop filter's level and the columns: the two things
/// a stream's name says. `None` for a frame that is not a keyframe of an
/// 8-bit stream, or whose header runs out.
fn keyframe_shape(frame: &[u8]) -> Option<Shape> {
    let mut bits = Bits { bytes: frame, at: 0 };
    if bits.take(2)? != 2 {
        return None;
    }
    let low = bits.take(1)?;
    let profile = bits.take(1)? << 1 | low;
    // Profiles 2 and 3 are high bit depth, which nothing here codes.
    // show_existing_frame, then frame_type, where 0 is a keyframe.
    if profile > 1 || bits.flag()? || bits.flag()? {
        return None;
    }
    let _show_frame = bits.flag()?;
    let error_resilient = bits.flag()?;
    if bits.take(24)? != 0x49_83_42 {
        return None;
    }
    // color_config: the colour space, and unless it is sRGB the range, and
    // in profile 1 the subsampling and a reserved bit.
    let srgb = bits.take(3)? == 7;
    if !srgb {
        bits.take(1)?;
    }
    if profile == 1 {
        bits.take(if srgb { 1 } else { 3 })?;
    }
    let width = bits.take(16)? + 1;
    let height = bits.take(16)? + 1;
    // render_size, when it differs.
    bits.optional(32)?;
    if !error_resilient {
        // refresh_frame_context and frame_parallel_decoding_mode.
        bits.take(2)?;
    }
    let _frame_context = bits.take(2)?;

    // loop_filter_params.
    let loop_filter_level = bits.take(6)? as u8;
    let _sharpness = bits.take(3)?;
    if bits.flag()? && bits.flag()? {
        // The four reference deltas and the two mode deltas, each behind a bit.
        for _ in 0..6 {
            bits.optional(7)?;
        }
    }
    // quantization_params: the base index and three deltas behind a bit each.
    bits.take(8)?;
    for _ in 0..3 {
        bits.optional(5)?;
    }
    // segmentation_params.
    if bits.flag()? {
        if bits.flag()? {
            for _ in 0..7 {
                bits.optional(8)?;
            }
            if bits.flag()? {
                for _ in 0..3 {
                    bits.optional(8)?;
                }
            }
        }
        if bits.flag()? {
            bits.take(1)?;
            for _ in 0..8 {
                // Each feature's value and its sign: the quantizer, the loop
                // filter, the reference, and skip, which has no value.
                for value in [8, 6, 2, 0] {
                    if bits.flag()? && value > 0 {
                        bits.take(value + u32::from(value != 2))?;
                    }
                }
            }
        }
    }
    // tile_info: the columns' log2, counted up from the least the width allows
    // a bit at a time to the most.
    let superblocks = width.div_ceil(64);
    let mut least = 0;
    while (64 << least) < superblocks {
        least += 1;
    }
    let mut most = 1;
    while (superblocks >> most) >= 4 {
        most += 1;
    }
    most -= 1;
    let mut tile_columns_log2 = least;
    while tile_columns_log2 < most && bits.flag()? {
        tile_columns_log2 += 1;
    }
    Some(Shape { width, height, loop_filter_level, tile_columns_log2 })
}
