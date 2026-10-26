//! A client that takes the daemon's tiles picture and keeps its region
//! streams: what the tiles encoding is checked against without the zDesk
//! gateway, the way `vp9-sink` checks the VP9 encoding.
//!
//! ```text
//! tiles-sink ADDR --size WxH [--quality 1..100] [--seconds N]
//!            [--streams DIR] [--keys]
//! ```
//!
//! The handshake is RFB 3.8 with security None, so the daemon is one started
//! without a `[pam]` or `[password]` table. The list names the tiles encoding
//! at the quality — the regions are H.264, through the host's hardware or
//! libx264, the daemon's choice; `hardware_h264 = false` skips the hardware
//! probe and uses libx264 — and Fence and
//! ContinuousUpdates so updates flow, ExtendedDesktopSize so
//! `--size` can be sent as a SetDesktopSize before the first picture, and
//! Cursor because the daemon requires it. Every rectangle is parsed by the
//! client's parser; a PNG and a region's START are counted, and with
//! `--streams` each region's stream is written to
//! `DIR/stream-<n>-<WxH>.h264` as it came, Annex B access units one after
//! another, for a decoder that is not this repository's to read back and
//! count. With `--keys` the sink types what
//! `vp9-sink` does: every line of its standard input a keysym in
//! hexadecimal, pressed and let go, or `@X,Y,BUTTONS`, a pointer event, so
//! that something moves on the desktop. Every second a line says what came,
//! and at the end a summary and one line per stream.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write as _;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpStream;

use crate::rfb::client::{self, RectBody, ServerMsg, TileBody};
use crate::rfb::msg::{FENCE_REQUEST, SECURITY_NONE};
use crate::rfb::{ENCODING_CONTINUOUS_UPDATES, ENCODING_CURSOR, ENCODING_EXTENDED_DESKTOP_SIZE, ENCODING_FENCE};

// The daemon's protocol module, of which the sink uses the client's half.
#[allow(unused)]
#[path = "../src/rfb/mod.rs"]
mod rfb;

struct Args {
    addr: String,
    size: (u16, u16),
    quality: u8,
    seconds: u64,
    streams: Option<String>,
    keys: bool,
    /// The dial held, not walked.
    held: bool,
}

/// One region stream as it came: its rectangle, frames and bytes, and the
/// file it is written to.
struct Stream {
    n: usize,
    rect: (u16, u16, u16, u16),
    frames: u64,
    bytes: u64,
    file: Option<File>,
}

/// A line of `--keys` that is a pointer's: `@X,Y,BUTTONS`.
fn pointer(line: &str) -> Option<(u16, u16, u8)> {
    let mut fields = line.strip_prefix('@')?.split(',');
    let at = (fields.next()?.parse().ok()?, fields.next()?.parse().ok()?, fields.next()?.parse().ok()?);
    fields.next().is_none().then_some(at)
}

fn size(s: &str) -> Result<(u16, u16), String> {
    let (w, h) = s.split_once('x').ok_or_else(|| format!("{s}: not WxH"))?;
    Ok((w.parse().map_err(|_| format!("{w}: not a width"))?, h.parse().map_err(|_| format!("{h}: not a height"))?))
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let addr = args.next().ok_or("usage: tiles-sink ADDR --size WxH [--quality Q] [--held] [--seconds N] [--streams DIR] [--keys]")?;
    let mut parsed = Args { addr, size: (0, 0), quality: 90, seconds: 10, streams: None, keys: false, held: false };
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} takes a value"));
        match flag.as_str() {
            "--size" => parsed.size = size(&value()?)?,
            "--quality" => parsed.quality = value()?.parse().map_err(|_| "--quality takes 1..100")?,
            "--seconds" => parsed.seconds = value()?.parse().map_err(|_| "--seconds takes a number")?,
            "--streams" => parsed.streams = Some(value()?),
            "--keys" => parsed.keys = true,
            "--held" => parsed.held = true,
            other => return Err(format!("{other}: unknown flag")),
        }
    }
    if parsed.size == (0, 0) {
        return Err("--size WxH is required".into());
    }
    if !(1..=100).contains(&parsed.quality) {
        return Err("--quality takes 1..100".into());
    }
    Ok(parsed)
}

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("tiles-sink: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(args).await {
        eprintln!("tiles-sink: {e}");
        std::process::exit(1);
    }
    std::process::exit(0);
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let socket = TcpStream::connect(&args.addr).await?;
    socket.set_nodelay(true)?;
    let (mut reader, mut out) = socket.into_split();
    client::read_version(&mut reader).await?;
    out.write_all(crate::rfb::msg::PROTOCOL_VERSION).await?;
    let types = client::read_security_types(&mut reader).await?;
    if !types.contains(&SECURITY_NONE) {
        return Err(format!("the daemon offers security {types:?}, not None: start it without [pam] and [password]").into());
    }
    out.write_all(&[SECURITY_NONE]).await?;
    client::read_security_result(&mut reader).await?;
    out.write_all(&[1]).await?;
    let init = client::read_server_init(&mut reader).await?;
    eprintln!("tiles-sink: {} is {}x{}", init.name, init.width, init.height);

    let mut encodings = wlshare_proto::tiles::TilesPicture { quality: args.quality, adaptive: !args.held }.encodings();
    encodings.extend([ENCODING_FENCE, ENCODING_CONTINUOUS_UPDATES, ENCODING_EXTENDED_DESKTOP_SIZE, ENCODING_CURSOR]);
    out.write_all(&client::set_encodings(&encodings)).await?;
    let (mut width, mut height) = (init.width, init.height);
    if (width, height) != args.size {
        out.write_all(&client::set_desktop_size(args.size.0, args.size.1)).await?;
    }
    out.write_all(&client::enable_continuous_updates(true, 0, 0, width, height)).await?;
    out.write_all(&client::framebuffer_update_request(false, 0, 0, width, height)).await?;
    if let Some(dir) = &args.streams {
        std::fs::create_dir_all(dir)?;
    }

    let began = Instant::now();
    let end = began + Duration::from_secs(args.seconds);
    let mut keys = args.keys.then(|| BufReader::new(tokio::io::stdin()).lines());
    let mut buf = Vec::with_capacity(1 << 20);
    let mut chunk = vec![0u8; 1 << 16];
    let (mut pngs, mut png_bytes, mut webps, mut webp_bytes, mut starts, mut frames, mut frame_bytes, mut ends, mut updates, mut second) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut live: HashMap<u8, Stream> = HashMap::new();
    let mut done: Vec<Stream> = Vec::new();
    let mut n = 0;
    while Instant::now() < end {
        let typed = async {
            match &mut keys {
                Some(lines) => lines.next_line().await,
                None => std::future::pending().await,
            }
        };
        let read = tokio::select! {
            read = reader.read(&mut chunk) => read?,
            line = typed => {
                match line? {
                    Some(line) => {
                        if let Some((x, y, buttons)) = pointer(line.trim()) {
                            out.write_all(&client::pointer_event(buttons, x, y)).await?;
                            continue;
                        }
                        let keysym = u32::from_str_radix(line.trim(), 16).map_err(|_| format!("{line}: not a keysym in hexadecimal"))?;
                        out.write_all(&client::key_event(true, keysym)).await?;
                        out.write_all(&client::key_event(false, keysym)).await?;
                    }
                    None => keys = None,
                }
                continue;
            }
            () = tokio::time::sleep(Duration::from_millis(100)) => continue,
        };
        if read == 0 {
            return Err("the daemon closed the connection".into());
        }
        buf.extend_from_slice(&chunk[..read]);
        let mut at = 0;
        while let Some((msg, used)) = client::parse(&buf[at..])? {
            at += used;
            match msg {
                ServerMsg::Update(rects) => {
                    updates += 1;
                    for rect in rects {
                        let place = (rect.x, rect.y, rect.width, rect.height);
                        match rect.body {
                            RectBody::Tiles(TileBody::Png(png)) => {
                                pngs += 1;
                                png_bytes += png.len() as u64;
                            }
                            RectBody::Tiles(TileBody::Webp(webp)) => {
                                webps += 1;
                                webp_bytes += webp.len() as u64;
                            }
                            RectBody::Tiles(TileBody::Start { stream, frame }) => {
                                if live.contains_key(&stream) {
                                    return Err(format!("stream {stream} started while live").into());
                                }
                                starts += 1;
                                n += 1;
                                let file = match &args.streams {
                                    Some(dir) => Some(File::create(format!("{dir}/stream-{n:03}-{}x{}.h264", rect.width, rect.height))?),
                                    None => None,
                                };
                                let mut opened = Stream { n, rect: place, frames: 0, bytes: 0, file };
                                write_frame(&mut opened, &frame)?;
                                frames += 1;
                                frame_bytes += frame.len() as u64;
                                live.insert(stream, opened);
                            }
                            RectBody::Tiles(TileBody::Frame { stream, frame }) => {
                                let held = live.get_mut(&stream).ok_or_else(|| format!("a frame of stream {stream}, which is not live"))?;
                                if held.rect != place {
                                    return Err(format!("stream {stream} moved from {:?} to {place:?}", held.rect).into());
                                }
                                write_frame(held, &frame)?;
                                frames += 1;
                                frame_bytes += frame.len() as u64;
                            }
                            RectBody::Tiles(TileBody::End { stream }) => {
                                let held = live.remove(&stream).ok_or_else(|| format!("stream {stream} ended while not live"))?;
                                ends += 1;
                                done.push(held);
                            }
                            RectBody::ExtendedDesktopSize { .. } => {
                                (width, height) = (rect.width, rect.height);
                                eprintln!("tiles-sink: {:.1}s desktop {width}x{height}", began.elapsed().as_secs_f64());
                                // A new size ends every stream with no END.
                                done.extend(live.drain().map(|(_, stream)| stream));
                                out.write_all(&client::enable_continuous_updates(true, 0, 0, width, height)).await?;
                                out.write_all(&client::framebuffer_update_request(false, 0, 0, width, height)).await?;
                            }
                            _ => {}
                        }
                    }
                }
                ServerMsg::Fence { flags, payload } if flags & FENCE_REQUEST != 0 => {
                    out.write_all(&client::fence(flags & !FENCE_REQUEST, &payload)).await?;
                }
                _ => {}
            }
        }
        buf.drain(..at);
        let elapsed = began.elapsed().as_secs();
        if elapsed > second {
            second = elapsed;
            eprintln!("tiles-sink: {second}s {updates} updates, {pngs} PNGs {png_bytes} bytes, {webps} WebPs {webp_bytes} bytes, {} live streams, {frames} frames {frame_bytes} bytes, {ends} ends", live.len());
        }
    }
    done.extend(live.drain().map(|(_, stream)| stream));
    done.sort_by_key(|stream| stream.n);
    for stream in &mut done {
        if let Some(file) = &mut stream.file {
            file.flush()?;
        }
        let (x, y, w, h) = stream.rect;
        eprintln!("tiles-sink: stream {} {w}x{h}+{x}+{y}: {} frames {} bytes", stream.n, stream.frames, stream.bytes);
    }
    eprintln!("tiles-sink: done, {updates} updates, {pngs} PNGs {png_bytes} bytes, {webps} WebPs {webp_bytes} bytes, {starts} streams started, {frames} frames {frame_bytes} bytes, {ends} ends");

    Ok(())
}

/// Count one frame of a stream, and write it where the stream is kept.
fn write_frame(stream: &mut Stream, frame: &[u8]) -> std::io::Result<()> {
    stream.frames += 1;
    stream.bytes += frame.len() as u64;
    if let Some(file) = &mut stream.file {
        file.write_all(frame)?;
    }
    Ok(())
}
