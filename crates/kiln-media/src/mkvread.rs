//! What a Matroska or WebM file says about its tracks: an EBML element walker,
//! and the `Info` and `Tracks` it finds in the first level of the `Segment`.
//!
//! Only the headers are read. A `Segment` is walked element by element,
//! hopping over each `Cluster` by its size, until both `Info` and `Tracks`
//! have been read (they come before the clusters in nearly every file). Each
//! of them is capped at 64 MB.
//!
//! [`Blocks`] reads one track's block times, cluster by cluster, as they are
//! asked for (R78).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// The biggest `Info` or `Tracks` that is read.
const MAX_HEADER: u64 = 64 * 1024 * 1024;
/// The most top-level elements hopped over looking for them.
const MAX_HOPS: u32 = 100_000;
/// The most elements looked at inside one `Info`, `TrackEntry`, `Video` or
/// `Colour`, and the most `TrackEntry`s kept. A real file has a few dozen.
const MAX_ELEMENTS: usize = 4096;
const MAX_TRACKS: usize = 256;
/// The biggest `CodecPrivate` that is kept: real ones are a few KB.
const MAX_CODEC_PRIVATE: usize = 1 << 20;

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const DURATION: u32 = 0x4489;
const DATE_UTC: u32 = 0x4461;
const TITLE: u32 = 0x7BA9;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63A2;
const DEFAULT_DURATION: u32 = 0x23_E383;
const CODEC_DELAY: u32 = 0x56AA;
const VIDEO: u32 = 0xE0;
const AUDIO: u32 = 0xE1;
const SAMPLING_FREQUENCY: u32 = 0xB5;
const CHANNELS: u32 = 0x9F;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const DISPLAY_WIDTH: u32 = 0x54B0;
const DISPLAY_HEIGHT: u32 = 0x54BA;
const DISPLAY_UNIT: u32 = 0x54B2;
const COLOUR: u32 = 0x55B0;
const BITS_PER_CHANNEL: u32 = 0x55B2;
const TRANSFER: u32 = 0x55BA;
const MATRIX: u32 = 0x55B1;
const PRIMARIES: u32 = 0x55BB;
const RANGE: u32 = 0x55B9;
const MAX_CLL: u32 = 0x55BC;
const MAX_FALL: u32 = 0x55BD;
const MASTERING: u32 = 0x55D0;
/// `MasteringMetadata`'s children, red x to luminance min, in their order.
const MASTERING_FIELDS: [u32; 10] = [
    0x55D1, 0x55D2, 0x55D3, 0x55D4, 0x55D5, 0x55D6, 0x55D7, 0x55D8, 0x55D9, 0x55DA,
];

/// `TrackType` of a video track.
pub(crate) const TYPE_VIDEO: u64 = 1;
/// `TrackType` of a sound track.
pub(crate) const TYPE_AUDIO: u64 = 2;
/// `TrackType` of a subtitle track.
pub(crate) const TYPE_SUBTITLE: u64 = 17;

/// The element at the start of `b`: its ID (with its length marker, as the
/// spec writes IDs), its size (`None` when unknown) and the header's length.
pub(crate) fn header(b: &[u8]) -> Option<(u32, Option<u64>, usize)> {
    let first = *b.first()?;
    let id_len = first.leading_zeros() as usize + 1;
    if id_len > 4 {
        return None;
    }
    let id = b
        .get(..id_len)?
        .iter()
        .fold(0u32, |acc, &x| (acc << 8) | x as u32);
    let size_first = *b.get(id_len)?;
    let size_len = size_first.leading_zeros() as usize + 1;
    if size_len > 8 {
        return None;
    }
    let mut size = ((size_first as u32) & (0xFF >> size_len)) as u64;
    for i in 1..size_len {
        size = (size << 8) | *b.get(id_len + i)? as u64;
    }
    let unknown = size == (1u64 << (7 * size_len)) - 1;
    Some((id, (!unknown).then_some(size), id_len + size_len))
}

/// The elements one level down in a master element's body, one at a time, as
/// (ID, body). Stops at the first one that is cut short or can't be read; an
/// element of unknown size takes the rest. Nothing is allocated, so a body of
/// millions of tiny elements costs time but not memory.
pub(crate) struct Elements<'a>(&'a [u8]);

impl<'a> Iterator for Elements<'a> {
    type Item = (u32, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let (id, size, head) = header(self.0)?;
        let rest = &self.0[head..];
        let body = match size {
            Some(s) if s <= rest.len() as u64 => &rest[..s as usize],
            Some(_) => return None,
            None => rest,
        };
        self.0 = &rest[body.len()..];
        Some((id, body))
    }
}

pub(crate) fn elements(b: &[u8]) -> Elements<'_> {
    Elements(b)
}

/// An unsigned integer element (0 to 8 bytes, big endian).
pub(crate) fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0, |acc, &x| (acc << 8) | x as u64)
}

fn float(b: &[u8]) -> Option<f64> {
    match b.len() {
        4 => Some(f32::from_be_bytes(b.try_into().ok()?) as f64),
        8 => Some(f64::from_be_bytes(b.try_into().ok()?)),
        _ => None,
    }
}

fn string(b: &[u8]) -> String {
    let end = b.iter().position(|&x| x == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// What `Segment/Info` and `Segment/Tracks` hold.
#[derive(Debug, Clone)]
pub(crate) struct Headers {
    /// Nanoseconds per tick of the timestamps; 1,000,000 unless the file says.
    pub timestamp_scale: u64,
    /// `Info/Duration`, in ticks.
    pub duration: Option<f64>,
    /// `Info/DateUTC`: when the file was made, in nanoseconds since
    /// 2001-01-01 00:00:00 UTC.
    pub date_utc: Option<i64>,
    /// `Info/Title`.
    pub title: Option<String>,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Track {
    /// `TrackNumber`: what the blocks of the track name it by.
    pub number: u64,
    /// `TrackType`: [`TYPE_VIDEO`], [`TYPE_AUDIO`], [`TYPE_SUBTITLE`] or
    /// another (logo, buttons, control).
    pub kind: u64,
    pub codec_id: String,
    pub codec_private: Vec<u8>,
    /// Nanoseconds per frame; `None` when the track doesn't say.
    pub default_duration: Option<u64>,
    /// `CodecDelay`: nanoseconds of decoded output to drop at the start.
    pub codec_delay: Option<u64>,
    pub video: Option<Video>,
    /// `Audio`: `SamplingFrequency` and `Channels`, when given.
    pub audio: Option<(f64, u64)>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Video {
    pub width: u32,
    pub height: u32,
    /// `DisplayWidth` and `DisplayHeight`, and `DisplayUnit`: 0 pixels (also
    /// when absent), 1 centimetres, 2 inches, 3 an aspect ratio.
    pub display_width: Option<u64>,
    pub display_height: Option<u64>,
    pub display_unit: u64,
    /// `Colour/TransferCharacteristics`.
    pub transfer: Option<u64>,
    /// `Colour/MatrixCoefficients` and `Colour/Primaries`.
    pub matrix: Option<u64>,
    pub primaries: Option<u64>,
    /// `Colour/Range`: 1 broadcast (limited), 2 full; 0 or absent unstated.
    pub range: Option<u64>,
    /// `Colour/BitsPerChannel`, when it is set.
    pub bits: Option<u64>,
    /// True when `Colour` has `MaxCLL`, `MaxFALL` or `MasteringMetadata`.
    pub hdr10_metadata: bool,
    /// `MaxCLL` and `MaxFALL`, in nits.
    pub max_cll: Option<u64>,
    pub max_fall: Option<u64>,
    /// `MasteringMetadata`: red, green and blue x and y, the white point's x
    /// and y, then the highest and lowest luminance in nits. Present only
    /// when all ten are given.
    pub mastering: Option<[f64; 10]>,
}

/// What `Info` holds: the time scale, the duration, when the file was made
/// and its title.
struct Info {
    scale: u64,
    duration: Option<f64>,
    date_utc: Option<i64>,
    title: Option<String>,
}

fn parse_info(body: &[u8]) -> Info {
    let mut info = Info {
        scale: 1_000_000,
        duration: None,
        date_utc: None,
        title: None,
    };
    for (id, v) in elements(body).take(MAX_ELEMENTS) {
        match id {
            TIMESTAMP_SCALE => info.scale = uint(v),
            DURATION => info.duration = float(v),
            // A signed 8-byte count of nanoseconds since 2001.
            DATE_UTC if v.len() == 8 => {
                info.date_utc = Some(i64::from_be_bytes(v.try_into().unwrap_or([0; 8])))
            }
            TITLE => info.title = Some(string(v)).filter(|t| !t.trim().is_empty()),
            _ => {}
        }
    }
    info
}

fn parse_video(body: &[u8]) -> Video {
    let mut video = Video::default();
    for (id, v) in elements(body).take(MAX_ELEMENTS) {
        match id {
            PIXEL_WIDTH => video.width = u32::try_from(uint(v)).unwrap_or(0),
            PIXEL_HEIGHT => video.height = u32::try_from(uint(v)).unwrap_or(0),
            DISPLAY_WIDTH => video.display_width = Some(uint(v)).filter(|&d| d > 0),
            DISPLAY_HEIGHT => video.display_height = Some(uint(v)).filter(|&d| d > 0),
            DISPLAY_UNIT => video.display_unit = uint(v),
            COLOUR => {
                for (id, v) in elements(v).take(MAX_ELEMENTS) {
                    match id {
                        TRANSFER => video.transfer = Some(uint(v)),
                        MATRIX => video.matrix = Some(uint(v)),
                        PRIMARIES => video.primaries = Some(uint(v)),
                        RANGE => video.range = Some(uint(v)),
                        BITS_PER_CHANNEL => video.bits = Some(uint(v)).filter(|&b| b > 0),
                        MAX_CLL | MAX_FALL | MASTERING => {
                            video.hdr10_metadata = true;
                            match id {
                                MAX_CLL => video.max_cll = Some(uint(v)),
                                MAX_FALL => video.max_fall = Some(uint(v)),
                                _ => video.mastering = parse_mastering(v),
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    video
}

/// The ten values of a `MasteringMetadata`, when it has them all.
fn parse_mastering(body: &[u8]) -> Option<[f64; 10]> {
    let mut out = [None; 10];
    for (id, v) in elements(body).take(MAX_ELEMENTS) {
        if let Some(i) = MASTERING_FIELDS.iter().position(|&f| f == id) {
            out[i] = float(v).filter(|x| x.is_finite() && *x >= 0.0);
        }
    }
    let mut all = [0.0; 10];
    for (slot, v) in all.iter_mut().zip(out) {
        *slot = v?;
    }
    Some(all)
}

fn parse_tracks(body: &[u8]) -> Vec<Track> {
    let mut tracks = Vec::new();
    let entries = elements(body)
        .take(MAX_ELEMENTS * 16)
        .filter(|&(id, _)| id == TRACK_ENTRY)
        .take(MAX_TRACKS);
    for (_, entry) in entries {
        let mut t = Track::default();
        for (id, v) in elements(entry).take(MAX_ELEMENTS) {
            match id {
                TRACK_NUMBER => t.number = uint(v),
                TRACK_TYPE => t.kind = uint(v),
                CODEC_ID => t.codec_id = string(v),
                CODEC_PRIVATE if v.len() <= MAX_CODEC_PRIVATE => t.codec_private = v.to_vec(),
                DEFAULT_DURATION => t.default_duration = Some(uint(v)).filter(|&d| d > 0),
                CODEC_DELAY => t.codec_delay = Some(uint(v)),
                VIDEO => t.video = Some(parse_video(v)),
                AUDIO => {
                    let (mut rate, mut channels) = (8000.0, 1);
                    for (id, v) in elements(v).take(MAX_ELEMENTS) {
                        match id {
                            SAMPLING_FREQUENCY => rate = float(v).unwrap_or(rate),
                            CHANNELS => channels = uint(v),
                            _ => {}
                        }
                    }
                    t.audio = Some((rate, channels));
                }
                _ => {}
            }
        }
        tracks.push(t);
    }
    tracks
}

/// The element header at `pos`, read from the file.
fn header_at(file: &mut File, pos: u64) -> Option<(u32, Option<u64>, u64)> {
    let mut buf = Vec::with_capacity(12);
    file.seek(SeekFrom::Start(pos)).ok()?;
    file.by_ref().take(12).read_to_end(&mut buf).ok()?;
    let (id, size, head) = header(&buf)?;
    Some((id, size, head as u64))
}

/// The top-level elements of a Segment, which end a Cluster of unknown size.
const LEVEL1: [u32; 8] = [
    0x114D_9B74, // SeekHead
    INFO,
    TRACKS,
    0x1C53_BB6B, // Cues
    0x1043_A770, // Chapters
    0x1941_A469, // Attachments
    0x1254_C367, // Tags
    CLUSTER,
];
const CLUSTER: u32 = 0x1F43_B675;

/// A window of a file for walking element headers: just a header's bytes
/// where the walk jumps, more where it steps through small elements.
struct Window<'a> {
    file: &'a mut File,
    start: u64,
    buf: Vec<u8>,
}

impl Window<'_> {
    /// Up to 12 bytes at `pos`: fewer only at the end of the file. When
    /// they aren't already read, reads `ahead` bytes (at least 12) from
    /// `pos`.
    fn head(&mut self, pos: u64, ahead: u64) -> &[u8] {
        let end = self.start + self.buf.len() as u64;
        if pos < self.start || pos + 12 > end {
            self.buf.clear();
            self.start = pos;
            if self.file.seek(SeekFrom::Start(pos)).is_ok() {
                let _ = self
                    .file
                    .by_ref()
                    .take(ahead.max(12))
                    .read_to_end(&mut self.buf);
            }
        }
        let from = (pos - self.start) as usize;
        &self.buf[from.min(self.buf.len())..(from + 12).min(self.buf.len())]
    }
}

/// True when a Matroska file's own structure says it was cut short: its
/// Segment's size, a Cluster's size, or the size of an element in a Cluster
/// of unknown size, runs past the end of the file, or an element header is
/// cut off by it. A file cut exactly between blocks of a stream written with
/// no sizes can't be told from a whole one. Leaves the position anywhere.
pub(crate) fn cut_short(file: &mut File) -> bool {
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return false;
    };
    let mut w = Window {
        file,
        start: 0,
        buf: Vec::new(),
    };
    // An element header at `pos`, reading `ahead` bytes if it must:
    // `Err(true)` when the file ends inside it, `Err(false)` when it isn't
    // one. A jump past a sized element reads only the next header; the
    // small elements of a Cluster of unknown size are read 64 KiB at a time.
    const JUMP: u64 = 12;
    const STEP: u64 = 64 * 1024;
    let mut at = |pos: u64, ahead: u64| -> Result<(u32, Option<u64>, u64), bool> {
        let b = w.head(pos, ahead);
        match header(b) {
            Some((id, size, head)) => Ok((id, size, head as u64)),
            None => Err(b.len() < 12),
        }
    };
    let Ok((EBML, Some(size), head)) = at(0, JUMP) else {
        return false;
    };
    let segment = head.saturating_add(size);
    let Ok((SEGMENT, size, head)) = at(segment, JUMP) else {
        return false;
    };
    let start = segment + head;
    if size.is_some_and(|s| start.saturating_add(s) > len) {
        return true;
    }
    let end = size.map_or(len, |s| start + s);
    let mut pos = start;
    while pos < end {
        let (id, size, head) = match at(pos, JUMP) {
            Ok(h) => h,
            Err(cut) => return cut,
        };
        match size {
            Some(s) => {
                let next = pos.saturating_add(head).saturating_add(s);
                if next > len {
                    return true;
                }
                pos = next;
            }
            None if id == CLUSTER => {
                // Walk its children until the next top-level element.
                let mut p = pos + head;
                loop {
                    if p >= len {
                        return false;
                    }
                    let (cid, csize, chead) = match at(p, STEP) {
                        Ok(h) => h,
                        Err(cut) => return cut,
                    };
                    if LEVEL1.contains(&cid) {
                        break;
                    }
                    let Some(cs) = csize else {
                        return false;
                    };
                    let next = p.saturating_add(chead).saturating_add(cs);
                    if next > len {
                        return true;
                    }
                    p = next;
                }
                pos = p;
            }
            // Something else of unknown size: nothing more can be judged.
            None => return false,
        }
    }
    false
}

const CLUSTER_TIMESTAMP: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const BLOCK_DURATION: u32 = 0x9B;
/// How much of the file is read at a time walking the clusters: a page,
/// which holds many small sound blocks' headers, or one picture block's.
const BLOCK_WINDOW: usize = 4096;

/// One block of a track: its time and, when its `BlockGroup` says, its
/// length, in ticks of the file's `TimestampScale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockTime {
    pub time: i64,
    pub duration: Option<u64>,
}

/// Why a track's blocks can't be timed one by one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Untimed {
    /// A laced block: several frames under one time.
    Laced,
    /// The clusters can't be followed: an element runs past its parent or
    /// the file, or one inside a cluster has no size.
    Broken,
}

/// The blocks of one track, in the file's order, read from the clusters as
/// they are asked for: only the element headers and each block's first
/// bytes are read. Ends at the end of the `Segment`, or at the first block
/// that can't be timed (given as an error).
pub(crate) struct Blocks {
    file: File,
    len: u64,
    buf: Vec<u8>,
    buf_at: u64,
    pos: u64,
    end: u64,
    /// Inside a `Cluster`: where it ends (`None` for one of unknown size,
    /// which ends at the next top-level element) and its time.
    cluster: Option<(Option<u64>, i64)>,
    track: u64,
    done: bool,
}

impl Blocks {
    /// The blocks of track `number` (its `TrackNumber`) of the Matroska file
    /// `file`, read with a handle of its own. `None` when the file isn't
    /// Matroska.
    pub(crate) fn new(mut file: File, number: u64) -> Option<Blocks> {
        let len = file.metadata().ok()?.len();
        let (id, size, head) = header_at(&mut file, 0)?;
        if id != EBML {
            return None;
        }
        let segment_at = head.checked_add(size?)?;
        let (id, size, head) = header_at(&mut file, segment_at)?;
        if id != SEGMENT {
            return None;
        }
        let start = segment_at.checked_add(head)?;
        Some(Blocks {
            file,
            len,
            buf: Vec::new(),
            buf_at: 0,
            pos: start,
            end: size.map_or(len, |s| start.saturating_add(s).min(len)),
            cluster: None,
            track: number,
            done: false,
        })
    }

    /// Up to `n` bytes at `pos`, fewer only at the end of the file.
    fn bytes(&mut self, pos: u64, n: usize) -> &[u8] {
        let have = self.buf_at + self.buf.len() as u64;
        if pos < self.buf_at || pos + n as u64 > have {
            self.buf.clear();
            self.buf_at = pos;
            if self.file.seek(SeekFrom::Start(pos)).is_ok() {
                let _ = self
                    .file
                    .by_ref()
                    .take(n.max(BLOCK_WINDOW) as u64)
                    .read_to_end(&mut self.buf);
            }
        }
        let from = (pos - self.buf_at) as usize;
        &self.buf[from.min(self.buf.len())..(from + n).min(self.buf.len())]
    }

    fn header(&mut self, pos: u64) -> Option<(u32, Option<u64>, u64)> {
        header(self.bytes(pos, 12)).map(|(id, size, head)| (id, size, head as u64))
    }

    /// A `Block` or `SimpleBlock` body's track and time (relative to its
    /// cluster), or `Laced` for a laced block of this reader's track.
    fn block_start(&mut self, pos: u64) -> Option<std::result::Result<(u64, i16), Untimed>> {
        let mine = self.track;
        let b = self.bytes(pos, 12);
        let first = *b.first()?;
        let n = first.leading_zeros() as usize + 1;
        if n > 8 {
            return None;
        }
        let mut track = (first & (0xFF >> n)) as u64;
        for i in 1..n {
            track = (track << 8) | *b.get(i)? as u64;
        }
        let time = i16::from_be_bytes([*b.get(n)?, *b.get(n + 1)?]);
        let flags = *b.get(n + 2)?;
        Some(if flags & 0x06 != 0 && track == mine {
            Err(Untimed::Laced)
        } else {
            Ok((track, time))
        })
    }

    /// The next block of the track, if any, and where the walk goes next.
    fn step(&mut self) -> std::result::Result<Option<BlockTime>, Untimed> {
        let pos = self.pos;
        if let Some((Some(cend), _)) = self.cluster {
            if pos >= cend {
                self.cluster = None;
                return Ok(None);
            }
        }
        let (id, size, head) = self.header(pos).ok_or(Untimed::Broken)?;
        let Some((cend, ctime)) = self.cluster else {
            // The Segment's own level: into a cluster, over anything else.
            if id == CLUSTER {
                self.cluster = Some((size.map(|s| pos + head + s), 0));
                self.pos = pos + head;
                return Ok(None);
            }
            let size = size.ok_or(Untimed::Broken)?;
            self.pos = pos.saturating_add(head).saturating_add(size);
            return Ok(None);
        };
        if cend.is_none() && LEVEL1.contains(&id) {
            self.cluster = None;
            return Ok(None);
        }
        let size = size.ok_or(Untimed::Broken)?;
        let body = pos + head;
        let next = body.saturating_add(size);
        if next > cend.unwrap_or(self.len).min(self.len) {
            return Err(Untimed::Broken);
        }
        self.pos = next;
        match id {
            CLUSTER_TIMESTAMP if size <= 8 => {
                let t = uint(self.bytes(body, size as usize));
                self.cluster = Some((cend, t.min(i64::MAX as u64) as i64));
                Ok(None)
            }
            SIMPLE_BLOCK => match self.block_start(body).ok_or(Untimed::Broken)? {
                Ok((track, _)) if track != self.track => Ok(None),
                Ok((_, t)) => Ok(Some(BlockTime {
                    time: ctime.saturating_add(t as i64),
                    duration: None,
                })),
                Err(e) => Err(e),
            },
            BLOCK_GROUP => {
                let mut block = None;
                let mut duration = None;
                let mut p = body;
                while p < next {
                    let (cid, csize, chead) = self.header(p).ok_or(Untimed::Broken)?;
                    let csize = csize.ok_or(Untimed::Broken)?;
                    let cbody = p + chead;
                    match cid {
                        BLOCK => block = Some(self.block_start(cbody).ok_or(Untimed::Broken)?),
                        // A BlockDuration of 0 gives no length.
                        BLOCK_DURATION if csize <= 8 => {
                            duration =
                                Some(uint(self.bytes(cbody, csize as usize))).filter(|&d| d > 0)
                        }
                        _ => {}
                    }
                    p = cbody.saturating_add(csize);
                }
                match block {
                    Some(Ok((track, t))) if track == self.track => Ok(Some(BlockTime {
                        time: ctime.saturating_add(t as i64),
                        duration,
                    })),
                    Some(Err(e)) => Err(e),
                    _ => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }
}

impl Iterator for Blocks {
    type Item = std::result::Result<BlockTime, Untimed>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done && self.pos < self.end {
            match self.step() {
                Ok(Some(b)) => return Some(Ok(b)),
                Ok(None) => {}
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
        self.done = true;
        None
    }
}

/// The `Info` and `Tracks` of a Matroska or WebM file. `None` when the file
/// isn't one, is cut short in them, or has no `Tracks`.
pub(crate) fn read_headers(file: &mut File) -> Option<Headers> {
    let len = file.metadata().ok()?.len();
    let (id, size, head) = header_at(file, 0)?;
    if id != EBML {
        return None;
    }
    let segment_at = head.checked_add(size?)?;
    let (id, size, head) = header_at(file, segment_at)?;
    if id != SEGMENT {
        return None;
    }
    let start = segment_at.checked_add(head)?;
    let end = size.map_or(len, |s| start.saturating_add(s).min(len));

    let mut info = None;
    let mut tracks = None;
    let mut pos = start;
    let mut hops = 0;
    while pos < end && (info.is_none() || tracks.is_none()) && hops < MAX_HOPS {
        hops += 1;
        let Some((id, size, head)) = header_at(file, pos) else {
            break;
        };
        // An element of unknown size can't be hopped over.
        let Some(size) = size else {
            break;
        };
        let body = pos.checked_add(head)?;
        let next = body.checked_add(size)?;
        if matches!(id, INFO | TRACKS) {
            if size > MAX_HEADER || next > end {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            file.seek(SeekFrom::Start(body)).ok()?;
            file.read_exact(&mut buf).ok()?;
            if id == INFO {
                info.get_or_insert_with(|| parse_info(&buf));
            } else {
                tracks.get_or_insert_with(|| parse_tracks(&buf));
            }
        }
        pos = next;
    }
    let info = info.unwrap_or(Info {
        scale: 1_000_000,
        duration: None,
        date_utc: None,
        title: None,
    });
    Some(Headers {
        timestamp_scale: info.scale,
        duration: info.duration,
        date_utc: info.date_utc,
        title: info.title,
        tracks: tracks?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_has_its_id_and_size() {
        // Segment (4-byte ID), 1-byte size 5.
        assert_eq!(
            header(&[0x18, 0x53, 0x80, 0x67, 0x85]),
            Some((SEGMENT, Some(5), 5))
        );
        // A 1-byte ID and an 8-byte size of 300.
        assert_eq!(
            header(&[0xAE, 0x01, 0, 0, 0, 0, 0, 0x01, 0x2C]),
            Some((TRACK_ENTRY, Some(300), 9))
        );
        // All ones in the size: unknown.
        assert_eq!(
            header(&[0x18, 0x53, 0x80, 0x67, 0xFF]),
            Some((SEGMENT, None, 5))
        );
        // An ID of 5 bytes, a size cut short, and nothing at all.
        assert_eq!(header(&[0x08, 0, 0, 0, 0, 0x80]), None);
        assert_eq!(header(&[0xAE, 0x40]), None);
        assert_eq!(header(&[]), None);
        assert_eq!(header(&[0, 0, 0, 0, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn elements_stop_where_one_is_cut_short() {
        // PixelWidth 320, PixelHeight 240, then a PixelWidth that says 4 bytes and has 1.
        let b = [0xB0, 0x82, 0x01, 0x40, 0xBA, 0x81, 0xF0, 0xB0, 0x84, 0x01];
        let e: Vec<_> = elements(&b).collect();
        assert_eq!(e, vec![(PIXEL_WIDTH, &b[2..4]), (PIXEL_HEIGHT, &b[6..7])]);
        assert_eq!(uint(e[0].1), 320);
        // An element of unknown size takes the rest.
        let b = [0x1F, 0x43, 0xB6, 0x75, 0xFF, 1, 2, 3];
        assert_eq!(
            elements(&b).collect::<Vec<_>>(),
            vec![(0x1F43_B675, &b[5..])]
        );
    }

    #[test]
    fn a_video_track_reads_its_size_colour_and_frame_length() {
        let mut video = vec![
            PIXEL_WIDTH as u8,
            0x82,
            0x01,
            0x40,
            PIXEL_HEIGHT as u8,
            0x81,
            0xF0,
        ];
        // Colour { TransferCharacteristics 16, BitsPerChannel 10 }
        video.extend([0x55, 0xB0, 0x88, 0x55, 0xBA, 0x81, 16, 0x55, 0xB2, 0x81, 10]);
        let mut entry = vec![TRACK_TYPE as u8, 0x81, 1, CODEC_ID as u8, 0x85];
        entry.extend(b"V_AV1");
        entry.extend([0x23, 0xE3, 0x83, 0x84, 0x01, 0xFC, 0xA0, 0x55]);
        entry.extend([VIDEO as u8, 0x80 | video.len() as u8]);
        entry.extend(&video);
        let mut tracks = vec![TRACK_ENTRY as u8, 0x80 | entry.len() as u8];
        tracks.extend(&entry);
        let t = &parse_tracks(&tracks)[0];
        assert_eq!((t.kind, t.codec_id.as_str()), (TYPE_VIDEO, "V_AV1"));
        assert_eq!(t.default_duration, Some(33_333_333));
        let v = t.video.as_ref().unwrap();
        assert_eq!((v.width, v.height), (320, 240));
        assert_eq!((v.transfer, v.bits), (Some(16), Some(10)));
        assert!(!v.hdr10_metadata);
    }

    /// A Matroska file: the EBML header, a Segment of unknown size, then the
    /// given top-level elements, each as (ID bytes, body).
    fn mkv_file(elements: &[(&[u8], &[u8])]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&[0x1A, 0x45, 0xDF, 0xA3, 0x80]).unwrap();
        f.write_all(&[0x18, 0x53, 0x80, 0x67, 0xFF]).unwrap();
        for (id, body) in elements {
            f.write_all(id).unwrap();
            // An 8-byte size.
            f.write_all(&[0x01]).unwrap();
            f.write_all(&(body.len() as u64).to_be_bytes()[1..])
                .unwrap();
            f.write_all(body).unwrap();
        }
        f.flush().unwrap();
        f
    }

    /// An element with a one-byte size.
    fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
        assert!(body.len() < 127);
        [id, &[0x80 | body.len() as u8], body].concat()
    }

    /// A `Block` or `SimpleBlock` body: track, time, flags and two bytes of
    /// frame.
    fn block(track: u8, time: i16, flags: u8) -> Vec<u8> {
        let mut b = vec![0x80 | track];
        b.extend(time.to_be_bytes());
        b.extend([flags, 9, 9]);
        b
    }

    const CLUSTER_ID: [u8; 4] = [0x1F, 0x43, 0xB6, 0x75];

    #[test]
    fn a_tracks_block_times_are_read_cluster_by_cluster() {
        let first = [
            el(&[0xE7], &1000u16.to_be_bytes()),
            el(&[0xA3], &block(1, 0, 0x80)),
            el(&[0xA3], &block(2, 5, 0x80)),
            el(&[0xA3], &block(1, 40, 0)),
            el(
                &[0xA0],
                &[el(&[0xA1], &block(1, 20, 0)), el(&[0x9B], &[33])].concat(),
            ),
            el(
                &[0xA0],
                &[el(&[0xA1], &block(1, 30, 0)), el(&[0x9B], &[0])].concat(),
            ),
        ]
        .concat();
        let second = [
            el(&[0xE7], &2000u16.to_be_bytes()),
            el(&[0xA3], &block(1, -10, 0x80)),
            // Xiph lacing: two frames under one time.
            el(&[0xA3], &block(1, 30, 0x02)),
            el(&[0xA3], &block(1, 60, 0)),
        ]
        .concat();
        let f = mkv_file(&[(&CLUSTER_ID, &first), (&CLUSTER_ID, &second)]);
        let got: Vec<_> = Blocks::new(File::open(f.path()).unwrap(), 1)
            .unwrap()
            .collect();
        let at = |time, duration| Ok(BlockTime { time, duration });
        assert_eq!(
            got,
            vec![
                at(1000, None),
                at(1040, None),
                at(1020, Some(33)),
                // A BlockDuration of 0 is no length.
                at(1030, None),
                at(1990, None),
                Err(Untimed::Laced),
            ]
        );
        // Track 2's one block.
        let got: Vec<_> = Blocks::new(File::open(f.path()).unwrap(), 2)
            .unwrap()
            .collect();
        assert_eq!(got, vec![at(1005, None)]);
    }

    #[test]
    fn the_fixtures_picture_blocks_are_every_frame() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/video/h264.mkv");
        let h = read_headers(&mut File::open(&path).unwrap()).unwrap();
        let picture = h.tracks.iter().find(|t| t.kind == TYPE_VIDEO).unwrap();
        let mut times: Vec<i64> = Blocks::new(File::open(&path).unwrap(), picture.number)
            .unwrap()
            .map(|b| b.unwrap().time)
            .collect();
        // B-frames: not in presentation order as stored.
        assert!(times.windows(2).any(|w| w[1] < w[0]));
        times.sort_unstable();
        let want: Vec<i64> = (0..60).map(|n| (n * 1000 + 15) / 30).collect();
        assert_eq!(times, want);
        // Not a Matroska file.
        let mp4 = path.with_file_name("h264.mp4");
        assert!(Blocks::new(File::open(mp4).unwrap(), 1).is_none());
    }

    const TRACKS_ID: [u8; 4] = [0x16, 0x54, 0xAE, 0x6B];
    const INFO_ID: [u8; 4] = [0x15, 0x49, 0xA9, 0x66];

    /// A body at the 64 MB cap that is nothing but tiny elements used to cost
    /// gigabytes: a Vec of every element, and a Track for every entry.
    #[test]
    fn a_header_full_of_tiny_elements_costs_neither_time_nor_memory() {
        let junk_entries: Vec<u8> = [0xAE, 0x80].repeat(31 * 1024 * 1024);
        // One entry: TrackType video.
        let tracks_with_a_track = [TRACK_ENTRY as u8, 0x83, TRACK_TYPE as u8, 0x81, 1];
        let small_info = [0x2A, 0xD7, 0xB1, 0x83, 0x0F, 0x42, 0x40];

        let began = std::time::Instant::now();
        // Tracks of nothing but empty entries.
        let f = mkv_file(&[(&INFO_ID, &small_info), (&TRACKS_ID, &junk_entries)]);
        let h = read_headers(&mut File::open(f.path()).unwrap()).unwrap();
        assert_eq!(h.tracks.len(), MAX_TRACKS);
        // An Info of nothing but empty Void elements.
        let voids: Vec<u8> = [0xEC, 0x80].repeat(31 * 1024 * 1024);
        let f = mkv_file(&[(&INFO_ID, &voids), (&TRACKS_ID, &tracks_with_a_track[..])]);
        let h = read_headers(&mut File::open(f.path()).unwrap()).unwrap();
        assert_eq!(h.timestamp_scale, 1_000_000);
        assert_eq!(h.tracks.len(), 1);
        // One entry of nothing but empty elements, in a Tracks of its own.
        let mut one = vec![TRACK_ENTRY as u8, 0x01];
        one.extend(&(voids.len() as u64).to_be_bytes()[1..]);
        one.extend(&voids);
        let f = mkv_file(&[(&TRACKS_ID, &one)]);
        let h = read_headers(&mut File::open(f.path()).unwrap()).unwrap();
        assert_eq!(h.tracks.len(), 1);
        assert!(
            began.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            began.elapsed()
        );
    }

    #[test]
    fn a_cut_short_file_is_told_from_its_sizes() {
        let block = |n: usize| {
            let mut b = vec![0xA3, 0x80 | (n as u8 + 4), 0x81, 0, 0, 0x80];
            b.extend(std::iter::repeat_n(7u8, n));
            b
        };
        // EBML header, then a Segment of unknown size holding `clusters`.
        let file = |clusters: &[Vec<u8>], cut: usize| {
            let mut f = vec![0x1A, 0x45, 0xDF, 0xA3, 0x80];
            f.extend([0x18, 0x53, 0x80, 0x67, 0xFF]);
            for c in clusters {
                f.extend(c);
            }
            f.truncate(f.len() - cut);
            let mut t = tempfile::NamedTempFile::new().unwrap();
            std::io::Write::write_all(&mut t, &f).unwrap();
            t
        };
        let sized = |blocks: &[Vec<u8>]| {
            let body: Vec<u8> = blocks.concat();
            let mut c = vec![0x1F, 0x43, 0xB6, 0x75, 0x80 | body.len() as u8];
            c.extend(body);
            c
        };
        let unsized_ = |blocks: &[Vec<u8>]| {
            let mut c = vec![0x1F, 0x43, 0xB6, 0x75, 0xFF];
            c.extend(blocks.concat());
            c
        };
        let short = |clusters: &[Vec<u8>], cut: usize| {
            cut_short(&mut File::open(file(clusters, cut).path()).unwrap())
        };
        let a = sized(&[block(10), block(20)]);
        let b = unsized_(&[block(10), block(30)]);
        // Whole: fine, sized and unsized, one after the other.
        assert!(!short(&[a.clone(), b.clone()], 0));
        assert!(!short(&[b.clone(), a.clone()], 0));
        // A sized Cluster cut: short, wherever the cut falls.
        for cut in 1..a.len() {
            assert!(short(&[b.clone(), a.clone()], cut), "cut {cut}");
        }
        // An unsized Cluster (57 bytes: a 5-byte header, blocks of 16 and 36)
        // cut inside a block or a block's header: short. Cut exactly after
        // a block, or after the Cluster's header: can't be told.
        for cut in (1..36).chain(37..52) {
            assert!(short(&[a.clone(), b.clone()], cut), "cut {cut}");
        }
        assert!(!short(&[a.clone(), b.clone()], 36));
        assert!(!short(&[a.clone(), b.clone()], 52));
        // A sized Segment that runs past the file: short.
        // The EBML header, then a Segment that says it holds 100 bytes.
        let f = vec![
            0x1A,
            0x45,
            0xDF,
            0xA3,
            0x80,
            0x18,
            0x53,
            0x80,
            0x67,
            0x80 | 100,
        ];
        let mut t = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut t, &[f, a].concat()).unwrap();
        assert!(cut_short(&mut File::open(t.path()).unwrap()));
    }

    #[test]
    fn colour_reads_hlg_and_the_hdr10_metadata() {
        // Video { Colour { TransferCharacteristics 18, MaxCLL 1000 } }
        let colour = [
            0x55, 0xB0, 0x89, 0x55, 0xBA, 0x81, 18, 0x55, 0xBC, 0x82, 0x03, 0xE8,
        ];
        let mut video = vec![PIXEL_WIDTH as u8, 0x81, 64, PIXEL_HEIGHT as u8, 0x81, 48];
        video.extend(colour);
        let v = parse_video(&video);
        assert_eq!((v.width, v.height), (64, 48));
        assert_eq!(v.transfer, Some(18));
        assert!(v.hdr10_metadata);
        assert_eq!(
            (v.max_cll, v.max_fall, v.mastering),
            (Some(1000), None, None)
        );
    }

    #[test]
    fn mastering_metadata_and_codec_delay_are_read() {
        let f = |id: u32, v: f64| {
            let mut b = id.to_be_bytes()[2..].to_vec();
            b.push(0x88);
            b.extend(v.to_be_bytes());
            b
        };
        let values = [
            0.708, 0.292, 0.17, 0.797, 0.131, 0.046, 0.3127, 0.329, 1000.0, 0.0001,
        ];
        let mut md = Vec::new();
        for (id, v) in MASTERING_FIELDS.iter().zip(values) {
            md.extend(f(*id, v));
        }
        let mut colour = vec![0x55, 0xD0, 0x80 | md.len() as u8];
        colour.extend(&md);
        colour.extend([0x55, 0xBD, 0x82, 0x01, 0x90]); // MaxFALL 400
        let mut video = vec![0x55, 0xB0, 0x80 | colour.len() as u8];
        video.extend(&colour);
        let v = parse_video(&video);
        assert_eq!(v.mastering, Some(values));
        assert_eq!(v.max_fall, Some(400));
        // One of the ten missing: none.
        let short = &md[..md.len() - 11];
        assert_eq!(parse_mastering(short), None);

        // TrackEntry { TrackType 2, CodecDelay 6,500,000 }
        let entry = [
            TRACK_TYPE as u8,
            0x81,
            2,
            0x56,
            0xAA,
            0x83,
            0x63,
            0x2E,
            0xA0,
        ];
        let mut tracks = vec![TRACK_ENTRY as u8, 0x80 | entry.len() as u8];
        tracks.extend(entry);
        assert_eq!(parse_tracks(&tracks)[0].codec_delay, Some(6_500_000));
    }
}
