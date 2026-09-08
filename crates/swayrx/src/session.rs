//! One client connection: the handshake, then the message loop.
//!
//! A session is one tokio task that reads client messages, watches the
//! framebuffer, and listens for events, and does one thing at a time so the
//! bytes it writes are always whole messages in the order they were decided.
//!
//! ## Sending pixels
//!
//! A client gets a FramebufferUpdate when it has asked for one — with a
//! `FramebufferUpdateRequest`, or once for all by enabling continuous updates —
//! and there is something to send: damage since the generation it last saw, or
//! everything after a non-incremental request or a resize. With Fence
//! negotiated, one update is in flight at a time: each ends with a fence the
//! client echoes, and the next waits for the echo, so a slow link is never
//! flooded and the compositor's frames coalesce in the framebuffer meanwhile.
//! Without Fence, TCP's own backpressure paces it.
//!
//! A size change goes out first, as its own update, and the whole framebuffer
//! follows in the next. A client that negotiated neither ExtendedDesktopSize nor
//! DesktopSize cannot be told and is disconnected at its next update instead of
//! being sent pixels at a size it does not know.
//!
//! ## The transport
//!
//! The handshake decides what the socket carries afterwards: RFB bytes as they
//! are, or — after RSA-AES — RFB bytes inside AES-EAX frames. The session sees
//! a [`Reader`] and a [`Writer`] either way; the writer takes whole messages,
//! which is what a frame is cut from.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::Context as _;
use log::{debug, info, warn};
use swayrx_rfb::density::{from_fixed, output_scale};
use swayrx_rfb::msg::{self, ClientMsg, Screen};
use swayrx_rfb::pixel::PixelFormat;
use swayrx_rfb::rsa_aes::{self, FrameReader, Sealer, ServerKey};
use swayrx_rfb::zrle::{ZrleEncoder, encode_raw_rect};
use swayrx_rfb::{auth, ENCODING_CONTINUOUS_UPDATES, ENCODING_DENSITY, ENCODING_DESKTOP_SIZE, ENCODING_EXTENDED_DESKTOP_SIZE, ENCODING_FENCE, ENCODING_RAW, ENCODING_ZRLE};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _, ReadBuf};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::broadcast;

use crate::framebuffer::{Rect, ResizeOrigin};
use crate::pam;
use crate::shared::{ClientId, Command, Event, Shared};

/// The one security type the server offers, and what it checks the client
/// against.
pub enum Security {
    /// Anyone who reaches the port is in.
    None,
    /// VncAuth with this password; the session is in the clear.
    VncAuth(String),
    /// RSA-AES at both widths, the credentials checked by PAM under `service`
    /// and the username required to be `account`, the process's own.
    RsaAes { key: Arc<ServerKey>, service: String, account: String },
}

pub struct SessionConfig {
    pub security: Security,
    pub name: String,
    pub resize: bool,
}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// The most rectangles one update carries before they collapse into one.
const MAX_RECTS: usize = 32;

/// The socket's read side, opened frame by frame after RSA-AES.
pub enum Reader {
    Plain(OwnedReadHalf),
    Sealed(FrameReader<OwnedReadHalf>),
}

impl AsyncRead for Reader {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(r) => Pin::new(r).poll_read(cx, buf),
            Self::Sealed(r) => Pin::new(r).poll_read(cx, buf),
        }
    }
}

/// The socket's write side: whole messages, each sealed into frames after
/// RSA-AES.
pub struct Writer {
    inner: OwnedWriteHalf,
    sealer: Option<Sealer>,
}

impl Writer {
    async fn send(&mut self, message: &[u8]) -> std::io::Result<()> {
        match &mut self.sealer {
            Some(sealer) => self.inner.write_all(&sealer.frame(message)).await,
            None => self.inner.write_all(message).await,
        }
    }
}

pub async fn run(id: ClientId, socket: TcpStream, shared: Arc<Shared>, config: Arc<SessionConfig>) -> anyhow::Result<()> {
    socket.set_nodelay(true)?;
    let peer = socket.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    let (reader, writer) = socket.into_split();
    let (reader, mut writer, shared_desktop) = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(reader, writer, &config, &peer))
        .await
        .map_err(|_| anyhow::anyhow!("the handshake took over {HANDSHAKE_TIMEOUT:?}"))??;
    let (width, height) = {
        let fb = shared.framebuffer.lock().unwrap();
        (fb.width, fb.height)
    };
    writer.send(&msg::server_init(width, height, &PixelFormat::NATIVE, &config.name)).await?;
    info!("client {}: authenticated; desktop {width}x{height}{}", id.0, if shared_desktop { "" } else { ", to itself" });
    shared.command(Command::ClientJoined { id, shared: shared_desktop });

    let events = shared.events.subscribe();
    let frames = shared.frame_tx.subscribe();
    let mut session = Session {
        id,
        shared,
        config,
        format: PixelFormat::NATIVE,
        zrle: None,
        use_zrle: false,
        continuous_supported: false,
        fence_supported: false,
        eds_supported: false,
        desktop_size_supported: false,
        density: false,
        continuous: false,
        pending: None,
        seen: 0,
        known_size: (width, height),
        fence_outstanding: false,
        fence_seq: 0,
        announce_eds: false,
        events,
        frames,
        scratch: Vec::new(),
        out: Vec::new(),
    };
    session.pump(reader, writer).await
}

/// RFB 3.8 version, security and ClientInit. Returns the transport the rest of
/// the session runs over and the ClientInit shared flag.
async fn handshake(mut reader: OwnedReadHalf, mut writer: OwnedWriteHalf, config: &SessionConfig, peer: &str) -> anyhow::Result<(Reader, Writer, bool)> {
    writer.write_all(msg::PROTOCOL_VERSION).await?;
    let mut version = [0u8; 12];
    reader.read_exact(&mut version).await.context("reading the client's version")?;
    if &version != msg::PROTOCOL_VERSION {
        let reason = "this server speaks RFB 3.8 only";
        writer.write_all(&msg::security_refusal(reason)).await?;
        anyhow::bail!("client version {:?}; {reason}", String::from_utf8_lossy(&version).trim_end());
    }
    let offered: &[u8] = match &config.security {
        Security::None => &[auth::SECURITY_NONE],
        Security::VncAuth(_) => &[auth::SECURITY_VNC_AUTH],
        Security::RsaAes { .. } => &[rsa_aes::SECURITY_RSA_AES_256, rsa_aes::SECURITY_RSA_AES_128],
    };
    writer.write_all(&msg::security_types(offered)).await?;
    let chosen = reader.read_u8().await.context("reading the security type")?;
    if !offered.contains(&chosen) {
        writer.write_all(&msg::security_failed("unsupported security type")).await?;
        anyhow::bail!("client chose security type {chosen}, not one of {offered:?}");
    }
    let (mut reader, mut writer) = match &config.security {
        Security::None => (Reader::Plain(reader), Writer { inner: writer, sealer: None }),
        Security::VncAuth(password) => {
            let challenge = auth::challenge();
            writer.write_all(&challenge).await?;
            let mut response = [0u8; 16];
            reader.read_exact(&mut response).await.context("reading the VncAuth response")?;
            if !auth::verify(password, &challenge, &response) {
                // Slow a guesser down before saying no.
                tokio::time::sleep(Duration::from_secs(1)).await;
                writer.write_all(&msg::security_failed("authentication failed")).await?;
                anyhow::bail!("authentication failed");
            }
            (Reader::Plain(reader), Writer { inner: writer, sealer: None })
        }
        Security::RsaAes { key, service, account } => {
            let strength = rsa_aes::Strength::of(chosen).expect("one of the two offered");
            let (credentials, session) = rsa_aes::authenticate(&mut reader, &mut writer, strength, key).await.context("RSA-AES key exchange")?;
            // Everything from here on is inside the frames, the refusal included.
            let mut writer = Writer { inner: writer, sealer: Some(session.sealer) };
            let reader = Reader::Sealed(FrameReader::new(reader, session.opener));
            let (service, account, peer) = (service.clone(), account.clone(), peer.to_owned());
            let checked = tokio::task::spawn_blocking(move || pam::check(&service, &account, &credentials.username, &credentials.password, &peer))
                .await
                .context("the PAM check did not finish")?;
            if let Err(refused) = checked {
                tokio::time::sleep(Duration::from_secs(1)).await;
                writer.send(&msg::security_failed("authentication failed")).await?;
                anyhow::bail!("login refused: {refused}");
            }
            (reader, writer)
        }
    };
    writer.send(&msg::security_ok()).await?;
    let shared = reader.read_u8().await.context("reading ClientInit")?;
    Ok((reader, writer, shared != 0))
}

struct Session {
    id: ClientId,
    shared: Arc<Shared>,
    config: Arc<SessionConfig>,
    format: PixelFormat,
    zrle: Option<ZrleEncoder>,
    use_zrle: bool,
    continuous_supported: bool,
    fence_supported: bool,
    eds_supported: bool,
    desktop_size_supported: bool,
    density: bool,
    /// The client enabled continuous updates.
    continuous: bool,
    /// An update request not yet answered: `true` for incremental.
    pending: Option<bool>,
    /// The framebuffer generation the client has.
    seen: u64,
    known_size: (u16, u16),
    fence_outstanding: bool,
    fence_seq: u32,
    announce_eds: bool,
    events: broadcast::Receiver<Event>,
    frames: tokio::sync::watch::Receiver<u64>,
    /// Pixels copied out of the framebuffer, rect by rect, for encoding.
    scratch: Vec<u8>,
    out: Vec<u8>,
}

/// A damaged rectangle and its pixels, taken out of the framebuffer.
struct Piece {
    rect: Rect,
    offset: usize,
}

impl Session {
    async fn pump(&mut self, mut reader: Reader, mut writer: Writer) -> anyhow::Result<()> {
        let mut inbuf: Vec<u8> = Vec::with_capacity(4096);
        loop {
            self.maybe_update(&mut writer).await?;
            tokio::select! {
                read = reader.read_buf(&mut inbuf) => {
                    let n = read.context("reading from the client")?;
                    if n == 0 {
                        return Ok(());
                    }
                    let mut consumed = 0;
                    while let Some((message, used)) = msg::parse(&inbuf[consumed..])? {
                        consumed += used;
                        self.handle(message, &mut writer).await?;
                    }
                    inbuf.drain(..consumed);
                }
                changed = self.frames.changed() => {
                    if changed.is_err() {
                        anyhow::bail!("the framebuffer is gone");
                    }
                }
                event = self.events.recv() => match event {
                    Ok(event) => self.handle_event(event, &mut writer).await?,
                    Err(broadcast::error::RecvError::Lagged(n)) => warn!("client {}: missed {n} events", self.id.0),
                    Err(broadcast::error::RecvError::Closed) => anyhow::bail!("the compositor thread is gone"),
                },
            }
        }
    }

    async fn handle(&mut self, message: ClientMsg, writer: &mut Writer) -> anyhow::Result<()> {
        match message {
            ClientMsg::SetPixelFormat(format) => {
                format.check().map_err(|e| anyhow::anyhow!("the client's pixel format cannot be produced: {e}"))?;
                debug!("client {}: pixel format {format:?}", self.id.0);
                self.format = format;
            }
            ClientMsg::SetEncodings(encodings) => {
                let has = |e: i32| encodings.contains(&e);
                self.use_zrle = has(ENCODING_ZRLE);
                if self.use_zrle && self.zrle.is_none() {
                    self.zrle = Some(ZrleEncoder::default());
                }
                if !self.use_zrle && !has(ENCODING_RAW) {
                    warn!("client {}: lists neither ZRLE nor Raw; sending Raw", self.id.0);
                }
                let announced_continuous = !self.continuous_supported && has(ENCODING_CONTINUOUS_UPDATES);
                self.continuous_supported |= has(ENCODING_CONTINUOUS_UPDATES);
                self.fence_supported = has(ENCODING_FENCE);
                if !self.eds_supported && has(ENCODING_EXTENDED_DESKTOP_SIZE) {
                    self.announce_eds = true;
                }
                self.eds_supported = has(ENCODING_EXTENDED_DESKTOP_SIZE);
                self.desktop_size_supported = has(ENCODING_DESKTOP_SIZE);
                let density = has(ENCODING_DENSITY);
                if density && !self.density {
                    info!("client {}: asked for density reports", self.id.0);
                }
                self.density = density;
                debug!(
                    "client {}: encodings {encodings:?}; zrle={} continuous={} fence={} eds={} density={}",
                    self.id.0, self.use_zrle, self.continuous_supported, self.fence_supported, self.eds_supported, self.density
                );
                if announced_continuous {
                    // The only way support is ever announced.
                    writer.send(&msg::end_of_continuous_updates()).await?;
                }
                if self.density {
                    // Every SetEncodings that lists the extension is answered.
                    self.send_geometry(writer).await?;
                }
            }
            ClientMsg::FramebufferUpdateRequest { incremental, .. } => {
                self.pending = Some(match self.pending {
                    Some(false) => false,
                    _ => incremental,
                });
            }
            ClientMsg::KeyEvent { down, keysym } => self.shared.command(Command::Key { client: self.id, keysym, down }),
            ClientMsg::PointerEvent { buttons, x, y } => self.shared.command(Command::Pointer { client: self.id, buttons, x, y }),
            ClientMsg::CutText(bytes) => self.shared.command(Command::SetClipboard(msg::latin1_to_string(&bytes))),
            ClientMsg::ExtendedCutText(_) => debug!("client {}: extended clipboard is not spoken here; ignored", self.id.0),
            ClientMsg::EnableContinuousUpdates { enable, .. } => {
                self.continuous = enable && self.continuous_supported;
                if !enable {
                    writer.send(&msg::end_of_continuous_updates()).await?;
                }
                debug!("client {}: continuous updates {}", self.id.0, if self.continuous { "on" } else { "off" });
            }
            ClientMsg::Fence { flags, payload } => {
                if flags & msg::FENCE_REQUEST != 0 {
                    // The client's own fence: nothing is reordered here, so every
                    // flag it asks for holds, and the echo is the whole answer.
                    let echo = flags & (msg::FENCE_BLOCK_BEFORE | msg::FENCE_BLOCK_AFTER | msg::FENCE_SYNC_NEXT);
                    writer.send(&msg::fence(echo, &payload)).await?;
                } else {
                    self.fence_outstanding = false;
                }
            }
            ClientMsg::SetDesktopSize { width, height, screens } => {
                if !self.eds_supported {
                    warn!("client {}: SetDesktopSize without ExtendedDesktopSize; ignored", self.id.0);
                    return Ok(());
                }
                let current = {
                    let fb = self.shared.framebuffer.lock().unwrap();
                    (fb.width, fb.height)
                };
                if !self.config.resize {
                    return self.send_eds(writer, msg::EDS_REASON_THIS_CLIENT, msg::EDS_STATUS_PROHIBITED, current).await;
                }
                if screens.len() != 1 || width == 0 || height == 0 {
                    return self.send_eds(writer, msg::EDS_REASON_THIS_CLIENT, msg::EDS_STATUS_INVALID_LAYOUT, current).await;
                }
                if (width, height) == current {
                    return self.send_eds(writer, msg::EDS_REASON_THIS_CLIENT, msg::EDS_STATUS_OK, current).await;
                }
                self.shared.command(Command::Resize { client: self.id, width, height });
            }
            ClientMsg::ClientDensity { fixed } => {
                if !self.density {
                    debug!("client {}: a density declaration without the extension; ignored", self.id.0);
                    return Ok(());
                }
                let scale = from_fixed(fixed);
                info!("client {}: declares a display density of {scale:.2}", self.id.0);
                if !(0.5..=8.0).contains(&scale) {
                    warn!("client {}: density {scale:.2} is out of range; reporting the scale as it is", self.id.0);
                    return self.send_geometry(writer).await;
                }
                self.shared.command(Command::Declare { client: self.id, scale });
            }
        }
        Ok(())
    }

    async fn handle_event(&mut self, event: Event, writer: &mut Writer) -> anyhow::Result<()> {
        match event {
            Event::Geometry { to } => {
                if self.density && to.is_none_or(|c| c == self.id) {
                    self.send_geometry(writer).await?;
                }
            }
            Event::ResizeRefused { client, status } => {
                if client == self.id && self.eds_supported {
                    let current = {
                        let fb = self.shared.framebuffer.lock().unwrap();
                        (fb.width, fb.height)
                    };
                    self.send_eds(writer, msg::EDS_REASON_THIS_CLIENT, status, current).await?;
                }
            }
            Event::Clipboard(text) => {
                writer.send(&msg::server_cut_text(&text)).await?;
            }
            Event::Exclusive { keep } => {
                if keep != self.id {
                    anyhow::bail!("client {} took the desktop to itself", keep.0);
                }
            }
        }
        Ok(())
    }

    async fn send_geometry(&mut self, writer: &mut Writer) -> anyhow::Result<()> {
        let g = self.shared.geometry();
        debug!("client {}: reporting {}x{} at scale {:.2}", self.id.0, g.width, g.height, g.scale);
        writer.send(&output_scale(g.width, g.height, g.scale)).await?;
        Ok(())
    }

    /// One ExtendedDesktopSize rectangle as its own update.
    async fn send_eds(&mut self, writer: &mut Writer, reason: u16, status: u16, size: (u16, u16)) -> anyhow::Result<()> {
        let mut update = msg::update_header(1).to_vec();
        update.extend_from_slice(&msg::extended_desktop_size_rect(reason, status, size.0, size.1, &[Screen::whole(size.0, size.1)]));
        writer.send(&update).await?;
        Ok(())
    }

    /// Send pixels if the client wants some and something has changed.
    async fn maybe_update(&mut self, writer: &mut Writer) -> anyhow::Result<()> {
        let wants = self.continuous || self.pending.is_some();
        if !wants || self.fence_outstanding {
            return Ok(());
        }
        // The ExtendedDesktopSize announcement is an update like any other, so
        // it waits for a request. Sent as its own update ahead of the pixels;
        // when there are none to send it is the request's whole answer.
        let announced = self.announce_eds;
        if announced {
            self.announce_eds = false;
            let size = self.known_size;
            self.send_eds(writer, msg::EDS_REASON_SERVER, msg::EDS_STATUS_OK, size).await?;
        }
        let answered = |this: &mut Self| {
            if announced {
                this.pending = None;
            }
        };

        // Under the lock: decide, and copy the pixels out. Encoding happens after.
        let mut pieces: Vec<Piece> = Vec::new();
        let mut resized = None;
        let generation;
        let size;
        {
            let fb = self.shared.framebuffer.lock().unwrap();
            if !fb.painted {
                drop(fb);
                answered(self);
                return Ok(());
            }
            size = (fb.width, fb.height);
            generation = fb.generation;
            if size != self.known_size {
                resized = Some(fb.resize_origin);
            }
            let full = resized.is_some() || self.pending == Some(false) || self.seen == 0;
            let rects = if full {
                vec![Rect::whole(fb.width, fb.height)]
            } else {
                match fb.damage_since(self.seen) {
                    Some(rects) if rects.is_empty() => {
                        drop(fb);
                        answered(self);
                        return Ok(());
                    }
                    Some(rects) => rects,
                    None => vec![Rect::whole(fb.width, fb.height)],
                }
            };
            let rects = if rects.len() > MAX_RECTS { crate::framebuffer::merge(rects, 1) } else { rects };
            self.scratch.clear();
            let stride = fb.stride();
            for rect in rects {
                let offset = self.scratch.len();
                let row_len = usize::from(rect.width) * 4;
                for row in usize::from(rect.y)..usize::from(rect.y) + usize::from(rect.height) {
                    let start = row * stride + usize::from(rect.x) * 4;
                    self.scratch.extend_from_slice(&fb.pixels[start..start + row_len]);
                }
                pieces.push(Piece { rect, offset });
            }
        }

        if let Some(origin) = resized {
            let (reason, status) = match origin {
                ResizeOrigin::Client(c) if c == self.id => (msg::EDS_REASON_THIS_CLIENT, msg::EDS_STATUS_OK),
                ResizeOrigin::Client(_) => (msg::EDS_REASON_OTHER_CLIENT, msg::EDS_STATUS_OK),
                ResizeOrigin::Server => (msg::EDS_REASON_SERVER, msg::EDS_STATUS_OK),
            };
            self.known_size = size;
            if self.eds_supported {
                self.send_eds(writer, reason, status, size).await?;
            } else if self.desktop_size_supported {
                let mut update = msg::update_header(1).to_vec();
                update.extend_from_slice(&msg::desktop_size_rect(size.0, size.1));
                writer.send(&update).await?;
            } else {
                anyhow::bail!(
                    "the desktop is now {}x{} and the client negotiated neither ExtendedDesktopSize nor DesktopSize to be told",
                    size.0,
                    size.1
                );
            }
        }

        self.out.clear();
        self.out.extend_from_slice(&msg::update_header(pieces.len() as u16));
        let encoding = if self.use_zrle { ENCODING_ZRLE } else { ENCODING_RAW };
        for piece in &pieces {
            let r = piece.rect;
            self.out.extend_from_slice(&msg::rect_header(r.x, r.y, r.width, r.height, encoding));
            let stride = usize::from(r.width) * 4;
            let pixels = &self.scratch[piece.offset..piece.offset + stride * usize::from(r.height)];
            match &mut self.zrle {
                Some(zrle) if self.use_zrle => {
                    zrle.encode_rect(pixels, stride, usize::from(r.width), usize::from(r.height), &self.format, &mut self.out)
                }
                _ => encode_raw_rect(pixels, stride, usize::from(r.width), usize::from(r.height), &self.format, &mut self.out),
            }
        }
        if self.fence_supported {
            self.fence_seq = self.fence_seq.wrapping_add(1);
            self.out.extend_from_slice(&msg::fence(msg::FENCE_REQUEST, &self.fence_seq.to_be_bytes()));
            self.fence_outstanding = true;
        }
        writer.send(&self.out).await.context("writing an update")?;
        self.seen = generation;
        self.pending = None;
        Ok(())
    }
}
