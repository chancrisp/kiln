//! Reading through Media Foundation: WMA (ASF), AC3, and HE-AAC in MP4 or
//! ADTS, which Symphonia can't decode.
//!
//! The reader is an `IMFSourceReader` over a byte stream on the Rust [`File`]
//! (Media Foundation never sees a path), asked for interleaved float PCM.
//! Tags come from the source's property store and its `IMFMetadata`.

use crate::error::Result;
use crate::pipeline::Source;
use std::fs::File;

/// Opens `file` with Media Foundation. `NeedsMediaFoundation` when Windows
/// has no Media Foundation, or not the decoder this file needs.
#[cfg(windows)]
pub fn open(file: File) -> Result<Box<dyn Source>> {
    win::open(file)
}

/// Only Windows has Media Foundation.
#[cfg(not(windows))]
pub fn open(_file: File) -> Result<Box<dyn Source>> {
    Err(crate::error::MediaError::NeedsMediaFoundation)
}

#[cfg(windows)]
mod win {
    use crate::error::{MediaError, Result};
    use crate::mf::com::{self, get_blob, get_u32, Mf};
    use crate::pipeline::{Progressed, Source, BLOCK_FRAMES};
    use crate::types::{default_mask, Block, Channels, Codec, SourceInfo, Tags};
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use windows::core::{Interface, GUID, PCWSTR, PWSTR};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::Storage::EnhancedStorage::{
        PKEY_Comment, PKEY_Media_Year, PKEY_Music_AlbumArtist, PKEY_Music_AlbumTitle,
        PKEY_Music_Artist, PKEY_Music_Genre, PKEY_Music_PartOfSet, PKEY_Music_TrackNumber,
        PKEY_Title,
    };
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Com::StructuredStorage::{
        PropVariantClear, PropVariantGetElementCount, PropVariantGetStringElem,
        PropVariantToStringAlloc, PropVariantToUInt64, PROPVARIANT,
    };
    use windows::Win32::System::Variant::{VT_BLOB, VT_VECTOR};
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

    const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    const SOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;

    /// What the first bytes say the file is: a name for the "damaged" message
    /// and a MIME type that helps Media Foundation pick its reader.
    fn sniff(file: &mut File) -> (&'static str, Option<&'static str>) {
        let mut head = [0u8; 12];
        let n = file.read(&mut head).unwrap_or(0);
        let _ = file.seek(SeekFrom::Start(0));
        let h = &head[..n];
        const ASF: [u8; 4] = [0x30, 0x26, 0xB2, 0x75];
        if h.starts_with(&ASF) {
            ("WMA", Some("audio/x-ms-wma"))
        } else if h.len() >= 8 && &h[4..8] == b"ftyp" {
            ("M4A", Some("audio/mp4"))
        } else if h.starts_with(&[0x0B, 0x77]) {
            ("AC3", Some("audio/vnd.dolby.dd-raw"))
        } else if h.len() >= 2 && h[0] == 0xFF && h[1] & 0xF6 == 0xF0 {
            ("AAC", Some("audio/aac"))
        } else {
            ("FILE", None)
        }
    }

    fn codec_of(subtype: &GUID, user: Option<&[u8]>) -> Codec {
        let s = *subtype;
        if s == MFAudioFormat_WMAudioV8
            || s == MFAudioFormat_WMAudioV9
            || s == MFAudioFormat_WMAudio_Lossless
            || s == MFAudioFormat_MSP1
        {
            Codec::Wma
        } else if s == MFAudioFormat_Dolby_AC3 || s == MFAudioFormat_Dolby_AC3_SPDIF {
            Codec::Ac3
        } else if s == MFAudioFormat_AAC || s == MFAudioFormat_ADTS {
            // MF_MT_USER_DATA is the tail of a HEAACWAVEINFO (12 bytes), then
            // the AudioSpecificConfig.
            match user.filter(|u| u.len() > 12) {
                Some(u) if crate::mp4::is_he_aac(&u[12..]) => Codec::HeAac,
                _ => Codec::Aac,
            }
        } else if s == MFAudioFormat_MP3 || s == MFAudioFormat_MPEG {
            Codec::Mp3
        } else if s == MFAudioFormat_Dolby_DDPlus {
            Codec::Other("eac3".into())
        } else if s == MFAudioFormat_FLAC {
            Codec::Flac
        } else if s == MFAudioFormat_ALAC {
            Codec::Alac
        } else if s == MFAudioFormat_Opus {
            Codec::Opus
        } else if s == MFAudioFormat_PCM || s == MFAudioFormat_Float {
            Codec::Pcm {
                bits: 0,
                float: s == MFAudioFormat_Float,
            }
        } else {
            Codec::Other("unknown".into())
        }
    }

    /// True when this Windows says it has a decoder for `codec`.
    fn has_decoder(codec: &Codec) -> bool {
        let c = crate::mf_codecs();
        match codec {
            Codec::Wma => c.wma_dec,
            Codec::Ac3 => c.ac3_dec,
            Codec::Aac | Codec::HeAac => c.heaac_dec,
            _ => true,
        }
    }

    pub fn open(mut file: File) -> Result<Box<dyn Source>> {
        let mf = Mf::start()?;
        let total = file.metadata()?.len();
        let (code, mime) = sniff(&mut file);
        let damaged = || MediaError::Damaged { code: code.into() };
        let (stream, furthest) = com::byte_stream(file, false)?;
        if let (Some(mime), Ok(attrs)) = (mime, stream.cast::<IMFAttributes>()) {
            let wide: Vec<u16> = mime.encode_utf16().chain([0]).collect();
            // SAFETY: the string is NUL-terminated and copied by the call.
            let _ = unsafe { attrs.SetString(&MF_BYTESTREAM_CONTENT_TYPE, PCWSTR(wide.as_ptr())) };
        }
        // SAFETY: the byte stream is live; the reader keeps its own reference.
        let reader = match unsafe { MFCreateSourceReaderFromByteStream(&stream, None) } {
            Ok(r) => r,
            Err(e) if e.code() == MF_E_UNSUPPORTED_BYTESTREAM_TYPE && mime.is_none() => {
                return Err(MediaError::Unsupported {
                    codec: Codec::Other("unknown".into()).display_name(),
                })
            }
            Err(_) => return Err(damaged()),
        };

        // SAFETY: plain calls on the reader we just made.
        let native = unsafe {
            let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
            match reader.GetNativeMediaType(AUDIO, 0) {
                Ok(t) => t,
                Err(e) if e.code() == MF_E_INVALIDSTREAMNUMBER => return Err(MediaError::NoAudio),
                Err(_) => return Err(damaged()),
            }
        };
        let subtype = unsafe { native.GetGUID(&MF_MT_SUBTYPE) }.map_err(|_| damaged())?;
        let user = get_blob(&native, &MF_MT_USER_DATA);
        let codec = codec_of(&subtype, user.as_deref());
        if !has_decoder(&codec) {
            return Err(MediaError::NeedsMediaFoundation);
        }
        let native_channels = get_u32(&native, &MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0) as u16;

        // Float out, as many channels as the file holds (a Dolby decoder
        // otherwise folds 5.1 to stereo). 16-bit PCM when float isn't offered.
        let mut float = true;
        let set = |float: bool| -> Result<()> {
            let t = com::audio_type(
                &if float {
                    MFAudioFormat_Float
                } else {
                    MFAudioFormat_PCM
                },
                0,
                0,
            )?;
            // SAFETY: plain attribute calls on a new partial type.
            unsafe {
                let _ = t.DeleteItem(&MF_MT_AUDIO_SAMPLES_PER_SECOND);
                if native_channels > 0 {
                    t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, native_channels as u32)
                        .map_err(|_| damaged())?;
                } else {
                    let _ = t.DeleteItem(&MF_MT_AUDIO_NUM_CHANNELS);
                }
                if !float {
                    let _ = t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16);
                }
                reader
                    .SetCurrentMediaType(AUDIO, None, &t)
                    .map_err(|_| damaged())?;
                reader
                    .SetStreamSelection(AUDIO, true)
                    .map_err(|_| damaged())
            }
        };
        if set(true).is_err() {
            float = false;
            if set(false).is_err() {
                return Err(match codec {
                    Codec::Wma | Codec::Ac3 | Codec::HeAac => MediaError::NeedsMediaFoundation,
                    other => MediaError::Unsupported {
                        codec: other.display_name(),
                    },
                });
            }
        }
        // SAFETY: a plain call on the reader.
        let current = unsafe { reader.GetCurrentMediaType(AUDIO) }.map_err(|_| damaged())?;
        let rate = get_u32(&current, &MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0);
        let count = get_u32(&current, &MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0) as u16;
        let bits = get_u32(&current, &MF_MT_AUDIO_BITS_PER_SAMPLE).unwrap_or(16) as u16;
        if rate == 0 || count == 0 || (!float && bits != 16) {
            return Err(damaged());
        }
        let mask = get_u32(&current, &MF_MT_AUDIO_CHANNEL_MASK)
            .filter(|m| m.count_ones() == count as u32)
            .unwrap_or_else(|| default_mask(count));

        let frames = presentation_u64(&reader, &MF_PD_DURATION)
            .filter(|&d| d > 0)
            .map(|hns| (hns as u128 * rate as u128 + 5_000_000) as u64 / 10_000_000);
        let kbps = presentation_u64(&reader, &MF_PD_AUDIO_ENCODING_BITRATE)
            .map(|b| ((b + 500) / 1000) as u32)
            .or_else(|| get_u32(&native, &MF_MT_AUDIO_AVG_BYTES_PER_SECOND).map(|b| b * 8 / 1000))
            .filter(|&k| k > 0);
        let tags = read_tags(&reader);

        Ok(Box::new(MfSource {
            info: SourceInfo {
                codec,
                rate,
                channels: Channels { count, mask },
                bits: None,
                frames,
                kbps,
                tags,
            },
            reader,
            float,
            pending: Vec::new(),
            cursor: 0,
            ended: false,
            frames_out: 0,
            furthest,
            total,
            code,
            _mf: mf,
        }))
    }

    fn presentation_u64(reader: &IMFSourceReader, key: &GUID) -> Option<u64> {
        // SAFETY: the PROPVARIANT is read, then cleared.
        unsafe {
            let mut v = reader.GetPresentationAttribute(SOURCE, key).ok()?;
            let n = PropVariantToUInt64(&v).ok();
            let _ = PropVariantClear(&mut v);
            n
        }
    }

    // -----------------------------------------------------------------------
    // Tags

    /// A PROPVARIANT as text, then cleared. A list of strings is joined with
    /// "; ", each different value once: an ASF can hold the same title in two
    /// places, which the property store then lists twice. A single value is
    /// kept whole, so a title that really is "Echo; Echo" stays as it is.
    fn text_of(mut v: PROPVARIANT) -> Option<String> {
        let clean = |t: String| {
            t.trim_matches(|c: char| c.is_whitespace() || c == '\0')
                .to_string()
        };
        // SAFETY: every string the helpers return is CoTaskMem the caller
        // frees; the PROPVARIANT is cleared after.
        unsafe {
            let take = |p: PWSTR| {
                let s = pwstr(p);
                CoTaskMemFree(Some(p.0 as *const _));
                s
            };
            let vector = v.Anonymous.Anonymous.vt.0 & VT_VECTOR.0 != 0;
            let count = PropVariantGetElementCount(&v);
            let text = if vector && count > 1 {
                let mut parts: Vec<String> = Vec::new();
                for i in 0..count {
                    if let Some(t) = PropVariantGetStringElem(&v, i).ok().and_then(take) {
                        parts.push(clean(t));
                    }
                }
                Some(join_unique(parts))
            } else {
                PropVariantToStringAlloc(&v).ok().and_then(take).map(clean)
            };
            let _ = PropVariantClear(&mut v);
            text.filter(|t| !t.is_empty())
        }
    }

    /// The values joined with "; ", each one once, empty ones left out.
    pub(super) fn join_unique(parts: Vec<String>) -> String {
        let mut kept: Vec<String> = Vec::new();
        for p in parts {
            if !p.is_empty() && !kept.contains(&p) {
                kept.push(p);
            }
        }
        kept.join("; ")
    }

    unsafe fn pwstr(p: PWSTR) -> Option<String> {
        if p.is_null() {
            None
        } else {
            p.to_string().ok()
        }
    }

    fn number(s: &str) -> Option<(u32, Option<u32>)> {
        let mut it = s.splitn(2, '/');
        let n = it.next()?.trim().parse().ok()?;
        let total = it.next().and_then(|t| t.trim().parse().ok());
        Some((n, total))
    }

    /// The tags the file holds: the property store first, then the source's
    /// metadata for anything still missing.
    fn read_tags(reader: &IMFSourceReader) -> Tags {
        let mut tags = Tags::default();
        let mut source: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: asks the reader for its media source; on success `source`
        // holds a reference that `from_raw` takes over.
        let source = unsafe {
            if reader
                .GetServiceForStream(SOURCE, &GUID::zeroed(), &IMFMediaSource::IID, &mut source)
                .is_err()
                || source.is_null()
            {
                return tags;
            }
            IMFMediaSource::from_raw(source)
        };
        from_property_store(&source, &mut tags);
        from_metadata(&source, &mut tags);
        tags
    }

    fn from_property_store(source: &IMFMediaSource, tags: &mut Tags) {
        let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: the service hands back a property store reference on success.
        let store = unsafe {
            if MFGetService(
                source,
                &MF_PROPERTY_HANDLER_SERVICE,
                &IPropertyStore::IID,
                &mut p,
            )
            .is_err()
                || p.is_null()
            {
                return;
            }
            IPropertyStore::from_raw(p)
        };
        // SAFETY: plain reads from the store.
        let get = |key: &PROPERTYKEY| unsafe { store.GetValue(key) }.ok().and_then(text_of);
        let fill = |slot: &mut Option<String>, key| {
            if slot.is_none() {
                *slot = get(key);
            }
        };
        fill(&mut tags.title, &PKEY_Title);
        fill(&mut tags.artist, &PKEY_Music_Artist);
        fill(&mut tags.album, &PKEY_Music_AlbumTitle);
        fill(&mut tags.album_artist, &PKEY_Music_AlbumArtist);
        fill(&mut tags.genre, &PKEY_Music_Genre);
        fill(&mut tags.comment, &PKEY_Comment);
        fill(&mut tags.year, &PKEY_Media_Year);
        if tags.track.is_none() {
            tags.track = get(&PKEY_Music_TrackNumber).and_then(|s| number(&s));
        }
        if tags.disc.is_none() {
            tags.disc = get(&PKEY_Music_PartOfSet).and_then(|s| number(&s));
        }
        // A year of 0 means none.
        if tags.year.as_deref() == Some("0") {
            tags.year = None;
        }
        if tags.track.is_some_and(|(n, _)| n == 0) {
            tags.track = None;
        }
    }

    fn from_metadata(source: &IMFMediaSource, tags: &mut Tags) {
        let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: the service hands back a metadata provider reference on
        // success; the presentation descriptor is the source's own.
        let meta = unsafe {
            if MFGetService(
                source,
                &MF_METADATA_PROVIDER_SERVICE,
                &IMFMetadataProvider::IID,
                &mut p,
            )
            .is_err()
                || p.is_null()
            {
                return;
            }
            let provider = IMFMetadataProvider::from_raw(p);
            let Ok(pd) = source.CreatePresentationDescriptor() else {
                return;
            };
            match provider.GetMFMetadata(&pd, 0, 0) {
                Ok(m) => m,
                Err(_) => return,
            }
        };
        let get = |name: &str| {
            let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
            // SAFETY: the name is NUL-terminated and outlives the call.
            unsafe { meta.GetProperty(PCWSTR(wide.as_ptr())) }
                .ok()
                .and_then(text_of)
        };
        let fill = |slot: &mut Option<String>, name| {
            if slot.is_none() {
                *slot = get(name);
            }
        };
        fill(&mut tags.title, "Title");
        fill(&mut tags.artist, "Author");
        fill(&mut tags.album, "WM/AlbumTitle");
        fill(&mut tags.album_artist, "WM/AlbumArtist");
        fill(&mut tags.genre, "WM/Genre");
        fill(&mut tags.comment, "Description");
        fill(&mut tags.year, "WM/Year");
        if tags.track.is_none() {
            tags.track = get("WM/TrackNumber").and_then(|s| number(&s));
        }
        if tags.disc.is_none() {
            tags.disc = get("WM/PartOfSet").and_then(|s| number(&s));
        }
        if tags.picture.is_none() {
            let wide: Vec<u16> = "WM/Picture".encode_utf16().chain([0]).collect();
            // SAFETY: the name is NUL-terminated; a blob's bytes are copied
            // out before the PROPVARIANT is cleared.
            unsafe {
                if let Ok(mut v) = meta.GetProperty(PCWSTR(wide.as_ptr())) {
                    let inner = &v.Anonymous.Anonymous;
                    if inner.vt == VT_BLOB && !inner.Anonymous.blob.pBlobData.is_null() {
                        let b = &inner.Anonymous.blob;
                        let bytes = std::slice::from_raw_parts(b.pBlobData, b.cbSize as usize);
                        tags.picture = crate::mf::wma::read_wm_picture(bytes);
                    }
                    let _ = PropVariantClear(&mut v);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Reading

    struct MfSource {
        info: SourceInfo,
        reader: IMFSourceReader,
        float: bool,
        /// Decoded samples not yet handed out, from `cursor` on.
        pending: Vec<f32>,
        cursor: usize,
        ended: bool,
        frames_out: u64,
        furthest: Arc<AtomicU64>,
        total: u64,
        code: &'static str,
        // Last, so Media Foundation outlives the reader.
        _mf: Mf,
    }

    // SAFETY: the reader belongs to this source alone and lives in the
    // multithreaded apartment; every call first joins the calling thread to it.
    unsafe impl Send for MfSource {}

    impl MfSource {
        /// Reads the next decoded sample into `pending`. False at the end.
        fn fill(&mut self) -> Result<bool> {
            com::com();
            loop {
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                // SAFETY: a synchronous read; the out pointers are locals.
                let r = unsafe {
                    self.reader.ReadSample(
                        AUDIO,
                        0,
                        None,
                        Some(&mut flags),
                        None,
                        Some(&mut sample),
                    )
                };
                if r.is_err() || flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                    return Err(MediaError::Damaged {
                        code: self.code.into(),
                    });
                }
                if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                    self.check_type()?;
                }
                if let Some(s) = sample {
                    let bytes = com::sample_bytes(&s).map_err(|_| MediaError::Damaged {
                        code: self.code.into(),
                    })?;
                    self.pending.clear();
                    self.cursor = 0;
                    if self.float {
                        self.pending.extend(
                            bytes
                                .chunks_exact(4)
                                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                        );
                    } else {
                        self.pending.extend(
                            bytes
                                .chunks_exact(2)
                                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0),
                        );
                    }
                    let ch = self.info.channels.count as usize;
                    self.pending.truncate(self.pending.len() / ch * ch);
                    if !self.pending.is_empty() {
                        return Ok(true);
                    }
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    return Ok(false);
                }
            }
        }

        /// The decoder changed its output mid-stream: fine while the rate and
        /// channels stay, damaged otherwise.
        fn check_type(&mut self) -> Result<()> {
            // SAFETY: a plain call on the reader.
            let t = unsafe { self.reader.GetCurrentMediaType(AUDIO) }.map_err(|_| {
                MediaError::Damaged {
                    code: self.code.into(),
                }
            })?;
            let rate = get_u32(&t, &MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0);
            let count = get_u32(&t, &MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0) as u16;
            if rate != self.info.rate || count != self.info.channels.count {
                return Err(MediaError::Damaged {
                    code: self.code.into(),
                });
            }
            Ok(())
        }
    }

    impl Source for MfSource {
        fn info(&self) -> &SourceInfo {
            &self.info
        }

        fn read(&mut self, out: &mut Block) -> Result<bool> {
            let ch = self.info.channels.count as usize;
            out.channels = self.info.channels.count;
            out.rate = self.info.rate;
            out.samples.clear();
            while out.samples.len() < BLOCK_FRAMES * ch {
                if self.cursor >= self.pending.len() && (self.ended || !self.fill()?) {
                    self.ended = true;
                    break;
                }
                let want = BLOCK_FRAMES * ch - out.samples.len();
                let take = want.min(self.pending.len() - self.cursor);
                out.samples
                    .extend_from_slice(&self.pending[self.cursor..self.cursor + take]);
                self.cursor += take;
            }
            self.frames_out += (out.samples.len() / ch) as u64;
            Ok(!out.samples.is_empty())
        }

        fn position(&self) -> Progressed {
            Progressed {
                frames: self.frames_out,
                bytes: self.furthest.load(Ordering::Relaxed).min(self.total),
                total_bytes: self.total,
            }
        }
    }

    impl Drop for MfSource {
        fn drop(&mut self) {
            com::com();
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::win::join_unique;

    #[test]
    fn a_list_keeps_each_value_once() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(join_unique(v(&["Tone", "Tone"])), "Tone");
        // One value that really holds "; " is kept whole.
        assert_eq!(join_unique(v(&["Echo; Echo", "Echo; Echo"])), "Echo; Echo");
        assert_eq!(join_unique(v(&["A", "B", "A"])), "A; B");
        assert_eq!(join_unique(v(&["", "A"])), "A");
    }
}
