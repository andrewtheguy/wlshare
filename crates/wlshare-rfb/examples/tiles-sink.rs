//! A client that takes the daemon's tiles picture and keeps its region
//! streams: what a daemon on a host with a hardware H.264 encoder is checked
//! against without the remotex gateway, the way `vp9-sink` checks the VP9
//! encoding.
//!
//! ```text
//! tiles-sink ADDR --size WxH [--quality 1..100] [--vp9] [--seconds N]
//!            [--streams DIR] [--keys]
//! ```
//!
//! The handshake is RFB 3.8 with security None, so the daemon is one started
//! without a `[pam]` or `[password]` table. The list names the tiles encoding
//! at the quality, and beside it, unless `--vp9`, that H.264 regions are
//! taken; Fence and ContinuousUpdates so updates flow, ExtendedDesktopSize so
//! `--size` can be sent as a SetDesktopSize before the first picture, and
//! Cursor because the daemon requires it. Every rectangle is parsed by the
//! client's parser; a PNG is counted, a region's START is counted under the
//! codec it says, and with `--streams` each region's stream is written to
//! `DIR/stream-<n>-<codec>-<WxH>.<h264|ivf>` as it came — Annex B access
//! units one after another, or an IVF — for a decoder that is not this
//! repository's to read back and count. With `--keys` the sink types what
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
use wlshare_rfb::client::{self, RectBody, ServerMsg, TileBody};
use wlshare_rfb::msg::{FENCE_REQUEST, SECURITY_NONE};
use wlshare_rfb::tiles::RegionCodec;
use wlshare_rfb::vp9::ENCODING_VP9_QUALITY_BASE;
use wlshare_rfb::{ENCODING_CONTINUOUS_UPDATES, ENCODING_CURSOR, ENCODING_EXTENDED_DESKTOP_SIZE, ENCODING_FENCE, ENCODING_TILES, ENCODING_TILES_H264};

struct Args {
    addr: String,
    size: (u16, u16),
    quality: u8,
    vp9: bool,
    seconds: u64,
    streams: Option<String>,
    keys: bool,
}

/// One region stream as it came: its codec, rectangle, frames and bytes,
/// and the file it is written to.
struct Stream {
    n: usize,
    codec: RegionCodec,
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

/// An IVF's header for a VP9 stream of this size, the frame count left at
/// zero: ffmpeg reads to the end regardless.
fn ivf_header(width: u16, height: u16) -> [u8; 32] {
    let mut header = [0u8; 32];
    header[..4].copy_from_slice(b"DKIF");
    header[6..8].copy_from_slice(&32u16.to_le_bytes());
    header[8..12].copy_from_slice(b"VP90");
    header[12..14].copy_from_slice(&width.to_le_bytes());
    header[14..16].copy_from_slice(&height.to_le_bytes());
    header[16..20].copy_from_slice(&30u32.to_le_bytes());
    header[20..24].copy_from_slice(&1u32.to_le_bytes());
    header
}

fn size(s: &str) -> Result<(u16, u16), String> {
    let (w, h) = s.split_once('x').ok_or_else(|| format!("{s}: not WxH"))?;
    Ok((w.parse().map_err(|_| format!("{w}: not a width"))?, h.parse().map_err(|_| format!("{h}: not a height"))?))
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let addr = args.next().ok_or("usage: tiles-sink ADDR --size WxH [--quality Q] [--vp9] [--seconds N] [--streams DIR] [--keys]")?;
    let mut parsed = Args { addr, size: (0, 0), quality: 90, vp9: false, seconds: 10, streams: None, keys: false };
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} takes a value"));
        match flag.as_str() {
            "--size" => parsed.size = size(&value()?)?,
            "--quality" => parsed.quality = value()?.parse().map_err(|_| "--quality takes 1..100")?,
            "--vp9" => parsed.vp9 = true,
            "--seconds" => parsed.seconds = value()?.parse().map_err(|_| "--seconds takes a number")?,
            "--streams" => parsed.streams = Some(value()?),
            "--keys" => parsed.keys = true,
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
    out.write_all(wlshare_rfb::msg::PROTOCOL_VERSION).await?;
    let types = client::read_security_types(&mut reader).await?;
    if !types.contains(&SECURITY_NONE) {
        return Err(format!("the daemon offers security {types:?}, not None: start it without [pam] and [password]").into());
    }
    out.write_all(&[SECURITY_NONE]).await?;
    client::read_security_result(&mut reader).await?;
    out.write_all(&[1]).await?;
    let init = client::read_server_init(&mut reader).await?;
    eprintln!("tiles-sink: {} is {}x{}", init.name, init.width, init.height);

    let mut encodings = vec![ENCODING_TILES, ENCODING_VP9_QUALITY_BASE + i32::from(args.quality), ENCODING_FENCE, ENCODING_CONTINUOUS_UPDATES, ENCODING_EXTENDED_DESKTOP_SIZE, ENCODING_CURSOR];
    if !args.vp9 {
        encodings.insert(2, ENCODING_TILES_H264);
    }
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
    let (mut pngs, mut png_bytes, mut frames, mut frame_bytes, mut ends, mut updates, mut second) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut starts: HashMap<RegionCodec, u64> = HashMap::new();
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
                            RectBody::Tiles(TileBody::Start { stream, codec, frame }) => {
                                let codec = RegionCodec::of(codec).ok_or_else(|| format!("stream {stream} started in codec {codec}, which is none of the tiles'"))?;
                                if live.contains_key(&stream) {
                                    return Err(format!("stream {stream} started while live").into());
                                }
                                *starts.entry(codec).or_default() += 1;
                                n += 1;
                                let file = match &args.streams {
                                    Some(dir) => {
                                        let (name, codec_name) = match codec {
                                            RegionCodec::Vp9 => ("ivf", "vp9"),
                                            RegionCodec::H264 => ("h264", "h264"),
                                        };
                                        let mut file = File::create(format!("{dir}/stream-{n:03}-{codec_name}-{}x{}.{name}", rect.width, rect.height))?;
                                        if codec == RegionCodec::Vp9 {
                                            file.write_all(&ivf_header(rect.width, rect.height))?;
                                        }
                                        Some(file)
                                    }
                                    None => None,
                                };
                                let mut opened = Stream { n, codec, rect: place, frames: 0, bytes: 0, file };
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
            eprintln!("tiles-sink: {second}s {updates} updates, {pngs} PNGs {png_bytes} bytes, {} live streams, {frames} frames {frame_bytes} bytes, {ends} ends", live.len());
        }
    }
    done.extend(live.drain().map(|(_, stream)| stream));
    done.sort_by_key(|stream| stream.n);
    for stream in &mut done {
        if let Some(file) = &mut stream.file {
            file.flush()?;
        }
        let (x, y, w, h) = stream.rect;
        eprintln!("tiles-sink: stream {} {:?} {w}x{h}+{x}+{y}: {} frames {} bytes", stream.n, stream.codec, stream.frames, stream.bytes);
    }
    let codecs: Vec<String> = starts.iter().map(|(codec, count)| format!("{count} {codec:?}")).collect();
    eprintln!("tiles-sink: done, {updates} updates, {pngs} PNGs {png_bytes} bytes, streams started: {}, {frames} frames {frame_bytes} bytes, {ends} ends", if codecs.is_empty() { "none".to_owned() } else { codecs.join(", ") });
    Ok(())
}

/// Count one frame of a stream, and write it where the stream is kept.
fn write_frame(stream: &mut Stream, frame: &[u8]) -> std::io::Result<()> {
    stream.frames += 1;
    stream.bytes += frame.len() as u64;
    if let Some(file) = &mut stream.file {
        match stream.codec {
            RegionCodec::H264 => file.write_all(frame)?,
            RegionCodec::Vp9 => {
                let mut header = [0u8; 12];
                header[..4].copy_from_slice(&(frame.len() as u32).to_le_bytes());
                header[4..12].copy_from_slice(&(stream.frames - 1).to_le_bytes());
                file.write_all(&header)?;
                file.write_all(frame)?;
            }
        }
    }
    Ok(())
}
