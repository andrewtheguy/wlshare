//! The daemon's pictures sent to its agent by `MSG_ZEROCOPY`: the kernel
//! sends the planes' own pages, pinned, rather than copying them into the
//! socket's buffers first, and says on the socket's error queue when it is
//! done with them. Nothing on the link changes: it is the daemon's way of
//! handing its bytes to the kernel, and the agent reads them as it reads any.
//!
//! Measured on the link it is for, a virtual machine to its host's container
//! (virtio-net, the bridge, a veth), a whole 1920×1080 picture costs the
//! sending processor 0.16–0.22 ms of the system's time where a copying
//! `writev` costs 0.76–0.87, and a frame takes as long as it did.
//!
//! What it holds to:
//!
//! - **The planes are not written until the kernel is done with them.** A
//!   frame's pages may still be in a queue of the network's when the agent's
//!   access unit is back, so a frame waits for the kernel's word on every
//!   call that sent zero-copy ([`ZeroCopy::settle`]) before it returns and
//!   the planes are read into again, within the frame's time.
//! - **Small writes are copied.** Pinning pages and the kernel's word cost
//!   more than copying a few kilobytes, so a frame goes zero-copy only where
//!   its slices are [`LEAST`] long on average: the whole picture, or rows
//!   the planes' whole width, and not a window's rows one at a time.
//! - **Where the kernel copies anyway, the daemon copies.** On loopback, or
//!   an interface that cannot gather pages, the kernel copies what it was
//!   given and says so; the stream then sends by copying from then on, which
//!   costs nothing to wait for.
//! - **Past what may be pinned, copied.** The pages pinned and not yet
//!   released count against the account's `RLIMIT_MEMLOCK`, 8 MiB by
//!   default, which a 3840×2160 picture, 12 MB, is past; and a call is
//!   charged its whole length before it sends a byte, so one call of a
//!   picture larger than the limit would be refused outright. So a call
//!   sends at most [`CALL`], and a call's pages are released once they are
//!   acknowledged, so that on the same link a 3840×2160 picture goes
//!   zero-copy whole under the default limit, for 0.3–0.5 ms of the system's
//!   time where copying it costs 2.7–3.0. Where less may be pinned (a lower
//!   limit, the account's other streams), the first call refused (`ENOBUFS`)
//!   is made again by copying, and so is the rest of the frame, since every
//!   call after it would be refused too.

use std::io::{ErrorKind, IoSlice, Write};
use std::os::fd::AsRawFd as _;
use std::time::Duration;

use crate::agent::link::{Timed, write_all_vectored};

/// The fewest bytes a slice of a frame is to be, on average, for the frame
/// to be sent zero-copy: the kernel's own guidance has it pay from about
/// 10 KB a call.
pub const LEAST: usize = 16 * 1024;

/// The most bytes one zero-copy call sends: a small part of the 8 MiB an
/// account may pin by default, so that the rest of what may be pinned is
/// used before a call is refused.
pub const CALL: usize = 1 << 20;

/// `SO_ZEROCOPY`, which the `libc` crate does not have for every target:
/// 60 on every architecture but MIPS, SPARC and PA-RISC, none of which the
/// daemon is built for.
const SO_ZEROCOPY: libc::c_int = 60;

/// Where a word on the error queue is the kernel's about zero-copy sends.
const SO_EE_ORIGIN_ZEROCOPY: u8 = 5;

/// That the kernel copied the pages after all.
const SO_EE_CODE_ZEROCOPY_COPIED: u8 = 1;

/// A connection's zero-copy sends: how many calls were made and how many the
/// kernel is done with, each numbered by the kernel from the first.
#[derive(Debug)]
pub struct ZeroCopy {
    sent: u32,
    done: u32,
    /// Whether frames go zero-copy at all: not once the kernel has copied.
    on: bool,
}

impl ZeroCopy {
    /// Zero-copy sends turned on for `stream`.
    pub fn new(stream: &std::net::TcpStream) -> std::io::Result<Self> {
        let one: libc::c_int = 1;
        // SAFETY: the option's value is the `c_int` it is given the size of,
        // on a socket the stream holds open.
        let set = unsafe {
            libc::setsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                SO_ZEROCOPY,
                (&raw const one).cast(),
                size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if set != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            sent: 0,
            done: 0,
            on: true,
        })
    }

    /// Whether the stream still sends zero-copy.
    pub fn on(&self) -> bool {
        self.on
    }

    /// Send every byte of `slices`, in order: zero-copy where they are
    /// large enough and the stream still sends so, copied otherwise. The
    /// slices are not to be written until [`Self::settle`] says the kernel is
    /// done with them.
    pub fn send(
        &mut self,
        link: &mut Timed<'_>,
        slices: &mut [IoSlice<'_>],
    ) -> std::io::Result<()> {
        let bytes: usize = slices.iter().map(|s| s.len()).sum();
        if self.on && bytes >= LEAST * slices.len() {
            let mut pinned = Pinned {
                link,
                sends: self,
                refused: false,
            };
            write_all_vectored(&mut pinned, slices)
        } else {
            write_all_vectored(link, slices)
        }
    }

    /// Wait, within what is left of `link`'s time, until the kernel is done
    /// with every page sent zero-copy.
    pub fn settle(&mut self, link: &Timed<'_>) -> std::io::Result<()> {
        let stream = link.stream();
        // A frame that sent nothing zero-copy has nothing to ask about.
        while self.done != self.sent {
            self.drain(stream)?;
            if self.done == self.sent {
                break;
            }
            // The time left, to the millisecond above; none is the frame's
            // whole wait gone.
            let wait = link.left()?.unwrap_or(Duration::from_secs(60));
            let wait = wait.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32;
            // Nothing asked for: an error queue with something in it is
            // always said.
            let mut on = libc::pollfd {
                fd: stream.as_raw_fd(),
                events: 0,
                revents: 0,
            };
            // SAFETY: one `pollfd`, as counted.
            match unsafe { libc::poll(&raw mut on, 1, wait) } {
                0 => continue, // `left` says it is late.
                n if n < 0 => {
                    let e = std::io::Error::last_os_error();
                    if e.kind() != ErrorKind::Interrupted {
                        return Err(e);
                    }
                }
                _ if on.revents & libc::POLLERR == 0 => {
                    return Err(std::io::Error::other(
                        "the connection closed with pages of a picture still in the kernel's hands",
                    ));
                }
                _ => {
                    // An error of the connection's own, not a word about
                    // its pages.
                    if let Some(e) = stream.take_error()? {
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Read every word the kernel has on `stream`'s error queue.
    fn drain(&mut self, stream: &std::net::TcpStream) -> std::io::Result<()> {
        loop {
            // Room for one word, aligned as a `cmsghdr` is.
            let mut control = [0u64; 16];
            // SAFETY: a `msghdr` is plain data, all zeros an empty one.
            let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = size_of_val(&control) as _;
            // SAFETY: the message points at the control buffer it was given
            // the size of, and at no data.
            let read = unsafe {
                libc::recvmsg(
                    stream.as_raw_fd(),
                    &raw mut message,
                    libc::MSG_ERRQUEUE | libc::MSG_DONTWAIT,
                )
            };
            if read < 0 {
                let e = std::io::Error::last_os_error();
                return match e.kind() {
                    ErrorKind::WouldBlock => Ok(()),
                    ErrorKind::Interrupted => continue,
                    _ => Err(e),
                };
            }
            // SAFETY: the control buffer as `recvmsg` filled it, walked by
            // the system's own macros.
            let mut header = unsafe { libc::CMSG_FIRSTHDR(&raw const message) };
            while !header.is_null() {
                // SAFETY: a header the macros found inside the buffer.
                let (level, kind) = unsafe { ((*header).cmsg_level, (*header).cmsg_type) };
                if matches!(
                    (level, kind),
                    (libc::SOL_IP, libc::IP_RECVERR) | (libc::SOL_IPV6, libc::IPV6_RECVERR)
                ) {
                    // SAFETY: an `IP_RECVERR` message's data is a
                    // `sock_extended_err`, not necessarily aligned.
                    let word: libc::sock_extended_err =
                        unsafe { std::ptr::read_unaligned(libc::CMSG_DATA(header).cast()) };
                    if word.ee_origin == SO_EE_ORIGIN_ZEROCOPY && word.ee_errno == 0 {
                        // The calls from `ee_info` to `ee_data`, both
                        // counted.
                        self.done = self
                            .done
                            .wrapping_add(word.ee_data.wrapping_sub(word.ee_info).wrapping_add(1));
                        if word.ee_code & SO_EE_CODE_ZEROCOPY_COPIED != 0 {
                            self.on = false;
                        }
                    }
                }
                // SAFETY: as above.
                header = unsafe { libc::CMSG_NXTHDR(&raw const message, header) };
            }
        }
    }
}

/// A connection written zero-copy, each call counted, until a call is
/// refused for what is pinned already.
struct Pinned<'s, 't, 'a> {
    link: &'t mut Timed<'a>,
    sends: &'s mut ZeroCopy,
    refused: bool,
}

impl Write for Pinned<'_, '_, '_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write_vectored(&[IoSlice::new(buf)])
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> std::io::Result<usize> {
        if self.refused {
            return self.link.write_vectored(bufs);
        }
        // The slices up to `CALL` bytes, the last one cut short where it
        // goes past.
        let mut total = 0;
        let past = bufs.iter().position(|buf| {
            total += buf.len();
            total > CALL
        });
        let cut;
        let call = match past {
            None => bufs,
            Some(at) => {
                let last = &bufs[at];
                let mut slices = bufs[..at].to_vec();
                slices.push(IoSlice::new(&last[..last.len() - (total - CALL)]));
                cut = slices;
                &cut[..]
            }
        };
        match self.link.send_vectored(call, libc::MSG_ZEROCOPY) {
            Ok(written) => {
                // A call that sent anything is numbered; one refused sent
                // nothing and is not.
                if written > 0 {
                    self.sends.sent = self.sends.sent.wrapping_add(1);
                }
                Ok(written)
            }
            // More pinned than the account may lock: this call copied, and
            // the rest of the frame.
            Err(e) if e.raw_os_error() == Some(libc::ENOBUFS) => {
                self.refused = true;
                self.link.write_vectored(bufs)
            }
            Err(e) => Err(e),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::link::FRAME_WITHIN;
    use std::io::Read as _;
    use std::net::{TcpListener, TcpStream};

    /// Bytes sent zero-copy arrive as sent, in order with the copied ones
    /// around them, and the kernel's word on every call is waited for.
    /// Loopback copies what it is handed zero-copy, and says so, so the
    /// stream copies from then on.
    #[test]
    fn what_is_sent_zero_copy_arrives_and_is_settled() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let sender = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut receiver, _) = listener.accept().unwrap();
        let picture: Vec<u8> = (0..3 << 20)
            .map(|n: u32| (n * 7 + n / 4093) as u8)
            .collect();
        let head = b"head";
        let mut expected = head.to_vec();
        expected.extend_from_slice(&picture);
        expected.extend_from_slice(&picture[..1000]);
        expected.extend_from_slice(head);
        let reader = std::thread::spawn(move || {
            let mut got = vec![0; expected.len()];
            receiver.read_exact(&mut got).unwrap();
            assert!(got == expected, "the bytes are not the ones sent");
        });
        let mut sends = ZeroCopy::new(&sender).unwrap();
        let mut link = Timed::new(&sender);
        link.within(FRAME_WITHIN);
        let mut slices = [
            IoSlice::new(head),
            IoSlice::new(&picture),
            IoSlice::new(&picture[..1000]),
        ];
        sends.send(&mut link, &mut slices).unwrap();
        // Three megabytes in calls of a megabyte at most.
        let sent = sends.sent;
        assert!(sent > 3, "{sent} zero-copy calls");
        // A frame of small slices is copied, zero-copy on or not.
        sends.send(&mut link, &mut [IoSlice::new(head)]).unwrap();
        assert_eq!(sends.sent, sent);
        reader.join().unwrap();
        sends.settle(&link).unwrap();
        assert_eq!(sends.done, sends.sent);
        assert!(!sends.on(), "loopback copied, and the stream does not");
    }
}
