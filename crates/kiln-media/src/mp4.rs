//! AAC out: Kiln's own M4A (MP4) writer, and bare AAC with ADTS headers.
//!
//! The M4A keeps its `moov` before the `mdat`, so it plays while it is still
//! downloading and needs no second pass by other tools. While encoding, the
//! AAC frames go to a temporary file next to the output; at the end the sink
//! writes `ftyp` and `moov` (whose chunk offsets now are known) to the output,
//! then copies the frames after them in an `mdat`, and removes the temporary
//! file. Offsets are 64-bit (`co64`) once the file passes 4 GiB.
//!
//! An `edts/elst` edit list presents only the real audio, after any encoder
//! delay and before the padding that fills out the last AAC frame, and an
//! `iTunSMPB` tag says the same for readers that ignore edit lists.
//!
//! The same encoder gives the sound of an MP4, MOV or M4V video as packets
//! ([`AacPackets`]), for the video writer.
//!
//! This module also reads AudioSpecificConfigs, to tell HE-AAC from AAC-LC.

use crate::error::{MediaError, Result};
use crate::mf::aac::AacEncoder;
use crate::pipeline::{no_audio, Sink, SinkSpec};
use crate::tags::mp4::{full_box, mp4_box, smpb, udta};
use crate::types::{Block, Tags};
use crate::video::packet::{AudioParams, Packet, StreamCodec, StreamSpec};
use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;

/// Samples of audio per AAC frame.
const FRAME: u64 = 1024;
/// AAC frames per MP4 chunk: about half a second.
const CHUNK: usize = 22;

// ---------------------------------------------------------------------------
// AudioSpecificConfig

/// What an AudioSpecificConfig says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asc {
    /// The core audio object type: 2 for AAC-LC, also under HE-AAC.
    pub object_type: u8,
    /// The sampling frequency index (15 when the rate is given outright).
    pub freq_index: u8,
    /// The core sample rate.
    pub rate: u32,
    /// The channel configuration (0 when a program config element says).
    pub channel_config: u8,
    /// Spectral band replication: HE-AAC.
    pub sbr: bool,
    /// Parametric stereo: HE-AAC v2.
    pub ps: bool,
}

impl Asc {
    /// True for HE-AAC (v1 or v2), signalled either way.
    pub fn is_he(&self) -> bool {
        self.sbr || self.ps
    }
}

const RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn left(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.at)
    }
    fn read(&mut self, n: usize) -> Option<u32> {
        if n > self.left() {
            return None;
        }
        let mut v = 0u32;
        for _ in 0..n {
            let bit = (self.data[self.at / 8] >> (7 - self.at % 8)) & 1;
            v = (v << 1) | bit as u32;
            self.at += 1;
        }
        Some(v)
    }
    fn object_type(&mut self) -> Option<u8> {
        let t = self.read(5)? as u8;
        if t == 31 {
            Some(32 + self.read(6)? as u8)
        } else {
            Some(t)
        }
    }
    fn rate(&mut self) -> Option<(u8, u32)> {
        let i = self.read(4)? as u8;
        let rate = if i == 15 {
            self.read(24)?
        } else {
            *RATES.get(i as usize).unwrap_or(&0)
        };
        Some((i, rate))
    }
}

/// Reads an AudioSpecificConfig (ISO 14496-3, 1.6.2.1). HE-AAC shows either
/// as object type 5 or 29 up front (explicit, hierarchical signalling), or as
/// AAC-LC followed by a 0x2B7 sync extension that turns SBR on (explicit,
/// backward-compatible signalling), with PS after a 0x548 sync extension.
pub fn parse_asc(asc: &[u8]) -> Option<Asc> {
    let mut b = Bits { data: asc, at: 0 };
    let mut object_type = b.object_type()?;
    let (freq_index, rate) = b.rate()?;
    let channel_config = b.read(4)? as u8;
    let mut sbr = false;
    let mut ps = false;
    if object_type == 5 || object_type == 29 {
        sbr = true;
        ps = object_type == 29;
        // The SBR output rate, then the core object type. A config cut short
        // here still says HE-AAC.
        object_type = b.rate().and_then(|_| b.object_type()).unwrap_or(2);
    }
    let done = |object_type, freq_index, rate, sbr, ps| Asc {
        object_type,
        freq_index,
        rate,
        channel_config,
        sbr,
        ps,
    };
    // GASpecificConfig, for the AAC object types.
    if !matches!(object_type, 1..=4 | 6 | 7 | 17 | 19..=23) {
        return Some(done(object_type, freq_index, rate, sbr, ps));
    }
    let _frame_length = b.read(1);
    if b.read(1) == Some(1) {
        b.read(14); // core coder delay
    }
    let extension = b.read(1) == Some(1);
    if channel_config == 0 {
        // A program config element follows; nothing after it is read.
        return Some(done(object_type, freq_index, rate, sbr, ps));
    }
    if matches!(object_type, 6 | 20) {
        b.read(3);
    }
    if extension {
        if object_type == 22 {
            b.read(16);
        }
        if matches!(object_type, 17 | 19 | 20 | 23) {
            b.read(3);
        }
        b.read(1);
    }
    if matches!(object_type, 17 | 19..=27 | 39) {
        b.read(2); // epConfig
    }
    if !sbr && b.left() >= 16 && b.read(11) == Some(0x2B7) && b.object_type() == Some(5) {
        sbr = b.read(1) == Some(1);
        if sbr {
            b.rate();
            if b.left() >= 12 && b.read(11) == Some(0x548) {
                ps = b.read(1) == Some(1);
            }
        }
    }
    Some(done(object_type, freq_index, rate, sbr, ps))
}

/// True when an AudioSpecificConfig says HE-AAC (v1 or v2).
pub fn is_he_aac(asc: &[u8]) -> bool {
    parse_asc(asc).is_some_and(|a| a.is_he())
}

// ---------------------------------------------------------------------------
// The MP4 boxes

/// The sound track, as the `moov` describes it.
struct Track<'a> {
    rate: u32,
    channels: u16,
    asc: &'a [u8],
    /// Each AAC frame's size, in order.
    sizes: &'a [u32],
    /// Frames of real audio: what the edit list presents.
    frames: u64,
    priming: u32,
}

/// 0x00010000 as a 16.16 fixed-point number, and the identity matrix.
const MATRIX: [u32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];

pub(crate) fn be32(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_be_bytes());
}

pub(crate) fn be64(v: &mut Vec<u8>, n: u64) {
    v.extend_from_slice(&n.to_be_bytes());
}

/// A time field: 32 bits in a version 0 box, 64 in version 1.
pub(crate) fn time(v: &mut Vec<u8>, long: bool, n: u64) {
    if long {
        be64(v, n)
    } else {
        be32(v, n as u32)
    }
}

/// An MPEG-4 descriptor: its tag, a length (one byte, or more with the high
/// bit set), and its body.
pub(crate) fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let mut len = body.len();
    let mut bytes = vec![(len & 0x7F) as u8];
    len >>= 7;
    while len > 0 {
        bytes.push(0x80 | (len & 0x7F) as u8);
        len >>= 7;
    }
    bytes.reverse();
    out.extend(bytes);
    out.extend_from_slice(body);
    out
}

fn esds(t: &Track) -> Vec<u8> {
    let max = t.sizes.iter().copied().max().unwrap_or(0);
    let total: u64 = t.sizes.iter().map(|&s| s as u64).sum();
    let secs = t.sizes.len() as f64 * FRAME as f64 / t.rate as f64;
    let avg = if secs > 0.0 {
        (total as f64 * 8.0 / secs) as u32
    } else {
        0
    };
    let peak = (max as u64 * 8 * t.rate as u64 / FRAME) as u32;
    esds_box(t.asc, max, peak.max(avg), avg)
}

/// An `esds` box for an AAC stream: its AudioSpecificConfig, the biggest
/// frame's size, and the peak and average bit rates.
pub(crate) fn esds_box(asc: &[u8], max_frame: u32, peak_bps: u32, avg_bps: u32) -> Vec<u8> {
    let mut config = vec![0x40, 0x15]; // MPEG-4 audio; an audio stream
    config.extend_from_slice(&max_frame.min(0xFF_FFFF).to_be_bytes()[1..]);
    be32(&mut config, peak_bps);
    be32(&mut config, avg_bps);
    config.extend(descriptor(0x05, asc));
    let mut es = vec![0, 0, 0]; // ES_ID 0, no flags
    es.extend(descriptor(0x04, &config));
    es.extend(descriptor(0x06, &[0x02]));
    full_box(b"esds", 0, 0, &descriptor(0x03, &es))
}

fn stbl(t: &Track, base: u64, long_offsets: bool) -> Vec<u8> {
    // Sample description: mp4a with its esds.
    let mut mp4a = vec![0u8; 6];
    mp4a.extend_from_slice(&1u16.to_be_bytes()); // data reference 1
    mp4a.extend_from_slice(&[0u8; 8]);
    mp4a.extend_from_slice(&t.channels.to_be_bytes());
    mp4a.extend_from_slice(&16u16.to_be_bytes());
    mp4a.extend_from_slice(&[0u8; 4]);
    be32(&mut mp4a, t.rate.min(0xFFFF) << 16);
    mp4a.extend(esds(t));
    let mut stsd = Vec::new();
    be32(&mut stsd, 1);
    stsd.extend(mp4_box(b"mp4a", &mp4a));

    let count = t.sizes.len() as u32;
    let mut stts = Vec::new();
    if count > 0 {
        be32(&mut stts, 1);
        be32(&mut stts, count);
        be32(&mut stts, FRAME as u32);
    } else {
        be32(&mut stts, 0);
    }

    let chunks = t.sizes.len().div_ceil(CHUNK);
    let last = t.sizes.len() - chunks.saturating_sub(1) * CHUNK;
    let mut stsc = Vec::new();
    match chunks {
        0 => be32(&mut stsc, 0),
        _ if last == CHUNK || chunks == 1 => {
            be32(&mut stsc, 1);
            for n in [1, if chunks == 1 { last } else { CHUNK } as u32, 1] {
                be32(&mut stsc, n);
            }
        }
        _ => {
            be32(&mut stsc, 2);
            for n in [1, CHUNK as u32, 1, chunks as u32, last as u32, 1] {
                be32(&mut stsc, n);
            }
        }
    }

    let mut stsz = vec![0u8; 4]; // every frame has its own size
    be32(&mut stsz, count);
    for &s in t.sizes {
        be32(&mut stsz, s);
    }

    let mut offsets = Vec::new();
    be32(&mut offsets, chunks as u32);
    let mut at = base;
    for chunk in t.sizes.chunks(CHUNK) {
        if long_offsets {
            be64(&mut offsets, at);
        } else {
            be32(&mut offsets, at as u32);
        }
        at += chunk.iter().map(|&s| s as u64).sum::<u64>();
    }

    let mut body = full_box(b"stsd", 0, 0, &stsd);
    body.extend(full_box(b"stts", 0, 0, &stts));
    body.extend(full_box(b"stsc", 0, 0, &stsc));
    body.extend(full_box(b"stsz", 0, 0, &stsz));
    body.extend(if long_offsets {
        full_box(b"co64", 0, 0, &offsets)
    } else {
        full_box(b"stco", 0, 0, &offsets)
    });
    mp4_box(b"stbl", &body)
}

/// The `moov` box, with chunk offsets counted from `base`, the file offset of
/// the first AAC frame.
fn moov(t: &Track, tags: &Tags, base: u64, long_offsets: bool) -> Vec<u8> {
    let media = t.sizes.len() as u64 * FRAME;
    // The presentation: the real audio, after the priming frames.
    let shown = t.frames.min(media.saturating_sub(t.priming as u64));
    let long = media > u32::MAX as u64;
    let v = long as u8;

    let mut mvhd = Vec::new();
    time(&mut mvhd, long, 0);
    time(&mut mvhd, long, 0);
    be32(&mut mvhd, t.rate);
    time(&mut mvhd, long, shown);
    be32(&mut mvhd, 0x10000); // rate 1.0
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    mvhd.extend_from_slice(&[0u8; 10]);
    for m in MATRIX {
        be32(&mut mvhd, m);
    }
    mvhd.extend_from_slice(&[0u8; 24]);
    be32(&mut mvhd, 2); // next track ID

    let mut tkhd = Vec::new();
    time(&mut tkhd, long, 0);
    time(&mut tkhd, long, 0);
    be32(&mut tkhd, 1); // track ID
    be32(&mut tkhd, 0);
    time(&mut tkhd, long, shown);
    tkhd.extend_from_slice(&[0u8; 8]);
    tkhd.extend_from_slice(&[0, 0, 0, 0]); // layer, alternate group
    tkhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    tkhd.extend_from_slice(&[0, 0]);
    for m in MATRIX {
        be32(&mut tkhd, m);
    }
    tkhd.extend_from_slice(&[0u8; 8]); // no width or height

    let mut elst = Vec::new();
    be32(&mut elst, 1);
    time(&mut elst, long, shown);
    time(&mut elst, long, t.priming as u64);
    be32(&mut elst, 0x10000); // rate 1.0
    let edts = mp4_box(b"edts", &full_box(b"elst", v, 0, &elst));

    let mut mdhd = Vec::new();
    time(&mut mdhd, long, 0);
    time(&mut mdhd, long, 0);
    be32(&mut mdhd, t.rate);
    time(&mut mdhd, long, media);
    mdhd.extend_from_slice(&0x55C4u16.to_be_bytes()); // "und"
    mdhd.extend_from_slice(&[0, 0]);

    let mut hdlr = vec![0u8; 4];
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0u8; 12]);
    hdlr.extend_from_slice(b"SoundHandler\0");

    let mut dref = Vec::new();
    be32(&mut dref, 1);
    dref.extend(full_box(b"url ", 0, 1, &[])); // in this file
    let mut minf = full_box(b"smhd", 0, 0, &[0u8; 4]);
    minf.extend(mp4_box(b"dinf", &full_box(b"dref", 0, 0, &dref)));
    minf.extend(stbl(t, base, long_offsets));

    let mut mdia = full_box(b"mdhd", v, 0, &mdhd);
    mdia.extend(full_box(b"hdlr", 0, 0, &hdlr));
    mdia.extend(mp4_box(b"minf", &minf));

    let mut trak = full_box(b"tkhd", v, 7, &tkhd); // enabled, in movie and preview
    trak.extend(edts);
    trak.extend(mp4_box(b"mdia", &mdia));

    let padding = media.saturating_sub(t.priming as u64 + shown);
    let mut body = full_box(b"mvhd", v, 0, &mvhd);
    body.extend(mp4_box(b"trak", &trak));
    body.extend(udta(
        tags,
        Some(&smpb(t.priming, padding.min(u32::MAX as u64) as u32, shown)),
    ));
    mp4_box(b"moov", &body)
}

fn ftyp() -> Vec<u8> {
    let mut b = b"M4A ".to_vec();
    be32(&mut b, 0);
    for brand in [b"M4A ", b"mp42", b"isom"] {
        b.extend_from_slice(brand);
    }
    mp4_box(b"ftyp", &b)
}

/// Everything before the AAC frames: `ftyp`, `moov`, and the `mdat` header
/// for `payload` bytes of frames.
fn header(t: &Track, tags: &Tags, payload: u64) -> Vec<u8> {
    let ftyp = ftyp();
    let mdat_head = if payload + 8 > u32::MAX as u64 { 16 } else { 8 };
    // The moov's size doesn't depend on the offsets' values, only on their width.
    let size = |long| (ftyp.len() + moov(t, tags, 0, long).len() + mdat_head) as u64;
    let long = size(false) + payload > u32::MAX as u64;
    let base = size(long);
    let mut out = ftyp;
    out.extend(moov(t, tags, base, long));
    if mdat_head == 16 {
        be32(&mut out, 1);
        out.extend_from_slice(b"mdat");
        be64(&mut out, payload + 16);
    } else {
        be32(&mut out, (payload + 8) as u32);
        out.extend_from_slice(b"mdat");
    }
    debug_assert_eq!(out.len() as u64, base);
    out
}

// ---------------------------------------------------------------------------
// The sinks

fn check(spec: &SinkSpec, enc: &AacEncoder) -> Result<()> {
    if spec.rate != enc.rate() || spec.channels.count != enc.channels() {
        return Err(MediaError::Encoder(format!(
            "the AAC encoder was made for {} channels at {} Hz, not {} at {} Hz",
            enc.channels(),
            enc.rate(),
            spec.channels.count,
            spec.rate
        )));
    }
    Ok(())
}

fn check_block(b: &Block, rate: u32, channels: u16) -> Result<()> {
    if b.rate != rate || b.channels != channels {
        return Err(MediaError::Encoder(format!(
            "a block of {} channels at {} Hz came to a {channels}-channel {rate} Hz encoder",
            b.channels, b.rate
        )));
    }
    Ok(())
}

/// Makes a sink that writes an M4A: `enc`'s AAC frames, with `spec`'s tags.
pub fn m4a_sink(spec: SinkSpec, enc: AacEncoder) -> Result<Box<dyn Sink>> {
    check(&spec, &enc)?;
    let temp = temp_beside(&spec.out);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&temp)?;
    Ok(Box::new(M4aSink {
        enc,
        out: spec.out,
        frames: BufWriter::with_capacity(256 * 1024, file),
        temp: Some(temp),
        tags: spec.tags.clone(),
        sizes: Vec::new(),
        payload: 0,
        frames_in: 0,
        packets: Vec::new(),
    }))
}

struct M4aSink {
    enc: AacEncoder,
    out: File,
    /// The AAC frames so far, in the temporary file.
    frames: BufWriter<File>,
    /// The temporary file's path, until it is removed.
    temp: Option<PathBuf>,
    tags: Tags,
    sizes: Vec<u32>,
    payload: u64,
    frames_in: u64,
    packets: Vec<Vec<u8>>,
}

impl M4aSink {
    fn store(&mut self) -> Result<()> {
        for p in self.packets.drain(..) {
            self.frames.write_all(&p)?;
            self.sizes.push(p.len() as u32);
            self.payload += p.len() as u64;
        }
        Ok(())
    }
}

impl Sink for M4aSink {
    fn write(&mut self, b: &Block) -> Result<()> {
        check_block(b, self.enc.rate(), self.enc.channels())?;
        self.enc.encode(&b.samples, &mut self.packets)?;
        self.frames_in += b.frames() as u64;
        self.store()
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        if self.frames_in == 0 {
            return Err(no_audio("M4A"));
        }
        self.enc.finish(&mut self.packets)?;
        self.store()?;
        let track = Track {
            rate: self.enc.rate(),
            channels: self.enc.channels(),
            asc: self.enc.asc(),
            sizes: &self.sizes,
            frames: self.frames_in,
            priming: self.enc.priming(),
        };
        let head = header(&track, &self.tags, self.payload);
        let mut out = BufWriter::with_capacity(256 * 1024, &self.out);
        out.write_all(&head)?;
        self.frames.flush()?;
        let mut frames = self.frames.get_ref();
        frames.seek(SeekFrom::Start(0))?;
        let copied = io::copy(&mut frames, &mut out)?;
        if copied != self.payload {
            return Err(MediaError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the temporary AAC file came back short",
            )));
        }
        out.flush()?;
        drop(out);
        self.out.sync_all().ok();
        Ok(())
    }
}

impl Drop for M4aSink {
    fn drop(&mut self) {
        if let Some(temp) = self.temp.take() {
            // The sink's own temporary file, finished or cancelled.
            let _ = std::fs::remove_file(temp);
        }
    }
}

/// A path for the temporary frames file, next to the output when its path can
/// be found, else in the system's temporary folder.
pub(crate) fn temp_beside(out: &File) -> PathBuf {
    match path_of(out) {
        Some(p) => temp_next_to(&p),
        None => std::env::temp_dir().join(temp_name("m4a")),
    }
}

/// A path for a temporary samples file in the same folder as `path`, so it
/// is beside the .part, in the folder the job writes in (R92, R96); in the
/// app that is kiln-core's working folder.
pub(crate) fn temp_next_to(path: &std::path::Path) -> PathBuf {
    let stem = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(temp_name(&stem))
}

/// A hidden name, unique to this process and call, for a temporary samples
/// file of the file called `stem`.
fn temp_name(stem: &str) -> String {
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(".{stem}.{}-{n}.kiln-mdat", std::process::id())
}

/// The path of an open file.
#[cfg(windows)]
fn path_of(f: &File) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: the handle is the open file's; the buffer's length is passed.
        let n = unsafe {
            GetFinalPathNameByHandleW(HANDLE(f.as_raw_handle()), &mut buf, FILE_NAME_NORMALIZED)
        } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            let p = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
            return Some(p).filter(|p| p.parent().is_some_and(std::path::Path::exists));
        }
        buf.resize(n + 1, 0);
    }
}

#[cfg(not(windows))]
fn path_of(_: &File) -> Option<PathBuf> {
    None
}

/// Makes a sink that writes bare AAC: each frame behind a 7-byte ADTS header.
/// ADTS carries no tags.
pub fn adts_sink(spec: SinkSpec, enc: AacEncoder) -> Result<Box<dyn Sink>> {
    check(&spec, &enc)?;
    let asc = parse_asc(enc.asc())
        .filter(|a| (1..=4).contains(&a.object_type) && a.freq_index < 13 && !a.is_he())
        .ok_or_else(|| MediaError::Encoder("ADTS can't carry this AAC".into()))?;
    Ok(Box::new(AdtsSink {
        enc,
        out: BufWriter::with_capacity(256 * 1024, spec.out),
        asc,
        packets: Vec::new(),
        frames_in: 0,
    }))
}

struct AdtsSink {
    enc: AacEncoder,
    out: BufWriter<File>,
    asc: Asc,
    packets: Vec<Vec<u8>>,
    frames_in: u64,
}

/// The 7-byte ADTS header (no CRC) for a frame of `len` bytes.
fn adts_header(asc: &Asc, len: usize) -> [u8; 7] {
    let total = len + 7;
    let profile = asc.object_type - 1;
    let ch = asc.channel_config;
    [
        0xFF,
        0xF1, // MPEG-4, layer 0, no CRC
        (profile << 6) | (asc.freq_index << 2) | (ch >> 2),
        ((ch & 3) << 6) | ((total >> 11) as u8 & 0x03),
        (total >> 3) as u8,
        (((total & 7) as u8) << 5) | 0x1F, // buffer fullness 0x7FF: variable
        0xFC,
    ]
}

impl AdtsSink {
    fn store(&mut self) -> Result<()> {
        for p in self.packets.drain(..) {
            if p.len() + 7 > 0x1FFF {
                return Err(MediaError::Encoder(
                    "an AAC frame is too big for ADTS".into(),
                ));
            }
            self.out.write_all(&adts_header(&self.asc, p.len()))?;
            self.out.write_all(&p)?;
        }
        Ok(())
    }
}

impl Sink for AdtsSink {
    fn write(&mut self, b: &Block) -> Result<()> {
        check_block(b, self.enc.rate(), self.enc.channels())?;
        self.enc.encode(&b.samples, &mut self.packets)?;
        self.frames_in += b.frames() as u64;
        self.store()
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        if self.frames_in == 0 {
            return Err(no_audio("AAC"));
        }
        self.enc.finish(&mut self.packets)?;
        self.store()?;
        self.out.flush()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Packets for a video's sound

/// AAC packets for the sound of an MP4, MOV or M4V video, from part 1's
/// blocks: the same encoder as [`m4a_sink`], its frames given as [`Packet`]s
/// for a muxer instead of written to an M4A.
///
/// Times count samples at the encoder's rate. A packet's `pts` is when its
/// first sample plays, the encoder's delay taken off (so the first packet
/// starts at minus the priming, which the MP4 writer's edit list skips). The
/// last packet's length ends where the input does.
pub struct AacPackets {
    enc: AacEncoder,
    stream: usize,
    frames_in: u64,
    /// The encoder's frames not yet given out.
    raw: Vec<Vec<u8>>,
    /// The newest packet and its number, held back until the next one shows
    /// it isn't the last.
    held: Option<(u64, Vec<u8>)>,
}

impl AacPackets {
    /// Packets from `enc`, with stream number `stream`.
    pub fn new(enc: AacEncoder, stream: usize) -> AacPackets {
        AacPackets {
            enc,
            stream,
            frames_in: 0,
            raw: Vec::new(),
            held: None,
        }
    }

    /// The stream the packets belong to, for a muxer.
    pub fn stream(&self) -> StreamSpec {
        StreamSpec {
            codec: StreamCodec::Aac {
                asc: self.enc.asc().to_vec(),
            },
            timescale: self.enc.rate(),
            video: None,
            audio: Some(AudioParams {
                rate: self.enc.rate(),
                channels: self.enc.channels(),
            }),
        }
    }

    /// Encodes a block (at the encoder's rate and channels), adding the
    /// packets ready to `out`.
    pub fn push(&mut self, b: &Block, out: &mut Vec<Packet>) -> Result<()> {
        check_block(b, self.enc.rate(), self.enc.channels())?;
        self.enc.encode(&b.samples, &mut self.raw)?;
        self.frames_in += b.frames() as u64;
        self.give(out);
        Ok(())
    }

    /// Encodes what the encoder still holds and adds every packet left to
    /// `out`. No sound at all is an error, as for an M4A.
    pub fn finish(&mut self, out: &mut Vec<Packet>) -> Result<()> {
        if self.frames_in == 0 {
            return Err(no_audio("AAC"));
        }
        self.enc.finish(&mut self.raw)?;
        self.give(out);
        if let Some((n, data)) = self.held.take() {
            let end = self.frames_in + self.enc.priming() as u64;
            let length = end.saturating_sub(n * FRAME).clamp(1, FRAME);
            out.push(self.packet(n, data, length));
        }
        Ok(())
    }

    /// Gives out every frame but the newest, which is held.
    fn give(&mut self, out: &mut Vec<Packet>) {
        let raw = std::mem::take(&mut self.raw);
        for data in raw {
            let n = self.held.as_ref().map_or(0, |(n, _)| n + 1);
            if let Some((prev, bytes)) = self.held.replace((n, data)) {
                out.push(self.packet(prev, bytes, FRAME));
            }
        }
    }

    /// Frame number `n`, `length` samples long.
    fn packet(&self, n: u64, data: Vec<u8>, length: u64) -> Packet {
        let pts = (n * FRAME) as i64 - self.enc.priming() as i64;
        Packet {
            stream: self.stream,
            pts,
            dts: pts,
            duration: length as i64,
            key: true,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `bits` (as "0"/"1" text, spaces ignored) into bytes.
    fn bits(s: &str) -> Vec<u8> {
        let v: Vec<u8> = s.bytes().filter(|c| *c != b' ').map(|c| c - b'0').collect();
        v.chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |a, (i, b)| a | (b << (7 - i)))
            })
            .collect()
    }

    #[test]
    fn plain_lc_is_not_he() {
        // AAC-LC, 44.1 kHz, stereo: 00010 0100 0010 000
        let lc = [0x12, 0x10];
        let a = parse_asc(&lc).unwrap();
        assert_eq!((a.object_type, a.rate, a.channel_config), (2, 44100, 2));
        assert!(!a.is_he());
        assert!(!is_he_aac(&lc));
    }

    #[test]
    fn object_type_5_and_29_are_he() {
        // SBR: type 5, 24 kHz core, stereo, then 48 kHz output, then type 2.
        let sbr = bits("00101 0110 0010 0011 00010 000");
        assert!(is_he_aac(&sbr));
        let a = parse_asc(&sbr).unwrap();
        assert_eq!((a.object_type, a.sbr, a.ps), (2, true, false));
        // PS: type 29, 24 kHz core, mono, 48 kHz, type 2.
        let ps = bits("11101 0110 0001 0011 00010 000");
        let a = parse_asc(&ps).unwrap();
        assert!(a.sbr && a.ps);
    }

    #[test]
    fn backward_compatible_sbr_and_ps_are_he() {
        // LC at 24 kHz stereo, GASpecificConfig 000, then sync 0x2B7, type 5,
        // SBR present, 48 kHz.
        let sbr = bits("00010 0110 0010 000 01010110111 00101 1 0011 0000");
        let a = parse_asc(&sbr).unwrap();
        assert!(a.sbr && !a.ps, "{a:?}");
        // The same, then sync 0x548 and PS present.
        let ps = bits("00010 0110 0001 000 01010110111 00101 1 0011 10101001000 1 000");
        let a = parse_asc(&ps).unwrap();
        assert!(a.sbr && a.ps, "{a:?}");
        // The sync extension with SBR switched off is plain LC.
        let off = bits("00010 0110 0010 000 01010110111 00101 0 00000000");
        assert!(!is_he_aac(&off));
    }

    #[test]
    fn short_or_empty_configs_are_not_he() {
        assert!(!is_he_aac(&[]));
        assert!(!is_he_aac(&[0x12]));
        assert_eq!(parse_asc(&[]), None);
    }

    #[test]
    fn escaped_object_types_are_read() {
        // 31 escapes to 32 + 6 bits: 32 + 10 = 42 (USAC).
        let usac = bits("11111 001010 0011 0010 0000000");
        assert_eq!(parse_asc(&usac).unwrap().object_type, 42);
    }

    #[test]
    fn adts_headers_carry_the_frame_length() {
        let asc = parse_asc(&[0x12, 0x10]).unwrap();
        let h = adts_header(&asc, 371);
        assert_eq!(&h[..2], &[0xFF, 0xF1]);
        // profile 1 (LC), frequency index 4 (44.1 kHz), channels 2
        assert_eq!(h[2], (1 << 6) | (4 << 2));
        assert_eq!(h[3] >> 6, 2);
        let len = ((h[3] as usize & 3) << 11) | ((h[4] as usize) << 3) | (h[5] as usize >> 5);
        assert_eq!(len, 378);
    }

    fn track(sizes: &[u32]) -> Track<'_> {
        Track {
            rate: 44100,
            channels: 2,
            asc: &[0x12, 0x10],
            sizes,
            frames: sizes.len() as u64 * FRAME - 2000,
            priming: 1024,
        }
    }

    /// The child boxes in `b` (a run of boxes).
    fn boxes(mut b: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        while b.len() >= 8 {
            let size = u32::from_be_bytes(b[..4].try_into().unwrap()) as usize;
            out.push((
                String::from_utf8_lossy(&b[4..8]).into_owned(),
                b[8..size].to_vec(),
            ));
            b = &b[size..];
        }
        out
    }

    fn find_box(b: &[u8], name: &[u8]) -> Option<usize> {
        b.windows(name.len()).position(|w| w == name)
    }

    #[test]
    fn chunk_offsets_point_past_the_header() {
        let sizes = vec![300u32; 50];
        let head = header(&track(&sizes), &Tags::default(), 15_000);
        let names: Vec<String> = boxes(&head[..head.len() - 8])
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, ["ftyp", "moov"]);
        assert_eq!(&head[head.len() - 4..], b"mdat");
        let stco = find_box(&head, b"stco").unwrap();
        let n = u32::from_be_bytes(head[stco + 8..stco + 12].try_into().unwrap());
        assert_eq!(n, 3); // 22 + 22 + 6 frames
        let first = u32::from_be_bytes(head[stco + 12..stco + 16].try_into().unwrap());
        let second = u32::from_be_bytes(head[stco + 16..stco + 20].try_into().unwrap());
        assert_eq!(first as usize, head.len());
        assert_eq!(second - first, 22 * 300);
        // Two stsc runs: full chunks, then the short last one.
        let stsc = find_box(&head, b"stsc").unwrap();
        let entries = u32::from_be_bytes(head[stsc + 8..stsc + 12].try_into().unwrap());
        assert_eq!(entries, 2);
    }

    #[test]
    fn past_4_gib_offsets_are_64_bit() {
        let sizes = vec![300u32; 50];
        let head = header(&track(&sizes), &Tags::default(), 5 << 30);
        assert!(find_box(&head, b"co64").is_some());
        assert!(find_box(&head, b"stco").is_none());
        // A 64-bit mdat size: size 1, "mdat", then the real size.
        let mdat = &head[head.len() - 16..];
        assert_eq!(&mdat[..8], &[0, 0, 0, 1, b'm', b'd', b'a', b't']);
        assert_eq!(
            u64::from_be_bytes(mdat[8..].try_into().unwrap()),
            (5u64 << 30) + 16
        );
        let co64 = find_box(&head, b"co64").unwrap();
        let first = u64::from_be_bytes(head[co64 + 12..co64 + 20].try_into().unwrap());
        assert_eq!(first, head.len() as u64);
    }

    #[test]
    fn the_edit_list_skips_the_priming() {
        let sizes = vec![300u32; 50];
        let t = track(&sizes);
        let head = header(&t, &Tags::default(), 15_000);
        let elst = find_box(&head, b"elst").unwrap();
        let e = &head[elst + 8..];
        assert_eq!(u32::from_be_bytes(e[0..4].try_into().unwrap()), 1);
        assert_eq!(
            u32::from_be_bytes(e[4..8].try_into().unwrap()) as u64,
            t.frames
        );
        assert_eq!(u32::from_be_bytes(e[8..12].try_into().unwrap()), 1024);
        let smpb = find_box(&head, b"iTunSMPB").unwrap();
        let text = String::from_utf8_lossy(&head[smpb + 8 + 16..smpb + 8 + 16 + 40]);
        // priming 1024, padding 50*1024 - 1024 - frames = 976
        assert!(text.starts_with(" 00000000 00000400 000003D0 "), "{text}");
    }

    #[test]
    fn descriptor_lengths_grow_past_127() {
        assert_eq!(descriptor(5, &[1, 2]), vec![5, 2, 1, 2]);
        let long = descriptor(5, &[0; 200]);
        assert_eq!(&long[..3], &[5, 0x81, 0x48]);
    }
}
