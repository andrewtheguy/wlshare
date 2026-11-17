//! What wlshare's binaries share: the daemon (`wlshare`) and the encoding agent
//! (`wlshare-enc-agent`), each a crate of its own built on this one.
//!
//! Today that is the hardware H.264 encoder ([`h264`]) and the link that
//! lends it from one machine to a daemon on another ([`agent`]): the daemon codes its
//! streams by the encoder where its host has a GPU and is the link's one end,
//! and the agent is the other end, in front of the same encoder. It is here
//! so that the agent's machine needs libva and a VA-API driver and nothing
//! else of the daemon's: no compositor, no PipeWire, no libx264.
//!
//! Not the private RFB extensions' bytes, which are `wlshare-proto`'s: that
//! crate is the one the zDesk gateway depends on, and takes no C with it.

pub mod agent;
pub mod h264;
