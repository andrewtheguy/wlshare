//! What is made of a capture ([`wlcap`]): the file a daemon started with
//! `--capture-frames` wrote of what its video encoder was handed, said,
//! played out as raw pictures, or cut into a shorter capture.
//!
//! ```text
//! wlcap info CAPTURE
//! wlcap times CAPTURE
//! wlcap frames CAPTURE [--from FRAME] [--fps N]
//! wlcap trim CAPTURE --out CAPTURE (--from FRAME [--frames N]
//!       | --frames N [--busiest])
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
//! `frames` writes the run of a size that starts at frame `--from`, or
//! without it the capture's longest (a second pass), to standard output as
//! raw `B, G, R, X` pictures, rows tight, at a constant `--fps` (30 by
//! default, 1000 at most): at each tick the picture as the latest frame at or before it
//! left it, so the session's own timing survives, pauses and all. Its size,
//! `WxH`, and the rate go to standard error first, for the encoder it is
//! piped into: `ffmpeg -f rawvideo -pix_fmt bgr0 -s WxH -r N -i -`. Nothing
//! is coded, so what is watched is what the session was handed.
//!
//! `trim` writes part of the capture as a capture of its own, its times
//! counted from its first frame and that frame a whole picture with a
//! keyframe asked, as a session's first is: from FRAME on, or only the N
//! frames from there with `--frames`, for a capture whose start is no part
//! of its scenario, such as the seconds it was still before it began; or,
//! with `--frames` alone, N frames of the capture's longest run of a size
//! ([`window`]), past the whole pictures that open it where nothing follows
//! them for over a second: its first N, or with `--busiest` the N in a row
//! whose frames said the most pixels changed, a whole picture counting as
//! none, the earliest such where several tie, for an encoder measured on a
//! part of a capture. A capture that ends short of the N is an error.
//!
//! Each command prints the path of every capture it wrote, one a line.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use wlcap::{Reader, Writer};

// The one item of the daemon's RFB module the capture file names.
mod rfb {
    include!("../src/rfb/rect.rs");
}
#[allow(dead_code)]
#[path = "../src/wlcap.rs"]
mod wlcap;

type Error = Box<dyn std::error::Error>;

struct Args {
    capture: PathBuf,
    frames: usize,
    busiest: bool,
    from: u64,
    out: PathBuf,
    fps: u32,
    /// `--from` was given, 0 among what it may be.
    from_given: bool,
}

const USAGE: &str = "usage: wlcap info CAPTURE
       wlcap times CAPTURE
       wlcap frames CAPTURE [--from FRAME] [--fps N]
       wlcap trim CAPTURE --out CAPTURE (--from FRAME [--frames N] | --frames N [--busiest])";

fn parse_args() -> Result<(String, Args), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("a command")?;
    let capture = PathBuf::from(args.next().ok_or("a capture to read")?);
    let mut parsed = Args { capture, frames: 0, busiest: false, from: 0, out: PathBuf::new(), fps: 30, from_given: false };
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} takes a value"));
        match flag.as_str() {
            "--frames" => parsed.frames = value()?.parse().map_err(|_| "--frames takes a count")?,
            "--busiest" => parsed.busiest = true,
            "--from" => {
                parsed.from = value()?.parse().map_err(|_| "--from takes a frame")?;
                parsed.from_given = true;
            }
            "--out" => parsed.out = PathBuf::from(value()?),
            "--fps" => parsed.fps = value()?.parse().ok().filter(|fps| (1..=1000).contains(fps)).ok_or("--fps takes a rate from 1 to 1000")?,
            other => return Err(format!("{other}: not a flag")),
        }
    }
    match command.as_str() {
        "info" | "times" | "frames" => {
            if parsed.frames > 0 || parsed.busiest {
                return Err("--frames and --busiest are trim's".into());
            }
        }
        "trim" => {
            if parsed.out.as_os_str().is_empty() || (parsed.from == 0 && parsed.frames == 0) {
                return Err("trim takes --out, and --from, a frame after the first, or --frames".into());
            }
            if parsed.busiest && (parsed.from_given || parsed.frames == 0) {
                return Err("--busiest takes --frames and no --from".into());
            }
        }
        other => return Err(format!("{other}: not a command")),
    }
    if command != "frames" && parsed.fps != 30 {
        return Err("--fps is frames'".into());
    }
    if !matches!(command.as_str(), "trim" | "frames") && parsed.from_given {
        return Err("--from is trim's and frames'".into());
    }
    if command != "trim" && !parsed.out.as_os_str().is_empty() {
        return Err("--out is trim's".into());
    }
    Ok((command, parsed))
}

fn main() {
    let (command, args) = match parse_args() {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("wlcap: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let done = match command.as_str() {
        "info" => info(&args),
        "times" => times(&args),
        "frames" => frames(&args, std::io::stdout().lock()),
        _ => trim(&args),
    };
    if let Err(e) = done {
        eprintln!("wlcap: {}: {e}", args.capture.display());
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

/// The run `--from` starts, or the longest, as raw pictures on `out`.
fn frames(args: &Args, out: impl std::io::Write) -> Result<(), Error> {
    // The run's first frame: `--from`, or the longest run's (a pass of its own).
    let first = match args.from_given {
        true => args.from,
        false => {
            let seen = scan(&args.capture)?;
            runs(&seen).into_iter().max_by_key(|run| (run.len(), std::cmp::Reverse(run.start))).ok_or("the capture holds no frames")?.start as u64
        }
    };
    let tick = Duration::from_secs(1) / args.fps;
    let mut out = BufWriter::with_capacity(1 << 22, out);
    let mut source = Source::open(&args.capture)?;
    // The frame waiting for the next to say when it ends, and the tick due.
    let mut held: Option<(Duration, Vec<u8>)> = None;
    let mut next: Option<Duration> = None;
    let mut size = None;
    while let Some(frame) = source.reader.next()? {
        if frame.index < first {
            continue;
        }
        // The run is the frames from the first at its size, until another.
        if *size.get_or_insert_with(|| {
            eprintln!("{}x{} {} fps", frame.size.0, frame.size.1, args.fps);
            frame.size
        }) != frame.size
        {
            break;
        }
        if let Some((at, pixels)) = &held {
            let due = next.get_or_insert(*at);
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
    let (_, pixels) = held.ok_or_else(|| format!("the capture has no frame {first}"))?;
    // The last frame lasts a tick.
    out.write_all(&pixels)?;
    out.flush()?;
    Ok(())
}

// ── Where a cut begins ──────────────────────────────────────────────────────

/// Where the `frames` that `trim --frames` cuts begin, in the capture's longest run
/// of a size from where it moves ([`moving`]): its start, or with `busiest`
/// the start of the `frames` in a row whose frames said the most pixels
/// changed, the earliest where several tie. A whole picture says nothing
/// changed: it is a keyframe asked or the desktop shown, not motion, and
/// counted as every pixel it would draw the window to wherever one is.
fn window(seen: &[Seen], frames: usize, busiest: bool) -> Result<usize, Error> {
    let moved = |f: &Seen| if f.whole { 0 } else { f.changed };
    let run = runs(seen).into_iter().max_by_key(|run| (run.len(), std::cmp::Reverse(run.start))).ok_or("the capture holds no frames")?;
    let (width, height) = seen[run.start].size;
    let run = moving(seen, run);
    if run.len() < frames {
        return Err(format!("its longest run of a size, from where it moves, is {} frames of {width}x{height}, short of the {frames} asked", run.len()).into());
    }
    let mut start = run.start;
    if busiest {
        let mut window: u64 = seen[start..start + frames].iter().map(moved).sum();
        let mut most = window;
        for at in run.start + 1..=run.end - frames {
            window = window - moved(&seen[at - 1]) + moved(&seen[at + frames - 1]);
            if window > most {
                (most, start) = (window, at);
            }
        }
    }
    Ok(start)
}

/// A run from where it moves: past its opening whole pictures where the next
/// frame comes more than a second after them, a scenario's desktop shown
/// before anything on it is under way, which a window would otherwise count
/// as frames the encoder takes in no time.
fn moving(seen: &[Seen], run: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let at = run.start + seen[run.clone()].iter().take_while(|f| f.whole).count();
    match at > run.start && at < run.end && seen[at].micros - seen[at - 1].micros > 1_000_000 {
        true => at..run.end,
        false => run,
    }
}

// ── trim ─────────────────────────────────────────────────────────────────────

fn trim(args: &Args) -> Result<(), Error> {
    // The frames kept, `start..end`.
    let (start, end) = match (args.from_given, args.frames as u64) {
        (true, 0) => (args.from, u64::MAX),
        (true, frames) => (args.from, args.from + frames),
        (false, frames) => {
            let start = window(&scan(&args.capture)?, args.frames, args.busiest)? as u64;
            (start, start + frames)
        }
    };
    let part = args.out.with_file_name(format!(".wlcap-{}.part", std::process::id()));
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
        eprintln!("wlcap: trim: frames {start}..{}", start + kept - 1);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A capture of 1×1 frames at 100 ms apart, one per `(size, value)`:
    /// each picture's every byte is the value, so the output says which.
    fn capture(name: &str, frames: &[(u16, u8)]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("wlcap-test-{}-{name}.wlcap", std::process::id()));
        let mut writer = Writer::new(File::create(&path).unwrap()).unwrap();
        for (i, &(width, value)) in frames.iter().enumerate() {
            let pixels = vec![value; usize::from(width) * 4];
            writer.frame_at(Duration::from_millis(100 * i as u64), (width, 1), 50, i == 0, None, &pixels, pixels.len()).unwrap();
        }
        writer.flush().unwrap();
        path
    }

    /// `frames` at 10 fps, a tick a frame: the values of the pictures written.
    fn played(path: &Path, from: Option<u64>) -> Vec<u8> {
        let args = Args { capture: path.to_owned(), frames: 0, busiest: false, from: from.unwrap_or(0), out: PathBuf::new(), fps: 10, from_given: from.is_some() };
        let mut out = Vec::new();
        frames(&args, &mut out).unwrap();
        out.chunks(4).map(|pixel| pixel[0]).collect()
    }

    /// A run of 1×1 frames at `micros`, the first `whole` of them whole
    /// pictures and every one changing as much as the next.
    fn seen(micros: &[u64], whole: usize) -> Vec<Seen> {
        micros.iter().enumerate().map(|(i, &micros)| Seen { size: (1, 1), micros, quality: 50, keyframe: i < whole, whole: i < whole, changed: 1 }).collect()
    }

    #[test]
    fn a_window_starts_where_a_still_opening_ends() {
        // Two whole pictures, 1.5 s of nothing, then the motion: the first
        // frames of it, quiet or busiest, the earliest of the ties.
        let frames = seen(&[0, 30_000, 1_530_000, 1_560_000, 1_590_000, 1_620_000], 2);
        assert_eq!(window(&frames, 3, false).unwrap(), 2);
        assert_eq!(window(&frames, 3, true).unwrap(), 2);
        assert!(window(&frames, 5, false).is_err(), "the still opening is not counted towards the frames asked");
    }

    #[test]
    fn a_whole_picture_does_not_draw_the_busiest_window() {
        // Even motion with a keyframe asked at the end: the earliest window,
        // not the one that ends on the keyframe.
        let mut frames = seen(&[0, 50_000, 100_000, 150_000, 200_000, 250_000], 0);
        frames[5].whole = true;
        frames[5].changed = 100;
        assert_eq!(window(&frames, 3, true).unwrap(), 0);
        // Where the motion is, it still wins.
        frames[3].changed = 10;
        assert_eq!(window(&frames, 3, true).unwrap(), 1);
    }

    #[test]
    fn a_window_keeps_an_opening_the_motion_follows() {
        // The motion within the second: the run from its first frame.
        let frames = seen(&[0, 30_000, 80_000, 130_000], 2);
        assert_eq!(window(&frames, 3, false).unwrap(), 0);
        // A run with no whole picture to open it is as it is.
        assert_eq!(window(&seen(&[0, 1_500_000, 1_550_000], 0), 3, false).unwrap(), 0);
    }

    #[test]
    fn the_longest_run_is_played_where_a_size_comes_again() {
        // 1 wide twice, 2 wide once, 1 wide three times: the second run of
        // the first size, not the frames at that size from the first on.
        let path = capture("longest", &[(1, 1), (1, 2), (2, 3), (1, 4), (1, 5), (1, 6)]);
        assert_eq!(played(&path, None), [4, 5, 6]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn of_runs_as_long_the_first_is_played() {
        let path = capture("tie", &[(1, 1), (1, 2), (2, 3), (1, 4), (1, 5)]);
        assert_eq!(played(&path, None), [1, 2]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn from_plays_its_run_alone() {
        let path = capture("from", &[(1, 1), (1, 2), (2, 3), (2, 4), (1, 5), (1, 6), (1, 7)]);
        // Its run's start, or within it; and a run before the longest.
        assert_eq!(played(&path, Some(4)), [5, 6, 7]);
        assert_eq!(played(&path, Some(5)), [6, 7]);
        assert_eq!(played(&path, Some(0)), [1, 2]);
        assert_eq!(played(&path, Some(2)).len() / 2, 2);
        std::fs::remove_file(path).unwrap();
    }
}
