//! Ogg Vorbis and Opus, in and out.
//!
//! * Vorbis is written by `vorbis_rs` (aoTuV libvorbis and libogg, built from
//!   source). Rust owns the file: the encoder writes to a [`BufWriter`] over
//!   the [`File`] in the [`SinkSpec`], so no path ever reaches C.
//! * Opus is written by libopus (`opusic-sys`), and the Ogg pages around it
//!   are Kiln's own: header, lacing, CRC-32 and granule positions are in
//!   [`PageWriter`]. The same module reads Opus back: [`PageReader`] and
//!   [`Packets`] demux the pages, libopus decodes the packets. The same
//!   encoder gives a Matroska video's sound as packets ([`OpusPackets`]).
//!
//! Channel order: Kiln keeps channels in WAVE order (front left, front right,
//! centre, LFE, back left, back right, ...), and Ogg Vorbis and Opus keep
//! them in Vorbis order (left, centre, right, ...). Every sink and source
//! here converts, so a file plays with each channel where it belongs.

use crate::error::{MediaError, Result};
use crate::gapfill::GapFill;
use crate::pcm::default_mask;
use crate::pipeline::{Progressed, Sink, SinkSpec, Source, BLOCK_FRAMES};
use crate::tags::vorbis;
use crate::tune::Setting;
use crate::types::{Block, Channels, Codec, Picture, SourceInfo, Tags};
use crate::video::packet::{AudioParams, Packet, StreamCodec, StreamSpec};
use opusic_sys as o;
use std::collections::VecDeque;
use std::ffi::c_int;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::num::{NonZeroU32, NonZeroU8};
use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoder, VorbisEncoderBuilder};

/// The extension shown in a "damaged" message.
const CODE: &str = "OPUS";

/// Both codecs here are defined for 1 to 8 channels with a known layout.
const MAX_CHANNELS: u16 = 8;

/// The comment that carries cover art: a FLAC PICTURE block, in base64.
const PICTURE_KEY: &str = "METADATA_BLOCK_PICTURE";

fn encoder_error(what: impl std::fmt::Display) -> MediaError {
    MediaError::Encoder(what.to_string())
}

/// What a Vorbis encoder failure is: an I/O error (a full disk, say) as
/// itself, so the row can say there is no room; anything else the encoder's.
fn vorbis_error(e: vorbis_rs::VorbisError) -> MediaError {
    match e {
        vorbis_rs::VorbisError::Io(e) => MediaError::Io(e),
        e => encoder_error(e),
    }
}

fn damaged() -> MediaError {
    MediaError::Damaged { code: CODE.into() }
}

// ---------------------------------------------------------------------------
// Channel order

/// Speaker bits of a WAVEFORMATEXTENSIBLE mask.
const FL: u32 = 1 << 0;
const FR: u32 = 1 << 1;
const FC: u32 = 1 << 2;
const LFE: u32 = 1 << 3;
const BL: u32 = 1 << 4;
const BR: u32 = 1 << 5;
const BC: u32 = 1 << 8;
const SL: u32 = 1 << 9;
const SR: u32 = 1 << 10;

/// The speakers of the Vorbis (and Opus family 1) layout for `count`
/// channels, in file order.
fn vorbis_layout(count: usize) -> &'static [u32] {
    match count {
        1 => &[FC],
        2 => &[FL, FR],
        3 => &[FL, FC, FR],
        4 => &[FL, FR, BL, BR],
        5 => &[FL, FC, FR, BL, BR],
        6 => &[FL, FC, FR, BL, BR, LFE],
        7 => &[FL, FC, FR, SL, SR, BC, LFE],
        8 => &[FL, FC, FR, SL, SR, BL, BR, LFE],
        _ => &[],
    }
}

/// For a layout of `ch`, the Kiln channel to put at each Vorbis position:
/// `vorbis[i] = kiln[order[i]]`. Kiln's channels run through the mask's
/// speakers from the lowest bit up. A back speaker stands in for the side one
/// of the same side, and the other way round, since Vorbis has one pair of
/// each at most. A mask that doesn't name `count` speakers, or that leaves a
/// Vorbis position without one, gives the order for the standard layout.
/// Empty for a count Vorbis has no layout for.
fn vorbis_order(ch: Channels) -> Vec<usize> {
    let count = ch.count as usize;
    let layout = vorbis_layout(count);
    if layout.is_empty() {
        return Vec::new();
    }
    if ch.mask.count_ones() as usize == count {
        let kiln: Vec<u32> = (0..32)
            .map(|b| 1u32 << b)
            .filter(|b| ch.mask & b != 0)
            .collect();
        let mut taken = vec![false; count];
        let mut order = Vec::with_capacity(count);
        for &want in layout {
            let alias = match want {
                BL => SL,
                SL => BL,
                BR => SR,
                SR => BR,
                other => other,
            };
            let found = [want, alias]
                .into_iter()
                .find_map(|s| kiln.iter().position(|&k| k == s).filter(|&i| !taken[i]));
            match found {
                Some(i) => {
                    taken[i] = true;
                    order.push(i);
                }
                None => break,
            }
        }
        if order.len() == count {
            return order;
        }
    }
    if count == 1 || ch.mask == default_mask(ch.count) {
        // Mono is the one speaker, whatever it is called; the standard layout
        // has nothing left to fall back to.
        return (0..count).collect();
    }
    vorbis_order(Channels {
        count: ch.count,
        mask: default_mask(ch.count),
    })
}

/// A sample libvorbis and libopus can take: finite and within full scale.
fn unit(x: f32) -> f32 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(-1.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// Base64, for the cover art comment

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decodes base64, ignoring white space and padding. `None` on any other
/// character.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The comment lines for `tags`: the usual ones, plus the cover as
/// `METADATA_BLOCK_PICTURE`.
fn comment_lines(tags: &Tags) -> Vec<(String, String)> {
    let mut lines = vorbis::comments(tags);
    if let Some(p) = tags.picture.as_ref().filter(|p| !p.data.is_empty()) {
        lines.push((
            PICTURE_KEY.to_string(),
            base64_encode(&vorbis::picture_block(p)),
        ));
    }
    lines
}

// ---------------------------------------------------------------------------
// Vorbis out

/// The Ogg stream serial number: unique enough to tell two files apart.
fn stream_serial() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mixed = nanos ^ (std::process::id() as u64).rotate_left(32) ^ 0x9E37_79B9_7F4A_7C15;
    (mixed ^ (mixed >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9) as u32
}

struct VorbisSink {
    enc: Option<VorbisEncoder<BufWriter<File>>>,
    channels: u16,
    order: Vec<usize>,
    planes: Vec<Vec<f32>>,
}

// SAFETY: the encoder is libvorbis and libogg state reached only through
// `&mut self` or by value. Neither library has thread affinity or global state
// that the encoder touches.
unsafe impl Send for VorbisSink {}

/// Makes a sink that writes an Ogg Vorbis file with the spec's tags and cover.
pub fn vorbis_sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    let strategy = match *spec.setting {
        Setting::VorbisQ { tenths } => VorbisBitrateManagementStrategy::QualityVbr {
            // The quality is on the -1 to 10 scale, times ten; libvorbis takes
            // it divided by ten again.
            target_quality: (tenths as f32 / 100.0).clamp(-0.2, 1.0),
        },
        Setting::VorbisAbr { kbps } => VorbisBitrateManagementStrategy::Abr {
            average_bitrate: NonZeroU32::new(kbps.saturating_mul(1000))
                .ok_or_else(|| encoder_error("a Vorbis bitrate of zero"))?,
        },
        _ => return Err(encoder_error("this setting isn't a Vorbis quality")),
    };
    let count = spec.channels.count;
    if count == 0 || count > MAX_CHANNELS {
        return Err(encoder_error(format!(
            "Vorbis holds 1 to {MAX_CHANNELS} channels, not {count}"
        )));
    }
    let rate = NonZeroU32::new(spec.rate).ok_or_else(|| encoder_error("a sample rate of zero"))?;
    let out = BufWriter::with_capacity(256 * 1024, spec.out);
    let mut builder = VorbisEncoderBuilder::new_with_serial(
        rate,
        NonZeroU8::new(count as u8).expect("a non-zero count"),
        out,
        stream_serial() as i32,
    );
    builder.bitrate_management_strategy(strategy);
    for (name, value) in comment_lines(spec.tags) {
        builder.comment_tag(name, value).map_err(vorbis_error)?;
    }
    let enc = builder.build().map_err(vorbis_error)?;
    Ok(Box::new(VorbisSink {
        enc: Some(enc),
        channels: count,
        order: vorbis_order(spec.channels),
        planes: vec![Vec::new(); count as usize],
    }))
}

impl Sink for VorbisSink {
    fn write(&mut self, b: &Block) -> Result<()> {
        if b.channels != self.channels {
            return Err(encoder_error(format!(
                "a block of {} channels reached a {}-channel Vorbis file",
                b.channels, self.channels
            )));
        }
        let frames = b.frames();
        if frames == 0 {
            return Ok(());
        }
        let width = self.channels as usize;
        for (plane, &from) in self.planes.iter_mut().zip(&self.order) {
            plane.clear();
            plane.extend(b.samples.chunks_exact(width).map(|f| unit(f[from])));
        }
        let enc = self.enc.as_mut().expect("an encoder until finish");
        enc.encode_audio_block(&self.planes).map_err(vorbis_error)
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        let enc = self.enc.take().expect("an encoder until finish");
        let mut out = enc.finish().map_err(vorbis_error)?;
        out.flush()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The Ogg page layer

/// CRC-32 with polynomial 0x04C11DB7, zero start, no reflection, no final xor.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            r = if r & 0x8000_0000 != 0 {
                (r << 1) ^ 0x04C1_1DB7
            } else {
                r << 1
            };
            bit += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
};

fn crc_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = (crc << 8) ^ CRC_TABLE[((crc >> 24) as u8 ^ b) as usize];
    }
    crc
}

/// The granule position of a page that ends no packet.
const NO_GRANULE: u64 = u64::MAX;

const CONTINUED: u8 = 1;
const FIRST: u8 = 2;
const LAST: u8 = 4;

/// Writes one logical Ogg stream: packets in, pages out.
///
/// Packets are laced into 255-byte segments, a page holds at most 255
/// segments, and a packet too big for the page carries on in the next one.
struct PageWriter<W: Write> {
    out: W,
    serial: u32,
    seq: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
    /// The granule of the last packet that ends on the page being built.
    granule: Option<u64>,
    /// True when the page being built starts inside a packet.
    continued: bool,
    started: bool,
    packets_on_page: u32,
}

impl<W: Write> PageWriter<W> {
    fn new(out: W, serial: u32) -> Self {
        PageWriter {
            out,
            serial,
            seq: 0,
            lacing: Vec::with_capacity(255),
            body: Vec::new(),
            granule: None,
            continued: false,
            started: false,
            packets_on_page: 0,
        }
    }

    /// Adds a packet whose last sample is `granule`.
    fn packet(&mut self, data: &[u8], granule: u64) -> io::Result<()> {
        let mut rest = data;
        loop {
            if self.lacing.len() == 255 {
                self.write_page(0)?;
            }
            let n = rest.len().min(255);
            self.lacing.push(n as u8);
            self.body.extend_from_slice(&rest[..n]);
            rest = &rest[n..];
            // A packet ends on a segment shorter than 255, which may be empty.
            if n < 255 {
                break;
            }
        }
        self.granule = Some(granule);
        self.packets_on_page += 1;
        Ok(())
    }

    /// Ends the page being built, if it holds anything.
    fn flush(&mut self) -> io::Result<()> {
        if self.lacing.is_empty() {
            return Ok(());
        }
        self.write_page(0)
    }

    /// Ends the stream: the last page, marked as the last.
    fn finish(&mut self) -> io::Result<()> {
        self.write_page(LAST)?;
        self.out.flush()
    }

    fn write_page(&mut self, extra: u8) -> io::Result<()> {
        let mut kind = extra;
        if self.continued {
            kind |= CONTINUED;
        }
        if !self.started {
            kind |= FIRST;
        }
        let granule = self.granule.take().unwrap_or(NO_GRANULE);
        let mut page = Vec::with_capacity(27 + self.lacing.len() + self.body.len());
        page.extend_from_slice(b"OggS");
        page.push(0);
        page.push(kind);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.seq.to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        page.push(self.lacing.len() as u8);
        page.extend_from_slice(&self.lacing);
        page.extend_from_slice(&self.body);
        let crc = crc_update(0, &page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        self.out.write_all(&page)?;
        self.seq = self.seq.wrapping_add(1);
        self.started = true;
        self.continued = self.lacing.last() == Some(&255);
        self.lacing.clear();
        self.body.clear();
        self.packets_on_page = 0;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Opus out

/// Samples per channel in one Opus frame: 20 ms at 48 kHz.
const FRAME: usize = 960;
/// Opus always runs at this rate.
pub(crate) const OPUS_RATE: u32 = 48_000;
/// Pages are ended about once a second.
const PACKETS_PER_PAGE: u32 = 50;
/// More than any packet the encoder makes for eight channels.
const MAX_PACKET: usize = 16 * 1024;

/// The Vorbis-order channel mapping an Opus stream uses.
struct Layout {
    family: u8,
    streams: u8,
    coupled: u8,
    mapping: Vec<u8>,
}

/// libopus's encoder as Kiln sets it up, for an Opus file ([`opus_sink`])
/// and for the sound of a Matroska video ([`OpusPackets`]): the bitrate in
/// constrained VBR, complexity 10, the surround layout, and the input turned
/// to Vorbis order and held within full scale.
struct OpusEnc {
    enc: *mut o::OpusMSEncoder,
    channels: usize,
    order: Vec<usize>,
    layout: Layout,
    pre_skip: u64,
    /// Input in Vorbis order, clamped, not yet encoded.
    pending: Vec<f32>,
    in_frames: u64,
    /// Packets made so far.
    packets: u64,
    scratch: Vec<u8>,
}

// SAFETY: the libopus encoder has no thread affinity, and nothing else holds
// its pointer: it is only used through `&mut self` or by value.
unsafe impl Send for OpusEnc {}

impl OpusEnc {
    /// An encoder for `channels` at 48 kHz, as `setting` (an Opus bitrate) asks.
    fn new(channels: Channels, setting: &Setting) -> Result<OpusEnc> {
        let Setting::Opus { kbps, speech } = *setting else {
            return Err(encoder_error("this setting isn't an Opus bitrate"));
        };
        let count = channels.count;
        if count == 0 || count > MAX_CHANNELS {
            return Err(encoder_error(format!(
                "Opus holds 1 to {MAX_CHANNELS} channels here, not {count}"
            )));
        }
        let family: c_int = if count <= 2 { 0 } else { 1 };
        let application = if speech {
            o::OPUS_APPLICATION_VOIP
        } else {
            o::OPUS_APPLICATION_AUDIO
        };

        let mut streams: c_int = 0;
        let mut coupled: c_int = 0;
        let mut mapping = [0u8; 255];
        let mut error: c_int = 0;
        // SAFETY: every pointer is to a live local; the call fills `mapping`
        // with `count` bytes.
        let enc = unsafe {
            o::opus_multistream_surround_encoder_create(
                OPUS_RATE as i32,
                count as c_int,
                family,
                &mut streams,
                &mut coupled,
                mapping.as_mut_ptr(),
                application,
                &mut error,
            )
        };
        if enc.is_null() || error != o::OPUS_OK {
            return Err(encoder_error(format!(
                "libopus can't start (error {error})"
            )));
        }
        // From here `this` owns `enc`, and `Drop` frees it.
        let mut this = OpusEnc {
            enc,
            channels: count as usize,
            order: vorbis_order(channels),
            layout: Layout {
                family: family as u8,
                streams: streams as u8,
                coupled: coupled as u8,
                mapping: mapping[..count as usize].to_vec(),
            },
            pre_skip: 0,
            pending: Vec::new(),
            in_frames: 0,
            packets: 0,
            scratch: vec![0; MAX_PACKET],
        };

        // SAFETY: `enc` is live. Each request takes the one argument given.
        let ok = unsafe {
            let mut lookahead: c_int = 0;
            let r = [
                o::opus_multistream_encoder_ctl(
                    enc,
                    o::OPUS_SET_BITRATE_REQUEST,
                    (kbps.clamp(1, 100_000) * 1000) as i32,
                ),
                o::opus_multistream_encoder_ctl(enc, o::OPUS_SET_VBR_REQUEST, 1i32),
                o::opus_multistream_encoder_ctl(enc, o::OPUS_SET_VBR_CONSTRAINT_REQUEST, 1i32),
                o::opus_multistream_encoder_ctl(enc, o::OPUS_SET_COMPLEXITY_REQUEST, 10i32),
                o::opus_multistream_encoder_ctl(
                    enc,
                    o::OPUS_GET_LOOKAHEAD_REQUEST,
                    &mut lookahead as *mut c_int,
                ),
            ];
            this.pre_skip = lookahead.max(0) as u64;
            r.iter().all(|&code| code == o::OPUS_OK)
        };
        if !ok {
            return Err(encoder_error("libopus refused the encoder settings"));
        }
        Ok(this)
    }

    /// The OpusHead: an Ogg stream's first packet, a Matroska track's
    /// private data.
    fn head(&self) -> Vec<u8> {
        opus_head(self.channels as u8, self.pre_skip as u16, &self.layout)
    }

    /// Takes a block's samples, in Vorbis order and held within full scale.
    fn take(&mut self, b: &Block) -> Result<()> {
        if b.channels as usize != self.channels {
            return Err(encoder_error(format!(
                "a block of {} channels reached a {}-channel Opus encoder",
                b.channels, self.channels
            )));
        }
        self.in_frames += b.frames() as u64;
        for f in b.samples.chunks_exact(self.channels) {
            self.pending.extend(self.order.iter().map(|&c| unit(f[c])));
        }
        Ok(())
    }

    /// Encodes the frame of `FRAME * channels` samples at `pending[from..]`.
    fn encode_frame(&mut self, from: usize) -> Result<Vec<u8>> {
        let end = from + FRAME * self.channels;
        // SAFETY: `pending[from..end]` is one frame of this encoder's width,
        // and `scratch` is the size passed as the limit.
        let n = unsafe {
            o::opus_multistream_encode_float(
                self.enc,
                self.pending[from..end].as_ptr(),
                FRAME as c_int,
                self.scratch.as_mut_ptr(),
                self.scratch.len() as i32,
            )
        };
        if n < 0 {
            return Err(encoder_error(format!("libopus can't encode (error {n})")));
        }
        self.packets += 1;
        Ok(self.scratch[..n as usize].to_vec())
    }

    /// Encodes every whole frame waiting, handing each packet to `out`.
    fn drain(&mut self, out: &mut dyn FnMut(Vec<u8>) -> Result<()>) -> Result<()> {
        let size = FRAME * self.channels;
        let mut at = 0;
        while self.pending.len() - at >= size {
            out(self.encode_frame(at)?)?;
            at += size;
        }
        self.pending.drain(..at);
        Ok(())
    }

    /// Encodes the end: the last partial frame padded with silence, then
    /// silence until the packets cover the delay and every input sample.
    /// Gives where the audio ends, in samples counting the pre-skip.
    fn finish(&mut self, out: &mut dyn FnMut(Vec<u8>) -> Result<()>) -> Result<u64> {
        let size = FRAME * self.channels;
        let want = self.pre_skip + self.in_frames;
        if !self.pending.is_empty() {
            self.pending.resize(size, 0.0);
            out(self.encode_frame(0)?)?;
            self.pending.clear();
        }
        self.pending.resize(size, 0.0);
        while self.packets * (FRAME as u64) < want {
            out(self.encode_frame(0)?)?;
        }
        self.pending.clear();
        Ok(want)
    }
}

impl Drop for OpusEnc {
    fn drop(&mut self) {
        // SAFETY: `enc` came from the create call and is freed once.
        unsafe { o::opus_multistream_encoder_destroy(self.enc) };
    }
}

struct OpusSink {
    enc: OpusEnc,
    pages: PageWriter<BufWriter<File>>,
    packets: u64,
    /// The newest packet, held back until the next one shows it isn't the last.
    held: Option<Vec<u8>>,
}

/// Makes a sink that writes an Opus file. The rate must be 48 kHz: the
/// conversion resamples first.
pub fn opus_sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    if spec.rate != OPUS_RATE {
        return Err(encoder_error(format!(
            "Opus is written at {OPUS_RATE} Hz, not {} Hz",
            spec.rate
        )));
    }
    let enc = OpusEnc::new(spec.channels, spec.setting)?;
    let mut pages = PageWriter::new(
        BufWriter::with_capacity(256 * 1024, spec.out),
        stream_serial(),
    );
    pages.packet(&enc.head(), 0)?;
    pages.flush()?;
    pages.packet(&opus_tags(&comment_lines(spec.tags)), 0)?;
    pages.flush()?;
    Ok(Box::new(OpusSink {
        enc,
        pages,
        packets: 0,
        held: None,
    }))
}

/// The identification header.
fn opus_head(channels: u8, pre_skip: u16, layout: &Layout) -> Vec<u8> {
    let mut h = Vec::with_capacity(32);
    h.extend_from_slice(b"OpusHead");
    h.push(1);
    h.push(channels);
    h.extend_from_slice(&pre_skip.to_le_bytes());
    h.extend_from_slice(&OPUS_RATE.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes());
    h.push(layout.family);
    if layout.family != 0 {
        h.push(layout.streams);
        h.push(layout.coupled);
        h.extend_from_slice(&layout.mapping);
    }
    h
}

/// The comment header: the vendor, then one line per comment.
fn opus_tags(lines: &[(String, String)]) -> Vec<u8> {
    const VENDOR: &str = "Kiln";
    let mut t = Vec::new();
    t.extend_from_slice(b"OpusTags");
    t.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
    t.extend_from_slice(VENDOR.as_bytes());
    t.extend_from_slice(&(lines.len() as u32).to_le_bytes());
    for (name, value) in lines {
        let line = format!("{name}={value}");
        t.extend_from_slice(&(line.len() as u32).to_le_bytes());
        t.extend_from_slice(line.as_bytes());
    }
    t
}

/// Takes a new packet and writes the one before it to the pages.
fn page_out(
    pages: &mut PageWriter<BufWriter<File>>,
    packets: &mut u64,
    held: &mut Option<Vec<u8>>,
    packet: Vec<u8>,
) -> Result<()> {
    *packets += 1;
    if let Some(prev) = held.replace(packet) {
        // `prev` is packet number `packets - 1`; each ends 960 samples on.
        pages.packet(&prev, (*packets - 1) * FRAME as u64)?;
        if pages.packets_on_page >= PACKETS_PER_PAGE {
            pages.flush()?;
        }
    }
    Ok(())
}

impl Sink for OpusSink {
    fn write(&mut self, b: &Block) -> Result<()> {
        if b.frames() == 0 && b.channels as usize == self.enc.channels {
            return Ok(());
        }
        self.enc.take(b)?;
        let OpusSink {
            enc,
            pages,
            packets,
            held,
        } = self;
        enc.drain(&mut |p| page_out(pages, packets, held, p))
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        let OpusSink {
            enc,
            pages,
            packets,
            held,
        } = &mut *self;
        let want = enc.finish(&mut |p| page_out(pages, packets, held, p))?;
        // The last packet ends where the input does, so a decoder trims the
        // padding exactly.
        if let Some(last) = held.take() {
            pages.packet(&last, want)?;
        }
        pages.finish()?;
        Ok(())
    }
}

/// Opus packets for the sound of a Matroska video, from part 1's blocks: the
/// same encoder as [`opus_sink`], its packets given as [`Packet`]s instead of
/// written to Ogg pages.
///
/// Times count 48 kHz samples, as FFmpeg gives them (R16): a packet's `pts`
/// is when its first sample is heard, so the first packet starts at minus
/// the pre-skip, which the Matroska writer turns into the track's
/// `CodecDelay`. The last packet's length ends where the input does.
pub struct OpusPackets {
    enc: OpusEnc,
    stream: usize,
    /// The newest packet and its number, held back until the next one shows
    /// it isn't the last.
    held: Option<(u64, Vec<u8>)>,
}

impl OpusPackets {
    /// An encoder for `channels` at 48 kHz with `setting` (an Opus bitrate),
    /// whose packets get stream number `stream`.
    pub fn new(channels: Channels, setting: &Setting, stream: usize) -> Result<OpusPackets> {
        Ok(OpusPackets {
            enc: OpusEnc::new(channels, setting)?,
            stream,
            held: None,
        })
    }

    /// The stream the packets belong to, for a muxer.
    pub fn stream(&self) -> StreamSpec {
        StreamSpec {
            codec: StreamCodec::Opus {
                head: self.enc.head(),
            },
            timescale: OPUS_RATE,
            video: None,
            audio: Some(AudioParams {
                rate: OPUS_RATE,
                channels: self.enc.channels as u16,
            }),
        }
    }

    /// Encodes a block (48 kHz, the encoder's channels), adding the packets
    /// ready to `out`.
    pub fn push(&mut self, b: &Block, out: &mut Vec<Packet>) -> Result<()> {
        if b.rate != OPUS_RATE {
            return Err(encoder_error(format!(
                "Opus is written at {OPUS_RATE} Hz, not {} Hz",
                b.rate
            )));
        }
        self.enc.take(b)?;
        let (stream, pre_skip) = (self.stream, self.enc.pre_skip);
        let OpusPackets { enc, held, .. } = self;
        enc.drain(&mut |p| {
            hold_opus(held, p, stream, pre_skip, out);
            Ok(())
        })
    }

    /// Encodes the end and adds every packet left to `out`.
    pub fn finish(&mut self, out: &mut Vec<Packet>) -> Result<()> {
        let (stream, pre_skip) = (self.stream, self.enc.pre_skip);
        let OpusPackets { enc, held, .. } = self;
        let want = enc.finish(&mut |p| {
            hold_opus(held, p, stream, pre_skip, out);
            Ok(())
        })?;
        if let Some((n, data)) = held.take() {
            let start = n * FRAME as u64;
            let length = want.saturating_sub(start).max(1);
            out.push(opus_packet(stream, n, pre_skip, data, length));
        }
        Ok(())
    }
}

/// Holds the next packet and gives out the one held before it, a whole
/// frame long.
fn hold_opus(
    held: &mut Option<(u64, Vec<u8>)>,
    data: Vec<u8>,
    stream: usize,
    pre_skip: u64,
    out: &mut Vec<Packet>,
) {
    let n = held.as_ref().map_or(0, |(n, _)| n + 1);
    if let Some((prev, bytes)) = held.replace((n, data)) {
        out.push(opus_packet(stream, prev, pre_skip, bytes, FRAME as u64));
    }
}

/// Packet number `n`, heard from `n` frames in less the pre-skip.
fn opus_packet(stream: usize, n: u64, pre_skip: u64, data: Vec<u8>, length: u64) -> Packet {
    let pts = (n * FRAME as u64) as i64 - pre_skip as i64;
    Packet {
        stream,
        pts,
        dts: pts,
        duration: length as i64,
        key: true,
        data,
    }
}

// ---------------------------------------------------------------------------
// The Ogg page reader

struct Page {
    kind: u8,
    granule: i64,
    serial: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
}

/// Reads whole, checked pages one after another.
struct PageReader {
    r: BufReader<File>,
    /// Bytes of whole pages read so far.
    pos: u64,
}

/// Reads until `buf` is full or the file ends; returns the count read.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

impl PageReader {
    /// The next page, or `None` at the end of the file or where a page is cut
    /// short. A page that is not a page, or whose checksum is wrong, is damage.
    fn next(&mut self) -> Result<Option<Page>> {
        let mut head = [0u8; 27];
        if read_full(&mut self.r, &mut head)? < head.len() {
            return Ok(None);
        }
        if &head[..4] != b"OggS" || head[4] != 0 {
            return Err(damaged());
        }
        let mut lacing = vec![0u8; head[26] as usize];
        if read_full(&mut self.r, &mut lacing)? < lacing.len() {
            return Ok(None);
        }
        let mut body = vec![0u8; lacing.iter().map(|&l| l as usize).sum()];
        if read_full(&mut self.r, &mut body)? < body.len() {
            return Ok(None);
        }
        let want = u32::from_le_bytes([head[22], head[23], head[24], head[25]]);
        head[22..26].fill(0);
        let crc = crc_update(crc_update(crc_update(0, &head), &lacing), &body);
        if crc != want {
            return Err(damaged());
        }
        self.pos += (27 + lacing.len() + body.len()) as u64;
        Ok(Some(Page {
            kind: head[5],
            granule: i64::from_le_bytes(head[6..14].try_into().expect("8 bytes")),
            serial: u32::from_le_bytes([head[14], head[15], head[16], head[17]]),
            lacing,
            body,
        }))
    }
}

/// Packets of one logical stream, put back together from its pages.
struct Packets {
    pages: PageReader,
    serial: Option<u32>,
    queue: VecDeque<Vec<u8>>,
    partial: Vec<u8>,
    /// The granule of the stream's last page, once it has been read.
    end_granule: Option<i64>,
    ended: bool,
    /// The file ended (rather than the stream's last page coming), so no
    /// chained stream can follow.
    at_eof: bool,
}

impl Packets {
    fn next(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(p) = self.queue.pop_front() {
                return Ok(Some(p));
            }
            if self.ended {
                return Ok(None);
            }
            let Some(page) = self.pages.next()? else {
                self.ended = true;
                self.at_eof = true;
                return Ok(None);
            };
            // Pages of other streams multiplexed with this one are passed over.
            let serial = *self.serial.get_or_insert(page.serial);
            if page.serial != serial {
                continue;
            }
            self.take(&page);
        }
    }

    /// Puts a page of this stream's packets in the queue.
    fn take(&mut self, page: &Page) {
        // A page that does not carry on from the last one drops a packet
        // that was left unfinished.
        if page.kind & CONTINUED == 0 {
            self.partial.clear();
        }
        let mut at = 0;
        for &lace in &page.lacing {
            self.partial
                .extend_from_slice(&page.body[at..at + lace as usize]);
            at += lace as usize;
            if lace < 255 {
                self.queue.push_back(std::mem::take(&mut self.partial));
            }
        }
        if page.kind & LAST != 0 {
            self.ended = true;
            if page.granule >= 0 {
                self.end_granule = Some(page.granule);
            }
        }
    }

    /// Once the stream's last page has been read: the identification packet
    /// of a stream that starts after it (a chained Ogg file, as stream rips
    /// and joined files are), with its comment packet read past, so the next
    /// packets are its audio. `None` at the end of the file. Pages that start
    /// no stream are passed over.
    fn next_part(&mut self) -> Result<Option<Vec<u8>>> {
        if self.at_eof || !self.ended || !self.queue.is_empty() {
            return Ok(None);
        }
        loop {
            let Some(page) = self.pages.next()? else {
                self.at_eof = true;
                return Ok(None);
            };
            if page.kind & FIRST == 0 {
                continue;
            }
            self.serial = Some(page.serial);
            self.queue.clear();
            self.partial.clear();
            self.end_granule = None;
            self.ended = false;
            self.take(&page);
            let head = self.next()?.ok_or_else(damaged)?;
            if !head.starts_with(b"OpusHead") {
                return Err(crate::sym::chained());
            }
            self.next()?.ok_or_else(damaged)?;
            return Ok(Some(head));
        }
    }
}

/// The granule of the last page of the stream `serial`, found in the tail of
/// the file. Leaves the file position where it was found, so callers seek.
fn last_granule(file: &mut (impl Read + Seek), len: u64, serial: u32) -> Option<i64> {
    // The biggest page is 27 + 255 + 255 * 255 bytes.
    const WINDOW: u64 = 65_307 + 1_024;
    let from = len.saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut tail = vec![0u8; (len - from) as usize];
    file.read_exact(&mut tail).ok()?;
    let mut end = tail.len();
    while let Some(at) = tail[..end].windows(4).rposition(|w| w == b"OggS") {
        end = at + 3;
        let head = &tail[at..];
        if head.len() < 27 || head[4] != 0 {
            continue;
        }
        let nsegs = head[26] as usize;
        if head.len() < 27 + nsegs {
            continue;
        }
        let body: usize = head[27..27 + nsegs].iter().map(|&l| l as usize).sum();
        let total = 27 + nsegs + body;
        if head.len() < total {
            continue;
        }
        let mut page = head[..total].to_vec();
        let want = u32::from_le_bytes([page[22], page[23], page[24], page[25]]);
        page[22..26].fill(0);
        if crc_update(0, &page) != want {
            continue;
        }
        if u32::from_le_bytes([page[14], page[15], page[16], page[17]]) != serial {
            continue;
        }
        let granule = i64::from_le_bytes(page[6..14].try_into().expect("8 bytes"));
        if granule >= 0 {
            return Some(granule);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Opus in

/// The most samples per channel one Opus packet can hold: 120 ms.
const MAX_PACKET_FRAMES: usize = 5760;
/// Packets that may fail to decode, after the first good one, before the
/// file counts as damaged.
const MAX_SKIPPED: u32 = 10;

struct OpusHead {
    channels: u8,
    pre_skip: u16,
    gain: i16,
    streams: u8,
    coupled: u8,
    mapping: Vec<u8>,
}

fn parse_head(p: &[u8]) -> Result<OpusHead> {
    if p.len() < 19 || &p[..8] != b"OpusHead" || p[8] >> 4 != 0 {
        return Err(damaged());
    }
    let channels = p[9];
    let pre_skip = u16::from_le_bytes([p[10], p[11]]);
    let gain = i16::from_le_bytes([p[16], p[17]]);
    let family = p[18];
    if channels == 0 {
        return Err(damaged());
    }
    match family {
        0 => {
            if channels > 2 {
                return Err(damaged());
            }
            Ok(OpusHead {
                channels,
                pre_skip,
                gain,
                streams: 1,
                coupled: channels - 1,
                mapping: (0..channels).collect(),
            })
        }
        1 => {
            let n = channels as usize;
            if channels > MAX_CHANNELS as u8 || p.len() < 21 + n {
                return Err(damaged());
            }
            let (streams, coupled) = (p[19], p[20]);
            let mapping = p[21..21 + n].to_vec();
            let used = streams as usize + coupled as usize;
            if streams == 0
                || coupled > streams
                || used > 255
                || mapping.iter().any(|&m| m != 255 && m as usize >= used)
            {
                return Err(damaged());
            }
            Ok(OpusHead {
                channels,
                pre_skip,
                gain,
                streams,
                coupled,
                mapping,
            })
        }
        // Free-form channels and ambisonics have no speaker positions Kiln
        // could keep.
        _ => Err(MediaError::Unsupported {
            codec: "Opus with free-form channels".into(),
        }),
    }
}

/// Reads the tags in an OpusTags packet. Anything that doesn't fit is left out.
fn parse_tags(p: &[u8]) -> Result<Tags> {
    if p.len() < 12 || &p[..8] != b"OpusTags" {
        return Err(damaged());
    }
    let u32_at = |at: usize| -> Option<usize> {
        p.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let mut tags = Tags::default();
    let mut side = Side::default();
    let mut at = 8;
    let Some(vendor) = u32_at(at) else {
        return Ok(tags);
    };
    at = at.saturating_add(4).saturating_add(vendor);
    let Some(count) = u32_at(at) else {
        return Ok(tags);
    };
    at += 4;
    for _ in 0..count {
        let Some(len) = u32_at(at) else { break };
        let Some(line) = p.get(at + 4..at.saturating_add(4).saturating_add(len)) else {
            break;
        };
        at += 4 + len;
        let line = String::from_utf8_lossy(line);
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        add_comment(&mut tags, &mut side, &name.to_ascii_uppercase(), value);
    }
    if let Some((_, total @ None)) = tags.track.as_mut() {
        *total = side.track_total;
    }
    if let Some((_, total @ None)) = tags.disc.as_mut() {
        *total = side.disc_total;
    }
    Ok(tags)
}

/// What a tag reader holds while it goes through the comments.
#[derive(Default)]
struct Side {
    track_total: Option<u32>,
    disc_total: Option<u32>,
    /// True once the cover kept is a front cover.
    front: bool,
}

fn add_comment(tags: &mut Tags, side: &mut Side, name: &str, value: &str) {
    let clean = value.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if clean.is_empty() {
        return;
    }
    let text = || Some(clean.to_string());
    let part = || {
        let mut it = clean.splitn(2, '/');
        let n = it.next()?.trim().parse().ok()?;
        Some((n, it.next().and_then(|t| t.trim().parse().ok())))
    };
    let number = || clean.parse::<u32>().ok();
    match name {
        "TITLE" => tags.title = tags.title.take().or_else(text),
        "ARTIST" => tags.artist = tags.artist.take().or_else(text),
        "ALBUM" => tags.album = tags.album.take().or_else(text),
        "ALBUMARTIST" => tags.album_artist = tags.album_artist.take().or_else(text),
        "GENRE" => tags.genre = tags.genre.take().or_else(text),
        "COMMENT" | "DESCRIPTION" => tags.comment = tags.comment.take().or_else(text),
        "DATE" | "YEAR" => tags.year = tags.year.take().or_else(text),
        "TRACKNUMBER" => tags.track = tags.track.or_else(part),
        "DISCNUMBER" => tags.disc = tags.disc.or_else(part),
        "TRACKTOTAL" | "TOTALTRACKS" => side.track_total = side.track_total.or_else(number),
        "DISCTOTAL" | "TOTALDISCS" => side.disc_total = side.disc_total.or_else(number),
        PICTURE_KEY if !side.front => {
            if let Some((front, picture)) = base64_decode(clean).and_then(|b| parse_picture(&b)) {
                if tags.picture.is_none() || front {
                    tags.picture = Some(picture);
                    side.front = front;
                }
            }
        }
        _ => {}
    }
}

/// The picture in a FLAC PICTURE block body, and whether it is a front cover.
fn parse_picture(b: &[u8]) -> Option<(bool, Picture)> {
    let u32_at = |at: usize| -> Option<usize> {
        b.get(at..at + 4)
            .map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]) as usize)
    };
    let mime_len = u32_at(4)?;
    let mime = b.get(8..8 + mime_len)?;
    let mut at = 8 + mime_len;
    let desc = u32_at(at)?;
    at += 4 + desc + 16;
    let len = u32_at(at)?;
    let data = b.get(at + 4..at + 4 + len)?;
    if data.is_empty() {
        return None;
    }
    let declared = String::from_utf8_lossy(mime).trim().to_ascii_lowercase();
    let mime = if declared.starts_with("image/") {
        declared
    } else if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png".into()
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg".into()
    } else {
        return None;
    };
    let picture = Picture {
        mime,
        data: data.to_vec(),
    };
    Some((u32_at(0)? == 3, picture))
}

/// Where an Opus source's packets come from: Kiln's own Ogg pages, or
/// another container's demuxer (Matroska and WebM, through Symphonia).
pub(crate) trait OpusDemux: Send {
    /// The next packet, or `None` once the stream has ended.
    fn next_packet(&mut self) -> Result<Option<Vec<u8>>>;
    /// How far into the file the demuxer has read.
    fn bytes_read(&self) -> u64;
    /// Once the stream has ended, the OpusHead of a stream chained after
    /// it, whose audio the next packets are. `None` where nothing follows.
    fn next_part(&mut self) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    /// Where the current stream ends, as its last page's granule (counting
    /// its pre-skip), once that is known. `None` leaves the end to the length
    /// the container states.
    fn part_end(&self) -> Option<u64> {
        None
    }
    /// When the packet `next_packet` last gave starts, in 48 kHz frames on
    /// any fixed clock, for a demuxer that times its packets one by one.
    /// `None` where it doesn't (Ogg gives a granule per page, not per
    /// packet), which leaves gaps in the sound unfilled.
    fn packet_pts(&self) -> Option<u64> {
        None
    }
    /// True once, for the first packet after the demuxer began its packets'
    /// times again (a codec reset in Matroska), which the silence owed must
    /// not be measured across.
    fn restarted(&mut self) -> bool {
        false
    }
}

impl OpusDemux for Packets {
    fn next_packet(&mut self) -> Result<Option<Vec<u8>>> {
        self.next()
    }
    fn bytes_read(&self) -> u64 {
        self.pages.pos
    }
    fn next_part(&mut self) -> Result<Option<Vec<u8>>> {
        Packets::next_part(self)
    }
    fn part_end(&self) -> Option<u64> {
        self.end_granule.map(|g| g.max(0) as u64)
    }
}

struct OpusSource {
    info: SourceInfo,
    packets: Box<dyn OpusDemux>,
    /// The upper-case code that names the file in a "damaged" message.
    code: String,
    dec: *mut o::OpusMSDecoder,
    channels: usize,
    order: Vec<usize>,
    gain: f32,
    /// What the decoder was made for: the stream count, the coupled count
    /// and the mapping. A chained stream must match to follow on.
    shape: (u8, u8, Vec<u8>),
    /// Samples still to drop from the start of the current stream.
    skip: u64,
    /// The current stream's pre-skip.
    pre_skip: u64,
    /// Samples to hand out in all, when the container states it and the
    /// packets don't say where the stream ends.
    limit: Option<u64>,
    emitted: u64,
    /// Samples handed out from the current stream.
    part_emitted: u64,
    /// Decoded, in Kiln's channel order, waiting to be handed out.
    pending: Vec<f32>,
    decoded: Vec<f32>,
    /// Works out the silence owed where packets are missing.
    gaps: GapFill,
    /// Frames libopus has given so far, the pre-skip included.
    decoded_frames: u64,
    /// Silent frames still to go into `pending` before the held packet.
    lead: u64,
    /// The packet that the silence in `lead` goes in front of.
    held: Option<Vec<u8>>,
    got_first: bool,
    skipped: u32,
    total_bytes: u64,
}

// SAFETY: the libopus decoder has no thread affinity, and nothing else holds
// its pointer: it is only used through `&mut self` or by value.
unsafe impl Send for OpusSource {}

/// Opens an Ogg Opus file for reading: Kiln demuxes the pages, libopus decodes
/// at 48 kHz, and the pre-skip and the end trim are honoured, so the length is
/// the length the file states.
pub fn opus_open(mut file: File) -> Result<Box<dyn Source>> {
    let total_bytes = file.metadata()?.len();
    file.seek(SeekFrom::Start(0))?;
    let mut packets = Packets {
        pages: PageReader {
            r: BufReader::with_capacity(64 * 1024, file),
            pos: 0,
        },
        serial: None,
        queue: VecDeque::new(),
        partial: Vec::new(),
        end_granule: None,
        ended: false,
        at_eof: false,
    };

    // The first page of the file must start an Opus stream.
    let head = packets.next()?.ok_or_else(damaged)?;
    let head = parse_head(&head)?;
    let tags = parse_tags(&packets.next()?.ok_or_else(damaged)?)?;

    // The length is the last page's granule less the pre-skip.
    let serial = packets.serial.expect("set by the first page");
    let resume = packets.pages.pos;
    let last = last_granule(packets.pages.r.get_mut(), total_bytes, serial);
    packets.pages.r.seek(SeekFrom::Start(resume))?;
    let frames = last.map(|g| (g.max(0) as u64).saturating_sub(head.pre_skip as u64));

    let kbps = frames
        .filter(|&f| f > 0)
        .map(|f| (total_bytes as f64 * 8.0 * OPUS_RATE as f64 / f as f64 / 1000.0).round() as u32);
    let mut src = opus_source(&head, Box::new(packets), frames, tags, total_bytes, CODE)?;
    src.info.kbps = kbps;
    // Each stream is cut at its own last page's granule instead, as it is
    // read: the length found in the tail belongs to the file's last stream,
    // which in a chained file is not the first.
    src.limit = None;
    Ok(Box::new(src))
}

/// An Opus source over packets from any container. `head` is the stream's
/// OpusHead (in Ogg its first packet; in Matroska the track's CodecPrivate),
/// which gives the channels, the pre-skip, the gain and the channel mapping.
/// `stated` is the length the container states, at 48 kHz and counting the
/// pre-skip (as a Matroska segment's duration does); the decoded audio is
/// cut there, which drops the end padding.
pub(crate) fn opus_from_head(
    head: &[u8],
    packets: Box<dyn OpusDemux>,
    stated: Option<u64>,
    tags: Tags,
    total_bytes: u64,
    code: &str,
) -> Result<Box<dyn Source>> {
    let bad = || MediaError::Damaged { code: code.into() };
    let head = parse_head(head).map_err(|_| bad())?;
    let frames = stated.map(|n| n.saturating_sub(head.pre_skip as u64));
    Ok(Box::new(opus_source(
        &head,
        packets,
        frames,
        tags,
        total_bytes,
        code,
    )?))
}

/// A libopus decoder for `head`'s channels, or `None` when libopus won't
/// make one.
fn opus_decoder(head: &OpusHead) -> Option<*mut o::OpusMSDecoder> {
    let mut error: c_int = 0;
    // SAFETY: the mapping table holds `channels` bytes, and the other
    // arguments are plain numbers and a live local.
    let dec = unsafe {
        o::opus_multistream_decoder_create(
            OPUS_RATE as i32,
            head.channels as c_int,
            head.streams as c_int,
            head.coupled as c_int,
            head.mapping.as_ptr(),
            &mut error,
        )
    };
    (!dec.is_null() && error == o::OPUS_OK).then_some(dec)
}

fn opus_source(
    head: &OpusHead,
    packets: Box<dyn OpusDemux>,
    frames: Option<u64>,
    tags: Tags,
    total_bytes: u64,
    code: &str,
) -> Result<OpusSource> {
    let count = head.channels as u16;
    let dec = opus_decoder(head).ok_or_else(|| MediaError::Damaged { code: code.into() })?;
    Ok(OpusSource {
        info: SourceInfo {
            codec: Codec::Opus,
            rate: OPUS_RATE,
            channels: Channels {
                count,
                mask: default_mask(count),
            },
            bits: None,
            frames,
            kbps: None,
            tags,
        },
        packets,
        code: code.into(),
        dec,
        channels: count as usize,
        order: vorbis_order(Channels {
            count,
            mask: default_mask(count),
        }),
        gain: 10f32.powf(head.gain as f32 / (20.0 * 256.0)),
        shape: (head.streams, head.coupled, head.mapping.clone()),
        skip: head.pre_skip as u64,
        pre_skip: head.pre_skip as u64,
        limit: frames,
        emitted: 0,
        part_emitted: 0,
        pending: Vec::new(),
        decoded: vec![0.0; MAX_PACKET_FRAMES * count as usize],
        gaps: GapFill::new(OPUS_RATE),
        decoded_frames: 0,
        lead: 0,
        held: None,
        got_first: false,
        skipped: 0,
        total_bytes,
    })
}

impl OpusSource {
    /// The next stream of a chained file starts with `head`. With the same
    /// channels laid out the same way it follows on, through a fresh decoder,
    /// with its own pre-skip, gain and end; anything else can't share the
    /// pipeline, which is said rather than ending early.
    fn next_part(&mut self, head: &[u8]) -> Result<()> {
        let head = parse_head(head).map_err(|_| MediaError::Damaged {
            code: self.code.clone(),
        })?;
        let shape = (head.streams, head.coupled, head.mapping.clone());
        if head.channels as usize != self.channels || shape != self.shape {
            return Err(crate::sym::chained());
        }
        let dec = opus_decoder(&head).ok_or_else(|| MediaError::Damaged {
            code: self.code.clone(),
        })?;
        // SAFETY: the old decoder came from the create call, is freed once,
        // and is replaced before anything else uses the pointer.
        unsafe { o::opus_multistream_decoder_destroy(self.dec) };
        self.dec = dec;
        self.gain = 10f32.powf(head.gain as f32 / (20.0 * 256.0));
        self.skip = head.pre_skip as u64;
        self.pre_skip = head.pre_skip as u64;
        self.part_emitted = 0;
        // The new stream's times begin again.
        self.gaps.restart();
        Ok(())
    }

    /// Where the current stream ends, as the samples it may hand out in all:
    /// its own end once its last page is in, else the length the container
    /// states. `None` when neither is known.
    fn end(&self) -> Option<u64> {
        self.packets
            .part_end()
            .map(|g| g.saturating_sub(self.pre_skip))
            .or(self.limit)
    }

    /// Decodes one packet into `pending`, or puts in some of the silence owed
    /// before it. False once the stream has ended.
    fn decode_next(&mut self) -> Result<bool> {
        // Silence for a gap in the packets' times goes in first, a second at
        // a time.
        if self.lead > 0 {
            let n = self.gaps.chunk(self.lead);
            self.lead -= n;
            self.emitted += n;
            self.part_emitted += n;
            let width = self.channels;
            self.pending
                .resize(self.pending.len() + n as usize * width, 0.0);
            return Ok(true);
        }
        let packet = match self.held.take() {
            Some(packet) => packet,
            None => {
                let Some(packet) = self.packets.next_packet()? else {
                    return match self.packets.next_part()? {
                        Some(head) => {
                            self.next_part(&head)?;
                            Ok(true)
                        }
                        None => Ok(false),
                    };
                };
                if self.packets.restarted() {
                    self.gaps.restart();
                }
                if let Some(pts) = self.packets.packet_pts() {
                    let owed = self.gaps.silence_before(pts, self.decoded_frames);
                    // The silence stops where the stream does.
                    let room = self
                        .end()
                        .map_or(u64::MAX, |end| end.saturating_sub(self.part_emitted));
                    self.lead = owed.min(room);
                    if self.lead > 0 {
                        self.held = Some(packet);
                        return Ok(true);
                    }
                }
                packet
            }
        };
        // An empty packet is not a packet: handed to libopus it would count as
        // a lost one and come back as invented audio, so it is a bad one.
        let n = if packet.is_empty() {
            o::OPUS_INVALID_PACKET
        } else {
            // SAFETY: `decoded` holds `MAX_PACKET_FRAMES` frames of this
            // width, and `packet` is a live slice of the length passed.
            unsafe {
                o::opus_multistream_decode_float(
                    self.dec,
                    packet.as_ptr(),
                    packet.len() as i32,
                    self.decoded.as_mut_ptr(),
                    MAX_PACKET_FRAMES as c_int,
                    0,
                )
            }
        };
        if n <= 0 {
            // A packet that can't be used: fatal before the first one, passed
            // over after it, fatal again once too many have gone.
            self.skipped += 1;
            if !self.got_first || self.skipped > MAX_SKIPPED {
                return Err(MediaError::Damaged {
                    code: self.code.clone(),
                });
            }
            // What it held isn't a gap in the timestamps, so it gets no
            // silence: the next packet sets the clock again.
            self.gaps.restart();
            return Ok(true);
        }
        self.got_first = true;
        self.decoded_frames += n as u64;
        let mut frames = n as usize;
        let mut first = 0usize;
        // Drop the encoder's delay from the start.
        let dropped = self.skip.min(frames as u64) as usize;
        self.skip -= dropped as u64;
        first += dropped;
        frames -= dropped;
        // And what the last packet holds past the end: the stream's own end
        // once its last page is in, else the length the container states.
        if let Some(end) = self.end() {
            let room = end.saturating_sub(self.part_emitted);
            if (frames as u64) > room {
                frames = room as usize;
            }
        }
        self.emitted += frames as u64;
        self.part_emitted += frames as u64;
        let width = self.channels;
        let gain = self.gain;
        let start = self.pending.len();
        self.pending.resize(start + frames * width, 0.0);
        let src = &self.decoded[first * width..(first + frames) * width];
        for (out, frame) in self.pending[start..]
            .chunks_exact_mut(width)
            .zip(src.chunks_exact(width))
        {
            for (v, &to) in frame.iter().zip(&self.order) {
                out[to] = v * gain;
            }
        }
        Ok(true)
    }
}

impl Source for OpusSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Block) -> Result<bool> {
        out.channels = self.channels as u16;
        out.rate = OPUS_RATE;
        out.samples.clear();
        let want = BLOCK_FRAMES * self.channels;
        while self.pending.len() < want {
            if !self.decode_next()? {
                break;
            }
        }
        if self.pending.is_empty() {
            return Ok(false);
        }
        let n = want.min(self.pending.len());
        out.samples.extend(self.pending.drain(..n));
        Ok(true)
    }

    fn position(&self) -> Progressed {
        let handed = self.emitted - (self.pending.len() / self.channels) as u64;
        Progressed {
            frames: handed,
            bytes: self.packets.bytes_read().min(self.total_bytes),
            total_bytes: self.total_bytes,
        }
    }
}

impl Drop for OpusSource {
    fn drop(&mut self) {
        // SAFETY: `dec` came from the create call and is freed once.
        unsafe { o::opus_multistream_decoder_destroy(self.dec) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_disk_under_vorbis_is_an_io_error() {
        let full = io::Error::from_raw_os_error(112);
        match vorbis_error(vorbis_rs::VorbisError::Io(full)) {
            MediaError::Io(e) => assert_eq!(e.raw_os_error(), Some(112)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            vorbis_error(vorbis_rs::VorbisError::UnsupportedStreamChaining),
            MediaError::Encoder(_)
        ));
    }

    #[test]
    fn crc_matches_the_ogg_checksum() {
        // The table-driven checksum against the plain bit-by-bit definition.
        fn slow(data: &[u8]) -> u32 {
            let mut crc = 0u32;
            for &b in data {
                crc ^= (b as u32) << 24;
                for _ in 0..8 {
                    crc = if crc & 0x8000_0000 != 0 {
                        (crc << 1) ^ 0x04C1_1DB7
                    } else {
                        crc << 1
                    };
                }
            }
            crc
        }
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        assert_eq!(crc_update(0, &data), slow(&data));
        assert_eq!(crc_update(0, b""), 0);
    }

    #[test]
    fn base64_round_trips_every_length() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        for n in 0..40 {
            assert_eq!(
                base64_decode(&base64_encode(&data[..n])).unwrap(),
                &data[..n]
            );
        }
        assert_eq!(base64_decode("Zm9v\nYg==").unwrap(), b"foob");
        assert!(base64_decode("Zm9v!").is_none());
    }

    fn ch(count: u16, mask: u32) -> Channels {
        Channels { count, mask }
    }

    #[test]
    fn channel_orders_are_permutations() {
        for n in 1..=8u16 {
            let mut seen = vorbis_order(ch(n, default_mask(n)));
            seen.sort_unstable();
            assert_eq!(seen, (0..n as usize).collect::<Vec<_>>(), "{n} channels");
        }
        assert!(vorbis_order(ch(9, 0)).is_empty());
    }

    #[test]
    fn channel_order_follows_the_mask() {
        // 5.1 with back speakers (the default) and with side speakers.
        assert_eq!(vorbis_order(ch(6, 0x3F)), [0, 2, 1, 4, 5, 3]);
        assert_eq!(vorbis_order(ch(6, 0x60F)), [0, 2, 1, 4, 5, 3]);
        // 6.1 as 0x70F (FL FR FC LFE BC SL SR): L C R SL SR BC LFE.
        assert_eq!(vorbis_order(ch(7, 0x70F)), [0, 2, 1, 5, 6, 4, 3]);
        // 6.1 with back left and right and a back centre.
        assert_eq!(vorbis_order(ch(7, 0x13F)), [0, 2, 1, 4, 5, 6, 3]);
        // 7.1.
        assert_eq!(vorbis_order(ch(8, 0x63F)), [0, 2, 1, 6, 7, 4, 5, 3]);
        // A mask of the wrong size, or one with a speaker Vorbis can't place,
        // gives the standard layout for the count.
        assert_eq!(vorbis_order(ch(6, 0)), vorbis_order(ch(6, 0x3F)));
        assert_eq!(vorbis_order(ch(6, 0x7)), vorbis_order(ch(6, 0x3F)));
        assert_eq!(vorbis_order(ch(3, 0x1C1)), vorbis_order(ch(3, 0x7)));
        // Mono and stereo are what they are.
        assert_eq!(vorbis_order(ch(1, 0x1)), [0]);
        assert_eq!(vorbis_order(ch(2, 0x3)), [0, 1]);
    }

    /// Packets in, pages out, packets back: sizes at the lacing edges.
    #[test]
    fn pages_hold_any_packet_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.ogg");
        let sizes = [
            0usize, 1, 254, 255, 256, 509, 510, 511, 65_024, 65_025, 65_026, 200_000, 3,
        ];
        {
            let mut w = PageWriter::new(BufWriter::new(File::create(&path).unwrap()), 7);
            for (i, &n) in sizes.iter().enumerate() {
                let packet: Vec<u8> = (0..n).map(|j| (i + j) as u8).collect();
                w.packet(&packet, i as u64 * 10).unwrap();
                if i % 4 == 0 && i + 1 < sizes.len() {
                    w.flush().unwrap();
                }
            }
            w.finish().unwrap();
        }
        let mut p = Packets {
            pages: PageReader {
                r: BufReader::new(File::open(&path).unwrap()),
                pos: 0,
            },
            serial: None,
            queue: VecDeque::new(),
            partial: Vec::new(),
            end_granule: None,
            ended: false,
            at_eof: false,
        };
        for (i, &n) in sizes.iter().enumerate() {
            let got = p.next().unwrap().expect("a packet");
            let want: Vec<u8> = (0..n).map(|j| (i + j) as u8).collect();
            assert_eq!(got, want, "packet {i} of {n} bytes");
        }
        assert!(p.next().unwrap().is_none());
        assert_eq!(p.end_granule, Some(120));
        assert_eq!(p.serial, Some(7));
    }

    #[test]
    fn free_form_channel_mapping_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.opus");
        {
            let mut w = PageWriter::new(BufWriter::new(File::create(&path).unwrap()), 1);
            let mut head = b"OpusHead".to_vec();
            head.extend_from_slice(&[1, 3, 56, 1]);
            head.extend_from_slice(&48_000u32.to_le_bytes());
            head.extend_from_slice(&[0, 0, 255, 3, 0, 0, 1, 2]);
            w.packet(&head, 0).unwrap();
            w.flush().unwrap();
            w.packet(&opus_tags(&[]), 0).unwrap();
            w.finish().unwrap();
        }
        let r = opus_open(File::open(&path).unwrap());
        assert!(matches!(r, Err(MediaError::Unsupported { .. })));
    }

    #[test]
    fn comment_parsing_takes_what_it_can() {
        let lines = vec![
            ("title".to_string(), "T".to_string()),
            ("TRACKNUMBER".to_string(), "4/9".to_string()),
            ("discnumber".to_string(), "2".to_string()),
            ("DISCTOTAL".to_string(), "3".to_string()),
            ("DATE".to_string(), "2001-05-06".to_string()),
            ("TITLE".to_string(), "second".to_string()),
            ("BAD".to_string(), "".to_string()),
        ];
        let mut p = opus_tags(&lines);
        let t = parse_tags(&p).unwrap();
        assert_eq!(t.title.as_deref(), Some("T"));
        assert_eq!(t.track, Some((4, Some(9))));
        assert_eq!(t.disc, Some((2, Some(3))));
        assert_eq!(t.year.as_deref(), Some("2001-05-06"));
        // A packet cut short keeps the comments before the cut.
        p.truncate(p.len() - 30);
        assert!(parse_tags(&p).unwrap().title.is_some());
        assert!(parse_tags(b"OpusTag").is_err());
    }

    #[test]
    fn a_front_cover_wins_over_an_earlier_picture() {
        fn comment(kind: u32, data: &[u8]) -> (String, String) {
            let mut block = vorbis::picture_block(&Picture {
                mime: "image/png".into(),
                data: data.to_vec(),
            });
            block[..4].copy_from_slice(&kind.to_be_bytes());
            (PICTURE_KEY.to_string(), base64_encode(&block))
        }
        let t = parse_tags(&opus_tags(&[comment(4, b"back"), comment(3, b"front")])).unwrap();
        assert_eq!(t.picture.unwrap().data, b"front");
        let t = parse_tags(&opus_tags(&[comment(4, b"back"), comment(0, b"other")])).unwrap();
        assert_eq!(t.picture.unwrap().data, b"back");
    }

    /// Rewrites an Opus file with `empty` empty packets added after its third
    /// audio packet, keeping every granule.
    fn with_empty_packets(from: &std::path::Path, to: &std::path::Path, empty: usize) {
        let mut p = Packets {
            pages: PageReader {
                r: BufReader::new(File::open(from).unwrap()),
                pos: 0,
            },
            serial: None,
            queue: VecDeque::new(),
            partial: Vec::new(),
            end_granule: None,
            ended: false,
            at_eof: false,
        };
        let mut w = PageWriter::new(BufWriter::new(File::create(to).unwrap()), 9);
        let mut n = 0u64;
        while let Some(packet) = p.next().unwrap() {
            if n < 2 {
                w.packet(&packet, 0).unwrap();
                w.flush().unwrap();
            } else {
                w.packet(&packet, (n - 1) * FRAME as u64).unwrap();
                if n == 4 {
                    for _ in 0..empty {
                        w.packet(&[], (n - 1) * FRAME as u64).unwrap();
                    }
                }
            }
            n += 1;
        }
        let last = p.end_granule.unwrap() as u64;
        w.packet(&[], last).unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn an_empty_packet_is_a_bad_packet_not_a_lost_one() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.opus"), dir.path().join("b.opus"));
        let tags = Tags::default();
        let setting = Setting::Opus {
            kbps: 64,
            speech: false,
        };
        let mut sink = opus_sink(SinkSpec {
            rate: OPUS_RATE,
            channels: Channels { count: 2, mask: 3 },
            tags: &tags,
            setting: &setting,
            source_bits: None,
            out: File::create(&a).unwrap(),
        })
        .unwrap();
        sink.write(&Block {
            channels: 2,
            rate: OPUS_RATE,
            samples: vec![0.1; 2 * 48_000],
        })
        .unwrap();
        sink.finish().unwrap();

        // A few are passed over, and the sound is the same length.
        with_empty_packets(&a, &b, 3);
        let mut src = opus_open(File::open(&b).unwrap()).unwrap();
        let mut got = 0;
        let mut block = Block {
            channels: 0,
            rate: 0,
            samples: Vec::new(),
        };
        while src.read(&mut block).unwrap() {
            got += block.frames();
        }
        assert_eq!(got, 48_000);

        // Too many make the file damaged.
        with_empty_packets(&a, &b, MAX_SKIPPED as usize + 1);
        let mut src = opus_open(File::open(&b).unwrap()).unwrap();
        let r = loop {
            match src.read(&mut block) {
                Ok(true) => {}
                other => break other,
            }
        };
        assert!(matches!(r, Err(MediaError::Damaged { .. })), "{r:?}");
    }

    /// Decodes an Opus file with libopus alone: the channels as the file
    /// keeps them, in Vorbis order, with no reordering.
    fn decode_in_file_order(path: &std::path::Path) -> (usize, Vec<f32>) {
        let mut packets = Packets {
            pages: PageReader {
                r: BufReader::new(File::open(path).unwrap()),
                pos: 0,
            },
            serial: None,
            queue: VecDeque::new(),
            partial: Vec::new(),
            end_granule: None,
            ended: false,
            at_eof: false,
        };
        let head = parse_head(&packets.next().unwrap().unwrap()).unwrap();
        packets.next().unwrap().unwrap();
        let n = head.channels as usize;
        let mut error = 0;
        // SAFETY: the table holds `n` bytes; the decoder is freed below.
        let dec = unsafe {
            o::opus_multistream_decoder_create(
                OPUS_RATE as i32,
                n as c_int,
                head.streams as c_int,
                head.coupled as c_int,
                head.mapping.as_ptr(),
                &mut error,
            )
        };
        assert!(!dec.is_null());
        let mut out = Vec::new();
        let mut pcm = vec![0.0f32; MAX_PACKET_FRAMES * n];
        while let Some(p) = packets.next().unwrap() {
            // SAFETY: `pcm` holds `MAX_PACKET_FRAMES` frames of `n` channels.
            let frames = unsafe {
                o::opus_multistream_decode_float(
                    dec,
                    p.as_ptr(),
                    p.len() as i32,
                    pcm.as_mut_ptr(),
                    MAX_PACKET_FRAMES as c_int,
                    0,
                )
            };
            assert!(frames > 0);
            out.extend_from_slice(&pcm[..frames as usize * n]);
        }
        // SAFETY: created above, freed once.
        unsafe { o::opus_multistream_decoder_destroy(dec) };
        (n, out)
    }

    /// The Opus sink writes the channels in Vorbis order. Read with libopus
    /// alone, a tone in each Kiln channel must sit where the Vorbis layout
    /// puts that speaker (the expectations are written out by hand: Vorbis
    /// order is L C R, then the sides, the backs and the LFE last).
    #[test]
    fn the_opus_sink_writes_vorbis_order() {
        // Kiln channel -> position in the file.
        let cases: [(u16, u32, &[usize]); 5] = [
            (3, 0x7, &[0, 2, 1]),                  // FL FR FC
            (5, 0x37, &[0, 2, 1, 3, 4]),           // FL FR FC BL BR
            (6, 0x3F, &[0, 2, 1, 5, 3, 4]),        // FL FR FC LFE BL BR
            (7, 0x70F, &[0, 2, 1, 6, 5, 3, 4]),    // FL FR FC LFE BC SL SR
            (8, 0x63F, &[0, 2, 1, 7, 5, 6, 3, 4]), // FL FR FC LFE BL BR SL SR
        ];
        let dir = tempfile::tempdir().unwrap();
        let tags = Tags::default();
        for (count, mask, want) in cases {
            let n = count as usize;
            let setting = Setting::Opus {
                kbps: 64 * count as u32,
                speech: false,
            };
            for ch in 0..n {
                let mut samples = vec![0.0f32; 24_000 * n];
                for (i, f) in samples.chunks_mut(n).enumerate() {
                    f[ch] = 0.5 * (i as f32 * 100.0 * std::f32::consts::TAU / 48_000.0).sin();
                }
                let path = dir.path().join(format!("c{count}_{ch}.opus"));
                let mut sink = opus_sink(SinkSpec {
                    rate: OPUS_RATE,
                    channels: Channels { count, mask },
                    tags: &tags,
                    setting: &setting,
                    source_bits: None,
                    out: File::create(&path).unwrap(),
                })
                .unwrap();
                sink.write(&Block {
                    channels: count,
                    rate: OPUS_RATE,
                    samples,
                })
                .unwrap();
                sink.finish().unwrap();
                let (width, got) = decode_in_file_order(&path);
                assert_eq!(width, n);
                let e: Vec<f64> = (0..n)
                    .map(|c| got.chunks(n).map(|f| (f[c] as f64).powi(2)).sum())
                    .collect();
                let total: f64 = e.iter().sum();
                assert!(
                    e[want[ch]] > 0.8 * total,
                    "{count} channels: kiln channel {ch} should be file channel {}: {e:?}",
                    want[ch]
                );
            }
        }
    }
}
