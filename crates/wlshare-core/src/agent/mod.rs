//! Another machine's hardware H.264 encoder, over the network: both ends of
//! the link between a daemon and an encoding agent. Experimental.
//!
//! [`link`] is the link itself, its bytes and the key in front of them, and
//! says why it is TCP and what the key does and does not do. [`client`] is
//! the daemon's end, an encoder like any other to the stream it codes, and
//! [`serve`] the agent's, which the agent's binary runs in front of its
//! machine's encoder ([`crate::h264::vaapi`]).
//!
//! What crosses the link today is H.264 alone; the agent is named for
//! encoding at large, since another codec's hardware encoder would be lent
//! the same way.

pub mod client;
pub mod link;
pub mod serve;
