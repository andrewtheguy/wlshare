//! The wlshare client, everything but the window: the session under both the
//! macOS app (`wlshare-macos`) and the Windows one (`wlshare-windows`).
//!
//! An app owns a window, something to draw into and the events its toolkit
//! hands it. This crate owns everything between those and the socket: the RFB
//! session, the decoders, the framebuffer they write into, the pointer's shape,
//! the clipboard and the desktop's sound. What stays in each app's own core is
//! what is about its platform — the tables that say what one of its keys or
//! one of its scrolls is on the wire, and the C ABI its app calls in through —
//! so nothing here knows what AppKit or WinUI is, and all of it is tested on a
//! machine that has neither.
//!
//! [`Client::connect`] starts a thread with a current-thread tokio runtime on
//! it and returns at once; the session runs there until it ends or the client
//! is dropped. The window learns there is something new from the callback given
//! to [`Client::on_frame`] and reads the pixels through [`Client::with_frame`],
//! which holds the framebuffer's lock for exactly as long as the upload takes.
//!
//! The desktop's sound, when the window asked for it, is decoded into a
//! buffer the platform's audio device takes from through [`Client::read_audio`],
//! on its own clock.
//!
//! **Scope.** The screen — as wlshare's VP9 stream or as exact ZRLE,
//! whichever the window chose — the keyboard, the pointer, the scale the
//! desktop is drawn at, the clipboard, as text both ways, and the desktop's
//! sound. The camera, the microphone and picking an output are wlshare
//! extensions this client does not list, so the server never sends them.
//!
//! `wlshare_rfb` is re-exported, so an app's core that needs a type of the
//! wire's names the one this crate was built with rather than pinning its own.

pub mod audio;
pub mod framebuffer;
pub mod session;

use std::sync::Arc;
use std::thread::JoinHandle;

use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

pub use framebuffer::{Framebuffer, Region};
pub use session::{Command, Config, Encoding, Shared, State, Status, Surface};
pub use wlshare_rfb;

/// The three real buttons of the RFB button mask.
pub const BUTTON_LEFT: u8 = 1;
pub const BUTTON_MIDDLE: u8 = 2;
pub const BUTTON_RIGHT: u8 = 4;

/// The four wheel "buttons" above them, as the server reads them: RFB has no
/// scroll event, and a notch is a press and release of one of these. How many
/// notches a scroll comes to is each platform's own reckoning.
pub const WHEEL_UP: u8 = 8;
pub const WHEEL_DOWN: u8 = 16;
pub const WHEEL_LEFT: u8 = 32;
pub const WHEEL_RIGHT: u8 = 64;

/// A connection and the thread running it.
///
/// Dropping one ends the session and waits for the thread, so a window that has
/// let go of its client can be sure nothing will call back into it afterwards.
pub struct Client {
    shared: Arc<Shared>,
    commands: UnboundedSender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Client {
    /// Start a session. It returns before the socket is open: what happened is
    /// in [`Client::status`], and the callback from [`Client::on_frame`] fires
    /// when that changes.
    pub fn connect(config: Config, surface: Surface) -> Self {
        let shared = Arc::new(Shared::default());
        let (commands, receiver) = unbounded_channel();
        let thread = std::thread::Builder::new()
            .name("wlshare-session".to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                move || {
                    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                        Ok(runtime) => runtime,
                        Err(error) => return shared.fail(format!("starting the session's runtime: {error}")),
                    };
                    // The session records its own ending in `shared`, which is
                    // where the window reads it; there is nobody to return it to.
                    let _ = runtime.block_on(session::run(config, surface, Arc::clone(&shared), receiver));
                }
            })
            .expect("spawning the session thread");
        Self { shared, commands, thread: Some(thread) }
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }

    /// Call `wake` whenever there is something new to draw — a frame, a cursor,
    /// a size, a state. It runs on the session's thread and must not block, so
    /// what belongs in it is one "this window is dirty" and nothing else.
    ///
    /// It is called once as it is installed, on this thread: the session has
    /// been running since [`Client::connect`], and a state it reached before
    /// there was anyone to tell — a connection refused at once — would
    /// otherwise never be announced. A window that is told it is dirty when it
    /// is not draws once for nothing.
    ///
    /// It is cleared before the session thread is joined, so it can never run
    /// after [`Client`] is dropped.
    pub fn on_frame(&self, wake: Option<Box<dyn Fn() + Send + Sync>>) {
        self.shared.set_wake(wake);
        self.shared.wake();
    }

    /// Show `visit` the framebuffer and the region of it that has changed since
    /// the last visit, and clear that region. The lock is held throughout, so
    /// `visit` uploads and returns; it must not wait for anything.
    ///
    /// A visit that had something to take is also the window's acknowledgment:
    /// the session holds the fence that ends each update until it happens, and
    /// the server times that round trip to pick the quality it codes the next
    /// frame at (`session::owe_fence`). So a window draws by calling this —
    /// not by reading the pixels some other way — and calls it from the draw
    /// itself, where what it costs to upload a frame is part of what the
    /// server is told.
    pub fn with_frame<T>(&self, visit: impl FnOnce(&Framebuffer, Option<Region>) -> T) -> T {
        let mut fb = self.shared.framebuffer.lock().unwrap();
        let damage = fb.take_damage();
        let taken = visit(&fb, damage);
        // Under the lock: the session reads the damage under it too, so a
        // paint can never be announced before the damage it took is cleared.
        if damage.is_some() {
            self.shared.took_frame();
        }
        taken
    }

    /// The framebuffer's size, leaving its damage for [`Client::with_frame`].
    /// Anything that only wants the size must ask here: a status read through
    /// `with_frame` takes the damage the next draw needed, and that draw then
    /// uploads nothing.
    pub fn desktop_size(&self) -> (u16, u16) {
        let fb = self.shared.framebuffer.lock().unwrap();
        (fb.width(), fb.height())
    }

    /// Say that the whole framebuffer must be uploaded again, for a window that
    /// has lost the texture it was drawing from.
    pub fn damage_all(&self) {
        self.shared.framebuffer.lock().unwrap().damage_all();
    }

    /// Show `visit` the pointer shape and which shape it is. A generation the
    /// window has seen before is the same shape; `None` is a hidden pointer.
    pub fn with_cursor<T>(&self, visit: impl FnOnce(u64, Option<&wlshare_rfb::cursor::CursorImage>) -> T) -> T {
        let cursor = self.shared.cursor.lock().unwrap();
        visit(cursor.0, cursor.1.as_ref())
    }

    /// The pointer, at a position in the window's device pixels.
    pub fn pointer(&self, buttons: u8, x: u16, y: u16) {
        self.send(Command::Pointer { buttons, x, y });
    }

    pub fn key(&self, down: bool, keysym: u32) {
        self.send(Command::Key { down, keysym });
    }

    /// The window changed: its size in device pixels, or the scale the desktop
    /// is to be drawn at. The desktop is asked to follow once the change
    /// settles.
    pub fn surface(&self, surface: Surface) {
        self.send(Command::Surface(surface));
    }

    /// The window's clipboard, for the desktop. The window gives it when it has
    /// changed and the window is where the person is — the desktop is sent
    /// only what it was given here, and only when it asks.
    pub fn clipboard(&self, text: String) {
        self.send(Command::Clipboard(text));
    }

    /// Show `visit` the desktop's clipboard and which arrival it is. A
    /// generation the window has seen is text it has already taken; `None` is
    /// a desktop that has provided nothing yet.
    pub fn with_clipboard<T>(&self, visit: impl FnOnce(u64, Option<&str>) -> T) -> T {
        let clipboard = self.shared.clipboard.lock().unwrap();
        visit(clipboard.0, clipboard.1.as_deref())
    }

    /// The next of the desktop's sound, as 48 kHz stereo into `left` and
    /// `right`, which are the same length — silence where there is none yet.
    /// Called from the audio device's own thread; the lock is held for the
    /// copy and nothing else.
    pub fn read_audio(&self, left: &mut [f32], right: &mut [f32]) {
        self.shared.audio.lock().unwrap().read(left, right);
    }

    fn send(&self, command: Command) {
        // A closed channel is a session that has already ended, which the
        // window learns from the status rather than from an input event.
        let _ = self.commands.send(command);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shared.set_wake(None);
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn nowhere() -> Config {
        // Port 0 never connects, which is the point: the session must fail into
        // the status rather than take the thread down with it.
        Config { host: "127.0.0.1".to_owned(), port: 0, username: String::new(), password: String::new(), audio: false, encoding: Encoding::Zrle }
    }

    #[test]
    fn a_session_that_cannot_connect_ends_in_the_status_and_not_in_a_panic() {
        let client = Client::connect(nowhere(), Surface { width: 800, height: 600, scale: 2.0 });
        let woken = Arc::new(AtomicUsize::new(0));
        client.on_frame(Some(Box::new({
            let woken = Arc::clone(&woken);
            move || {
                woken.fetch_add(1, Ordering::SeqCst);
            }
        })));

        let status = loop {
            let status = client.status();
            if status.state == State::Closed {
                break status;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(status.error.is_some(), "a session that failed says why");
        assert!(woken.load(Ordering::SeqCst) > 0, "the window is woken when the state changes");
    }

    /// The session runs from `connect`, so a window that installs its wake
    /// after the session has already ended must still be told to look.
    #[test]
    fn a_wake_installed_after_the_state_changed_still_fires() {
        let client = Client::connect(nowhere(), Surface { width: 800, height: 600, scale: 2.0 });
        while client.status().state != State::Closed {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let woken = Arc::new(AtomicUsize::new(0));
        client.on_frame(Some(Box::new({
            let woken = Arc::clone(&woken);
            move || {
                woken.fetch_add(1, Ordering::SeqCst);
            }
        })));
        assert_eq!(woken.load(Ordering::SeqCst), 1, "installing the wake is the one call a closed session makes");
        client.on_frame(None);
        assert_eq!(woken.load(Ordering::SeqCst), 1, "clearing it calls nothing");
    }

    #[test]
    fn input_before_a_connection_is_dropped_rather_than_queued_forever() {
        let client = Client::connect(nowhere(), Surface { width: 800, height: 600, scale: 2.0 });
        client.pointer(BUTTON_LEFT, 10, 10);
        client.key(true, 0x61);
        client.clipboard("画面".to_owned());
        client.surface(Surface { width: 400, height: 300, scale: 1.0 });
        // Nothing has been drawn, so there is no damage and no cursor.
        client.with_frame(|fb, damage| {
            assert_eq!((fb.width(), fb.height()), (0, 0));
            assert_eq!(damage, None);
        });
        client.with_cursor(|generation, image| {
            assert_eq!(generation, 0);
            assert!(image.is_none());
        });
        client.with_clipboard(|generation, text| assert_eq!((generation, text), (0, None)));
    }

    /// A server that accepts the socket and then says nothing is the worst
    /// case for a shutdown: there is no timeout short of the connect timeout,
    /// and `Drop` waits for the thread, so the window would freeze with it.
    #[test]
    fn a_client_dropped_during_the_handshake_does_not_wait_for_the_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port to listen on");
        let port = listener.local_addr().expect("the port").port();
        // Accepts and hands the socket back, and nothing writes to it: the
        // session is left waiting for the server's version.
        let accept = std::thread::spawn(move || listener.accept().expect("the client's connection").0);

        let config = Config { host: "127.0.0.1".to_owned(), port, username: String::new(), password: String::new(), audio: false, encoding: Encoding::Zrle };
        let client = Client::connect(config, Surface { width: 800, height: 600, scale: 2.0 });
        let _socket = accept.join().expect("the accepting thread");

        let start = std::time::Instant::now();
        drop(client);
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "dropping took {:?}", start.elapsed());
    }

    /// The window reads the status on every wake and draws after it, so a
    /// status that took the damage left every draw with nothing to upload and
    /// the desktop black until something changed between the two.
    #[test]
    fn reading_the_size_leaves_the_damage_for_the_draw() {
        let client = Client::connect(nowhere(), Surface { width: 800, height: 600, scale: 2.0 });
        client.shared.framebuffer.lock().unwrap().resize(64, 48).unwrap();

        assert_eq!(client.desktop_size(), (64, 48));

        client.with_frame(|_, damage| assert_eq!(damage, Some(Region { x: 0, y: 0, width: 64, height: 48 })));
    }

    #[test]
    fn dropping_a_client_joins_its_thread_and_the_callback_stops_first() {
        let client = Client::connect(nowhere(), Surface { width: 800, height: 600, scale: 2.0 });
        client.on_frame(Some(Box::new(|| {})));
        drop(client);
    }
}
