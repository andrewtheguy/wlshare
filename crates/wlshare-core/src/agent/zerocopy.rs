//! The daemon's pictures sent to its agent by `MSG_ZEROCOPY`: the kernel
//! sends the planes' own pages, pinned, rather than copying them into the
//! socket's buffers first, and says on the socket's error queue when it is
//! done with them. Nothing on the link changes: it is the daemon's way of
//! handing its bytes to the kernel, and the agent reads them as it reads any.
//!
//! On the link it is for, a virtual machine to its host's container, it
//! takes most of the sending processor's time off a picture, and a frame
//! takes as long as it did: measured in `docs/video-performance.md`, *The
//! encoding agent's link*.
//!
//! What it holds to:
//!
//! - **The planes are not written until the kernel is done with them.** A
//!   frame's pages may still be in a queue of the network's when the agent's
//!   access unit is back, so a frame waits for the kernel's word on every
//!   call that sent zero-copy ([`ZeroCopy::settle`]) before it returns and
//!   the planes are read into again, within the frame's time.
//! - **A frame that fails returns once the kernel is done too.** A frame
//!   that fails mid-send, or whose access unit is not back in its time, has
//!   pages the kernel may still send, and the caller is then free to write
//!   the planes or let them go, their memory the allocator's to give to
//!   anything. So the stream resets its connection, which frees every packet
//!   TCP holds, and waits for the kernel's word on what the network device
//!   has yet to send or drop ([`ZeroCopy::abandon`]), with no limit but the
//!   device's: nothing of the daemon's memory is sent after the frame has
//!   returned.
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
//!   zero-copy whole under the default limit. Where less may be pinned (a lower
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

/// How long [`ZeroCopy::abandon`] waits before it says it is waiting: the
/// kernel lets go of what a reset frees at once, and a device of what it
/// holds in milliseconds.
const LATE: Duration = Duration::from_secs(1);

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

    /// After a frame has failed: reset the connection, so that TCP lets go
    /// of every packet it holds, and wait until the kernel is done with every
    /// page sent zero-copy, those a network device has yet to send or drop
    /// among them, for as long as that takes. The connection is no use after
    /// it.
    pub fn abandon(&mut self, stream: &std::net::TcpStream) {
        if self.done == self.sent {
            return;
        }
        // A connect to no address disconnects a TCP socket: its queues are
        // purged and the other end is sent a reset, and the socket, still
        // open, is told of the pages as they are let go.
        // SAFETY: a `sockaddr` of no family, as long as it is said to be.
        let reset = unsafe {
            let mut none: libc::sockaddr = std::mem::zeroed();
            none.sa_family = libc::AF_UNSPEC as libc::sa_family_t;
            libc::connect(
                stream.as_raw_fd(),
                &raw const none,
                size_of::<libc::sockaddr>() as libc::socklen_t,
            )
        };
        if reset != 0 {
            log::warn!(
                "the encoding agent's connection: resetting it: {}",
                std::io::Error::last_os_error()
            );
        }
        let began = std::time::Instant::now();
        let mut said = false;
        // A closed socket is always ready, so its error queue is looked at
        // every millisecond rather than polled for.
        while self.done != self.sent {
            if let Err(e) = self.drain(stream) {
                log::error!(
                    "the encoding agent's connection: no word from the kernel on what it still \
                     holds of a picture: {e}"
                );
                return;
            }
            if !said && began.elapsed() > LATE {
                said = true;
                log::warn!(
                    "the encoding agent's connection: the kernel still holds pages of a picture \
                     after {LATE:?}: waiting for the network device to let them go"
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }
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

/// Whether the account may pin enough for the tests' pictures to go
/// zero-copy: 8 MiB, the default, or no limit. Below it, a call may be
/// refused and copied, which is no failure.
#[cfg(test)]
pub(crate) fn roomy() -> bool {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: one `rlimit`, written by the call.
    let got = unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &raw mut limit) };
    got == 0 && (limit.rlim_cur == libc::RLIM_INFINITY || limit.rlim_cur >= 8 << 20)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::link::FRAME_WITHIN;
    use std::io::Read as _;
    use std::net::{TcpListener, TcpStream};

    /// A connection on loopback, its sender's sends, and the receiving end
    /// held to the bytes of a large picture between two small slices and a
    /// small frame after them, which is what the tests send.
    fn sent_and_read(
        sends: impl FnOnce(&TcpStream) -> ZeroCopy,
    ) -> (TcpStream, ZeroCopy, Vec<u8>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let sender = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut receiver, _) = listener.accept().unwrap();
        let picture: Vec<u8> = (0..3 << 20)
            .map(|n: u32| (n * 7 + n / 4093) as u8)
            .collect();
        let mut expected = HEAD.to_vec();
        expected.extend_from_slice(&picture);
        expected.extend_from_slice(&picture[..1000]);
        expected.extend_from_slice(HEAD);
        let reader = std::thread::spawn(move || {
            let mut got = vec![0; expected.len()];
            receiver.read_exact(&mut got).unwrap();
            assert!(got == expected, "the bytes are not the ones sent");
        });
        let sends = sends(&sender);
        (sender, sends, picture, reader)
    }

    const HEAD: &[u8] = b"head";

    /// The large frame and the small one, sent; how many calls went
    /// zero-copy for the large one.
    fn send_both(sends: &mut ZeroCopy, link: &mut Timed<'_>, picture: &[u8]) -> u32 {
        let mut slices = [
            IoSlice::new(HEAD),
            IoSlice::new(picture),
            IoSlice::new(&picture[..1000]),
        ];
        sends.send(link, &mut slices).unwrap();
        let sent = sends.sent;
        // A frame of small slices is copied, zero-copy on or not.
        sends.send(link, &mut [IoSlice::new(HEAD)]).unwrap();
        assert_eq!(sends.sent, sent);
        sent
    }

    /// Bytes sent zero-copy arrive as sent, in order with the copied ones
    /// around them, and the kernel's word on every call is waited for.
    /// Loopback copies what it is handed zero-copy, and says so, so the
    /// stream copies from then on; an account that may pin too little has
    /// its calls refused and copied, and nothing to wait for.
    #[test]
    fn what_is_sent_zero_copy_arrives_and_is_settled() {
        let (sender, mut sends, picture, reader) =
            sent_and_read(|sender| ZeroCopy::new(sender).unwrap());
        let mut link = Timed::new(&sender);
        link.within(FRAME_WITHIN);
        let sent = send_both(&mut sends, &mut link, &picture);
        if roomy() {
            // Three megabytes in calls of a megabyte at most.
            assert!(sent > 3, "{sent} zero-copy calls");
        }
        reader.join().unwrap();
        sends.settle(&link).unwrap();
        assert_eq!(sends.done, sends.sent);
        assert_eq!(
            sends.on(),
            sent == 0,
            "loopback copied what went zero-copy, and only that says so"
        );
    }

    /// A frame cut short by its time, its pages in the kernel's queues for
    /// an agent that reads nothing, is let go of whole once abandoned.
    #[test]
    fn a_frame_abandoned_is_let_go_of() {
        let listener =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        // A small window, so that most of the picture waits in the
        // sender's queues.
        listener.set_recv_buffer_size(64 << 10).unwrap();
        listener
            .bind(
                &"127.0.0.1:0"
                    .parse::<std::net::SocketAddr>()
                    .unwrap()
                    .into(),
            )
            .unwrap();
        listener.listen(1).unwrap();
        let listener: TcpListener = listener.into();
        let sender = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_reads_nothing, _) = listener.accept().unwrap();
        let picture = vec![7u8; 16 << 20];
        let mut sends = ZeroCopy::new(&sender).unwrap();
        let mut link = Timed::new(&sender);
        link.within(Duration::from_millis(300));
        let late = sends.send(&mut link, &mut [IoSlice::new(&picture)]);
        assert!(late.is_err(), "sixteen megabytes in a window of 64 KiB");
        if roomy() {
            assert!(sends.sent > 0, "nothing went zero-copy");
        }
        let began = std::time::Instant::now();
        sends.abandon(&sender);
        assert_eq!(sends.done, sends.sent);
        assert!(began.elapsed() < LATE, "{:?}", began.elapsed());
    }

    /// A stream that copies, as one does once the kernel has copied, sends
    /// the same bytes, none of them zero-copy, and waits for nothing.
    #[test]
    fn what_is_copied_arrives_and_waits_for_nothing() {
        let (sender, mut sends, picture, reader) = sent_and_read(|sender| {
            let mut sends = ZeroCopy::new(sender).unwrap();
            sends.on = false;
            sends
        });
        let mut link = Timed::new(&sender);
        link.within(FRAME_WITHIN);
        assert_eq!(send_both(&mut sends, &mut link, &picture), 0);
        reader.join().unwrap();
        sends.settle(&link).unwrap();
        assert!(!sends.on());
    }
}
