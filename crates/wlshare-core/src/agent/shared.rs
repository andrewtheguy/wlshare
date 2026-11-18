//! The pictures of a daemon and its agent in memory both map, where the
//! agent is on the host of the daemon's virtual machine, or in a container
//! beside it: the agent reads a picture from where the daemon wrote it, and
//! the link carries the rest ([`crate::agent::link`]). Experimental.
//!
//! **The memory** is a file at each end, the same pages: in the guest, the
//! BAR of QEMU's `ivshmem-plain` device as sysfs gives it to map
//! (`/sys/bus/pci/devices/<address>/resource2_wc`, write-combined, since the
//! daemon only writes it), and on the host, or in a container it is bound
//! into, the file the device is backed by. Each end is told its file, and
//! maps all of it ([`Region`]); two ends that map different memory are told
//! so as the stream opens, by a mark the daemon writes and the agent looks
//! for.
//!
//! **A picture a stream**, whole, laid out as [`Planes`] lays its planes
//! out, luma then chroma, at a page of the region the agent gives the stream
//! as it opens it ([`Places`]). The picture stays there from frame to
//! frame: the stream's planes at each end are views of it, so the daemon
//! converts what changed of the desktop straight into it and sends the
//! frame's header and its rectangles, what did not change being still there
//! from the frames before; and the agent's encoder uploads it from where it
//! lies. Neither end copies any of it. A stream's first frame is the whole
//! picture, so a place is written whole before its stream first reads it,
//! and what an earlier stream left there is never read.
//!
//! **Whose a picture is, and when.** The link is request and reply, so the
//! picture is the daemon's to write between an access unit back and the
//! next frame's header, when the agent is done with it; a store fence after
//! the writes puts them ahead of the header that names them, which a
//! write-combined mapping would otherwise leave in its buffers. A frame that
//! fails, by its time or by its connection, leaves the agent perhaps still
//! uploading it, and the daemon, having reset the connection, is told no
//! more on it: so a failed stream's planes at the daemon are its own again,
//! none of the region, and
//! the agent gives a place to no other stream until the one that had it is
//! done, its thread having ended. The agent's account of its places is the
//! one there is, its own memory and not the region's, so an agent that
//! restarts has every place free, its streams having ended with it, and a
//! daemon that restarts asks for places anew. A stream the region has no
//! room for, while the places are held, has its pictures cross the socket.
//!
//! **The file's permissions are its access control.** Whoever can open the
//! file in the agent's machine, or the device's resource in the guest, reads
//! the desktop's pictures and can write into them; the key holds the socket
//! alone.

use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Context as _;

use crate::agent::link::MARK;
use crate::h264::Planes;

/// Where every picture starts in the region: a page.
pub const ALIGN: usize = 4096;

/// A file both ends map, all of it.
pub struct Region {
    map: memmap2::MmapRaw,
}

impl Region {
    /// The memory at `path`, mapped to be read and written.
    pub fn open(path: &Path) -> anyhow::Result<Arc<Self>> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        let length = file
            .metadata()
            .with_context(|| format!("reading the size of {}", path.display()))?
            .len();
        anyhow::ensure!(
            length > 0,
            "{} is empty: it is no memory to share",
            path.display()
        );
        let length = usize::try_from(length)
            .with_context(|| format!("{} is past what can be mapped", path.display()))?;
        let map = memmap2::MmapOptions::new()
            .len(length)
            .map_raw(&file)
            .with_context(|| format!("mapping {}", path.display()))?;
        Ok(Arc::new(Self { map }))
    }

    /// How many bytes the region is.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the region has no bytes, which an opened one always has.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The region's bytes from `at`, which is inside it.
    fn at(&self, at: usize) -> *mut u8 {
        assert!(
            at < self.len(),
            "{at} is past the region's {} bytes",
            self.len()
        );
        // SAFETY: inside the mapping, just held to it.
        unsafe { self.map.as_mut_ptr().add(at) }
    }
}

/// The bytes a `width`×`height` picture takes of the region, its planes and
/// what is left of the last page.
pub fn picture_bytes((width, height): (usize, usize)) -> usize {
    let (_, y, uv) = Planes::layout(width as u16, height as u16);
    (y + uv).next_multiple_of(ALIGN)
}

/// Put every write before this one ahead of what is sent after it, a
/// write-combined mapping's included.
fn fence() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: SFENCE is in the baseline of x86-64, and only orders stores.
    #[allow(unused_unsafe)]
    unsafe {
        std::arch::x86_64::_mm_sfence();
    }
    #[cfg(not(target_arch = "x86_64"))]
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
}

/// The daemon's picture of a stream: where the agent placed it in the
/// region, which the daemon writes and never reads.
pub struct Picture {
    region: Arc<Region>,
    at: usize,
    /// Where the chroma plane starts, after the luma.
    uv: usize,
    size: (usize, usize),
}

impl Picture {
    /// The picture of a `width`×`height` stream at `at` in `region`, where
    /// the agent said it is: an error for a place that is not one.
    pub fn new(
        region: Arc<Region>,
        at: u64,
        (width, height): (usize, usize),
    ) -> anyhow::Result<Self> {
        let bytes = picture_bytes((width, height));
        let at = usize::try_from(at).ok().filter(|at| {
            at.is_multiple_of(ALIGN) && at.checked_add(bytes).is_some_and(|end| end <= region.len())
        });
        let at = at.with_context(|| {
            format!(
                "it placed a {width}x{height} picture where none fits in the {} bytes of shared memory",
                region.len()
            )
        })?;
        let (_, y, _) = Planes::layout(width as u16, height as u16);
        Ok(Self {
            region,
            at,
            uv: at + y,
            size: (width, height),
        })
    }

    /// Write `mark` where the picture starts, for the agent to look for
    /// there, and fence it.
    pub fn mark(&self, mark: &[u8; MARK]) {
        // SAFETY: the picture is inside the region, and longer than a mark.
        unsafe { std::ptr::copy_nonoverlapping(mark.as_ptr(), self.region.at(self.at), MARK) };
        fence();
    }

    /// The stream's planes, over the picture where it lies, black as
    /// [`Planes::new`]'s are: the daemon converts the desktop into them and
    /// reads none of them, the mapping being write-combined. Made once, as
    /// the stream opens.
    pub fn planes(&self) -> Planes {
        let (width, height) = (self.size.0 as u16, self.size.1 as u16);
        let (_, y, uv) = Planes::layout(width, height);
        let view = |at, bytes| View {
            region: self.region.clone(),
            _place: None,
            at,
            bytes,
        };
        let mut planes = Planes::over(
            width,
            height,
            view(self.at, y).into(),
            view(self.uv, uv).into(),
        );
        planes.y.fill(16);
        planes.uv.fill(128);
        planes
    }

    /// Whether `planes` are this picture's, as [`Self::planes`] made them.
    pub fn holds(&self, planes: &Planes) -> bool {
        planes.y.as_ptr() == self.region.at(self.at).cast_const()
            && planes.uv.as_ptr() == self.region.at(self.uv).cast_const()
    }

    /// The planes were written: put what was written ahead of the frame's
    /// header, which names it, a write-combined mapping's buffers included.
    pub fn written(&self) {
        fence();
    }
}

/// The agent's places in its region, each a stream's picture: which are
/// held, and by whom is their [`Place`]'s to say.
pub struct Places {
    region: Arc<Region>,
    /// The bytes of the region held, in no order.
    held: Mutex<Vec<Range<usize>>>,
}

impl Places {
    /// Every place in `region` free.
    pub fn new(region: Arc<Region>) -> Arc<Self> {
        Arc::new(Self {
            region,
            held: Mutex::default(),
        })
    }

    /// The region the places are in.
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// A place for a `width`×`height` picture, the first free one that fits;
    /// none while the region has no room for one.
    pub fn take(self: &Arc<Self>, size: (usize, usize)) -> Option<Place> {
        let bytes = picture_bytes(size);
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.sort_by_key(|range| range.start);
        let mut at = 0;
        for range in held.iter() {
            if range.start >= at + bytes {
                break;
            }
            at = at.max(range.end);
        }
        if at + bytes > self.region.len() {
            return None;
        }
        held.push(at..at + bytes);
        Some(Place {
            places: self.clone(),
            range: at..at + bytes,
            size,
        })
    }
}

/// A place held by a stream, free again once dropped, which its planes'
/// views are the last of.
pub struct Place {
    places: Arc<Places>,
    range: Range<usize>,
    size: (usize, usize),
}

impl Drop for Place {
    fn drop(&mut self) {
        self.places
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|range| *range != self.range);
    }
}

impl Place {
    /// Where the place starts in the region.
    pub fn at(&self) -> u64 {
        self.range.start as u64
    }

    /// Whether the place starts with `mark`, as the daemon wrote it there.
    pub fn holds(&self, mark: &[u8; MARK]) -> bool {
        let mut there = [0; MARK];
        // SAFETY: the place is inside the region and longer than a mark.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.places.region.at(self.range.start),
                there.as_mut_ptr(),
                MARK,
            );
        }
        there == *mark
    }

    /// The stream's planes, over its picture where it lies: the place is
    /// held for as long as they are.
    pub fn planes(self) -> Planes {
        let (width, height) = (self.size.0 as u16, self.size.1 as u16);
        let (_, y, uv) = Planes::layout(width, height);
        let start = self.range.start;
        let region = self.places.region.clone();
        let place = Arc::new(self);
        let view = |at, bytes| View {
            region: region.clone(),
            _place: Some(place.clone()),
            at,
            bytes,
        };
        Planes::over(
            width,
            height,
            view(start, y).into(),
            view(start + y, uv).into(),
        )
    }
}

/// A plane of a stream's picture, where it lies in the region: the
/// daemon's, or the agent's, which holds its place.
pub struct View {
    region: Arc<Region>,
    /// The agent's place, held for as long as its planes are.
    _place: Option<Arc<Place>>,
    at: usize,
    bytes: usize,
}

impl std::ops::Deref for View {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        // SAFETY: inside the picture, which is inside the region. The daemon
        // writes these bytes only while the agent is not reading them, the
        // link being request and reply, and never again after a failed
        // frame, which leaves them to the agent.
        unsafe { std::slice::from_raw_parts(self.region.at(self.at), self.bytes) }
    }
}

impl std::ops::DerefMut for View {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: as for reading them, and the view is the one way its end
        // has to its plane.
        unsafe { std::slice::from_raw_parts_mut(self.region.at(self.at), self.bytes) }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A file of `bytes` for a test, removed once the test is done with it.
    pub(crate) struct Scratch(pub std::path::PathBuf);

    impl Scratch {
        pub(crate) fn new(name: &str, bytes: u64) -> Self {
            let path = std::env::temp_dir().join(format!(
                "wlshare-shared-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::File::create(&path)
                .unwrap()
                .set_len(bytes)
                .unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Places are given first fit, a page apart, none to two streams at
    /// once, and every one again once its stream lets it go.
    #[test]
    fn a_place_is_one_streams_at_a_time() {
        let bytes = picture_bytes((1920, 1080));
        assert_eq!(bytes % ALIGN, 0);
        let file = Scratch::new("places", (bytes * 3) as u64);
        let places = Places::new(Region::open(&file.0).unwrap());
        let first = places.take((1920, 1080)).unwrap();
        let second = places.take((1920, 1080)).unwrap();
        let third = places.take((1920, 1080)).unwrap();
        assert_eq!(
            [first.at(), second.at(), third.at()],
            [0, bytes as u64, 2 * bytes as u64]
        );
        assert!(places.take((64, 64)).is_none(), "the region is full");
        drop(second);
        // A small one where the middle one was, and no large one beside it.
        let small = places.take((64, 64)).unwrap();
        assert_eq!(small.at(), bytes as u64);
        assert!(places.take((1920, 1080)).is_none());
        // Held for as long as its planes are.
        let planes = third.planes();
        assert!(places.take((1920, 1080)).is_none());
        drop(planes);
        assert!(places.take((1920, 1080)).is_some());
        assert!(places.take((3840, 2160)).is_none(), "past the region");
        drop(first);
    }

    /// The daemon's planes and the agent's are the one picture: black as
    /// the daemon's are made, over the mark the agent found, and every
    /// sample the daemon writes the agent's to read, none of it outside the
    /// picture; and only the agent's planes hold its place.
    #[test]
    fn the_daemons_planes_are_the_agents() {
        let size = (150, 90);
        let file = Scratch::new("pictures", (picture_bytes(size) * 2) as u64);
        let (daemon, agent) = (
            Region::open(&file.0).unwrap(),
            Places::new(Region::open(&file.0).unwrap()),
        );
        let first = agent.take(size).unwrap();
        let place = agent.take(size).unwrap();
        let picture = Picture::new(daemon, place.at(), size).unwrap();
        let mark = [0xC3; MARK];
        assert!(!place.holds(&mark));
        picture.mark(&mark);
        assert!(place.holds(&mark));

        let mut written = picture.planes();
        assert!(
            !place.holds(&mark),
            "the planes are not black over the mark"
        );
        assert!(picture.holds(&written));
        assert!(!picture.holds(&Planes::new(150, 90)));
        let seen = place.planes();
        let black = Planes::new(150, 90);
        assert!(seen.y == black.y && seen.uv == black.uv);
        for (at, sample) in written.y.iter_mut().chain(&mut written.uv).enumerate() {
            *sample = (at % 253) as u8;
        }
        picture.written();
        assert!(seen.y == written.y && seen.uv == written.uv);
        let before = first.planes();
        assert!(
            before.y.iter().chain(&before.uv).all(|sample| *sample == 0),
            "written past the picture"
        );
        drop(seen);
        assert!(
            agent.take(size).is_some(),
            "the daemon's planes hold the place"
        );
        drop(written);
    }

    /// The daemon takes no place the agent could not have given: one off a
    /// page, or past the region's end.
    #[test]
    fn a_place_past_the_region_is_refused() {
        let size = (64, 64);
        let file = Scratch::new("refused", (picture_bytes(size) + ALIGN) as u64);
        let region = Region::open(&file.0).unwrap();
        assert!(Picture::new(region.clone(), 0, size).is_ok());
        assert!(Picture::new(region.clone(), ALIGN as u64, size).is_ok());
        assert!(Picture::new(region.clone(), 2 * ALIGN as u64, size).is_err());
        assert!(Picture::new(region.clone(), 8, size).is_err());
        assert!(Picture::new(region, u64::MAX, size).is_err());
        let empty = Scratch::new("empty", 0);
        let why = Region::open(&empty.0).err().unwrap().to_string();
        assert!(why.contains("empty"), "{why}");
    }
}
