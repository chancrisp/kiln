//! Reading a video file through Media Foundation's source reader, which only
//! ever sees the file through Kiln's own byte stream.
//!
//! Two ways:
//! - [`ReadMode::Compressed`]: the streams as the file holds them, as
//!   [`Packet`]s for a muxer to copy. No decoder is involved.
//! - [`ReadMode::Decoded`]: the picture decoded to NV12 (P010 for a deeper
//!   source, when the decoder gives it), as [`Frame`]s. Given a [`D3d`], the
//!   decoder runs on that graphics card and each frame stays a texture there.
//!
//! What Media Foundation gives isn't quite what the file says, so the reader
//! puts it right from Kiln's own reading of the container:
//! - Codec private data (`avcC`, `hvcC`, `av1C`, `vpcC`, the AAC
//!   AudioSpecificConfig, `OpusHead`, Vorbis' and FLAC's headers) comes from
//!   the container's own boxes and elements. Media Foundation's sequence
//!   header (H.264 and HEVC, in Annex B) and user data (AAC, Opus) are used
//!   only when the container has none.
//! - H.264 and HEVC samples come from Media Foundation in Annex B, with the
//!   parameter sets repeated in every key frame. They are given out with
//!   4-byte lengths, the repeats that match the private data taken back out.
//! - An MP4's edit list is applied here, never by Media Foundation, which
//!   reads `edts` as `free` (see `first_edit`): each track's times are
//!   moved by its first edit (empty edits first, then the media time the
//!   presentation starts at), as FFmpeg presents them. In a Matroska file,
//!   `CodecDelay` moves its track earlier; an Opus track without one is
//!   moved by its pre-skip, so
//!   `Packet::pts` is FFmpeg's (the pre-skip is written back as
//!   `CodecDelay`).
//! - A Matroska picture's decoded frames take its block times, the n-th
//!   frame the n-th smallest (R78): Media Foundation's decoder doesn't always
//!   give the first frame its block's time. Where the frames and blocks don't
//!   line up one for one, the frames keep the decoder's times
//!   ([`Reader::decoder_times`]).
//! - Decode times are worked out here, as Media Foundation gives them on only
//!   some samples: the picture's are its sorted presentation times, shifted
//!   back by the stream's reorder depth. Sound's are its presentation times.
//! - The frame rate and length are the container's ([`Reader::info`]), not
//!   Media Foundation's (which reads a 30 fps Matroska as 15.000015).
//!
//! Every time is in Media Foundation's 100 ns units: each stream's
//! `timescale` is 10,000,000, and the muxers rescale.

// Off Windows only the stubs below use this module, so the helpers the
// Windows reader calls go unused there.
#![cfg_attr(not(windows), allow(dead_code))]

use super::info::{self, Hdr, VideoCodec, VideoInfo};
use super::packet::{
    rescale, AudioParams, Packet, StatedColour, StreamCodec, StreamSpec, VideoParams,
};
use crate::error::{MediaError, Result};
use crate::mf::d3d::D3d;
use crate::mp4read::{be16, be32, be64, boxes, child, timescale};
use crate::{mkvread, mp4read};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// Ticks per second of every time the reader gives: Media Foundation's.
pub const TIMESCALE: u32 = 10_000_000;

/// The most decoded [`Frame`]s a caller may hold at once when decoding on a
/// graphics card. Each one is a texture from the decoder's own pool, and
/// holding too many breaks the decoder. Measured on this PC with the
/// fixtures: H.264 stalls (the next read never returns) at 8 held, HEVC at
/// 12. AV1 doesn't stall but fails with `Damaged`: at 7 held in the review's
/// run, at 12 in later runs on both cards; 5 and 6 were always fine. Media
/// Foundation ignores a request for a bigger pool
/// (`MF_SA_MINIMUM_OUTPUT_SAMPLE_COUNT`, set on the reader's decoder). A
/// stream with more reference frames leaves fewer free, so the limit is well
/// under the least seen. Frames decoded without a card are in memory and
/// have no limit.
pub const MAX_HELD_FRAMES: usize = 4;

/// How to read.
pub enum ReadMode {
    /// The streams as stored, for copying.
    Compressed,
    /// The picture decoded; on the graphics card when `d3d` is given.
    Decoded { d3d: Option<D3d> },
}

/// One stream the reader gives out. `index` is its place in
/// [`Reader::streams`], which is also its packets' `Packet::stream`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadStream {
    pub index: usize,
    pub spec: StreamSpec,
}

/// The pixel layout of decoded frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pixel {
    /// 8-bit 4:2:0.
    Nv12,
    /// 10-bit 4:2:0, in the top bits of 16.
    P010,
}

/// A rectangle of a frame, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The brightness plane of a decoded frame's picture, for a look at it in
/// memory ([`Frame::luma`]): `width` by `height` samples, row by row, each
/// from 0 to `max` (255 for NV12, 1023 for P010).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Luma {
    pub width: u32,
    pub height: u32,
    pub max: u16,
    pub samples: Vec<u16>,
}

impl Luma {
    /// Row `y`.
    pub fn row(&self, y: u32) -> &[u16] {
        let w = self.width as usize;
        &self.samples[y as usize * w..(y as usize + 1) * w]
    }
}

fn damaged() -> MediaError {
    MediaError::Damaged {
        code: "video".into(),
    }
}

fn hns(v: u64, scale: u32) -> i64 {
    rescale(v.min(i64::MAX as u64) as i64, scale, TIMESCALE)
}

// ---------------------------------------------------------------------------
// What the container says, by Kiln's own reading

/// A track's codec, as the container names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tag {
    H264,
    Hevc,
    Av1,
    Vp9,
    Aac,
    Opus,
    Vorbis,
    Flac,
    Ac3,
    /// A codec the reader doesn't give packets of, by name ("DTS").
    Other(String),
}

impl Tag {
    fn name(&self) -> String {
        match self {
            Tag::H264 => "H.264".into(),
            Tag::Hevc => "HEVC".into(),
            Tag::Av1 => "AV1".into(),
            Tag::Vp9 => "VP9".into(),
            Tag::Aac => "AAC".into(),
            Tag::Opus => "Opus".into(),
            Tag::Vorbis => "Vorbis".into(),
            Tag::Flac => "FLAC".into(),
            Tag::Ac3 => "AC-3".into(),
            Tag::Other(n) => n.clone(),
        }
    }
}

/// One track's facts from the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Track {
    pub tag: Tag,
    /// The codec's private data, as [`StreamCodec`] holds it; empty when the
    /// container has none.
    pub private: Vec<u8>,
    /// Added to Media Foundation's times (100 ns) to put the track where the
    /// container presents it: the edit list, or `CodecDelay`.
    pub shift: i64,
    /// Where the track's presentation starts (100 ns): its edit list's empty
    /// edits, 0 without. What comes before it is hidden.
    pub start: i64,
    /// An MP4 track's media time scale: Media Foundation's times are put
    /// back on it before the shift, so its rounding doesn't add up with ours.
    pub clock: Option<u32>,
    pub mastering: Option<[u8; 24]>,
    pub content_light: Option<[u8; 4]>,
    /// The colour the container states for a picture track, kept on a copy
    /// (R94).
    pub colour: Option<StatedColour>,
    /// A sound track's rate and channels, as the container says them.
    pub rate: Option<u32>,
    pub channels: Option<u16>,
}

impl Track {
    fn of(tag: Tag) -> Track {
        Track {
            tag,
            private: Vec::new(),
            shift: 0,
            start: 0,
            clock: None,
            mastering: None,
            content_light: None,
            colour: None,
            rate: None,
            channels: None,
        }
    }
}

/// The colour an MP4 sample entry's `colr` (`nclx` or `nclc`) states, among
/// the entry's boxes `inner`. `None` without one, or when it says nothing
/// (every value unspecified).
fn colr_colour(inner: &[u8]) -> Option<StatedColour> {
    let v = boxes(inner)
        .filter(|(k, _)| k == b"colr")
        .find(|(_, v)| matches!(v.get(..4), Some(b"nclx" | b"nclc")))?
        .1;
    let at = |i| be16(v, i).and_then(|n| u8::try_from(n).ok());
    let full_range = (v.get(..4) == Some(b"nclx"))
        .then(|| v.get(10).map(|b| b & 0x80 != 0))
        .flatten();
    stated(at(4)?, at(6)?, at(8)?, full_range)
}

/// A stated colour, or `None` when its values say nothing (see
/// [`super::info::colour_stated`]).
fn stated(
    primaries: u8,
    transfer: u8,
    matrix: u8,
    full_range: Option<bool>,
) -> Option<StatedColour> {
    super::info::colour_stated([primaries, transfer, matrix].map(u64::from)).then_some(
        StatedColour {
            primaries,
            transfer,
            matrix,
            full_range,
        },
    )
}

/// The colour a Matroska `Colour` states; values it leaves out are
/// unspecified (2), and a range of 0 is unstated.
fn matroska_colour(v: &mkvread::Video) -> Option<StatedColour> {
    let value = |n: Option<u64>| u8::try_from(n.unwrap_or(2)).ok();
    let full_range = match v.range {
        Some(1) => Some(false),
        Some(2) => Some(true),
        _ => None,
    };
    stated(
        value(v.primaries)?,
        value(v.transfer)?,
        value(v.matrix)?,
        full_range,
    )
}

/// The first picture and sound tracks of a file, and what kind of file it is.
#[derive(Debug, Clone, Default)]
pub(crate) struct Facts {
    pub matroska: bool,
    pub quicktime: bool,
    pub video: Option<Track>,
    pub sound: Option<Track>,
    /// True when the container's own structure says the file was cut short
    /// (see [`mp4_data_end`] and `mkvread::cut_short`).
    pub truncated: bool,
    /// Where each track's `edts` box has its type, in an MP4 or MOV (see
    /// [`edit_list_boxes`]).
    pub edit_lists: Vec<u64>,
    /// A Matroska picture's `TrackNumber`, the file's `TimestampScale` and
    /// the track's `DefaultDuration` (ns): how its blocks are timed (R78).
    pub mkv_picture: Option<MkvPicture>,
}

/// How a Matroska picture's blocks are found and timed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MkvPicture {
    pub number: u64,
    pub scale: u64,
    pub default_duration: Option<u64>,
}

/// Reads the facts from the file's headers, leaving its position at 0.
pub(crate) fn facts(file: &mut File) -> Facts {
    let mut head = [0u8; 12];
    let n = file
        .seek(SeekFrom::Start(0))
        .and_then(|_| file.read(&mut head))
        .unwrap_or(0);
    let len = file.metadata().map_or(u64::MAX, |m| m.len());
    let mut out = if n >= 4 && head[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        let mut f = mkvread::read_headers(file)
            .map(|h| mkv_facts(&h))
            .unwrap_or_default();
        f.truncated = mkvread::cut_short(file);
        f
    } else {
        match mp4read::find_moov(file, mp4read::may_start_a_movie) {
            Some((at, moov)) => {
                let mut f = mp4_facts(&moov);
                f.truncated = mp4_data_end(&moov).is_some_and(|end| end > len)
                    || fragments_cut_short(file, &moov, len);
                f.edit_lists = edit_list_boxes(at, &moov);
                f
            }
            None => Facts::default(),
        }
    };
    out.quicktime = n >= 12 && &head[4..8] == b"ftyp" && &head[8..12] == b"qt  ";
    let _ = file.seek(SeekFrom::Start(0));
    out
}

fn mp4_facts(moov: &[u8]) -> Facts {
    let movie_scale = child(moov, b"mvhd").and_then(timescale);
    let mut f = Facts::default();
    for (kind, trak) in boxes(moov) {
        if &kind != b"trak" {
            continue;
        }
        let handler = child(trak, b"mdia")
            .and_then(|m| child(m, b"hdlr"))
            .and_then(|h| h.get(8..12));
        match handler {
            Some(b"vide") if f.video.is_none() => f.video = mp4_track(trak, movie_scale, true),
            Some(b"soun") if f.sound.is_none() => f.sound = mp4_track(trak, movie_scale, false),
            _ => {}
        }
    }
    f
}

fn mp4_track(trak: &[u8], movie_scale: Option<u32>, video: bool) -> Option<Track> {
    let mdia = child(trak, b"mdia")?;
    let media_scale = timescale(child(mdia, b"mdhd")?)?;
    let stbl = child(child(mdia, b"minf")?, b"stbl")?;
    let stsd = child(stbl, b"stsd")?;
    let (fourcc, entry) = boxes(stsd.get(8..)?).next()?;
    // A sound entry's channels and rate: 16.16 at 24 in versions 0 and 1, a
    // double at 32 (and a 32-bit count at 40) in a QuickTime version 2.
    let (rate, channels) = match (video, be16(entry, 8)) {
        (false, Some(0 | 1)) => (be32(entry, 24).map(|r| r >> 16), be16(entry, 16)),
        (false, Some(2)) => (
            be64(entry, 32)
                .map(f64::from_bits)
                .filter(|r| r.is_finite() && *r > 0.0 && *r < 1e7)
                .map(|r| r.round() as u32),
            be32(entry, 40).and_then(|c| u16::try_from(c).ok()),
        ),
        _ => (None, None),
    };
    let inner = if video {
        entry.get(78..)?
    } else {
        // A sound sample entry: 28 bytes, 16 more in a QuickTime version 1
        // and 36 more in a version 2, then its boxes.
        let extra = match be16(entry, 8)? {
            1 => 16,
            2 => 36,
            _ => 0,
        };
        entry.get(28 + extra..)?
    };
    let (tag, private) = mp4_codec(&fourcc, inner);
    // AAC's own configuration says its channels; the entry's count is
    // often a template 2.
    let channels = if tag == Tag::Aac {
        asc_channels(&private)
    } else {
        channels
    };
    let elst = child(trak, b"edts")
        .and_then(|e| child(e, b"elst"))
        .zip(movie_scale);
    let shift = elst
        .and_then(|(elst, ms)| edit_shift(elst, ms, media_scale))
        .unwrap_or(0);
    let start = elst.map_or(0, |(elst, ms)| edit_start(elst, ms));
    Some(Track {
        tag,
        private,
        shift,
        start,
        clock: Some(media_scale),
        mastering: child(inner, b"mdcv").and_then(|b| b.get(..24)?.try_into().ok()),
        content_light: child(inner, b"clli").and_then(|b| b.get(..4)?.try_into().ok()),
        colour: video.then(|| colr_colour(inner)).flatten(),
        rate: rate.filter(|&r| r > 0),
        channels: channels.filter(|&c| c > 0),
    })
}

/// Where the last sample of any track ends, by the tables of every track
/// (`stsc`, `stco` or `co64`, `stsz`): a file shorter than that was cut
/// short. `None` when there are no sample tables to go by (a fragmented
/// MP4). Where the tables disagree on how many samples there are, the
/// walk stops at the last sample all of them give, as Media Foundation does.
pub(crate) fn mp4_data_end(moov: &[u8]) -> Option<u64> {
    let mut furthest: Option<u64> = None;
    for (kind, trak) in boxes(moov) {
        if &kind != b"trak" {
            continue;
        }
        if let Some(end) = track_data_end(trak) {
            furthest = Some(furthest.map_or(end, |f| f.max(end)));
        }
    }
    furthest
}

fn track_data_end(trak: &[u8]) -> Option<u64> {
    let stbl = child(child(child(trak, b"mdia")?, b"minf")?, b"stbl")?;
    let offsets: Vec<u64> = if let Some(co) = child(stbl, b"stco") {
        let n = be32(co, 4)? as usize;
        (0..n)
            .map(|i| be32(co, 8 + i * 4).map(u64::from))
            .collect::<Option<_>>()?
    } else {
        let co = child(stbl, b"co64")?;
        let n = be32(co, 4)? as usize;
        (0..n).map(|i| be64(co, 8 + i * 8)).collect::<Option<_>>()?
    };
    let stsz = child(stbl, b"stsz")?;
    let each = be32(stsz, 4)? as u64;
    let count = be32(stsz, 8)? as usize;
    let size = |i: usize| -> Option<u64> {
        if each != 0 {
            (i < count).then_some(each)
        } else {
            be32(stsz, 12 + i * 4).map(u64::from).filter(|_| i < count)
        }
    };
    let stsc = child(stbl, b"stsc")?;
    let runs = be32(stsc, 4)? as usize;
    let run = |i: usize| -> Option<(u32, u32)> {
        Some((be32(stsc, 8 + i * 12)?, be32(stsc, 12 + i * 12)?))
    };
    // The walk stops where any table runs out: Media Foundation reads no
    // sample past the last one all three tables give.
    let mut end = 0u64;
    let mut sample = 0usize;
    for r in 0..runs {
        let Some((first, per)) = run(r) else {
            break;
        };
        let last = match run(r + 1) {
            Some((next, _)) if r + 1 < runs => next,
            _ => offsets.len() as u32 + 1,
        };
        for chunk in first..last {
            let Some(&start) = chunk.checked_sub(1).and_then(|c| offsets.get(c as usize)) else {
                return Some(end);
            };
            let mut at = start;
            for _ in 0..per {
                let Some(next) = size(sample).and_then(|n| at.checked_add(n)) else {
                    return Some(end.max(at));
                };
                at = next;
                end = end.max(at);
                sample += 1;
            }
        }
    }
    Some(end)
}

/// True when a fragmented MP4 (one whose `moov` has an `mvex`) was cut
/// short: a top-level box runs past the end of the file or has its header
/// cut off, or a fragment's `trun` puts sample data past the end. A file
/// cut exactly between fragments can't be told from a whole one. Leaves the
/// position anywhere.
pub(crate) fn fragments_cut_short(file: &mut File, moov: &[u8], len: u64) -> bool {
    let Some(mvex) = child(moov, b"mvex") else {
        return false;
    };
    // Each track's default sample size, for a fragment that gives none.
    let defaults: Vec<(u32, u32)> = boxes(mvex)
        .filter(|(kind, _)| kind == b"trex")
        .filter_map(|(_, trex)| Some((be32(trex, 4)?, be32(trex, 16)?)))
        .collect();
    let mut pos = 0u64;
    while pos < len {
        let mut head = Vec::with_capacity(16);
        if file.seek(SeekFrom::Start(pos)).is_err()
            || file.by_ref().take(16).read_to_end(&mut head).is_err()
        {
            return false;
        }
        if head.len() < 8 {
            return true;
        }
        let (size, header) = match be32(&head, 0) {
            Some(0) => (len - pos, 8),
            Some(1) => match be64(&head, 8) {
                Some(size) => (size, 16),
                None => return true,
            },
            Some(n) => (u64::from(n), 8),
            None => return false,
        };
        if size < header {
            return false;
        }
        let Some(end) = pos.checked_add(size).filter(|&end| end <= len) else {
            return true;
        };
        if &head[4..8] == b"moof" && size <= 16 << 20 {
            let mut moof = vec![0u8; (size - header) as usize];
            if file.seek(SeekFrom::Start(pos + header)).is_err()
                || file.read_exact(&mut moof).is_err()
            {
                return false;
            }
            if moof_data_end(&moof, pos, &defaults).is_some_and(|data| data > len) {
                return true;
            }
        }
        pos = end;
    }
    false
}

/// How long a fragmented MP4's picture lasts, in seconds, by adding up its
/// fragments' sample durations: such a file states no length (R22), and the
/// job's progress needs one. `None` for a file that isn't fragmented, or
/// whose fragments don't say.
pub(crate) fn fragmented_seconds(file: &mut File) -> Option<f64> {
    let len = file.metadata().ok()?.len();
    let moov = mp4read::read_moov_with(file, mp4read::may_start_a_movie)?;
    let mvex = child(&moov, b"mvex")?;
    let trak = boxes(&moov).map(|(_, b)| b).find(|t| {
        child(t, b"mdia")
            .and_then(|m| child(m, b"hdlr"))
            .and_then(|h| h.get(8..12))
            == Some(b"vide")
    })?;
    let tkhd = child(trak, b"tkhd")?;
    let track = be32(tkhd, if *tkhd.first()? == 1 { 20 } else { 12 })?;
    let scale = timescale(child(child(trak, b"mdia")?, b"mdhd")?)?;
    let default = boxes(mvex)
        .filter(|(kind, _)| kind == b"trex")
        .find(|(_, trex)| be32(trex, 4) == Some(track))
        .and_then(|(_, trex)| be32(trex, 12))
        .unwrap_or(0);
    let mut ticks = 0u64;
    let mut pos = 0u64;
    while pos < len {
        let mut head = Vec::with_capacity(16);
        file.seek(SeekFrom::Start(pos)).ok()?;
        file.by_ref().take(16).read_to_end(&mut head).ok()?;
        let (size, header) = match be32(&head, 0)? {
            0 => (len - pos, 8),
            1 => (be64(&head, 8)?, 16),
            n => (u64::from(n), 8),
        };
        if size < header || head.len() < 8 {
            break;
        }
        if &head[4..8] == b"moof" && size <= 16 << 20 {
            let mut moof = vec![0u8; (size - header) as usize];
            file.seek(SeekFrom::Start(pos + header)).ok()?;
            file.read_exact(&mut moof).ok()?;
            for (_, traf) in boxes(&moof).filter(|(kind, _)| kind == b"traf") {
                ticks = ticks.saturating_add(traf_ticks(traf, track, default).unwrap_or(0));
            }
        }
        pos = pos.checked_add(size)?;
    }
    (ticks > 0).then(|| ticks as f64 / scale as f64)
}

/// The sample durations a track fragment of `track` adds up to.
fn traf_ticks(traf: &[u8], track: u32, trex_default: u32) -> Option<u64> {
    let tfhd = child(traf, b"tfhd")?;
    if be32(tfhd, 4)? != track {
        return Some(0);
    }
    let flags = be32(tfhd, 0)? & 0xFF_FFFF;
    // The base offset and sample description index come before the default
    // duration.
    let mut field = 8;
    for (flag, width) in [(0x1, 8), (0x2, 4)] {
        if flags & flag != 0 {
            field += width;
        }
    }
    let default = if flags & 0x8 != 0 {
        be32(tfhd, field)?
    } else {
        trex_default
    };
    let mut ticks = 0u64;
    for (_, trun) in boxes(traf).filter(|(kind, _)| kind == b"trun") {
        let flags = be32(trun, 0)? & 0xFF_FFFF;
        let count = u64::from(be32(trun, 4)?);
        if flags & 0x100 == 0 {
            ticks = ticks.saturating_add(count.saturating_mul(u64::from(default)));
            continue;
        }
        let mut field = 8;
        for flag in [0x1, 0x4] {
            if flags & flag != 0 {
                field += 4;
            }
        }
        let stride = [0x100, 0x200, 0x400, 0x800]
            .into_iter()
            .filter(|f| flags & f != 0)
            .count()
            * 4;
        for i in 0..count as usize {
            ticks = ticks.saturating_add(u64::from(be32(trun, field + i * stride)?));
        }
    }
    Some(ticks)
}

/// Where the sample data of a movie fragment ends, by its `trun`s: `moof`
/// is the box's body and `at` where the box starts in the file. `defaults`
/// pairs each track with its `trex` sample size. A track fragment whose
/// boxes don't add up is passed over.
pub(crate) fn moof_data_end(moof: &[u8], at: u64, defaults: &[(u32, u32)]) -> Option<u64> {
    let mut furthest: Option<u64> = None;
    // Without a base of its own or default-base-is-moof, a track fragment's
    // data follows the one before it.
    let mut previous = at;
    for (kind, traf) in boxes(moof) {
        if &kind != b"traf" {
            continue;
        }
        if let Some(end) = traf_data_end(traf, at, previous, defaults) {
            previous = end;
            furthest = Some(furthest.map_or(end, |f| f.max(end)));
        }
    }
    furthest
}

fn traf_data_end(traf: &[u8], moof: u64, previous: u64, defaults: &[(u32, u32)]) -> Option<u64> {
    let tfhd = child(traf, b"tfhd")?;
    let flags = be32(tfhd, 0)? & 0xFF_FFFF;
    let track = be32(tfhd, 4)?;
    let mut field = 8;
    let base = if flags & 0x1 != 0 {
        field += 8;
        be64(tfhd, 8)?
    } else if flags & 0x2_0000 != 0 {
        moof
    } else {
        previous
    };
    // The sample description index and default duration come before the
    // default size.
    for flag in [0x2, 0x8] {
        if flags & flag != 0 {
            field += 4;
        }
    }
    let default = if flags & 0x10 != 0 {
        be32(tfhd, field)
    } else {
        defaults
            .iter()
            .find(|(t, _)| *t == track)
            .map(|(_, size)| *size)
    };
    let mut next = base;
    let mut end = base;
    for (kind, trun) in boxes(traf) {
        if &kind != b"trun" {
            continue;
        }
        let flags = be32(trun, 0)? & 0xFF_FFFF;
        let count = be32(trun, 4)? as usize;
        let mut field = 8;
        let start = if flags & 0x1 != 0 {
            field += 4;
            base.checked_add_signed(i64::from(be32(trun, 8)? as i32))?
        } else {
            next
        };
        if flags & 0x4 != 0 {
            field += 4;
        }
        // Each sample's fields, those present: duration, size, flags, time
        // offset.
        let present: Vec<u32> = [0x100, 0x200, 0x400, 0x800]
            .into_iter()
            .filter(|f| flags & f != 0)
            .collect();
        let bytes = if flags & 0x200 != 0 {
            let stride = present.len() * 4;
            let size_at = present.iter().position(|&f| f == 0x200)? * 4;
            let mut sum = 0u64;
            for i in 0..count {
                let size = be32(trun, field + i * stride + size_at)?;
                sum = sum.checked_add(u64::from(size))?;
            }
            sum
        } else {
            u64::from(default?).checked_mul(count as u64)?
        };
        next = start.checked_add(bytes)?;
        end = end.max(next);
    }
    Some(end)
}

/// The channels an AAC AudioSpecificConfig's channel configuration gives;
/// `None` for configuration 0 (the layout is in a program config element)
/// or one this doesn't know.
pub(crate) fn asc_channels(asc: &[u8]) -> Option<u16> {
    let mut r = info::Bits::new(asc);
    if r.u(5)? == 31 {
        r.skip(6)?; // the object type's extension
    }
    if r.u(4)? == 15 {
        r.skip(24)?; // an explicit sampling frequency
    }
    match r.u(4)? {
        c @ 1..=6 => Some(c as u16),
        7 | 12 | 14 => Some(8),
        11 => Some(7),
        13 => Some(24),
        _ => None,
    }
}

/// The codec and private data of an MP4 sample entry of type `fourcc`,
/// whose boxes are `inner`.
fn mp4_codec(fourcc: &[u8; 4], inner: &[u8]) -> (Tag, Vec<u8>) {
    let body = |kind: &[u8; 4]| child(inner, kind).map(<[u8]>::to_vec).unwrap_or_default();
    match fourcc {
        b"avc1" | b"avc3" => (Tag::H264, body(b"avcC")),
        b"hvc1" | b"hev1" | b"dvh1" | b"dvhe" => (Tag::Hevc, body(b"hvcC")),
        b"av01" => (Tag::Av1, body(b"av1C")),
        b"vp09" => (Tag::Vp9, body(b"vpcC")),
        b"mp4a" => {
            let esds = child(inner, b"esds")
                .or_else(|| child(child(inner, b"wave")?, b"esds"))
                .and_then(esds_config);
            match esds {
                Some((0x40 | 0x66 | 0x67 | 0x68, asc)) => (Tag::Aac, asc),
                Some((0x69 | 0x6B, _)) => (Tag::Other("MP3".into()), Vec::new()),
                Some((0xA5, _)) => (Tag::Ac3, Vec::new()),
                Some((0xA6, _)) => (Tag::Other("E-AC-3".into()), Vec::new()),
                Some((0xA9..=0xAC, _)) => (Tag::Other("DTS".into()), Vec::new()),
                _ => (Tag::Other("an unknown format".into()), Vec::new()),
            }
        }
        b"Opus" => (
            Tag::Opus,
            child(inner, b"dOps")
                .and_then(opus_head_from_dops)
                .unwrap_or_default(),
        ),
        b"fLaC" => (
            Tag::Flac,
            child(inner, b"dfLa")
                .and_then(|d| d.get(4..))
                .filter(|blocks| !blocks.is_empty())
                .map(|blocks| [b"fLaC".as_slice(), blocks].concat())
                .unwrap_or_default(),
        ),
        b"ac-3" => (Tag::Ac3, Vec::new()),
        b"ec-3" => (Tag::Other("E-AC-3".into()), Vec::new()),
        b"dtsc" | b"dtsh" | b"dtsl" | b"dtse" | b"dtsx" => (Tag::Other("DTS".into()), Vec::new()),
        b"alac" => (Tag::Other("ALAC".into()), Vec::new()),
        b".mp3" => (Tag::Other("MP3".into()), Vec::new()),
        b"lpcm" | b"sowt" | b"twos" | b"in24" | b"in32" | b"fl32" | b"fl64" | b"ipcm" | b"fpcm"
        | b"raw " => (Tag::Other("PCM".into()), Vec::new()),
        other => (
            Tag::Other(String::from_utf8_lossy(other).trim().to_string()),
            Vec::new(),
        ),
    }
}

/// An `elst` body's first edit with media in it, as Kiln honours an edit
/// list (R18): the empty edits before it (in the movie's time scale) and
/// the media time the presentation starts at (in the track's). `None` when
/// no edit has media.
///
/// This is the one model of an edit list, shared by the reader and by
/// [`VideoInfo::lead_in`](super::info::VideoInfo::lead_in) (R75). Media
/// Foundation never applies an edit list itself: the reader's byte stream
/// reads every `edts` box as `free` ([`edit_list_boxes`]). Left to it,
/// Media Foundation applies a list of more than one entry its own way: it
/// takes the first media time off every sample and gives those before it
/// as 0, leaves the empty edits out, and with two media edits drops the
/// samples before the edit, key frame and all. A list of one edit it
/// ignores.
pub(crate) fn first_edit(elst: &[u8]) -> Option<(u64, i64)> {
    let long = *elst.first()? == 1;
    let count = be32(elst, 4)? as usize;
    let size = if long { 20 } else { 12 };
    let entries = count.min(elst.len().saturating_sub(8) / size);
    let mut empty = 0u64;
    for i in 0..entries {
        let at = 8 + i * size;
        let (duration, media_time) = if long {
            (be64(elst, at)?, be64(elst, at + 8)? as i64)
        } else {
            (be32(elst, at)? as u64, be32(elst, at + 4)? as i32 as i64)
        };
        if media_time == -1 {
            empty = empty.saturating_add(duration);
            continue;
        }
        return Some((empty, media_time.max(0)));
    }
    None
}

/// What the reader moves a track's times by, in 100 ns, for its `elst`
/// body ([`first_edit`]): the empty edits later, the media time earlier.
/// `None` when it has no edit with media in it.
pub(crate) fn edit_shift(elst: &[u8], movie_scale: u32, media_scale: u32) -> Option<i64> {
    let (empty, media_time) = first_edit(elst)?;
    Some(hns(empty, movie_scale) - hns(media_time as u64, media_scale))
}

/// Where a track's presentation starts for its `elst` body, in 100 ns: the
/// empty edits before its first media edit.
pub(crate) fn edit_start(elst: &[u8], movie_scale: u32) -> i64 {
    first_edit(elst).map_or(0, |(empty, _)| hns(empty, movie_scale))
}

/// Where each track's `edts` box has its type in an MP4 or MOV file whose
/// `moov` body, `moov`, starts at `at`: for the byte stream to read as
/// `free` (see [`first_edit`]).
pub(crate) fn edit_list_boxes(at: u64, moov: &[u8]) -> Vec<u64> {
    let base = moov.as_ptr() as usize;
    let mut out = Vec::new();
    for (kind, trak) in boxes(moov) {
        if &kind != b"trak" {
            continue;
        }
        for (kind, body) in boxes(trak) {
            if &kind != b"edts" {
                continue;
            }
            // The type comes 4 bytes before the body, or 12 after a 64-bit
            // size.
            let body_at = body.as_ptr() as usize - base;
            let named = [4, 12].into_iter().find(|&back| {
                body_at >= back && moov.get(body_at - back..body_at - back + 4) == Some(b"edts")
            });
            if let Some(back) = named {
                out.push(at + (body_at - back) as u64);
            }
        }
    }
    out
}

/// An MPEG-4 descriptor at the start of `b`: its tag, its body, and the
/// length of the whole.
fn descriptor(b: &[u8]) -> Option<(u8, &[u8], usize)> {
    let tag = *b.first()?;
    let mut len = 0usize;
    let mut at = 1;
    loop {
        let x = *b.get(at)?;
        at += 1;
        len = (len << 7) | (x & 0x7F) as usize;
        if x & 0x80 == 0 || at > 4 {
            break;
        }
    }
    let body = b.get(at..at.checked_add(len)?)?;
    Some((tag, body, at + len))
}

/// The object type and the decoder-specific info (the AAC
/// AudioSpecificConfig) of an `esds` body.
pub(crate) fn esds_config(esds: &[u8]) -> Option<(u8, Vec<u8>)> {
    let (tag, es, _) = descriptor(esds.get(4..)?)?;
    if tag != 3 {
        return None;
    }
    let flags = *es.get(2)?;
    let mut at = 3;
    if flags & 0x80 != 0 {
        at += 2; // dependsOn_ES_ID
    }
    if flags & 0x40 != 0 {
        at += 1 + *es.get(at)? as usize; // URL
    }
    if flags & 0x20 != 0 {
        at += 2; // OCR_ES_ID
    }
    let (tag, config, _) = descriptor(es.get(at..)?)?;
    if tag != 4 {
        return None;
    }
    let object = *config.first()?;
    let specific = config
        .get(13..)
        .and_then(descriptor)
        .filter(|(t, _, _)| *t == 5)
        .map(|(_, b, _)| b.to_vec())
        .unwrap_or_default();
    Some((object, specific))
}

/// The `OpusHead` an MP4's `dOps` box stands for: the same fields, with the
/// multi-byte ones little-endian.
pub(crate) fn opus_head_from_dops(d: &[u8]) -> Option<Vec<u8>> {
    let channels = *d.get(1)?;
    let pre_skip = be16(d, 2)?;
    let rate = be32(d, 4)?;
    let gain = be16(d, 8)?;
    let family = *d.get(10)?;
    let mut h = b"OpusHead".to_vec();
    h.push(1);
    h.push(channels);
    h.extend(pre_skip.to_le_bytes());
    h.extend(rate.to_le_bytes());
    h.extend(gain.to_le_bytes());
    h.push(family);
    if family != 0 {
        // Stream count, coupled count, and the channel mapping.
        h.extend_from_slice(d.get(11..13 + channels as usize)?);
    }
    Some(h)
}

fn mkv_facts(h: &mkvread::Headers) -> Facts {
    let mut f = Facts {
        matroska: true,
        ..Facts::default()
    };
    let delay =
        |t: &mkvread::Track| -(t.codec_delay.unwrap_or(0).min(i64::MAX as u64) as i64 / 100);
    if let Some(t) = h.tracks.iter().find(|t| t.kind == mkvread::TYPE_VIDEO) {
        f.mkv_picture = Some(MkvPicture {
            number: t.number,
            scale: h.timestamp_scale,
            default_duration: t.default_duration,
        });
        let tag = match t.codec_id.as_str() {
            "V_MPEG4/ISO/AVC" => Tag::H264,
            "V_MPEGH/ISO/HEVC" => Tag::Hevc,
            "V_AV1" => Tag::Av1,
            "V_VP9" => Tag::Vp9,
            id => Tag::Other(id.trim_start_matches("V_").to_string()),
        };
        let v = t.video.as_ref();
        f.video = Some(Track {
            tag,
            private: t.codec_private.clone(),
            shift: delay(t),
            start: 0,
            clock: None,
            mastering: v.and_then(|v| v.mastering).map(mdcv_from_matroska),
            colour: v.and_then(matroska_colour),
            content_light: v.and_then(|v| {
                let cll = u16::try_from(v.max_cll?).ok()?;
                let fall = u16::try_from(v.max_fall.unwrap_or(0)).ok()?;
                let mut b = [0u8; 4];
                b[..2].copy_from_slice(&cll.to_be_bytes());
                b[2..].copy_from_slice(&fall.to_be_bytes());
                Some(b)
            }),
            ..Track::of(Tag::H264)
        });
    }
    if let Some(t) = h.tracks.iter().find(|t| t.kind == mkvread::TYPE_AUDIO) {
        let id = t.codec_id.as_str();
        let tag = match id {
            _ if id.starts_with("A_AAC") => Tag::Aac,
            "A_OPUS" => Tag::Opus,
            "A_VORBIS" => Tag::Vorbis,
            "A_FLAC" => Tag::Flac,
            "A_AC3" => Tag::Ac3,
            "A_EAC3" => Tag::Other("E-AC-3".into()),
            "A_TRUEHD" => Tag::Other("TrueHD".into()),
            "A_ALAC" => Tag::Other("ALAC".into()),
            "A_MPEG/L3" => Tag::Other("MP3".into()),
            "A_MPEG/L2" => Tag::Other("MP2".into()),
            _ if id.starts_with("A_DTS") => Tag::Other("DTS".into()),
            _ if id.starts_with("A_PCM") => Tag::Other("PCM".into()),
            _ => Tag::Other(id.trim_start_matches("A_").to_string()),
        };
        let shift = match (&tag, t.codec_delay) {
            // Without CodecDelay, the pre-skip, as the writer adds it back.
            (Tag::Opus, None) => t
                .codec_private
                .get(10..12)
                .filter(|_| t.codec_private.starts_with(b"OpusHead"))
                .map_or(0, |b| -hns(u16::from_le_bytes([b[0], b[1]]) as u64, 48_000)),
            _ => delay(t),
        };
        let (rate, channels) = t.audio.unwrap_or((0.0, 0));
        let channels = if tag == Tag::Aac {
            asc_channels(&t.codec_private)
        } else {
            u16::try_from(channels).ok().filter(|&c| c > 0)
        };
        f.sound = Some(Track {
            private: t.codec_private.clone(),
            shift,
            rate: (1.0..1e7).contains(&rate).then(|| rate.round() as u32),
            channels,
            ..Track::of(tag)
        });
    }
    f
}

/// An `mdcv` body from Matroska's mastering values (red, green, blue, white
/// x and y; luminance max and min in nits).
pub(crate) fn mdcv_from_matroska(m: [f64; 10]) -> [u8; 24] {
    let chroma = |v: f64| ((v * 50_000.0).round().clamp(0.0, 65_535.0) as u16).to_be_bytes();
    let lum = |v: f64| ((v * 10_000.0).round().clamp(0.0, u32::MAX as f64) as u32).to_be_bytes();
    let mut b = [0u8; 24];
    // mdcv lists green, blue, red, then the white point.
    for (slot, i) in [2usize, 3, 4, 5, 0, 1, 6, 7].iter().enumerate() {
        b[slot * 2..slot * 2 + 2].copy_from_slice(&chroma(m[*i]));
    }
    b[16..20].copy_from_slice(&lum(m[8]));
    b[20..24].copy_from_slice(&lum(m[9]));
    b
}

// ---------------------------------------------------------------------------
// H.264 and HEVC: Annex B, length prefixes, configuration records

/// True when `data` starts with an Annex B start code.
pub(crate) fn is_annex_b(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1])
}

/// The NAL units of an Annex B stream, without start codes or the zero
/// bytes before them.
pub(crate) fn annex_b_units(data: &[u8]) -> Vec<&[u8]> {
    fn close<'a>(data: &'a [u8], from: usize, to: usize, units: &mut Vec<&'a [u8]>) {
        let mut end = to;
        while end > from && data[end - 1] == 0 {
            end -= 1;
        }
        if end > from {
            units.push(&data[from..end]);
        }
    }
    let mut units = Vec::new();
    let mut start: Option<usize> = None;
    let mut i = 0;
    let n = data.len();
    while i + 3 <= n {
        if data[i + 2] > 1 {
            i += 3;
        } else if data[i + 2] == 1 && data[i + 1] == 0 && data[i] == 0 {
            if let Some(s) = start {
                close(data, s, i, &mut units);
            }
            i += 3;
            start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        close(data, s, n, &mut units);
    }
    units
}

/// The NAL units of a sample with `size`-byte lengths. `None` unless the
/// lengths add up to the sample.
pub(crate) fn prefixed_units(data: &[u8], size: usize) -> Option<Vec<&[u8]>> {
    let mut units = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let len = data
            .get(at..at + size)?
            .iter()
            .fold(0usize, |n, &b| (n << 8) | b as usize);
        let from = at + size;
        units.push(data.get(from..from.checked_add(len)?)?);
        at = from + len;
    }
    Some(units)
}

/// NAL units with 4-byte lengths.
pub(crate) fn length_prefixed(units: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.iter().map(|u| u.len() + 4).sum());
    for u in units {
        out.extend_from_slice(&(u.len() as u32).to_be_bytes());
        out.extend_from_slice(u);
    }
    out
}

/// The `sei_message()`s of an HEVC SEI NAL unit (with its 2-byte header), as
/// (payload type, payload). `None` when they don't add up to the unit.
pub(crate) fn sei_messages(nal: &[u8]) -> Option<Vec<(usize, Vec<u8>)>> {
    let rbsp = info::unescape(nal.get(2..)?);
    let mut b = rbsp.as_slice();
    let mut out = Vec::new();
    // The type and the size each come in runs of 255; the payloads end with
    // the RBSP's trailing bits (0x80).
    let number = |b: &mut &[u8]| -> Option<usize> {
        let mut n = 0usize;
        loop {
            let x = *b.first()?;
            *b = &b[1..];
            n += x as usize;
            if x != 0xFF {
                return Some(n);
            }
        }
    };
    while !b.is_empty() && b != [0x80] {
        let kind = number(&mut b)?;
        let size = number(&mut b)?;
        out.push((kind, b.get(..size)?.to_vec()));
        b = &b[size..];
    }
    Some(out)
}

/// How a stream's samples come from Media Foundation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    AnnexB,
    /// Length-prefixed, with lengths of this many bytes.
    Prefixed(usize),
}

/// Puts Media Foundation's H.264 and HEVC samples back as the file holds
/// them, with 4-byte lengths:
/// - Media Foundation repeats the record's parameter sets in front of every
///   key frame. Units equal to the record's are taken out, until the stream
///   brings a set the record doesn't hold; from then on nothing is taken out
///   (as the MP4 writer does), so a stream whose sets change keeps them all.
/// - Reading an MP4, MOV or M4V, in front of an HEVC key frame with HDR10
///   metadata, right after those sets, it adds a prefix SEI of its own,
///   made from the stream's facts: content light level (144) and mastering
///   display (137) messages. A prefix SEI in that place whose messages are
///   all of those two kinds and say exactly what the stream's facts say is
///   taken out too; the file's own copies sit after it and stay. From a
///   Matroska no SEI is ever taken out: there the file's own can sit right
///   after the sets, and an extra copy is harmless where losing the file's
///   is not.
/// - Annex B or length-prefixed is judged once, on the stream's first sample.
#[derive(Debug)]
pub(crate) struct NalCleaner {
    hevc: bool,
    sets: Vec<Vec<u8>>,
    /// The NAL length size of samples that come length-prefixed: the
    /// container's own.
    size: usize,
    framing: Option<Framing>,
    stripping: bool,
    /// The SEI payloads the stream's facts hold: (137, mdcv), (144, clli).
    hdr: Vec<(usize, Vec<u8>)>,
    /// True for an MP4, MOV or M4V source, where Media Foundation's own SEI
    /// is taken out.
    strip_sei: bool,
    /// For an encoded stream: the colour every SPS is made to say (see
    /// [`with_colour`](NalCleaner::with_colour)).
    colour: Option<[u8; 3]>,
}

impl NalCleaner {
    /// For a stream whose record is `record` (the one the muxer gets) and
    /// whose container's own record is `container` (for the length size).
    /// `hdr` is the stream's HDR facts as SEI payloads, and `strip_sei` says
    /// the source is an MP4, MOV or M4V.
    pub(crate) fn new(
        hevc: bool,
        record: &[u8],
        container: &[u8],
        hdr: Vec<(usize, Vec<u8>)>,
        strip_sei: bool,
    ) -> NalCleaner {
        let tag = if hevc { Tag::Hevc } else { Tag::H264 };
        let (sets, _) = config_sets(&tag, record);
        let (_, size) = config_sets(&tag, container);
        NalCleaner {
            hevc,
            sets,
            size,
            framing: None,
            stripping: true,
            hdr,
            strip_sei,
            colour: None,
        }
    }

    /// For a stream Kiln encoded: every SPS in its samples is made to say
    /// `colour` (primaries, transfer, matrix) in its VUI, as the record's
    /// were (see [`annex_b_with_colour`]), so the in-band copies stay equal
    /// to them.
    pub(crate) fn with_colour(mut self, colour: Option<[u8; 3]>) -> NalCleaner {
        self.colour = colour;
        self
    }

    /// True for a parameter set: SPS or PPS (7, 8); VPS, SPS or PPS (32-34).
    fn is_set(&self, u: &[u8]) -> bool {
        match u.first() {
            Some(&b) if self.hevc => (32..=34).contains(&((b >> 1) & 0x3F)),
            Some(&b) => matches!(b & 0x1F, 7 | 8),
            None => false,
        }
    }

    /// True for the SEI Media Foundation makes up (see above).
    fn is_added_sei(&self, u: &[u8]) -> bool {
        if u.first().is_none_or(|b| (b >> 1) & 0x3F != 39) {
            return false;
        }
        match sei_messages(u) {
            Some(m) if !m.is_empty() => m
                .iter()
                .all(|(kind, body)| self.hdr.iter().any(|(k, b)| k == kind && b == body)),
            _ => false,
        }
    }

    /// One sample, cleaned. A sample that can't be split is given as it is.
    pub(crate) fn clean(&mut self, data: Vec<u8>, key: bool) -> Vec<u8> {
        let framing = *self.framing.get_or_insert(if is_annex_b(&data) {
            Framing::AnnexB
        } else {
            Framing::Prefixed(self.size)
        });
        let units = match framing {
            Framing::AnnexB => annex_b_units(&data),
            Framing::Prefixed(n) => match prefixed_units(&data, n) {
                Some(u) => u,
                None => return data,
            },
        };
        if units.is_empty() {
            return data;
        }
        let recoloured: Vec<Vec<u8>>;
        let units = match self.colour {
            Some(c) if units.iter().any(|u| is_sps(self.hevc, u)) => {
                recoloured = units
                    .iter()
                    .map(|u| sps_coloured(self.hevc, u, c))
                    .collect();
                recoloured.iter().map(Vec::as_slice).collect()
            }
            _ => units,
        };
        if self.stripping
            && units
                .iter()
                .any(|u| self.is_set(u) && !self.sets.iter().any(|s| s == u))
        {
            self.stripping = false;
        }
        if !self.stripping {
            return length_prefixed(&units);
        }
        let lead = units.iter().take_while(|u| self.is_set(u)).count();
        let kept: Vec<&[u8]> = units
            .iter()
            .enumerate()
            .filter(|&(i, u)| {
                let added_sei = self.strip_sei
                    && self.hevc
                    && key
                    && lead > 0
                    && i == lead
                    && self.is_added_sei(u);
                !self.is_set(u) && !added_sei
            })
            .map(|(_, u)| *u)
            .collect();
        if kept.is_empty() {
            length_prefixed(&units)
        } else {
            length_prefixed(&kept)
        }
    }
}

/// True for an SPS: NAL unit type 7 (H.264) or 33 (HEVC).
fn is_sps(hevc: bool, u: &[u8]) -> bool {
    match u.first() {
        Some(&b) if hevc => (b >> 1) & 0x3F == 33,
        Some(&b) => b & 0x1F == 7,
        None => false,
    }
}

/// `u` when it isn't an SPS; an SPS saying `colour` (see
/// [`info::sps_with_colour`]), or as it was when it can't be read.
fn sps_coloured(hevc: bool, u: &[u8], colour: [u8; 3]) -> Vec<u8> {
    if is_sps(hevc, u) {
        if let Some(sps) = info::sps_with_colour(u, hevc, colour) {
            return sps;
        }
    }
    u.to_vec()
}

/// Annex B data (an encoder's sequence header or packet) with every SPS
/// saying `colour` (primaries, transfer, matrix) in its VUI, each unit
/// after a 4-byte start code. Data that isn't Annex B comes back as it is.
pub(crate) fn annex_b_with_colour(data: &[u8], hevc: bool, colour: [u8; 3]) -> Vec<u8> {
    if !is_annex_b(data) {
        return data.to_vec();
    }
    let mut out = Vec::with_capacity(data.len() + 16);
    for u in annex_b_units(data) {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend(sps_coloured(hevc, u, colour));
    }
    out
}

/// An AV1 key frame without the sequence header Media Foundation puts in
/// front of it from the `av1C` (`config`, its configOBUs), when the sample
/// carries its own right after.
pub(crate) fn without_added_header<'a>(data: &'a [u8], config: &[u8]) -> &'a [u8] {
    match data.strip_prefix(config) {
        // OBU type 1 is a sequence header.
        Some(rest) if !config.is_empty() && rest.first().is_some_and(|b| (b >> 3) & 0xF == 1) => {
            rest
        }
        _ => data,
    }
}

/// The parameter sets an `avcC` (SPS, PPS) or `hvcC` (VPS, SPS, PPS) holds,
/// and the size of the NAL lengths it says the samples use.
pub(crate) fn config_sets(codec: &Tag, cfg: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let mut sets = Vec::new();
    let mut read = || -> Option<usize> {
        match codec {
            Tag::H264 => {
                let size = (*cfg.get(4)? & 3) as usize + 1;
                let mut at = 6;
                let sps = (*cfg.get(5)? & 0x1F) as usize;
                for _ in 0..sps {
                    let len = be16(cfg, at)? as usize;
                    sets.push(cfg.get(at + 2..at + 2 + len)?.to_vec());
                    at += 2 + len;
                }
                let pps = *cfg.get(at)? as usize;
                at += 1;
                for _ in 0..pps {
                    let len = be16(cfg, at)? as usize;
                    sets.push(cfg.get(at + 2..at + 2 + len)?.to_vec());
                    at += 2 + len;
                }
                Some(size)
            }
            Tag::Hevc => {
                let size = (*cfg.get(21)? & 3) as usize + 1;
                let mut at = 23;
                for _ in 0..*cfg.get(22)? {
                    let kind = *cfg.get(at)? & 0x3F;
                    let count = be16(cfg, at + 1)?;
                    at += 3;
                    for _ in 0..count {
                        let len = be16(cfg, at)? as usize;
                        if (32..=34).contains(&kind) {
                            sets.push(cfg.get(at + 2..at + 2 + len)?.to_vec());
                        }
                        at += 2 + len;
                    }
                }
                Some(size)
            }
            _ => None,
        }
    };
    let size = read().unwrap_or(4);
    (sets, size)
}

/// The configuration record with its NAL length size set to 4 bytes.
fn with_four_byte_lengths(codec: &Tag, mut cfg: Vec<u8>) -> Vec<u8> {
    match codec {
        Tag::H264 if cfg.len() > 4 => cfg[4] |= 3,
        Tag::Hevc if cfg.len() > 21 => cfg[21] |= 3,
        _ => {}
    }
    cfg
}

/// An `avcC` built from an Annex B sequence header (SPS and PPS).
pub(crate) fn avcc_from_annex_b(seq: &[u8]) -> Option<Vec<u8>> {
    let units = annex_b_units(seq);
    let of = |t: u8| -> Vec<&[u8]> {
        units
            .iter()
            .copied()
            .filter(|u| u.first().is_some_and(|b| b & 0x1F == t))
            .collect()
    };
    let (sps, pps, ext) = (of(7), of(8), of(13));
    let first = sps.first().filter(|s| s.len() >= 4)?;
    if pps.is_empty() || sps.len() > 31 || pps.len() > 255 {
        return None;
    }
    let mut out = vec![
        1,
        first[1],
        first[2],
        first[3],
        0xFF,
        0xE0 | sps.len() as u8,
    ];
    for s in &sps {
        out.extend((s.len() as u16).to_be_bytes());
        out.extend_from_slice(s);
    }
    out.push(pps.len() as u8);
    for p in &pps {
        out.extend((p.len() as u16).to_be_bytes());
        out.extend_from_slice(p);
    }
    if info::H264_HIGH.contains(&first[1]) {
        let (chroma, luma, chroma_depth) = info::h264_sps_format(first).unwrap_or((1, 0, 0));
        out.push(0xFC | (chroma & 3) as u8);
        out.push(0xF8 | (luma & 7) as u8);
        out.push(0xF8 | (chroma_depth & 7) as u8);
        out.push(ext.len().min(255) as u8);
        for e in ext.iter().take(255) {
            out.extend((e.len() as u16).to_be_bytes());
            out.extend_from_slice(e);
        }
    }
    Some(out)
}

/// An `hvcC` built from an Annex B sequence header (VPS, SPS and PPS).
pub(crate) fn hvcc_from_annex_b(seq: &[u8]) -> Option<Vec<u8>> {
    let units = annex_b_units(seq);
    let of = |t: u8| -> Vec<&[u8]> {
        units
            .iter()
            .copied()
            .filter(|u| u.first().is_some_and(|b| (b >> 1) & 0x3F == t))
            .collect()
    };
    let arrays = [(32u8, of(32)), (33, of(33)), (34, of(34))];
    let sps = arrays[1].1.first()?;
    let head = info::hevc_sps_format(sps)?;
    if arrays.iter().any(|(_, v)| v.is_empty()) {
        return None;
    }
    let mut out = vec![1];
    out.extend_from_slice(&head.general);
    out.extend([0xF0, 0x00]); // min_spatial_segmentation_idc 0
    out.push(0xFC); // parallelismType unknown
    out.push(0xFC | (head.chroma_format_idc & 3) as u8);
    out.push(0xF8 | (head.bit_depth_luma_minus8 & 7) as u8);
    out.push(0xF8 | (head.bit_depth_chroma_minus8 & 7) as u8);
    out.extend([0, 0]); // avgFrameRate unknown
    let layers = (head.max_sub_layers_minus1 as u8 + 1) & 7;
    out.push((layers << 3) | ((head.temporal_id_nesting as u8) << 2) | 3);
    out.push(arrays.len() as u8);
    for (kind, list) in &arrays {
        out.push(0x80 | kind); // array_completeness
        out.extend((list.len() as u16).to_be_bytes());
        for u in list {
            out.extend((u.len() as u16).to_be_bytes());
            out.extend_from_slice(u);
        }
    }
    Some(out)
}

/// The static HDR metadata in an `hvcC`'s prefix SEI messages, as an `mdcv`
/// body (mastering display colour volume, payload 137, laid out the same)
/// and a `clli` body (content light level, payload 144). An encoder such as
/// x265 puts them there when the MP4 has no `mdcv` or `clli` box.
pub(crate) fn hdr_from_hvcc(hvcc: &[u8]) -> (Option<[u8; 24]>, Option<[u8; 4]>) {
    let mut mastering = None;
    let mut light = None;
    let mut at = 23;
    let mut walk = || -> Option<()> {
        for _ in 0..*hvcc.get(22)? {
            let kind = *hvcc.get(at)? & 0x3F;
            let count = be16(hvcc, at + 1)?;
            at += 3;
            for _ in 0..count {
                let len = be16(hvcc, at)? as usize;
                let nal = hvcc.get(at + 2..at + 2 + len)?;
                at += 2 + len;
                if kind != 39 {
                    continue;
                }
                for (payload, body) in sei_messages(nal).unwrap_or_default() {
                    match (payload, body.len()) {
                        (137, 24) => mastering = body.try_into().ok(),
                        (144, 4) => light = body.try_into().ok(),
                        _ => {}
                    }
                }
            }
        }
        Some(())
    };
    let _ = walk();
    (mastering, light)
}

/// A `vpcC` (version 1) for a VP9 stream whose container gives none, from
/// what the container says of the picture.
fn vpcc_for(info: &VideoInfo) -> Vec<u8> {
    let depth = info.bit_depth.clamp(8, 12);
    let (primaries, transfer, matrix) = match info.hdr {
        Hdr::Pq => (9, 16, 9),
        Hdr::Hlg => (9, 18, 9),
        Hdr::None => (2, 2, 2),
    };
    let samples = info.width as u64 * info.height as u64;
    let level = [
        (36_864, 10),
        (73_728, 11),
        (122_880, 20),
        (245_760, 21),
        (552_960, 30),
        (983_040, 31),
        (2_228_224, 40),
        (8_912_896, 50),
        (35_651_584, 60),
    ]
    .iter()
    .find(|(max, _)| samples <= *max)
    .map_or(62, |(_, l)| *l);
    vec![
        1,
        0,
        0,
        0,
        if depth > 8 { 2 } else { 0 },
        level,
        (depth << 4) | (1 << 1), // 4:2:0 co-sited, limited range
        primaries,
        transfer,
        matrix,
        0,
        0,
    ]
}

/// The picture's stream, from the container's facts; Media Foundation's
/// sequence header stands in for a missing `avcC` or `hvcC`. `None` when
/// the codec isn't one a muxer takes, or its private data can't be found.
pub(crate) fn video_spec(t: &Track, info: &VideoInfo, mf_seq: Option<&[u8]>) -> Option<StreamSpec> {
    let valid = |p: &Vec<u8>, min: usize| p.len() >= min && p[0] == 1;
    let codec = match t.tag {
        Tag::H264 => {
            let cfg = if valid(&t.private, 7) {
                t.private.clone()
            } else {
                avcc_from_annex_b(mf_seq?)?
            };
            StreamCodec::H264 {
                avcc: with_four_byte_lengths(&t.tag, cfg),
            }
        }
        Tag::Hevc => {
            let cfg = if valid(&t.private, 23) {
                t.private.clone()
            } else {
                hvcc_from_annex_b(mf_seq?)?
            };
            StreamCodec::Hevc {
                hvcc: with_four_byte_lengths(&t.tag, cfg),
            }
        }
        Tag::Av1 => {
            if t.private.len() < 4 || t.private[0] != 0x81 {
                return None;
            }
            StreamCodec::Av1 {
                av1c: t.private.clone(),
            }
        }
        Tag::Vp9 => StreamCodec::Vp9 {
            vpcc: if t.private.len() >= 12 {
                t.private.clone()
            } else {
                vpcc_for(info)
            },
        },
        _ => return None,
    };
    // The container's boxes first, then an hvcC's SEI.
    let (mastering, content_light) = match &codec {
        StreamCodec::Hevc { hvcc } => {
            let (m, l) = hdr_from_hvcc(hvcc);
            (t.mastering.or(m), t.content_light.or(l))
        }
        _ => (t.mastering, t.content_light),
    };
    Some(StreamSpec {
        codec,
        timescale: TIMESCALE,
        video: Some(VideoParams {
            width: info.width,
            height: info.height,
            rotation: info.rotation,
            hdr: info.hdr,
            bit_depth: info.bit_depth,
            mastering,
            content_light,
            sar: info.sar,
            tag_colour: false,
            bt601: false,
            frame_ns: info.frame_ns(),
            // The source's own description, as FFmpeg's copy keeps it.
            stated_colour: t.colour,
        }),
        audio: None,
    })
}

/// The sound's stream, or the name of its codec when the reader can't give
/// its packets. `mf_user` is Media Foundation's `MF_MT_USER_DATA`, which
/// stands in for a missing AudioSpecificConfig or `OpusHead`.
pub(crate) fn sound_spec(
    t: &Track,
    rate: u32,
    channels: u16,
    mf_user: Option<&[u8]>,
) -> std::result::Result<StreamSpec, String> {
    let missing = || t.tag.name();
    let given = |p: &Vec<u8>| (!p.is_empty()).then(|| p.clone());
    let codec = match t.tag {
        Tag::Aac => StreamCodec::Aac {
            // MF_MT_USER_DATA is the tail of a HEAACWAVEINFO (12 bytes), then
            // the AudioSpecificConfig.
            asc: given(&t.private)
                .or_else(|| {
                    mf_user
                        .and_then(|u| u.get(12..))
                        .filter(|a| !a.is_empty())
                        .map(<[u8]>::to_vec)
                })
                .ok_or_else(missing)?,
        },
        Tag::Opus => StreamCodec::Opus {
            head: Some(t.private.clone())
                .filter(|p| p.starts_with(b"OpusHead"))
                .or_else(|| {
                    mf_user
                        .filter(|u| u.starts_with(b"OpusHead"))
                        .map(<[u8]>::to_vec)
                })
                .ok_or_else(missing)?,
        },
        Tag::Vorbis => StreamCodec::Vorbis {
            private: given(&t.private).ok_or_else(missing)?,
        },
        Tag::Flac => StreamCodec::Flac {
            streaminfo: given(&t.private).ok_or_else(missing)?,
        },
        Tag::Ac3 => StreamCodec::Ac3,
        _ => return Err(missing()),
    };
    if rate == 0 || channels == 0 {
        return Err(missing());
    }
    Ok(StreamSpec {
        codec,
        timescale: TIMESCALE,
        video: None,
        audio: Some(AudioParams { rate, channels }),
    })
}

/// True when a sound track's rate and channels, as its container gives
/// them, fit the stream Media Foundation lists: the same channels, and the
/// same rate or one twice or half of it (HE-AAC's core rate and its output
/// rate). What the container doesn't give doesn't count against it.
pub(crate) fn sound_matches(t: &Track, rate: u32, channels: u16) -> bool {
    let channels_fit = t.channels.is_none_or(|c| c == channels);
    let rate_fit = t
        .rate
        .is_none_or(|r| r == rate || r == rate.saturating_mul(2) || r.saturating_mul(2) == rate);
    channels_fit && rate_fit
}

/// The container's name for the picture's codec, when it has no facts of
/// its own for the track.
fn tag_of(codec: &VideoCodec) -> Tag {
    match codec {
        VideoCodec::H264 => Tag::H264,
        VideoCodec::Hevc => Tag::Hevc,
        VideoCodec::Av1 => Tag::Av1,
        VideoCodec::Vp9 => Tag::Vp9,
        VideoCodec::Mpeg4 => Tag::Other("MPEG-4".into()),
        VideoCodec::Mjpeg => Tag::Other("MJPEG".into()),
        VideoCodec::Other(n) => Tag::Other(n.clone()),
    }
}

// ---------------------------------------------------------------------------
// Decode times

/// Packets read ahead before the reorder depth is judged, from the first
/// half of them.
const WINDOW: usize = 32;
/// Packets held back once it is: more than the deepest reorder seen in
/// practice (H.264 allows 16).
const LOOKAHEAD: usize = WINDOW / 2;

/// Gives the picture's packets their decode times: the sorted presentation
/// times, shifted back by the reorder depth. The depth is the most any packet
/// comes later in decode order than in presentation order, which is the
/// least shift that never decodes a picture after it is shown (2 for x264's
/// B-pyramid, as its MP4s say). The first `depth` packets go before the
/// first presentation time, a frame apart.
///
/// The depth is first judged from the stream's first packets. Each time a
/// packet is given out, those held behind it are looked at again; if one
/// would need a deeper shift, the depth grows by one there (that packet's
/// decode time goes just after the last one, and the rest follow one place
/// further back). Should a picture still decode after it shows, the stream
/// is marked broken.
#[derive(Debug, Default)]
pub(crate) struct Reorder {
    queue: VecDeque<Packet>,
    /// Presentation times not yet given out as a decode time.
    heap: BinaryHeap<Reverse<i64>>,
    /// The depth; `None` until the first packets have been judged.
    depth: Option<usize>,
    /// The packets that go before the first presentation time.
    initial: usize,
    first: i64,
    step: i64,
    /// Packets given out, and presentation times taken from the heap.
    given: usize,
    popped: usize,
    last: Option<i64>,
    /// True when the front packet's decode time is settled.
    front_ready: bool,
    broken: bool,
}

impl Reorder {
    pub(crate) fn push(&mut self, p: Packet) {
        self.heap.push(Reverse(p.pts));
        self.queue.push_back(p);
    }

    /// Packets waiting.
    pub(crate) fn len(&self) -> usize {
        self.queue.len()
    }

    /// True once a picture would have had to decode after it shows.
    pub(crate) fn broken(&self) -> bool {
        self.broken
    }

    /// The decode time of the front packet, once it can be settled: with
    /// enough packets read past it, or at the end (`ended`).
    pub(crate) fn ready(&mut self, ended: bool) -> Option<i64> {
        if self.front_ready {
            return self.queue.front().map(|p| p.dts);
        }
        if self.queue.is_empty() {
            return None;
        }
        if self.depth.is_none() {
            if self.queue.len() < WINDOW && !ended {
                return None;
            }
            self.settle();
        }
        if self.queue.len() <= LOOKAHEAD && !ended {
            return None;
        }
        let pts = self.queue.front()?.pts;
        let mut dts = if self.given < self.initial {
            self.first - (self.initial - self.given) as i64 * self.step
        } else if self.lagging() {
            self.depth = self.depth.map(|d| d + 1);
            self.last.map_or(self.first - self.step, |l| l + 1)
        } else {
            self.popped += 1;
            self.heap.pop().map_or(self.first, |Reverse(t)| t)
        };
        if let Some(last) = self.last {
            // Decode times only go forward.
            dts = dts.max(last + 1);
        }
        if dts > pts {
            self.broken = true;
        }
        self.given += 1;
        self.last = Some(dts);
        self.front_ready = true;
        let front = self.queue.front_mut()?;
        front.dts = dts;
        Some(dts)
    }

    /// True when a packet held would decode after it shows at the depth so
    /// far: its place in decode order is further behind its place in
    /// presentation order than the depth.
    fn lagging(&self) -> bool {
        let depth = self.depth.unwrap_or(0);
        self.queue.iter().enumerate().any(|(q, p)| {
            let rank = self.popped + self.heap.iter().filter(|r| r.0 < p.pts).count();
            (self.given + q).saturating_sub(rank) > depth
        })
    }

    /// The front packet, once [`Reorder::ready`] has settled it.
    pub(crate) fn pop(&mut self) -> Option<Packet> {
        if !std::mem::take(&mut self.front_ready) {
            return None;
        }
        self.queue.pop_front()
    }

    /// Judges the reorder depth from the packets read so far.
    fn settle(&mut self) {
        let mut sorted: Vec<i64> = self.queue.iter().map(|p| p.pts).collect();
        sorted.sort_unstable();
        let judged = if self.queue.len() >= WINDOW {
            LOOKAHEAD
        } else {
            self.queue.len()
        };
        let depth = self
            .queue
            .iter()
            .take(judged)
            .enumerate()
            .map(|(i, p)| i.saturating_sub(sorted.partition_point(|&s| s < p.pts)))
            .max()
            .unwrap_or(0);
        self.depth = Some(depth);
        self.initial = depth;
        self.first = sorted.first().copied().unwrap_or(0);
        self.step = sorted
            .windows(2)
            .map(|w| w[1] - w[0])
            .filter(|&d| d > 0)
            .min()
            .or_else(|| self.queue.front().map(|p| p.duration).filter(|&d| d > 0))
            .unwrap_or(1);
    }
}

// ---------------------------------------------------------------------------
// The reader

#[cfg(windows)]
pub use win::{missing_extension, Frame, FrameFormat, Reader};

#[cfg(windows)]
mod win {
    use super::*;
    use crate::mf::com::{self, get_blob, get_u32, Mf};
    use crate::video::block_clock::{self, BlockClock, Timed};
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Media::MediaFoundation::*;

    const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
    const ANY: u32 = MF_SOURCE_READER_ANY_STREAM.0 as u32;
    /// Sound packets held while the picture's decode times settle, at most.
    const SOUND_HOLD: usize = 4096;
    /// Picture packets held while waiting for sound, at most.
    const VIDEO_HOLD: usize = LOOKAHEAD + 256;

    /// One decoded picture. `pts` and `duration` are in 100 ns, the edit
    /// list applied; the sample's own time is set to the same.
    ///
    /// With a [`D3d`], the sample's buffer is a texture on that card
    /// (`IMFDXGIBuffer`) from the decoder's own small pool. Never hold more
    /// than [`MAX_HELD_FRAMES`] of them at once (the next read would then
    /// wait for ever): run each through the video processor, whose output is
    /// a texture of its own, and drop the frame before encoding that output.
    pub struct Frame {
        pub sample: IMFSample,
        pub pts: i64,
        pub duration: i64,
        /// What the frame is: the decoder's output type when it was given,
        /// which the video processor takes as its input type.
        pub format: FrameFormat,
    }

    // SAFETY: Media Foundation samples are free-threaded objects, used from
    // the multithreaded apartment.
    unsafe impl Send for Frame {}

    impl Frame {
        /// The brightness of the picture (`format.shown`), copied out of a
        /// frame decoded in memory (a reader opened with no [`D3d`]). A frame
        /// on a graphics card can't be read this way.
        pub fn luma(&self) -> Result<Luma> {
            com::com();
            let f = &self.format;
            let shown = f.shown;
            let deep = f.pixel == Pixel::P010;
            let bytes = if deep { 2 } else { 1 };
            let fail = |e: windows::core::Error| com::failed("can't read the decoded frame", e);
            // SAFETY: the buffer is locked while its rows are copied out, each
            // within the pitch it gives and the frame's own size, then let go.
            unsafe {
                let buffer = self.sample.GetBufferByIndex(0).map_err(fail)?;
                let b: IMF2DBuffer = windows::core::Interface::cast(&buffer).map_err(fail)?;
                let mut p = std::ptr::null_mut();
                let mut pitch = 0i32;
                b.Lock2D(&mut p, &mut pitch).map_err(fail)?;
                let fits = shown.x.saturating_add(shown.width) <= f.width
                    && shown.y.saturating_add(shown.height) <= f.height
                    && pitch.unsigned_abs() as usize >= f.width as usize * bytes;
                if !fits || p.is_null() {
                    let _ = b.Unlock2D();
                    return Err(damaged());
                }
                let mut samples = Vec::with_capacity(shown.width as usize * shown.height as usize);
                for y in shown.y..shown.y + shown.height {
                    let row = p.offset(y as isize * pitch as isize);
                    let row = std::slice::from_raw_parts(row, f.width as usize * bytes);
                    let row =
                        &row[shown.x as usize * bytes..(shown.x + shown.width) as usize * bytes];
                    if deep {
                        // P010 keeps its 10 bits at the top of each 16.
                        samples.extend(
                            row.chunks_exact(2)
                                .map(|c| u16::from_le_bytes([c[0], c[1]]) >> 6),
                        );
                    } else {
                        samples.extend(row.iter().map(|&v| v as u16));
                    }
                }
                let _ = b.Unlock2D();
                Ok(Luma {
                    width: shown.width,
                    height: shown.height,
                    max: if deep { 1023 } else { 255 },
                    samples,
                })
            }
        }
    }

    /// What the decoded frames are: their layout and size, and the part of
    /// them that is picture. A frame can be bigger than the picture (an odd
    /// size is padded out, 885x1919 to 896x1920); the processor crops it to
    /// `shown`.
    #[derive(Clone)]
    pub struct FrameFormat {
        pub pixel: Pixel,
        /// The frame's size.
        pub width: u32,
        pub height: u32,
        /// The picture within it: the minimum display aperture, else the
        /// geometric aperture, else the whole frame.
        pub shown: Area,
        /// The decoder's output type, with its colour attributes.
        pub media_type: IMFMediaType,
    }

    impl std::fmt::Debug for FrameFormat {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FrameFormat")
                .field("pixel", &self.pixel)
                .field("width", &self.width)
                .field("height", &self.height)
                .field("shown", &self.shown)
                .finish()
        }
    }

    /// How one of Media Foundation's streams becomes packets or frames.
    struct Lane {
        /// Media Foundation's index of the stream.
        mf: u32,
        /// The stream's place in `streams`.
        ours: usize,
        video: bool,
        shift: i64,
        /// Where the track's presentation starts (see [`Track::start`]).
        start: i64,
        /// For H.264 and HEVC: puts the samples back as the file has them.
        nal: Option<NalCleaner>,
        /// For AV1: the `av1C`'s configOBUs.
        av1_header: Option<Vec<u8>>,
        /// The track's own clock, for MP4 (see [`Track::clock`]).
        clock: Option<u32>,
        /// A Matroska FLAC track's first sample is its header.
        skip_flac_header: bool,
        /// AAC that comes with ADTS headers.
        adts: bool,
        /// Each frame's length, when the container says the rate is constant.
        fixed_duration: Option<i64>,
        /// Where the next sample starts, for one that has no time.
        next: i64,
        ended: bool,
    }

    impl Lane {
        fn new(mf: u32, ours: usize, t: &Track) -> Lane {
            Lane {
                mf,
                ours,
                video: ours == 0,
                shift: t.shift,
                start: t.start,
                nal: None,
                av1_header: None,
                clock: t.clock,
                skip_flac_header: false,
                adts: false,
                fixed_duration: None,
                next: 0,
                ended: false,
            }
        }

        /// A sample's time and length (from Media Foundation, or made up
        /// when it has none) as the stream's: on the track's own clock, and
        /// moved by the shift.
        fn times(&mut self, s: &IMFSample) -> (i64, i64) {
            // SAFETY: plain reads from a sample the reader gave.
            let (time, length) = unsafe { (s.GetSampleTime().ok(), s.GetSampleDuration().ok()) };
            let mut time = time.unwrap_or(self.next);
            let duration = self.fixed_duration.or(length).unwrap_or(0).max(0);
            self.next = time.saturating_add(duration);
            if let Some(clock) = self.clock {
                time = rescale(rescale(time, TIMESCALE, clock), clock, TIMESCALE);
            }
            (time.saturating_add(self.shift), duration)
        }
    }

    /// Closes the byte stream, so the file goes as soon as the reader does.
    struct Closer(IMFByteStream);

    impl Drop for Closer {
        fn drop(&mut self) {
            com::com();
            // SAFETY: a plain call on the stream; later calls get MF_E_SHUTDOWN.
            unsafe {
                let _ = self.0.Close();
            }
        }
    }

    /// A video file being read.
    pub struct Reader {
        reader: IMFSourceReader,
        // After the reader, so the source has let go of the stream by then.
        _closer: Closer,
        decoded: bool,
        d3d: Option<D3d>,
        info: VideoInfo,
        streams: Vec<ReadStream>,
        lanes: Vec<Lane>,
        sound_unavailable: Option<String>,
        video: Reorder,
        sound: VecDeque<Packet>,
        format: Option<FrameFormat>,
        /// A Matroska picture's block times, for its decoded frames (R78).
        /// The first frame is held until the second is read, while the
        /// caller has been given none, so it adds nothing to
        /// [`MAX_HELD_FRAMES`].
        clock: Option<BlockClock<IMFSample>>,
        /// Decoded frames timed and waiting to be given.
        timed: VecDeque<Timed<IMFSample>>,
        /// The end of the furthest packet or frame given out, in 100 ns.
        reached: i64,
        /// Every byte Media Foundation has read, what it had read when the
        /// reader was open, and the file's length: progress when the length
        /// in time is unknown.
        read: std::sync::Arc<std::sync::atomic::AtomicU64>,
        read_at_open: u64,
        file_len: u64,
        // Last, so Media Foundation outlives everything above.
        _mf: Mf,
    }

    // SAFETY: the reader and everything it holds belong to it alone and live
    // in the multithreaded apartment; every call first joins the calling
    // thread to it.
    unsafe impl Send for Reader {}

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// A failed read as the row should say it. The graphics card failing
    /// (DXGI and D3D11 errors, a lost device) or memory running out is no
    /// fault of the file; nor is the file becoming unreadable (a Windows
    /// error from the byte stream). Anything else is the file's.
    fn read_failed(e: windows::core::Error) -> MediaError {
        const E_OUTOFMEMORY: u32 = 0x8007_000E;
        let code = e.code().0 as u32;
        let facility = (code >> 16) & 0x1FFF;
        match facility {
            _ if code == E_OUTOFMEMORY => MediaError::Encoder(format!(
                "Windows ran out of memory reading the video ({code:#010X})"
            )),
            0x87A | 0x87C => MediaError::Encoder(format!(
                "the graphics card failed while decoding the video ({code:#010X})"
            )),
            7 => MediaError::Read(std::io::Error::from_raw_os_error((code & 0xFFFF) as i32)),
            _ => damaged(),
        }
    }

    /// The Store extension that decodes `codec`, when one does.
    fn extension(codec: &Tag) -> Option<(&'static str, &'static str)> {
        match codec {
            Tag::Hevc => Some(("HEVC", "9N4WGH0Z6VHQ")),
            Tag::Av1 => Some(("AV1", "9MVZQVXJBQ9V")),
            Tag::Vp9 => Some(("VP9", "9N4D0MSMP0PT")),
            _ => None,
        }
    }

    /// The Store extension (its codec name and product ID) a picture of
    /// `tag`, whose Media Foundation subtype is `subtype`, needs before
    /// Windows can decode it, when that extension isn't installed. `None`
    /// for a codec no extension reads, or when the decoder is there.
    fn extension_missing(tag: &Tag, subtype: &GUID) -> Option<(&'static str, &'static str)> {
        let (codec, product) = extension(tag)?;
        ((codec == "HEVC" && hevc_hidden()) || !has_decoder(subtype)).then_some((codec, product))
    }

    /// What [`Reader::open`] would refuse to decode `codec` for: the Store
    /// extension (its codec name, "HEVC", and product ID) Windows needs to
    /// decode it and lacks, by the reader's own check. `None` when the
    /// decoder is installed, for a codec no extension reads, and without
    /// Media Foundation (that is `NeedsMediaFoundation`, not an extension).
    /// Lists Media Foundation's decoders, so keep it off the window's thread.
    pub fn missing_extension(codec: &VideoCodec) -> Option<(&'static str, &'static str)> {
        let tag = tag_of(codec);
        let subtype = match tag {
            Tag::Hevc => MFVideoFormat_HEVC,
            Tag::Av1 => MFVideoFormat_AV1,
            Tag::Vp9 => MFVideoFormat_VP90,
            _ => return None,
        };
        let _mf = Mf::start().ok()?;
        extension_missing(&tag, &subtype)
    }

    /// True when, in a debug build, `KILN_HIDE_HEVC` asks to act as if the
    /// HEVC extension weren't installed.
    fn hevc_hidden() -> bool {
        #[cfg(debug_assertions)]
        if std::env::var_os("KILN_HIDE_HEVC").is_some() {
            return true;
        }
        false
    }

    /// True when Media Foundation lists a decoder for `subtype`.
    fn has_decoder(subtype: &GUID) -> bool {
        !com::enum_transforms(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_ALL | MFT_ENUM_FLAG_UNTRUSTED_STOREMFT,
            Some((MFMediaType_Video, *subtype)),
            None,
            None,
        )
        .is_empty()
    }

    fn guid(t: &IMFMediaType, key: &GUID) -> GUID {
        // SAFETY: a plain attribute read.
        unsafe { t.GetGUID(key) }.unwrap_or_default()
    }

    /// The `MFVideoArea` blob of `key`, when it holds a real area.
    fn area(t: &IMFMediaType, key: &GUID) -> Option<Area> {
        let b = get_blob(t, key)?;
        let x = i16::from_le_bytes(b.get(2..4)?.try_into().ok()?);
        let y = i16::from_le_bytes(b.get(6..8)?.try_into().ok()?);
        let w = i32::from_le_bytes(b.get(8..12)?.try_into().ok()?);
        let h = i32::from_le_bytes(b.get(12..16)?.try_into().ok()?);
        (x >= 0 && y >= 0 && w > 0 && h > 0).then_some(Area {
            x: x as u32,
            y: y as u32,
            width: w as u32,
            height: h as u32,
        })
    }

    fn read_format(reader: &IMFSourceReader, index: u32) -> Result<FrameFormat> {
        // SAFETY: plain calls on the reader and its type.
        let t = unsafe { reader.GetCurrentMediaType(index) }.map_err(|_| damaged())?;
        let size = unsafe { t.GetUINT64(&MF_MT_FRAME_SIZE) }.map_err(|_| damaged())?;
        let (width, height) = ((size >> 32) as u32, size as u32);
        if width == 0 || height == 0 {
            return Err(damaged());
        }
        let whole = Area {
            x: 0,
            y: 0,
            width,
            height,
        };
        let shown = area(&t, &MF_MT_MINIMUM_DISPLAY_APERTURE)
            .or_else(|| area(&t, &MF_MT_GEOMETRIC_APERTURE))
            .filter(|a| a.x + a.width <= width && a.y + a.height <= height)
            .unwrap_or(whole);
        Ok(FrameFormat {
            pixel: if guid(&t, &MF_MT_SUBTYPE) == MFVideoFormat_P010 {
                Pixel::P010
            } else {
                Pixel::Nv12
            },
            width,
            height,
            shown,
            media_type: t,
        })
    }

    impl Reader {
        /// Opens `file` to read in `mode`. The file is read through Kiln's
        /// byte stream and let go when the reader is dropped.
        ///
        /// Fails with `NoVideo` for a file with no picture, `Damaged` when
        /// it can't be read, and `NeedsMediaFoundation` without Media
        /// Foundation. To decode HEVC, AV1 or VP9 without the Store extension
        /// that does it, `NeedsExtension`. A picture codec no muxer takes
        /// can't be read compressed.
        ///
        /// Sound is read only compressed: the first sound stream, when its
        /// codec is AAC, Opus, Vorbis, FLAC or AC-3 and Media Foundation
        /// hands it over. Otherwise the picture is still read, and
        /// [`Reader::sound_unavailable`] names the sound's codec.
        pub fn open(mut file: File, mode: ReadMode) -> Result<Reader> {
            let mf = Mf::start()?;
            let info = info::read(&mut file)?;
            let facts = facts(&mut file);
            if facts.truncated {
                // R20: judged from the container's structure alone.
                return Err(damaged());
            }
            let (decoded, d3d) = match mode {
                ReadMode::Compressed => (false, None),
                ReadMode::Decoded { d3d } => (true, d3d),
            };
            let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
            // R78: a Matroska picture's blocks, read with a handle of their
            // own, time its decoded frames.
            let blocks = match facts.mkv_picture {
                Some(m) if decoded => Some((
                    m,
                    com::reopen_for_reading(&file).and_then(|f| mkvread::Blocks::new(f, m.number)),
                )),
                _ => None,
            };
            // Media Foundation never sees an edit list (see `first_edit`).
            let hidden = facts.edit_lists.clone();
            let (stream, _, read) = com::byte_stream_hiding(file, false, hidden)?;
            let closer = Closer(stream.clone());
            let mime = if facts.matroska {
                "video/x-matroska"
            } else if facts.quicktime {
                "video/quicktime"
            } else {
                "video/mp4"
            };
            let mime = wide(mime);
            // SAFETY: plain attribute calls; the string is NUL-terminated and
            // copied; the reader keeps its own reference to the stream.
            let reader = unsafe {
                if let Ok(attrs) = windows::core::Interface::cast::<IMFAttributes>(&stream) {
                    let _ = attrs.SetString(&MF_BYTESTREAM_CONTENT_TYPE, PCWSTR(mime.as_ptr()));
                }
                let mut attrs = None;
                MFCreateAttributes(&mut attrs, 2).map_err(|_| MediaError::NeedsMediaFoundation)?;
                let attrs = attrs.ok_or(MediaError::NeedsMediaFoundation)?;
                if let Some(d) = &d3d {
                    attrs
                        .SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, d.manager())
                        .and_then(|_| attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1))
                        .map_err(|e| {
                            com::failed("can't hand the graphics card to the reader", e)
                        })?;
                }
                MFCreateSourceReaderFromByteStream(&stream, &attrs).map_err(|_| damaged())?
            };

            // The first picture and sound streams Media Foundation lists.
            let mut picture = None;
            let mut sound = None;
            for i in 0..256u32 {
                // SAFETY: plain calls on the reader.
                let Ok(t) = (unsafe { reader.GetNativeMediaType(i, 0) }) else {
                    break;
                };
                let major = guid(&t, &MF_MT_MAJOR_TYPE);
                if major == MFMediaType_Video && picture.is_none() {
                    picture = Some((i, t));
                } else if major == MFMediaType_Audio && sound.is_none() {
                    sound = Some((i, t));
                }
            }
            let (vi, vtype) = picture.ok_or(MediaError::NoVideo)?;
            // SAFETY: a plain call on the reader.
            unsafe { reader.SetStreamSelection(ALL, false) }.map_err(|_| damaged())?;

            let vtrack = facts
                .video
                .clone()
                .unwrap_or_else(|| Track::of(tag_of(&info.codec)));
            let seq = get_blob(&vtype, &MF_MT_MPEG_SEQUENCE_HEADER);
            let vspec = video_spec(&vtrack, &info, seq.as_deref());
            let fixed_duration = info
                .fps
                .filter(|_| facts.matroska && !info.variable_rate)
                .map(|fps| (TIMESCALE as f64 / fps).round() as i64);
            let mut streams = Vec::new();
            let clock = blocks.map(|(m, blocks)| {
                BlockClock::new(
                    blocks.map(|b| Box::new(b) as block_clock::Blocks),
                    m.scale,
                    m.default_duration,
                    vtrack.shift,
                    fixed_duration.is_some(),
                )
            });
            let mut lanes = vec![Lane {
                fixed_duration,
                ..Lane::new(vi, 0, &vtrack)
            }];
            let mut sound_unavailable = None;
            let mut format = None;

            if decoded {
                let subtype = guid(&vtype, &MF_MT_SUBTYPE);
                if let Some((codec, product)) = extension_missing(&vtrack.tag, &subtype) {
                    return Err(MediaError::NeedsExtension { codec, product });
                }
                let set = |out: &GUID| -> windows::core::Result<()> {
                    // SAFETY: plain calls making a partial type and giving it
                    // to the reader.
                    unsafe {
                        let t = MFCreateMediaType()?;
                        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                        t.SetGUID(&MF_MT_SUBTYPE, out)?;
                        reader.SetCurrentMediaType(vi, None, &t)
                    }
                };
                // SAFETY: a plain call on the reader.
                unsafe { reader.SetStreamSelection(vi, true) }.map_err(|_| damaged())?;
                let deep = info.bit_depth > 8 && set(&MFVideoFormat_P010).is_ok();
                if !deep && set(&MFVideoFormat_NV12).is_err() {
                    return Err(if has_decoder(&subtype) {
                        damaged()
                    } else if vtrack.tag == Tag::H264 {
                        MediaError::NeedsMediaFoundation
                    } else {
                        MediaError::NoDecoder {
                            codec: vtrack.tag.name().to_string(),
                        }
                    });
                }
                format = Some(read_format(&reader, vi)?);
                if let Some(spec) = vspec {
                    streams.push(ReadStream { index: 0, spec });
                }
            } else {
                let Some(spec) = vspec else {
                    return Err(match vtrack.tag {
                        Tag::Other(n) => MediaError::Encoder(format!("{n} video can't be copied")),
                        _ => damaged(),
                    });
                };
                let mut hdr = Vec::new();
                if let Some(v) = &spec.video {
                    hdr.extend(v.mastering.map(|m| (137, m.to_vec())));
                    hdr.extend(v.content_light.map(|l| (144, l.to_vec())));
                }
                match &spec.codec {
                    StreamCodec::H264 { avcc: r } => {
                        lanes[0].nal = Some(NalCleaner::new(
                            false,
                            r,
                            &vtrack.private,
                            hdr,
                            !facts.matroska,
                        ));
                    }
                    StreamCodec::Hevc { hvcc: r } => {
                        lanes[0].nal = Some(NalCleaner::new(
                            true,
                            r,
                            &vtrack.private,
                            hdr,
                            !facts.matroska,
                        ));
                    }
                    StreamCodec::Av1 { av1c } => {
                        lanes[0].av1_header = av1c.get(4..).map(<[u8]>::to_vec);
                    }
                    _ => {}
                }
                streams.push(ReadStream { index: 0, spec });
                // SAFETY: plain calls on the reader with a type it gave.
                unsafe {
                    reader
                        .SetStreamSelection(vi, true)
                        .and_then(|_| reader.SetCurrentMediaType(vi, None, &vtype))
                        .map_err(|_| damaged())?;
                }

                match (sound, &facts.sound) {
                    (Some((ai, atype)), container) => {
                        let strack = container
                            .clone()
                            .unwrap_or_else(|| Track::of(Tag::Other("an unknown format".into())));
                        let rate = get_u32(&atype, &MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0);
                        let channels =
                            get_u32(&atype, &MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0) as u16;
                        let user = get_blob(&atype, &MF_MT_USER_DATA);
                        // The container's first sound track must be the
                        // stream Media Foundation lists first; if their rate
                        // or channels disagree, the private data can't be
                        // paired with it.
                        let paired = sound_matches(&strack, rate, channels);
                        let spec = if paired {
                            sound_spec(&strack, rate, channels, user.as_deref())
                        } else {
                            Err(strack.tag.name())
                        };
                        match spec {
                            Ok(spec) => {
                                // SAFETY: plain calls on the reader with a
                                // type it gave.
                                let selected = unsafe {
                                    reader
                                        .SetStreamSelection(ai, true)
                                        .and_then(|_| reader.SetCurrentMediaType(ai, None, &atype))
                                };
                                if selected.is_ok() {
                                    lanes.push(Lane {
                                        skip_flac_header: facts.matroska && strack.tag == Tag::Flac,
                                        adts: strack.tag == Tag::Aac
                                            && get_u32(&atype, &MF_MT_AAC_PAYLOAD_TYPE) == Some(1),
                                        ..Lane::new(ai, 1, &strack)
                                    });
                                    streams.push(ReadStream { index: 1, spec });
                                } else {
                                    // SAFETY: a plain call on the reader.
                                    let _ = unsafe { reader.SetStreamSelection(ai, false) };
                                    sound_unavailable = Some(strack.tag.name());
                                }
                            }
                            Err(name) => sound_unavailable = Some(name),
                        }
                    }
                    (None, Some(t)) => sound_unavailable = Some(t.tag.name()),
                    (None, None) => {}
                }
            }

            Ok(Reader {
                reader,
                _closer: closer,
                decoded,
                d3d,
                info,
                streams,
                lanes,
                sound_unavailable,
                video: Reorder::default(),
                sound: VecDeque::new(),
                format,
                clock,
                timed: VecDeque::new(),
                reached: 0,
                read_at_open: read.load(std::sync::atomic::Ordering::Relaxed),
                read,
                file_len,
                _mf: mf,
            })
        }

        /// The streams read: the picture first, then the first sound stream
        /// (compressed only). Opened to decode, the picture is listed when
        /// its codec is one the muxers take.
        pub fn streams(&self) -> &[ReadStream] {
            &self.streams
        }

        /// Where stream `index`'s presentation starts, in 100 ns: an MP4
        /// edit list's empty edits, else 0. What comes before it is hidden;
        /// decoded frames that end by then aren't given.
        pub fn start(&self, index: usize) -> i64 {
            self.lanes
                .iter()
                .find(|l| l.ours == index)
                .map_or(0, |l| l.start)
        }

        /// The picture's facts from the container: size, rotation, frame
        /// rate, length and the rest. `sound` is left empty.
        pub fn info(&self) -> &VideoInfo {
            &self.info
        }

        /// True when a Matroska picture's decoded frames didn't line up one
        /// for one with its blocks (or its blocks couldn't time them), so
        /// from where they parted the frames keep Media Foundation's times;
        /// or when every frame lined up but the blocks outnumber them (R78).
        /// Known for certain once the last frame has been given.
        pub fn decoder_times(&self) -> bool {
            self.clock.as_ref().is_some_and(|c| c.decoder_times())
        }

        /// The codec of a sound stream the reader can't give packets of
        /// ("DTS"), when the file has one. The picture is read all the same.
        pub fn sound_unavailable(&self) -> Option<&str> {
            self.sound_unavailable.as_deref()
        }

        /// The graphics card the decoder runs on, when there is one.
        pub fn d3d(&self) -> Option<&D3d> {
            self.d3d.as_ref()
        }

        /// What the decoded frames are, as of the last one given out.
        /// `None` when reading compressed.
        pub fn frame_format(&self) -> Option<&FrameFormat> {
            self.format.as_ref()
        }

        /// How far into the file the reading has got, in seconds.
        pub fn position(&self) -> f64 {
            self.reached.max(0) as f64 / TIMESCALE as f64
        }

        /// How much of the file Media Foundation has read since the reader
        /// opened, as a share of its length, from 0 to 1: the samples, read
        /// in order. (Opening a fragmented MP4, it looks through every
        /// fragment first.) `None` when the file's length is unknown.
        pub fn bytes_fraction(&self) -> Option<f64> {
            let read = self.read.load(std::sync::atomic::Ordering::Relaxed);
            let since = read.saturating_sub(self.read_at_open);
            (self.file_len > 0).then(|| (since as f64 / self.file_len as f64).min(1.0))
        }

        fn note(&mut self, end: i64) {
            self.reached = self.reached.max(end);
        }

        /// The next packet, in decode order across the streams. `None` at
        /// the end. Only when reading compressed.
        pub fn next_packet(&mut self) -> Result<Option<Packet>> {
            if self.decoded {
                return Err(MediaError::Encoder(
                    "the video reader was opened for frames, not packets".into(),
                ));
            }
            com::com();
            loop {
                let video_ended = self.lanes[0].ended;
                let sound_ended = self.lanes.get(1).is_none_or(|l| l.ended);
                let v = self.video.ready(video_ended);
                let a = self.sound.front().map(|p| p.dts);
                let take_video = match (v, a) {
                    (Some(v), Some(a)) => Some(v <= a),
                    (Some(_), None) => {
                        (sound_ended || self.video.len() > VIDEO_HOLD).then_some(true)
                    }
                    (None, Some(_)) => {
                        (video_ended || self.sound.len() > SOUND_HOLD).then_some(false)
                    }
                    (None, None) if video_ended && sound_ended => return Ok(None),
                    (None, None) => None,
                };
                if self.video.broken() {
                    return Err(damaged());
                }
                let p = match take_video {
                    Some(true) => self.video.pop(),
                    Some(false) => self.sound.pop_front(),
                    None => {
                        self.read_one()?;
                        continue;
                    }
                };
                if let Some(p) = p {
                    self.note(p.pts.saturating_add(p.duration.max(0)));
                    return Ok(Some(p));
                }
            }
        }

        /// Reads one sample from any stream into the queues.
        fn read_one(&mut self) -> Result<()> {
            let mut index = 0u32;
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            // SAFETY: a synchronous read; the out pointers are locals.
            unsafe {
                self.reader.ReadSample(
                    ANY,
                    0,
                    Some(&mut index),
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                )
            }
            .map_err(|e| self.failed(e))?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(self.flagged());
            }
            let Some(lane) = self.lanes.iter().position(|l| l.mf == index) else {
                return Ok(());
            };
            if let Some(s) = sample {
                if let Some(p) = self.packet(lane, &s)? {
                    if self.lanes[lane].video {
                        self.video.push(p);
                    } else {
                        self.sound.push_back(p);
                    }
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                self.lanes[lane].ended = true;
            }
            Ok(())
        }

        /// A sample of `lane` as a packet (its decode time still to come for
        /// the picture). `None` for one that isn't a packet of the stream.
        fn packet(&mut self, lane: usize, s: &IMFSample) -> Result<Option<Packet>> {
            let l = &mut self.lanes[lane];
            let (pts, duration) = l.times(s);
            let key = !l.video || get_u32(s, &MFSampleExtension_CleanPoint) == Some(1);
            let data = com::sample_bytes(s).map_err(|_| damaged())?;
            if l.skip_flac_header && data.starts_with(b"fLaC") {
                return Ok(None);
            }
            let data = match &mut l.nal {
                Some(nal) => nal.clean(data, key),
                None if key && l.av1_header.is_some() => {
                    let header = l.av1_header.as_deref().unwrap_or_default();
                    without_added_header(&data, header).to_vec()
                }
                None if l.adts && data.len() > 9 && data[0] == 0xFF && data[1] & 0xF6 == 0xF0 => {
                    let head = if data[1] & 1 == 1 { 7 } else { 9 };
                    data[head..].to_vec()
                }
                None => data,
            };
            if data.is_empty() {
                return Ok(None);
            }
            Ok(Some(Packet {
                stream: l.ours,
                pts,
                dts: pts,
                duration,
                key,
                data,
            }))
        }

        /// What a read that failed with `e` should say: a lost graphics
        /// card first, as the decoder's own error then means little.
        fn failed(&self, e: windows::core::Error) -> MediaError {
            match self.device_lost() {
                Some(lost) => read_failed(lost),
                None => read_failed(e),
            }
        }

        /// What a read Media Foundation flags as failed (with no error of
        /// its own to give, as a synchronous read has none) should say: the
        /// graphics card's removal reason when it was lost, else damaged.
        fn flagged(&self) -> MediaError {
            self.device_lost().map_or_else(damaged, read_failed)
        }

        /// Why the graphics card's device was lost, if it was.
        fn device_lost(&self) -> Option<windows::core::Error> {
            // SAFETY: a plain query on the device.
            self.d3d
                .as_ref()
                .and_then(|d| unsafe { d.device().GetDeviceRemovedReason() }.err())
        }

        /// The next decoded picture, in presentation order. `None` at the
        /// end. Only when reading decoded.
        pub fn next_frame(&mut self) -> Result<Option<Frame>> {
            if !self.decoded {
                return Err(MediaError::Encoder(
                    "the video reader was opened for packets, not frames".into(),
                ));
            }
            com::com();
            loop {
                if let Some((s, pts, duration)) = self.timed.pop_front() {
                    if pts.saturating_add(duration.max(1)) <= self.lanes[0].start {
                        // Before the start the edit list presents.
                        continue;
                    }
                    // SAFETY: plain calls on a sample that is ours now.
                    unsafe {
                        let _ = s.SetSampleTime(pts);
                        let _ = s.SetSampleDuration(duration);
                    }
                    self.note(pts.saturating_add(duration));
                    let format = self.format.clone().ok_or_else(damaged)?;
                    return Ok(Some(Frame {
                        sample: s,
                        pts,
                        duration,
                        format,
                    }));
                }
                if self.lanes[0].ended {
                    return Ok(None);
                }
                let vi = self.lanes[0].mf;
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                // SAFETY: a synchronous read; the out pointers are locals.
                unsafe {
                    self.reader
                        .ReadSample(vi, 0, None, Some(&mut flags), None, Some(&mut sample))
                }
                .map_err(|e| self.failed(e))?;
                if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                    return Err(self.flagged());
                }
                if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                    // The decoder settles its output on the first frame (and
                    // adds the apertures and colour); carry on with the new.
                    self.format = Some(read_format(&self.reader, vi)?);
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    self.lanes[0].ended = true;
                }
                if let Some(s) = sample {
                    let (pts, duration) = self.lanes[0].times(&s);
                    match &mut self.clock {
                        Some(c) => c.frame((s, pts, duration), &mut self.timed),
                        None => self.timed.push_back((s, pts, duration)),
                    }
                }
                if self.lanes[0].ended {
                    if let Some(c) = &mut self.clock {
                        c.end(&mut self.timed);
                    }
                }
            }
        }
        /// Moves a reader of decoded frames to `seconds` from the start of
        /// the picture, for a look at part of the file (the estimate's
        /// sample). The decoder starts again at the key frame at or before
        /// it, so the first frames given may be a little earlier; a caller
        /// that wants none before `seconds` skips them by their times. A
        /// Matroska picture's frames keep Media Foundation's times after a
        /// move (R78's block times are read from the start only).
        pub fn seek(&mut self, seconds: f64) -> Result<()> {
            use std::mem::ManuallyDrop;
            use windows::Win32::System::Com::StructuredStorage::{
                PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
            };
            use windows::Win32::System::Variant::VT_I8;
            if !self.decoded {
                return Err(MediaError::Encoder(
                    "the video reader was opened for packets, not frames".into(),
                ));
            }
            com::com();
            let lane = &self.lanes[0];
            // The file's own times: the presented ones less the track's shift.
            let ticks = ((seconds.max(0.0) * TIMESCALE as f64) as i64)
                .saturating_sub(lane.shift)
                .max(0);
            let at = PROPVARIANT {
                Anonymous: PROPVARIANT_0 {
                    Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                        vt: VT_I8,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: PROPVARIANT_0_0_0 { hVal: ticks },
                    }),
                },
            };
            // SAFETY: a plain call on the reader; the PROPVARIANT holds a
            // plain integer, so it needs no clearing, and lives across it.
            unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &at) }
                .map_err(|e| self.failed(e))?;
            self.timed.clear();
            self.clock = None;
            let lane = &mut self.lanes[0];
            lane.ended = false;
            lane.next = ticks;
            Ok(())
        }
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            com::com();
        }
    }
}

/// Off Windows there is no Media Foundation: nothing can be opened, so no
/// reader, frame or format is ever made. Every way in says
/// `NeedsMediaFoundation`.
#[cfg(not(windows))]
pub use other::{missing_extension, Frame, FrameFormat, Reader};

#[cfg(not(windows))]
mod other {
    use super::*;

    /// A decoded picture; there is never one off Windows.
    pub struct Frame {
        pub pts: i64,
        pub duration: i64,
        pub format: FrameFormat,
        _never: Never,
    }

    impl Frame {
        pub fn luma(&self) -> Result<Luma> {
            match self._never {}
        }
    }

    /// No extension is missing where nothing can be decoded at all.
    pub fn missing_extension(_codec: &VideoCodec) -> Option<(&'static str, &'static str)> {
        None
    }

    /// What decoded frames are; there is never one off Windows.
    #[derive(Debug, Clone)]
    pub struct FrameFormat {
        pub pixel: Pixel,
        pub width: u32,
        pub height: u32,
        pub shown: Area,
        _never: Never,
    }

    #[derive(Debug, Clone)]
    enum Never {}

    /// A video file being read; there is never one off Windows.
    pub enum Reader {}

    impl Reader {
        pub fn open(_file: File, _mode: ReadMode) -> Result<Reader> {
            Err(MediaError::NeedsMediaFoundation)
        }

        pub fn streams(&self) -> &[ReadStream] {
            match *self {}
        }

        pub fn start(&self, _index: usize) -> i64 {
            match *self {}
        }

        pub fn info(&self) -> &VideoInfo {
            match *self {}
        }

        pub fn sound_unavailable(&self) -> Option<&str> {
            match *self {}
        }

        pub fn d3d(&self) -> Option<&D3d> {
            match *self {}
        }

        pub fn frame_format(&self) -> Option<&FrameFormat> {
            match *self {}
        }

        pub fn position(&self) -> f64 {
            match *self {}
        }

        pub fn bytes_fraction(&self) -> Option<f64> {
            match *self {}
        }

        pub fn next_packet(&mut self) -> Result<Option<Packet>> {
            match *self {}
        }

        pub fn next_frame(&mut self) -> Result<Option<Frame>> {
            match *self {}
        }

        pub fn seek(&mut self, _seconds: f64) -> Result<()> {
            match *self {}
        }

        pub fn decoder_times(&self) -> bool {
            match *self {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(pts: i64) -> Packet {
        Packet {
            stream: 0,
            pts,
            dts: pts,
            duration: 10,
            key: false,
            data: vec![1],
        }
    }

    /// Runs `pts` (in decode order) through a Reorder, as the reader does.
    fn decode_times(pts: &[i64]) -> Vec<i64> {
        let mut r = Reorder::default();
        let mut out = Vec::new();
        for &p in pts {
            r.push(packet(p));
            while r.ready(false).is_some() {
                out.push(r.pop().unwrap().dts);
            }
        }
        while r.ready(true).is_some() {
            out.push(r.pop().unwrap().dts);
        }
        out
    }

    #[test]
    fn decode_times_are_the_sorted_times_held_back_by_the_depth() {
        // I P B B, repeated: FFmpeg's x264 order in the fixtures.
        let mut pts = Vec::new();
        for g in 0..20 {
            let b = g * 40;
            pts.extend([b, b + 30, b + 10, b + 20]);
        }
        let dts = decode_times(&pts);
        assert_eq!(dts.len(), pts.len());
        assert_eq!(&dts[..4], &[-10, 0, 10, 20]);
        for (d, p) in dts.iter().zip(&pts) {
            assert!(d <= p, "{d} > {p}");
        }
        assert!(dts.windows(2).all(|w| w[0] < w[1]));

        // A B-pyramid: I P B b b. Two frames back, as x264 has it.
        let mut pts = Vec::new();
        for g in 0..10 {
            let b = g * 50;
            pts.extend([b, b + 40, b + 20, b + 10, b + 30]);
        }
        let dts = decode_times(&pts);
        assert_eq!(&dts[..5], &[-20, -10, 0, 10, 20]);
        for (d, p) in dts.iter().zip(&pts) {
            assert!(d <= p, "{d} > {p}");
        }
        assert!(dts.windows(2).all(|w| w[0] < w[1]));

        // No reordering: decode times are presentation times.
        let pts: Vec<i64> = (0..50).map(|i| i * 7).collect();
        assert_eq!(decode_times(&pts), pts);
        // A short stream settles at its end.
        assert_eq!(decode_times(&[0, 20, 10]), vec![-10, 0, 10]);
        assert_eq!(decode_times(&[]), Vec::<i64>::new());
    }

    #[test]
    fn a_reorder_deeper_than_first_judged_grows_the_depth() {
        // Thirty pictures in order, then I P B B: the depth judged at the
        // start (0) is too shallow for what follows.
        let mut pts: Vec<i64> = (0..30).map(|i| i * 10).collect();
        for g in 0..10 {
            let b = 300 + g * 40;
            pts.extend([b, b + 30, b + 10, b + 20]);
        }
        let mut r = Reorder::default();
        let mut dts = Vec::new();
        for &p in &pts {
            r.push(packet(p));
            while r.ready(false).is_some() {
                dts.push(r.pop().unwrap().dts);
            }
        }
        while r.ready(true).is_some() {
            dts.push(r.pop().unwrap().dts);
        }
        assert!(!r.broken());
        assert_eq!(dts.len(), pts.len());
        for (i, (d, p)) in dts.iter().zip(&pts).enumerate() {
            assert!(d <= p, "packet {i}: dts {d} > pts {p}");
        }
        assert!(dts.windows(2).all(|w| w[0] < w[1]));
        // The start keeps its own times.
        assert_eq!(&dts[..5], &[0, 10, 20, 30, 40]);
    }

    /// An Annex B sample of these units.
    fn annex_b(units: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for u in units {
            out.extend([0, 0, 0, 1]);
            out.extend_from_slice(u);
        }
        out
    }

    fn prefixed(units: &[&[u8]]) -> Vec<u8> {
        length_prefixed(units)
    }

    #[test]
    fn annex_b_becomes_length_prefixed_without_the_repeated_sets() {
        let data = [
            &[0, 0, 0, 1, 0x67, 1, 2, 3][..],
            &[0, 0, 1, 0x68, 4],
            &[0, 0, 0, 1, 0x65, 9, 9, 0, 0],
            &[0, 0, 1, 0x06, 5],
        ]
        .concat();
        assert!(is_annex_b(&data));
        let units = annex_b_units(&data);
        assert_eq!(units.len(), 4);
        assert_eq!(units[2], &[0x65, 9, 9]);
        let out = prefixed(&units[2..]);
        assert_eq!(out, vec![0, 0, 0, 3, 0x65, 9, 9, 0, 0, 0, 2, 0x06, 5]);
        assert_eq!(
            prefixed_units(&out, 4).unwrap(),
            vec![&[0x65, 9, 9][..], &[0x06, 5][..]]
        );
        assert_eq!(prefixed_units(&out[..5], 4), None);
        assert!(annex_b_units(&[1, 2, 3]).is_empty());
    }

    /// A High profile SPS and a PPS, and the avcC that holds them.
    fn h264_record() -> ([u8; 6], [u8; 4], Vec<u8>) {
        let sps = [0x67, 100, 0, 13, 0b1010_1110, 0x80];
        let pps = [0x68, 0xEB, 0xE3, 0xCB];
        let avcc = avcc_from_annex_b(&annex_b(&[&sps, &pps])).unwrap();
        (sps, pps, avcc)
    }

    #[test]
    fn repeated_sets_come_out_until_a_foreign_one_comes() {
        let (a, pps, avcc) = h264_record();
        let mut c = NalCleaner::new(false, &avcc, &avcc, Vec::new(), true);
        let idr = [0x65, 0x88, 0x84];
        let slice = [0x41, 0x9A, 0x22];
        // A, as the record holds it: taken out.
        assert_eq!(c.clean(annex_b(&[&a, &pps, &idr]), true), prefixed(&[&idr]));
        assert_eq!(c.clean(annex_b(&[&slice]), false), prefixed(&[&slice]));
        // B, in the stream: kept, and from here on nothing is taken out.
        let b = [0x67, 100, 0, 13, 0b1010_1110, 0x81];
        assert_eq!(
            c.clean(annex_b(&[&b, &pps, &idr]), true),
            prefixed(&[&b, &pps, &idr])
        );
        // A again: it is the stream's, and stays.
        assert_eq!(
            c.clean(annex_b(&[&a, &pps, &idr]), true),
            prefixed(&[&a, &pps, &idr])
        );
        // A sample of only sets keeps them.
        let mut fresh = NalCleaner::new(false, &avcc, &avcc, Vec::new(), true);
        assert_eq!(
            fresh.clean(annex_b(&[&a, &pps]), true),
            prefixed(&[&a, &pps])
        );
    }

    #[test]
    fn emulation_prevention_bytes_pass_through() {
        let (a, pps, avcc) = h264_record();
        let mut c = NalCleaner::new(false, &avcc, &avcc, Vec::new(), true);
        // 00 00 03 inside a unit is the stream's own escape: kept as it is.
        let idr = [0x65, 0x00, 0x00, 0x03, 0x01, 0x00, 0x00, 0x03, 0x00, 0x07];
        assert_eq!(c.clean(annex_b(&[&a, &pps, &idr]), true), prefixed(&[&idr]));
        assert_eq!(annex_b_units(&annex_b(&[&idr])), vec![&idr[..]]);
    }

    #[test]
    fn the_framing_is_judged_once() {
        let (_, _, avcc) = h264_record();
        let mut c = NalCleaner::new(false, &avcc, &avcc, Vec::new(), true);
        let small = [0x65, 1, 2];
        assert_eq!(c.clean(prefixed(&[&small]), true), prefixed(&[&small]));
        // 260 bytes: its length, 00 00 01 04, looks like a start code, but
        // the stream came length-prefixed, so it is read that way.
        let mut big = vec![0x41];
        big.extend(std::iter::repeat_n(7u8, 259));
        let sample = prefixed(&[&big]);
        assert!(is_annex_b(&sample));
        assert_eq!(c.clean(sample.clone(), false), sample);
    }

    #[test]
    fn media_foundations_hdr_sei_after_the_sets_comes_back_out() {
        let vps = [0x40, 0x01, 0x0C];
        let sps = [0x42, 0x01, 0x01];
        let pps = [0x44, 0x01, 0xC1];
        let mut hvcc = vec![0u8; 21];
        hvcc.push(0xF3); // 4-byte lengths
        hvcc.push(3);
        for u in [&vps, &sps, &pps] {
            hvcc.push(0x80 | ((u[0] >> 1) & 0x3F));
            hvcc.extend(1u16.to_be_bytes());
            hvcc.extend((u.len() as u16).to_be_bytes());
            hvcc.extend_from_slice(u);
        }
        let light = vec![0x03, 0xE8, 0x01, 0x90];
        let hdr = vec![(144usize, light.clone())];
        let sei = |tid: u8, payload: u8, body: &[u8]| {
            let mut u = vec![0x4E, tid, payload, body.len() as u8];
            u.extend_from_slice(body);
            u.push(0x80);
            u
        };
        let added = sei(0x02, 144, &light);
        let own = sei(0x01, 144, &light);
        let slice = [0x28, 0x01, 0xAF];
        let mut c = NalCleaner::new(true, &hvcc, &hvcc, hdr.clone(), true);
        assert_eq!(
            c.clean(annex_b(&[&vps, &sps, &pps, &added, &own, &slice]), true),
            prefixed(&[&own, &slice])
        );
        // From a Matroska, no SEI is taken out: the file's own may sit
        // right after the sets.
        let mut mkv = NalCleaner::new(true, &hvcc, &hvcc, hdr.clone(), false);
        assert_eq!(
            mkv.clean(annex_b(&[&vps, &sps, &pps, &own, &slice]), true),
            prefixed(&[&own, &slice])
        );
        // Not right after the sets, or saying something else, or of another
        // kind, or not on a key frame: it stays.
        let other = sei(0x01, 144, &[0, 1, 0, 1]);
        let user = sei(0x01, 5, &[1, 2, 3]);
        for (units, key) in [
            (vec![&vps[..], &sps, &pps, &other, &slice], true),
            (vec![&vps[..], &sps, &pps, &user, &slice], true),
            (vec![&added[..], &slice], true),
            (vec![&vps[..], &sps, &pps, &added, &slice], false),
        ] {
            let mut c = NalCleaner::new(true, &hvcc, &hvcc, hdr.clone(), true);
            let kept: Vec<&[u8]> = units
                .iter()
                .copied()
                .filter(|u| u[0] >> 1 != 32 && u[0] >> 1 != 33 && u[0] >> 1 != 34)
                .collect();
            assert_eq!(c.clean(annex_b(&units), key), prefixed(&kept));
        }
        assert_eq!(sei_messages(&added), Some(vec![(144, light)]));
    }

    #[test]
    fn an_av1_key_frame_loses_the_header_put_in_front() {
        let config = [0x0A, 0x02, 0xAA, 0xBB];
        let sample = [&config[..], &[0x0A, 0x02, 0xCC, 0xDD, 0x32, 0x00]].concat();
        assert_eq!(without_added_header(&sample, &config), &sample[4..]);
        // With no header of its own after it, it stays.
        let bare = [&config[..], &[0x32, 0x00]].concat();
        assert_eq!(without_added_header(&bare, &config), &bare[..]);
        assert_eq!(without_added_header(&sample, &[]), &sample[..]);
    }

    #[test]
    fn aacs_channels_come_from_its_configuration() {
        // AAC-LC, 48 kHz: stereo, 5.1, and configuration 0 (a PCE).
        assert_eq!(asc_channels(&[0x11, 0x90]), Some(2));
        assert_eq!(asc_channels(&[0x11, 0xB0]), Some(6));
        assert_eq!(asc_channels(&[0x11, 0x80]), None);
        // An explicit 24-bit frequency before the configuration.
        assert_eq!(asc_channels(&[0x17, 0x80, 0x00, 0x00, 0x10]), Some(2));
        assert_eq!(asc_channels(&[]), None);
        // A 5.1 track whose sample entry says 2 pairs with a 6-channel stream.
        let t = Track {
            rate: Some(48_000),
            channels: asc_channels(&[0x11, 0xB0]),
            ..Track::of(Tag::Aac)
        };
        assert!(sound_matches(&t, 48_000, 6));
    }

    #[test]
    fn mp4_sample_tables_say_where_the_data_ends() {
        let full = |kind: &[u8; 4], body: Vec<u8>| {
            let mut b = ((body.len() + 12) as u32).to_be_bytes().to_vec();
            b.extend(kind);
            b.extend([0, 0, 0, 0]);
            b.extend(body);
            b
        };
        let plain = |kind: &[u8; 4], body: Vec<u8>| {
            let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            b.extend(kind);
            b.extend(body);
            b
        };
        let words = |w: &[u32]| w.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<u8>>();
        // Two chunks, at 1000 and 5000: 3 samples then 2, of 10..=50 bytes.
        let stco = full(b"stco", words(&[2, 1000, 5000]));
        let stsc = full(b"stsc", words(&[2, 1, 3, 1, 2, 2, 1]));
        let stsz = full(b"stsz", words(&[0, 5, 10, 20, 30, 40, 50]));
        let stbl = plain(b"stbl", [stco, stsc, stsz.clone()].concat());
        let trak = plain(b"trak", plain(b"mdia", plain(b"minf", stbl)));
        assert_eq!(mp4_data_end(&trak), Some(5090));
        // Where the tables disagree, the bound is the last sample all of them
        // give. stsc says 9 to a chunk, stsz lists 5: they end at 1150.
        let tracks = |stco: Vec<u8>, stsc: Vec<u8>| {
            let stbl = plain(b"stbl", [stco, stsc, stsz.clone()].concat());
            plain(b"trak", plain(b"mdia", plain(b"minf", stbl)))
        };
        let one_chunk = || full(b"stco", words(&[1, 1000]));
        let trak = tracks(one_chunk(), full(b"stsc", words(&[1, 1, 9, 1])));
        assert_eq!(mp4_data_end(&trak), Some(1150));
        // The review's case: the last run grown by one past what stsz lists.
        let trak = tracks(
            full(b"stco", words(&[2, 1000, 5000])),
            full(b"stsc", words(&[2, 1, 3, 1, 2, 3, 1])),
        );
        assert_eq!(mp4_data_end(&trak), Some(5090));
        // stsc naming a chunk stco hasn't: the end of the chunks there are.
        let trak = tracks(one_chunk(), full(b"stsc", words(&[2, 1, 3, 1, 2, 2, 1])));
        assert_eq!(mp4_data_end(&trak), Some(1060));
        // A track with no samples ends nowhere.
        let trak = tracks(full(b"stco", words(&[0])), full(b"stsc", words(&[0])));
        assert_eq!(mp4_data_end(&trak), Some(0));
    }

    #[test]
    fn a_movie_fragment_says_where_its_data_ends() {
        let full = |kind: &[u8; 4], flags: u32, body: Vec<u8>| {
            let mut b = ((body.len() + 12) as u32).to_be_bytes().to_vec();
            b.extend(kind);
            b.extend(flags.to_be_bytes());
            b.extend(body);
            b
        };
        let plain = |kind: &[u8; 4], body: Vec<u8>| {
            let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            b.extend(kind);
            b.extend(body);
            b
        };
        let words = |w: &[u32]| w.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<u8>>();
        // Track 1, its base the moof (at 1000): 3 samples of 10, 20 and 30
        // bytes from 200 on, each with a duration and a size.
        let traf1 = plain(
            b"traf",
            [
                full(b"tfhd", 0x2_0000, words(&[1])),
                full(b"trun", 0x301, words(&[3, 200, 1, 10, 1, 20, 1, 30])),
            ]
            .concat(),
        );
        assert_eq!(moof_data_end(&traf1, 1000, &[]), Some(1260));
        // Track 2 follows track 1's data: 4 samples of trex's 100 bytes.
        let traf2 = plain(
            b"traf",
            [full(b"tfhd", 0, words(&[2])), full(b"trun", 0, words(&[4]))].concat(),
        );
        let moof = [traf1, traf2.clone()].concat();
        assert_eq!(moof_data_end(&moof, 1000, &[(2, 100)]), Some(1660));
        // An explicit base and a default size in the tfhd, and first-sample
        // flags in the trun, which take no room per sample.
        let tfhd = [words(&[3]), 9000u64.to_be_bytes().to_vec(), words(&[50])].concat();
        let traf3 = plain(
            b"traf",
            [
                full(b"tfhd", 0x11, tfhd),
                full(b"trun", 0x5, words(&[2, 8, 0])),
            ]
            .concat(),
        );
        assert_eq!(moof_data_end(&traf3, 1000, &[]), Some(9108));
        // No size anywhere: that fragment is passed over.
        assert_eq!(moof_data_end(&traf2, 1000, &[]), None);
    }

    #[test]
    fn a_sound_track_pairs_only_with_a_stream_like_it() {
        let t = Track {
            rate: Some(48_000),
            channels: Some(2),
            ..Track::of(Tag::Aac)
        };
        assert!(sound_matches(&t, 48_000, 2));
        // HE-AAC: the core rate on one side, the output rate on the other.
        assert!(sound_matches(&t, 24_000, 2));
        assert!(!sound_matches(&t, 44_100, 2));
        assert!(!sound_matches(&t, 48_000, 6));
        assert!(sound_matches(&Track::of(Tag::Aac), 44_100, 6));
    }

    #[test]
    fn an_avcc_is_built_from_a_sequence_header_and_read_back() {
        // High profile 4:2:0 8-bit: profile 100, level 13; id 0, chroma 1,
        // luma 0, chroma 0 as Exp-Golomb.
        let sps = [0x67, 100, 0, 13, 0b1010_1110, 0x80];
        let pps = [0x68, 0xEB, 0xE3, 0xCB];
        let seq = [&[0, 0, 0, 1][..], &sps, &[0, 0, 1], &pps].concat();
        let avcc = avcc_from_annex_b(&seq).unwrap();
        assert_eq!(&avcc[..6], &[1, 100, 0, 13, 0xFF, 0xE1]);
        assert_eq!(&avcc[avcc.len() - 4..], &[0xFD, 0xF8, 0xF8, 0]);
        let (sets, size) = config_sets(&Tag::H264, &avcc);
        assert_eq!(size, 4);
        assert_eq!(sets, vec![sps.to_vec(), pps.to_vec()]);
        // No PPS: no record.
        assert_eq!(avcc_from_annex_b(&[0, 0, 1, 0x67, 66, 0, 30]), None);
        // A 2-byte length record is moved to 4 bytes.
        let mut short = avcc.clone();
        short[4] = 0xFD;
        assert_eq!(config_sets(&Tag::H264, &short).1, 2);
        assert_eq!(with_four_byte_lengths(&Tag::H264, short)[4], 0xFF);
    }

    #[test]
    fn every_tracks_edit_list_is_found_for_media_foundation_not_to_see() {
        // h264.mp4: a picture and a sound track, each with an edit list.
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/video/h264.mp4");
        let bytes = std::fs::read(&path).unwrap();
        let found = facts(&mut File::open(&path).unwrap()).edit_lists;
        assert_eq!(found.len(), 2, "{found:?}");
        for at in found {
            let at = at as usize;
            assert_eq!(&bytes[at..at + 4], b"edts", "at {at}");
        }
        // A Matroska file has none.
        let mkv = path.with_file_name("h264.mkv");
        let f = facts(&mut File::open(mkv).unwrap());
        assert!(f.edit_lists.is_empty());
        assert_eq!(
            f.mkv_picture,
            Some(MkvPicture {
                number: 1,
                scale: 1_000_000,
                default_duration: Some(33_333_333)
            })
        );
    }

    #[test]
    fn container_boxes_give_their_codec_data() {
        // esds: ES_Descriptor { ES_ID, flags 0, DecoderConfig { AAC,
        // DecSpecificInfo 11 90 } }, with a 4-byte length on the config.
        let mut config = vec![0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        config.extend([5, 2, 0x11, 0x90]);
        let mut es = vec![0, 1, 0];
        es.extend([4, 0x80, 0x80, 0x80, config.len() as u8]);
        es.extend(&config);
        let mut esds = vec![0, 0, 0, 0, 3, es.len() as u8];
        esds.extend(&es);
        assert_eq!(esds_config(&esds), Some((0x40, vec![0x11, 0x90])));
        assert_eq!(esds_config(&esds[..8]), None);

        // dOps: 2 channels, pre-skip 312, 48 kHz, gain 0, family 0.
        let dops = [0, 2, 0x01, 0x38, 0, 0, 0xBB, 0x80, 0, 0, 0];
        assert_eq!(
            opus_head_from_dops(&dops).unwrap(),
            [
                &b"OpusHead"[..],
                &[1, 2, 0x38, 0x01, 0x80, 0xBB, 0, 0, 0, 0, 0]
            ]
            .concat()
        );

        // elst: one edit starting the media 1,024 ticks in (at 15,360 a
        // second), and an empty edit of 500 ms (at 1,000) before another.
        let mut elst = vec![0, 0, 0, 0, 0, 0, 0, 1];
        elst.extend(2000u32.to_be_bytes());
        elst.extend(1024u32.to_be_bytes());
        elst.extend(0x10000u32.to_be_bytes());
        assert_eq!(edit_shift(&elst, 1000, 15_360), Some(-666_667));
        let mut elst = vec![0, 0, 0, 0, 0, 0, 0, 2];
        elst.extend(500u32.to_be_bytes());
        elst.extend(u32::MAX.to_be_bytes());
        elst.extend(0x10000u32.to_be_bytes());
        elst.extend(2000u32.to_be_bytes());
        elst.extend(0u32.to_be_bytes());
        elst.extend(0x10000u32.to_be_bytes());
        assert_eq!(edit_shift(&elst, 1000, 48_000), Some(5_000_000));
        // The same empty edit before a media time of 1,024 (FFmpeg's late
        // start with B-frames): both apply, and it starts at the empty
        // edit's end.
        let mut elst = vec![0, 0, 0, 0, 0, 0, 0, 2];
        elst.extend(500u32.to_be_bytes());
        elst.extend(u32::MAX.to_be_bytes());
        elst.extend(0x10000u32.to_be_bytes());
        elst.extend(2000u32.to_be_bytes());
        elst.extend(1024u32.to_be_bytes());
        elst.extend(0x10000u32.to_be_bytes());
        assert_eq!(edit_shift(&elst, 1000, 15_360), Some(5_000_000 - 666_667));
        assert_eq!(edit_start(&elst, 1000), 5_000_000);
        assert_eq!(first_edit(&elst), Some((500, 1024)));
        assert_eq!(edit_shift(&[0, 0, 0, 0, 0, 0, 0, 0], 1000, 1000), None);
        assert_eq!(edit_start(&[0, 0, 0, 0, 0, 0, 0, 0], 1000), 0);
    }

    #[test]
    fn an_hvccs_sei_gives_the_hdr10_metadata() {
        // One SEI array holding one prefix SEI NAL with a content light level
        // (144) and a mastering display (137) message.
        let mut sei = vec![0x4E, 0x01, 144, 4, 0x03, 0xE8, 0x01, 0x90, 137, 24];
        let mdcv: Vec<u8> = (1..=24).collect();
        sei.extend(&mdcv);
        sei.push(0x80);
        let mut hvcc = vec![0u8; 22];
        hvcc.push(1);
        hvcc.push(39);
        hvcc.extend(1u16.to_be_bytes());
        hvcc.extend((sei.len() as u16).to_be_bytes());
        hvcc.extend(&sei);
        let (m, l) = hdr_from_hvcc(&hvcc);
        assert_eq!(l, Some([0x03, 0xE8, 0x01, 0x90]));
        assert_eq!(m.map(|m| m.to_vec()), Some(mdcv));
        // Cut short: nothing, and no panic.
        for cut in 0..hvcc.len() {
            let _ = hdr_from_hvcc(&hvcc[..cut]);
        }
        assert_eq!(hdr_from_hvcc(&hvcc[..30]).1, None);
    }

    #[test]
    fn matroska_mastering_becomes_an_mdcv() {
        let m = [
            0.708, 0.292, 0.17, 0.797, 0.131, 0.046, 0.3127, 0.329, 1000.0, 0.0001,
        ];
        let b = mdcv_from_matroska(m);
        // Green first: 0.17 and 0.797 in steps of 0.00002.
        assert_eq!(&b[..4], &[0x21, 0x34, 0x9B, 0xAA]);
        assert_eq!(
            u32::from_be_bytes(b[16..20].try_into().unwrap()),
            10_000_000
        );
        assert_eq!(u32::from_be_bytes(b[20..24].try_into().unwrap()), 1);
    }

    #[test]
    fn a_sound_stream_needs_its_private_data() {
        let t = Track::of(Tag::Aac);
        // The ASC from MF's user data stands in.
        let user = [[0u8; 12].as_slice(), &[0x11, 0x90]].concat();
        let s = sound_spec(&t, 48_000, 2, Some(&user)).unwrap();
        assert_eq!(
            s.codec,
            StreamCodec::Aac {
                asc: vec![0x11, 0x90]
            }
        );
        assert_eq!(s.timescale, TIMESCALE);
        assert_eq!(sound_spec(&t, 48_000, 2, None), Err("AAC".into()));
        assert_eq!(
            sound_spec(&Track::of(Tag::Other("DTS".into())), 48_000, 2, None),
            Err("DTS".into())
        );
        assert!(sound_spec(&Track::of(Tag::Ac3), 48_000, 6, None).is_ok());
        assert!(sound_spec(&Track::of(Tag::Ac3), 0, 6, None).is_err());
    }

    /// Media Foundation's sequence header for the H.264 fixture turns into
    /// the very `avcC` the file holds.
    #[cfg(windows)]
    #[test]
    fn media_foundations_sequence_header_makes_the_files_avcc() {
        use crate::mf::com::{byte_stream, get_blob, Mf};
        use windows::Win32::Media::MediaFoundation::*;
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/video/h264.mp4");
        let mut file = File::open(&path).unwrap();
        let f = facts(&mut file);
        let container = f.video.unwrap().private;
        let _mf = Mf::start().unwrap();
        let (stream, _) = byte_stream(file, false).unwrap();
        // SAFETY: plain calls on a reader made here.
        let seq = unsafe {
            let r = MFCreateSourceReaderFromByteStream(&stream, None).unwrap();
            let t = r
                .GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0)
                .unwrap();
            get_blob(&t, &MF_MT_MPEG_SEQUENCE_HEADER).unwrap()
        };
        assert_eq!(avcc_from_annex_b(&seq).unwrap(), container);
    }
}
