//! WMA out, through Windows' WMA encoder and Media Foundation's sink writer,
//! into an ASF file on a byte stream over the Rust [`File`].
//!
//! The bitrates on offer are the ones the encoder lists for the rate and
//! channels; [`FALLBACK_KBPS`] stands in where Media Foundation is missing,
//! so tuning works the same off Windows.

use crate::error::Result;
use crate::pipeline::{Sink, SinkSpec};
use crate::types::Picture;

/// The bitrates offered when Media Foundation can't be asked.
pub const FALLBACK_KBPS: [u32; 7] = [48, 64, 80, 96, 128, 160, 192];

/// The constant bitrates (kbps, ascending) Windows' WMA encoder offers at
/// `rate` and `channels`. Empty when it offers none there;
/// [`FALLBACK_KBPS`] when Media Foundation isn't available.
pub fn bitrates(rate: u32, channels: u16) -> Vec<u32> {
    #[cfg(windows)]
    {
        if crate::mf_codecs().wma_enc {
            return win::bitrates(rate, channels);
        }
    }
    let _ = (rate, channels);
    FALLBACK_KBPS.to_vec()
}

/// Makes a sink that writes a WMA file at `Setting::Kbps`, with the tags in
/// the ASF header. Mono or stereo at a rate the encoder lists.
pub fn sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    #[cfg(windows)]
    {
        win::sink(spec)
    }
    #[cfg(not(windows))]
    {
        let _ = spec;
        Err(crate::error::MediaError::NeedsMediaFoundation)
    }
}

/// Bytes per second as Windows lists them (a few over the round number, e.g.
/// 16,002 for 128 kbps), to the nearest kbps.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn kbps_of(avg_bytes_per_second: u32) -> u32 {
    (avg_bytes_per_second as u64 * 8 + 500) as u32 / 1000
}

/// The biggest cover read back from a `WM/Picture`.
const MAX_PICTURE: usize = 64 * 1024 * 1024;

/// A front cover as an ASF `WM/Picture` value: the picture type (3, front
/// cover), the data length (32-bit little-endian), the MIME type and an empty
/// description (both UTF-16LE, NUL-terminated), then the image.
pub(crate) fn wm_picture(p: &Picture) -> Vec<u8> {
    let mut out = vec![3u8];
    out.extend_from_slice(&(p.data.len() as u32).to_le_bytes());
    for c in p.mime.replace('\0', "").encode_utf16().chain([0, 0]) {
        out.extend_from_slice(&c.to_le_bytes());
    }
    out.extend_from_slice(&p.data);
    out
}

/// Reads a `WM/Picture` value. `None` when it doesn't hold together.
pub(crate) fn read_wm_picture(b: &[u8]) -> Option<Picture> {
    let len = u32::from_le_bytes(b.get(1..5)?.try_into().ok()?) as usize;
    let mut at = 5;
    let mut text = || -> Option<String> {
        let mut units = Vec::new();
        loop {
            let u = u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?);
            at += 2;
            if u == 0 {
                return String::from_utf16(&units).ok();
            }
            units.push(u);
        }
    };
    let mime = text()?;
    let _description = text()?;
    if len == 0 || len > MAX_PICTURE {
        return None;
    }
    let data = b.get(at..at.checked_add(len)?)?.to_vec();
    let mime = if mime.trim().to_ascii_lowercase().starts_with("image/") {
        mime.trim().to_ascii_lowercase()
    } else if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png".into()
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg".into()
    } else {
        return None;
    };
    Some(Picture { mime, data })
}

#[cfg(windows)]
mod win {
    use super::{kbps_of, wm_picture};
    use crate::error::{MediaError, Result};
    use crate::mf::com::{self, failed, get_u32, Mf};
    use crate::pipeline::{no_audio, Sink, SinkSpec};
    use crate::tune::Setting;
    use crate::types::{Block, Tags};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use windows::core::{Interface, GUID, PCWSTR};
    use windows::Win32::Media::MediaFoundation::*;

    /// Bitrates already asked for, by rate and channels.
    type Cache = Mutex<Option<HashMap<(u32, u16), Vec<u32>>>>;
    use windows::Win32::System::Com::CoTaskMemAlloc;
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
    use windows::Win32::System::Variant::{VT_BLOB, VT_LPWSTR, VT_UI4};

    /// The encoder's output types for `rate` and `channels`, WMA 9 Standard only.
    fn output_types(rate: u32, channels: u16) -> Vec<IMFMediaType> {
        let Some(t) = com::encoder(&MFAudioFormat_WMAudioV8) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in 0.. {
            // SAFETY: plain enumeration on a transform we own.
            let Ok(mt) = (unsafe { t.GetOutputAvailableType(0, i) }) else {
                break;
            };
            // SAFETY: a plain attribute read.
            let subtype = unsafe { mt.GetGUID(&MF_MT_SUBTYPE) }.unwrap_or(GUID::zeroed());
            if subtype == MFAudioFormat_WMAudioV8
                && get_u32(&mt, &MF_MT_AUDIO_SAMPLES_PER_SECOND) == Some(rate)
                && get_u32(&mt, &MF_MT_AUDIO_NUM_CHANNELS) == Some(channels as u32)
                && get_u32(&mt, &MF_MT_AUDIO_AVG_BYTES_PER_SECOND).is_some()
            {
                out.push(mt);
            }
        }
        out
    }

    pub fn bitrates(rate: u32, channels: u16) -> Vec<u32> {
        static CACHE: Cache = Mutex::new(None);
        let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = cache.as_ref().and_then(|c| c.get(&(rate, channels))) {
            return v.clone();
        }
        let Ok(_mf) = Mf::start() else {
            return super::FALLBACK_KBPS.to_vec();
        };
        let mut list: Vec<u32> = output_types(rate, channels)
            .iter()
            .filter_map(|t| get_u32(t, &MF_MT_AUDIO_AVG_BYTES_PER_SECOND))
            .map(kbps_of)
            .collect();
        list.sort_unstable();
        list.dedup();
        cache
            .get_or_insert_with(HashMap::new)
            .insert((rate, channels), list.clone());
        list
    }

    pub fn sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
        let Setting::Kbps { kbps } = *spec.setting else {
            return Err(MediaError::Encoder(
                "this setting isn't a WMA setting".into(),
            ));
        };
        let (rate, channels) = (spec.rate, spec.channels.count);
        if !crate::mf_codecs().wma_enc {
            return Err(MediaError::NeedsMediaFoundation);
        }
        if channels == 0 || channels > 2 {
            return Err(MediaError::Encoder(format!(
                "WMA holds 1 or 2 channels, not {channels}"
            )));
        }
        let mf = Mf::start()?;
        let out_type = output_types(rate, channels)
            .into_iter()
            .find(|t| get_u32(t, &MF_MT_AUDIO_AVG_BYTES_PER_SECOND).map(kbps_of) == Some(kbps))
            .ok_or_else(|| {
                MediaError::Encoder(format!(
                    "Windows' WMA encoder has no {kbps} kbps for {channels} channels at {rate} Hz"
                ))
            })?;

        let (stream, _) = com::byte_stream(spec.out, true)?;
        // SAFETY: plain Media Foundation calls; every object made here is
        // owned by the writer or by locals that drop at the end.
        let (writer, index, float) = unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 2).map_err(|e| failed("no attributes", e))?;
            let attrs = attrs.ok_or_else(|| MediaError::Encoder("no attributes".into()))?;
            attrs
                .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_ASF)
                .and_then(|_| attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 0))
                .map_err(|e| failed("can't ask for ASF", e))?;
            let writer = MFCreateSinkWriterFromURL(PCWSTR::null(), &stream, &attrs)
                .map_err(|e| failed("can't write ASF", e))?;
            let index = writer
                .AddStream(&out_type)
                .map_err(|e| failed("the ASF writer won't take WMA", e))?;
            let mask = spec.channels.mask;
            let mut float = true;
            if writer
                .SetInputMediaType(index, &com::pcm_type(true, rate, channels, mask)?, None)
                .is_err()
            {
                float = false;
                writer
                    .SetInputMediaType(index, &com::pcm_type(false, rate, channels, mask)?, None)
                    .map_err(|e| failed("the WMA encoder won't take the audio", e))?;
            }
            write_tags(&writer, spec.tags)?;
            writer
                .BeginWriting()
                .map_err(|e| failed("the WMA writer won't start", e))?;
            (writer, index, float)
        };
        Ok(Box::new(WmaSink {
            writer,
            index,
            float,
            rate,
            channels,
            frames: 0,
            bytes: Vec::new(),
            _mf: mf,
        }))
    }

    /// A PROPVARIANT that clears itself.
    struct Prop(PROPVARIANT);

    impl Drop for Prop {
        fn drop(&mut self) {
            // SAFETY: the variant was made by `text` or `uint` below.
            unsafe {
                let _ = PropVariantClear(&mut self.0);
            }
        }
    }

    fn text(s: &str) -> Result<Prop> {
        let wide: Vec<u16> = s.replace('\0', "").encode_utf16().chain([0]).collect();
        // SAFETY: the string goes into CoTaskMem, which PropVariantClear frees.
        unsafe {
            let p = CoTaskMemAlloc(wide.len() * 2) as *mut u16;
            if p.is_null() {
                return Err(MediaError::Encoder("out of memory".into()));
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            let mut v = PROPVARIANT::default();
            (*v.Anonymous.Anonymous).vt = VT_LPWSTR;
            (*v.Anonymous.Anonymous).Anonymous.pwszVal = windows::core::PWSTR(p);
            Ok(Prop(v))
        }
    }

    fn blob(bytes: &[u8]) -> Result<Prop> {
        // SAFETY: the bytes go into CoTaskMem, which PropVariantClear frees.
        unsafe {
            let p = CoTaskMemAlloc(bytes.len().max(1)) as *mut u8;
            if p.is_null() {
                return Err(MediaError::Encoder("out of memory".into()));
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            let mut v = PROPVARIANT::default();
            (*v.Anonymous.Anonymous).vt = VT_BLOB;
            (*v.Anonymous.Anonymous).Anonymous.blob.cbSize = bytes.len() as u32;
            (*v.Anonymous.Anonymous).Anonymous.blob.pBlobData = p;
            Ok(Prop(v))
        }
    }

    fn uint(n: u32) -> Prop {
        let mut v = PROPVARIANT::default();
        // SAFETY: a plain integer variant.
        unsafe {
            (*v.Anonymous.Anonymous).vt = VT_UI4;
            (*v.Anonymous.Anonymous).Anonymous.ulVal = n;
        }
        Prop(v)
    }

    /// Puts `tags` into the ASF header, through the content info's metadata.
    fn write_tags(writer: &IMFSinkWriter, tags: &Tags) -> Result<()> {
        if *tags == Tags::default() {
            return Ok(());
        }
        let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: GetServiceForStream with no service is a QueryInterface on
        // the ASF media sink; on success `p` holds a reference `from_raw`
        // takes over.
        let meta = unsafe {
            writer
                .GetServiceForStream(
                    MF_SINK_WRITER_MEDIASINK.0,
                    &GUID::zeroed(),
                    &IMFMetadataProvider::IID,
                    &mut p,
                )
                .map_err(|e| failed("the ASF writer can't take tags", e))?;
            let provider = IMFMetadataProvider::from_raw(p);
            provider
                .GetMFMetadata(None, 0, 0)
                .map_err(|e| failed("the ASF writer can't take tags", e))?
        };
        let set = |name: &str, v: Prop| -> Result<()> {
            let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
            // SAFETY: the name is NUL-terminated; the value is copied by the call.
            unsafe { meta.SetProperty(PCWSTR(wide.as_ptr()), &v.0) }
                .map_err(|e| failed("the ASF writer refused a tag", e))
        };
        let put = |name: &str, v: &Option<String>| -> Result<()> {
            match v.as_deref().filter(|s| !s.is_empty()) {
                Some(s) => set(name, text(s)?),
                None => Ok(()),
            }
        };
        put("Title", &tags.title)?;
        put("Author", &tags.artist)?;
        put("WM/AlbumTitle", &tags.album)?;
        put("WM/AlbumArtist", &tags.album_artist)?;
        put("WM/Genre", &tags.genre)?;
        put("Description", &tags.comment)?;
        put("WM/Year", &tags.year)?;
        if let Some((n, _)) = tags.track {
            set("WM/TrackNumber", uint(n))?;
        }
        if let Some((n, total)) = tags.disc {
            let s = match total {
                Some(t) => format!("{n}/{t}"),
                None => n.to_string(),
            };
            set("WM/PartOfSet", text(&s)?)?;
        }
        if let Some(p) = &tags.picture {
            set("WM/Picture", blob(&wm_picture(p))?)?;
        }
        Ok(())
    }

    struct WmaSink {
        writer: IMFSinkWriter,
        index: u32,
        float: bool,
        rate: u32,
        channels: u16,
        frames: u64,
        bytes: Vec<u8>,
        // Last, so Media Foundation outlives the writer.
        _mf: Mf,
    }

    // SAFETY: the writer belongs to this sink alone and lives in the
    // multithreaded apartment; every call first joins the calling thread to it.
    unsafe impl Send for WmaSink {}

    impl Sink for WmaSink {
        fn write(&mut self, b: &Block) -> Result<()> {
            com::com();
            if b.rate != self.rate || b.channels != self.channels {
                return Err(MediaError::Encoder(format!(
                    "a block of {} channels at {} Hz came to a {}-channel {} Hz WMA encoder",
                    b.channels, b.rate, self.channels, self.rate
                )));
            }
            if self.float {
                com::float_bytes(&b.samples, &mut self.bytes);
            } else {
                com::i16_bytes(&b.samples, &mut self.bytes);
            }
            if self.bytes.is_empty() {
                return Ok(());
            }
            let frames = b.frames() as u64;
            let s = com::sample(&self.bytes, self.frames, frames, self.rate)?;
            // SAFETY: a sample we own, handed to the writer.
            unsafe { self.writer.WriteSample(self.index, &s) }
                .map_err(|e| failed("the WMA encoder failed", e))?;
            self.frames += frames;
            Ok(())
        }

        fn finish(self: Box<Self>) -> Result<()> {
            com::com();
            // Finalize would fail with MF_E_SINK_NO_SAMPLES_PROCESSED.
            if self.frames == 0 {
                return Err(no_audio("WMA"));
            }
            // SAFETY: a plain call; it writes the ASF header's final values.
            unsafe { self.writer.Finalize() }.map_err(|e| failed("the WMA file won't close", e))
        }
    }

    impl Drop for WmaSink {
        fn drop(&mut self) {
            com::com();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wm_picture_round_trips() {
        let p = Picture {
            mime: "image/png".into(),
            data: vec![0x89, b'P', b'N', b'G', 1, 2, 3],
        };
        let b = wm_picture(&p);
        assert_eq!(b[0], 3);
        assert_eq!(&b[1..5], &7u32.to_le_bytes());
        // "image/png" in UTF-16, NUL, then an empty description's NUL.
        assert_eq!(b.len(), 5 + 2 * 10 + 2 + 7);
        assert_eq!(read_wm_picture(&b), Some(p));
        assert_eq!(read_wm_picture(&b[..b.len() - 1]), None);
        assert_eq!(read_wm_picture(&[]), None);
    }

    #[test]
    fn listed_byte_rates_round_to_the_nearest_kbps() {
        assert_eq!(kbps_of(16_002), 128);
        assert_eq!(kbps_of(40_001), 320);
        assert_eq!(kbps_of(3_998), 32);
        assert_eq!(kbps_of(12_003), 96);
    }
}
