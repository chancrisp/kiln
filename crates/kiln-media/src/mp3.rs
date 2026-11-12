//! MP3 out, through LAME in its own DLL.
//!
//! LAME is LGPL, so it is not linked into Kiln. It is built as `lame_enc.dll`
//! (the `kiln-lame` crate) and loaded here at run time, from the folder the
//! executable is in and nowhere else. A missing DLL, or one without every
//! function this file calls, is [`MediaError::LameMissing`].
//!
//! The sink writes an ID3v2.4 tag first, then LAME's frames, and last goes back
//! to the first frame (LAME reserves it) and writes the Xing or Info tag there,
//! so a VBR file seeks and reports its length correctly.

use crate::error::{MediaError, Result};
use crate::pipeline::{Sink, SinkSpec, BLOCK_FRAMES};
use crate::tags::id3;
use crate::tune::Setting;
use crate::types::Block;
use std::ffi::c_int;
use std::ffi::c_void;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The most an MP3 holds: two channels, at up to 48 kHz. The caller's
/// `Process` brings anything else down to this.
const MAX_CHANNELS: u16 = 2;
const MAX_RATE: u32 = 48_000;

/// LAME's `MPEG_mode` values.
const JOINT_STEREO: c_int = 1;
const MONO: c_int = 3;
/// LAME's `vbr_mode` values.
const VBR_OFF: c_int = 0;
const VBR_ABR: c_int = 3;
const VBR_DEFAULT: c_int = 4;
/// Quality 2 is LAME's "near-best": the slowest settings that still matter.
const QUALITY: c_int = 2;
/// The Xing or Info frame is at most this big (LAME's own front end uses 2,880).
const TAG_BUFFER: usize = 4096;

type Lame = *mut c_void;
type InitFn = unsafe extern "C" fn() -> Lame;
type SetIntFn = unsafe extern "C" fn(Lame, c_int) -> c_int;
type SetFloatFn = unsafe extern "C" fn(Lame, f32) -> c_int;
type SetVoidIntFn = unsafe extern "C" fn(Lame, c_int);
type InitParamsFn = unsafe extern "C" fn(Lame) -> c_int;
type EncodeInterleavedFn = unsafe extern "C" fn(Lame, *const f32, c_int, *mut u8, c_int) -> c_int;
type EncodeFn = unsafe extern "C" fn(Lame, *const f32, *const f32, c_int, *mut u8, c_int) -> c_int;
type FlushFn = unsafe extern "C" fn(Lame, *mut u8, c_int) -> c_int;
type TagFn = unsafe extern "C" fn(Lame, *mut u8, usize) -> usize;
type CloseFn = unsafe extern "C" fn(Lame) -> c_int;
type VersionFn = unsafe extern "C" fn() -> *const std::ffi::c_char;

/// The functions of `lame_enc.dll` that Kiln calls.
struct Api {
    init: InitFn,
    set_in_samplerate: SetIntFn,
    set_num_channels: SetIntFn,
    set_mode: SetIntFn,
    set_quality: SetIntFn,
    set_vbr: SetIntFn,
    set_vbr_quality: SetFloatFn,
    set_brate: SetIntFn,
    set_vbr_mean_bitrate_kbps: SetIntFn,
    set_write_id3tag_automatic: SetVoidIntFn,
    init_params: InitParamsFn,
    encode_interleaved: EncodeInterleavedFn,
    encode_mono: EncodeFn,
    encode_flush: FlushFn,
    get_lametag_frame: TagFn,
    close: CloseFn,
    version: VersionFn,
}

/// The DLL, loaded once and never unloaded.
static API: OnceLock<Api> = OnceLock::new();

/// Loads `lame_enc.dll` from `dir`.
#[cfg(windows)]
fn load(dir: &Path) -> Result<Api> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_APPLICATION_DIR,
        LOAD_LIBRARY_SEARCH_SYSTEM32,
    };

    let path = dir.join("lame_enc.dll");
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call. The flags keep the
    // search for the DLL's own dependencies to this program's folder and
    // System32, never PATH or the current folder.
    let module = unsafe {
        LoadLibraryExW(
            PCWSTR(wide.as_ptr()),
            None,
            LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }
    .map_err(|_| MediaError::LameMissing)?;

    macro_rules! symbol {
        ($name:literal, $ty:ty) => {{
            // SAFETY: the name is NUL-terminated; the DLL exports it with the
            // C signature `$ty` describes.
            match unsafe { GetProcAddress(module, PCSTR(concat!($name, "\0").as_ptr())) } {
                Some(f) => unsafe {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(f)
                },
                None => return Err(MediaError::LameMissing),
            }
        }};
    }
    Ok(Api {
        init: symbol!("lame_init", InitFn),
        set_in_samplerate: symbol!("lame_set_in_samplerate", SetIntFn),
        set_num_channels: symbol!("lame_set_num_channels", SetIntFn),
        set_mode: symbol!("lame_set_mode", SetIntFn),
        set_quality: symbol!("lame_set_quality", SetIntFn),
        set_vbr: symbol!("lame_set_VBR", SetIntFn),
        set_vbr_quality: symbol!("lame_set_VBR_quality", SetFloatFn),
        set_brate: symbol!("lame_set_brate", SetIntFn),
        set_vbr_mean_bitrate_kbps: symbol!("lame_set_VBR_mean_bitrate_kbps", SetIntFn),
        set_write_id3tag_automatic: symbol!("lame_set_write_id3tag_automatic", SetVoidIntFn),
        init_params: symbol!("lame_init_params", InitParamsFn),
        encode_interleaved: symbol!(
            "lame_encode_buffer_interleaved_ieee_float",
            EncodeInterleavedFn
        ),
        encode_mono: symbol!("lame_encode_buffer_ieee_float", EncodeFn),
        encode_flush: symbol!("lame_encode_flush", FlushFn),
        get_lametag_frame: symbol!("lame_get_lametag_frame", TagFn),
        close: symbol!("lame_close", CloseFn),
        version: symbol!("get_lame_version", VersionFn),
    })
}

/// Only Windows has the DLL.
#[cfg(not(windows))]
fn load(_dir: &Path) -> Result<Api> {
    Err(MediaError::LameMissing)
}

/// The folder the running program is in.
fn exe_dir() -> Result<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or(MediaError::LameMissing)
}

/// Where tests send the loader instead: `KILN_LAME_DIR`, or what
/// `with_lame_dir` set for this thread. Not in release builds.
#[cfg(test)]
fn dir_override() -> Option<PathBuf> {
    tests::THREAD_DIR
        .with(|d| d.borrow().clone())
        .or_else(|| std::env::var_os("KILN_LAME_DIR").map(PathBuf::from))
}

#[cfg(not(test))]
fn dir_override() -> Option<PathBuf> {
    None
}

fn api() -> Result<&'static Api> {
    // An override loads afresh each time and is not remembered, so a test that
    // points at an empty folder can't leave the real DLL unreachable for others.
    if let Some(dir) = dir_override() {
        return Ok(Box::leak(Box::new(load(&dir)?)));
    }
    if let Some(a) = API.get() {
        return Ok(a);
    }
    let loaded = load(&exe_dir()?)?;
    Ok(API.get_or_init(|| loaded))
}

/// LAME's version, for example "3.100". `LameMissing` when the DLL can't be loaded.
pub fn lame_version() -> Result<String> {
    let api = api()?;
    // SAFETY: `get_lame_version` returns a static, NUL-terminated string.
    let s = unsafe { std::ffi::CStr::from_ptr((api.version)()) };
    Ok(s.to_string_lossy().into_owned())
}

/// Makes a sink that writes an MP3 file with LAME, with an ID3v2.4 tag when the
/// spec has tags.
///
/// The input is at most 2 channels and 48 kHz: the caller's `Process`
/// downmixes and resamples first, and the sink reports an error for more.
pub fn sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    let count = spec.channels.count;
    if count == 0 || count > MAX_CHANNELS {
        return Err(MediaError::Encoder(format!(
            "MP3 holds 1 or 2 channels, not {count}"
        )));
    }
    if spec.rate == 0 || spec.rate > MAX_RATE {
        return Err(MediaError::Encoder(format!(
            "MP3 goes up to {MAX_RATE} Hz, not {} Hz",
            spec.rate
        )));
    }
    let (vbr, quality, brate) = match *spec.setting {
        Setting::LameVbr { tenths } => (VBR_DEFAULT, Some(tenths as f32 / 10.0), 0),
        Setting::LameCbr { kbps } => (VBR_OFF, None, kbps),
        Setting::LameAbr { kbps } => (VBR_ABR, None, kbps),
        _ => {
            return Err(MediaError::Encoder(
                "this setting isn't an MP3 setting".into(),
            ))
        }
    };
    let api = api()?;

    let mut out = BufWriter::with_capacity(256 * 1024, spec.out);
    let tag = id3::write_v24(spec.tags);
    out.write_all(&tag)?;

    let mut s = Mp3Sink {
        api,
        gfp: std::ptr::null_mut(),
        out,
        audio_start: tag.len() as u64,
        channels: count,
        clamped: Vec::new(),
        coded: Vec::new(),
    };
    // SAFETY: `gfp` comes from `lame_init` and is used from this struct only;
    // `Drop` closes it.
    unsafe {
        s.gfp = (api.init)();
        if s.gfp.is_null() {
            return Err(MediaError::Encoder("LAME is out of memory".into()));
        }
        let gfp = s.gfp;
        let mut ok = (api.set_in_samplerate)(gfp, spec.rate as c_int) == 0
            && (api.set_num_channels)(gfp, count as c_int) == 0
            && (api.set_mode)(gfp, if count == 1 { MONO } else { JOINT_STEREO }) == 0
            && (api.set_quality)(gfp, QUALITY) == 0
            && (api.set_vbr)(gfp, vbr) == 0;
        // Kiln writes the ID3 tag itself.
        (api.set_write_id3tag_automatic)(gfp, 0);
        match quality {
            Some(q) => ok = ok && (api.set_vbr_quality)(gfp, q) == 0,
            None if vbr == VBR_ABR => {
                ok = ok && (api.set_vbr_mean_bitrate_kbps)(gfp, brate as c_int) == 0
            }
            None => ok = ok && (api.set_brate)(gfp, brate as c_int) == 0,
        }
        if !ok {
            return Err(MediaError::Encoder("LAME refused the settings".into()));
        }
        if (api.init_params)(gfp) < 0 {
            return Err(MediaError::Encoder("LAME can't start".into()));
        }
    }
    Ok(Box::new(s))
}

struct Mp3Sink {
    api: &'static Api,
    gfp: Lame,
    out: BufWriter<File>,
    /// Where LAME's first frame starts: just after the ID3 tag.
    audio_start: u64,
    channels: u16,
    /// The block's samples, held to [-1, 1].
    clamped: Vec<f32>,
    /// What LAME wrote for the block.
    coded: Vec<u8>,
}

// SAFETY: the LAME handle belongs to this sink alone and is never shared.
unsafe impl Send for Mp3Sink {}

impl Drop for Mp3Sink {
    fn drop(&mut self) {
        if !self.gfp.is_null() {
            // SAFETY: the handle came from `lame_init` and is closed once.
            unsafe { (self.api.close)(self.gfp) };
        }
    }
}

fn lame_error(code: c_int) -> MediaError {
    let why = match code {
        -1 => "its output buffer was too small",
        -2 => "it ran out of memory",
        -3 => "it wasn't started",
        -4 => "its psycho-acoustic model failed",
        _ => "an unknown fault",
    };
    MediaError::Encoder(format!("LAME failed: {why}"))
}

impl Mp3Sink {
    /// Encodes up to a block of frames from `self.clamped`.
    fn encode(&mut self, from: usize, frames: usize) -> Result<()> {
        let ch = self.channels as usize;
        // LAME's rule: 1.25 bytes a sample, and 7,200 more.
        let room = frames * 5 / 4 + 7200;
        self.coded.resize(room, 0);
        let pcm = self.clamped[from * ch..(from + frames) * ch].as_ptr();
        // SAFETY: `pcm` holds `frames * ch` floats and `coded` has `room`
        // bytes, both live for the call.
        let n = unsafe {
            if ch == 1 {
                (self.api.encode_mono)(
                    self.gfp,
                    pcm,
                    pcm,
                    frames as c_int,
                    self.coded.as_mut_ptr(),
                    room as c_int,
                )
            } else {
                (self.api.encode_interleaved)(
                    self.gfp,
                    pcm,
                    frames as c_int,
                    self.coded.as_mut_ptr(),
                    room as c_int,
                )
            }
        };
        if n < 0 {
            return Err(lame_error(n));
        }
        self.out.write_all(&self.coded[..n as usize])?;
        Ok(())
    }
}

impl Sink for Mp3Sink {
    fn write(&mut self, b: &Block) -> Result<()> {
        if b.channels != self.channels {
            return Err(MediaError::Encoder(format!(
                "a block of {} channels reached an MP3 of {}",
                b.channels, self.channels
            )));
        }
        self.clamped.clear();
        self.clamped.extend(b.samples.iter().map(|&s| {
            if s.is_nan() {
                0.0
            } else {
                s.clamp(-1.0, 1.0)
            }
        }));
        let total = b.frames();
        let mut at = 0;
        while at < total {
            let n = (total - at).min(BLOCK_FRAMES);
            self.encode(at, n)?;
            at += n;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        self.coded.resize(7200, 0);
        // SAFETY: `coded` has the 7,200 bytes LAME's flush needs.
        let n = unsafe { (self.api.encode_flush)(self.gfp, self.coded.as_mut_ptr(), 7200) };
        if n < 0 {
            return Err(lame_error(n));
        }
        self.out.write_all(&self.coded[..n as usize])?;

        // LAME's Xing or Info frame, now that it knows the length and the
        // seek table. It replaces the frame LAME reserved at the start.
        let mut tag = vec![0u8; TAG_BUFFER];
        // SAFETY: `tag` has `TAG_BUFFER` bytes.
        let mut size =
            unsafe { (self.api.get_lametag_frame)(self.gfp, tag.as_mut_ptr(), tag.len()) };
        if size > tag.len() {
            tag.resize(size, 0);
            // SAFETY: as above, with the size LAME asked for.
            size = unsafe { (self.api.get_lametag_frame)(self.gfp, tag.as_mut_ptr(), tag.len()) };
        }
        if size > 0 && size <= tag.len() {
            // Seeking flushes what is buffered first.
            self.out.seek(SeekFrom::Start(self.audio_start))?;
            self.out.write_all(&tag[..size])?;
        }
        self.out.flush()?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::types::{Channels, Tags};
    use std::cell::RefCell;

    thread_local! {
        /// Set by a test to point the loader at another folder, for this thread only.
        pub(super) static THREAD_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    fn spec<'a>(out: File, tags: &'a Tags, setting: &'a Setting) -> SinkSpec<'a> {
        SinkSpec {
            rate: 44100,
            channels: Channels { count: 2, mask: 3 },
            tags,
            setting,
            source_bits: None,
            out,
        }
    }

    #[test]
    fn missing_dll_is_lame_missing() {
        // An empty folder, as KILN_LAME_DIR would name it.
        let dir = tempfile::tempdir().unwrap();
        THREAD_DIR.with(|d| *d.borrow_mut() = Some(dir.path().to_path_buf()));
        let out = tempfile::tempfile().unwrap();
        let r = sink(spec(
            out,
            &Tags::default(),
            &Setting::LameVbr { tenths: 20 },
        ));
        THREAD_DIR.with(|d| *d.borrow_mut() = None);
        assert!(matches!(r, Err(MediaError::LameMissing)));
        assert_eq!(
            MediaError::LameMissing.to_string(),
            "Kiln's MP3 encoder is missing. Reinstall Kiln to fix it."
        );
    }

    #[test]
    fn a_dll_without_the_functions_is_lame_missing() {
        // Any DLL that loads but isn't LAME: kernel32 is always there.
        let system = std::env::var_os("SystemRoot").map(PathBuf::from);
        let Some(system) = system else { return };
        let dir = tempfile::tempdir().unwrap();
        let from = system.join("System32").join("kernel32.dll");
        if std::fs::copy(from, dir.path().join("lame_enc.dll")).is_err() {
            return;
        }
        THREAD_DIR.with(|d| *d.borrow_mut() = Some(dir.path().to_path_buf()));
        let out = tempfile::tempfile().unwrap();
        let r = sink(spec(
            out,
            &Tags::default(),
            &Setting::LameVbr { tenths: 20 },
        ));
        THREAD_DIR.with(|d| *d.borrow_mut() = None);
        assert!(matches!(r, Err(MediaError::LameMissing)));
    }

    #[test]
    fn the_dll_next_to_the_tests_is_lame_3_100() {
        assert_eq!(lame_version().unwrap(), "3.100");
    }
}
