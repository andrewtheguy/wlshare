//! Where the compositor says the cursor is, on every output: a line for each
//! event of an ext-image-copy-capture pointer cursor session on each, the
//! witness `scripts/pointer-e2e.sh` holds the daemon's pointers to.
//!
//! ```text
//! cursor-watch
//! ```
//!
//! It connects to the compositor `WAYLAND_DISPLAY` names, waits for the seat to
//! have a pointer — the daemon's virtual one — and opens a cursor session with
//! no capture on every output there is then. Every line is the output's name
//! and what its session said:
//!
//! ```text
//! HEADLESS-1 enter
//! HEADLESS-1 position 100,100
//! HEADLESS-2 leave
//! ```
//!
//! A position is the hotspot's place on the output in its pixels, upright,
//! rounded, and sent only when it changes; an output counts the cursor as on
//! it while any of its image is. It runs until it is killed or the compositor
//! goes.

use wayland_client::protocol::{wl_output, wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_cursor_session_v1::{self, ExtImageCopyCaptureCursorSessionV1},
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
};

#[derive(Default)]
struct Watch {
    outputs: Vec<(wl_output::WlOutput, String)>,
    seat: Option<wl_seat::WlSeat>,
    pointer: bool,
    sources: Option<ExtOutputImageCaptureSourceManagerV1>,
    manager: Option<ExtImageCopyCaptureManagerV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Watch {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_output" if version >= 4 => state.outputs.push((registry.bind(name, 4, qh, ()), String::new())),
                "wl_seat" => state.seat = Some(registry.bind(name, 1, qh, ())),
                "ext_output_image_capture_source_manager_v1" => state.sources = Some(registry.bind(name, 1, qh, ())),
                "ext_image_copy_capture_manager_v1" => state.manager = Some(registry.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Watch {
    fn event(state: &mut Self, output: &wl_output::WlOutput, event: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = event
            && let Some(entry) = state.outputs.iter_mut().find(|(o, _)| o == output)
        {
            entry.1 = name;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Watch {
    fn event(state: &mut Self, _: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } = event
            && capabilities.contains(wl_seat::Capability::Pointer)
        {
            state.pointer = true;
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, String> for Watch {
    fn event(_: &mut Self, _: &ExtImageCopyCaptureCursorSessionV1, event: ext_image_copy_capture_cursor_session_v1::Event, output: &String, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            ext_image_copy_capture_cursor_session_v1::Event::Enter => println!("{output} enter"),
            ext_image_copy_capture_cursor_session_v1::Event::Leave => println!("{output} leave"),
            ext_image_copy_capture_cursor_session_v1::Event::Position { x, y } => println!("{output} position {x},{y}"),
            _ => {}
        }
    }
}

delegate_noop!(Watch: ignore wl_pointer::WlPointer);
delegate_noop!(Watch: ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(Watch: ignore ExtImageCaptureSourceV1);
delegate_noop!(Watch: ExtImageCopyCaptureManagerV1);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let mut state = Watch::default();
    conn.display().get_registry(&qh, ());
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    let (Some(seat), Some(sources), Some(manager)) = (state.seat.clone(), state.sources.clone(), state.manager.clone()) else {
        return Err("the compositor has no seat or no ext-image-copy-capture with output sources".into());
    };
    // `get_pointer` is a protocol error before the seat has had a pointer.
    while !state.pointer {
        queue.blocking_dispatch(&mut state)?;
    }
    let pointer = seat.get_pointer(&qh, ());
    let mut sessions = Vec::new();
    for (output, name) in &state.outputs {
        let source = sources.create_source(output, &qh, ());
        sessions.push(manager.create_pointer_cursor_session(&source, &pointer, &qh, name.clone()));
    }
    eprintln!("cursor-watch: watching {}", state.outputs.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", "));
    loop {
        queue.blocking_dispatch(&mut state)?;
    }
}
