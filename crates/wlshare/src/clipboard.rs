//! The compositor's clipboard through wlr-data-control, both ways, text only.
//!
//! A selection the compositor announces is read into a pipe on its own thread
//! and forwarded to clients as latin-1 cut text; a selection that is cleared or
//! stops being text is forwarded as empty text, so a client never keeps what the
//! compositor no longer has. Text from a client is offered as a data source and
//! becomes the selection; the compositor then announces that selection back,
//! which is ignored while the source is ours, so a paste never echoes.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;

use log::{debug, warn};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};

use crate::compositor::Compositor;
use crate::shared::{Event, Shared};

/// Text MIME types, most specific first.
const TEXT_MIMES: [&str; 5] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"];

#[derive(Default)]
pub struct Clipboard {
    pub manager: Option<ZwlrDataControlManagerV1>,
    device: Option<ZwlrDataControlDeviceV1>,
    offers: HashMap<ObjectId, Vec<String>>,
    /// The source this server currently owns the selection through.
    source: Option<ZwlrDataControlSourceV1>,
    text: Arc<str>,
}

impl Clipboard {
    pub fn attach(&mut self, qh: &QueueHandle<Compositor>, seat: &WlSeat) {
        if let Some(manager) = &self.manager {
            self.device = Some(manager.get_data_device(seat, qh, ()));
        }
    }

    /// Put `text` on the compositor's clipboard.
    pub fn set(&mut self, qh: &QueueHandle<Compositor>, text: String) {
        let (Some(manager), Some(device)) = (&self.manager, &self.device) else { return };
        if let Some(old) = self.source.take() {
            old.destroy();
        }
        let source = manager.create_data_source(qh, ());
        for mime in TEXT_MIMES {
            source.offer(mime.to_owned());
        }
        device.set_selection(Some(&source));
        self.text = text.into();
        self.source = Some(source);
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for Compositor {
    fn event(state: &mut Self, _: &ZwlrDataControlDeviceV1, event: zwlr_data_control_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                state.clipboard.offers.insert(id.id(), Vec::new());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                if state.clipboard.source.is_some() {
                    // Our own selection coming back.
                    if let Some(offer) = id {
                        state.clipboard.offers.remove(&offer.id());
                        offer.destroy();
                    }
                    return;
                }
                let Some(offer) = id else {
                    debug!("the selection was cleared");
                    state.shared().emit(Event::Clipboard(String::new()));
                    return;
                };
                let mimes = state.clipboard.offers.remove(&offer.id()).unwrap_or_default();
                let Some(mime) = TEXT_MIMES.iter().find(|m| mimes.iter().any(|have| have == *m)) else {
                    debug!("a selection with no text: {mimes:?}");
                    state.shared().emit(Event::Clipboard(String::new()));
                    offer.destroy();
                    return;
                };
                match rustix::pipe::pipe() {
                    Ok((read, write)) => {
                        offer.receive((*mime).to_owned(), write.as_fd());
                        drop(write);
                        read_selection(read, state.shared().clone());
                    }
                    Err(e) => warn!("cannot make a pipe for the clipboard: {e}"),
                }
                offer.destroy();
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => {
                state.clipboard.offers.remove(&offer.id());
                offer.destroy();
            }
            zwlr_data_control_device_v1::Event::Finished => {
                warn!("the compositor withdrew the clipboard device");
                state.clipboard.device = None;
            }
            _ => {}
        }
    }

    event_created_child!(Compositor, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

/// Read a selection to its end off the loop, then hand it to the sessions.
fn read_selection(read: OwnedFd, shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || {
            let mut file = std::fs::File::from(read);
            let mut bytes = Vec::new();
            match file.read_to_end(&mut bytes) {
                Ok(_) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    debug!("clipboard: {} bytes from the compositor", bytes.len());
                    shared.emit(Event::Clipboard(text));
                }
                Err(e) => warn!("reading the selection: {e}"),
            }
        })
        .ok();
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for Compositor {
    fn event(state: &mut Self, offer: &ZwlrDataControlOfferV1, event: zwlr_data_control_offer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Some(mimes) = state.clipboard.offers.get_mut(&offer.id())
        {
            mimes.push(mime_type);
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for Compositor {
    fn event(state: &mut Self, source: &ZwlrDataControlSourceV1, event: zwlr_data_control_source_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_data_control_source_v1::Event::Send { fd, .. } => {
                let text = state.clipboard.text.clone();
                std::thread::Builder::new()
                    .name("clipboard-send".into())
                    .spawn(move || {
                        let mut file = std::fs::File::from(fd);
                        if let Err(e) = file.write_all(text.as_bytes()) {
                            debug!("writing the selection: {e}");
                        }
                    })
                    .ok();
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                if state.clipboard.source.as_ref() == Some(source) {
                    state.clipboard.source = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Compositor: ignore ZwlrDataControlManagerV1);
