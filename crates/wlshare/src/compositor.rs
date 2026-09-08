//! The thread that owns the Wayland connection.
//!
//! Everything the compositor is asked or tells happens here, on one calloop:
//! the Wayland socket, the command channel from the sessions, and the capture
//! timer. Sessions never touch a Wayland object; they send [`Command`]s and read
//! the [`Shared`] state and [`Event`]s this thread produces.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context as _;
use calloop::channel;
use calloop::{EventLoop, LoopHandle};
use calloop_wayland_source::WaylandSource;
use log::{debug, info, warn};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_manager_v1::ZwlrOutputManagerV1;
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;

use crate::capture::Capture;
use crate::clipboard::Clipboard;
use crate::config::{Config, Xkb};
use crate::framebuffer::Framebuffer;
use crate::input::Input;
use crate::outputs::{ConfigKind, OutputInfo, Outputs};
use crate::shared::{ClientId, Command, Event, Geometry, Shared};

pub struct Compositor {
    shared: Option<Arc<Shared>>,
    pub qh: QueueHandle<Compositor>,
    pub handle: LoopHandle<'static, Compositor>,
    registry: WlRegistry,
    pub shm: WlShm,
    seat: Option<WlSeat>,
    keyboards: Option<ZwpVirtualKeyboardManagerV1>,
    pointers: Option<ZwlrVirtualPointerManagerV1>,
    pub outputs: Outputs,
    pub capture: Capture,
    input: Option<Input>,
    pub clipboard: Clipboard,
    pub clients: HashSet<ClientId>,
    /// The client whose resize or declaration the output last followed; the
    /// layout is theirs until they leave, as wayvnc has it.
    layout_owner: Option<ClientId>,
    /// A client's SetDesktopSize the compositor accepted, until the frame at that
    /// size arrives.
    pub pending_resize: Option<(ClientId, u16, u16)>,
    pub max_fps: u32,
    resize_allowed: bool,
    xkb: Xkb,
    /// The connection and queue, held between `connect` and `discover`.
    pending_queue: Option<(Connection, wayland_client::EventQueue<Compositor>)>,
    exit: Option<anyhow::Result<()>>,
}

/// The running compositor thread, as the main thread sees it.
pub struct Handle {
    done: tokio::sync::oneshot::Receiver<anyhow::Result<()>>,
}

impl Handle {
    /// Resolves when the thread ends: `Ok` for a compositor that closed the
    /// connection, `Err` for anything that went wrong.
    pub async fn exited(&mut self) -> anyhow::Result<()> {
        match (&mut self.done).await {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("the compositor thread ended without a word")),
        }
    }
}

/// Connect to the compositor, learn the outputs, and run the thread. Returns
/// once the shared state exists, so the listener can announce a framebuffer.
pub fn start(config: &Config) -> anyhow::Result<(Handle, Arc<Shared>)> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<Arc<Shared>>>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let output = config.output.clone();
    let (max_fps, resize, xkb) = (config.max_fps, config.resize, config.xkb.clone());
    std::thread::Builder::new()
        .name("compositor".into())
        .spawn(move || {
            let result = run(output.as_deref(), max_fps, resize, xkb, ready_tx);
            let _ = done_tx.send(result);
        })
        .context("spawning the compositor thread")?;
    let shared = ready_rx.recv().map_err(|_| anyhow::anyhow!("the compositor thread died during startup"))??;
    Ok((Handle { done: done_rx }, shared))
}

fn run(
    output: Option<&str>,
    max_fps: u32,
    resize: bool,
    xkb: Xkb,
    ready: std::sync::mpsc::Sender<anyhow::Result<Arc<Shared>>>,
) -> anyhow::Result<()> {
    let mut event_loop: EventLoop<'static, Compositor> = EventLoop::try_new().context("creating the event loop")?;
    let (commands_tx, commands_rx) = channel::channel::<Command>();
    let mut compositor = match connect(event_loop.handle(), max_fps, resize, xkb) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(e));
            return Ok(());
        }
    };
    let queue = match compositor.discover(output, commands_tx) {
        Ok(queue) => queue,
        Err(e) => {
            let _ = ready.send(Err(e));
            return Ok(());
        }
    };
    let shared = compositor.shared().clone();

    WaylandSource::new(queue.0, queue.1)
        .insert(event_loop.handle())
        .map_err(|e| anyhow::anyhow!("inserting the Wayland source: {e}"))?;
    event_loop
        .handle()
        .insert_source(commands_rx, |event, _, state: &mut Compositor| match event {
            channel::Event::Msg(command) => state.handle_command(command),
            channel::Event::Closed => state.exit = Some(Err(anyhow::anyhow!("the command channel closed"))),
        })
        .map_err(|e| anyhow::anyhow!("inserting the command channel: {e}"))?;
    let _ = ready.send(Ok(shared));

    let signal = event_loop.get_signal();
    event_loop
        .run(None, &mut compositor, |state| {
            if state.exit.is_some() {
                signal.stop();
            }
        })
        .context("the Wayland event loop")?;
    compositor.exit.take().unwrap_or(Ok(()))
}

/// Connect and bind the globals. The queue is returned separately because the
/// registry roundtrip needs `&mut Compositor` beside it.
fn connect(handle: LoopHandle<'static, Compositor>, max_fps: u32, resize: bool, xkb: Xkb) -> anyhow::Result<Compositor> {
    let conn = Connection::connect_to_env().context("connecting to the Wayland compositor (is WAYLAND_DISPLAY set?)")?;
    let (globals, queue) = registry_queue_init::<Compositor>(&conn).context("reading the registry")?;
    let qh = queue.handle();

    let shm: WlShm = globals.bind(&qh, 1..=1, ()).context("wl_shm")?;
    let seat: Option<WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
    let screencopy: Option<ZwlrScreencopyManagerV1> = globals.bind(&qh, 2..=3, ()).ok();
    let output_manager: Option<ZwlrOutputManagerV1> = globals.bind(&qh, 1..=4, ()).ok();
    let keyboards: Option<ZwpVirtualKeyboardManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    let pointers: Option<ZwlrVirtualPointerManagerV1> = globals.bind(&qh, 2..=2, ()).ok();
    let data_control: Option<ZwlrDataControlManagerV1> = globals.bind(&qh, 1..=2, ()).ok();
    anyhow::ensure!(screencopy.is_some(), "the compositor does not offer wlr-screencopy version 2 or later");
    if output_manager.is_none() {
        warn!("no wlr-output-management: the output cannot be resized or rescaled, and its scale is read from wl_output only");
    }
    if keyboards.is_none() || pointers.is_none() {
        warn!("no virtual keyboard or pointer protocol: input will be dropped");
    }
    if data_control.is_none() {
        warn!("no wlr-data-control: the clipboard is not shared");
    }

    let mut compositor = Compositor {
        shared: None,
        qh: qh.clone(),
        handle,
        registry: globals.registry().clone(),
        shm,
        seat,
        keyboards,
        pointers,
        outputs: Outputs::default(),
        capture: Capture::default(),
        input: None,
        clipboard: Clipboard::default(),
        clients: HashSet::new(),
        layout_owner: None,
        pending_resize: None,
        max_fps,
        resize_allowed: resize,
        xkb,
        pending_queue: None,
        exit: None,
    };
    compositor.outputs.manager = output_manager;
    compositor.capture.manager = screencopy;
    compositor.clipboard.manager = data_control;
    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            compositor.bind_output(global.name, global.version);
        }
    }
    compositor.pending_queue = Some((conn, queue));
    Ok(compositor)
}

impl Compositor {
    pub fn shared(&self) -> &Arc<Shared> {
        self.shared.as_ref().expect("the shared state exists once discovery is done")
    }

    fn bind_output(&mut self, name: u32, version: u32) {
        if version < 4 {
            warn!("wl_output {name} is version {version}; version 4 is needed to learn its name");
        }
        let output: WlOutput = self.registry.bind(name, version.min(4), &self.qh, ());
        self.outputs.outputs.push(OutputInfo {
            output,
            global: name,
            name: None,
            mode: (0, 0),
            transform: wayland_client::protocol::wl_output::Transform::Normal,
            wl_scale: 1,
            done: false,
        });
    }

    /// Learn the outputs, pick one, and build the shared state.
    fn discover(&mut self, wanted: Option<&str>, commands: channel::Sender<Command>) -> anyhow::Result<(Connection, wayland_client::EventQueue<Compositor>)> {
        let (conn, mut queue) = self.pending_queue.take().expect("the queue from connect");
        // Two round trips: the first delivers the outputs and heads, the second the
        // events of the mode objects the heads created.
        queue.roundtrip(self).context("learning the outputs")?;
        queue.roundtrip(self).context("learning the outputs")?;
        self.outputs.select(wanted)?;
        let (width, height) = self.outputs.size();
        anyhow::ensure!(width > 0 && height > 0, "the shared output has no mode");
        let geometry = Geometry { width, height, scale: self.outputs.scale() };
        let shared = Arc::new(Shared::new(Framebuffer::new(width, height), geometry, commands));
        self.shared = Some(shared);

        if let (Some(seat), Some(keyboards), Some(pointers)) = (&self.seat, &self.keyboards, &self.pointers) {
            let output = self.outputs.selected().expect("selected").output.clone();
            self.input = Some(Input::new(&self.qh, keyboards, pointers, seat, &output, &self.xkb)?);
        }
        if let Some(seat) = &self.seat {
            self.clipboard.attach(&self.qh, seat);
        }
        Ok((conn, queue))
    }

    /// Refresh the shared geometry from the outputs; tell the sessions if it
    /// changed.
    pub fn geometry_changed(&mut self) {
        if self.refresh_geometry() {
            self.shared().emit(Event::Geometry { to: None });
        }
    }

    /// Refresh the shared geometry and report it whether or not it changed: a
    /// declaration is owed an answer.
    pub fn answer_geometry(&mut self, to: Option<ClientId>) {
        self.refresh_geometry();
        self.shared().emit(Event::Geometry { to });
    }

    fn refresh_geometry(&mut self) -> bool {
        let (width, height) = self.outputs.size();
        let scale = self.outputs.scale();
        let now = Geometry { width, height, scale };
        let mut g = self.shared().geometry.lock().unwrap();
        if *g == now {
            return false;
        }
        info!("output geometry: {width}x{height} at scale {scale:.2}");
        *g = now;
        true
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::ClientJoined { id, shared } => {
                if !shared && !self.clients.is_empty() {
                    info!("client {} asked for the desktop to itself; disconnecting {} other client(s)", id.0, self.clients.len());
                    self.shared().emit(Event::Exclusive { keep: id });
                }
                self.clients.insert(id);
                self.start_capture();
            }
            Command::ClientLeft(id) => {
                if !self.clients.remove(&id) {
                    // A connection that never got past the handshake held nothing.
                    return;
                }
                if self.layout_owner == Some(id) {
                    debug!("client {} released the layout", id.0);
                    self.layout_owner = None;
                }
                if self.pending_resize.is_some_and(|(c, _, _)| c == id) {
                    self.pending_resize = None;
                }
                if let Some(input) = &mut self.input {
                    input.release_client(id);
                }
                if self.clients.is_empty() {
                    self.stop_capture();
                }
            }
            Command::Key { client, keysym, down } => {
                if let Some(input) = &mut self.input {
                    input.key(client, keysym, down);
                }
            }
            Command::Pointer { client, buttons, x, y } => {
                let extent = {
                    let fb = self.shared().framebuffer.lock().unwrap();
                    (fb.width, fb.height)
                };
                if let Some(input) = &mut self.input {
                    input.pointer(client, buttons, x, y, extent);
                }
            }
            Command::Resize { client, width, height } => self.resize(client, width, height),
            Command::Declare { client, scale } => self.declare(client, scale),
            Command::SetClipboard(text) => self.clipboard.set(&self.qh.clone(), text),
        }
    }

    /// Own the layout for `client`, or say who does.
    fn claim_layout(&mut self, client: ClientId) -> bool {
        match self.layout_owner {
            Some(owner) if owner != client => false,
            _ => {
                self.layout_owner = Some(client);
                true
            }
        }
    }

    fn resize(&mut self, client: ClientId, width: u16, height: u16) {
        let refuse = |this: &Self, status: u16| this.shared().emit(Event::ResizeRefused { client, status });
        if !self.resize_allowed {
            info!("client {} asked for {width}x{height}: resizing is disabled", client.0);
            return refuse(self, wlshare_rfb::msg::EDS_STATUS_PROHIBITED);
        }
        if !self.claim_layout(client) {
            info!("client {} asked for {width}x{height}: another client owns the layout", client.0);
            return refuse(self, wlshare_rfb::msg::EDS_STATUS_PROHIBITED);
        }
        if width == 0 || height == 0 {
            return refuse(self, wlshare_rfb::msg::EDS_STATUS_INVALID_LAYOUT);
        }
        info!("client {} asks for a {width}x{height} desktop", client.0);
        self.pending_resize = Some((client, width, height));
        let qh = self.qh.clone();
        if !self.outputs.configure(&qh, Some((width, height)), None, ConfigKind::Resize { client }) {
            self.pending_resize = None;
            refuse(self, wlshare_rfb::msg::EDS_STATUS_PROHIBITED);
        }
    }

    fn declare(&mut self, client: ClientId, scale: f64) {
        let current = self.outputs.scale();
        if (current - scale).abs() < 0.005 {
            return self.answer_geometry(Some(client));
        }
        if !self.resize_allowed {
            info!("not following client {}'s density {scale:.2}: resizing is disabled", client.0);
            return self.answer_geometry(Some(client));
        }
        if !self.claim_layout(client) {
            info!("not following client {}'s density {scale:.2}: another client owns the layout", client.0);
            return self.answer_geometry(Some(client));
        }
        info!("following client {}'s density: output scale {current:.2} -> {scale:.2}", client.0);
        let qh = self.qh.clone();
        if !self.outputs.configure(&qh, None, Some(scale), ConfigKind::Scale) {
            self.answer_geometry(Some(client));
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Compositor {
    fn event(state: &mut Self, _: &WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_output" => {
                debug!("a new output appeared");
                state.bind_output(name, version);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = state.outputs.outputs.iter().position(|o| o.global == name) {
                    let removed = state.outputs.outputs.remove(i);
                    let was_selected = removed.name.as_deref() == state.outputs.selected.as_deref();
                    warn!("output {} went away{}", removed.name.unwrap_or_default(), if was_selected { "; it is the shared one" } else { "" });
                    removed.output.release();
                }
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Compositor: ignore WlSeat);
