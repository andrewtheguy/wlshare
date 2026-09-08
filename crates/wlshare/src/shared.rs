//! What the compositor thread and the client sessions share, and the two
//! channels between them.
//!
//! The compositor side is one thread that owns the Wayland connection
//! ([`crate::compositor`]); clients are tokio tasks ([`crate::session`]). They
//! meet in three places, all here: the [`Framebuffer`] behind a mutex, a
//! `watch` that ticks when it changes, and a broadcast of [`Event`]s for what a
//! frame cannot carry. Sessions talk back through [`Command`]s on a calloop
//! channel the compositor thread polls beside the Wayland socket.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tokio::sync::{broadcast, watch};

use crate::framebuffer::Framebuffer;

/// One connection, numbered from 1 for the life of the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(pub u64);

/// What the compositor thread is asked to do.
#[derive(Debug)]
pub enum Command {
    /// A client connected; capture runs while there is at least one. `shared`
    /// is the ClientInit flag: a client that clears it asks for the desktop to
    /// itself, and the others are disconnected.
    ClientJoined { id: ClientId, shared: bool },
    /// A client left: its input is let go, capture may stop, and the layout is
    /// released if it held it. Sent for every connection, joined or not.
    ClientLeft(ClientId),
    Key { client: ClientId, keysym: u32, down: bool },
    Pointer { client: ClientId, buttons: u8, x: u16, y: u16 },
    /// SetDesktopSize: the client wants the desktop `width`×`height` pixels.
    Resize { client: ClientId, width: u16, height: u16 },
    /// ClientDensity: the client wants the output drawn at `scale`.
    Declare { client: ClientId, scale: f64 },
    /// Text for the compositor's clipboard.
    SetClipboard(String),
}

/// What the compositor thread tells the sessions, beyond the framebuffer.
#[derive(Debug, Clone)]
pub enum Event {
    /// The output's scale or size changed, or a declaration was answered: send an
    /// OutputScale to every density client (`to` is `None`) or to one of them.
    Geometry { to: Option<ClientId> },
    /// A client's SetDesktopSize was refused with an ExtendedDesktopSize status.
    ResizeRefused { client: ClientId, status: u16 },
    /// Text arrived on the compositor's clipboard — or left it: empty when the
    /// selection was cleared or is no longer text.
    Clipboard(String),
    /// A client took the desktop to itself with ClientInit; every other client
    /// is disconnected.
    Exclusive { keep: ClientId },
}

/// The captured output as the sessions describe it to clients: the framebuffer's
/// size in pixels and the scale it is drawn at. Kept apart from the framebuffer
/// because a report can precede the frame that carries the size it names.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub width: u16,
    pub height: u16,
    pub scale: f64,
}

pub struct Shared {
    pub framebuffer: Mutex<Framebuffer>,
    /// Ticks with [`Framebuffer::generation`] on every change.
    pub frame_tx: watch::Sender<u64>,
    pub geometry: Mutex<Geometry>,
    pub events: broadcast::Sender<Event>,
    pub commands: calloop::channel::Sender<Command>,
    next_client: AtomicU64,
}

impl Shared {
    pub fn new(framebuffer: Framebuffer, geometry: Geometry, commands: calloop::channel::Sender<Command>) -> Self {
        let (frame_tx, _) = watch::channel(framebuffer.generation);
        let (events, _) = broadcast::channel(64);
        Self {
            framebuffer: Mutex::new(framebuffer),
            frame_tx,
            geometry: Mutex::new(geometry),
            events,
            commands,
            next_client: AtomicU64::new(1),
        }
    }

    pub fn next_client(&self) -> ClientId {
        ClientId(self.next_client.fetch_add(1, Ordering::Relaxed))
    }

    pub fn geometry(&self) -> Geometry {
        *self.geometry.lock().unwrap()
    }

    /// Tell the sessions the framebuffer changed.
    pub fn frame_changed(&self, generation: u64) {
        self.frame_tx.send_replace(generation);
    }

    pub fn emit(&self, event: Event) {
        // No receivers is not an error: nobody is connected.
        let _ = self.events.send(event);
    }

    pub fn command(&self, command: Command) {
        if self.commands.send(command).is_err() {
            log::error!("the compositor thread is gone");
        }
    }
}
