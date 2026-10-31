//! A client that takes the daemon's tiles picture and keeps its region
//! streams: what the tiles encoding is checked against without the zDesk
//! gateway, the way `video-sink` checks the video encoding.
//!
//! ```text
//! tiles-sink ADDR --size WxH [--held | --lossless]
//!            [--seconds N] [--streams DIR] [--keys]
//! ```
//!
//! The handshake is RFB 3.8 with security None, so the daemon is one started
//! without a `[pam]` or `[password]` table. The list names the tiles encoding
//! — the regions are H.264, through the host's hardware or
//! libx264, the daemon's choice; `hardware_h264 = false` skips the hardware
//! probe and uses libx264 — and Fence and
//! ContinuousUpdates so updates flow, ExtendedDesktopSize so
//! `--size` can be sent as a SetDesktopSize before the first picture,
//! Cursor because the daemon requires it, and the protocol version, whose
//! answer is read before anything else and must be this build's. Every rectangle is parsed by the
//! client's parser; a PNG, a progressive tile (all a list whose dial walks
//! is sent; `--held` holds it, for PNGs and streams), a planar bitmap (all a
//! lossless list is sent, `--lossless`), a copy and a region's START are
//! counted, a copy's source and a progressive tile's tile checked, a planar
//! bitmap read back whole at its rectangle's size, and with
//! `--streams` each region's stream is written to
//! `DIR/stream-<n>-<WxH>.h264` as it came, Annex B access units one after
//! another, for a decoder that is not this repository's to read back and
//! count. With `--keys` the sink types what
//! `video-sink` does: every line of its standard input a keysym in
//! hexadecimal, pressed and let go, or `@X,Y,BUTTONS`, a pointer event, so
//! that something moves on the desktop. Every second a line says what came,
//! and at the end a summary and one line per stream.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write as _;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpStream;
use wlshare_proto::tiles::TilesPicture;

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
    seconds: u64,
    streams: Option<String>,
    keys: bool,
    /// The picture asked for: progressive tiles unless `--held` (PNGs and
    /// streams) or `--lossless` (planar tiles).
    picture: TilesPicture,
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
    let addr = args.next().ok_or("usage: tiles-sink ADDR --size WxH [--held | --lossless] [--seconds N] [--streams DIR] [--keys]")?;
    let mut parsed = Args { addr, size: (0, 0), seconds: 10, streams: None, keys: false, picture: TilesPicture::Progressive };
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} takes a value"));
        match flag.as_str() {
            "--size" => parsed.size = size(&value()?)?,
            "--seconds" => parsed.seconds = value()?.parse().map_err(|_| "--seconds takes a number")?,
            "--streams" => parsed.streams = Some(value()?),
            "--keys" => parsed.keys = true,
            "--held" => parsed.picture = TilesPicture::Held,
            "--lossless" => parsed.picture = TilesPicture::Lossless,
            other => return Err(format!("{other}: unknown flag")),
        }
    }
    if parsed.size == (0, 0) {
        return Err("--size WxH is required".into());
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

    let mut encodings = args.picture.encodings();
    encodings.extend([ENCODING_FENCE, ENCODING_CONTINUOUS_UPDATES, ENCODING_EXTENDED_DESKTOP_SIZE, ENCODING_CURSOR]);
    encodings.push(wlshare_proto::version::encoding(wlshare_proto::PROTOCOL_VERSION));
    out.write_all(&client::set_encodings(&encodings)).await?;
    // The daemon answers the version first, before anything is asked of it.
    client::read_protocol_version(&mut reader).await?;
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
    let (mut progressives, mut progressive_bytes, mut planars, mut planar_bytes) = (0u64, 0u64, 0u64, 0u64);
    let (mut pngs, mut png_bytes, mut copies, mut cached, mut starts, mut frames, mut frame_bytes, mut ends, mut updates, mut second) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
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
                            RectBody::Tiles(TileBody::Copy { x, y }) => {
                                if u32::from(x) + u32::from(rect.width) > u32::from(width) || u32::from(y) + u32::from(rect.height) > u32::from(height) {
                                    return Err(format!("a copy from {x},{y} of {}x{}, past the {width}x{height} desktop", rect.width, rect.height).into());
                                }
                                copies += 1;
                            }
                            RectBody::Tiles(TileBody::Cached { .. }) => cached += 1,
                            RectBody::Tiles(TileBody::Progressive(block)) => {
                                let cell = wlshare_proto::progressive::tile_of(&block).ok_or("a progressive body that is no tile block")?;
                                if (u32::from(cell.0) * 64, u32::from(cell.1) * 64) != (u32::from(rect.x), u32::from(rect.y)) {
                                    return Err(format!("a progressive tile {cell:?} at {},{}", rect.x, rect.y).into());
                                }
                                progressives += 1;
                                progressive_bytes += block.len() as u64;
                            }
                            RectBody::Tiles(TileBody::Planar(bitmap)) => {
                                let (_, used) = crate::rfb::planar::decoder::decode(&bitmap, usize::from(rect.width), usize::from(rect.height))?;
                                if used != bitmap.len() {
                                    return Err(format!("a planar bitmap of {} bytes, {used} of them its {}x{} pixels", bitmap.len(), rect.width, rect.height).into());
                                }
                                planars += 1;
                                planar_bytes += bitmap.len() as u64;
                            }
                            RectBody::Tiles(TileBody::Store { .. }) => {}
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
            eprintln!("tiles-sink: {second}s {updates} updates, {pngs} PNGs {png_bytes} bytes, {progressives} progressive {progressive_bytes} bytes, {planars} planar {planar_bytes} bytes, {copies} copies, {cached} cached, {} live streams, {frames} frames {frame_bytes} bytes, {ends} ends", live.len());
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
    eprintln!("tiles-sink: done, {updates} updates, {pngs} PNGs {png_bytes} bytes, {progressives} progressive {progressive_bytes} bytes, {planars} planar {planar_bytes} bytes, {copies} copies, {cached} cached, {starts} streams started, {frames} frames {frame_bytes} bytes, {ends} ends");

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
