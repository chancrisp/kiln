//! Kiln's own Matroska writer, for any number of video and sound tracks.
//!
//! The file is written in place, front to back: the EBML header, a `Segment`
//! (its size unknown until the end, then patched), a `SeekHead`, `Info`
//! (with a `Duration` that is patched too) and `Tracks`, then `Cluster`s, then
//! the `Cues` and `Tags`; the `SeekHead` is rewritten last to point at them.
//!
//! A cluster is gathered in memory and written whole, so its size is known.
//! It runs about two seconds, and the next one starts on a key frame of the
//! first video stream (or on any packet, with no video). Each cluster with a
//! video key frame gets a cue at it. Block times are in milliseconds, the
//! stream's presentation times rescaled; the blocks are in decode order, as
//! they come.
//!
//! A video track of constant rate says its frame length (`DefaultDuration`),
//! and the frame a video track shows last carries its own length
//! (`BlockDuration`, in a `BlockGroup`), so a reader knows where the picture
//! ends (R80). That frame is known only once a later one is written or the
//! file ends; when a cluster is written before then, the track's latest
//! frame in it carries a length as well.
//!
//! Matroska has no times before 0, so when a stream starts early (the priming
//! of AAC, say), everything is moved later by the same amount, which is
//! settled when the first cluster is written. An Opus stream's block times
//! carry its pre-skip (`CodecDelay`), as the format asks: a packet's `pts` is
//! when its first sample is heard, and the pre-skip is dropped by the decoder.

use super::packet::{check_spec, rescale, Mux, Packet, StreamCodec, StreamSpec, VideoParams};
use crate::error::{MediaError, Result};
use crate::types::Tags;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};

// Element IDs, as the spec writes them.
const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POSITION: u32 = 0x53AC;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const CLUSTER: u32 = 0x1F43_B675;
const CUES: u32 = 0x1C53_BB6B;
const TAGS: u32 = 0x1254_C367;
const VOID: u32 = 0xEC;

/// Milliseconds per tick of the timestamps: the usual, and the only one this
/// writer uses.
const TIMESTAMP_SCALE: u64 = 1_000_000;
/// A cluster runs about this long (ms) before the next key frame starts another.
const CLUSTER_MS: i64 = 2000;
/// The most a cluster holds before it is written, key frame or not.
const CLUSTER_BYTES: usize = 24 << 20;
/// Track numbers fit one byte in a block header.
const MAX_STREAMS: usize = 126;

// ---------------------------------------------------------------------------
// EBML

/// An element ID's bytes.
fn id_bytes(id: u32) -> Vec<u8> {
    let b = id.to_be_bytes();
    let skip = b.iter().take_while(|&&x| x == 0).count().min(3);
    b[skip..].to_vec()
}

/// A size as a variable-length integer, in as few bytes as hold it.
fn size_bytes(n: u64) -> Vec<u8> {
    let len = (1..=8usize)
        .find(|&l| n < (1u64 << (7 * l)) - 1)
        .unwrap_or(8);
    let v = (1u64 << (7 * len)) | n;
    v.to_be_bytes()[8 - len..].to_vec()
}

/// An element: its ID, its size and its body.
fn elem(id: u32, body: &[u8]) -> Vec<u8> {
    let mut out = id_bytes(id);
    out.extend(size_bytes(body.len() as u64));
    out.extend_from_slice(body);
    out
}

/// An unsigned integer element, in as few bytes as hold it.
fn uint(id: u32, v: u64) -> Vec<u8> {
    let b = v.to_be_bytes();
    let skip = b.iter().take_while(|&&x| x == 0).count().min(7);
    elem(id, &b[skip..])
}

/// A signed integer element, in as few bytes as hold it.
fn sint(id: u32, v: i64) -> Vec<u8> {
    let b = v.to_be_bytes();
    let mut skip = 0;
    while skip < 7 && (b[skip] == 0 && b[skip + 1] < 0x80 || b[skip] == 0xFF && b[skip + 1] >= 0x80)
    {
        skip += 1;
    }
    elem(id, &b[skip..])
}

fn float(id: u32, v: f64) -> Vec<u8> {
    elem(id, &v.to_be_bytes())
}

fn text(id: u32, s: &str) -> Vec<u8> {
    elem(id, s.as_bytes())
}

/// A `Void` element `n` bytes long in all (`n` from 2 to 128).
fn void(n: usize) -> Vec<u8> {
    let mut out = vec![VOID as u8, 0x80 | (n - 2) as u8];
    out.resize(n, 0);
    out
}

/// The EBML header: Matroska, written for version 4, readable by version 2.
fn ebml_header() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(uint(0x4286, 1)); // EBMLVersion
    b.extend(uint(0x42F7, 1)); // EBMLReadVersion
    b.extend(uint(0x42F2, 4)); // EBMLMaxIDLength
    b.extend(uint(0x42F3, 8)); // EBMLMaxSizeLength
    b.extend(text(0x4282, "matroska")); // DocType
    b.extend(uint(0x4287, 4)); // DocTypeVersion
    b.extend(uint(0x4285, 2)); // DocTypeReadVersion
    elem(EBML, &b)
}

/// A `SeekHead` pointing at `entries` (an element ID and its position in the
/// segment; `None` leaves the room as a `Void`). Its size doesn't depend on
/// the positions.
fn seek_head(entries: &[(u32, Option<u64>)]) -> Vec<u8> {
    let mut b = Vec::new();
    for &(id, pos) in entries {
        match pos {
            Some(pos) => {
                let mut s = elem(SEEK_ID, &id_bytes(id));
                s.extend(elem(SEEK_POSITION, &pos.to_be_bytes()));
                b.extend(elem(SEEK, &s));
            }
            None => b.extend(void(21)),
        }
    }
    elem(SEEK_HEAD, &b)
}

// ---------------------------------------------------------------------------
// The tracks

fn codec_id(c: &StreamCodec) -> &'static str {
    match c {
        StreamCodec::H264 { .. } => "V_MPEG4/ISO/AVC",
        StreamCodec::Hevc { .. } => "V_MPEGH/ISO/HEVC",
        StreamCodec::Av1 { .. } => "V_AV1",
        StreamCodec::Vp9 { .. } => "V_VP9",
        StreamCodec::Aac { .. } => "A_AAC",
        StreamCodec::Opus { .. } => "A_OPUS",
        StreamCodec::Vorbis { .. } => "A_VORBIS",
        StreamCodec::Flac { .. } => "A_FLAC",
        StreamCodec::Ac3 => "A_AC3",
    }
}

/// What goes in `CodecPrivate`, if anything.
fn codec_private(c: &StreamCodec) -> Option<Vec<u8>> {
    let given = |b: &Vec<u8>| (!b.is_empty()).then(|| b.clone());
    match c {
        StreamCodec::H264 { avcc: b } => given(b),
        StreamCodec::Hevc { hvcc: b } => given(b),
        StreamCodec::Av1 { av1c: b } => given(b),
        StreamCodec::Aac { asc: b } => given(b),
        StreamCodec::Opus { head: b } => given(b),
        StreamCodec::Vorbis { private: b } => given(b),
        // Matroska keeps the whole header: "fLaC", then STREAMINFO as the
        // last metadata block.
        StreamCodec::Flac { streaminfo: b } if b.starts_with(b"fLaC") => given(b),
        StreamCodec::Flac { streaminfo: b } if !b.is_empty() => {
            let mut out = b"fLaC".to_vec();
            out.push(0x80); // the last block; type 0, STREAMINFO
            out.extend_from_slice(&(b.len() as u32).to_be_bytes()[1..]);
            out.extend_from_slice(b);
            Some(out)
        }
        // A VP9 track needs none; AC-3 has none.
        _ => None,
    }
}

fn colour(v: &VideoParams) -> Option<Vec<u8>> {
    let [primaries, transfer, matrix] = v.described_colour()?.map(u64::from);
    let mut b = Vec::new();
    b.extend(uint(0x55B1, matrix)); // MatrixCoefficients
    if v.bit_depth > 0 {
        b.extend(uint(0x55B2, v.bit_depth as u64)); // BitsPerChannel
    }
    if v.tag_colour {
        b.extend(uint(0x55B9, 1)); // Range: broadcast (limited)
    } else if let Some(full) = v.full_range() {
        b.extend(uint(0x55B9, if full { 2 } else { 1 })); // Range, as stated
    }
    b.extend(uint(0x55BA, transfer)); // TransferCharacteristics
    b.extend(uint(0x55BB, primaries)); // Primaries
    if let Some(c) = &v.content_light {
        b.extend(uint(0x55BC, u16::from_be_bytes([c[0], c[1]]) as u64)); // MaxCLL
        b.extend(uint(0x55BD, u16::from_be_bytes([c[2], c[3]]) as u64)); // MaxFALL
    }
    if let Some(m) = &v.mastering {
        let u16_at = |i: usize| u16::from_be_bytes([m[i], m[i + 1]]) as f64 / 50_000.0;
        let u32_at = |i: usize| u32::from_be_bytes([m[i], m[i + 1], m[i + 2], m[i + 3]]) as f64;
        // mdcv lists green, blue and red; Matroska names them.
        let mut md = Vec::new();
        for (id, v) in [
            (0x55D1, u16_at(8)),             // red x
            (0x55D2, u16_at(10)),            // red y
            (0x55D3, u16_at(0)),             // green x
            (0x55D4, u16_at(2)),             // green y
            (0x55D5, u16_at(4)),             // blue x
            (0x55D6, u16_at(6)),             // blue y
            (0x55D7, u16_at(12)),            // white point x
            (0x55D8, u16_at(14)),            // white point y
            (0x55D9, u32_at(16) / 10_000.0), // luminance max
            (0x55DA, u32_at(20) / 10_000.0), // luminance min
        ] {
            md.extend(float(id, v));
        }
        b.extend(elem(0x55D0, &md));
    }
    Some(elem(0x55B0, &b))
}

fn track_entry(number: u8, s: &StreamSpec) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(uint(0xD7, number as u64)); // TrackNumber
    b.extend(uint(0x73C5, 0x4B69_6C6E_0000 + number as u64)); // TrackUID
    b.extend(uint(0x83, if s.codec.is_video() { 1 } else { 2 })); // TrackType
    b.extend(uint(0x9C, 0)); // FlagLacing
    b.extend(text(0x86, codec_id(&s.codec)));
    if let Some(p) = codec_private(&s.codec) {
        b.extend(elem(0x63A2, &p));
    }
    if let Some(ns) = s.video.as_ref().and_then(|v| v.frame_ns) {
        b.extend(uint(0x23_E383, ns)); // DefaultDuration
    }
    if let StreamCodec::Opus { head } = &s.codec {
        if let Some(skip) = head.get(10..12) {
            // The pre-skip is in 48 kHz samples; Matroska wants nanoseconds.
            let pre_skip = u16::from_le_bytes([skip[0], skip[1]]) as u64;
            b.extend(uint(0x56AA, pre_skip * 1_000_000_000 / 48_000)); // CodecDelay
            b.extend(uint(0x56BB, 80_000_000)); // SeekPreRoll
        }
    }
    if let Some(v) = &s.video {
        let mut vb = Vec::new();
        vb.extend(uint(0xB0, v.width as u64));
        vb.extend(uint(0xBA, v.height as u64));
        if let Some((h, w)) = v.anamorphic() {
            // The size shown, in pixels: the width stretched by the pixel
            // aspect ratio, as FFmpeg writes it.
            let shown = (v.width as u64 * h as u64 + w as u64 / 2) / w as u64;
            vb.extend(uint(0x54B0, shown.max(1))); // DisplayWidth
            vb.extend(uint(0x54BA, v.height as u64)); // DisplayHeight
        }
        if let Some(c) = colour(v) {
            vb.extend(c);
        }
        if v.rotation != 0 {
            // A roll of the projection, as FFmpeg writes and reads it: -90 for
            // a picture turned 90 degrees clockwise to show upright.
            let roll = match v.rotation {
                90 => -90.0,
                180 => 180.0,
                _ => 90.0,
            };
            let mut p = uint(0x7671, 0); // ProjectionType: rectangular
            p.extend(float(0x7675, roll)); // ProjectionPoseRoll
            vb.extend(elem(0x7670, &p));
        }
        b.extend(elem(0xE0, &vb));
    }
    if let Some(a) = &s.audio {
        let mut ab = Vec::new();
        ab.extend(float(0xB5, a.rate as f64));
        ab.extend(uint(0x9F, a.channels as u64));
        b.extend(elem(0xE1, &ab));
    }
    elem(0xAE, &b)
}

// ---------------------------------------------------------------------------
// The writer

struct MkvStream {
    spec: StreamSpec,
    /// The 1-based track number.
    number: u8,
    packets: u64,
    /// The last decode time written, for the order check.
    last_dts: Option<i64>,
    /// An Opus stream's pre-skip, in 48 kHz samples.
    pre_skip: u64,
    /// The latest presentation time of its blocks, and the block written
    /// last, in milliseconds.
    shown_last: Option<i64>,
    written_last: Option<i64>,
}

/// One block, waiting for its cluster to be written.
struct Block {
    /// Milliseconds.
    ts: i64,
    track: u8,
    key: bool,
    data: Vec<u8>,
    /// A video frame's length in milliseconds: its packet's, else the gap
    /// from the frame shown before it; 0 when neither is known.
    length: i64,
    /// The time of the track's block before it, for a frame that isn't a
    /// key frame (`ReferenceBlock`). `None` for sound.
    before: Option<i64>,
}

/// A cluster being gathered.
struct Cluster {
    /// Milliseconds: the first block's.
    ts: i64,
    blocks: Vec<Block>,
    bytes: usize,
    /// The first video key frame in it: its track number and time.
    cue: Option<(u8, i64)>,
}

struct Cue {
    time: i64,
    track: u8,
    /// The cluster's position in the segment.
    position: u64,
}

pub(crate) struct MkvMux {
    out: BufWriter<File>,
    /// Bytes written so far.
    pos: u64,
    streams: Vec<MkvStream>,
    started: bool,
    /// Where the segment's size field is, and where its data starts.
    size_at: u64,
    data_at: u64,
    /// Where the Info, Tracks and Duration are in the file.
    info_pos: u64,
    tracks_pos: u64,
    duration_at: u64,
    /// The first video stream: the one whose key frames start clusters.
    primary: Option<usize>,
    cluster: Option<Cluster>,
    cues: Vec<Cue>,
    /// The end of the last block shown, in milliseconds.
    end_ms: i64,
    /// What every time is moved later by so none is before 0; settled when
    /// the first cluster is written.
    shift: Option<i64>,
    /// The text of the first error a write met; the muxer is done after it.
    failed: Option<String>,
    /// `Info/DateUTC` (nanoseconds since 2001) and `Info/Title`.
    date_utc: Option<i64>,
    title: Option<String>,
}

/// `pts` ticks of `scale` per second, plus `extra` samples at 48 kHz, as
/// milliseconds rounded to the nearest (halves up).
fn to_ms(pts: i64, scale: u32, extra: u64) -> i64 {
    if extra == 0 {
        return rescale(pts, scale, 1000);
    }
    let num = (pts as i128 * 48_000 + extra as i128 * scale as i128) * 1000;
    let den = scale as i128 * 48_000;
    let q = num.div_euclid(den);
    let r = num.rem_euclid(den);
    (if r * 2 >= den { q + 1 } else { q }).clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

fn encoder(msg: impl Into<String>) -> MediaError {
    MediaError::Encoder(msg.into())
}

impl MkvMux {
    pub(crate) fn new(out: File) -> MkvMux {
        MkvMux {
            out: BufWriter::with_capacity(256 * 1024, out),
            pos: 0,
            streams: Vec::new(),
            started: false,
            size_at: 0,
            data_at: 0,
            info_pos: 0,
            tracks_pos: 0,
            duration_at: 0,
            primary: None,
            cluster: None,
            cues: Vec::new(),
            end_ms: 0,
            shift: None,
            failed: None,
            date_utc: None,
            title: None,
        }
    }

    fn put(&mut self, bytes: &[u8]) -> Result<()> {
        self.out.write_all(bytes)?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    /// Writes everything before the first cluster.
    fn start(&mut self) -> Result<()> {
        self.started = true;
        self.put(&ebml_header())?;
        self.put(&id_bytes(SEGMENT))?;
        self.size_at = self.pos;
        self.put(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])?; // unknown for now
        self.data_at = self.pos;

        let seek_len = seek_head(&[
            (INFO, Some(0)),
            (TRACKS, Some(0)),
            (CUES, None),
            (TAGS, None),
        ])
        .len();
        let mut info = Vec::new();
        info.extend(uint(0x2AD7B1, TIMESTAMP_SCALE));
        info.extend(text(0x4D80, "Kiln")); // MuxingApp
        info.extend(text(0x5741, "Kiln")); // WritingApp
        if let Some(d) = self.date_utc {
            info.extend(elem(0x4461, &d.to_be_bytes())); // DateUTC
        }
        if let Some(t) = &self.title {
            info.extend(text(0x7BA9, t)); // Title
        }
        info.extend(float(0x4489, 0.0)); // Duration, patched at the end
        let info = elem(INFO, &info);
        let mut tracks = Vec::new();
        for s in &self.streams {
            tracks.extend(track_entry(s.number, &s.spec));
        }
        let tracks = elem(TRACKS, &tracks);

        self.info_pos = seek_len as u64;
        self.tracks_pos = self.info_pos + info.len() as u64;
        self.duration_at = self.data_at + self.info_pos + info.len() as u64 - 8;
        let head = self.seek_head(None, None);
        debug_assert_eq!(head.len(), seek_len);
        self.put(&head)?;
        self.put(&info)?;
        self.put(&tracks)
    }

    fn seek_head(&self, cues: Option<u64>, tags: Option<u64>) -> Vec<u8> {
        seek_head(&[
            (INFO, Some(self.info_pos)),
            (TRACKS, Some(self.tracks_pos)),
            (CUES, cues),
            (TAGS, tags),
        ])
    }

    /// Writes the cluster being gathered, if any. `next` is the track of
    /// the packet that starts the next cluster, `None` at the end.
    fn close_cluster(&mut self, next: Option<u8>) -> Result<()> {
        let Some(c) = self.cluster.take() else {
            return Ok(());
        };
        // R80: each video track's frame shown latest so far carries its
        // length, unless a frame of the track is about to follow.
        let mut lasting = vec![false; c.blocks.len()];
        for s in self.streams.iter().filter(|s| s.spec.codec.is_video()) {
            if next == Some(s.number) {
                continue;
            }
            let latest = c
                .blocks
                .iter()
                .enumerate()
                .filter(|(_, b)| b.track == s.number)
                .max_by_key(|(_, b)| b.ts);
            if let Some((i, b)) = latest {
                lasting[i] = Some(b.ts) == s.shown_last && b.length > 0;
            }
        }
        let shift = *self.shift.get_or_insert_with(|| {
            let first = c.blocks.iter().map(|b| b.ts).min().unwrap_or(0);
            (-first).max(0)
        });
        let position = self.pos - self.data_at;
        let at = (c.ts + shift).max(0);
        let mut body = uint(0xE7, at as u64);
        for (b, lasting) in c.blocks.iter().zip(lasting) {
            // A SimpleBlock: the track, the time within the cluster, flags, the data.
            if b.ts + shift < 0 {
                return Err(encoder("a packet is earlier than the start of the file"));
            }
            // The span from the cluster's first block was checked to fit when
            // the block arrived.
            let rel = b.ts + shift - at;
            debug_assert!(i16::try_from(rel).is_ok());
            let rel = rel as i16;
            let mut block = vec![0x80 | b.track];
            block.extend_from_slice(&rel.to_be_bytes());
            if lasting {
                // A BlockGroup: the Block (no flags), its length, and for a
                // frame that isn't a key frame, the one before it.
                block.push(0);
                block.extend_from_slice(&b.data);
                let mut group = elem(0xA1, &block);
                group.extend(uint(0x9B, b.length as u64)); // BlockDuration
                if !b.key {
                    let back = b.before.map_or(-1, |t| t - b.ts);
                    group.extend(sint(0xFB, if back == 0 { -1 } else { back }));
                    // ReferenceBlock
                }
                body.extend(elem(0xA0, &group));
                continue;
            }
            block.push(if b.key { 0x80 } else { 0 });
            block.extend_from_slice(&b.data);
            body.extend(elem(0xA3, &block));
        }
        self.put(&id_bytes(CLUSTER))?;
        self.put(&size_bytes(body.len() as u64))?;
        self.put(&body)?;
        if let Some((track, time)) = c.cue {
            self.cues.push(Cue {
                time: (time + shift).max(0),
                track,
                position,
            });
        }
        Ok(())
    }
}

impl Mux for MkvMux {
    fn describe(&mut self, created: Option<i64>, title: Option<&str>) {
        let epoch = super::info::MATROSKA_EPOCH;
        self.date_utc = created
            .and_then(|s| s.checked_sub(epoch))
            .and_then(|s| s.checked_mul(1_000_000_000));
        self.title = title
            .map(|t| t.replace('\0', ""))
            .filter(|t| !t.trim().is_empty());
    }

    fn add_stream(&mut self, s: StreamSpec) -> Result<usize> {
        check_spec(&s)?;
        if self.started {
            return Err(encoder("a stream was added after the first packet"));
        }
        if self.streams.len() >= MAX_STREAMS {
            return Err(encoder("too many streams"));
        }
        if self.primary.is_none() && s.codec.is_video() {
            self.primary = Some(self.streams.len());
        }
        let pre_skip = match &s.codec {
            StreamCodec::Opus { head } => head
                .get(10..12)
                .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as u64),
            _ => 0,
        };
        self.streams.push(MkvStream {
            spec: s,
            number: self.streams.len() as u8 + 1,
            packets: 0,
            last_dts: None,
            pre_skip,
            shown_last: None,
            written_last: None,
        });
        Ok(self.streams.len() - 1)
    }

    fn write(&mut self, p: Packet) -> Result<()> {
        self.check_failed()?;
        let r = self.write_packet(p);
        self.note(r)
    }

    fn finish(mut self: Box<Self>, tags: &Tags) -> Result<()> {
        self.check_failed()?;
        let r = self.finish_file(tags);
        self.note(r)
    }
}

impl MkvMux {
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
        let idx = p.stream;
        let shift = self.shift;
        let (number, video, ts, end, length, before) = {
            let s = self
                .streams
                .get_mut(idx)
                .ok_or_else(|| encoder("a packet came for a stream that was never added"))?;
            // Decode times never go back, and a picture's never repeat.
            let video = s.spec.codec.is_video();
            if s.last_dts
                .is_some_and(|last| p.dts < last || (video && p.dts == last))
            {
                return Err(encoder(format!(
                    "{} packets came out of decode order",
                    s.spec.codec.name()
                )));
            }
            s.last_dts = Some(p.dts);
            s.packets += 1;
            let scale = s.spec.timescale;
            let ts = to_ms(p.pts, scale, s.pre_skip);
            let end = to_ms(p.pts.saturating_add(p.duration.max(0)), scale, s.pre_skip);
            // A frame with no length of its own lasts the gap from the frame
            // shown before it, when it is shown last so far.
            let gap = s.shown_last.filter(|&l| ts > l).map_or(0, |l| ts - l);
            let length = if end > ts { end - ts } else { gap };
            let before = s.written_last;
            s.written_last = Some(ts);
            s.shown_last = Some(s.shown_last.map_or(ts, |l| l.max(ts)));
            (s.number, video, ts, end, length, before)
        };
        // Once the first cluster is written and the times moved, a packet
        // before the start of the file can't be given a place.
        if shift.is_some_and(|sh| ts + sh < 0) {
            return Err(encoder("a packet is earlier than the start of the file"));
        }
        if !self.started {
            self.start()?;
        }
        self.end_ms = self.end_ms.max(end);
        // Every block of sound can be started at.
        let key = p.key || !video;

        let split = match &self.cluster {
            None => true,
            Some(c) => {
                let span = ts - c.ts;
                let due = match self.primary {
                    Some(v) => v == idx && key && span >= CLUSTER_MS,
                    None => span >= CLUSTER_MS,
                };
                due || i16::try_from(span).is_err() || c.bytes >= CLUSTER_BYTES
            }
        };
        if split {
            self.close_cluster(Some(number))?;
            self.cluster = Some(Cluster {
                ts,
                blocks: Vec::new(),
                bytes: 0,
                cue: None,
            });
        }
        let c = self.cluster.as_mut().expect("a cluster was just made");
        let cues_here = match self.primary {
            Some(v) => v == idx && key,
            None => true,
        };
        if cues_here && c.cue.is_none() {
            c.cue = Some((number, ts));
        }
        c.bytes += p.data.len();
        c.blocks.push(Block {
            ts,
            track: number,
            key,
            data: p.data,
            length,
            before: video.then_some(before).flatten(),
        });
        Ok(())
    }

    fn finish_file(&mut self, tags: &Tags) -> Result<()> {
        if !self.started || self.streams.iter().any(|s| s.packets == 0) {
            return Err(encoder("a stream got no packets"));
        }
        self.close_cluster(None)?;

        let mut cues_pos = None;
        if !self.cues.is_empty() {
            let mut body = Vec::new();
            for c in &self.cues {
                let mut pos = uint(0xF7, c.track as u64); // CueTrack
                pos.extend(uint(0xF1, c.position)); // CueClusterPosition
                let mut point = uint(0xB3, c.time.max(0) as u64); // CueTime
                point.extend(elem(0xB7, &pos));
                body.extend(elem(0xBB, &point));
            }
            cues_pos = Some(self.pos - self.data_at);
            self.put(&elem(CUES, &body))?;
        }
        let mut tags_pos = None;
        if let Some(t) = tags_element(tags) {
            tags_pos = Some(self.pos - self.data_at);
            self.put(&t)?;
        }

        // Fill in what wasn't known: the segment's size, the duration, and
        // where the cues and tags are.
        let shift = self.shift.unwrap_or(0);
        let end = self.pos;
        let size = (end - self.data_at).min((1 << 56) - 2);
        let mut field = vec![0x01];
        field.extend_from_slice(&size.to_be_bytes()[1..]);
        let head = self.seek_head(cues_pos, tags_pos);
        let patches: [(u64, Vec<u8>); 3] = [
            (self.size_at, field),
            (
                self.duration_at,
                ((self.end_ms + shift) as f64).to_be_bytes().to_vec(),
            ),
            (self.data_at, head),
        ];
        for (at, bytes) in patches {
            self.out.seek(SeekFrom::Start(at))?;
            self.out.write_all(&bytes)?;
        }
        self.out.flush()?;
        self.out.get_ref().sync_all().ok();
        Ok(())
    }
}

/// The `Tags` element for what `tags` holds; `None` when it holds nothing.
fn tags_element(tags: &Tags) -> Option<Vec<u8>> {
    let mut simple = Vec::new();
    let mut add = |name: &str, v: &Option<String>| {
        if let Some(v) = v.as_deref().map(|s| s.replace('\0', "")) {
            if !v.is_empty() {
                let mut s = text(0x45A3, name);
                s.extend(text(0x4487, &v));
                simple.extend(elem(0x67C8, &s));
            }
        }
    };
    add("TITLE", &tags.title);
    add("ARTIST", &tags.artist);
    add("ALBUM", &tags.album);
    add("GENRE", &tags.genre);
    add("COMMENT", &tags.comment);
    add("DATE_RELEASED", &tags.year);
    if simple.is_empty() {
        return None;
    }
    let mut tag = elem(0x63C0, &[]); // Targets: the whole file
    tag.extend(simple);
    Some(elem(TAGS, &elem(0x7373, &tag)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::info::Hdr;

    #[test]
    fn sizes_use_the_fewest_bytes() {
        assert_eq!(size_bytes(0), [0x80]);
        assert_eq!(size_bytes(126), [0xFE]);
        // 127 is all ones in one byte: the "unknown" size, so it takes two.
        assert_eq!(size_bytes(127), [0x40, 0x7F]);
        assert_eq!(size_bytes(300), [0x41, 0x2C]);
        assert_eq!(size_bytes(16_382), [0x7F, 0xFE]);
        assert_eq!(size_bytes(16_383), [0x20, 0x3F, 0xFF]);
    }

    #[test]
    fn ids_and_integers_are_minimal() {
        assert_eq!(id_bytes(0xA3), [0xA3]);
        assert_eq!(id_bytes(0x4489), [0x44, 0x89]);
        assert_eq!(id_bytes(EBML), [0x1A, 0x45, 0xDF, 0xA3]);
        assert_eq!(uint(0xD7, 0), [0xD7, 0x81, 0]);
        assert_eq!(uint(0xD7, 256), [0xD7, 0x82, 1, 0]);
    }

    #[test]
    fn signed_integers_are_minimal() {
        assert_eq!(sint(0xFB, -1), [0xFB, 0x81, 0xFF]);
        assert_eq!(sint(0xFB, -33), [0xFB, 0x81, 0xDF]);
        assert_eq!(sint(0xFB, -200), [0xFB, 0x82, 0xFF, 0x38]);
        assert_eq!(sint(0xFB, 100), [0xFB, 0x81, 100]);
        assert_eq!(sint(0xFB, 200), [0xFB, 0x82, 0, 200]);
    }

    /// An H.264 stream on a millisecond clock, `frame_ns` its rate.
    fn h264(frame_ns: Option<u64>) -> StreamSpec {
        StreamSpec {
            codec: StreamCodec::H264 {
                avcc: vec![
                    1, 0x64, 0, 0x28, 0xFF, 0xE1, 0, 4, 0x67, 0x64, 0, 0x28, 1, 0, 2, 0x68, 0xEE,
                ],
            },
            timescale: 1000,
            video: Some(VideoParams {
                width: 64,
                height: 48,
                rotation: 0,
                hdr: Hdr::None,
                bit_depth: 8,
                mastering: None,
                content_light: None,
                sar: (1, 1),
                tag_colour: false,
                bt601: false,
                frame_ns,
                stated_colour: None,
            }),
            audio: None,
        }
    }

    /// Seven frames with B-frames, in decode order, each `length` ms long:
    /// the frame shown last (200 ms) is written fifth.
    fn written(frame_ns: Option<u64>, length: i64) -> tempfile::NamedTempFile {
        let f = tempfile::NamedTempFile::new().unwrap();
        let mut m = MkvMux::new(f.reopen().unwrap());
        m.add_stream(h264(frame_ns)).unwrap();
        for (i, pts) in [0, 100, 33, 67, 200, 133, 167].into_iter().enumerate() {
            m.write_packet(Packet {
                stream: 0,
                pts,
                dts: i as i64 * 10 - 30,
                duration: length,
                key: i == 0,
                data: vec![0, 0, 0, 1, 0x65 - (i > 0) as u8 * 4, i as u8],
            })
            .unwrap();
        }
        m.finish_file(&Tags::default()).unwrap();
        f
    }

    #[test]
    fn a_frame_length_of_nothing_is_refused() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let mut m = MkvMux::new(f.reopen().unwrap());
        assert!(m.add_stream(h264(Some(0))).is_err());
        assert!(m.add_stream(h264(Some(33_333_333))).is_ok());
    }

    #[test]
    fn a_constant_rate_says_its_frame_length_and_the_frame_shown_last_its_own() {
        let f = written(Some(33_333_333), 33);
        let mut file = f.reopen().unwrap();
        let h = crate::mkvread::read_headers(&mut file).unwrap();
        assert_eq!(h.tracks[0].default_duration, Some(33_333_333));
        let blocks: Vec<_> = crate::mkvread::Blocks::new(f.reopen().unwrap(), 1)
            .unwrap()
            .map(|b| b.unwrap())
            .collect();
        assert_eq!(blocks.len(), 7);
        let lasting: Vec<_> = blocks.iter().filter(|b| b.duration.is_some()).collect();
        assert_eq!(lasting.len(), 1, "{blocks:?}");
        assert_eq!((lasting[0].time, lasting[0].duration), (200, Some(33)));
        // That frame isn't a key frame: after its data, its length, then
        // the block written before it (67 ms), 133 ms back.
        let bytes = std::fs::read(f.path()).unwrap();
        let data = [0, 0, 0, 1, 0x61, 4];
        let at = bytes.windows(6).position(|w| w == data).unwrap() + 6;
        assert_eq!(bytes[at..at + 7], [0x9B, 0x81, 33, 0xFB, 0x82, 0xFF, 0x7B]);
    }

    #[test]
    fn a_variable_rate_says_none_and_a_frame_with_no_length_lasts_the_gap() {
        let f = written(None, 0);
        let h = crate::mkvread::read_headers(&mut f.reopen().unwrap()).unwrap();
        assert_eq!(h.tracks[0].default_duration, None);
        let lasting: Vec<_> = crate::mkvread::Blocks::new(f.reopen().unwrap(), 1)
            .unwrap()
            .map(|b| b.unwrap())
            .filter(|b| b.duration.is_some())
            .collect();
        // The frame at 200 ms came after the one at 100: 100 ms.
        assert_eq!(lasting.len(), 1);
        assert_eq!((lasting[0].time, lasting[0].duration), (200, Some(100)));
    }

    #[test]
    fn the_seek_head_has_a_fixed_size() {
        let a = seek_head(&[(INFO, Some(0)), (CUES, None)]);
        let b = seek_head(&[(INFO, Some(u64::MAX)), (CUES, Some(1))]);
        assert_eq!(a.len(), b.len());
        assert_eq!(void(21).len(), 21);
        assert_eq!(void(21)[..2], [0xEC, 0x80 | 19]);
    }

    #[test]
    fn a_flac_streaminfo_gets_its_header() {
        let p = codec_private(&StreamCodec::Flac {
            streaminfo: vec![7; 34],
        })
        .unwrap();
        assert_eq!(&p[..8], b"fLaC\x80\0\0\x22");
        assert_eq!(p.len(), 42);
    }
}
