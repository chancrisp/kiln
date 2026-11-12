//! FLAC out, through libFLAC's stream encoder.
//!
//! Rust owns the file: the encoder writes, seeks and tells through callbacks
//! onto the [`File`] in the [`SinkSpec`]. It needs the seek to go back and
//! write the real STREAMINFO (length, MD5, frame sizes) once the audio ends.

use crate::error::{MediaError, Result};
use crate::pcm::{default_mask, lossless_bits, quantise_into};
use crate::pipeline::{Sink, SinkSpec};
use crate::tags::vorbis;
use crate::tune::Setting;
use crate::types::{Block, Channels, Tags};
use libflac_sys as f;
use std::ffi::c_void;
use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};

/// FLAC holds at most this many channels.
const MAX_CHANNELS: u16 = 8;
/// A metadata block's length is 24 bits.
const MAX_BLOCK: usize = 0x00FF_FFFF;

/// What the callbacks write to.
struct Client {
    out: BufWriter<File>,
    /// The first I/O error a callback met, kept to report once libFLAC fails.
    error: Option<io::Error>,
}

unsafe extern "C" fn write_cb(
    _enc: *const f::FLAC__StreamEncoder,
    buffer: *const f::FLAC__byte,
    bytes: usize,
    _samples: u32,
    _current_frame: u32,
    client: *mut c_void,
) -> f::FLAC__StreamEncoderWriteStatus {
    let c = &mut *(client as *mut Client);
    let data = std::slice::from_raw_parts(buffer, bytes);
    match c.out.write_all(data) {
        Ok(()) => f::FLAC__STREAM_ENCODER_WRITE_STATUS_OK,
        Err(e) => {
            c.error.get_or_insert(e);
            f::FLAC__STREAM_ENCODER_WRITE_STATUS_FATAL_ERROR
        }
    }
}

unsafe extern "C" fn seek_cb(
    _enc: *const f::FLAC__StreamEncoder,
    offset: f::FLAC__uint64,
    client: *mut c_void,
) -> f::FLAC__StreamEncoderSeekStatus {
    let c = &mut *(client as *mut Client);
    match c.out.seek(SeekFrom::Start(offset)) {
        Ok(_) => f::FLAC__STREAM_ENCODER_SEEK_STATUS_OK,
        Err(e) => {
            c.error.get_or_insert(e);
            f::FLAC__STREAM_ENCODER_SEEK_STATUS_ERROR
        }
    }
}

unsafe extern "C" fn tell_cb(
    _enc: *const f::FLAC__StreamEncoder,
    offset: *mut f::FLAC__uint64,
    client: *mut c_void,
) -> f::FLAC__StreamEncoderTellStatus {
    let c = &mut *(client as *mut Client);
    match c.out.stream_position() {
        Ok(p) => {
            *offset = p;
            f::FLAC__STREAM_ENCODER_TELL_STATUS_OK
        }
        Err(e) => {
            c.error.get_or_insert(e);
            f::FLAC__STREAM_ENCODER_TELL_STATUS_ERROR
        }
    }
}

/// Makes a sink that writes a FLAC file, with Vorbis comments and a front
/// cover from the spec's tags.
pub fn sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    let Setting::FlacLevel { level, bits } = *spec.setting else {
        return Err(MediaError::Encoder(
            "this setting isn't a FLAC level".into(),
        ));
    };
    let count = spec.channels.count;
    if count == 0 || count > MAX_CHANNELS {
        return Err(MediaError::Encoder(format!(
            "FLAC holds 1 to {MAX_CHANNELS} channels, not {count}"
        )));
    }
    let bits = lossless_bits(bits, spec.source_bits);

    let mut s = FlacSink {
        enc: std::ptr::null_mut(),
        metadata: Vec::new(),
        client: Box::new(Client {
            out: BufWriter::with_capacity(256 * 1024, spec.out),
            error: None,
        }),
        channels: count,
        bits,
        ints: Vec::new(),
    };
    // SAFETY: the encoder is used from this struct only; the metadata objects
    // and the client outlive it (see `Drop`).
    unsafe {
        s.enc = f::FLAC__stream_encoder_new();
        if s.enc.is_null() {
            return Err(MediaError::Encoder("libFLAC is out of memory".into()));
        }
        let ok = f::FLAC__stream_encoder_set_verify(s.enc, 0) != 0
            && f::FLAC__stream_encoder_set_streamable_subset(s.enc, 0) != 0
            && f::FLAC__stream_encoder_set_channels(s.enc, count as u32) != 0
            && f::FLAC__stream_encoder_set_bits_per_sample(s.enc, bits as u32) != 0
            && f::FLAC__stream_encoder_set_sample_rate(s.enc, spec.rate) != 0
            && f::FLAC__stream_encoder_set_compression_level(s.enc, level.min(8) as u32) != 0;
        if !ok {
            return Err(MediaError::Encoder(
                "libFLAC refused the encoder settings".into(),
            ));
        }
        s.add_metadata(spec.tags, spec.channels)?;
        if !s.metadata.is_empty() {
            let n = s.metadata.len() as u32;
            if f::FLAC__stream_encoder_set_metadata(s.enc, s.metadata.as_mut_ptr(), n) == 0 {
                return Err(MediaError::Encoder("libFLAC refused the tags".into()));
            }
        }
        let status = f::FLAC__stream_encoder_init_stream(
            s.enc,
            Some(write_cb),
            Some(seek_cb),
            Some(tell_cb),
            None,
            &mut *s.client as *mut Client as *mut c_void,
        );
        if status != f::FLAC__STREAM_ENCODER_INIT_STATUS_OK {
            return Err(s.failure(&format!("FLAC can't start: {}", init_problem(status))));
        }
    }
    Ok(Box::new(s))
}

struct FlacSink {
    enc: *mut f::FLAC__StreamEncoder,
    /// Owned by this struct; libFLAC only keeps pointers to them.
    metadata: Vec<*mut f::FLAC__StreamMetadata>,
    client: Box<Client>,
    channels: u16,
    bits: u16,
    ints: Vec<i32>,
}

// SAFETY: libFLAC's encoder has no thread affinity, and nothing else holds
// its pointers: it is only ever used through `&mut self` or by value.
unsafe impl Send for FlacSink {}

impl FlacSink {
    /// Builds the Vorbis comment and PICTURE blocks.
    ///
    /// # Safety
    /// `self.enc` must be a live encoder that is not yet initialised.
    unsafe fn add_metadata(&mut self, tags: &Tags, channels: Channels) -> Result<()> {
        let mut lines = vorbis::comments(tags);
        // A layout other than the one FLAC assumes for the count is named.
        let mask = channels.mask;
        if mask != 0 && mask != default_mask(channels.count) {
            lines.push((
                "WAVEFORMATEXTENSIBLE_CHANNEL_MASK".to_string(),
                format!("0x{mask:X}"),
            ));
        }
        if !lines.is_empty() {
            let obj = f::FLAC__metadata_object_new(f::FLAC__METADATA_TYPE_VORBIS_COMMENT);
            if obj.is_null() {
                return Err(MediaError::Encoder("libFLAC is out of memory".into()));
            }
            self.metadata.push(obj);
            for (name, value) in lines {
                let mut line = format!("{name}={value}").into_bytes();
                let entry = f::FLAC__StreamMetadata_VorbisComment_Entry {
                    length: line.len() as u32,
                    entry: line.as_mut_ptr(),
                };
                // `copy` is true: libFLAC keeps its own bytes.
                if f::FLAC__metadata_object_vorbiscomment_append_comment(obj, entry, 1) == 0 {
                    return Err(MediaError::Encoder("libFLAC refused a tag".into()));
                }
            }
        }
        if let Some(p) = &tags.picture {
            let mut raw = vec![6u8, 0, 0, 0];
            raw.extend_from_slice(&vorbis::picture_block(p));
            let body = raw.len() - 4;
            // A cover too big for a block is dropped rather than failing the file.
            if body <= MAX_BLOCK && !p.data.is_empty() {
                raw[1..4].copy_from_slice(&(body as u32).to_be_bytes()[1..]);
                let obj = f::FLAC__metadata_object_set_raw(raw.as_mut_ptr(), raw.len() as u32);
                if !obj.is_null() {
                    self.metadata.push(obj);
                }
            }
        }
        Ok(())
    }

    /// The error to report: an I/O error a callback met, else libFLAC's state.
    fn failure(&mut self, what: &str) -> MediaError {
        if let Some(e) = self.client.error.take() {
            return MediaError::Io(e);
        }
        let state = if self.enc.is_null() {
            0
        } else {
            // SAFETY: `enc` is a live encoder.
            unsafe { f::FLAC__stream_encoder_get_state(self.enc) }
        };
        MediaError::Encoder(format!("{what} ({})", state_name(state)))
    }
}

impl Sink for FlacSink {
    fn write(&mut self, b: &Block) -> Result<()> {
        if b.channels != self.channels {
            return Err(MediaError::Encoder(format!(
                "a block of {} channels reached a {}-channel FLAC",
                b.channels, self.channels
            )));
        }
        let frames = b.frames();
        if frames == 0 {
            return Ok(());
        }
        quantise_into(&b.samples, self.bits as u32, &mut self.ints);
        // SAFETY: `ints` holds `frames * channels` samples; `enc` is live.
        let ok = unsafe {
            f::FLAC__stream_encoder_process_interleaved(self.enc, self.ints.as_ptr(), frames as u32)
        };
        if ok == 0 {
            return Err(self.failure("FLAC can't take more audio"));
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        // SAFETY: `enc` is live; finishing writes the last frame and, through
        // the seek callback, the final STREAMINFO.
        let ok = unsafe { f::FLAC__stream_encoder_finish(self.enc) };
        if ok == 0 {
            return Err(self.failure("FLAC can't finish"));
        }
        self.client.out.flush()?;
        if let Some(e) = self.client.error.take() {
            return Err(MediaError::Io(e));
        }
        Ok(())
    }
}

impl Drop for FlacSink {
    fn drop(&mut self) {
        // SAFETY: the encoder goes first, because deleting an unfinished one
        // finishes it, and that calls back into `client`. The metadata
        // objects go after it, since it holds pointers to them.
        unsafe {
            if !self.enc.is_null() {
                f::FLAC__stream_encoder_delete(self.enc);
                self.enc = std::ptr::null_mut();
            }
            for obj in self.metadata.drain(..) {
                f::FLAC__metadata_object_delete(obj);
            }
        }
    }
}

fn init_problem(status: f::FLAC__StreamEncoderInitStatus) -> &'static str {
    match status {
        1 => "encoder error",
        2 => "unsupported container",
        3 => "invalid callbacks",
        4 => "invalid number of channels",
        5 => "invalid bit depth",
        6 => "invalid sample rate",
        7 => "invalid block size",
        8 => "invalid LPC order",
        9 => "invalid coefficient precision",
        10 => "block size too small for the LPC order",
        11 => "settings not streamable",
        12 => "invalid metadata",
        13 => "already initialised",
        _ => "unknown problem",
    }
}

fn state_name(state: f::FLAC__StreamEncoderState) -> &'static str {
    match state {
        0 => "ok",
        1 => "uninitialised",
        2 => "ogg error",
        3 => "verify decoder error",
        4 => "verify mismatch",
        5 => "client error",
        6 => "I/O error",
        7 => "framing error",
        8 => "memory allocation error",
        _ => "unknown state",
    }
}
