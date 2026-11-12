//! WavPack in and out, through the vendored libwavpack (see `vendor/README.md`).
//!
//! Rust owns the file. Reading hands libwavpack a set of stream callbacks over
//! the [`File`]; writing hands it a block-output callback. The library's own
//! file-opening code isn't compiled in, so no C `fopen` can happen.
//!
//! The FFI declarations are written by hand: only the few functions below are
//! used, and tags are Kiln's own APEv2 code, not libwavpack's.

use crate::error::{MediaError, Result};
use crate::pcm::{default_mask, lossless_bits, quantise_into};
use crate::pipeline::{Progressed, Sink, SinkSpec, Source, BLOCK_FRAMES};
use crate::tags::ape;
use crate::tune::Setting;
use crate::types::{Block, Channels, Codec, SourceInfo, Tags};
use std::ffi::{c_char, c_int, c_void, CStr};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};

/// The extension shown in a "damaged" message.
const CODE: &str = "WV";

// ---------------------------------------------------------------------------
// The C side

#[repr(C)]
struct Context {
    _private: [u8; 0],
}

/// `WavpackConfig`.
#[repr(C)]
struct Config {
    bitrate: f32,
    shaping_weight: f32,
    bits_per_sample: c_int,
    bytes_per_sample: c_int,
    qmode: c_int,
    flags: c_int,
    xmode: c_int,
    num_channels: c_int,
    float_norm_exp: c_int,
    block_samples: i32,
    extra_flags: i32,
    sample_rate: i32,
    channel_mask: i32,
    md5_checksum: [u8; 16],
    md5_read: u8,
    num_tag_strings: c_int,
    tag_strings: *mut *mut c_char,
}

/// `WavpackStreamReader64`.
#[repr(C)]
struct StreamReader {
    read_bytes: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> i32>,
    write_bytes: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> i32>,
    get_pos: Option<unsafe extern "C" fn(*mut c_void) -> i64>,
    set_pos_abs: Option<unsafe extern "C" fn(*mut c_void, i64) -> c_int>,
    set_pos_rel: Option<unsafe extern "C" fn(*mut c_void, i64, c_int) -> c_int>,
    push_back_byte: Option<unsafe extern "C" fn(*mut c_void, c_int) -> c_int>,
    get_length: Option<unsafe extern "C" fn(*mut c_void) -> i64>,
    can_seek: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    truncate_here: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    close: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
}

type BlockOutput = unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> c_int;

extern "C" {
    fn WavpackOpenFileInputEx64(
        reader: *mut StreamReader,
        wv_id: *mut c_void,
        wvc_id: *mut c_void,
        error: *mut c_char,
        flags: c_int,
        norm_offset: c_int,
    ) -> *mut Context;
    fn WavpackOpenFileOutput(
        blockout: BlockOutput,
        wv_id: *mut c_void,
        wvc_id: *mut c_void,
    ) -> *mut Context;
    fn WavpackCloseFile(wpc: *mut Context) -> *mut Context;
    fn WavpackGetErrorMessage(wpc: *mut Context) -> *mut c_char;
    fn WavpackGetMode(wpc: *mut Context) -> c_int;
    fn WavpackGetNumSamples64(wpc: *mut Context) -> i64;
    fn WavpackGetNumErrors(wpc: *mut Context) -> c_int;
    fn WavpackGetSampleRate(wpc: *mut Context) -> u32;
    fn WavpackGetBitsPerSample(wpc: *mut Context) -> c_int;
    fn WavpackGetBytesPerSample(wpc: *mut Context) -> c_int;
    fn WavpackGetNumChannels(wpc: *mut Context) -> c_int;
    fn WavpackGetChannelMask(wpc: *mut Context) -> c_int;
    fn WavpackGetAverageBitrate(wpc: *mut Context, count_wvc: c_int) -> f64;
    fn WavpackUnpackSamples(wpc: *mut Context, buffer: *mut i32, samples: u32) -> u32;
    fn WavpackSetConfiguration64(
        wpc: *mut Context,
        config: *mut Config,
        total_samples: i64,
        chan_ids: *const u8,
    ) -> c_int;
    fn WavpackPackInit(wpc: *mut Context) -> c_int;
    fn WavpackPackSamples(wpc: *mut Context, buffer: *mut i32, count: u32) -> c_int;
    fn WavpackFlushSamples(wpc: *mut Context) -> c_int;
    fn WavpackUpdateNumSamples(wpc: *mut Context, first_block: *mut c_void);
}

const OPEN_NORMALIZE: c_int = 0x10;
const MODE_FLOAT: c_int = 0x8;
const CONFIG_FAST_FLAG: c_int = 0x200;
const CONFIG_HIGH_FLAG: c_int = 0x800;
const CONFIG_VERY_HIGH_FLAG: c_int = 0x1000;

/// libwavpack's message for the last problem on `wpc`.
///
/// # Safety
/// `wpc` must be a live context.
unsafe fn message(wpc: *mut Context) -> String {
    let p = WavpackGetErrorMessage(wpc);
    if p.is_null() {
        return String::new();
    }
    CStr::from_ptr(p).to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Reading

/// The file as libwavpack's stream callbacks see it: only the audio, without
/// a tag after it.
struct ReadState {
    file: BufReader<File>,
    pos: u64,
    /// Where the audio ends.
    len: u64,
    /// The first I/O error met, kept to report after libwavpack fails.
    error: Option<io::Error>,
}

unsafe extern "C" fn rd_read(id: *mut c_void, data: *mut c_void, count: i32) -> i32 {
    let s = &mut *(id as *mut ReadState);
    if count <= 0 {
        return 0;
    }
    let want = (count as u64).min(s.len.saturating_sub(s.pos)) as usize;
    let buf = std::slice::from_raw_parts_mut(data as *mut u8, want);
    let mut got = 0;
    while got < want {
        match s.file.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                s.error.get_or_insert(e);
                break;
            }
        }
    }
    s.pos += got as u64;
    got as i32
}

unsafe extern "C" fn rd_write(_: *mut c_void, _: *mut c_void, _: i32) -> i32 {
    0 // reading only
}

unsafe extern "C" fn rd_pos(id: *mut c_void) -> i64 {
    (*(id as *mut ReadState)).pos as i64
}

unsafe extern "C" fn rd_seek_abs(id: *mut c_void, pos: i64) -> c_int {
    let s = &mut *(id as *mut ReadState);
    if pos < 0 {
        return -1;
    }
    match s.file.seek(SeekFrom::Start(pos as u64)) {
        Ok(p) => {
            s.pos = p;
            0
        }
        Err(e) => {
            s.error.get_or_insert(e);
            -1
        }
    }
}

unsafe extern "C" fn rd_seek_rel(id: *mut c_void, delta: i64, mode: c_int) -> c_int {
    let s = &mut *(id as *mut ReadState);
    // The C library's modes: SEEK_SET 0, SEEK_CUR 1, SEEK_END 2.
    let target = match mode {
        0 => delta,
        1 => s.pos as i64 + delta,
        2 => s.len as i64 + delta,
        _ => return -1,
    };
    rd_seek_abs(id, target)
}

unsafe extern "C" fn rd_push_back(id: *mut c_void, c: c_int) -> c_int {
    let s = &mut *(id as *mut ReadState);
    if s.pos == 0 || s.file.seek_relative(-1).is_err() {
        return -1; // EOF, as ungetc reports a failure
    }
    s.pos -= 1;
    c
}

unsafe extern "C" fn rd_len(id: *mut c_void) -> i64 {
    (*(id as *mut ReadState)).len as i64
}

unsafe extern "C" fn rd_can_seek(_: *mut c_void) -> c_int {
    1
}

unsafe extern "C" fn rd_nothing(_: *mut c_void) -> c_int {
    0
}

/// Opens a WavPack file for reading. Its APEv2 tag, if any, gives the tags.
pub fn open(mut file: File) -> Result<Box<dyn Source>> {
    let total_bytes = file.metadata()?.len();
    let (len, tags) = match ape::locate(&mut file)? {
        Some((start, bytes)) => (start, ape::read(&bytes)),
        None => (total_bytes, Tags::default()),
    };
    file.seek(SeekFrom::Start(0))?;

    let mut state = Box::new(ReadState {
        file: BufReader::with_capacity(256 * 1024, file),
        pos: 0,
        len,
        error: None,
    });
    let mut reader = Box::new(StreamReader {
        read_bytes: Some(rd_read),
        write_bytes: Some(rd_write),
        get_pos: Some(rd_pos),
        set_pos_abs: Some(rd_seek_abs),
        set_pos_rel: Some(rd_seek_rel),
        push_back_byte: Some(rd_push_back),
        get_length: Some(rd_len),
        can_seek: Some(rd_can_seek),
        truncate_here: Some(rd_nothing),
        close: Some(rd_nothing),
    });
    let mut error = [0 as c_char; 128];
    // SAFETY: libwavpack keeps the reader pointer, so the box is kept in the
    // source below; `state` is the callbacks' id and is kept there too.
    let ctx = unsafe {
        WavpackOpenFileInputEx64(
            &mut *reader,
            &mut *state as *mut ReadState as *mut c_void,
            std::ptr::null_mut(),
            error.as_mut_ptr(),
            OPEN_NORMALIZE,
            0,
        )
    };
    if ctx.is_null() {
        if let Some(e) = state.error.take() {
            return Err(MediaError::Io(e));
        }
        return Err(MediaError::Damaged { code: CODE.into() });
    }

    // SAFETY: `ctx` is a live context.
    let (info, float, scale) = unsafe {
        let channels = WavpackGetNumChannels(ctx).max(0) as u16;
        let bits = WavpackGetBitsPerSample(ctx).max(0) as u16;
        let bytes = WavpackGetBytesPerSample(ctx).max(1);
        let float = WavpackGetMode(ctx) & MODE_FLOAT != 0;
        let mask = WavpackGetChannelMask(ctx) as u32;
        let frames = WavpackGetNumSamples64(ctx);
        let kbps = WavpackGetAverageBitrate(ctx, 0);
        let info = SourceInfo {
            codec: Codec::WavPack,
            rate: WavpackGetSampleRate(ctx),
            channels: Channels {
                count: channels,
                mask: if mask.count_ones() == channels as u32 {
                    mask
                } else {
                    default_mask(channels)
                },
            },
            bits: (!float).then_some(bits),
            frames: (frames >= 0).then_some(frames as u64),
            kbps: (kbps > 0.0).then_some((kbps / 1000.0).round() as u32),
            tags,
        };
        (info, float, (1u64 << (bytes * 8 - 1)) as f64)
    };
    if info.channels.count == 0 || info.rate == 0 {
        // SAFETY: closing the live context.
        unsafe { WavpackCloseFile(ctx) };
        return Err(MediaError::Damaged { code: CODE.into() });
    }
    let ints = vec![0i32; BLOCK_FRAMES * info.channels.count as usize];
    Ok(Box::new(WvSource {
        info,
        ctx,
        _reader: reader,
        state,
        total_bytes,
        float,
        scale,
        ints,
        frames_read: 0,
        ended: false,
    }))
}

struct WvSource {
    info: SourceInfo,
    ctx: *mut Context,
    /// Kept alive for libwavpack, which holds a pointer to it.
    _reader: Box<StreamReader>,
    state: Box<ReadState>,
    total_bytes: u64,
    float: bool,
    scale: f64,
    ints: Vec<i32>,
    frames_read: u64,
    ended: bool,
}

// SAFETY: the context and the state behind its callbacks are only touched
// through `&mut self` (or by value, in `Drop`).
unsafe impl Send for WvSource {}

impl WvSource {
    fn damaged() -> MediaError {
        MediaError::Damaged { code: CODE.into() }
    }
}

impl Source for WvSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Block) -> Result<bool> {
        out.channels = self.info.channels.count;
        out.rate = self.info.rate;
        out.samples.clear();
        if self.ended {
            return Ok(false);
        }
        // SAFETY: `ints` has room for BLOCK_FRAMES frames; `ctx` is live.
        let n =
            unsafe { WavpackUnpackSamples(self.ctx, self.ints.as_mut_ptr(), BLOCK_FRAMES as u32) }
                as usize;
        if let Some(e) = self.state.error.take() {
            return Err(MediaError::Io(e));
        }
        // SAFETY: `ctx` is live.
        if unsafe { WavpackGetNumErrors(self.ctx) } > 0 {
            return Err(Self::damaged());
        }
        if n == 0 {
            self.ended = true;
            // The header promised more audio than the file holds.
            if self.info.frames.is_some_and(|f| self.frames_read < f) {
                return Err(Self::damaged());
            }
            return Ok(false);
        }
        let used = n * self.info.channels.count as usize;
        if self.float {
            out.samples
                .extend(self.ints[..used].iter().map(|&v| f32::from_bits(v as u32)));
        } else {
            let scale = self.scale;
            out.samples
                .extend(self.ints[..used].iter().map(|&v| (v as f64 / scale) as f32));
        }
        self.frames_read += n as u64;
        Ok(true)
    }

    fn position(&self) -> Progressed {
        Progressed {
            frames: self.frames_read,
            bytes: self.state.pos,
            total_bytes: self.total_bytes,
        }
    }
}

impl Drop for WvSource {
    fn drop(&mut self) {
        // SAFETY: the context is closed before the reader and state it uses.
        unsafe { WavpackCloseFile(self.ctx) };
    }
}

// ---------------------------------------------------------------------------
// Writing

/// Where libwavpack's blocks go.
struct WriteState {
    out: BufWriter<File>,
    /// The first block, kept to rewrite with the real length at the end.
    first: Option<Vec<u8>>,
    error: Option<io::Error>,
}

unsafe extern "C" fn block_out(id: *mut c_void, data: *mut c_void, count: i32) -> c_int {
    let s = &mut *(id as *mut WriteState);
    if count <= 0 {
        return 1;
    }
    let block = std::slice::from_raw_parts(data as *const u8, count as usize);
    if s.first.is_none() {
        s.first = Some(block.to_vec());
    }
    match s.out.write_all(block) {
        Ok(()) => 1,
        Err(e) => {
            s.error.get_or_insert(e);
            0
        }
    }
}

/// Makes a sink that writes a WavPack file, with an APEv2 tag at the end.
pub fn sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    let Setting::WavPack { mode, bits } = *spec.setting else {
        return Err(MediaError::Encoder(
            "this setting isn't a WavPack mode".into(),
        ));
    };
    let count = spec.channels.count;
    if count == 0 || count > 256 {
        return Err(MediaError::Encoder(format!(
            "WavPack holds 1 to 256 channels, not {count}"
        )));
    }
    // WavPack stores whole bytes: a 12-bit source is written as 16 bits,
    // which holds its samples exactly.
    let container = lossless_bits(bits, spec.source_bits).div_ceil(8) * 8;
    let mask = if spec.channels.mask != 0 {
        spec.channels.mask
    } else {
        default_mask(count)
    };
    let flags = match mode {
        0 => CONFIG_FAST_FLAG,
        1 => 0,
        2 => CONFIG_HIGH_FLAG,
        _ => CONFIG_VERY_HIGH_FLAG,
    };

    let mut state = Box::new(WriteState {
        out: BufWriter::with_capacity(256 * 1024, spec.out),
        first: None,
        error: None,
    });
    // SAFETY: `state` is the callback's id and is kept in the sink.
    let ctx = unsafe {
        WavpackOpenFileOutput(
            block_out,
            &mut *state as *mut WriteState as *mut c_void,
            std::ptr::null_mut(),
        )
    };
    if ctx.is_null() {
        return Err(MediaError::Encoder("libwavpack is out of memory".into()));
    }
    let mut sink = WvSink {
        ctx,
        state,
        channels: count,
        bits: container as u32,
        tag: ape::write(spec.tags),
        ints: Vec::new(),
    };

    // SAFETY: `Config` is plain numbers and a null pointer, all valid as zero.
    let mut config: Config = unsafe { std::mem::zeroed() };
    config.bits_per_sample = container as c_int;
    config.bytes_per_sample = (container / 8) as c_int;
    config.num_channels = count as c_int;
    config.sample_rate = spec.rate as i32;
    config.channel_mask = mask as i32;
    config.flags = flags;
    // SAFETY: `ctx` is live. The length isn't known yet (-1); the first block
    // is rewritten in `finish`.
    let ok = unsafe {
        WavpackSetConfiguration64(ctx, &mut config, -1, std::ptr::null()) != 0
            && WavpackPackInit(ctx) != 0
    };
    if !ok {
        return Err(sink.failure("WavPack can't start"));
    }
    Ok(Box::new(sink))
}

struct WvSink {
    ctx: *mut Context,
    state: Box<WriteState>,
    channels: u16,
    bits: u32,
    /// The APEv2 tag, written after the last block.
    tag: Vec<u8>,
    ints: Vec<i32>,
}

// SAFETY: as for `WvSource`.
unsafe impl Send for WvSink {}

impl WvSink {
    /// The error to report: an I/O error the callback met, else libwavpack's.
    fn failure(&mut self, what: &str) -> MediaError {
        if let Some(e) = self.state.error.take() {
            return MediaError::Io(e);
        }
        // SAFETY: `ctx` is live.
        let why = unsafe { message(self.ctx) };
        if why.is_empty() {
            MediaError::Encoder(what.to_string())
        } else {
            MediaError::Encoder(format!("{what} ({why})"))
        }
    }
}

impl Sink for WvSink {
    fn write(&mut self, b: &Block) -> Result<()> {
        if b.channels != self.channels {
            return Err(MediaError::Encoder(format!(
                "a block of {} channels reached a {}-channel WavPack",
                b.channels, self.channels
            )));
        }
        let frames = b.frames();
        if frames == 0 {
            return Ok(());
        }
        quantise_into(&b.samples, self.bits, &mut self.ints);
        // SAFETY: `ints` holds `frames * channels` samples; `ctx` is live.
        let ok = unsafe { WavpackPackSamples(self.ctx, self.ints.as_mut_ptr(), frames as u32) };
        if ok == 0 {
            return Err(self.failure("WavPack can't take more audio"));
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        // SAFETY: `ctx` is live.
        if unsafe { WavpackFlushSamples(self.ctx) } == 0 {
            return Err(self.failure("WavPack can't finish"));
        }
        self.state.out.flush()?;
        if let Some(e) = self.state.error.take() {
            return Err(MediaError::Io(e));
        }
        // The first block was written before the length was known: put the
        // real length in it, and in the header it carries for a WAV wrapper.
        if let Some(mut first) = self.state.first.take() {
            // SAFETY: `first` is a whole block, as libwavpack handed it out.
            unsafe { WavpackUpdateNumSamples(self.ctx, first.as_mut_ptr() as *mut c_void) };
            let out = &mut self.state.out;
            out.seek(SeekFrom::Start(0))?;
            out.write_all(&first)?;
            out.seek(SeekFrom::End(0))?;
        }
        if !self.tag.is_empty() {
            let tag = std::mem::take(&mut self.tag);
            self.state.out.write_all(&tag)?;
        }
        self.state.out.flush()?;
        Ok(())
    }
}

impl Drop for WvSink {
    fn drop(&mut self) {
        // SAFETY: closing the live context; `state` outlives it.
        unsafe { WavpackCloseFile(self.ctx) };
    }
}
