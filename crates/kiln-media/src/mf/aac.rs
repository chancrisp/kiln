//! AAC-LC through Windows' AAC encoder (an MFT), as raw AAC frames plus the
//! AudioSpecificConfig. [`crate::mp4`] wraps the frames in an M4A or ADTS.
//!
//! The encoder takes 16-bit PCM at 44.1 or 48 kHz, in 1, 2 or 6 channels
//! (Windows refuses 3 to 5). Mono and stereo take 96, 128, 160 or 192 kbps;
//! 5.1 takes the totals Windows lists for it, which [`bitrates`] gives. The
//! caller's `Process` resamples and maps channels first; anything else is an
//! `Encoder` error.

use crate::error::{MediaError, Result};

/// The bitrates Windows' AAC encoder takes for mono and stereo.
pub const KBPS: [u32; 4] = [96, 128, 160, 192];

/// Frames of encoder delay at the start of the stream, written into the M4A's
/// edit list and iTunSMPB. Windows' encoder takes its own delay out: decoded
/// by Symphonia, by Media Foundation and by FFmpeg (edit list ignored), the
/// first output frame is the first input frame. Measured by cross-correlation
/// in `tests/mf.rs` (`aac_priming_is_what_the_m4a_says`). The edit list then
/// trims only the padding at the end.
pub const PRIMING: u32 = 0;

/// The total bitrates (kbps, ascending) the encoder takes for `channels` at
/// `rate`. Mono and stereo: [`KBPS`]. More channels: what Windows lists and
/// accepts there (for 5.1 at 48 kHz, 128 to 1,152 kbps; nothing for 3 to 5
/// channels). Without Media Foundation, [`KBPS`] scaled per channel pair.
pub fn bitrates(rate: u32, channels: u16) -> Vec<u32> {
    if channels <= 2 {
        return KBPS.to_vec();
    }
    #[cfg(windows)]
    {
        if crate::mf_codecs().aac_enc {
            if let Some(list) = win::surround_bitrates(rate, channels) {
                return list;
            }
        }
    }
    let _ = rate;
    KBPS.iter().map(|k| k * channels as u32 / 2).collect()
}

/// Checks the rate and channels, before Windows is asked.
fn check(rate: u32, channels: u16) -> Result<()> {
    if rate != 44_100 && rate != 48_000 {
        return Err(MediaError::Encoder(format!(
            "AAC takes 44.1 or 48 kHz, not {rate} Hz"
        )));
    }
    if channels == 0 || channels > 6 {
        return Err(MediaError::Encoder(format!(
            "AAC takes 1 to 6 channels, not {channels}"
        )));
    }
    Ok(())
}

/// Checks the bitrate against what the encoder takes.
fn check_kbps(rate: u32, channels: u16, kbps: u32) -> Result<()> {
    let listed = bitrates(rate, channels);
    if listed.contains(&kbps) {
        return Ok(());
    }
    let list: Vec<String> = listed.iter().map(u32::to_string).collect();
    Err(MediaError::Encoder(format!(
        "AAC in {channels} channels takes {} kbps, not {kbps}",
        if list.is_empty() {
            "no".to_string()
        } else {
            list.join(", ")
        }
    )))
}

#[cfg(windows)]
pub use win::{encoder, AacEncoder};

#[cfg(not(windows))]
pub use other::{encoder, AacEncoder};

#[cfg(windows)]
mod win {
    use super::{check, check_kbps, PRIMING};
    use crate::error::{MediaError, Result};
    use crate::mf::com::{self, failed, Mf, Transform};
    use crate::types::default_mask;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use windows::Win32::Media::MediaFoundation::*;

    /// AAC Profile level 2 covers stereo; 5.1 needs level 4.
    fn level(channels: u16) -> u32 {
        if channels > 2 {
            0x2A
        } else {
            0x29
        }
    }

    fn output_type(rate: u32, channels: u16, kbps: u32) -> Result<IMFMediaType> {
        let out = com::audio_type(&MFAudioFormat_AAC, rate, channels)?;
        // SAFETY: plain attribute calls.
        unsafe {
            out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                .and_then(|_| out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, kbps * 125))
                // Raw AAC frames, AAC-LC.
                .and_then(|_| out.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0))
                .and_then(|_| {
                    out.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, level(channels))
                })
                .map_err(|e| failed("can't describe AAC", e))?;
        }
        Ok(out)
    }

    /// Bitrates already asked for, by rate and channels.
    type Cache = Mutex<Option<HashMap<(u32, u16), Vec<u32>>>>;

    /// The totals Windows lists for more than two channels and accepts when
    /// tried. `None` when the encoder can't be made.
    pub fn surround_bitrates(rate: u32, channels: u16) -> Option<Vec<u32>> {
        static CACHE: Cache = Mutex::new(None);
        let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = cache.as_ref().and_then(|c| c.get(&(rate, channels))) {
            return Some(v.clone());
        }
        let _mf = Mf::start().ok()?;
        let t = com::encoder(&MFAudioFormat_AAC)?;
        let mut list = Vec::new();
        for i in 0.. {
            // SAFETY: plain enumeration on a transform we own.
            let Ok(mt) = (unsafe { t.GetOutputAvailableType(0, i) }) else {
                break;
            };
            let get = |k| com::get_u32(&mt, k);
            if get(&MF_MT_AUDIO_SAMPLES_PER_SECOND) == Some(rate)
                && get(&MF_MT_AUDIO_NUM_CHANNELS) == Some(channels as u32)
                && get(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION) == Some(level(channels))
            {
                if let Some(b) = get(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND) {
                    list.push(b * 8 / 1000);
                }
            }
        }
        list.sort_unstable();
        list.dedup();
        list.retain(|&kbps| {
            output_type(rate, channels, kbps).is_ok_and(|o| {
                // SAFETY: a test-only type check; nothing is set.
                unsafe { t.SetOutputType(0, &o, MFT_SET_TYPE_TEST_ONLY.0 as u32) }.is_ok()
            })
        });
        cache
            .get_or_insert_with(HashMap::new)
            .insert((rate, channels), list.clone());
        Some(list)
    }

    /// Windows' AAC encoder, made ready for one stream.
    pub struct AacEncoder {
        tf: Transform,
        asc: Vec<u8>,
        rate: u32,
        channels: u16,
        kbps: u32,
        bytes: Vec<u8>,
        // Last, so Media Foundation outlives the transform.
        _mf: Mf,
    }

    // SAFETY: the transform belongs to this encoder alone and lives in the
    // multithreaded apartment; every call first joins the calling thread to it.
    unsafe impl Send for AacEncoder {}

    /// Makes an AAC-LC encoder for `rate` (44.1 or 48 kHz), 1, 2 or 6
    /// channels, and `kbps`, the total bitrate, one that [`super::bitrates`]
    /// lists.
    pub fn encoder(rate: u32, channels: u16, kbps: u32) -> Result<AacEncoder> {
        check(rate, channels)?;
        if !crate::mf_codecs().aac_enc {
            return Err(MediaError::NeedsMediaFoundation);
        }
        check_kbps(rate, channels, kbps)?;
        let mf = Mf::start()?;
        let t = com::encoder(&MFAudioFormat_AAC).ok_or(MediaError::NeedsMediaFoundation)?;
        let refused = || {
            MediaError::Encoder(format!(
                "Windows' AAC encoder won't take {channels} channels at {rate} Hz and {kbps} kbps"
            ))
        };
        // SAFETY: plain type calls on a transform we own.
        let asc = unsafe {
            let out = output_type(rate, channels, kbps)?;
            t.SetOutputType(0, &out, 0).map_err(|_| refused())?;
            let input = com::pcm_type(false, rate, channels, default_mask(channels))?;
            t.SetInputType(0, &input, 0).map_err(|_| refused())?;
            let current = t
                .GetOutputCurrentType(0)
                .map_err(|e| failed("the AAC encoder has no output type", e))?;
            // The user data is the tail of a HEAACWAVEINFO (12 bytes), then
            // the AudioSpecificConfig.
            let user = com::get_blob(&current, &MF_MT_USER_DATA).unwrap_or_default();
            if user.len() <= 12 {
                return Err(MediaError::Encoder(
                    "the AAC encoder gave no AudioSpecificConfig".into(),
                ));
            }
            user[12..].to_vec()
        };
        let tf = Transform::new(t, 2 * channels as u32, rate)?;
        Ok(AacEncoder {
            tf,
            asc,
            rate,
            channels,
            kbps,
            bytes: Vec::new(),
            _mf: mf,
        })
    }

    impl AacEncoder {
        pub fn rate(&self) -> u32 {
            self.rate
        }
        pub fn channels(&self) -> u16 {
            self.channels
        }
        pub fn kbps(&self) -> u32 {
            self.kbps
        }
        /// The AudioSpecificConfig, as an MP4 `esds` carries it.
        pub fn asc(&self) -> &[u8] {
            &self.asc
        }
        /// Frames of delay before the first input frame comes out.
        pub fn priming(&self) -> u32 {
            PRIMING
        }

        /// Encodes interleaved samples (held to [-1, 1] first), adding each
        /// finished AAC frame to `out`.
        pub fn encode(&mut self, samples: &[f32], out: &mut Vec<Vec<u8>>) -> Result<()> {
            com::com();
            com::i16_bytes(samples, &mut self.bytes);
            self.tf.push(&self.bytes, out)
        }

        /// Encodes what the encoder still holds, adding the last frames to `out`.
        pub fn finish(&mut self, out: &mut Vec<Vec<u8>>) -> Result<()> {
            com::com();
            self.tf.drain(out)
        }
    }
}

#[cfg(not(windows))]
mod other {
    use super::{check, check_kbps, PRIMING};
    use crate::error::{MediaError, Result};

    /// Only Windows has the AAC encoder; off Windows this can't be made.
    pub struct AacEncoder {
        never: std::convert::Infallible,
    }

    pub fn encoder(rate: u32, channels: u16, kbps: u32) -> Result<AacEncoder> {
        check(rate, channels)?;
        check_kbps(rate, channels, kbps)?;
        Err(MediaError::NeedsMediaFoundation)
    }

    impl AacEncoder {
        pub fn rate(&self) -> u32 {
            match self.never {}
        }
        pub fn channels(&self) -> u16 {
            match self.never {}
        }
        pub fn kbps(&self) -> u32 {
            match self.never {}
        }
        pub fn asc(&self) -> &[u8] {
            match self.never {}
        }
        pub fn priming(&self) -> u32 {
            let _ = PRIMING;
            match self.never {}
        }
        pub fn encode(&mut self, _: &[f32], _: &mut Vec<Vec<u8>>) -> Result<()> {
            match self.never {}
        }
        pub fn finish(&mut self, _: &mut Vec<Vec<u8>>) -> Result<()> {
            match self.never {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_checked_before_windows_is_asked() {
        assert!(matches!(check(32_000, 2), Err(MediaError::Encoder(_))));
        assert!(matches!(check(44_100, 7), Err(MediaError::Encoder(_))));
        assert!(matches!(check(44_100, 0), Err(MediaError::Encoder(_))));
        assert!(check(44_100, 6).is_ok());
        assert!(check(48_000, 1).is_ok());
        assert!(matches!(
            check_kbps(48_000, 2, 320),
            Err(MediaError::Encoder(_))
        ));
        assert!(check_kbps(48_000, 2, 160).is_ok());
    }
}
