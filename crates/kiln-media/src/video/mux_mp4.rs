//! Kiln's own MP4, MOV and M4V writer, for any number of video and sound
//! tracks.
//!
//! It works as part 1's M4A writer does (`mp4.rs`): the samples go into a
//! temporary file beside the output while the stream is written, and at the
//! end `ftyp` and `moov` (whose chunk offsets are known by then) are written
//! to the output, followed by the copied samples in an `mdat`. The `moov` is
//! first in the file, so it plays while it downloads. Offsets are 64-bit
//! (`co64`) in a track whose chunks start past 4 GiB.
//!
//! Each stream's samples are gathered into one chunk per half second of its
//! decode time, and a chunk is written when the next one starts, so streams
//! that arrive about in step end up interleaved in the file.
//!
//! Each track gets a media time scale of its own. A sound track uses the
//! sample rate. A picture track keeps the stream's time scale when that is
//! 90,000 or less; a finer clock (Media Foundation's 100 ns) goes to 90,000,
//! or to the first of 24,000, 30,000, 60,000 and 120,000 in which every
//! frame is the same length when 90,000 doesn't do that (23.976 and 59.94
//! fps), so constant-rate video makes a one-run `stts`. Every packet's
//! decode and presentation time is converted on its own, rounded to the
//! nearest tick, and the steps are the differences of those, so nothing adds
//! up over a long file.
//!
//! Decode times are shifted so a track's first sample is at 0 (`stts`), the
//! difference to the presentation time goes in `ctts` (version 1, signed),
//! and an edit list starts the track at its first presentation time: an
//! empty edit when that is later than 0, a media start past the samples it
//! must skip otherwise.
//!
//! An HEVC track is `hvc1`, with its parameter sets in the `hvcC` only, and
//! the samples' own copies of them left out. If a sample then carries a
//! parameter set the `hvcC` doesn't hold, stripping stops for the rest of the
//! stream and the track's entry is `hev1` (the sets are in the samples too).

use super::packet::{check_spec, rescale, Container, Mux, Packet, StreamCodec, StreamSpec};
use crate::error::{MediaError, Result};
use crate::mp4::{be32, be64, esds_box, temp_beside, temp_next_to, time};
use crate::tags::mp4::{full_box, mp4_box, udta};
use crate::types::Tags;
use std::borrow::Cow;
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The movie's time scale: milliseconds.
const MOVIE_SCALE: u32 = 1000;

/// How much of the samples `finish` copies between looks at cancel.
const COPY_PIECE: usize = 1 << 20;

/// One sample, as the tables need it.
#[derive(Debug, Clone, Copy)]
struct Sample {
    size: u32,
    dts: i64,
    pts: i64,
    duration: i64,
    key: bool,
}

/// A run of samples stored together, `offset` bytes into the `mdat`'s samples.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    offset: u64,
    samples: u32,
}

struct Stream {
    spec: StreamSpec,
    /// The parameter sets an HEVC stream's `hvcC` holds, to leave out of the
    /// samples (an `hvc1` entry keeps them out of band). `None` once a sample
    /// has brought one the `hvcC` doesn't hold.
    params: Option<ParamSets>,
    /// True once that has happened: the entry is `hev1`.
    hev1: bool,
    samples: Vec<Sample>,
    chunks: Vec<Chunk>,
    pending: Vec<u8>,
    pending_samples: u32,
    pending_from: i64,
    last_dts: Option<i64>,
    /// The start of the first AC-3 frame, for the `dac3` box.
    head: Vec<u8>,
    /// Where the presentation starts, in the stream's ticks (`present_from`).
    from: i64,
}

pub(crate) struct Mp4Mux {
    kind: Container,
    out: File,
    /// The samples so far, in the temporary file beside the output.
    mdat: BufWriter<File>,
    temp: Option<PathBuf>,
    mdat_len: u64,
    /// Added to every chunk offset: test-only, to reach past 4 GiB.
    bias: u64,
    streams: Vec<Stream>,
    /// The text of the first error a write met; the muxer is done after it.
    failed: Option<String>,
    /// When the movie was made, in seconds since 1904 (the `mvhd`'s
    /// clock); 0 when unknown.
    created: u64,
}

fn encoder(msg: impl Into<String>) -> MediaError {
    MediaError::Encoder(msg.into())
}

impl Mp4Mux {
    /// A writer of `kind` into `out`, whose samples wait in a temporary
    /// file beside `at` (the output's path, R92), or else beside wherever
    /// `out` turns out to be.
    pub(crate) fn new(kind: Container, out: File, bias: u64, at: Option<&Path>) -> Result<Mp4Mux> {
        if kind == Container::Mkv {
            return Err(encoder("a Matroska file isn't written by the MP4 writer"));
        }
        let temp = at.map_or_else(|| temp_beside(&out), temp_next_to);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temp)?;
        Ok(Mp4Mux {
            kind,
            out,
            mdat: BufWriter::with_capacity(256 * 1024, file),
            temp: Some(temp),
            mdat_len: 0,
            bias,
            streams: Vec::new(),
            failed: None,
            created: 0,
        })
    }

    /// Writes stream `i`'s gathered samples to the temporary file as a chunk.
    fn flush(&mut self, i: usize) -> Result<()> {
        let st = &mut self.streams[i];
        if st.pending_samples == 0 {
            return Ok(());
        }
        self.mdat.write_all(&st.pending)?;
        st.chunks.push(Chunk {
            offset: self.mdat_len,
            samples: st.pending_samples,
        });
        self.mdat_len += st.pending.len() as u64;
        st.pending.clear();
        st.pending_samples = 0;
        Ok(())
    }
}

impl Mux for Mp4Mux {
    fn describe(&mut self, created: Option<i64>, _title: Option<&str>) {
        self.created = created
            .and_then(|s| s.checked_add(super::info::MP4_EPOCH))
            .and_then(|s| u64::try_from(s).ok())
            .unwrap_or(0);
    }

    fn add_stream(&mut self, s: StreamSpec) -> Result<usize> {
        check_spec(&s)?;
        let named = |what: &str| encoder(format!("{} {what}", s.codec.name()));
        match &s.codec {
            StreamCodec::H264 { avcc: c }
            | StreamCodec::Hevc { hvcc: c }
            | StreamCodec::Av1 { av1c: c }
            | StreamCodec::Vp9 { vpcc: c }
            | StreamCodec::Aac { asc: c }
                if c.is_empty() =>
            {
                return Err(named("stream has no codec configuration"));
            }
            StreamCodec::H264 { .. }
            | StreamCodec::Hevc { .. }
            | StreamCodec::Av1 { .. }
            | StreamCodec::Vp9 { .. }
            | StreamCodec::Aac { .. }
            | StreamCodec::Ac3 => {}
            _ => return Err(named("can't go into an MP4 file")),
        }
        let params = match &s.codec {
            StreamCodec::Hevc { hvcc } => ParamSets::from_hvcc(hvcc),
            _ => None,
        };
        self.streams.push(Stream {
            spec: s,
            params,
            hev1: false,
            samples: Vec::new(),
            chunks: Vec::new(),
            pending: Vec::new(),
            pending_samples: 0,
            pending_from: 0,
            last_dts: None,
            head: Vec::new(),
            from: 0,
        });
        Ok(self.streams.len() - 1)
    }

    fn present_from(&mut self, stream: usize, ticks: i64) {
        if let Some(s) = self.streams.get_mut(stream) {
            s.from = ticks;
        }
    }

    fn write(&mut self, p: Packet) -> Result<()> {
        self.check_failed()?;
        let r = self.write_packet(p);
        self.note(r)
    }

    fn finish(self: Box<Self>, tags: &Tags) -> Result<()> {
        self.finish_watched(tags, &mut |_| false)
    }

    fn finish_watched(
        mut self: Box<Self>,
        tags: &Tags,
        watch: &mut dyn FnMut(f64) -> bool,
    ) -> Result<()> {
        self.check_failed()?;
        self.finish_file(tags, watch)
    }
}

impl Mp4Mux {
    /// The error again, if a write has failed.
    fn check_failed(&self) -> Result<()> {
        match &self.failed {
            Some(msg) => Err(encoder(format!("an earlier write failed: {msg}"))),
            None => Ok(()),
        }
    }

    /// `r`, remembering its error.
    fn note(&mut self, r: Result<()>) -> Result<()> {
        if let Err(e) = &r {
            self.failed = Some(e.to_string());
        }
        r
    }

    fn write_packet(&mut self, p: Packet) -> Result<()> {
        let i = p.stream;
        let st = self
            .streams
            .get_mut(i)
            .ok_or_else(|| encoder("a packet came for a stream that was never added"))?;
        // Decode times never go back, and a picture's never repeat.
        let video = st.spec.codec.is_video();
        if st
            .last_dts
            .is_some_and(|last| p.dts < last || (video && p.dts == last))
        {
            return Err(encoder(format!(
                "{} packets came out of decode order",
                st.spec.codec.name()
            )));
        }
        let data = match st.params.as_ref().map(|ps| ps.strip(&p.data)) {
            Some(Stripped::Cleaned(stripped)) => Cow::Owned(stripped),
            Some(Stripped::Foreign) => {
                // A parameter set the hvcC doesn't hold: from here on the
                // samples keep theirs, and the entry says so.
                st.params = None;
                st.hev1 = true;
                Cow::Borrowed(&p.data)
            }
            _ => Cow::Borrowed(&p.data),
        };
        let size = u32::try_from(data.len()).map_err(|_| encoder("a packet is too big"))?;
        let window = (st.spec.timescale / 2).max(1) as i64;
        let new_chunk = st.pending_samples > 0 && p.dts.saturating_sub(st.pending_from) >= window;
        if st.samples.is_empty() && st.spec.codec == StreamCodec::Ac3 {
            st.head = data[..data.len().min(8)].to_vec();
        }
        if new_chunk {
            self.flush(i)?;
        }
        let st = &mut self.streams[i];
        if st.pending_samples == 0 {
            st.pending_from = p.dts;
        }
        st.pending.extend_from_slice(&data);
        st.pending_samples += 1;
        st.last_dts = Some(p.dts);
        st.samples.push(Sample {
            size,
            dts: p.dts,
            pts: p.pts,
            duration: p.duration,
            // Every block of sound can be started at.
            key: p.key || !st.spec.codec.is_video(),
        });
        Ok(())
    }

    fn finish_file(&mut self, tags: &Tags, watch: &mut dyn FnMut(f64) -> bool) -> Result<()> {
        for i in 0..self.streams.len() {
            self.flush(i)?;
        }
        if self.streams.is_empty() || self.streams.iter().any(|s| s.samples.is_empty()) {
            return Err(encoder("a stream got no packets"));
        }
        let plans = self
            .streams
            .iter()
            .enumerate()
            .map(|(i, s)| Plan::new(i as u32 + 1, s))
            .collect::<Result<Vec<_>>>()?;
        let head = self.header(&plans, tags)?;

        let mut out = BufWriter::with_capacity(256 * 1024, &self.out);
        out.write_all(&head)?;
        self.mdat.flush()?;
        let mut samples = self.mdat.get_ref();
        samples.seek(SeekFrom::Start(0))?;
        // The samples are copied a piece at a time, so a cancel lands
        // within one piece and the progress moves with the copy (R92).
        let mut piece = vec![0u8; COPY_PIECE];
        let mut copied = 0u64;
        loop {
            if watch(copied as f64 / self.mdat_len.max(1) as f64) {
                return Err(MediaError::Cancelled);
            }
            let n = match samples.read(&mut piece) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            out.write_all(&piece[..n])?;
            copied += n as u64;
        }
        if copied != self.mdat_len {
            return Err(MediaError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the temporary sample file came back short",
            )));
        }
        out.flush()?;
        drop(out);
        self.out.sync_all().ok();
        Ok(())
    }
}

impl Drop for Mp4Mux {
    fn drop(&mut self) {
        if let Some(temp) = self.temp.take() {
            // The writer's own temporary file, finished or abandoned.
            let _ = std::fs::remove_file(temp);
        }
    }
}

impl Mp4Mux {
    /// Everything before the samples: `ftyp`, `moov`, and the `mdat` header.
    fn header(&self, plans: &[Plan], tags: &Tags) -> Result<Vec<u8>> {
        let ftyp = ftyp(self.kind);
        let payload = self.mdat_len;
        let mdat_head = if payload + 8 > u32::MAX as u64 { 16 } else { 8 };
        // The moov's size doesn't depend on the offsets' values, only on
        // their widths. A track goes to 64-bit offsets when its last chunk
        // starts past 4 GiB, which makes the moov bigger, which can push
        // another track over: so settle it by going round until nothing moves.
        let mut longs = vec![false; plans.len()];
        let base = loop {
            let moov = moov(self.kind, plans, tags, 0, 0, &longs, self.created)?;
            let base = (ftyp.len() + moov.len() + mdat_head) as u64;
            let mut moved = false;
            for (long, plan) in longs.iter_mut().zip(plans) {
                if !*long && self.bias + base + plan.last_offset > u32::MAX as u64 {
                    *long = true;
                    moved = true;
                }
            }
            if !moved {
                break base;
            }
        };
        let mut out = ftyp;
        out.extend(moov(
            self.kind,
            plans,
            tags,
            base,
            self.bias,
            &longs,
            self.created,
        )?);
        if mdat_head == 16 {
            be32(&mut out, 1);
            out.extend_from_slice(b"mdat");
            be64(&mut out, payload + 16);
        } else {
            be32(&mut out, (payload + 8) as u32);
            out.extend_from_slice(b"mdat");
        }
        debug_assert_eq!(out.len() as u64, base);
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// HEVC parameter sets

/// The VPS, SPS and PPS an `hvcC` holds.
struct ParamSets {
    /// The size of each NAL unit's length prefix in a sample.
    length_size: usize,
    sets: Vec<Vec<u8>>,
}

impl ParamSets {
    /// `None` when the `hvcC` doesn't hold all three kinds, in which case the
    /// samples' own are the only ones there are and stay.
    fn from_hvcc(hvcc: &[u8]) -> Option<ParamSets> {
        let length_size = (*hvcc.get(21)? & 3) as usize + 1;
        let arrays = *hvcc.get(22)?;
        let mut at = 23;
        let mut sets = Vec::new();
        let mut kinds = [false; 3];
        for _ in 0..arrays {
            let kind = *hvcc.get(at)? & 0x3F;
            let count = u16::from_be_bytes(hvcc.get(at + 1..at + 3)?.try_into().ok()?);
            at += 3;
            for _ in 0..count {
                let len = u16::from_be_bytes(hvcc.get(at..at + 2)?.try_into().ok()?) as usize;
                let nal = hvcc.get(at + 2..at + 2 + len)?;
                at += 2 + len;
                if (32..=34).contains(&kind) {
                    kinds[kind as usize - 32] = true;
                    sets.push(nal.to_vec());
                }
            }
        }
        kinds
            .iter()
            .all(|&k| k)
            .then_some(ParamSets { length_size, sets })
    }

    /// What to do with a sample: the sample without the NAL units that repeat
    /// the `hvcC`'s; or, if it carries a parameter set the `hvcC` doesn't
    /// hold, the news that stripping must stop; or nothing, when there is
    /// nothing to remove or the sample isn't a clean run of length-prefixed
    /// NAL units.
    fn strip(&self, sample: &[u8]) -> Stripped {
        let ls = self.length_size;
        let mut out = Vec::with_capacity(sample.len());
        let mut at = 0;
        let mut removed = false;
        while at < sample.len() {
            let Some(prefix) = sample.get(at..at + ls) else {
                return Stripped::Nothing;
            };
            let len = prefix.iter().fold(0usize, |n, &b| (n << 8) | b as usize);
            let Some(nal) = at
                .checked_add(ls)
                .and_then(|from| sample.get(from..from.checked_add(len)?))
            else {
                return Stripped::Nothing;
            };
            if self.sets.iter().any(|s| s == nal) {
                removed = true;
            } else {
                // VPS, SPS and PPS are NAL unit types 32 to 34.
                if nal
                    .first()
                    .is_some_and(|b| (32..=34).contains(&((b >> 1) & 0x3F)))
                {
                    return Stripped::Foreign;
                }
                out.extend_from_slice(prefix);
                out.extend_from_slice(nal);
            }
            at += ls + len;
        }
        if removed && !out.is_empty() {
            Stripped::Cleaned(out)
        } else {
            Stripped::Nothing
        }
    }
}

/// What [`ParamSets::strip`] found in a sample.
#[derive(Debug, PartialEq, Eq)]
enum Stripped {
    Nothing,
    Cleaned(Vec<u8>),
    Foreign,
}

// ---------------------------------------------------------------------------
// The tables of one track

struct Plan<'a> {
    id: u32,
    s: &'a Stream,
    /// Runs of (samples, decode-time step).
    stts: Vec<(u32, u32)>,
    /// Runs of (samples, presentation time minus decode time), when they differ.
    ctts: Option<Vec<(u32, i32)>>,
    /// The 1-based numbers of the key samples, unless every sample is one.
    stss: Option<Vec<u32>>,
    /// The track's media time scale.
    scale: u32,
    /// The track's length in it.
    media_duration: u64,
    /// The edit list: (length in the movie's time scale, media time or -1).
    edits: Vec<(u64, i64)>,
    /// The track's length in the movie's time scale.
    track_duration: u64,
    /// The offset in the sample file of the track's last chunk.
    last_offset: u64,
}

impl<'a> Plan<'a> {
    fn new(id: u32, s: &'a Stream) -> Result<Plan<'a>> {
        let n = s.samples.len();
        let from = s.spec.timescale;
        let scale = media_scale(s);
        let backwards = || encoder("a stream's times went backwards or are too far apart");
        // Every time on its own, from the stream's clock to the track's.
        let samples: Vec<Sample> = s
            .samples
            .iter()
            .map(|x| Sample {
                dts: rescale(x.dts, from, scale),
                pts: rescale(x.pts, from, scale),
                ..*x
            })
            .collect();

        // The step to each sample's successor; the last one's own length.
        let mut steps = Vec::with_capacity(n);
        for w in samples.windows(2) {
            steps.push(u32::try_from(w[1].dts - w[0].dts).map_err(|_| backwards())?);
        }
        let last = s.samples[n - 1].duration;
        let last = if last > 0 {
            // Where the last sample ends, on the same footing as the rest.
            let end = rescale(s.samples[n - 1].dts.saturating_add(last), from, scale);
            u32::try_from((end - samples[n - 1].dts).max(1)).map_err(|_| backwards())?
        } else {
            steps.last().copied().unwrap_or((scale / 25).max(1))
        };
        steps.push(last);

        let stts = runs(steps.iter().copied());
        let media_duration = steps.iter().map(|&d| d as u64).sum();

        let dts0 = samples[0].dts;
        let mut offsets = Vec::with_capacity(n);
        for x in &samples {
            offsets.push(i32::try_from(x.pts - x.dts).map_err(|_| backwards())?);
        }
        let ctts = offsets
            .iter()
            .any(|&o| o != 0)
            .then(|| runs(offsets.iter().copied()));

        let stss = (!samples.iter().all(|x| x.key)).then(|| {
            samples
                .iter()
                .enumerate()
                .filter(|(_, x)| x.key)
                .map(|(i, _)| i as u32 + 1)
                .collect()
        });

        // Where the track starts and ends in presentation time.
        let first = samples.iter().map(|x| x.pts).min().unwrap_or(0);
        let end = samples
            .iter()
            .zip(&steps)
            .map(|(x, &d)| x.pts.saturating_add(d as i64))
            .max()
            .unwrap_or(0);
        let start = first.max(rescale(s.from, from, scale)).max(0);
        let media_time = (start - dts0).max(0);
        let shown = rescale((end - start).max(0), scale, MOVIE_SCALE);
        let shown = if end > start { shown.max(1) } else { 0 } as u64;
        let empty = rescale(start, scale, MOVIE_SCALE) as u64;
        let mut edits = Vec::new();
        if empty > 0 {
            edits.push((empty, -1));
        }
        edits.push((shown, media_time));

        Ok(Plan {
            id,
            s,
            stts,
            ctts,
            stss,
            scale,
            media_duration,
            edits,
            track_duration: empty + shown,
            last_offset: s.chunks.last().map_or(0, |c| c.offset),
        })
    }
}

/// The media time scale of a track whose stream counts in `s.spec.timescale`.
fn media_scale(s: &Stream) -> u32 {
    let from = s.spec.timescale;
    if let Some(a) = &s.spec.audio {
        return a.rate;
    }
    if from <= 90_000 {
        return from;
    }
    // A finer clock: 90,000, unless some other common one makes every frame
    // the same length (and 90,000 doesn't).
    let even = |to: u32| {
        let mut steps = s
            .samples
            .windows(2)
            .map(|w| rescale(w[1].dts, from, to) - rescale(w[0].dts, from, to));
        let first = steps.next();
        steps.all(|d| Some(d) == first)
    };
    [90_000, 24_000, 30_000, 60_000, 120_000]
        .into_iter()
        .find(|&to| even(to))
        .unwrap_or(90_000)
}

/// `values` as (count, value) runs.
fn runs<T: PartialEq + Copy>(values: impl Iterator<Item = T>) -> Vec<(u32, T)> {
    let mut out: Vec<(u32, T)> = Vec::new();
    for v in values {
        match out.last_mut() {
            Some((n, last)) if *last == v => *n += 1,
            _ => out.push((1, v)),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The boxes, one function each

fn be16(v: &mut Vec<u8>, n: u16) {
    v.extend_from_slice(&n.to_be_bytes());
}

/// The brands a file of this kind lists.
fn ftyp(kind: Container) -> Vec<u8> {
    let (major, compatible): (&[u8; 4], &[&[u8; 4]]) = match kind {
        Container::Mov => (b"qt  ", &[b"qt  "]),
        Container::M4v => (b"M4V ", &[b"M4V ", b"M4A ", b"mp42"]),
        _ => (b"isom", &[b"isom", b"iso2", b"avc1", b"mp41"]),
    };
    let mut b = major.to_vec();
    be32(&mut b, 0); // minor version
    for brand in compatible {
        b.extend_from_slice(*brand);
    }
    mp4_box(b"ftyp", &b)
}

/// The `tkhd` matrix that turns a picture `rotation` degrees clockwise: a, b,
/// u, c, d, v, x, y, w, with 90 degrees as a=0, b=1, c=-1, d=0 (16.16).
fn matrix(rotation: u16) -> [u32; 9] {
    const ONE: u32 = 0x10000;
    const MINUS_ONE: u32 = 0xFFFF_0000;
    let (a, b, c, d) = match rotation {
        90 => (0, ONE, MINUS_ONE, 0),
        180 => (MINUS_ONE, 0, 0, MINUS_ONE),
        270 => (0, MINUS_ONE, ONE, 0),
        _ => (ONE, 0, 0, ONE),
    };
    [a, b, 0, c, d, 0, 0, 0, 0x4000_0000]
}

/// The movie header; `created` is in seconds since 1904, 0 when unknown.
fn mvhd(plans: &[Plan], created: u64) -> Vec<u8> {
    let duration = plans.iter().map(|p| p.track_duration).max().unwrap_or(0);
    let long = duration.max(created) > u32::MAX as u64;
    let mut b = Vec::new();
    time(&mut b, long, created); // creation
    time(&mut b, long, created); // modification
    be32(&mut b, MOVIE_SCALE);
    time(&mut b, long, duration);
    be32(&mut b, 0x10000); // rate 1.0
    be16(&mut b, 0x0100); // volume 1.0
    b.extend_from_slice(&[0u8; 10]);
    for m in matrix(0) {
        be32(&mut b, m);
    }
    b.extend_from_slice(&[0u8; 24]);
    be32(&mut b, plans.len() as u32 + 1); // next track ID
    full_box(b"mvhd", long as u8, 0, &b)
}

fn tkhd(p: &Plan) -> Vec<u8> {
    let long = p.track_duration > u32::MAX as u64;
    let v = p.s.spec.video.as_ref();
    let mut b = Vec::new();
    time(&mut b, long, 0);
    time(&mut b, long, 0);
    be32(&mut b, p.id);
    be32(&mut b, 0);
    time(&mut b, long, p.track_duration);
    b.extend_from_slice(&[0u8; 8]);
    b.extend_from_slice(&[0, 0, 0, 0]); // layer, alternate group
    be16(&mut b, if v.is_some() { 0 } else { 0x0100 }); // volume
    b.extend_from_slice(&[0, 0]);
    for m in matrix(v.map_or(0, |v| v.rotation)) {
        be32(&mut b, m);
    }
    // The coded size, as 16.16; the matrix turns it.
    be32(&mut b, v.map_or(0, |v| v.width) << 16);
    be32(&mut b, v.map_or(0, |v| v.height) << 16);
    full_box(b"tkhd", long as u8, 7, &b) // enabled, in movie and preview
}

fn edts(p: &Plan) -> Vec<u8> {
    let long = p
        .edits
        .iter()
        .any(|&(d, t)| d > u32::MAX as u64 || t > i32::MAX as i64);
    let mut b = Vec::new();
    be32(&mut b, p.edits.len() as u32);
    for &(duration, media_time) in &p.edits {
        if long {
            be64(&mut b, duration);
            be64(&mut b, media_time as u64);
        } else {
            be32(&mut b, duration as u32);
            be32(&mut b, media_time as i32 as u32);
        }
        be32(&mut b, 0x10000); // rate 1.0
    }
    mp4_box(b"edts", &full_box(b"elst", long as u8, 0, &b))
}

fn mdhd(p: &Plan) -> Vec<u8> {
    let long = p.media_duration > u32::MAX as u64;
    let mut b = Vec::new();
    time(&mut b, long, 0);
    time(&mut b, long, 0);
    be32(&mut b, p.scale);
    time(&mut b, long, p.media_duration);
    be16(&mut b, 0x55C4); // "und"
    be16(&mut b, 0);
    full_box(b"mdhd", long as u8, 0, &b)
}

fn hdlr(kind: Container, video: bool) -> Vec<u8> {
    // QuickTime says "mhlr" (a media handler) where an ISO file has zeros.
    let mut b = if kind == Container::Mov {
        b"mhlr".to_vec()
    } else {
        vec![0u8; 4]
    };
    b.extend_from_slice(if video { b"vide" } else { b"soun" });
    b.extend_from_slice(&[0u8; 12]);
    b.extend_from_slice(if video {
        b"VideoHandler\0"
    } else {
        b"SoundHandler\0"
    });
    full_box(b"hdlr", 0, 0, &b)
}

fn dinf() -> Vec<u8> {
    let mut dref = Vec::new();
    be32(&mut dref, 1);
    dref.extend(full_box(b"url ", 0, 1, &[])); // in this file
    mp4_box(b"dinf", &full_box(b"dref", 0, 0, &dref))
}

/// `colr` (when the picture is HDR, or its colour is to be described),
/// `pasp` (when its pixels aren't square), `mdcv` and `clli`.
fn colour_boxes(v: &super::packet::VideoParams) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(described) = v.described_colour() {
        let mut colr = b"nclx".to_vec();
        // Primaries, transfer and matrix.
        for c in described {
            be16(&mut colr, c as u16);
        }
        // The full range flag, then 7 reserved bits.
        colr.push(if v.full_range() == Some(true) {
            0x80
        } else {
            0
        });
        out.extend(mp4_box(b"colr", &colr));
    }
    if let Some((h, w)) = v.anamorphic() {
        let mut pasp = Vec::new();
        be32(&mut pasp, h);
        be32(&mut pasp, w);
        out.extend(mp4_box(b"pasp", &pasp));
    }
    if let Some(m) = &v.mastering {
        out.extend(mp4_box(b"mdcv", m));
    }
    if let Some(c) = &v.content_light {
        out.extend(mp4_box(b"clli", c));
    }
    out
}

/// A visual sample entry named `name`, holding `boxes`.
fn visual_entry(name: &[u8; 4], v: &super::packet::VideoParams, boxes: Vec<u8>) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    be16(&mut b, 1); // data reference 1
    b.extend_from_slice(&[0u8; 16]); // version, revision, vendor, quality
    be16(&mut b, v.width as u16);
    be16(&mut b, v.height as u16);
    be32(&mut b, 0x0048_0000); // 72 dpi
    be32(&mut b, 0x0048_0000);
    be32(&mut b, 0);
    be16(&mut b, 1); // one frame per sample
    b.extend_from_slice(&[0u8; 32]); // no compressor name
    be16(&mut b, 0x18); // depth
    be16(&mut b, 0xFFFF); // no colour table
    b.extend(boxes);
    mp4_box(name, &b)
}

/// A sound sample entry named `name`, holding `boxes`.
fn audio_entry(name: &[u8; 4], channels: u16, rate: u32, boxes: Vec<u8>) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    be16(&mut b, 1); // data reference 1
    b.extend_from_slice(&[0u8; 8]);
    be16(&mut b, channels);
    be16(&mut b, 16);
    b.extend_from_slice(&[0u8; 4]);
    be32(&mut b, rate.min(0xFFFF) << 16);
    b.extend(boxes);
    mp4_box(name, &b)
}

/// The `dac3` box for an AC-3 stream, from the start of its first frame.
fn dac3(frame: &[u8]) -> Option<Vec<u8>> {
    let f = frame.get(..8)?;
    if f[..2] != [0x0B, 0x77] {
        return None;
    }
    let (fscod, frmsizecod) = (f[4] >> 6, f[4] & 0x3F);
    let (bsid, bsmod) = (f[5] >> 3, f[5] & 7);
    if bsid > 8 {
        return None; // E-AC-3 and later
    }
    let acmod = f[6] >> 5;
    // The optional mix levels sit between acmod and lfeon.
    let mut skip = 0;
    if acmod & 1 != 0 && acmod != 1 {
        skip += 2;
    }
    if acmod & 4 != 0 {
        skip += 2;
    }
    if acmod == 2 {
        skip += 2;
    }
    let bits = u16::from_be_bytes([f[6], f[7]]);
    let lfeon = ((bits >> (16 - 3 - skip - 1)) & 1) as u32;
    let rate_code = (frmsizecod >> 1) as u32;
    let word = (fscod as u32) << 22
        | (bsid as u32) << 17
        | (bsmod as u32) << 14
        | (acmod as u32) << 11
        | lfeon << 10
        | rate_code << 5;
    Some(mp4_box(b"dac3", &word.to_be_bytes()[1..]))
}

fn stsd(p: &Plan) -> Result<Vec<u8>> {
    let s = &p.s.spec;
    let entry = match (&s.codec, &s.video, &s.audio) {
        (codec, Some(v), _) if codec.is_video() => {
            let (name, config) = match codec {
                StreamCodec::H264 { avcc } => (b"avc1", mp4_box(b"avcC", avcc)),
                // `hvc1`, which Apple's players accept, unless the samples
                // carry parameter sets the hvcC doesn't hold.
                StreamCodec::Hevc { hvcc } => (
                    if p.s.hev1 { b"hev1" } else { b"hvc1" },
                    mp4_box(b"hvcC", hvcc),
                ),
                StreamCodec::Av1 { av1c } => (b"av01", mp4_box(b"av1C", av1c)),
                StreamCodec::Vp9 { vpcc } => (b"vp09", mp4_box(b"vpcC", vpcc)),
                _ => unreachable!("is_video"),
            };
            let mut boxes = config;
            boxes.extend(colour_boxes(v));
            visual_entry(name, v, boxes)
        }
        (codec, _, Some(a)) => {
            let sizes = p.s.samples.iter().map(|x| x.size);
            let max = sizes.clone().max().unwrap_or(0);
            let total: u64 = sizes.map(|x| x as u64).sum();
            let rate = p.scale as f64;
            let avg = (total as f64 * 8.0 * rate / p.media_duration.max(1) as f64) as u32;
            // The fastest any one frame goes by.
            let peak =
                p.s.samples
                    .iter()
                    .zip(
                        p.stts
                            .iter()
                            .flat_map(|&(n, d)| std::iter::repeat_n(d, n as usize)),
                    )
                    .filter(|(_, d)| *d > 0)
                    .map(|(x, d)| (x.size as f64 * 8.0 * rate / d as f64) as u32)
                    .max()
                    .unwrap_or(0);
            match codec {
                StreamCodec::Aac { asc } => audio_entry(
                    b"mp4a",
                    a.channels,
                    a.rate,
                    esds_box(asc, max, peak.max(avg), avg),
                ),
                StreamCodec::Ac3 => {
                    let dac3 = dac3(&p.s.head).ok_or_else(|| {
                        encoder("the AC-3 stream doesn't start with an AC-3 frame")
                    })?;
                    audio_entry(b"ac-3", a.channels, a.rate, dac3)
                }
                other => {
                    return Err(encoder(format!(
                        "{} can't go into an MP4 file",
                        other.name()
                    )))
                }
            }
        }
        _ => return Err(encoder("a stream has no facts to describe it by")),
    };
    let mut b = Vec::new();
    be32(&mut b, 1);
    b.extend(entry);
    Ok(full_box(b"stsd", 0, 0, &b))
}

fn stts(p: &Plan) -> Vec<u8> {
    let mut b = Vec::new();
    be32(&mut b, p.stts.len() as u32);
    for &(n, d) in &p.stts {
        be32(&mut b, n);
        be32(&mut b, d);
    }
    full_box(b"stts", 0, 0, &b)
}

fn ctts(runs: &[(u32, i32)]) -> Vec<u8> {
    let mut b = Vec::new();
    be32(&mut b, runs.len() as u32);
    for &(n, o) in runs {
        be32(&mut b, n);
        be32(&mut b, o as u32);
    }
    full_box(b"ctts", 1, 0, &b)
}

fn stss(keys: &[u32]) -> Vec<u8> {
    let mut b = Vec::new();
    be32(&mut b, keys.len() as u32);
    for &k in keys {
        be32(&mut b, k);
    }
    full_box(b"stss", 0, 0, &b)
}

fn stsc(p: &Plan) -> Vec<u8> {
    let mut entries: Vec<(u32, u32)> = Vec::new(); // (first chunk, samples per chunk)
    for (i, c) in p.s.chunks.iter().enumerate() {
        if entries.last().is_none_or(|&(_, n)| n != c.samples) {
            entries.push((i as u32 + 1, c.samples));
        }
    }
    let mut b = Vec::new();
    be32(&mut b, entries.len() as u32);
    for (first, n) in entries {
        be32(&mut b, first);
        be32(&mut b, n);
        be32(&mut b, 1); // sample description 1
    }
    full_box(b"stsc", 0, 0, &b)
}

fn stsz(p: &Plan) -> Vec<u8> {
    let mut b = vec![0u8; 4]; // every sample has its own size
    be32(&mut b, p.s.samples.len() as u32);
    for x in &p.s.samples {
        be32(&mut b, x.size);
    }
    full_box(b"stsz", 0, 0, &b)
}

/// `stco`, or `co64` when `long`. Offsets are counted from `base`, the file
/// offset of the first sample, plus `bias`.
fn chunk_offsets(p: &Plan, base: u64, bias: u64, long: bool) -> Vec<u8> {
    let mut b = Vec::new();
    be32(&mut b, p.s.chunks.len() as u32);
    for c in &p.s.chunks {
        let at = bias + base + c.offset;
        if long {
            be64(&mut b, at);
        } else {
            be32(&mut b, at as u32);
        }
    }
    full_box(if long { b"co64" } else { b"stco" }, 0, 0, &b)
}

fn stbl(p: &Plan, base: u64, bias: u64, long: bool) -> Result<Vec<u8>> {
    let mut b = stsd(p)?;
    b.extend(stts(p));
    if let Some(c) = &p.ctts {
        b.extend(ctts(c));
    }
    if let Some(k) = &p.stss {
        b.extend(stss(k));
    }
    b.extend(stsc(p));
    b.extend(stsz(p));
    b.extend(chunk_offsets(p, base, bias, long));
    Ok(mp4_box(b"stbl", &b))
}

fn trak(kind: Container, p: &Plan, base: u64, bias: u64, long: bool) -> Result<Vec<u8>> {
    let video = p.s.spec.codec.is_video();
    let mut minf = if video {
        full_box(b"vmhd", 0, 1, &[0u8; 8])
    } else {
        full_box(b"smhd", 0, 0, &[0u8; 4])
    };
    minf.extend(dinf());
    minf.extend(stbl(p, base, bias, long)?);
    let mut mdia = mdhd(p);
    mdia.extend(hdlr(kind, video));
    mdia.extend(mp4_box(b"minf", &minf));
    let mut b = tkhd(p);
    b.extend(edts(p));
    b.extend(mp4_box(b"mdia", &mdia));
    Ok(mp4_box(b"trak", &b))
}

fn moov(
    kind: Container,
    plans: &[Plan],
    tags: &Tags,
    base: u64,
    bias: u64,
    longs: &[bool],
    created: u64,
) -> Result<Vec<u8>> {
    let mut b = mvhd(plans, created);
    for (p, &long) in plans.iter().zip(longs) {
        b.extend(trak(kind, p, base, bias, long)?);
    }
    if *tags != Tags::default() {
        b.extend(udta(tags, None));
    }
    Ok(mp4_box(b"moov", &b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_collapse_equal_neighbours() {
        assert_eq!(runs([5, 5, 5, 6, 5].into_iter()), [(3, 5), (1, 6), (1, 5)]);
        assert!(runs(std::iter::empty::<u32>()).is_empty());
    }

    #[test]
    fn rotations_are_the_matrices_ffprobe_reads() {
        // 90 degrees clockwise: a=0, b=1, c=-1, d=0, as 16.16 fixed point.
        assert_eq!(
            matrix(90),
            [0, 0x10000, 0, 0xFFFF_0000, 0, 0, 0, 0, 0x4000_0000]
        );
        assert_eq!(matrix(0)[0], 0x10000);
        assert_eq!(matrix(180)[0], 0xFFFF_0000);
        assert_eq!(matrix(270)[1], 0xFFFF_0000);
    }

    #[test]
    fn the_brands_are_the_ones_listed() {
        let b = ftyp(Container::Mp4);
        assert_eq!(
            b,
            [
                &[0, 0, 0, 32][..],
                b"ftyp",
                b"isom",
                &[0, 0, 0, 0],
                b"isom",
                b"iso2",
                b"avc1",
                b"mp41"
            ]
            .concat()
        );
        assert_eq!(
            ftyp(Container::Mov),
            [&[0, 0, 0, 20][..], b"ftyp", b"qt  ", &[0, 0, 0, 0], b"qt  "].concat()
        );
        assert_eq!(
            ftyp(Container::M4v),
            [
                &[0, 0, 0, 28][..],
                b"ftyp",
                b"M4V ",
                &[0, 0, 0, 0],
                b"M4V ",
                b"M4A ",
                b"mp42"
            ]
            .concat()
        );
    }

    #[test]
    fn ac3_frames_give_their_dac3() {
        // 48 kHz (fscod 0), frmsizecod 28 (192 kbit/s: code 14), bsid 8,
        // bsmod 0, acmod 7 (3/2) with both mix levels, LFE on.
        let mut frame = vec![0x0B, 0x77, 0, 0, 28, 8 << 3, 0, 0];
        // acmod 111, cmixlev 00, surmixlev 00, lfeon 1, then padding.
        frame[6] = 0b1110_0001;
        frame[7] = 0;
        let b = dac3(&frame).unwrap();
        // fscod 0, bsid 8, bsmod 0, acmod 7, lfeon 1, bit rate code 14.
        let word = (8u32 << 17) | (7 << 11) | (1 << 10) | (14 << 5);
        assert_eq!(&b[..8], &[0, 0, 0, 11, b'd', b'a', b'c', b'3']);
        assert_eq!(&b[8..], &word.to_be_bytes()[1..]);
        // Not an AC-3 frame, or E-AC-3 (bsid 16).
        assert!(dac3(&[0; 8]).is_none());
        frame[5] = 16 << 3;
        assert!(dac3(&frame).is_none());
    }

    fn hvcc_with(sets: &[(u8, &[u8])]) -> Vec<u8> {
        let mut b = vec![0u8; 23];
        b[21] = 0x03; // four-byte lengths
        b[22] = sets.len() as u8;
        for (kind, nal) in sets {
            b.push(0x80 | kind);
            b.extend_from_slice(&1u16.to_be_bytes());
            b.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            b.extend_from_slice(nal);
        }
        b
    }

    fn prefixed(nals: &[&[u8]]) -> Vec<u8> {
        nals.iter()
            .flat_map(|n| {
                (n.len() as u32)
                    .to_be_bytes()
                    .into_iter()
                    .chain(n.iter().copied())
            })
            .collect()
    }

    #[test]
    fn in_band_parameter_sets_that_repeat_the_hvcc_are_removed() {
        let (vps, sps, pps) = ([0x40, 1, 9], [0x42, 1, 8, 8], [0x44, 1, 7]);
        let hvcc = hvcc_with(&[(32, &vps), (33, &sps), (34, &pps)]);
        let ps = ParamSets::from_hvcc(&hvcc).unwrap();
        let idr = [0x26, 1, 0xAA, 0xBB];
        let sample = prefixed(&[&vps, &sps, &pps, &idr]);
        assert_eq!(ps.strip(&sample), Stripped::Cleaned(prefixed(&[&idr])));
        // A different SPS is news; a sample with none to remove is nothing.
        let other = [0x42, 1, 5, 5];
        assert_eq!(ps.strip(&prefixed(&[&other, &idr])), Stripped::Foreign);
        assert_eq!(ps.strip(&prefixed(&[&idr])), Stripped::Nothing);
        // A sample that isn't clean NAL units is left alone.
        assert_eq!(ps.strip(&[0, 0, 0, 9, 1]), Stripped::Nothing);
        // An hvcC missing a kind leaves everything in.
        assert!(ParamSets::from_hvcc(&hvcc_with(&[(33, &sps), (34, &pps)])).is_none());
    }
}
