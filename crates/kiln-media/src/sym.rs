//! MP3, AAC-LC, ALAC, FLAC and Vorbis, in their own files and in the MP4,
//! MKV and Ogg containers, read with Symphonia.
//!
//! Symphonia reads from a [`File`] wrapped in a small counter, so progress can
//! fall back to the byte position when a file's length isn't known. Audio
//! comes out as interleaved `f32` blocks of at most 8,192 frames.
//!
//! Opus in Matroska and WebM is demuxed by Symphonia and decoded by libopus
//! (through [`crate::ogg::opus_from_head`]); Opus in Ogg is left to
//! [`crate::ogg::opus_open`], which reads the Ogg pages itself.
//!
//! Symphonia names no codec it can't decode, so for an MP4 or MKV whose sound
//! it can't read, this module looks into the container itself to say what the
//! sound is: DTS or AC3 should be named, not reported as a silent video.

use crate::error::{MediaError, Result};
use crate::gapfill::GapFill;
use crate::pcm::default_mask;
use crate::pipeline::{Progressed, Source, BLOCK_FRAMES};
use crate::tags::id3;
use crate::types::{Block, Channels, Codec, Picture, SourceInfo, Tags};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{
    CodecParameters, CodecType, Decoder, DecoderOptions, CODEC_TYPE_AAC, CODEC_TYPE_AC4,
    CODEC_TYPE_ADPCM_G722, CODEC_TYPE_ADPCM_G726, CODEC_TYPE_ADPCM_G726LE, CODEC_TYPE_ADPCM_IMA_QT,
    CODEC_TYPE_ADPCM_IMA_WAV, CODEC_TYPE_ADPCM_MS, CODEC_TYPE_ALAC, CODEC_TYPE_ATRAC1,
    CODEC_TYPE_ATRAC3, CODEC_TYPE_ATRAC3PLUS, CODEC_TYPE_ATRAC9, CODEC_TYPE_DCA, CODEC_TYPE_EAC3,
    CODEC_TYPE_FLAC, CODEC_TYPE_MONKEYS_AUDIO, CODEC_TYPE_MP1, CODEC_TYPE_MP2, CODEC_TYPE_MP3,
    CODEC_TYPE_MUSEPACK, CODEC_TYPE_NULL, CODEC_TYPE_OPUS, CODEC_TYPE_PCM_ALAW,
    CODEC_TYPE_PCM_F32BE, CODEC_TYPE_PCM_F32BE_PLANAR, CODEC_TYPE_PCM_F32LE,
    CODEC_TYPE_PCM_F32LE_PLANAR, CODEC_TYPE_PCM_F64BE, CODEC_TYPE_PCM_F64BE_PLANAR,
    CODEC_TYPE_PCM_F64LE, CODEC_TYPE_PCM_F64LE_PLANAR, CODEC_TYPE_PCM_MULAW, CODEC_TYPE_PCM_S16BE,
    CODEC_TYPE_PCM_S16BE_PLANAR, CODEC_TYPE_PCM_S16LE, CODEC_TYPE_PCM_S16LE_PLANAR,
    CODEC_TYPE_PCM_S24BE, CODEC_TYPE_PCM_S24BE_PLANAR, CODEC_TYPE_PCM_S24LE,
    CODEC_TYPE_PCM_S24LE_PLANAR, CODEC_TYPE_PCM_S32BE, CODEC_TYPE_PCM_S32BE_PLANAR,
    CODEC_TYPE_PCM_S32LE, CODEC_TYPE_PCM_S32LE_PLANAR, CODEC_TYPE_PCM_S8, CODEC_TYPE_PCM_S8_PLANAR,
    CODEC_TYPE_PCM_U16BE, CODEC_TYPE_PCM_U16BE_PLANAR, CODEC_TYPE_PCM_U16LE,
    CODEC_TYPE_PCM_U16LE_PLANAR, CODEC_TYPE_PCM_U24BE, CODEC_TYPE_PCM_U24BE_PLANAR,
    CODEC_TYPE_PCM_U24LE, CODEC_TYPE_PCM_U24LE_PLANAR, CODEC_TYPE_PCM_U32BE,
    CODEC_TYPE_PCM_U32BE_PLANAR, CODEC_TYPE_PCM_U32LE, CODEC_TYPE_PCM_U32LE_PLANAR,
    CODEC_TYPE_PCM_U8, CODEC_TYPE_PCM_U8_PLANAR, CODEC_TYPE_SPEEX, CODEC_TYPE_TTA,
    CODEC_TYPE_VORBIS, CODEC_TYPE_WAVPACK, CODEC_TYPE_WMA,
};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, Track};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::{
    MetadataOptions, MetadataRevision, StandardTagKey, StandardVisualKey, Value,
};
use symphonia::core::probe::Hint;
use symphonia::core::units::TimeBase;
use symphonia::default::{get_codecs, get_probe};

/// How many undecodable packets are skipped before the file counts as damaged.
const MAX_SKIPPED: u32 = 10;
/// A cover bigger than this is dropped rather than carried around.
const MAX_PICTURE: usize = 64 * 1024 * 1024;
/// How much of an MKV is searched for its codec names.
const MKV_HEAD: usize = 256 * 1024;
/// The biggest MP4 `moov` box that is read to find the sound's codec.
const MAX_MOOV: u64 = 64 * 1024 * 1024;

/// The track [`open`] reads out of a container's `tracks`: the first whose
/// codec Symphonia names. A track it can't name (AC-3, E-AC-3, DTS, TrueHD)
/// has the null codec and is passed over, so a later track is read instead.
fn picked(tracks: &[Track]) -> Option<&Track> {
    tracks
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
}

/// Whether the sound [`open`] reads from the video at `path` (an MP4, MOV,
/// M4V, MKV or WebM) is the first sound track of the container. (The video
/// engine plans the sound it converts from the same read.) It isn't when
/// that track is one Symphonia can't name, since `open` then takes the first
/// it can ([`picked`]). When it can name none, `open` fails and Media
/// Foundation reads the first sound stream, which counts as the first. `None`
/// when the file can't be read as one of those containers.
pub fn takes_first_sound(path: &Path) -> Option<bool> {
    let mut file = File::open(path).ok()?;
    let first = first_sound_track_id(&mut file)?;
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(&ext.to_ascii_lowercase());
    }
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let probed = get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    Some(picked(probed.format.tracks()).is_none_or(|t| t.id == first))
}

/// The id Symphonia gives the first sound track of an MKV, WebM, MP4 or MOV
/// (read from the container's own headers): a Matroska track's number, or an
/// MP4 track's place among the `trak` boxes of the `moov`, counted from 0.
fn first_sound_track_id(file: &mut File) -> Option<u32> {
    let mut magic = [0u8; 4];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut magic).ok()?;
    if magic == [0x1A, 0x45, 0xDF, 0xA3] {
        let headers = crate::mkvread::read_headers(file)?;
        let first = headers
            .tracks
            .iter()
            .find(|t| t.kind == crate::mkvread::TYPE_AUDIO)?;
        return u32::try_from(first.number).ok();
    }
    let moov = crate::mp4read::read_moov_with(file, crate::mp4read::may_start_a_movie)?;
    let first = crate::mp4read::boxes(&moov)
        .filter(|(kind, _)| kind == b"trak")
        .position(|(_, trak)| {
            crate::mp4read::child(trak, b"mdia")
                .and_then(|mdia| crate::mp4read::child(mdia, b"hdlr"))
                .and_then(|hdlr| hdlr.get(8..12))
                == Some(b"soun")
        })?;
    u32::try_from(first).ok()
}

/// Opens an audio file or the sound of a video file for reading.
///
/// `ext_hint` is the file's extension: it helps Symphonia choose a format and
/// names the file in a "damaged" message.
pub fn open(mut file: File, ext_hint: &str) -> Result<Box<dyn Source>> {
    let code = code_of(ext_hint);
    let total = file.metadata()?.len();
    // Looked at before Symphonia starts: a clone of a file shares its position.
    // An ID3v2 tag in front is read by Kiln's own reader, which skips what it
    // can't read, and Symphonia is given the file from the audio on.
    let prefix = id3_prefix(&mut file, total);
    let mp3 = mp3_head(&mut file, prefix.len, total);
    let adts = is_adts(&mut file, prefix.len);
    let matroska = is_matroska(&mut file);
    file.seek(SeekFrom::Start(prefix.len))?;
    // Only used once Symphonia has given up, to say why.
    let mut side = file.try_clone()?;
    let at = Arc::new(AtomicU64::new(prefix.len));
    let mss = MediaSourceStream::new(
        Box::new(Counted {
            file,
            at: at.clone(),
            start: prefix.len,
            len: total,
        }),
        MediaSourceStreamOptions::default(),
    );
    let mut hint = Hint::new();
    if !ext_hint.is_empty() {
        hint.with_extension(&ext_hint.to_ascii_lowercase());
    }
    // Gapless playback trims the audio to the length the file states. When that
    // length is only an estimate (an MP3 with no Xing header), trimming would
    // cut the end off a VBR file, so leave it off.
    let estimated_length = mp3.as_ref().is_some_and(|h| !h.length_header);
    let format_options = FormatOptions {
        enable_gapless: !estimated_length,
        ..FormatOptions::default()
    };
    let mut probed =
        match get_probe().format(&hint, mss, &format_options, &MetadataOptions::default()) {
            Ok(p) => p,
            Err(e) => return Err(probe_failure(e, &mut side, &code)),
        };

    // The tag in front, then whatever the format holds.
    let mut gathered = Gathered {
        tags: prefix.tags,
        ..Gathered::default()
    };
    if let Some(m) = probed.metadata.get() {
        if let Some(rev) = m.current() {
            gathered.add(rev);
        }
    }
    if let Some(rev) = probed.format.metadata().current() {
        gathered.add(rev);
    }
    let tags = gathered.finish();

    let format = probed.format;
    let Some(track) = picked(format.tracks()) else {
        return Err(match sound_of_unknown_codec(&mut side, format.tracks()) {
            Some(codec) => MediaError::Unsupported {
                codec: codec.display_name(),
            },
            None => MediaError::NoAudio,
        });
    };
    let params = track.codec_params.clone();
    let track_id = track.id;
    let single_track = format.tracks().len() == 1;

    let codec = codec_of(params.codec);
    if params.codec == CODEC_TYPE_OPUS && matroska {
        // The track's CodecPrivate is the OpusHead; the segment's duration
        // counts the pre-skip.
        let Some(head) = params.extra_data.as_deref() else {
            return Err(MediaError::Damaged { code });
        };
        let stated = frame_count(&params, crate::ogg::OPUS_RATE);
        let packets = TrackPackets {
            format,
            track_id,
            time_base: params.time_base,
            pts: None,
            restarted: false,
            head: head.to_vec(),
            at,
            code: code.clone(),
        };
        return crate::ogg::opus_from_head(head, Box::new(packets), stated, tags, total, &code);
    }
    if is_he_aac(&params) {
        return Err(MediaError::Unsupported {
            codec: Codec::HeAac.display_name(),
        });
    }
    let decoder = match get_codecs().make(&params, &DecoderOptions::default()) {
        Ok(d) => d,
        Err(SymError::Unsupported(_)) => {
            return Err(MediaError::Unsupported {
                codec: codec.display_name(),
            })
        }
        Err(SymError::IoError(e)) if e.kind() != io::ErrorKind::UnexpectedEof => {
            return Err(e.into())
        }
        Err(_) => return Err(MediaError::Damaged { code }),
    };

    let mut src = SymSource {
        info: SourceInfo {
            codec: codec.clone(),
            rate: params.sample_rate.unwrap_or(0),
            channels: Channels { count: 0, mask: 0 },
            bits: None,
            frames: None,
            kbps: None,
            tags: Tags::default(),
        },
        format,
        decoder,
        track_id,
        sample_buf: None,
        pending: Vec::new(),
        cursor: 0,
        channels: 0,
        code,
        skipped: 0,
        got_first: false,
        ended: false,
        frames_out: 0,
        time_base: params.time_base,
        gaps: GapFill::new(params.sample_rate.unwrap_or(0)),
        decoded_frames: 0,
        lead: 0,
        at,
        total,
    };

    // Decoding the first packet gives the true rate and channels, and any
    // trouble with the audio shows now rather than half-way through a convert.
    if !src.fill()? {
        // No audio at all: a source with nothing in it, if the file says what it is.
        let (Some(rate), Some(ch)) = (params.sample_rate, params.channels) else {
            return Err(src.damaged());
        };
        // As for a first packet: a stream that states no rate or no channels can't be converted.
        if rate == 0 || ch.count() == 0 {
            return Err(src.damaged());
        }
        src.info.rate = rate;
        src.info.channels = channels_of(ch.count() as u16, ch.bits());
        src.channels = ch.count();
        src.ended = true;
    }

    let rate = src.info.rate;
    // Without a Xing, Info or VBRI header Symphonia only estimates an MP3's
    // length from the first frames' bitrate, and does the same for bare AAC.
    // A VBR file makes that up: say none. A CBR file's length follows from
    // its bitrate, as Kiln counts it (every frame is decoded, gapless off).
    let cbr = mp3
        .as_ref()
        .and_then(|h| h.cbr)
        .filter(|_| estimated_length && codec == Codec::Mp3);
    let frames = match cbr {
        Some(c) => Some(c.frames),
        None if (estimated_length && codec == Codec::Mp3) || (adts && codec == Codec::Aac) => None,
        None => frame_count(&params, rate),
    };
    let overhead = if prefix.len > 0 {
        prefix.len
    } else {
        tags.picture.as_ref().map_or(0, |p| p.data.len() as u64)
    };
    src.info.frames = frames;
    src.info.bits = match codec {
        Codec::Pcm { bits, .. } => Some(bits),
        Codec::Flac => params.bits_per_sample.map(|b| b as u16),
        // The bit depth is the sixth byte of ALAC's magic cookie.
        Codec::Alac => params
            .extra_data
            .as_deref()
            .and_then(|e| e.get(5))
            .map(|&b| b as u16),
        _ => None,
    };
    if let (Some(f), true) = (frames, single_track) {
        // Symphonia reports no bitrate, so work it out from the file.
        let secs = f as f64 / rate.max(1) as f64;
        if secs > 0.0 {
            let bytes = total.saturating_sub(overhead);
            let kbps = (bytes as f64 * 8.0 / secs / 1000.0).round();
            if kbps >= 1.0 && kbps < u32::MAX as f64 {
                src.info.kbps = Some(kbps as u32);
            }
        }
    }
    if let Some(c) = cbr {
        // The bitrate every frame states, rather than one worked out from a
        // size that counts any tags at the end.
        src.info.kbps = Some(c.kbps);
    }
    src.info.tags = tags;
    Ok(Box::new(src))
}

/// The tags of an MP4, MOV or MKV as [`open`] reads them, for a file whose
/// sound can't be opened (a silent video, say). Empty when Symphonia can't
/// read the container.
pub(crate) fn container_tags(file: File, ext_hint: &str) -> Tags {
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mss = MediaSourceStream::new(
        Box::new(Counted {
            file,
            at: Arc::new(AtomicU64::new(0)),
            start: 0,
            len,
        }),
        MediaSourceStreamOptions::default(),
    );
    let mut hint = Hint::new();
    if !ext_hint.is_empty() {
        hint.with_extension(&ext_hint.to_ascii_lowercase());
    }
    let probed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        get_probe().format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
    }));
    let Ok(Ok(mut probed)) = probed else {
        return Tags::default();
    };
    let mut gathered = Gathered::default();
    if let Some(m) = probed.metadata.get() {
        if let Some(rev) = m.current() {
            gathered.add(rev);
        }
    }
    if let Some(rev) = probed.format.metadata().current() {
        gathered.add(rev);
    }
    gathered.finish()
}

/// The channel count and speaker mask for a decoded layout. Symphonia's
/// speaker bits follow the WAVE order, so its bits are the mask when they
/// agree with the count.
fn channels_of(count: u16, bits: u32) -> Channels {
    // Mono is the front centre, as in a WAV; Symphonia calls it front left.
    let mask = if count == 1 {
        default_mask(1)
    } else if bits.count_ones() == count as u32 {
        bits
    } else {
        default_mask(count)
    };
    Channels { count, mask }
}

/// The upper-case extension, or "FILE" for none.
fn code_of(ext: &str) -> String {
    let e = ext.trim_start_matches('.');
    if e.is_empty() {
        "FILE".into()
    } else {
        e.to_ascii_uppercase()
    }
}

/// What a failed probe means: WavPack (which Symphonia doesn't know), an
/// I/O error, or a file that isn't audio it can read.
fn probe_failure(e: SymError, file: &mut File, code: &str) -> MediaError {
    if let SymError::IoError(e) = e {
        if e.kind() != io::ErrorKind::UnexpectedEof {
            return e.into();
        }
    }
    let mut magic = [0u8; 4];
    if file.seek(SeekFrom::Start(0)).is_ok()
        && file.read_exact(&mut magic).is_ok()
        && &magic == b"wvpk"
    {
        return MediaError::Unsupported {
            codec: Codec::WavPack.display_name(),
        };
    }
    MediaError::Damaged { code: code.into() }
}

/// A [`File`] that counts how far it has been read, and that starts `start`
/// bytes in: Symphonia sees those bytes as not there. `at` is the position in
/// the file itself.
struct Counted {
    file: File,
    at: Arc<AtomicU64>,
    start: u64,
    len: u64,
}

impl Read for Counted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        self.at.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl Seek for Counted {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let pos = match pos {
            SeekFrom::Start(p) => SeekFrom::Start(p + self.start),
            other => other,
        };
        let abs = self.file.seek(pos)?;
        if abs < self.start {
            self.file.seek(SeekFrom::Start(self.start))?;
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek to before the start of the audio",
            ));
        }
        self.at.store(abs, Ordering::Relaxed);
        Ok(abs - self.start)
    }
}

impl MediaSource for Counted {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len - self.start)
    }
}

struct SymSource {
    info: SourceInfo,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    sample_buf: Option<SampleBuffer<f32>>,
    /// The latest decoded packet, interleaved, and how much of it is used.
    pending: Vec<f32>,
    cursor: usize,
    channels: usize,
    code: String,
    skipped: u32,
    got_first: bool,
    ended: bool,
    frames_out: u64,
    /// The track's time base, which its packets' times count in.
    time_base: Option<TimeBase>,
    /// Works out the silence owed where packets are missing.
    gaps: GapFill,
    /// Frames the decoder has given so far.
    decoded_frames: u64,
    /// Silent frames still to be handed out before `pending`.
    lead: u64,
    at: Arc<AtomicU64>,
    total: u64,
}

impl SymSource {
    fn damaged(&self) -> MediaError {
        MediaError::Damaged {
            code: self.code.clone(),
        }
    }

    /// A packet that can't be used: fatal before the first block, skipped
    /// after it, fatal again once too many have gone. What it held isn't a
    /// gap in the timestamps, so it gets no silence: the next packet sets the
    /// clock again.
    fn bad_packet(&mut self) -> Result<()> {
        if !self.got_first {
            return Err(self.damaged());
        }
        self.gaps.restart();
        self.skipped += 1;
        if self.skipped > MAX_SKIPPED {
            return Err(self.damaged());
        }
        Ok(())
    }

    /// The format reader started a new part (the next stream of a chained
    /// Ogg file): carry on with its track, under a new decoder, when it has
    /// the rate, channels and codec of what came before. Otherwise the file
    /// can't be converted whole, which is said rather than ending early.
    fn next_part(&mut self) -> Result<()> {
        let Some(track) = picked(self.format.tracks()) else {
            return Err(chained());
        };
        let p = &track.codec_params;
        let same = codec_of(p.codec) == self.info.codec
            && p.sample_rate == Some(self.info.rate)
            && p.channels.map(|c| c.count()) == Some(self.channels);
        if self.got_first && !same {
            return Err(chained());
        }
        self.decoder = get_codecs()
            .make(p, &DecoderOptions::default())
            .map_err(|_| self.damaged())?;
        self.track_id = track.id;
        // The new part's times begin again: its first packet sets the clock.
        self.time_base = p.time_base;
        self.gaps.restart();
        Ok(())
    }

    /// Decodes packets until one gives samples, which replace `pending`, and
    /// sets `lead` to the silence that goes in front of them where packets
    /// are missing. False once the audio has ended.
    fn fill(&mut self) -> Result<bool> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                // Symphonia ends a stream with an unexpected end of file.
                Err(SymError::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(false)
                }
                // A chained Ogg stream, or a codec change in Matroska: the
                // next part needs a decoder of its own.
                Err(SymError::ResetRequired) => {
                    self.next_part()?;
                    continue;
                }
                Err(SymError::IoError(e)) => return Err(e.into()),
                Err(_) => {
                    self.bad_packet()?;
                    continue;
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(_) => {
                    self.bad_packet()?;
                    continue;
                }
            };
            if decoded.frames() == 0 {
                continue;
            }
            let spec = *decoded.spec();
            let count = spec.channels.count() as u16;
            // A stream that states no rate or no channels can't be converted.
            if spec.rate == 0 || count == 0 {
                return Err(self.damaged());
            }
            if !self.got_first {
                self.info.rate = spec.rate;
                self.info.channels = channels_of(count, spec.channels.bits());
                self.channels = count as usize;
                self.gaps = GapFill::new(spec.rate);
                self.got_first = true;
            } else if spec.rate != self.info.rate || count != self.info.channels.count {
                // The pipeline is set up for one rate and channel count.
                return Err(MediaError::Damaged {
                    code: self.code.clone(),
                });
            }
            // Sound that isn't there between this packet and the last is
            // silence, counted from the first packet.
            if let Some(pts) = pts_frames(packet.ts(), self.time_base, spec.rate) {
                self.lead = self.gaps.silence_before(pts, self.decoded_frames);
            }
            self.decoded_frames += decoded.frames() as u64;
            let need = decoded.capacity() * self.channels;
            if self.sample_buf.as_ref().is_none_or(|b| b.capacity() < need) {
                self.sample_buf = Some(SampleBuffer::new(decoded.capacity() as u64, spec));
            }
            let buf = self.sample_buf.as_mut().expect("just made");
            buf.copy_interleaved_ref(decoded);
            self.pending.clear();
            self.pending.extend_from_slice(buf.samples());
            self.cursor = 0;
            return Ok(true);
        }
    }
}

impl Source for SymSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Block) -> Result<bool> {
        out.channels = self.info.channels.count;
        out.rate = self.info.rate;
        out.samples.clear();
        let want = BLOCK_FRAMES * self.channels.max(1);
        while out.samples.len() < want {
            if self.lead == 0
                && self.cursor >= self.pending.len()
                && (self.ended || !self.fill()?)
            {
                self.ended = true;
                break;
            }
            if self.lead > 0 {
                let width = self.channels.max(1);
                let room = ((want - out.samples.len()) / width) as u64;
                let n = self.gaps.chunk(self.lead).min(room);
                out.samples
                    .resize(out.samples.len() + n as usize * width, 0.0);
                self.lead -= n;
                continue;
            }
            let take = (self.pending.len() - self.cursor).min(want - out.samples.len());
            out.samples
                .extend_from_slice(&self.pending[self.cursor..self.cursor + take]);
            self.cursor += take;
        }
        let frames = out.samples.len() / self.channels.max(1);
        self.frames_out += frames as u64;
        Ok(frames > 0)
    }

    fn position(&self) -> Progressed {
        Progressed {
            frames: self.frames_out,
            bytes: self.at.load(Ordering::Relaxed).min(self.total),
            total_bytes: self.total,
        }
    }
}

/// The frame count a track states, in sample frames, or `None` when it states
/// none. MKV and some MP4 files count in their own time base, not in frames.
fn frame_count(params: &CodecParameters, rate: u32) -> Option<u64> {
    let n = params.n_frames.filter(|&n| n > 0)?;
    match params.time_base {
        Some(tb) if tb.numer != 1 || tb.denom != rate => {
            let frames = n as f64 * tb.numer as f64 / tb.denom as f64 * rate as f64;
            Some(frames.round() as u64)
        }
        _ => Some(n),
    }
}

/// A packet's time `ts` in its track's time base, as sample frames at `rate`.
/// A track with no time base counts in frames already. `None` for a time base
/// that can't be used.
fn pts_frames(ts: u64, base: Option<TimeBase>, rate: u32) -> Option<u64> {
    let Some(tb) = base else {
        return Some(ts);
    };
    if tb.denom == 0 {
        return None;
    }
    if tb.numer == 1 && tb.denom == rate {
        return Some(ts);
    }
    let scaled = ts as u128 * tb.numer as u128 * rate as u128;
    let denom = tb.denom as u128;
    u64::try_from((scaled + denom / 2) / denom).ok()
}

/// Kiln's name for a Symphonia codec.
fn codec_of(ct: CodecType) -> Codec {
    if let Some((bits, float)) = pcm_of(ct) {
        return Codec::Pcm { bits, float };
    }
    match ct {
        CODEC_TYPE_MP3 => Codec::Mp3,
        CODEC_TYPE_AAC => Codec::Aac,
        CODEC_TYPE_ALAC => Codec::Alac,
        CODEC_TYPE_FLAC => Codec::Flac,
        CODEC_TYPE_VORBIS => Codec::Vorbis,
        CODEC_TYPE_OPUS => Codec::Opus,
        CODEC_TYPE_WMA => Codec::Wma,
        CODEC_TYPE_WAVPACK => Codec::WavPack,
        CODEC_TYPE_DCA => other("dts"),
        CODEC_TYPE_EAC3 => other("eac3"),
        CODEC_TYPE_AC4 => other("ac4"),
        CODEC_TYPE_MP1 => other("mp1"),
        CODEC_TYPE_MP2 => other("mp2"),
        CODEC_TYPE_SPEEX => other("speex"),
        CODEC_TYPE_MUSEPACK => other("musepack"),
        CODEC_TYPE_MONKEYS_AUDIO => other("ape"),
        CODEC_TYPE_TTA => other("tta"),
        CODEC_TYPE_ATRAC1 | CODEC_TYPE_ATRAC3 | CODEC_TYPE_ATRAC3PLUS | CODEC_TYPE_ATRAC9 => {
            other("atrac")
        }
        CODEC_TYPE_ADPCM_G722
        | CODEC_TYPE_ADPCM_G726
        | CODEC_TYPE_ADPCM_G726LE
        | CODEC_TYPE_ADPCM_IMA_QT
        | CODEC_TYPE_ADPCM_IMA_WAV
        | CODEC_TYPE_ADPCM_MS => other("adpcm"),
        // Companded, so not bit-exact PCM.
        CODEC_TYPE_PCM_ALAW => other("alaw"),
        CODEC_TYPE_PCM_MULAW => other("mulaw"),
        // Whatever Symphonia calls it, else nothing to call it by.
        _ => match get_codecs().get_codec(ct) {
            Some(d) => other(d.short_name),
            None => other("unknown"),
        },
    }
}

fn other(name: &str) -> Codec {
    Codec::Other(name.into())
}

/// Bits and float-ness of a PCM codec.
fn pcm_of(ct: CodecType) -> Option<(u16, bool)> {
    Some(match ct {
        CODEC_TYPE_PCM_S8
        | CODEC_TYPE_PCM_S8_PLANAR
        | CODEC_TYPE_PCM_U8
        | CODEC_TYPE_PCM_U8_PLANAR => (8, false),
        CODEC_TYPE_PCM_S16LE
        | CODEC_TYPE_PCM_S16LE_PLANAR
        | CODEC_TYPE_PCM_S16BE
        | CODEC_TYPE_PCM_S16BE_PLANAR
        | CODEC_TYPE_PCM_U16LE
        | CODEC_TYPE_PCM_U16LE_PLANAR
        | CODEC_TYPE_PCM_U16BE
        | CODEC_TYPE_PCM_U16BE_PLANAR => (16, false),
        CODEC_TYPE_PCM_S24LE
        | CODEC_TYPE_PCM_S24LE_PLANAR
        | CODEC_TYPE_PCM_S24BE
        | CODEC_TYPE_PCM_S24BE_PLANAR
        | CODEC_TYPE_PCM_U24LE
        | CODEC_TYPE_PCM_U24LE_PLANAR
        | CODEC_TYPE_PCM_U24BE
        | CODEC_TYPE_PCM_U24BE_PLANAR => (24, false),
        CODEC_TYPE_PCM_S32LE
        | CODEC_TYPE_PCM_S32LE_PLANAR
        | CODEC_TYPE_PCM_S32BE
        | CODEC_TYPE_PCM_S32BE_PLANAR
        | CODEC_TYPE_PCM_U32LE
        | CODEC_TYPE_PCM_U32LE_PLANAR
        | CODEC_TYPE_PCM_U32BE
        | CODEC_TYPE_PCM_U32BE_PLANAR => (32, false),
        CODEC_TYPE_PCM_F32LE
        | CODEC_TYPE_PCM_F32LE_PLANAR
        | CODEC_TYPE_PCM_F32BE
        | CODEC_TYPE_PCM_F32BE_PLANAR => (32, true),
        CODEC_TYPE_PCM_F64LE
        | CODEC_TYPE_PCM_F64LE_PLANAR
        | CODEC_TYPE_PCM_F64BE
        | CODEC_TYPE_PCM_F64BE_PLANAR => (64, true),
        _ => return None,
    })
}

/// True when an AAC track's AudioSpecificConfig says HE-AAC: SBR or PS,
/// signalled by object type 5 or 29 or by a sync extension after AAC-LC.
/// Symphonia decodes only AAC-LC, so these would come out at half the rate or
/// wrong; the open fails with `Unsupported` ("HE-AAC") and the caller turns to
/// Media Foundation.
fn is_he_aac(params: &CodecParameters) -> bool {
    params.codec == CODEC_TYPE_AAC
        && params
            .extra_data
            .as_deref()
            .is_some_and(crate::mp4::is_he_aac)
}

// ---------------------------------------------------------------------------
// Tags

/// Tags gathered from one or more metadata revisions: the first value for
/// each field wins.
#[derive(Default)]
struct Gathered {
    tags: Tags,
    track_total: Option<u32>,
    disc_total: Option<u32>,
}

impl Gathered {
    fn add(&mut self, rev: &MetadataRevision) {
        for tag in rev.tags() {
            let Some(key) = tag.std_key else { continue };
            let t = &mut self.tags;
            match key {
                StandardTagKey::TrackTitle => fill(&mut t.title, &tag.value),
                StandardTagKey::Artist => fill(&mut t.artist, &tag.value),
                StandardTagKey::Album => fill(&mut t.album, &tag.value),
                StandardTagKey::AlbumArtist => fill(&mut t.album_artist, &tag.value),
                StandardTagKey::Genre => fill(&mut t.genre, &tag.value),
                StandardTagKey::Comment => fill(&mut t.comment, &tag.value),
                StandardTagKey::Date | StandardTagKey::ReleaseDate => fill(&mut t.year, &tag.value),
                StandardTagKey::TrackNumber => fill_part(&mut t.track, &tag.value),
                StandardTagKey::DiscNumber => fill_part(&mut t.disc, &tag.value),
                StandardTagKey::TrackTotal => {
                    self.track_total = self.track_total.or_else(|| number(&tag.value))
                }
                StandardTagKey::DiscTotal => {
                    self.disc_total = self.disc_total.or_else(|| number(&tag.value))
                }
                _ => {}
            }
        }
        if self.tags.picture.is_none() {
            self.tags.picture = pick_picture(rev);
        }
    }

    fn finish(mut self) -> Tags {
        if let Some((_, total @ None)) = self.tags.track.as_mut() {
            *total = self.track_total;
        }
        if let Some((_, total @ None)) = self.tags.disc.as_mut() {
            *total = self.disc_total;
        }
        self.tags
    }
}

/// A tag value as text, without padding.
fn text(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s
            .trim_matches(|c: char| c.is_whitespace() || c == '\0')
            .to_string(),
        Value::UnsignedInt(n) => n.to_string(),
        Value::SignedInt(n) => n.to_string(),
        _ => return None,
    };
    (!s.is_empty()).then_some(s)
}

fn fill(slot: &mut Option<String>, v: &Value) {
    if slot.is_none() {
        *slot = text(v);
    }
}

fn number(v: &Value) -> Option<u32> {
    text(v)?.parse().ok()
}

/// "3" or "3/12".
fn fill_part(slot: &mut Option<(u32, Option<u32>)>, v: &Value) {
    if slot.is_some() {
        return;
    }
    let Some(s) = text(v) else { return };
    let mut it = s.splitn(2, '/');
    let Some(n) = it.next().and_then(|n| n.trim().parse().ok()) else {
        return;
    };
    let total = it.next().and_then(|t| t.trim().parse().ok());
    *slot = Some((n, total));
}

/// The front cover, else the first picture.
fn pick_picture(rev: &MetadataRevision) -> Option<Picture> {
    let v = rev
        .visuals()
        .iter()
        .find(|v| v.usage == Some(StandardVisualKey::FrontCover))
        .or_else(|| rev.visuals().first())?;
    if v.data.is_empty() || v.data.len() > MAX_PICTURE {
        return None;
    }
    let declared = v.media_type.trim().to_ascii_lowercase();
    let mime = if declared.starts_with("image/") {
        declared
    } else {
        sniff_mime(&v.data)?.to_string()
    };
    Some(Picture {
        mime,
        data: v.data.to_vec(),
    })
}

fn sniff_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// MP3 files

/// An ID3v2 tag at the start of a file.
struct Prefix {
    /// Bytes the tag takes, 0 when there is none.
    len: u64,
    tags: Tags,
}

/// The biggest tag whose contents are read; a bigger one is only skipped.
const MAX_TAG: u64 = 64 * 1024 * 1024;

/// Finds and reads an ID3v2 tag at the start of a file. A header whose size
/// can't be right counts as no tag at all.
fn id3_prefix(file: &mut File, total: u64) -> Prefix {
    let none = Prefix {
        len: 0,
        tags: Tags::default(),
    };
    let mut head = [0u8; 10];
    if file.seek(SeekFrom::Start(0)).is_err() || file.read_exact(&mut head).is_err() {
        return none;
    }
    if &head[0..3] != b"ID3" || head[6..10].iter().any(|b| b & 0x80 != 0) {
        return none;
    }
    let size = head[6..10]
        .iter()
        .fold(0u64, |acc, &b| (acc << 7) | b as u64);
    let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
    let len = (10 + size + footer).min(total);
    let mut tags = Tags::default();
    if len <= MAX_TAG {
        let mut bytes = vec![0u8; len as usize];
        if file.seek(SeekFrom::Start(0)).is_ok() && file.read_exact(&mut bytes).is_ok() {
            tags = id3::read(&bytes);
        }
    }
    Prefix { len, tags }
}

/// True when the audio starts with an ADTS header: a bare AAC stream.
/// True when the file starts with an EBML header: Matroska or WebM.
fn is_matroska(file: &mut File) -> bool {
    let mut b = [0u8; 4];
    file.seek(SeekFrom::Start(0)).is_ok()
        && file.read_exact(&mut b).is_ok()
        && b == [0x1A, 0x45, 0xDF, 0xA3]
}

/// What a file is when its next part can't follow on from the one before:
/// a chained stream whose rate, channels or codec change.
pub(crate) fn chained() -> MediaError {
    MediaError::Unsupported {
        codec: "a chained stream".into(),
    }
}

/// One track's packets, as Symphonia's demuxer hands them out.
struct TrackPackets {
    format: Box<dyn FormatReader>,
    track_id: u32,
    /// The track's time base, which its packets' times count in.
    time_base: Option<TimeBase>,
    /// When the packet last handed out starts, in 48 kHz frames.
    pts: Option<u64>,
    /// Set by a codec reset, until the next packet is handed out.
    restarted: bool,
    /// The track's OpusHead, which a next part must match to follow on.
    head: Vec<u8>,
    at: Arc<AtomicU64>,
    code: String,
}

impl crate::ogg::OpusDemux for TrackPackets {
    fn next_packet(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            match self.format.next_packet() {
                Ok(p) if p.track_id() == self.track_id => {
                    self.pts = pts_frames(p.ts(), self.time_base, crate::ogg::OPUS_RATE);
                    return Ok(Some(p.data.to_vec()));
                }
                Ok(_) => {}
                // Symphonia ends a stream with an unexpected end of file.
                Err(SymError::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(None)
                }
                // A next part goes on through the same decoder only when its
                // head is the same (rate, channels, mapping and pre-skip).
                Err(SymError::ResetRequired) => {
                    let next = self.format.tracks().iter().find(|t| {
                        t.codec_params.codec == CODEC_TYPE_OPUS
                            && t.codec_params.extra_data.as_deref() == Some(&self.head[..])
                    });
                    match next {
                        Some(t) => {
                            self.track_id = t.id;
                            self.time_base = t.codec_params.time_base;
                            // The new part's times begin again.
                            self.restarted = true;
                        }
                        None => return Err(chained()),
                    }
                }
                Err(SymError::IoError(e)) => return Err(e.into()),
                Err(_) => {
                    return Err(MediaError::Damaged {
                        code: self.code.clone(),
                    })
                }
            }
        }
    }

    fn bytes_read(&self) -> u64 {
        self.at.load(Ordering::Relaxed)
    }

    fn packet_pts(&self) -> Option<u64> {
        self.pts
    }

    fn restarted(&mut self) -> bool {
        std::mem::take(&mut self.restarted)
    }
}

fn is_adts(file: &mut File, start: u64) -> bool {
    let mut b = [0u8; 2];
    file.seek(SeekFrom::Start(start)).is_ok()
        && file.read_exact(&mut b).is_ok()
        // 12 sync bits, then MPEG version, then layer 00 (MP3's layer is not 00)
        && b[0] == 0xFF
        && b[1] & 0xF6 == 0xF0
}

struct Mp3Head {
    /// True when the first frame is a Xing, Info or VBRI header.
    length_header: bool,
    /// With no such header, what a constant bitrate says, when every frame
    /// looked at states the same one.
    cbr: Option<Mp3Cbr>,
}

/// A constant-bitrate MP3's length and bitrate, worked out from its size.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Mp3Cbr {
    /// Samples per channel: every MPEG frame's, as Kiln decodes them all.
    frames: u64,
    kbps: u32,
}

/// One MPEG audio Layer III frame header.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MpegHeader {
    /// 3 MPEG-1, 2 MPEG-2, 0 MPEG-2.5 (the header's own numbering).
    version: u8,
    kbps: u32,
    rate: u32,
    /// Bytes, the padding byte included.
    len: usize,
}

impl MpegHeader {
    /// The header in the four bytes at the start of `b`, when they are one.
    /// Layer III only, and not free-format (which states no bitrate).
    fn parse(b: &[u8]) -> Option<MpegHeader> {
        const MPEG1: [u32; 15] = [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ];
        const MPEG2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
        let b = b.get(..4)?;
        if b[0] != 0xFF || b[1] & 0xE0 != 0xE0 {
            return None;
        }
        let version = (b[1] >> 3) & 3;
        let layer = (b[1] >> 1) & 3;
        let (bitrate, rate_index) = ((b[2] >> 4) as usize, ((b[2] >> 2) & 3) as usize);
        if version == 1 || layer != 1 || bitrate == 0 || bitrate == 15 || rate_index == 3 {
            return None;
        }
        let base = [44_100u32, 48_000, 32_000][rate_index];
        let (kbps, rate) = match version {
            3 => (MPEG1[bitrate], base),
            2 => (MPEG2[bitrate], base / 2),
            _ => (MPEG2[bitrate], base / 4),
        };
        let pad = ((b[2] >> 1) & 1) as usize;
        Some(MpegHeader {
            version,
            kbps,
            rate,
            len: (Self::per_kbps(version) * kbps as u64 / rate as u64) as usize + pad,
        })
    }

    /// A frame's bytes per kbps, times the rate: 144,000 for MPEG-1 (1,152
    /// samples), 72,000 for MPEG-2 and 2.5 (576).
    fn per_kbps(version: u8) -> u64 {
        if version == 3 {
            144_000
        } else {
            72_000
        }
    }

    fn samples(self) -> u64 {
        if self.version == 3 {
            1152
        } else {
            576
        }
    }

    /// The same stream: version and rate. The bitrate may differ (VBR).
    fn same_stream(self, o: MpegHeader) -> bool {
        self.version == o.version && self.rate == o.rate
    }
}

/// Frames walked from the start, and from the middle, to call a file CBR.
const CBR_FRAMES_FROM_START: usize = 32;
const CBR_FRAMES_FROM_MIDDLE: usize = 8;

/// The headers of up to `n` frames that follow each other from `at` in
/// `buf`, each where the one before ends, all of one stream.
fn mpeg_run(buf: &[u8], mut at: usize, n: usize) -> Vec<MpegHeader> {
    let mut run: Vec<MpegHeader> = Vec::new();
    while run.len() < n {
        let Some(h) = buf.get(at..).and_then(MpegHeader::parse) else {
            break;
        };
        if run.first().is_some_and(|f| !f.same_stream(h)) {
            break;
        }
        run.push(h);
        at += h.len;
    }
    run
}

/// Where the audio of a bare MP3 ends: before an ID3v1 tag and an APEv2
/// tag, when it has either.
fn mp3_audio_end(file: &mut File, total: u64) -> u64 {
    let mut end = total;
    let mut tail = [0u8; 128];
    if end >= 128
        && file.seek(SeekFrom::Start(end - 128)).is_ok()
        && file.read_exact(&mut tail).is_ok()
        && tail.starts_with(b"TAG")
    {
        end -= 128;
    }
    let mut footer = [0u8; 32];
    if end >= 32
        && file.seek(SeekFrom::Start(end - 32)).is_ok()
        && file.read_exact(&mut footer).is_ok()
        && footer.starts_with(b"APETAGEX")
    {
        // The size counts the items and this footer; a header, when the
        // flags say there is one, is 32 bytes more.
        let word = |at: usize| {
            u32::from_le_bytes([footer[at], footer[at + 1], footer[at + 2], footer[at + 3]])
        };
        let header = if word(20) & 0x8000_0000 != 0 { 32 } else { 0 };
        end = end.saturating_sub(u64::from(word(12)) + header);
    }
    end
}

/// Reads up to `len` bytes from `from`, stopping at `end`.
fn read_at(file: &mut File, from: u64, len: u64, end: u64) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    file.take(len.min(end.saturating_sub(from)))
        .read_to_end(&mut buf)
        .ok()?;
    Some(buf)
}

/// Whether a bare MP3 with no length header is CBR, and if so its length:
/// the first 32 frames from the audio's start, and 8 from the middle, all
/// state one bitrate. `sync` is where the first frame starts.
fn mp3_cbr(file: &mut File, sync: u64, total: u64) -> Option<Mp3Cbr> {
    let end = mp3_audio_end(file, total);
    let audio = end.checked_sub(sync).filter(|&n| n > 0)?;
    // 32 frames are at most 46 KB (MPEG-1 at 320 kbps and 32 kHz).
    let head = read_at(file, sync, 48 * 1024, end)?;
    let first = mpeg_run(&head, 0, CBR_FRAMES_FROM_START);
    let one = *first.first()?;
    // A shorter run is only enough when it is the whole file.
    let walked: u64 = first.iter().map(|h| h.len as u64).sum();
    if first.len() < CBR_FRAMES_FROM_START && walked + 4 < audio {
        return None;
    }
    if first.iter().any(|h| h.kbps != one.kbps) {
        return None;
    }
    // From the middle: the first sync from which a run of frames follows.
    if audio > walked {
        let mid = read_at(file, sync + audio / 2, 16 * 1024, end)?;
        let run = (0..mid.len().saturating_sub(4))
            .filter(|&i| mid[i] == 0xFF && mid[i + 1] & 0xE0 == 0xE0)
            .map(|i| mpeg_run(&mid, i, CBR_FRAMES_FROM_MIDDLE))
            .find(|r| r.len() == CBR_FRAMES_FROM_MIDDLE && r[0].same_stream(one))?;
        if run.iter().any(|h| h.kbps != one.kbps) {
            return None;
        }
    }
    // A CBR stream pads frames to keep its bytes on the bitrate, so the
    // count is the audio's bytes over a frame's mean length.
    let mean = MpegHeader::per_kbps(one.version) as f64 * one.kbps as f64 / one.rate as f64;
    let count = (audio as f64 / mean).round() as u64;
    Some(Mp3Cbr {
        frames: count * one.samples(),
        kbps: one.kbps,
    })
}

/// Looks at the first frame of a bare MP3 file, `start` bytes in. `None` when
/// the file doesn't begin like one (an MP3 inside MP4 or MKV counts its own
/// length).
fn mp3_head(file: &mut File, start: u64, total: u64) -> Option<Mp3Head> {
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity(4096);
    file.take(4096).read_to_end(&mut buf).ok()?;
    let is_sync = |w: &[u8]| w[0] == 0xFF && w[1] & 0xE0 == 0xE0;
    let sync = if start == 0 {
        // No tag in front: the first frame must be right at the start.
        buf.get(..2).filter(|w| is_sync(w)).map(|_| 0)?
    } else {
        buf.windows(2).position(is_sync)?
    };
    let window = buf.get(sync + 4..)?;
    let window = &window[..window.len().min(40)];
    let length_header = [&b"Xing"[..], b"Info", b"VBRI"]
        .iter()
        .any(|m| window.windows(m.len()).any(|w| w == *m));
    let cbr = if length_header {
        None
    } else {
        mp3_cbr(file, start + sync as u64, total)
    };
    Some(Mp3Head { length_header, cbr })
}

// ---------------------------------------------------------------------------
// Sound in a container whose codec Symphonia doesn't know

/// The codec of the first sound track whose codec Symphonia can't name, found
/// by reading the MKV or MP4 headers. `None` when there is no such track.
fn sound_of_unknown_codec(file: &mut File, tracks: &[Track]) -> Option<Codec> {
    if let Some(ids) = mkv_codec_ids(file) {
        let n = tracks.len();
        let pick = if ids.len() == n {
            tracks
                .iter()
                .zip(&ids)
                .find(|(t, id)| t.codec_params.sample_rate.is_some() && id.starts_with("A_"))
                .map(|(_, id)| id)
        } else {
            ids.iter().find(|id| id.starts_with("A_"))
        };
        return pick.map(|id| mkv_codec(id));
    }
    mp4_sound_entry(file).map(|cc| fourcc_codec(&cc))
}

/// The name of the first sound track's codec, read from the MKV or MP4
/// headers. For saying what a sound is when it can't be opened.
pub(crate) fn sound_codec_name(file: &mut File) -> Option<String> {
    sound_of_unknown_codec(file, &[]).map(|c| c.display_name())
}

/// Every CodecID in the start of an MKV, in track order. `None` when the file
/// isn't an MKV.
fn mkv_codec_ids(file: &mut File) -> Option<Vec<String>> {
    let mut buf = Vec::with_capacity(MKV_HEAD);
    file.seek(SeekFrom::Start(0)).ok()?;
    file.take(MKV_HEAD as u64).read_to_end(&mut buf).ok()?;
    if !buf.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return None;
    }
    // A CodecID element: id 0x86, a one-byte size, then the name.
    let mut ids = Vec::new();
    let mut i = 0;
    while i + 2 < buf.len() {
        if buf[i] == 0x86 && buf[i + 1] & 0x80 != 0 {
            let n = (buf[i + 1] & 0x7F) as usize;
            if let Some(name) = buf.get(i + 2..i + 2 + n) {
                let ok = n >= 3
                    && matches!(name[0], b'A' | b'V' | b'S')
                    && name[1] == b'_'
                    && name.iter().all(|b| (0x20..0x7F).contains(b));
                if ok {
                    ids.push(String::from_utf8_lossy(name).into_owned());
                    i += 2 + n;
                    continue;
                }
            }
        }
        i += 1;
    }
    Some(ids)
}

fn mkv_codec(id: &str) -> Codec {
    let name = id.trim_start_matches("A_");
    let family = name.split('/').next().unwrap_or(name);
    match family {
        "AC3" => Codec::Ac3,
        "EAC3" => other("eac3"),
        "DTS" => other("dts"),
        "TRUEHD" => other("truehd"),
        "WAVPACK4" => Codec::WavPack,
        "OPUS" => Codec::Opus,
        _ => other(&family.to_ascii_lowercase()),
    }
}

/// The sample entry type of the first `soun` track in an MP4 or MOV.
fn mp4_sound_entry(file: &mut File) -> Option<[u8; 4]> {
    let len = file.metadata().ok()?.len();
    let mut pos = 0u64;
    while pos.checked_add(8).is_some_and(|end| end <= len) {
        file.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        let got = file.take(16).read(&mut h).ok()?;
        if got < 8 {
            return None;
        }
        let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let (size, header) = match size32 {
            0 => (len - pos, 8),
            1 if got >= 16 => (u64::from_be_bytes(h[8..16].try_into().ok()?), 16),
            1 => return None,
            n => (n, 8),
        };
        if size < header {
            return None;
        }
        if &h[4..8] == b"moov" {
            let body = size - header;
            if body > MAX_MOOV || size > len - pos {
                return None;
            }
            let mut moov = vec![0u8; body as usize];
            file.seek(SeekFrom::Start(pos + header)).ok()?;
            file.read_exact(&mut moov).ok()?;
            return sound_entry_in_moov(&moov);
        }
        pos = pos.checked_add(size)?;
    }
    None
}

/// The boxes at one level of an MP4: type and body.
fn boxes(mut buf: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    while buf.len() >= 8 {
        let size32 = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let kind = [buf[4], buf[5], buf[6], buf[7]];
        let (size, header) = match size32 {
            0 => (buf.len(), 8),
            1 if buf.len() >= 16 => {
                let big = u64::from_be_bytes(buf[8..16].try_into().expect("8 bytes"));
                (usize::try_from(big).unwrap_or(usize::MAX), 16)
            }
            1 => break,
            n => (n, 8),
        };
        if size < header || size > buf.len() {
            break;
        }
        out.push((kind, &buf[header..size]));
        buf = &buf[size..];
    }
    out
}

fn child<'a>(buf: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(buf)
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, b)| b)
}

fn sound_entry_in_moov(moov: &[u8]) -> Option<[u8; 4]> {
    for (kind, trak) in boxes(moov) {
        if &kind != b"trak" {
            continue;
        }
        let Some(mdia) = child(trak, b"mdia") else {
            continue;
        };
        // version and flags, pre-defined, then the handler type
        let is_sound = child(mdia, b"hdlr").is_some_and(|h| h.len() >= 12 && &h[8..12] == b"soun");
        if !is_sound {
            continue;
        }
        let stsd = child(child(child(mdia, b"minf")?, b"stbl")?, b"stsd")?;
        // version and flags, entry count, then the first entry: size, type
        if stsd.len() >= 16 {
            return Some([stsd[12], stsd[13], stsd[14], stsd[15]]);
        }
    }
    None
}

fn fourcc_codec(cc: &[u8; 4]) -> Codec {
    let name: String = cc.iter().map(|&b| b as char).collect();
    match name.to_ascii_lowercase().as_str() {
        "ac-3" => Codec::Ac3,
        "ec-3" => other("eac3"),
        "dtsc" | "dtsh" | "dtsl" | "dtse" | "dtsx" => other("dts"),
        "mlpa" => other("truehd"),
        "samr" | "sawb" => other("amr"),
        n => other(n.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn he_aac_is_spotted_in_the_audio_specific_config() {
        let aac = |extra: &[u8]| {
            let mut p = CodecParameters::new();
            p.for_codec(CODEC_TYPE_AAC).with_extra_data(extra.into());
            p
        };
        // object type 2 (LC), 44.1 kHz, stereo
        assert!(!is_he_aac(&aac(&[0x12, 0x10])));
        // object type 5 (SBR)
        assert!(is_he_aac(&aac(&[0x2B, 0x92, 0x08, 0x00])));
        // object type 29 (SBR and PS)
        assert!(is_he_aac(&aac(&[0xEB, 0x92])));
        // LC with a backward-compatible SBR sync extension (0x2B7, type 5, on)
        assert!(is_he_aac(&aac(&[0x13, 0x10, 0x56, 0xE5, 0x98])));
        // the escape (31, then 6 bits) is not HE-AAC
        assert!(!is_he_aac(&aac(&[0xF8, 0x20])));
        assert!(!is_he_aac(&aac(&[])));
        // not AAC at all
        let mut mp3 = aac(&[0x2B, 0x92, 0x08, 0x00]);
        mp3.for_codec(CODEC_TYPE_MP3);
        assert!(!is_he_aac(&mp3));
    }

    #[test]
    fn codec_names() {
        assert_eq!(codec_of(CODEC_TYPE_MP3), Codec::Mp3);
        assert_eq!(codec_of(CODEC_TYPE_DCA).display_name(), "DTS");
        assert_eq!(codec_of(CODEC_TYPE_EAC3), other("eac3"));
        assert_eq!(codec_of(CODEC_TYPE_OPUS).display_name(), "Opus");
        assert_eq!(
            codec_of(CODEC_TYPE_PCM_S24LE),
            Codec::Pcm {
                bits: 24,
                float: false
            }
        );
        assert_eq!(
            codec_of(CODEC_TYPE_PCM_F32LE),
            Codec::Pcm {
                bits: 32,
                float: true
            }
        );
        assert_eq!(mkv_codec("A_DTS/EXPRESS").display_name(), "DTS");
        assert_eq!(mkv_codec("A_TRUEHD").display_name(), "TrueHD");
        assert_eq!(mkv_codec("A_AC3"), Codec::Ac3);
        assert_eq!(codec_of(CODEC_TYPE_PCM_ALAW), other("alaw"));
        assert_eq!(codec_of(CODEC_TYPE_PCM_MULAW), other("mulaw"));
        // A type nobody registered has no name to give.
        assert_eq!(
            codec_of(symphonia::core::codecs::decl_codec_type(b"zzz")),
            other("unknown")
        );
        assert_eq!(fourcc_codec(b"ec-3"), other("eac3"));
        assert_eq!(fourcc_codec(b"dtsc"), other("dts"));
        assert_eq!(fourcc_codec(b"mlpa").display_name(), "TrueHD");
    }

    #[test]
    fn pictures_are_sniffed_by_their_first_bytes() {
        assert_eq!(sniff_mime(&[0x89, b'P', b'N', b'G', 13]), Some("image/png"));
        assert_eq!(sniff_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_mime(b"GIF89a"), None);
    }

    #[test]
    fn track_numbers_split_on_the_slash() {
        let mut slot = None;
        fill_part(&mut slot, &Value::String("3/12".into()));
        assert_eq!(slot, Some((3, Some(12))));
        let mut slot = None;
        fill_part(&mut slot, &Value::String(" 7 ".into()));
        assert_eq!(slot, Some((7, None)));
        let mut slot = None;
        fill_part(&mut slot, &Value::UnsignedInt(5));
        assert_eq!(slot, Some((5, None)));
        let mut slot = None;
        fill_part(&mut slot, &Value::String("x".into()));
        assert_eq!(slot, None);
    }

    #[test]
    fn the_sound_entry_is_found_in_a_moov() {
        fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(body);
            v
        }
        let video_hdlr = bx(b"hdlr", &[&[0u8; 8][..], b"vide", &[0; 12]].concat());
        let sound_hdlr = bx(b"hdlr", &[&[0u8; 8][..], b"soun", &[0; 12]].concat());
        let entry = bx(b"dtsc", &[0; 20]);
        let stsd = bx(b"stsd", &[&[0u8, 0, 0, 0, 0, 0, 0, 1][..], &entry].concat());
        let minf = bx(b"minf", &bx(b"stbl", &stsd));
        let video = bx(b"trak", &bx(b"mdia", &[video_hdlr].concat()));
        let sound = bx(b"trak", &bx(b"mdia", &[sound_hdlr, minf].concat()));
        let moov = [video, sound].concat();
        assert_eq!(sound_entry_in_moov(&moov), Some(*b"dtsc"));
        assert_eq!(sound_entry_in_moov(&moov[..20]), None);
    }
}
