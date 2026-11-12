//! AC3 (Dolby Digital) out, written as a raw elementary stream.
//!
//! Mono and stereo go through Windows' Dolby Digital encoder (an MFT), which
//! takes 48 kHz and only the bitrates [`bitrates`] lists. Windows' encoder
//! has no surround mode, so 3 to 6 channels go to Kiln's own encoder
//! ([`crate::ac3enc`]) through [`surround_sink`].

use crate::error::{MediaError, Result};
use crate::pipeline::{Sink, SinkSpec};

/// What Windows' encoder takes at 48 kHz, used when Media Foundation can't be
/// asked: mono, then stereo.
pub const MONO_KBPS: [u32; 9] = [64, 80, 96, 112, 128, 160, 192, 224, 256];
pub const STEREO_KBPS: [u32; 8] = [128, 160, 192, 224, 256, 320, 384, 448];
/// Every AC3 bitrate from 96 kbps up, offered for surround.
pub const SURROUND_KBPS: [u32; 13] = [
    96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
];
/// Every AC3 bitrate.
#[cfg_attr(not(windows), allow(dead_code))]
const ALL_KBPS: [u32; 19] = [
    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
];
/// The one rate the encoder is used at.
const RATE: u32 = 48_000;

/// The bitrates (kbps, ascending) Windows' Dolby Digital encoder takes for
/// `channels` at `rate`, each checked against the encoder. Empty for more
/// than two channels, which Windows can't encode; the fixed mono and stereo
/// lists when Media Foundation can't be asked.
pub fn bitrates(rate: u32, channels: u16) -> Vec<u32> {
    if !(1..=2).contains(&channels) {
        return Vec::new();
    }
    #[cfg(windows)]
    {
        if crate::mf_codecs().ac3_enc {
            // Every probe failing is no reason to offer nothing.
            if let Some(list) = win::bitrates(rate, channels).filter(|l| !l.is_empty()) {
                return list;
            }
        }
    }
    let _ = rate;
    fixed(channels)
}

/// The fixed list for mono or stereo.
fn fixed(channels: u16) -> Vec<u32> {
    if channels == 1 {
        MONO_KBPS.to_vec()
    } else {
        STEREO_KBPS.to_vec()
    }
}

/// Makes a sink that writes a raw AC3 stream at `Setting::Kbps`: 48 kHz,
/// 1 to 6 channels (the caller's `Process` resamples and maps channels).
pub fn sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    let channels = spec.channels.count;
    if spec.rate != RATE {
        return Err(MediaError::Encoder(format!(
            "AC3 is written at 48 kHz, not {} Hz",
            spec.rate
        )));
    }
    match channels {
        1 | 2 => stereo_sink(spec),
        3..=6 => surround_sink(spec),
        _ => Err(MediaError::Encoder("AC3 takes up to 5.1 channels.".into())),
    }
}

/// AC3 in 3 to 6 channels, which Windows' encoder can't do: Kiln's own
/// encoder, written from the A/52 standard.
pub fn surround_sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    crate::ac3enc::sink(spec)
}

#[cfg(windows)]
fn stereo_sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
    win::sink(spec)
}

#[cfg(not(windows))]
fn stereo_sink(_spec: SinkSpec) -> Result<Box<dyn Sink>> {
    Err(MediaError::NeedsMediaFoundation)
}

#[cfg(windows)]
mod win {
    use super::ALL_KBPS;
    use crate::error::{MediaError, Result};
    use crate::mf::com::{self, failed, Mf, Transform};
    use crate::pipeline::{no_audio, Sink, SinkSpec};
    use crate::tune::Setting;
    use crate::types::{default_mask, Block};
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::{BufWriter, Write};
    use std::sync::Mutex;
    use windows::Win32::Media::MediaFoundation::*;

    /// Bitrates already asked for, by rate and channels.
    type Cache = Mutex<Option<HashMap<(u32, u16), Vec<u32>>>>;

    fn output_type(rate: u32, channels: u16, kbps: u32) -> Result<IMFMediaType> {
        let t = com::audio_type(&MFAudioFormat_Dolby_AC3, rate, channels)?;
        // SAFETY: plain attribute calls.
        unsafe {
            t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, kbps * 125)
                .and_then(|_| t.SetUINT32(&MF_MT_AUDIO_CHANNEL_MASK, default_mask(channels)))
                .map_err(|e| failed("can't describe AC3", e))?;
        }
        Ok(t)
    }

    /// The bitrates the encoder takes, tried one by one. `None` when the
    /// encoder can't be made.
    pub fn bitrates(rate: u32, channels: u16) -> Option<Vec<u32>> {
        static CACHE: Cache = Mutex::new(None);
        let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = cache.as_ref().and_then(|c| c.get(&(rate, channels))) {
            return Some(v.clone());
        }
        let _mf = Mf::start().ok()?;
        let t = com::encoder(&MFAudioFormat_Dolby_AC3)?;
        let list: Vec<u32> = ALL_KBPS
            .iter()
            .copied()
            .filter(|&kbps| {
                output_type(rate, channels, kbps).is_ok_and(|o| {
                    // SAFETY: a test-only type check; nothing is set.
                    unsafe { t.SetOutputType(0, &o, MFT_SET_TYPE_TEST_ONLY.0 as u32) }.is_ok()
                })
            })
            .collect();
        cache
            .get_or_insert_with(HashMap::new)
            .insert((rate, channels), list.clone());
        Some(list)
    }

    pub fn sink(spec: SinkSpec) -> Result<Box<dyn Sink>> {
        let Setting::Kbps { kbps } = *spec.setting else {
            return Err(MediaError::Encoder(
                "this setting isn't an AC3 setting".into(),
            ));
        };
        let (rate, channels) = (spec.rate, spec.channels.count);
        if !crate::mf_codecs().ac3_enc {
            return Err(MediaError::NeedsMediaFoundation);
        }
        let mf = Mf::start()?;
        let t = com::encoder(&MFAudioFormat_Dolby_AC3).ok_or(MediaError::NeedsMediaFoundation)?;
        // SAFETY: plain type calls on a transform we own.
        unsafe {
            t.SetOutputType(0, &output_type(rate, channels, kbps)?, 0)
                .map_err(|_| {
                    MediaError::Encoder(format!(
                        "Windows' AC3 encoder won't take {kbps} kbps for {channels} channels"
                    ))
                })?;
            t.SetInputType(
                0,
                &com::pcm_type(true, rate, channels, default_mask(channels))?,
                0,
            )
            .map_err(|e| failed("the AC3 encoder won't take float audio", e))?;
        }
        Ok(Box::new(Ac3Sink {
            tf: Transform::new(t, 4 * channels as u32, rate)?,
            out: BufWriter::with_capacity(256 * 1024, spec.out),
            rate,
            channels,
            bytes: Vec::new(),
            packets: Vec::new(),
            frames: 0,
            _mf: mf,
        }))
    }

    struct Ac3Sink {
        tf: Transform,
        out: BufWriter<File>,
        rate: u32,
        channels: u16,
        bytes: Vec<u8>,
        packets: Vec<Vec<u8>>,
        frames: u64,
        // Last, so Media Foundation outlives the transform.
        _mf: Mf,
    }

    // SAFETY: the transform belongs to this sink alone and lives in the
    // multithreaded apartment; every call first joins the calling thread to it.
    unsafe impl Send for Ac3Sink {}

    impl Ac3Sink {
        fn store(&mut self) -> Result<()> {
            for p in self.packets.drain(..) {
                self.out.write_all(&p)?;
            }
            Ok(())
        }
    }

    impl Sink for Ac3Sink {
        fn write(&mut self, b: &Block) -> Result<()> {
            com::com();
            if b.rate != self.rate || b.channels != self.channels {
                return Err(MediaError::Encoder(format!(
                    "a block of {} channels at {} Hz came to a {}-channel {} Hz AC3 encoder",
                    b.channels, b.rate, self.channels, self.rate
                )));
            }
            com::float_bytes(&b.samples, &mut self.bytes);
            self.tf.push(&self.bytes, &mut self.packets)?;
            self.frames += b.frames() as u64;
            self.store()
        }

        fn finish(mut self: Box<Self>) -> Result<()> {
            com::com();
            if self.frames == 0 {
                return Err(no_audio("AC3"));
            }
            self.tf.drain(&mut self.packets)?;
            self.store()?;
            self.out.flush()?;
            Ok(())
        }
    }
}
