//! What an MP4 says about its AAC's encoder delay and padding, so the audio
//! Symphonia decodes can be trimmed to what the file presents.
//!
//! Symphonia ignores edit lists, so an AAC read through it keeps the encoder's
//! priming at the start (FFmpeg's encoder writes 1,024 frames of it) and the
//! padding that fills out the last frame. The first audio track's
//! `edts/elst` says where the real audio starts (`media_time`) and how long it
//! is (`segment_duration`); without an edit list, an iTunes `iTunSMPB` tag says
//! the same. [`aac_trim`] reads either; [`Trimmed`] applies it.

use crate::error::Result;
use crate::pipeline::{Progressed, Source};
use crate::types::{Block, SourceInfo};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// The biggest `moov` that is read.
const MAX_MOOV: u64 = 64 * 1024 * 1024;

/// Frames to drop from the start, and how many to keep after them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trim {
    pub skip: u64,
    pub keep: Option<u64>,
}

/// The trim the first audio track of an MP4 asks for, in frames at `rate`.
/// `None` when the file isn't an MP4, says nothing, or says it in a way this
/// doesn't follow (several edits, a rate other than 1). Leaves the file's
/// position where it was.
pub fn aac_trim(file: &mut File, rate: u32) -> Option<Trim> {
    let at = file.stream_position().ok()?;
    let moov = read_moov(file);
    file.seek(SeekFrom::Start(at)).ok()?;
    trim_from_moov(&moov?, rate)
}

/// The body of the top-level `moov` box, when the file starts with `ftyp`.
fn read_moov(file: &mut File) -> Option<Vec<u8>> {
    read_moov_with(file, |kind| kind == b"ftyp")
}

/// The boxes a QuickTime movie may start with when it has no `ftyp`.
pub(crate) fn may_start_a_movie(kind: &[u8; 4]) -> bool {
    matches!(
        kind,
        b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free" | b"skip" | b"pnot"
    )
}

/// The body of the top-level `moov` box, when the file's first box is one
/// `first_ok` accepts. Capped at 64 MB: `None` past that, or when the box is
/// cut short, missing or not an MP4's.
pub(crate) fn read_moov_with(file: &mut File, first_ok: fn(&[u8; 4]) -> bool) -> Option<Vec<u8>> {
    find_moov(file, first_ok).map(|(_, moov)| moov)
}

/// As [`read_moov_with`], with where the body starts in the file.
pub(crate) fn find_moov(file: &mut File, first_ok: fn(&[u8; 4]) -> bool) -> Option<(u64, Vec<u8>)> {
    let len = file.metadata().ok()?.len();
    let mut pos = 0u64;
    let mut first = true;
    while pos.checked_add(8).is_some_and(|end| end <= len) {
        file.seek(SeekFrom::Start(pos)).ok()?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head[..8]).ok()?;
        let size32 = u32::from_be_bytes(head[..4].try_into().ok()?) as u64;
        let kind: [u8; 4] = head[4..8].try_into().ok()?;
        if first && !first_ok(&kind) {
            return None;
        }
        first = false;
        let (size, header) = match size32 {
            0 => (len - pos, 8),
            1 => {
                file.read_exact(&mut head[8..16]).ok()?;
                (u64::from_be_bytes(head[8..16].try_into().ok()?), 16)
            }
            n => (n, 8),
        };
        if size < header {
            return None;
        }
        if &kind == b"moov" {
            let body = size - header;
            if body > MAX_MOOV || size > len - pos {
                return None;
            }
            let mut buf = vec![0u8; body as usize];
            file.read_exact(&mut buf).ok()?;
            return Some((pos + header, buf));
        }
        pos = pos.checked_add(size)?;
    }
    None
}

/// The child boxes of a box body, one at a time, as (type, body). Stops at the
/// first box that is cut short or says it is smaller than its own header.
pub(crate) struct Boxes<'a>(&'a [u8]);

impl<'a> Iterator for Boxes<'a> {
    type Item = ([u8; 4], &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let b = self.0;
        if b.len() < 8 {
            return None;
        }
        let size32 = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
        let kind = [b[4], b[5], b[6], b[7]];
        let (size, header) = match size32 {
            0 => (b.len() as u64, 8),
            1 if b.len() >= 16 => (u64::from_be_bytes(b[8..16].try_into().unwrap()), 16),
            1 => return None,
            n => (n, 8),
        };
        if size < header || size > b.len() as u64 {
            return None;
        }
        self.0 = &b[size as usize..];
        Some((kind, &b[header as usize..size as usize]))
    }
}

/// The child boxes of a box body, lazily: nothing is allocated, so a body of
/// millions of tiny boxes costs time but not memory.
pub(crate) fn boxes(b: &[u8]) -> Boxes<'_> {
    Boxes(b)
}

/// The child boxes of a box body, as (type, body).
pub(crate) fn children(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    boxes(b).collect()
}

pub(crate) fn child<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(b).find(|(k, _)| k == kind).map(|(_, v)| v)
}

pub(crate) fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

pub(crate) fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

pub(crate) fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// The timescale of an `mvhd` or `mdhd` body.
pub(crate) fn timescale(full: &[u8]) -> Option<u32> {
    let at = if *full.first()? == 1 { 20 } else { 12 };
    be32(full, at).filter(|&t| t > 0)
}

fn scale(v: u64, from: u32, to: u32) -> u64 {
    (v as u128 * to as u128 / from as u128) as u64
}

fn trim_from_moov(moov: &[u8], rate: u32) -> Option<Trim> {
    if rate == 0 {
        return None;
    }
    let movie_scale = timescale(child(moov, b"mvhd")?)?;
    let trak = children(moov)
        .into_iter()
        .filter(|(k, _)| k == b"trak")
        .map(|(_, v)| v)
        .find(|t| {
            child(t, b"mdia")
                .and_then(|m| child(m, b"hdlr"))
                .and_then(|h| h.get(8..12))
                == Some(b"soun")
        })?;
    let media_scale = timescale(child(child(trak, b"mdia")?, b"mdhd")?)?;
    if let Some(elst) = child(trak, b"edts").and_then(|e| child(e, b"elst")) {
        if let Some(t) = from_elst(elst, movie_scale, media_scale, rate) {
            return Some(t);
        }
    }
    from_smpb(moov)
}

/// The one edit with media in it (empty edits ignored), in frames at `rate`.
fn from_elst(elst: &[u8], movie_scale: u32, media_scale: u32, rate: u32) -> Option<Trim> {
    let long = *elst.first()? == 1;
    let count = be32(elst, 4)? as usize;
    let entry = if long { 20 } else { 12 };
    let mut found = None;
    for i in 0..count {
        let at = 8 + i * entry;
        let (duration, media_time, rate_at) = if long {
            (be64(elst, at)?, be64(elst, at + 8)? as i64, at + 16)
        } else {
            (
                be32(elst, at)? as u64,
                be32(elst, at + 4)? as i32 as i64,
                at + 8,
            )
        };
        if media_time == -1 {
            continue;
        }
        if found.is_some() || be32(elst, rate_at)? != 0x10000 || media_time < 0 {
            return None;
        }
        found = Some((duration, media_time as u64));
    }
    let (duration, media_time) = found?;
    Some(Trim {
        skip: scale(media_time, media_scale, rate),
        keep: (duration > 0).then(|| scale(duration, movie_scale, rate)),
    })
}

/// iTunes' gapless tag: " 00000000 priming padding length ...", in hex.
fn from_smpb(moov: &[u8]) -> Option<Trim> {
    let ilst = child(child(child(moov, b"udta")?, b"meta")?.get(4..)?, b"ilst")?;
    for (kind, item) in children(ilst) {
        if &kind != b"----" {
            continue;
        }
        let parts = children(item);
        let body = |kind: &[u8; 4], skip: usize| {
            parts
                .iter()
                .find(|(k, _)| k == kind)
                .and_then(|(_, v)| v.get(skip..))
        };
        // An item missing its name or data is someone else's: pass it over.
        let (Some(name), Some(data)) = (body(b"name", 4), body(b"data", 8)) else {
            continue;
        };
        if name != b"iTunSMPB" {
            continue;
        }
        let text = std::str::from_utf8(data).ok()?;
        let fields: Vec<&str> = text.split_whitespace().collect();
        let priming = u64::from_str_radix(fields.get(1)?, 16).ok()?;
        let length = u64::from_str_radix(fields.get(3)?, 16).ok()?;
        return Some(Trim {
            skip: priming,
            keep: (length > 0).then_some(length),
        });
    }
    None
}

/// A source with its first `skip` frames dropped and only `keep` after them.
pub struct Trimmed {
    inner: Box<dyn Source>,
    info: SourceInfo,
    to_skip: u64,
    left: Option<u64>,
    out_frames: u64,
}

impl Trimmed {
    pub fn new(inner: Box<dyn Source>, trim: Trim) -> Trimmed {
        let mut info = inner.info().clone();
        info.frames = match (trim.keep, info.frames) {
            (Some(k), Some(f)) => Some(k.min(f.saturating_sub(trim.skip))),
            (Some(k), None) => Some(k),
            (None, f) => f.map(|f| f.saturating_sub(trim.skip)),
        };
        Trimmed {
            inner,
            info,
            to_skip: trim.skip,
            left: trim.keep,
            out_frames: 0,
        }
    }
}

impl Source for Trimmed {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Block) -> Result<bool> {
        loop {
            if self.left == Some(0) {
                out.samples.clear();
                return Ok(false);
            }
            if !self.inner.read(out)? {
                return Ok(false);
            }
            let ch = out.channels.max(1) as usize;
            let mut frames = out.frames() as u64;
            if self.to_skip > 0 {
                let drop = self.to_skip.min(frames);
                out.samples.drain(..drop as usize * ch);
                self.to_skip -= drop;
                frames -= drop;
            }
            if let Some(left) = &mut self.left {
                let take = frames.min(*left);
                out.samples.truncate(take as usize * ch);
                *left -= take;
                frames = take;
            }
            if frames > 0 {
                self.out_frames += frames;
                return Ok(true);
            }
        }
    }

    fn position(&self) -> Progressed {
        Progressed {
            frames: self.out_frames,
            ..self.inner.position()
        }
    }
}

// ---------------------------------------------------------------------------
// Video

/// A box that is there but doesn't hold what its type promises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Malformed;

/// What the first `vide` track of an MP4 or MOV says, from its `tkhd`, `mdhd`,
/// `stsd`, `stts` and `stsz` and the boxes inside its sample entry.
#[derive(Debug, Clone)]
pub(crate) struct Mp4Video<'a> {
    /// The `tkhd` matrix: a, b, u, c, d, v, x, y, w. a to d, x and y are 16.16
    /// fixed point; u, v and w are 2.30.
    pub matrix: [i32; 9],
    /// The movie's time scale (`mvhd`), and its length in it; 0 when unknown.
    pub movie_scale: Option<u32>,
    pub movie_duration: u64,
    /// The track's length (`tkhd`) in the movie's time scale; 0 when unknown.
    pub track_duration: u64,
    /// The media's time scale (`mdhd`), and its length in it; 0 when unknown.
    pub media_scale: u32,
    pub media_duration: u64,
    /// The first sample entry's type: `avc1`, `hvc1`, `av01` and so on.
    pub fourcc: [u8; 4],
    /// The coded picture size the sample entry says.
    pub width: u32,
    pub height: u32,
    /// The boxes inside the sample entry (`avcC`, `colr`, `clli`, ...).
    pub entry: &'a [u8],
    /// The transfer characteristics of a `colr` box of type `nclx` or `nclc`.
    pub colr_transfer: Option<u16>,
    /// That box's primaries, transfer characteristics and matrix, as H.273
    /// numbers them.
    pub colr: Option<[u16; 3]>,
    /// True when the entry has `mdcv` or `clli`, the static HDR10 metadata.
    pub hdr10_metadata: bool,
    /// Samples, and their summed duration in the media's time scale (`stts`).
    pub samples: u64,
    pub sample_ticks: u64,
    /// True when `stts` has more than one distinct sample duration.
    pub varied_deltas: bool,
    /// The summed size of the samples (`stsz`); `None` when there is no `stsz`.
    pub bytes: Option<u64>,
    /// The `trak` body itself, for the tables read only when needed.
    pub trak: &'a [u8],
}

/// The first `vide` track of a `moov` body. `Ok(None)` when it has none.
pub(crate) fn video_track(moov: &[u8]) -> std::result::Result<Option<Mp4Video<'_>>, Malformed> {
    let Some(trak) = boxes(moov)
        .filter(|(k, _)| k == b"trak")
        .map(|(_, v)| v)
        .find(|t| {
            child(t, b"mdia")
                .and_then(|m| child(m, b"hdlr"))
                .and_then(|h| h.get(8..12))
                == Some(b"vide")
        })
    else {
        return Ok(None);
    };
    let (movie_scale, movie_duration) = match child(moov, b"mvhd") {
        Some(h) => (timescale(h), duration_at(h, 16, 24)),
        None => (None, 0),
    };
    let tkhd = child(trak, b"tkhd").ok_or(Malformed)?;
    let long = *tkhd.first().ok_or(Malformed)? == 1;
    let track_duration = if long {
        duration_at(tkhd, 0, 28)
    } else {
        duration_at(tkhd, 20, 0)
    };
    let at = if long { 52 } else { 40 };
    let mut matrix = [0i32; 9];
    for (i, m) in matrix.iter_mut().enumerate() {
        *m = be32(tkhd, at + i * 4).ok_or(Malformed)? as i32;
    }
    let mdia = child(trak, b"mdia").ok_or(Malformed)?;
    let mdhd = child(mdia, b"mdhd").ok_or(Malformed)?;
    let media_scale = timescale(mdhd).ok_or(Malformed)?;
    let media_duration = duration_at(mdhd, 16, 24);
    let stbl = child(child(mdia, b"minf").ok_or(Malformed)?, b"stbl").ok_or(Malformed)?;

    let stsd = child(stbl, b"stsd").ok_or(Malformed)?;
    if be32(stsd, 4).ok_or(Malformed)? == 0 {
        return Err(Malformed);
    }
    let (fourcc, sample) = boxes(stsd.get(8..).ok_or(Malformed)?)
        .next()
        .ok_or(Malformed)?;
    // A visual sample entry: 6 reserved bytes, the data reference index, 16
    // bytes of version and vendor, the size, 50 more bytes, then its boxes.
    let entry = sample.get(78..).ok_or(Malformed)?;
    let width = be16(sample, 24).ok_or(Malformed)? as u32;
    let height = be16(sample, 26).ok_or(Malformed)? as u32;

    let colr = boxes(entry)
        .filter(|(k, _)| k == b"colr")
        .find(|(_, v)| matches!(v.get(..4), Some(b"nclx" | b"nclc")))
        .and_then(|(_, v)| Some([be16(v, 4)?, be16(v, 6)?, be16(v, 8)?]));
    let colr_transfer = colr.map(|c| c[1]);
    let hdr10_metadata = boxes(entry)
        .any(|(k, v)| (&k == b"mdcv" && v.len() >= 24) || (&k == b"clli" && v.len() >= 4));

    let (samples, sample_ticks, varied_deltas) = match child(stbl, b"stts") {
        Some(stts) => stts_summary(stts).ok_or(Malformed)?,
        None => (0, 0, false),
    };
    let bytes = match child(stbl, b"stsz") {
        Some(stsz) => Some(stsz_total(stsz).ok_or(Malformed)?),
        None => None,
    };
    Ok(Some(Mp4Video {
        matrix,
        movie_scale,
        movie_duration,
        track_duration,
        media_scale,
        media_duration,
        fourcc,
        width,
        height,
        entry,
        colr_transfer,
        colr,
        hdr10_metadata,
        samples,
        sample_ticks,
        varied_deltas,
        bytes,
        trak,
    }))
}

/// The duration in an `mvhd`, `mdhd` or `tkhd` body: a 32-bit field at
/// `short` (version 0) or a 64-bit one at `long` (version 1). 0 when it is
/// missing or says "unknown" (all ones).
fn duration_at(full: &[u8], short: usize, long: usize) -> u64 {
    let d = if full.first() == Some(&1) {
        be64(full, long).filter(|&d| d != u64::MAX)
    } else {
        be32(full, short).filter(|&d| d != u32::MAX).map(u64::from)
    };
    d.unwrap_or(0)
}

/// Samples, their summed duration, and whether not all last the same, from an
/// `stts` body.
fn stts_summary(stts: &[u8]) -> Option<(u64, u64, bool)> {
    let entries = be32(stts, 4)? as u64;
    let table = stts.get(8..)?;
    if (table.len() as u64) / 8 < entries {
        return None;
    }
    let entry = |e: &[u8]| {
        (
            u32::from_be_bytes([e[0], e[1], e[2], e[3]]) as u64,
            u32::from_be_bytes([e[4], e[5], e[6], e[7]]),
        )
    };
    let all = || {
        table
            .chunks_exact(8)
            .take(entries as usize)
            .map(entry)
            .filter(|&(count, _)| count > 0)
    };
    let (mut samples, mut ticks) = (0u64, 0u64);
    for (count, delta) in all() {
        samples = samples.saturating_add(count);
        ticks = ticks.saturating_add(count * delta as u64);
    }
    // The last frame of a constant-rate file often has its own length, so a
    // final entry for a single sample doesn't make the rate variable.
    let judged = match all().next_back() {
        Some((1, _)) => all().count() - 1,
        _ => usize::MAX,
    };
    let mut deltas = all().take(judged).map(|(_, delta)| delta);
    let first = deltas.next();
    let varied = deltas.any(|d| Some(d) != first);
    Some((samples, ticks, varied))
}

/// The summed size of the samples of an `stsz` body.
fn stsz_total(stsz: &[u8]) -> Option<u64> {
    let each = be32(stsz, 4)? as u64;
    let count = be32(stsz, 8)? as u64;
    if each != 0 {
        return Some(each * count);
    }
    let table = stsz.get(12..)?;
    if (table.len() as u64) / 4 < count {
        return None;
    }
    Some(
        table
            .chunks_exact(4)
            .take(count as usize)
            .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as u64)
            .sum(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn header(scale: u32) -> Vec<u8> {
        let mut h = vec![0u8; 12];
        h.extend_from_slice(&scale.to_be_bytes());
        h.extend_from_slice(&[0u8; 8]);
        h
    }

    fn moov(elst: Option<Vec<u8>>, smpb: Option<&str>) -> Vec<u8> {
        let mut hdlr = vec![0u8; 8];
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0u8; 12]);
        let mdia = [bx(b"mdhd", &header(48_000)), bx(b"hdlr", &hdlr)].concat();
        let mut trak = Vec::new();
        if let Some(e) = elst {
            trak.extend(bx(b"edts", &bx(b"elst", &e)));
        }
        trak.extend(bx(b"mdia", &mdia));
        let mut m = [bx(b"mvhd", &header(1000)), bx(b"trak", &trak)].concat();
        if let Some(s) = smpb {
            let mut data = vec![0u8; 8];
            data.extend_from_slice(s.as_bytes());
            let mut name = vec![0u8; 4];
            name.extend_from_slice(b"iTunSMPB");
            let item = [bx(b"name", &name), bx(b"data", &data)].concat();
            let ilst = bx(b"ilst", &bx(b"----", &item));
            let mut meta = vec![0u8; 4];
            meta.extend(ilst);
            m.extend(bx(b"udta", &bx(b"meta", &meta)));
        }
        m
    }

    fn elst_v0(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut e = vec![0u8; 4];
        e.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (d, t) in entries {
            e.extend_from_slice(&d.to_be_bytes());
            e.extend_from_slice(&t.to_be_bytes());
            e.extend_from_slice(&0x10000u32.to_be_bytes());
        }
        e
    }

    #[test]
    fn an_edit_list_gives_the_priming_and_the_length() {
        let m = moov(Some(elst_v0(&[(2000, 1024)])), None);
        assert_eq!(
            trim_from_moov(&m, 48_000),
            Some(Trim {
                skip: 1024,
                keep: Some(96_000)
            })
        );
        // An empty edit in front is a delay, not part of the media.
        let m = moov(Some(elst_v0(&[(500, -1), (2000, 2112)])), None);
        assert_eq!(trim_from_moov(&m, 48_000).unwrap().skip, 2112);
        // Two edits with media: not followed.
        let m = moov(Some(elst_v0(&[(1000, 0), (1000, 4096)])), None);
        assert_eq!(trim_from_moov(&m, 48_000), None);
    }

    #[test]
    fn itunsmpb_stands_in_without_an_edit_list() {
        let text = " 00000000 00000840 000001C0 0000000000046E00 00000000";
        let m = moov(None, Some(text));
        assert_eq!(
            trim_from_moov(&m, 44_100),
            Some(Trim {
                skip: 0x840,
                keep: Some(0x46E00)
            })
        );
        assert_eq!(trim_from_moov(&moov(None, None), 44_100), None);
    }

    #[test]
    fn a_freeform_item_without_a_name_or_data_is_passed_over() {
        let mut name = vec![0u8; 4];
        name.extend_from_slice(b"iTunSMPB");
        let mut data = vec![0u8; 8];
        data.extend_from_slice(b" 00000000 00000400 00000000 0000000000001000");
        let nameless = bx(b"----", &bx(b"data", &data));
        let dataless = bx(b"----", &bx(b"name", &name));
        let good = bx(b"----", &[bx(b"name", &name), bx(b"data", &data)].concat());
        let ilst = bx(b"ilst", &[nameless, dataless, good].concat());
        let mut meta = vec![0u8; 4];
        meta.extend(ilst);
        let mut m = moov(None, None);
        m.extend(bx(b"udta", &bx(b"meta", &meta)));
        assert_eq!(
            trim_from_moov(&m, 48_000),
            Some(Trim {
                skip: 0x400,
                keep: Some(0x1000)
            })
        );
    }

    struct Ramp {
        info: SourceInfo,
        next: u64,
        total: u64,
    }

    impl Source for Ramp {
        fn info(&self) -> &SourceInfo {
            &self.info
        }
        fn read(&mut self, out: &mut Block) -> Result<bool> {
            out.channels = 1;
            out.rate = 48_000;
            out.samples.clear();
            let n = (self.total - self.next).min(1000);
            if n == 0 {
                return Ok(false);
            }
            out.samples
                .extend((self.next..self.next + n).map(|i| i as f32));
            self.next += n;
            Ok(true)
        }
        fn position(&self) -> Progressed {
            Progressed::default()
        }
    }

    #[test]
    fn trimmed_drops_the_start_and_the_end() {
        let info = SourceInfo {
            codec: crate::types::Codec::Aac,
            rate: 48_000,
            channels: crate::types::Channels { count: 1, mask: 4 },
            bits: None,
            frames: Some(5000),
            kbps: None,
            tags: Default::default(),
        };
        let ramp = Ramp {
            info,
            next: 0,
            total: 5000,
        };
        let mut t = Trimmed::new(
            Box::new(ramp),
            Trim {
                skip: 2500,
                keep: Some(1800),
            },
        );
        assert_eq!(t.info().frames, Some(1800));
        let mut all = Vec::new();
        let mut b = Block {
            channels: 0,
            rate: 0,
            samples: Vec::new(),
        };
        while t.read(&mut b).unwrap() {
            assert!(b.frames() > 0);
            all.extend_from_slice(&b.samples);
        }
        assert_eq!(all.len(), 1800);
        assert_eq!(all[0], 2500.0);
        assert_eq!(*all.last().unwrap(), 4299.0);
        assert_eq!(t.position().frames, 1800);
    }
}
