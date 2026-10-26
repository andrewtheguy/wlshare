//! VP9 for a desktop picture, as wlshare codes it.
//!
//! One [`Encoder`], written for a desktop and nothing else: eight bits, at the
//! chroma its maker names ([`Chroma`]) — 4:4:4, VP9 profile 1, or 4:2:0,
//! profile 0 — a quantizer pinned to a 1–100 quality dial rather than a bitrate, no lag, no dropped frames, no keyframe that was not asked
//! for, and the colour matrix and range declared in the bitstream so a decoder
//! does not guess. It is libvpx set up that way. In front of it a [`Picture`]
//! holds the planes it reads — BT.601 at studio swing, its colour at the
//! stream's chroma — converted from packed RGB or the `B, G, R, X` a
//! framebuffer holds. Behind it a [`Decoder`], libvpx's, gives the planes
//! back, for every test here and in the daemon's.
//!
//! The stream's shape is chosen for whoever decodes it. Tile columns follow the
//! picture's width, one under 1440, two from 1440, four from 2048 and eight
//! from 5760, and never outnumber the encoder's threads. A stream in four
//! columns or more is coded without the loop filter, which is what a software
//! decoder's threads wait on there. The measurements are at `TILE_WIDTHS` and
//! `loop_filtered`.
//!
//! A frame need not cost the whole picture. [`Picture::read_bgrx_rect`] converts
//! the rectangle that changed, and [`Encoder::encode`] told where the picture changed
//! skips every block outside it, which a caller that knows its damage says and
//! one that does not leaves unsaid. [`Stream`] is the two together, and what a
//! session holds: it reads a packed picture where that changed
//! ([`Stream::read_bgrx`]), straight from wherever the pixels are, and codes
//! what it read ([`Stream::encode`]). It keeps the rules a frame of damage
//! needs — when a frame is the whole picture anyway, and how coarse the blocks
//! no frame has touched since still are — and the frame that settles them, the
//! whole picture once at the ceiling with the dial left where the link had it
//! ([`Stream::settle`]).
//!
//! Frame metadata is handled here too: [`frame_header`] reads the profile and
//! keyframe bit of a frame this process did not encode, and [`codec_string`]
//! builds the WebCodecs string a browser's `VideoDecoder` is configured with
//! from a stream's size and frame rate. VP9 carries no parameter sets a
//! client could read that string out of.
//!
//! The dial is walked here too: [`walk::QualityWalk`] gives quality up when the
//! link is behind and frame rate once the quality reaches its floor; severe lag
//! can reduce both at once. It takes them back when the link keeps up. What is
//! not here is the rest of *when*: which picture to encode and where the link's
//! lag is read from, which are the daemon's, and how a frame is framed on a
//! wire, which is `wlshare-proto`'s.

mod libvpx;
// Not the codec: the file a daemon run by hand writes of what its encoder was
// handed, and the tools that play it back read.
pub mod capture;
pub mod walk;

pub use libvpx::{Decoded, Decoder};

use thiserror::Error;

/// The coarsest end of the quality dial.
pub const QUALITY_MIN: u8 = 1;
/// The finest end of the quality dial.
pub const QUALITY_MAX: u8 = 100;

/// How a stream samples colour, which is its VP9 profile: a sample per pixel,
/// or one per 2×2 block of them. A session names it for every stream, by who
/// decodes it.
///
/// 4:4:4 is the picture worth having on a desktop: a one-pixel coloured glyph
/// stem shares its one 4:2:0 colour sample with three background pixels and
/// comes back at a quarter of its saturation, which no quantizer recovers. But
/// no browser's hardware VP9 decoder takes profile 1, and a browser with no
/// software VP9 decoder, Safari on iOS and iPadOS, refuses it. 4:2:0 is what
/// every decoder takes, and half the samples to convert and code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    /// 4:4:4, VP9 profile 1: a colour sample per pixel.
    Yuv444,
    /// 4:2:0, VP9 profile 0: a colour sample per 2×2 pixels, the last row and
    /// column of an odd-sized picture each a sample per two pixels or one.
    Yuv420,
}

impl Chroma {
    /// The VP9 profile of a stream at this chroma, eight bits, which is what
    /// [`FrameHeader::profile`] says of one of its frames.
    pub fn profile(self) -> u8 {
        match self {
            Self::Yuv444 => 1,
            Self::Yuv420 => 0,
        }
    }

    /// The chroma at a VP9 `profile`, of the two here.
    pub fn of_profile(profile: u8) -> Option<Self> {
        [Self::Yuv444, Self::Yuv420].into_iter().find(|chroma| chroma.profile() == profile)
    }

    /// The side of the square of pixels one colour sample covers.
    fn block(self) -> usize {
        match self {
            Self::Yuv444 => 1,
            Self::Yuv420 => 2,
        }
    }

    /// The codec string's chroma field: 3 is 4:4:4, and 1 is 4:2:0 with its
    /// samples sited at the luma's, WebM's default, which is what a decoder
    /// assumes of a VP9 4:2:0 picture anyway: the bitstream does not say.
    fn codec_field(self) -> &'static str {
        match self {
            Self::Yuv444 => "03",
            Self::Yuv420 => "01",
        }
    }
}

/// The finest quantizer the dial reaches. VP9's range is 0–63, coarsest last;
/// below about 8 a VP9 desktop is visually lossless and costs several times the
/// bytes to be so, so a dial that mapped past it would have a top third where
/// turning the knob bought nothing but bandwidth. Settled by measurement.
const Q_FINEST: u32 = 8;
/// VP9's coarsest quantizer.
const Q_COARSEST: u32 = 63;

/// The widths from which a picture is coded in two tile columns, in four and
/// in eight, so the frame header's tile columns are the log2 of that: one column under
/// 1440 pixels, two from 1440 through 1080p, four from 2048 and at 4K. The
/// width rather than the thread count decides it because a tile is a cost as
/// well as a split — the columns are coded apart, which costs bytes, and at
/// 1080p four of them coded slower than two whatever the threads — while at 4K
/// four were worth having. Two at 1440 are what a decoder's threads have to
/// split: libvpx's own decoder, which is the browser's, parses and reconstructs
/// a one-column frame on one thread and gives the others only the loop filter,
/// and vp9-wasm, which reconstructs by rows whatever the tiles, still parses a
/// column in order. A 1440×900 desktop re-encoded with two columns cost 8% more
/// bytes and vp9-wasm decoded it 10% faster on four threads; a busy Mac screen
/// of that size cost 0.1% more and decoded 21% faster. Four from 2048 and not
/// 2880 were measured on the zDesk gateway's Mac captures at 2048×1536 and
/// 2560×1600, at quality 90 on four threads, on the 300 quietest frames of each
/// and the 300 busiest: four columns cost 0.1% more bytes than two on the quiet
/// frames and 1.4% and 1.0% on the busy, under 0.1 dB, libvpx's encoder coded
/// them 9% to 14% faster, and libvpx's four decoding threads, with a column each,
/// took 30.4 ms a busy frame for 51.9 and 30.9 for 54.3. Not from 1920: the
/// busy frames of a 1920×1080 Windows capture cost 15% more bytes in four.
/// Never more than the
/// threads can fill, since a tile no thread is free for is bytes for nothing.
const TILE_WIDTHS: [u16; 3] = [1440, 2048, 5760];

/// The most threads an encoder takes.
const MAX_THREADS: usize = 64;

/// How many threads the encoder gets: the machine less two cores, at most
/// `MAX_ENCODER_THREADS` (8). An encode is a burst of milliseconds a frame that
/// the person at the other end is waiting on, and the encoder splits it
/// across threads by rows and tile columns; the two cores kept back are for
/// the compositor and the session, which are what make the next frame. The
/// count also bounds the stream's tile columns, and so what a decoder's
/// threads have to split. The daemon's software H.264 encoder takes the same
/// count, so a region costs the machine's cores alike in either codec.
pub fn encoder_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(2)).clamp(1, MAX_ENCODER_THREADS)
}

/// The most threads the encoder takes, whatever the machine: what a 4K
/// picture can use. The useful count is the picture's, not the machine's —
/// superblock rows are handed out within each tile column, so the work to
/// share grows with the picture — at about one thread a megapixel: eight for
/// 4K's 8.3. 4K is the largest desktop this is tuned for; a larger one is an
/// edge case that streams, not a target, and gets the 4K count.
const MAX_ENCODER_THREADS: usize = 8;


/// The 1–100 quality dial as a VP9 quantizer: [`QUALITY_MIN`] is the coarsest
/// picture and becomes [`Q_COARSEST`], [`QUALITY_MAX`] the finest and becomes
/// [`Q_FINEST`]. Out-of-range input is clamped rather than refused; a
/// configuration is what rejects a bad dial.
fn quality_to_q(quality: u8) -> u32 {
    let quality = u32::from(quality.clamp(QUALITY_MIN, QUALITY_MAX));
    Q_COARSEST - (quality - 1) * (Q_COARSEST - Q_FINEST) / 99
}

/// The log2 of the tile columns a `width`-wide picture is coded in by `threads`.
fn tile_columns_log2(width: u16, threads: usize) -> u32 {
    (TILE_WIDTHS.iter().filter(|&&from| width >= from).count() as u32).min(threads.max(1).ilog2())
}

/// Whether a stream's frames are loop filtered: all but those in four tile
/// columns or more, `tile_cols_log2` being the log2 of the columns the frames
/// are in, the ones the format forces on a very wide picture among them. The filter's level is taken from the quantizer, as libvpx's
/// encoder takes it, 7 to 9 at the fine end of the dial, a light filter that
/// still visits every block edge of every frame, and it is a quarter to two
/// fifths of what vp9-wasm spends on a frame. Measured on vp9-wasm 0.0.5 with
/// what the zDesk gateway wrote of one four-pane desktop at eight sizes,
/// 1280×800 to 3840×2160, each coded both ways at quality 90: without the
/// filter one thread decodes in 29% to 41% fewer cycles, and the stream is 3%
/// to 6% larger and 0.6 to 1.4 dB of luma further from its source, since every
/// later frame predicts from what the filter smoothed. What four threads gain
/// is what the tiles leave. In one or two columns a frame waits on their
/// parsing: 7.2 ms for 7.5 at 1280 wide, 5.7 for 5.8 at 1440, 6.1 for 6.4 at
/// 1600, 9.8 for 9.0 at 1920, 9.5 for 10.3 at 2560. In four: 6.4 for 10.4 at
/// 2880, 14.2 for 21.3 at 3456, 12.1 for 17.4 at 3840. An earlier 3456-wide
/// desktop coded in two columns: 27.4 for 29.0, and the 2560-wide capture in
/// four: 6.1 for 9.0, a 2048-wide one: 5.3 for 7.8. Those are each capture's
/// first 300 frames, its quietest; on its 300 busiest the filter is less of a
/// frame and of the stream, 11% to 16% of one thread's cycles for 0.2% to 1.3%
/// of the bytes and 0.3 to 0.6 dB, and four threads gain 14% to 27% in four
/// columns and nothing to speak of in one or two. So the filter goes where the
/// columns have left it as what a
/// decoder's threads wait on, which is also where a frame is slowest to decode,
/// and stays where it would be bytes and picture given for no time.
fn loop_filtered(tile_cols_log2: u32) -> bool {
    tile_cols_log2 < 2
}

/// Why a picture could not be encoded or a frame decoded.
#[derive(Debug, Error)]
pub enum Error {
    /// libvpx refused a call, in its own words.
    #[error("vp9 {call}: {detail}")]
    Codec { call: &'static str, detail: String },
    /// libvpx took the picture and made no frame of it: nothing was appended.
    /// With no lag and no dropped frames it should not happen, and it is an
    /// error and not an assertion because a caller can carry on from it, the
    /// pixels being the next frame's to carry.
    #[error("libvpx made no frame of the picture")]
    NoFrame,
    #[error("a {0}x{1} picture has no pixels")]
    Empty(u16, u16),
    #[error("an encoder for {0} threads: it takes 1 to {MAX_THREADS}")]
    Threads(usize),
    #[error("a crop of {0} bytes is not a {1}x{2} picture")]
    Crop(usize, usize, usize),
    #[error("a {width}x{height} picture at a stride of {stride} does not fit in {bytes} bytes")]
    Buffer { width: usize, height: usize, stride: usize, bytes: usize },
    #[error("a {0}x{1} encoder was handed a {2}x{3} picture")]
    Mismatch(u16, u16, u16, u16),
    #[error("a {0:?} encoder was handed a {1:?} picture")]
    Sampling(Chroma, Chroma),
    #[error("a stream was asked for a frame before any picture was read into it")]
    Unread,
    #[error("the frame decoded to no picture")]
    NoPicture,
    #[error("the frame is not 8-bit 4:4:4 or 4:2:0 (format {0}, {1} bits)")]
    Format(u32, u32),
}

/// Whether `bytes` hold a `width`×`height` picture of 4-byte pixels whose rows
/// are `stride` apart, in measures the conversions' `u32` can carry. A last row
/// with nothing after it is a picture that fits.
fn fits(width: usize, height: usize, stride: usize, bytes: usize) -> bool {
    if [width, height, stride].iter().any(|&n| u32::try_from(n).is_err()) {
        return false;
    }
    width == 0 || height == 0 || (stride >= width * 4 && bytes >= (height - 1) * stride + width * 4)
}

/// A rectangle of a picture, in pixels: where it differs from the picture
/// encoded before it, for [`Encoder::encode`]. What lies past the picture's
/// edge is not part of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// One picture as the planes the encoder reads: 8-bit Y, U and V, BT.601 at
/// studio swing, a luma sample per pixel and the colour at the picture's
/// [`Chroma`]. Reused across frames, so a 1080p conversion is not a 6 MB
/// allocation apiece.
pub struct Picture {
    size: (u16, u16),
    chroma: Chroma,
    /// Each plane with a row of nothing after its last: the conversion writes a
    /// row as a whole stride from where it starts, and a rectangle's rows start
    /// at its left edge, so its last row's stride ends up to a row past the
    /// plane.
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    /// A tight copy of the last row of a packed picture where it is not
    /// followed by a whole stride, made only for that: the conversion reads a
    /// row as a whole stride too.
    tight: Vec<u8>,
    /// How many threads a conversion is split across.
    threads: usize,
}

/// The fewest pixels a conversion gives a thread of its own, and the most
/// threads it is split across. A conversion waits on memory more than on a
/// core: a 4K picture's 33 MB of `B, G, R, X` took 5.5 ms on one thread, 3.3 on
/// two, 2.9 on three and on four and 3.0 on six, and a 1080p one 1.5, 1.0, 0.8,
/// 0.9 and 1.0. A thread also costs some 50 µs to start, which is what half a
/// megapixel's share of the gain pays for and less does not.
const THREAD_PIXELS: usize = 1 << 19;
const CONVERT_THREADS: usize = 4;

impl Picture {
    /// A buffer for a `width`×`height` picture at `chroma`, converted on up
    /// to `threads` threads, which are the encoder's: it is idle while its
    /// picture is read.
    pub fn new(width: u16, height: u16, chroma: Chroma, threads: usize) -> Result<Self, Error> {
        if width == 0 || height == 0 {
            return Err(Error::Empty(width, height));
        }
        let (w, h) = (usize::from(width), usize::from(height));
        let (cw, ch) = (w.div_ceil(chroma.block()), h.div_ceil(chroma.block()));
        Ok(Self { size: (width, height), chroma, y: vec![0; w * (h + 1)], u: vec![0; cw * (ch + 1)], v: vec![0; cw * (ch + 1)], tight: Vec::new(), threads })
    }

    /// The picture's size.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// How the picture's colour is sampled.
    pub fn chroma(&self) -> Chroma {
        self.chroma
    }

    /// The size of a chroma plane.
    fn chroma_size(&self) -> (usize, usize) {
        let block = self.chroma.block();
        (usize::from(self.size.0).div_ceil(block), usize::from(self.size.1).div_ceil(block))
    }

    /// Y, U and V, each tight at its plane's width.
    pub fn planes(&self) -> [&[u8]; 3] {
        let len = usize::from(self.size.0) * usize::from(self.size.1);
        let (cw, ch) = self.chroma_size();
        [&self.y[..len], &self.u[..cw * ch], &self.v[..cw * ch]]
    }

    /// The width of a row of each plane.
    pub fn strides(&self) -> [usize; 3] {
        let cw = self.chroma_size().0;
        [usize::from(self.size.0), cw, cw]
    }

    /// The whole picture as a rectangle.
    fn whole(&self) -> Rect {
        Rect { x: 0, y: 0, width: self.size.0, height: self.size.1 }
    }

    /// Convert `rgb` — packed RGB888, tight, for exactly this picture — in place.
    ///
    /// The length is checked rather than trusted, because everything after the
    /// check indexes by the picture size.
    pub fn read_rgb(&mut self, rgb: &[u8]) -> Result<(), Error> {
        self.read_rgb_rect(rgb, self.whole())
    }

    /// Convert `rect` of `rgb`, which holds the whole picture as
    /// [`Self::read_rgb`] takes it, and leave every other pixel as the last
    /// conversion made it, as [`Self::read_bgrx_rect`] does, 4:2:0's whole
    /// blocks included.
    pub fn read_rgb_rect(&mut self, rgb: &[u8], rect: Rect) -> Result<(), Error> {
        self.holds_rgb(rgb)?;
        self.convert(rgb, usize::from(self.size.0) * 3, rect, Packed::Rgb);
        Ok(())
    }

    /// Whether `rgb` is this picture, packed and tight.
    fn holds_rgb(&self, rgb: &[u8]) -> Result<(), Error> {
        let (w, h) = (usize::from(self.size.0), usize::from(self.size.1));
        if rgb.len() != w * h * 3 {
            return Err(Error::Crop(rgb.len(), w, h));
        }
        Ok(())
    }

    /// Whether `pixels` hold this picture as `B, G, R, X`, rows `stride`
    /// bytes apart.
    fn holds_bgrx(&self, pixels: &[u8], stride: usize) -> Result<(), Error> {
        let (w, h) = (usize::from(self.size.0), usize::from(self.size.1));
        if !fits(w, h, stride, pixels.len()) {
            return Err(Error::Buffer { width: w, height: h, stride, bytes: pixels.len() });
        }
        Ok(())
    }

    /// Convert `pixels` — `B, G, R, X`, this picture's size, rows `stride` bytes
    /// apart — in place. The X byte is ignored.
    pub fn read_bgrx(&mut self, pixels: &[u8], stride: usize) -> Result<(), Error> {
        self.read_bgrx_rect(pixels, stride, self.whole())
    }

    /// Convert `rect` of `pixels`, which hold the whole picture as
    /// [`Self::read_bgrx`] takes it, and leave every other pixel as the last
    /// conversion made it: what a picture that changed in one place costs is
    /// that place and not the screen, nor the rows across it. What of `rect`
    /// lies past the picture's edge is not part of it.
    ///
    /// At 4:2:0 `rect` is read as the 2×2 blocks it touches, since a block's
    /// one colour sample is all four of its pixels': `pixels` must hold the
    /// picture there too, and every other pixel is left as it was.
    pub fn read_bgrx_rect(&mut self, pixels: &[u8], stride: usize, rect: Rect) -> Result<(), Error> {
        self.holds_bgrx(pixels, stride)?;
        self.convert(pixels, stride, rect, Packed::Bgrx);
        Ok(())
    }

    /// Convert `rect` of `pixels`, a whole picture in `packed` order that its
    /// caller has checked fits, rows `stride` bytes apart.
    ///
    /// At 4:2:0 the rectangle is widened to the whole 2×2 blocks it touches,
    /// whose one colour sample each is taken from all four of their pixels.
    fn convert(&mut self, pixels: &[u8], stride: usize, rect: Rect, packed: Packed) {
        let (w, h) = (usize::from(self.size.0), usize::from(self.size.1));
        let block = self.chroma.block();
        let (x, y) = (usize::from(rect.x), usize::from(rect.y));
        let (left, right) = (x - x % block, (x + usize::from(rect.width)).next_multiple_of(block).min(w));
        let (top, bottom) = (y - y % block, (y + usize::from(rect.height)).next_multiple_of(block).min(h));
        if left >= right || top >= bottom {
            return;
        }
        // The crate reads a row as a whole stride from its first pixel, which
        // here is the rectangle's left edge, and at 4:2:0 two rows at a time.
        // The picture's last row has only what is left of its own after that,
        // and in a buffer that ends with the picture not always that much, so
        // the rows that end the rectangle, a block's, are copied tight first
        // where the buffer does not go on a stride past them. The rows above
        // have those under them to read into.
        let bytes = packed.bytes();
        let at = |row: usize| row * stride + left * bytes;
        let tail = (bottom - 1) % block + 1;
        let borrowed = if at(bottom) <= pixels.len() { bottom } else { bottom - tail };
        self.convert_rows(&pixels[at(top)..at(borrowed)], stride, left..right, top..borrowed, packed);
        if borrowed < bottom {
            let row = (right - left) * bytes;
            let mut tight = std::mem::take(&mut self.tight);
            tight.clear();
            for line in borrowed..bottom {
                tight.extend_from_slice(&pixels[at(line)..][..row]);
            }
            self.convert_rows(&tight, row, left..right, borrowed..bottom, packed);
            self.tight = tight;
        }
    }

    /// Convert `cols` of `rows` from `pixels`, which begin at the first of
    /// them and give every row a whole `stride`, on as many threads as the
    /// pixels are worth. At 4:2:0 both start on a block's first pixel, and
    /// end on a block's last or the picture's.
    fn convert_rows(&mut self, pixels: &[u8], stride: usize, cols: std::ops::Range<usize>, rows: std::ops::Range<usize>, packed: Packed) {
        if rows.is_empty() {
            return;
        }
        let (w, chroma, block) = (usize::from(self.size.0), self.chroma, self.chroma.block());
        let cw = self.chroma_size().0;
        let width = cols.len();
        let threads = self.threads.min(CONVERT_THREADS).min(width * rows.len() / THREAD_PIXELS).max(1);
        // A band is whole blocks, so that no colour sample is two threads'.
        let band = rows.len().div_ceil(threads).next_multiple_of(block);
        let span = rows.start * w + cols.start..rows.end * w + cols.start;
        // The chroma planes' rows of these: exactly as many as the rows have
        // samples, since the crate takes the last of them for an odd row.
        let start = rows.start / block * cw + cols.start / block;
        let chroma_span = start..start + rows.len().div_ceil(block) * cw;
        let (mut y, mut u, mut v) = (&mut self.y[span], &mut self.u[chroma_span.clone()], &mut self.v[chroma_span]);
        let mut pixels = pixels;
        std::thread::scope(|scope| {
            let mut left = rows.len();
            while left > 0 {
                let height = band.min(left);
                left -= height;
                let (band_y, band_u, band_v, band_pixels);
                (band_y, y) = y.split_at_mut(height * w);
                (band_u, u) = u.split_at_mut(height.div_ceil(block) * cw);
                (band_v, v) = v.split_at_mut(height.div_ceil(block) * cw);
                (band_pixels, pixels) = pixels.split_at(height * stride);
                let planes = [band_y, band_u, band_v];
                // The last band is this thread's own.
                if left > 0 {
                    scope.spawn(move || convert_band(band_pixels, stride, packed, chroma, planes, [w, cw], (width, height)));
                } else {
                    convert_band(band_pixels, stride, packed, chroma, planes, [w, cw], (width, height));
                }
            }
        });
    }
}

/// Convert `size` pixels from `pixels`, rows `stride` bytes apart, into the
/// `planes` Y, U and V at `chroma`, whose rows are `plane_strides` apart, the
/// luma's and the chroma's. Every row of the pixels and of the planes is a
/// whole stride.
#[allow(clippy::too_many_arguments)]
fn convert_band(pixels: &[u8], stride: usize, packed: Packed, chroma: Chroma, planes: [&mut [u8]; 3], plane_strides: [usize; 2], size: (usize, usize)) {
    use yuv::{YuvConversionMode, YuvRange, YuvStandardMatrix};
    let [y, u, v] = planes;
    let [luma_stride, chroma_stride] = plane_strides;
    let mut image = yuv::YuvPlanarImageMut {
        y_plane: yuv::BufferStoreMut::Borrowed(y),
        y_stride: luma_stride as u32,
        u_plane: yuv::BufferStoreMut::Borrowed(u),
        u_stride: chroma_stride as u32,
        v_plane: yuv::BufferStoreMut::Borrowed(v),
        v_stride: chroma_stride as u32,
        width: size.0 as u32,
        height: size.1 as u32,
    };
    // Professional: 15 bits a coefficient, and the one mode whose every
    // path makes the same planes, so that a picture is the same stream on
    // arm64 and amd64. Balanced's NEON made a luma one under its AVX2's in
    // two colours of five, and each path's chroma differed by one in a
    // sixth; this is within 0.51 of a code value of BT.601's for every colour.
    // On an M2 Max it is 1.6 times Balanced's time on one thread, 0.77 ms a
    // 1920×1080 against 0.49, and on four, as a whole picture is converted,
    // 0.23 against 0.18 and 0.97 a 3840×2160 against 0.96, which waits on
    // memory.
    let (range, matrix, mode) = (YuvRange::Limited, YuvStandardMatrix::Bt601, YuvConversionMode::Professional);
    match (packed, chroma) {
        (Packed::Rgb, Chroma::Yuv444) => yuv::rgb_to_yuv444(&mut image, pixels, stride as u32, range, matrix, mode),
        (Packed::Bgrx, Chroma::Yuv444) => yuv::bgra_to_yuv444(&mut image, pixels, stride as u32, range, matrix, mode),
        (Packed::Rgb, Chroma::Yuv420) => yuv::rgb_to_yuv420(&mut image, pixels, stride as u32, range, matrix, mode),
        (Packed::Bgrx, Chroma::Yuv420) => yuv::bgra_to_yuv420(&mut image, pixels, stride as u32, range, matrix, mode),
    }
    .expect("the picture fits its buffer, which was checked");
}

/// The byte order of a packed picture a [`Picture`] is read from.
#[derive(Clone, Copy)]
enum Packed {
    Rgb,
    Bgrx,
}

impl Packed {
    /// The bytes of one pixel.
    fn bytes(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Bgrx => 4,
        }
    }
}

/// One VP9 stream at one picture size, coded by libvpx. A picture of another
/// size needs another encoder, whose first frame is a keyframe by
/// construction. Within one encoder, every inter frame is expressed as a
/// change from earlier frames, which is what makes an inter-frame stream mean
/// anything.
///
/// Boxed: libvpx's holds its configuration and image in itself, hundreds of
/// bytes, and a session's task carries it from one of the runtime's threads
/// to another.
pub struct Encoder(Box<libvpx::Encoder>);

impl Encoder {
    /// An encoder for a `width`×`height` picture at `chroma`, starting at
    /// `quality` (1–100, clamped), coded on `threads` threads. The
    /// thread count is the caller's: how many cores a machine can spare is a
    /// question about what else it runs. It also bounds the tile columns, and
    /// so decides with the width whether the stream is loop filtered: from
    /// 2048 wide on four threads or more it is not.
    pub fn new(width: u16, height: u16, quality: u8, chroma: Chroma, threads: usize) -> Result<Self, Error> {
        Ok(Self(Box::new(libvpx::Encoder::new(width, height, quality, chroma, threads)?)))
    }

    /// How this encoder's stream samples colour.
    pub fn chroma(&self) -> Chroma {
        self.0.chroma()
    }

    /// The picture size this encoder codes.
    pub fn size(&self) -> (u16, u16) {
        self.0.size()
    }

    /// The dial this encoder is coding at.
    pub fn quality(&self) -> u8 {
        self.0.quality()
    }

    /// Move the dial, without a keyframe: the next frame is coded at the new
    /// quantizer against the frames before it. This is how a link that is
    /// behind gives quality up, and "without a keyframe" is the whole reason
    /// it is written this way rather than by rebuilding the encoder, which
    /// would force a keyframe — the most bytes a frame can be — at the exact
    /// moment the link has run out of room. It cannot fail: a libvpx that
    /// refuses the dial is the error of the frame that was to be coded at it.
    pub fn set_quality(&mut self, quality: u8) {
        self.0.set_quality(quality);
    }

    /// The coarsest dial any block of the picture a decoder now holds was last
    /// coded at: [`Self::quality`] as each frame was encoded, for the blocks
    /// that frame coded. A frame told where its picture changed leaves the
    /// rest at the dial they had, so a picture the dial was walked down for is
    /// coarse here until every block of it has been coded finer, which a whole
    /// frame does at once — what a caller that settles a quiet picture asks
    /// before deciding it owes one. Before the first frame, the dial.
    pub fn coarsest(&self) -> u8 {
        self.0.coarsest()
    }

    /// Settle `picture` at `quality`: encode it whole and at that dial, and
    /// leave the dial where it was, for the frames after it. Returns whether
    /// the frame is a keyframe, as [`Self::encode`] does.
    pub fn settle(&mut self, picture: &Picture, quality: u8, keyframe: bool, out: &mut Vec<u8>) -> Result<bool, Error> {
        self.0.settle(picture, quality, keyframe, out)
    }

    /// Encode `picture` and append the frame to `out`. `keyframe` makes it one a
    /// decoder can start from; the first frame is one either way.
    ///
    /// `changed` is where the picture differs from the one encoded before it,
    /// or `None` for a picture that may differ anywhere. Blocks outside every
    /// rectangle are skipped — not searched, and left in the decoder's picture
    /// as they are — which is most of what a frame costs to encode when little
    /// moved. It is the caller's word and not checked: a change outside the
    /// rectangles is not coded, by this frame or by a later one that does not
    /// name it. A skipped block also keeps the quantizer it was last coded at,
    /// so the frame that is to sharpen an unchanged picture after
    /// [`Self::set_quality`] passes `None`. A keyframe codes everything
    /// whatever `changed` says.
    ///
    /// Returns whether the frame is a keyframe. On an error nothing is
    /// appended; libvpx making no frame of the picture is one
    /// ([`Error::NoFrame`]).
    pub fn encode(&mut self, picture: &Picture, keyframe: bool, changed: Option<&[Rect]>, out: &mut Vec<u8>) -> Result<bool, Error> {
        self.0.encode(picture, keyframe, changed, out)
    }
}

/// A desktop's stream from the pixels it is shown as: an [`Encoder`] and the
/// [`Picture`] in front of it, which is read from a packed picture where that
/// changed and then coded. What a session would otherwise keep itself: the rectangles that changed converted and the blocks they touch coded,
/// and the whole picture read and coded where there is nothing for it to be a
/// change to.
///
/// Reading and coding are two calls because they are not one moment to a
/// session: the pixels may be behind a lock the compositor waits on, which a
/// conversion is held for and an encode is not, and read straight from there
/// they are copied nowhere on the way.
pub struct Stream {
    encoder: Encoder,
    picture: Picture,
    /// The planes hold a picture: one was read whole. Until then there is
    /// nothing for rectangles to be read over, nor anything to code.
    filled: bool,
    /// What of the planes no decoder holds, which the next frame codes.
    owed: Owed,
    /// The rectangles read since the last frame, while that is all that is
    /// owed.
    changed: Vec<Rect>,
}

/// What of a [`Stream`]'s planes its decoder is yet to be sent.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Owed {
    /// Nothing: the last frame carried the planes as they stand.
    Nothing,
    /// The rectangles read since the last frame.
    Changed,
    /// All of it: the picture was read whole, or a frame of it came to
    /// nothing.
    Whole,
}

impl Stream {
    /// A stream of `width`×`height` pictures at `chroma`, as [`Encoder::new`]
    /// takes them.
    pub fn new(width: u16, height: u16, quality: u8, chroma: Chroma, threads: usize) -> Result<Self, Error> {
        Ok(Self {
            encoder: Encoder::new(width, height, quality, chroma, threads)?,
            picture: Picture::new(width, height, chroma, threads)?,
            filled: false,
            owed: Owed::Whole,
            changed: Vec::new(),
        })
    }

    /// How this stream samples colour.
    pub fn chroma(&self) -> Chroma {
        self.encoder.chroma()
    }

    /// The picture size this stream codes.
    pub fn size(&self) -> (u16, u16) {
        self.encoder.size()
    }

    /// The dial this stream is coding at.
    pub fn quality(&self) -> u8 {
        self.encoder.quality()
    }

    /// Move the dial without a keyframe, as [`Encoder::set_quality`] does.
    pub fn set_quality(&mut self, quality: u8) {
        self.encoder.set_quality(quality);
    }

    /// The coarsest dial any block of the decoder's picture was last coded at
    /// ([`Encoder::coarsest`]).
    pub fn coarsest(&self) -> u8 {
        self.encoder.coarsest()
    }

    /// Read `pixels` — `B, G, R, X`, this stream's size, rows `stride` bytes
    /// apart — as the picture the next frame carries ([`Self::encode`]).
    ///
    /// `changed` is where the picture differs from the one read before it, or
    /// `None` for one that may differ anywhere, a keyframe's included: the
    /// caller's word and not checked. With it only the rectangles are read —
    /// at 4:2:0 the 2×2 blocks they touch, which never reach past the blocks
    /// a frame codes ([`Picture::read_bgrx_rect`]) — and the next frame codes
    /// only the blocks they touch, and those of the reads before it that no
    /// frame has carried: the rest of `pixels` may be whatever it is. The
    /// stream's first read is the whole picture whatever `changed` says, and
    /// reads all of `pixels`.
    ///
    /// Nothing of `pixels` is kept: it is the planes that are coded, so the
    /// frame may be a while after the read, and the pixels changed by then.
    pub fn read_bgrx(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> Result<(), Error> {
        match changed.filter(|_| self.filled) {
            Some(rects) => {
                // Refused whole or not at all, a read of no rectangle too.
                self.picture.holds_bgrx(pixels, stride)?;
                for rect in rects {
                    self.picture.read_bgrx_rect(pixels, stride, *rect)?;
                }
                self.read(rects);
            }
            None => {
                self.picture.read_bgrx(pixels, stride)?;
                self.read_whole();
            }
        }
        Ok(())
    }

    /// Read `rgb` — packed RGB888, tight, for exactly this stream's picture —
    /// as [`Self::read_bgrx`] does `B, G, R, X`.
    pub fn read_rgb(&mut self, rgb: &[u8], changed: Option<&[Rect]>) -> Result<(), Error> {
        match changed.filter(|_| self.filled) {
            Some(rects) => {
                self.picture.holds_rgb(rgb)?;
                for rect in rects {
                    self.picture.read_rgb_rect(rgb, *rect)?;
                }
                self.read(rects);
            }
            None => {
                self.picture.read_rgb(rgb)?;
                self.read_whole();
            }
        }
        Ok(())
    }

    /// The planes were read at `rects`, which the next frame owes with what
    /// it owed already.
    fn read(&mut self, rects: &[Rect]) {
        match self.owed {
            Owed::Whole => {}
            Owed::Changed => self.changed.extend_from_slice(rects),
            Owed::Nothing => {
                self.changed.clear();
                self.changed.extend_from_slice(rects);
                self.owed = Owed::Changed;
            }
        }
    }

    /// The planes were read whole, and the next frame owes all of them.
    fn read_whole(&mut self) {
        self.filled = true;
        self.owed = Owed::Whole;
    }

    /// Encode the picture as it was last read and append the frame to `out`,
    /// returning what [`Encoder::encode`] does. The frame codes what was read
    /// since the frame before it: the blocks those rectangles touch, the whole
    /// picture if any read was whole, and no block at all if nothing was
    /// read, which is a frame all the same. `keyframe` makes it one a decoder
    /// can start from, which codes everything; the stream's first frame is
    /// one either way.
    ///
    /// A frame that failed, libvpx making nothing of it among them
    /// ([`Error::NoFrame`]), leaves the whole picture owed to the next, since
    /// no decoder was handed what this one coded. A stream nothing was ever
    /// read into has no picture to code, and says so ([`Error::Unread`]).
    pub fn encode(&mut self, keyframe: bool, out: &mut Vec<u8>) -> Result<bool, Error> {
        if !self.filled {
            return Err(Error::Unread);
        }
        let changed = match self.owed {
            Owed::Nothing => Some(&[][..]),
            Owed::Changed => Some(&self.changed[..]),
            Owed::Whole => None,
        };
        // Until a frame comes out the planes are ahead of every decoder.
        self.owed = Owed::Whole;
        let key = self.encoder.encode(&self.picture, keyframe, changed, out)?;
        self.owed = Owed::Nothing;
        Ok(key)
    }

    /// Settle the picture at `quality`: encode it as it was last read, whole
    /// and at that dial, and leave the dial where it was
    /// ([`Encoder::settle`]). What a desktop that went quiet while the link
    /// had it coarse is owed ([`walk::QualityWalk::settle_at`]): as an inter
    /// frame the unchanged picture costs no keyframe and sharpens every block,
    /// so [`Self::coarsest`] is `quality` after it, and the frames of the
    /// motion that follows are at what the link bears again. `keyframe` makes
    /// it one a decoder can start from, at `quality` all the same. Nothing
    /// need be read for it: a quiet desktop is the picture the planes hold,
    /// and one that changed since is read where it did ([`Self::read_bgrx`])
    /// first.
    pub fn settle(&mut self, quality: u8, keyframe: bool, out: &mut Vec<u8>) -> Result<bool, Error> {
        if !self.filled {
            return Err(Error::Unread);
        }
        self.owed = Owed::Whole;
        let key = self.encoder.settle(&self.picture, quality, keyframe, out)?;
        self.owed = Owed::Nothing;
        Ok(key)
    }
}

/// What the opening bits of a VP9 frame say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// The VP9 profile, 0–3. Streams encoded here are their chroma's, 1 for
    /// 4:4:4 and 0 for 4:2:0 ([`Chroma::profile`]).
    pub profile: u8,
    /// Whether a decoder that has seen nothing before this frame can start here.
    pub keyframe: bool,
}

/// Read the first fields of a frame's uncompressed header (VP9 bitstream §6.2):
/// `frame_marker` (two bits, always 2), `profile_low_bit`, `profile_high_bit`, a
/// reserved zero bit on profile 3, `show_existing_frame`, and `frame_type`,
/// where 0 is a keyframe. A frame that only shows an earlier one is not a
/// keyframe.
///
/// For a stream this process did not encode, whose keyframe bit is therefore
/// not the encoder's to report. `None` for bytes that do not start a VP9 frame.
pub fn frame_header(frame: &[u8]) -> Option<FrameHeader> {
    let mut bits = frame.iter().take(2).flat_map(|byte| (0..8).rev().map(move |i| byte >> i & 1));
    let mut bit = || bits.next();
    if (bit()? << 1 | bit()?) != 2 {
        return None;
    }
    let low = bit()?;
    let profile = bit()? << 1 | low;
    if profile == 3 && bit()? != 0 {
        return None;
    }
    let show_existing = bit()? == 1;
    let keyframe = !show_existing && bit()? == 0;
    Some(FrameHeader { profile, keyframe })
}

/// VP9's levels, as `(level, max_luma_sample_rate, max_luma_picture_size,
/// max_luma_breadth)`.
///
/// Transcribed from `vp9_level_defs[]` in libvpx's own
/// `vp9/encoder/vp9_encoder.c` at the commit `libvpx-prebuilt` pins, and not
/// from memory: the table is the normative one, the rows are not evenly spaced,
/// and 4K30 lands on level 5.0 rather than the 5.1 a plausible guess gives. Only
/// the three fields a picture size decides are kept. A level also caps the
/// bitrate, the buffer size and the compression ratio, and a pinned quantizer
/// holds none of those: a busy frame costs what it costs, and it is the quality
/// walk behind the encoder, not a level, that keeps the stream inside the link.
/// So the level announced is the picture's, which is what a browser reads it
/// for: it picks a decoder by profile and size, and it refuses a string whose
/// level is not one of the fourteen — Chromium's parser rejects the "undefined"
/// 0 the file-format binding allows — so a level must be named.
///
/// The lowest row a picture fits is the level announced, which matters because
/// a level is a ceiling: a decoder that accepts 5.0 accepts every stream below
/// it, so the smallest level the picture fits is the widest claim to make.
const LEVELS: [(u8, u64, u32, u16); 14] = [
    (10, 829_440, 36_864, 512),
    (11, 2_764_800, 73_728, 768),
    (20, 4_608_000, 122_880, 960),
    (21, 9_216_000, 245_760, 1_344),
    (30, 20_736_000, 552_960, 2_048),
    (31, 36_864_000, 983_040, 2_752),
    (40, 83_558_400, 2_228_224, 4_160),
    (41, 160_432_128, 2_228_224, 4_160),
    (50, 311_951_360, 8_912_896, 8_384),
    (51, 588_251_136, 8_912_896, 8_384),
    (52, 1_176_502_272, 8_912_896, 8_384),
    (60, 1_176_502_272, 35_651_584, 16_832),
    (61, 2_353_004_544, 35_651_584, 16_832),
    (62, 4_706_009_088, 35_651_584, 16_832),
];

/// The WebCodecs codec string for a `w`×`h` stream of this crate's at
/// `chroma`, every field of it:
/// `vp09.<profile>.<level>.<depth>.<chroma>.<primaries>.<transfer>.<matrix>.<range>`.
///
/// The chroma's profile, eight bits and the chroma, because that is what the
/// encoder produces. The optional fields are spelled out rather than left to
/// their defaults because the defaults are wrong for this stream: an omitted
/// chroma field means 4:2:0, which Chromium reads as 4:2:2 on a profile 1
/// string since 4:2:0 is not a profile 1 picture, and omitted colour fields
/// mean BT.709 where the keyframe header says BT.601 (SMPTE 170M primaries,
/// transfer and matrix — code 6 each, what Chromium's own VP9 parser maps that
/// header flag to) at studio swing. The level comes from `LEVELS` at `fps`
/// frames a second. `None` for a picture no VP9 level covers.
pub fn codec_string(w: u16, h: u16, fps: u64, chroma: Chroma) -> Option<String> {
    let size = u32::from(w) * u32::from(h);
    let rate = u64::from(size) * fps;
    let breadth = w.max(h);
    let (level, ..) = LEVELS.iter().find(|(_, max_rate, max_size, max_breadth)| rate <= *max_rate && size <= *max_size && breadth <= *max_breadth).copied()?;
    Some(format!("vp09.{:02}.{level:02}.08.{}.06.06.06.00", chroma.profile(), chroma.codec_field()))
}

// The crate's own tests: what a stream does by its calls. libvpx's are beside
// it (`libvpx::tests`).
#[cfg(test)]
mod tests {
    use super::*;

    /// Both chromas, which a test of what they share runs at each.
    const CHROMAS: [Chroma; 2] = [Chroma::Yuv444, Chroma::Yuv420];

    /// `w`×`h` of one colour, packed RGB.
    fn flat(w: u16, h: u16, colour: [u8; 3]) -> Vec<u8> {
        colour.iter().copied().cycle().take(usize::from(w) * usize::from(h) * 3).collect()
    }

    /// A stream is coded by the same calls at either chroma: a keyframe first, a frame of what
    /// was read after it, and a settle at the dial it names, each a frame of
    /// the chroma's profile the decoder takes at that chroma.
    #[test]
    fn a_stream_is_coded_at_either_chroma() {
        let (w, h) = (96u16, 64u16);
        for chroma in CHROMAS {
            let mut stream = Stream::new(w, h, 40, chroma, 2).expect("a stream");
            assert_eq!((stream.chroma(), stream.size(), stream.quality()), (chroma, (w, h), 40));
            let mut decoder = Decoder::new(1).expect("a decoder");
            let mut frame = Vec::new();
            assert!(matches!(stream.encode(false, &mut frame), Err(Error::Unread)), "{chroma:?}");
            stream.read_rgb(&flat(w, h, [200, 40, 40]), None).expect("its own picture");
            assert!(stream.encode(false, &mut frame).expect("a frame"), "{chroma:?}: a first frame that is no keyframe");
            assert_eq!(frame_header(&frame), Some(FrameHeader { profile: chroma.profile(), keyframe: true }), "{chroma:?}");
            let decoded = decoder.decode(&frame).expect("a decode");
            assert_eq!((decoded.size(), decoded.chroma()), ((u32::from(w), u32::from(h)), chroma), "{chroma:?}");

            stream.set_quality(20);
            frame.clear();
            stream.read_rgb(&flat(w, h, [40, 200, 40]), Some(&[Rect { x: 0, y: 0, width: 32, height: 32 }])).expect("its own picture");
            assert!(!stream.encode(false, &mut frame).expect("a frame"), "{chroma:?}: a keyframe nobody asked for");
            decoder.decode(&frame).expect("a decode");
            assert_eq!(stream.coarsest(), 20, "{chroma:?}");

            frame.clear();
            assert!(!stream.settle(40, false, &mut frame).expect("a settle"), "{chroma:?}: a settle that is a keyframe");
            decoder.decode(&frame).expect("a decode");
            assert_eq!((stream.coarsest(), stream.quality()), (40, 20), "{chroma:?}");
        }
    }

    /// A rectangle converts to what the whole picture's conversion holds
    /// there and leaves the rest alone, at either chroma: on an odd row or column, at the last
    /// row and column, in a buffer that ends with its last row, past the
    /// picture's edge, and on any number of threads, whose bands a picture
    /// this small is given by a rectangle the size of a desktop below.
    #[test]
    fn a_rectangle_converts_as_the_whole_picture_does() {
        let rect = |x, y, width, height| Rect { x, y, width, height };
        let noise = |len: usize, salt: u32| {
            let mut seed = salt;
            (0..len).map(|_| { seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345); (seed >> 16) as u8 }).collect::<Vec<u8>>()
        };
        for (width, height, rects) in [
            (37usize, 21usize, vec![rect(0, 0, 37, 1), rect(5, 3, 1, 1), rect(11, 5, 9, 7), rect(36, 20, 1, 1), rect(30, 19, 40, 40), rect(0, 0, 37, 21), rect(7, 7, 0, 3), rect(40, 30, 5, 5), rect(1, 20, 35, 1), rect(3, 0, 31, 21)]),
            (1203, 1001, vec![rect(0, 0, 1203, 1001), rect(1, 1, 1202, 1000), rect(101, 3, 1100, 997), rect(0, 500, 1203, 501)]),
        ] {
            let stride = width * 4 + 12;
            let (before, after) = (noise((height - 1) * stride + width * 4, 1), noise((height - 1) * stride + width * 4, 2));
            // Packed RGB, which is tight.
            let tight = |pixels: &[u8]| -> Vec<u8> { pixels.chunks(stride).flat_map(|row| row[..width * 4].chunks(4).flat_map(|px| [px[2], px[1], px[0]])).collect() };
            let (before_rgb, after_rgb) = (tight(&before), tight(&after));
            for (threads, chroma) in [1, 4].into_iter().flat_map(|threads| CHROMAS.map(|chroma| (threads, chroma))) {
                let mut whole = Picture::new(width as u16, height as u16, chroma, 1).expect("a picture");
                for named in &rects {
                    // What changed is this rectangle and nothing else.
                    let (mut changed, mut changed_rgb) = (before.clone(), before_rgb.clone());
                    for row in usize::from(named.y)..(usize::from(named.y) + usize::from(named.height)).min(height) {
                        let cols = usize::from(named.x).min(width)..(usize::from(named.x) + usize::from(named.width)).min(width);
                        changed[row * stride..][cols.start * 4..cols.end * 4].copy_from_slice(&after[row * stride..][cols.start * 4..cols.end * 4]);
                        changed_rgb[row * width * 3..][cols.start * 3..cols.end * 3].copy_from_slice(&after_rgb[row * width * 3..][cols.start * 3..cols.end * 3]);
                    }
                    whole.read_bgrx(&changed, stride).expect("a picture that fits");
                    let mut partial = Picture::new(width as u16, height as u16, chroma, threads).expect("a picture");
                    partial.read_bgrx(&before, stride).expect("a picture that fits");
                    partial.read_bgrx_rect(&changed, stride, *named).expect("a picture that fits");
                    assert!(partial.planes() == whole.planes(), "{named:?} on {threads} at {chroma:?}: the planes are not the whole conversion's");

                    whole.read_rgb(&changed_rgb).expect("its own picture");
                    let mut partial = Picture::new(width as u16, height as u16, chroma, threads).expect("a picture");
                    partial.read_rgb(&before_rgb).expect("its own picture");
                    partial.read_rgb_rect(&changed_rgb, *named).expect("its own picture");
                    assert!(partial.planes() == whole.planes(), "RGB {named:?} on {threads} at {chroma:?}: the planes are not the whole conversion's");
                }
                assert!(matches!(whole.read_bgrx_rect(&before[1..], stride, rect(0, 0, 1, 1)), Err(Error::Buffer { .. })), "a buffer too short for the picture was read");
                assert!(matches!(whole.read_rgb_rect(&before_rgb[3..], rect(0, 0, 1, 1)), Err(Error::Crop(..))), "a buffer that is not the picture was read");
            }
        }
    }

    #[test]
    fn the_quality_dial_maps_onto_the_encoders_whole_range() {
        assert_eq!(quality_to_q(QUALITY_MIN), Q_COARSEST);
        assert_eq!(quality_to_q(QUALITY_MAX), Q_FINEST);
        assert_eq!(quality_to_q(0), Q_COARSEST, "clamped, not wrapped");
        assert_eq!(quality_to_q(255), Q_FINEST);
        for quality in 0..=u8::MAX {
            let q = quality_to_q(quality);
            assert!((Q_FINEST..=Q_COARSEST).contains(&q), "quality {quality} gave q {q}, outside VP9's range");
        }
        // Monotone: a higher dial is never a coarser picture.
        for quality in 1..100u8 {
            assert!(quality_to_q(quality) >= quality_to_q(quality + 1));
        }
    }

    /// Tile columns follow the width — one under 1440, two from 1440 through
    /// 1080p, four from 2048 and at 4K and 5K, eight from 5760 — and never
    /// outnumber the threads.
    #[test]
    fn tile_columns_follow_the_width_and_never_outnumber_the_threads() {
        assert_eq!(tile_columns_log2(1280, 8), 0);
        assert_eq!(tile_columns_log2(1439, 8), 0);
        assert_eq!(tile_columns_log2(1440, 8), 1);
        assert_eq!(tile_columns_log2(1920, 8), 1);
        assert_eq!(tile_columns_log2(2047, 8), 1);
        assert_eq!(tile_columns_log2(2048, 8), 2);
        assert_eq!(tile_columns_log2(2560, 8), 2);
        assert_eq!(tile_columns_log2(2880, 8), 2);
        assert_eq!(tile_columns_log2(3840, 8), 2);
        assert_eq!(tile_columns_log2(5120, 8), 2);
        assert_eq!(tile_columns_log2(5760, 8), 3);
        assert_eq!(tile_columns_log2(5760, 4), 2);
        assert_eq!(tile_columns_log2(3840, 2), 1);
        assert_eq!(tile_columns_log2(3840, 1), 0);
        assert_eq!(tile_columns_log2(3840, 0), 0);
    }

    #[test]
    fn a_picture_refuses_a_crop_that_is_not_its_picture() {
        let mut picture = Picture::new(64, 32, Chroma::Yuv444, 1).expect("a picture");
        picture.read_rgb(&flat(64, 32, [10, 20, 30])).expect("its own picture");
        assert!(picture.read_rgb(&flat(64, 31, [10, 20, 30])).is_err(), "a mis-sized crop would have indexed out of the planes");
        // Three full-size planes, tight.
        let [y, u, v] = picture.planes();
        assert_eq!((y.len(), u.len(), v.len()), (64 * 32, 64 * 32, 64 * 32));
        assert_eq!(picture.strides(), [64, 64, 64]);
        assert!(matches!(Picture::new(0, 16, Chroma::Yuv444, 1), Err(Error::Empty(0, 16))));
    }

    /// The conversion's arithmetic, at the points BT.601 studio swing pins
    /// exactly: black and white land on 16 and 235, every grey is chroma-neutral
    /// at 128, and a saturated red is the strongest V a swing this size has. The
    /// tolerance is one code value, which is the rounding the integer
    /// coefficients are allowed.
    #[test]
    fn the_conversion_is_bt601_studio_swing() {
        let mut yuv = Picture::new(2, 2, Chroma::Yuv444, 1).expect("a picture");
        let close = |got: u8, want: u8, what: &str| {
            assert!(got.abs_diff(want) <= 1, "{what}: got {got}, wanted {want}");
        };
        for (colour, y_want, u_want, v_want, name) in [
            ([0u8, 0, 0], 16u8, 128u8, 128u8, "black"),
            ([255, 255, 255], 235, 128, 128, "white"),
            ([128, 128, 128], 126, 128, 128, "mid grey"),
            ([255, 0, 0], 81, 90, 240, "red"),
            ([0, 0, 255], 41, 240, 110, "blue"),
        ] {
            yuv.read_rgb(&flat(2, 2, colour)).expect("a 2x2 picture");
            let [y, u, v] = yuv.planes();
            close(y[0], y_want, name);
            close(u[0], u_want, name);
            close(v[0], v_want, name);
            // The same pixels as `B, G, R, X` land on the same planes.
            let bgrx: Vec<u8> = (0..4).flat_map(|_| [colour[2], colour[1], colour[0], 0x55]).collect();
            yuv.read_bgrx(&bgrx, 8).expect("a 2x2 picture");
            let [y, u, v] = yuv.planes();
            close(y[0], y_want, name);
            close(u[0], u_want, name);
            close(v[0], v_want, name);
        }
        // Each pixel keeps its own colour: a checkerboard of full red and full
        // blue is not averaged.
        let mut quad = Vec::new();
        quad.extend_from_slice(&[255, 0, 0, 0, 0, 255]);
        quad.extend_from_slice(&[0, 0, 255, 255, 0, 0]);
        yuv.read_rgb(&quad).expect("a 2x2 picture");
        let [_, u, v] = yuv.planes();
        close(u[0], 90, "red pixel U");
        close(v[0], 240, "red pixel V");
        close(u[1], 240, "blue pixel U");
        close(v[1], 110, "blue pixel V");
    }

    /// The conversion is one integer formula on every machine: `yuv`'s
    /// professional mode, 15 bits a coefficient, which every one of its paths
    /// makes to the code value (NEON with and without RDM, AVX2, SSE and
    /// scalar were each run over all 2^24 colours), so that a picture is the
    /// same planes on arm64 and on amd64. Its coefficients are BT.601's at
    /// studio swing, `kr` and `kb` in `f32` times 2^15, rounded.
    #[test]
    fn the_conversion_is_the_same_integer_formula_everywhere() {
        const Y: [i32; 3] = [8414, 16519, 3208];
        const U: [i32; 3] = [-4857, -9535, 14392];
        const V: [i32; 3] = [14392, -12052, -2341];
        let formula = |[r, g, b]: [u8; 3], k: [i32; 3], bias: i32| {
            let [r, g, b] = [r, g, b].map(i32::from);
            ((r * k[0] + g * k[1] + b * k[2] + (bias << 15) + (1 << 14) - 1) >> 15) as u8
        };
        // Every seventh value of each channel and its last, a row of the
        // picture per red: wide enough for every path's vectors and a tail.
        let values: Vec<u8> = (0..=255u8).step_by(7).chain([255]).collect();
        let w = values.len() * values.len();
        let mut picture = Picture::new(w as u16, values.len() as u16, Chroma::Yuv444, 1).expect("a picture");
        let colours: Vec<[u8; 3]> = values.iter().flat_map(|&r| values.iter().map(move |&g| (r, g))).flat_map(|(r, g)| values.iter().map(move |&b| [r, g, b])).collect();
        let want: Vec<[u8; 3]> = colours.iter().map(|&c| [formula(c, Y, 16), formula(c, U, 128), formula(c, V, 128)]).collect();
        let held = |picture: &Picture, read: &str| {
            let [y, u, v] = picture.planes();
            for (i, (colour, want)) in colours.iter().zip(&want).enumerate() {
                assert_eq!([y[i], u[i], v[i]], *want, "RGB {colour:?} read as {read}");
            }
        };
        let bgrx: Vec<u8> = colours.iter().flat_map(|&[r, g, b]| [b, g, r, 0]).collect();
        picture.read_bgrx(&bgrx, w * 4).expect("the picture");
        held(&picture, "BGRX");
        picture.read_rgb(colours.as_flattened()).expect("the picture");
        held(&picture, "RGB");
    }

    #[test]
    fn a_picture_that_does_not_fit_its_buffer_is_refused() {
        assert!(fits(4, 2, 16, 32));
        assert!(fits(4, 2, 20, 36), "the last row needs no padding after it");
        assert!(!fits(4, 2, 12, 32), "a stride shorter than a row");
        assert!(!fits(4, 2, 16, 31));
        assert!(fits(0, 0, 0, 0));
        assert!(!fits(4, 1, usize::MAX, 16), "a stride the conversion cannot carry, however the bytes add up");
        let mut picture = Picture::new(4, 2, Chroma::Yuv444, 1).expect("a picture");
        assert!(matches!(picture.read_bgrx(&[0; 12], 16), Err(Error::Buffer { .. })));
    }

    /// At 4:2:0 a rectangle off the even grid reads the 2×2 blocks it touches
    /// and not a pixel past them: a buffer that is not the picture anywhere
    /// else leaves the rest of the planes as they were.
    #[test]
    fn a_420_rectangle_reads_the_blocks_it_touches_and_no_more() {
        let (w, h) = (8u16, 6u16);
        let mut picture = Picture::new(w, h, Chroma::Yuv420, 1).expect("a picture");
        picture.read_rgb(&flat(w, h, [30, 60, 90])).expect("its own picture");
        let (before_y, before_u) = (picture.planes()[0].to_vec(), picture.planes()[1].to_vec());
        // The picture, red, in the block (2..4, 2..4) alone; garbage elsewhere.
        let mut rgb = flat(w, h, [7, 250, 3]);
        for (x, y) in [(2usize, 2usize), (3, 2), (2, 3), (3, 3)] {
            rgb[(y * usize::from(w) + x) * 3..][..3].copy_from_slice(&[255, 0, 0]);
        }
        picture.read_rgb_rect(&rgb, Rect { x: 3, y: 3, width: 1, height: 1 }).expect("its own picture");
        let [y_plane, u_plane, v_plane] = picture.planes();
        for (i, (&now, &was)) in y_plane.iter().zip(&before_y).enumerate() {
            let (x, y) = (i % usize::from(w), i / usize::from(w));
            if (2..4).contains(&x) && (2..4).contains(&y) {
                assert!(now.abs_diff(81) <= 1, "luma at {x},{y} is {now}, not red's");
            } else {
                assert_eq!(now, was, "luma at {x},{y} moved");
            }
        }
        // The block's sample: row 1 of the 4-sample-wide chroma plane, column 1.
        let block = 4 + 1;
        assert!(u_plane[block].abs_diff(90) <= 1 && v_plane[block].abs_diff(240) <= 1, "the block's colour is not red's");
        for (i, (&now, &was)) in u_plane.iter().zip(&before_u).enumerate().filter(|&(i, _)| i != block) {
            assert_eq!(now, was, "chroma sample {i} moved");
        }
    }

    #[test]
    fn a_chroma_is_its_profile() {
        assert_eq!((Chroma::Yuv444.profile(), Chroma::Yuv420.profile()), (1, 0));
        for chroma in CHROMAS {
            assert_eq!(Chroma::of_profile(chroma.profile()), Some(chroma));
        }
        assert_eq!(Chroma::of_profile(2), None, "ten and twelve bits are no chroma here");
    }

    /// A 4:2:0 picture's chroma planes are a sample per 2×2 pixels, the odd
    /// last row and column included, and a block's sample is its four pixels'
    /// colour together.
    #[test]
    fn a_420_picture_samples_colour_per_block() {
        let mut picture = Picture::new(5, 3, Chroma::Yuv420, 1).expect("a picture");
        picture.read_rgb(&flat(5, 3, [255, 0, 0])).expect("its own picture");
        let [y, u, v] = picture.planes();
        assert_eq!((y.len(), u.len(), v.len()), (15, 6, 6));
        assert_eq!(picture.strides(), [5, 3, 3]);
        assert!(u.iter().all(|&u| u.abs_diff(90) <= 1) && v.iter().all(|&v| v.abs_diff(240) <= 1), "red is red at 4:2:0: {u:?} {v:?}");
        // A block of two red pixels and two blue is their mean, not either.
        let mut quad = Picture::new(2, 2, Chroma::Yuv420, 1).expect("a picture");
        quad.read_rgb(&[255, 0, 0, 0, 0, 255, 0, 0, 255, 255, 0, 0]).expect("a 2x2 picture");
        let [_, u, v] = quad.planes();
        assert_eq!((u.len(), v.len()), (1, 1));
        assert!(u[0].abs_diff(165) <= 1 && v[0].abs_diff(175) <= 1, "a block's colour is its pixels' mean: {u:?} {v:?}");
    }

    #[test]
    fn a_frame_header_is_read_from_the_bitstream_alone() {
        assert_eq!(frame_header(&[]), None);
        assert_eq!(frame_header(&[0x00, 0x00]), None, "no frame marker");
    }

    /// The level table, at the rows a desktop actually lands on. The numbers are
    /// libvpx's own; what is tested is that the *lowest* fitting row is chosen,
    /// since a level is a ceiling and announcing a higher one narrows the set of
    /// decoders that will accept the stream.
    #[test]
    fn the_codec_string_names_the_lowest_level_that_fits() {
        let codec_string = |w, h| super::codec_string(w, h, 30, Chroma::Yuv444);
        // 1280x800 at 30: 1_024_000 samples. Level 3.1 allows only 983_040 of
        // them, so this is level 4 — the *picture size* binds here, not the
        // sample rate, which at 30_720_000 is well inside 3.1's 36_864_000.
        assert_eq!(codec_string(1280, 800).as_deref(), Some("vp09.01.40.08.03.06.06.06.00"));
        // 1920x1080: 2_073_600 samples, inside level 4's picture size of
        // 2_228_224, and 62_208_000 per second inside its 83_558_400.
        assert_eq!(codec_string(1920, 1080).as_deref(), Some("vp09.01.40.08.03.06.06.06.00"));
        // 3840x2160: 8_294_400 samples — past 4.1's picture size, inside 5.0's
        // 8_912_896 — and 248_832_000 per second, inside 5.0's 311_951_360.
        assert_eq!(codec_string(3840, 2160).as_deref(), Some("vp09.01.50.08.03.06.06.06.00"));
        // 3840x2400: 9_216_000 samples, past every level 5's 8_912_896 picture
        // size by the 16:10 panel's extra 240 rows — level 6.0, the first that
        // holds it.
        assert_eq!(codec_string(3840, 2400).as_deref(), Some("vp09.01.60.08.03.06.06.06.00"));
        // Small, but not level 1: 76_800 samples is already past level 1.1's
        // 73_728. Level 1 is 256x144, which no desktop is.
        assert_eq!(codec_string(320, 240).as_deref(), Some("vp09.01.20.08.03.06.06.06.00"));
        // Profile 1, eight bits and 4:4:4 whatever the size, and the colour
        // fields never move: BT.601 at studio swing is what every keyframe
        // header declares, and a string that left them out would be read as
        // BT.709.
        for (w, h) in [(320u16, 240u16), (1920, 1080), (3840, 2160), (3840, 2400)] {
            let string = codec_string(w, h).expect("a level for a real desktop");
            assert!(string.starts_with("vp09.01."), "not profile 1: {string}");
            assert!(string.ends_with(".08.03.06.06.06.00"), "not 8-bit 4:4:4 BT.601: {string}");
        }
        // At 60 frames a second the sample rate binds where the picture size did
        // not: 1080p60 is 124_416_000 samples a second, past 4.0's 83_558_400,
        // and 4K60 497_664_000, past 5.0's 311_951_360.
        let at_60 = |w, h| super::codec_string(w, h, 60, Chroma::Yuv444);
        assert_eq!(at_60(1920, 1080).as_deref(), Some("vp09.01.41.08.03.06.06.06.00"));
        assert_eq!(at_60(3840, 2160).as_deref(), Some("vp09.01.51.08.03.06.06.06.00"));
        assert_eq!(super::codec_string(20_000, 20_000, 60, Chroma::Yuv444), None, "no level covers it");
        // 4:2:0 is profile 0 and chroma field 1 at the same level and colour.
        assert_eq!(super::codec_string(1920, 1080, 30, Chroma::Yuv420).as_deref(), Some("vp09.00.40.08.01.06.06.06.00"));
        assert_eq!(super::codec_string(3840, 2160, 60, Chroma::Yuv420).as_deref(), Some("vp09.00.51.08.01.06.06.06.00"));
    }
}
