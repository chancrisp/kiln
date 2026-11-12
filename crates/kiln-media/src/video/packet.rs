//! The muxers' common face: encoded packets in, one container file out.
//!
//! Interleaving is the caller's job. A muxer takes each stream's packets in
//! decode order, and the streams in any order as long as they stay within a
//! window of about half a second of each other (the MP4 writer's chunk, the
//! Matroska writer's cluster); it writes the chunks and clusters itself.

use super::info::Hdr;
use super::{mux_mkv, mux_mp4};
use crate::error::{MediaError, Result};
use crate::types::Tags;
use std::fs::File;

/// A stream's codec and the private data its container needs for it.
///
/// What each `Vec` holds is what the codec's MP4 box holds, which is also
/// what Matroska keeps in `CodecPrivate`:
/// - `avcc`, `hvcc` and `av1c`: the body of the `avcC`, `hvcC` or `av1C` box.
/// - `vpcc`: the body of the `vpcC` box, version and flags included. (A VP9
///   track in Matroska needs no `CodecPrivate`, and gets none.)
/// - `asc`: the AAC AudioSpecificConfig.
/// - `head`: the `OpusHead` packet.
/// - `private`: Vorbis' three headers, Xiph-laced, as Matroska keeps them.
/// - `streaminfo`: FLAC's 34-byte STREAMINFO block, or a whole `fLaC` header.
///
/// Packets of H.264 and HEVC are length-prefixed (as in an MP4 sample), the
/// prefix's size being the one the `avcC` or `hvcC` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamCodec {
    H264 { avcc: Vec<u8> },
    Hevc { hvcc: Vec<u8> },
    Av1 { av1c: Vec<u8> },
    Vp9 { vpcc: Vec<u8> },
    Aac { asc: Vec<u8> },
    Opus { head: Vec<u8> },
    Vorbis { private: Vec<u8> },
    Flac { streaminfo: Vec<u8> },
    Ac3,
}

impl StreamCodec {
    /// True for the picture codecs.
    pub fn is_video(&self) -> bool {
        matches!(
            self,
            StreamCodec::H264 { .. }
                | StreamCodec::Hevc { .. }
                | StreamCodec::Av1 { .. }
                | StreamCodec::Vp9 { .. }
        )
    }

    /// The codec's name, for messages.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            StreamCodec::H264 { .. } => "H.264",
            StreamCodec::Hevc { .. } => "HEVC",
            StreamCodec::Av1 { .. } => "AV1",
            StreamCodec::Vp9 { .. } => "VP9",
            StreamCodec::Aac { .. } => "AAC",
            StreamCodec::Opus { .. } => "Opus",
            StreamCodec::Vorbis { .. } => "Vorbis",
            StreamCodec::Flac { .. } => "FLAC",
            StreamCodec::Ac3 => "AC-3",
        }
    }
}

/// One stream to be written. `video` is set for the picture codecs and
/// `audio` for the sound ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSpec {
    pub codec: StreamCodec,
    /// Ticks per second of this stream's packet times. Media Foundation's
    /// are 100 ns, so 10,000,000.
    pub timescale: u32,
    pub video: Option<VideoParams>,
    pub audio: Option<AudioParams>,
}

/// The picture's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoParams {
    /// The coded size, before rotation.
    pub width: u32,
    pub height: u32,
    /// Degrees clockwise the picture is turned to show it upright: 0, 90,
    /// 180 or 270.
    pub rotation: u16,
    pub hdr: Hdr,
    pub bit_depth: u8,
    /// The `mdcv` box's 24 bytes: the display's primaries (green, blue, red)
    /// and white point as 16-bit x and y in steps of 0.00002, then the
    /// highest and lowest luminance in steps of 0.0001 nits.
    pub mastering: Option<[u8; 24]>,
    /// The `clli` box's 4 bytes: the brightest pixel and the brightest frame
    /// average, in nits, 16 bits each.
    pub content_light: Option<[u8; 4]>,
    /// The pixel aspect ratio, horizontal to vertical, of the coded picture:
    /// (1, 1) for square pixels. Otherwise the MP4 writer adds a `pasp` box
    /// and the Matroska writer a display size.
    pub sar: (u32, u32),
    /// True when the writer describes the colour even for standard range,
    /// as BT.709 (primaries, transfer and matrix) in limited range, as for a
    /// picture Kiln encoded. HDR is always described. A copy leaves this
    /// false and keeps the source's own description (`stated_colour`).
    pub tag_colour: bool,
    /// With `tag_colour`, standard range is described as BT.601 (SMPTE 170M
    /// primaries, transfer and matrix) instead of BT.709: the picture of an
    /// untagged source under 720 lines, whose samples are kept as players
    /// read them (R68).
    pub bt601: bool,
    /// Every frame's length in nanoseconds when the rate is constant: the
    /// Matroska writer's `DefaultDuration` (R80). `None` for a variable or
    /// unknown rate.
    pub frame_ns: Option<u64>,
    /// A copy's colour as the source's container states it, written as it
    /// is for standard range, as FFmpeg's stream copy does (R94). `None`
    /// for an encode, which `tag_colour` describes, and for a source that
    /// states none, which stays untagged.
    pub stated_colour: Option<StatedColour>,
}

/// A colour description as a container states it (an MP4's `colr`, a
/// Matroska's `Colour`): H.273's primaries, transfer and matrix, and the
/// range when it is stated (true for full range).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatedColour {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: Option<bool>,
}

impl VideoParams {
    /// The pixel aspect ratio when it isn't square, in lowest terms.
    pub(crate) fn anamorphic(&self) -> Option<(u32, u32)> {
        let (h, v) = self.sar;
        if h == 0 || v == 0 || h == v {
            return None;
        }
        let g = gcd(h, v);
        Some((h / g, v / g))
    }

    /// The colour the writers describe, as H.273 numbers it: primaries,
    /// transfer and matrix (R40, R68), or for a copy the source's own
    /// (R94). `None` when they describe none.
    pub(crate) fn described_colour(&self) -> Option<[u8; 3]> {
        match self.hdr {
            Hdr::Pq => Some([9, 16, 9]),
            Hdr::Hlg => Some([9, 18, 9]),
            Hdr::None if self.tag_colour && self.bt601 => Some([6, 6, 6]),
            Hdr::None if self.tag_colour => Some([1, 1, 1]),
            Hdr::None => self
                .stated_colour
                .map(|c| [c.primaries, c.transfer, c.matrix]),
        }
    }

    /// The range the writers state with the colour: limited (false) for
    /// HDR and for a picture Kiln encoded, else the source's as its
    /// container states it (R94). `None` when none is stated.
    pub(crate) fn full_range(&self) -> Option<bool> {
        match self.hdr {
            Hdr::None if !self.tag_colour => self.stated_colour.and_then(|c| c.full_range),
            _ => Some(false),
        }
    }
}

/// The greatest common divisor.
pub(crate) fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// The sound's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioParams {
    pub rate: u32,
    pub channels: u16,
}

/// One encoded frame or block of sound. Times are in the stream's
/// `timescale`; `dts` is the time it is decoded and `pts` the time it is
/// shown, which differ when the stream has B-frames.
#[derive(Debug, Clone)]
pub struct Packet {
    pub stream: usize,
    pub pts: i64,
    pub dts: i64,
    pub duration: i64,
    pub key: bool,
    pub data: Vec<u8>,
}

/// A container being written.
pub trait Mux: Send {
    /// Adds a stream, before the first packet. Returns its index.
    fn add_stream(&mut self, s: StreamSpec) -> Result<usize>;
    /// Says when the movie was made, in seconds since 1970-01-01 UTC, and
    /// its title, before the first packet. An MP4 keeps the time in its
    /// `mvhd` (the title goes in the tags given to `finish`); a Matroska
    /// keeps both in its `Info`.
    fn describe(&mut self, created: Option<i64>, title: Option<&str>);
    /// Writes a packet. Each stream's packets come in decode order.
    fn write(&mut self, p: Packet) -> Result<()>;
    /// Says that stream `stream` is presented from `ticks` (its time scale)
    /// on, before `finish`: an MP4's edit list then hides what comes before
    /// (a copy's lead-in, R69). Without it a stream starts at its first
    /// packet, or at 0 when that is earlier. Matroska, which has no edit
    /// list, ignores it.
    fn present_from(&mut self, _stream: usize, _ticks: i64) {}
    /// Finishes the file, with the tags in it. Dropping the muxer instead
    /// leaves the file unfinished and removes any temporary file.
    fn finish(self: Box<Self>, tags: &Tags) -> Result<()>;
    /// [`Mux::finish`], telling `watch` how far the finishing has got, from
    /// 0 to 1, as it goes; when `watch` answers true the file is left
    /// unfinished and this returns [`MediaError::Cancelled`]. An MP4 copies
    /// all its samples here (R92); a writer with nothing long to do just
    /// finishes.
    fn finish_watched(
        self: Box<Self>,
        tags: &Tags,
        watch: &mut dyn FnMut(f64) -> bool,
    ) -> Result<()> {
        let _ = watch;
        self.finish(tags)
    }
}

/// The containers Kiln writes video into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Mp4,
    Mov,
    M4v,
    Mkv,
}

/// Makes a muxer that writes `c` to `out`. An MP4, MOV or M4V needs room
/// beside `out` for a temporary file; a Matroska is written in place.
pub fn muxer(c: Container, out: File) -> Result<Box<dyn Mux>> {
    match c {
        Container::Mkv => Ok(Box::new(mux_mkv::MkvMux::new(out))),
        _ => Ok(Box::new(mux_mp4::Mp4Mux::new(c, out, 0, None)?)),
    }
}

/// [`muxer`] for `out`, the file at `path`: an MP4's temporary samples file
/// goes in the same folder as `path`, on its volume (R92).
pub fn muxer_at(c: Container, out: File, path: &std::path::Path) -> Result<Box<dyn Mux>> {
    match c {
        Container::Mkv => Ok(Box::new(mux_mkv::MkvMux::new(out))),
        _ => Ok(Box::new(mux_mp4::Mp4Mux::new(c, out, 0, Some(path))?)),
    }
}

/// Like [`muxer`] for an MP4, as if `bias` bytes came before the file's
/// own, so chunk offsets past 4 GiB can be tested without writing 4 GiB.
pub(crate) fn mp4_muxer_with_bias(c: Container, out: File, bias: u64) -> Result<Box<dyn Mux>> {
    Ok(Box::new(mux_mp4::Mp4Mux::new(c, out, bias, None)?))
}

/// `v` ticks of `from` per second as ticks of `to`, rounded to the nearest
/// (halves up).
pub(crate) fn rescale(v: i64, from: u32, to: u32) -> i64 {
    let (from, to) = (from.max(1) as i128, to as i128);
    let n = v as i128 * to;
    let q = n.div_euclid(from);
    let r = n.rem_euclid(from);
    (if r * 2 >= from { q + 1 } else { q }).clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// Checks a stream against what its codec needs, shared by both writers.
pub(crate) fn check_spec(s: &StreamSpec) -> Result<()> {
    let bad = |what: &str| Err(MediaError::Encoder(format!("{} {what}", s.codec.name())));
    if s.timescale == 0 {
        return bad("stream has no time scale");
    }
    if s.codec.is_video() {
        match &s.video {
            Some(v) if v.width > 0 && v.height > 0 && v.width <= 0xFFFF && v.height <= 0xFFFF => {}
            Some(_) => return bad("stream has a size that can't be written"),
            None => return bad("stream has no picture size"),
        }
        if !matches!(
            s.video.as_ref().map(|v| v.rotation),
            Some(0 | 90 | 180 | 270)
        ) {
            return bad("stream has a rotation that isn't a quarter turn");
        }
        if s.video.as_ref().is_some_and(|v| v.frame_ns == Some(0)) {
            return bad("stream has frames of no length");
        }
    } else {
        match &s.audio {
            Some(a) if a.rate > 0 && a.channels > 0 => {}
            _ => return bad("stream has no sound facts"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescaling_rounds_to_the_nearest() {
        assert_eq!(rescale(10_000_000, 10_000_000, 1000), 1000);
        assert_eq!(rescale(4_999, 10_000, 1000), 500);
        assert_eq!(rescale(5, 10_000, 1000), 1); // half rounds up
        assert_eq!(rescale(4, 10_000, 1000), 0);
        assert_eq!(rescale(-4, 10_000, 1000), 0);
        assert_eq!(rescale(-5, 10_000, 1000), 0); // -0.5 rounds up to 0
        assert_eq!(rescale(-6, 10_000, 1000), -1);
        assert_eq!(rescale(i64::MAX, 1, u32::MAX), i64::MAX);
        assert_eq!(rescale(3, 48_000, 48_000), 3);
    }
}
