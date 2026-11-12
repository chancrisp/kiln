//! What each encoder is told besides its quality or bitrate (R85, Task 10d).
//!
//! Each setting was kept only where the quality harness showed Kiln's VMAF
//! gaining on x264's or x265's at a matched size, with the encode time
//! within twice what it was (Task 10d's report has the measurements):
//! - Microsoft's software H.264 encoder: CABAC, which it leaves off unless
//!   asked; quality-vs-speed 66 (at 100 it switches on the 8×8 transform,
//!   and with CABAC its pictures then decode broken); a key frame every
//!   10 s; no B-frames.
//! - Microsoft's software HEVC encoder (the HEVC Video Extensions): a key
//!   frame every 2 s, as before, and on the quality steps key frames four
//!   QP steps finer than the rest, given frame by frame since it takes no
//!   per-type QP. Its pictures soften the longer a run of P-frames goes, so
//!   a key frame every half second scores far better on moving pictures,
//!   but it makes a still screen recording two to three times the size;
//!   that is left to the owner. Its own quality-vs-speed stays: 66 gains
//!   but takes 2.2 to 2.4 times as long.
//! - NVIDIA's encoders: a key frame every 10 s. Their own quality vs speed
//!   stays: 66 passed no more rows (one fewer for HEVC) and took 1.4 times
//!   as long. They offer no B-frames through Media Foundation.
//! - Any other encoder (Intel's, AMD's; not measured): a key frame every
//!   2 s and two B-frames where offered, set once before the media types,
//!   exactly as before.
//!
//! The guard is the family: each encoder gets only what was measured on it,
//! picked by Windows' H.264 encoder's CLSID, NVIDIA's vendor ID or the HEVC
//! Video Extensions' friendly name. `ICodecAPI::IsSupported` is no guard:
//! every encoder here says yes to nearly every property, including ones it
//! then ignores. The HEVC Video Extensions' encoder has no CLSID attribute,
//! so it is matched by its friendly name; should Microsoft rename it, it
//! falls to Other and gets the old settings.

use super::tune::VCodec;

/// The encoders whose settings were measured, by maker and codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderFamily {
    /// Windows' own software H.264 encoder (`CLSID_MSH264EncoderMFT`).
    MicrosoftH264,
    /// The HEVC Video Extensions' software encoder.
    MicrosoftHevc,
    /// An NVIDIA graphics card's H.264 or HEVC encoder (vendor `VEN_10DE`).
    Nvidia,
    /// Anything else.
    Other,
}

/// The friendly name the HEVC Video Extensions give their encoder.
pub const MICROSOFT_HEVC_NAME: &str = "HEVCVideoExtensionEncoder";
/// NVIDIA's hardware vendor ID, as Media Foundation gives it.
pub const NVIDIA_VENDOR: &str = "VEN_10DE";

impl EncoderFamily {
    /// The family of an encoder, from what Media Foundation says of it:
    /// whether it is hardware, its CLSID being Windows' H.264 encoder's,
    /// its friendly name and its vendor ID.
    pub fn of(
        codec: VCodec,
        hardware: bool,
        microsoft_h264_clsid: bool,
        name: Option<&str>,
        vendor: Option<&str>,
    ) -> EncoderFamily {
        if hardware {
            if vendor.is_some_and(|v| v.eq_ignore_ascii_case(NVIDIA_VENDOR)) {
                EncoderFamily::Nvidia
            } else {
                EncoderFamily::Other
            }
        } else if codec == VCodec::H264 && microsoft_h264_clsid {
            EncoderFamily::MicrosoftH264
        } else if codec == VCodec::Hevc && name == Some(MICROSOFT_HEVC_NAME) {
            EncoderFamily::MicrosoftHevc
        } else {
            EncoderFamily::Other
        }
    }
}

/// The settings an encoder gets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EncoderSettings {
    /// Seconds from one key frame to the next.
    pub key_every: f64,
    /// B-frames between references (`AVEncMPVDefaultBPictureCount`), where
    /// set.
    pub b_frames: Option<u32>,
    /// CABAC rather than CAVLC (`AVEncH264CABACEnable`), where set.
    pub cabac: Option<bool>,
    /// `AVEncCommonQualityVsSpeed`, 0 to 100, where set.
    pub quality_vs_speed: Option<u32>,
    /// On a quality step, how many QP steps finer than the others each key
    /// frame is, set frame by frame (`AVEncVideoEncodeQP`).
    pub key_qp_finer: Option<u32>,
}

/// The settings for `family`.
pub fn settings_for(family: EncoderFamily) -> EncoderSettings {
    match family {
        EncoderFamily::MicrosoftH264 => EncoderSettings {
            key_every: 10.0,
            b_frames: Some(0),
            cabac: Some(true),
            quality_vs_speed: Some(66),
            key_qp_finer: None,
        },
        EncoderFamily::MicrosoftHevc => EncoderSettings {
            key_every: 2.0,
            b_frames: None,
            cabac: None,
            quality_vs_speed: None,
            key_qp_finer: Some(4),
        },
        EncoderFamily::Nvidia => EncoderSettings {
            key_every: 10.0,
            b_frames: None,
            cabac: None,
            quality_vs_speed: None,
            key_qp_finer: None,
        },
        EncoderFamily::Other => EncoderSettings {
            key_every: 2.0,
            b_frames: Some(2),
            cabac: None,
            quality_vs_speed: None,
            key_qp_finer: None,
        },
    }
}

/// The settings for `family` as this process uses them: those of
/// [`settings_for`], except that in a debug build `KILN_H264_B_FRAMES=n`
/// gives Microsoft's H.264 encoder `n` B-frames, so a test can still make a
/// picture whose frames are reordered.
pub fn settings_in_use(family: EncoderFamily) -> EncoderSettings {
    let s = settings_for(family);
    #[cfg(debug_assertions)]
    if family == EncoderFamily::MicrosoftH264 {
        if let Some(n) = std::env::var("KILN_H264_B_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            return EncoderSettings {
                b_frames: Some(n),
                ..s
            };
        }
    }
    s
}

/// Frames from one key frame to the next at `fps`, at least 1.
pub fn key_interval(s: &EncoderSettings, fps: f64) -> u32 {
    let fps = if fps.is_finite() && fps > 0.0 {
        fps
    } else {
        30.0
    };
    (fps * s.key_every).round().clamp(1.0, 1200.0) as u32
}

/// The QP Media Foundation's encoders give a quality (`AVEncCommonQuality`,
/// 0 to 100) in Quality mode, the same for every frame: measured on
/// Microsoft's H.264 and HEVC software encoders, which agree (NVIDIA's
/// differ from it by one at a few qualities).
const QUALITY_QP: [u8; 101] = [
    44, 44, 44, 43, 43, 43, 43, 42, 42, 42, 41, 41, 41, 41, 40, 40, 40, 39, 39, 39, 39, 38, 38, 38,
    37, 37, 37, 37, 36, 36, 36, 36, 35, 35, 35, 34, 34, 34, 34, 33, 33, 33, 32, 32, 32, 32, 31, 31,
    31, 30, 30, 30, 30, 29, 29, 29, 28, 28, 28, 28, 27, 27, 27, 26, 26, 26, 26, 25, 25, 25, 24, 24,
    24, 24, 23, 23, 23, 23, 22, 22, 22, 21, 21, 21, 21, 20, 20, 20, 19, 19, 19, 19, 18, 18, 18, 17,
    17, 17, 17, 16, 16,
];

/// The QP the encoder gives `quality` (clamped to 100).
pub fn quality_qp(quality: u8) -> u32 {
    QUALITY_QP[usize::from(quality.min(100))] as u32
}

/// The QP for frame `n` (from 0) on a quality step, with `s`'s finer key
/// frames: the key frames are every [`key_interval`] frames from the first.
/// `None` when `s` sets no QP frame by frame.
pub fn frame_qp(s: &EncoderSettings, quality: u8, interval: u32, n: u64) -> Option<u32> {
    let finer = s.key_qp_finer?;
    let qp = quality_qp(quality);
    Some(if n % u64::from(interval.max(1)) == 0 {
        qp.saturating_sub(finer)
    } else {
        qp
    })
}

/// One setting as the encoder read it back after it was set: its name and
/// value (a flag as 0 or 1), or `None` when the encoder refused it.
pub type ReadBack = (&'static str, Option<u32>);

/// The read-back name of the key-frame interval, in frames.
pub const KEY_INTERVAL: &str = "key frame interval";

#[cfg(windows)]
mod win {
    use super::{key_interval, EncoderSettings, ReadBack};
    use windows::core::GUID;
    use windows::Win32::Media::MediaFoundation::{
        CODECAPI_AVEncCommonQualityVsSpeed, CODECAPI_AVEncH264CABACEnable,
        CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize,
        CODECAPI_AVEncVideoEncodeQP, ICodecAPI,
    };
    use windows::Win32::System::Variant::VARIANT;

    /// Sets `key` to `v` and reads it back, a `flag` (`VT_BOOL`, which a
    /// number conversion reads as 0xFFFFFFFF for true) as 0 or 1.
    fn set(api: &ICodecAPI, key: &GUID, v: VARIANT, flag: bool) -> Option<u32> {
        // SAFETY: plain ICodecAPI calls with a VARIANT that outlives them.
        unsafe {
            api.SetValue(key, &v).ok()?;
            let back = api.GetValue(key).ok()?;
            if flag {
                bool::try_from(&back).map(u32::from).ok()
            } else {
                u32::try_from(&back).ok()
            }
        }
    }

    /// Gives the encoder behind `api` the settings `s` at `fps` frames a
    /// second, and returns each as the encoder reads it back.
    pub fn apply(api: &ICodecAPI, s: &EncoderSettings, fps: f64) -> Vec<ReadBack> {
        let mut back = vec![(
            super::KEY_INTERVAL,
            set(
                api,
                &CODECAPI_AVEncMPVGOPSize,
                VARIANT::from(key_interval(s, fps)),
                false,
            ),
        )];
        if let Some(b) = s.b_frames {
            back.push((
                "B-frames",
                set(
                    api,
                    &CODECAPI_AVEncMPVDefaultBPictureCount,
                    VARIANT::from(b),
                    false,
                ),
            ));
        }
        if let Some(c) = s.cabac {
            back.push((
                "CABAC",
                set(api, &CODECAPI_AVEncH264CABACEnable, VARIANT::from(c), true),
            ));
        }
        if let Some(q) = s.quality_vs_speed {
            back.push((
                "quality vs speed",
                set(
                    api,
                    &CODECAPI_AVEncCommonQualityVsSpeed,
                    VARIANT::from(q),
                    false,
                ),
            ));
        }
        back
    }

    /// Sets the QP of the next frame; true when the encoder took it.
    pub fn set_frame_qp(api: &ICodecAPI, qp: u32) -> bool {
        // SAFETY: a plain ICodecAPI call with a VARIANT that outlives it.
        unsafe { api.SetValue(&CODECAPI_AVEncVideoEncodeQP, &VARIANT::from(u64::from(qp))) }.is_ok()
    }
}

#[cfg(windows)]
pub(crate) use win::{apply, set_frame_qp};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_encoder_is_known_by_what_media_foundation_says() {
        use EncoderFamily::*;
        let of = EncoderFamily::of;
        assert_eq!(of(VCodec::H264, false, true, None, None), MicrosoftH264);
        assert_eq!(
            of(VCodec::Hevc, false, false, Some(MICROSOFT_HEVC_NAME), None),
            MicrosoftHevc
        );
        assert_eq!(
            of(VCodec::Hevc, false, false, Some("x265 MFT"), None),
            Other
        );
        assert_eq!(
            of(VCodec::H264, true, false, None, Some("VEN_10DE")),
            Nvidia
        );
        assert_eq!(
            of(VCodec::Hevc, true, false, None, Some("ven_10de")),
            Nvidia
        );
        assert_eq!(of(VCodec::H264, true, false, None, Some("VEN_8086")), Other);
        assert_eq!(of(VCodec::H264, true, false, None, Some("VEN_1002")), Other);
        assert_eq!(of(VCodec::H264, false, false, None, None), Other);
    }

    #[test]
    fn the_settings_chosen_for_each_encoder() {
        let ms264 = settings_for(EncoderFamily::MicrosoftH264);
        assert_eq!(ms264.cabac, Some(true));
        assert_eq!(ms264.b_frames, Some(0));
        assert_eq!(ms264.quality_vs_speed, Some(66));
        assert_eq!(ms264.key_every, 10.0);
        assert_eq!(ms264.key_qp_finer, None);

        let ms265 = settings_for(EncoderFamily::MicrosoftHevc);
        assert_eq!(ms265.key_every, 2.0);
        assert_eq!(ms265.key_qp_finer, Some(4));
        assert_eq!(
            (ms265.cabac, ms265.b_frames, ms265.quality_vs_speed),
            (None, None, None)
        );

        let nv = settings_for(EncoderFamily::Nvidia);
        assert_eq!(nv.key_every, 10.0);
        assert_eq!(
            (nv.cabac, nv.b_frames, nv.quality_vs_speed, nv.key_qp_finer),
            (None, None, None, None)
        );

        // Unmeasured encoders keep what every encoder had before.
        let other = settings_for(EncoderFamily::Other);
        assert_eq!(other.key_every, 2.0);
        assert_eq!(other.b_frames, Some(2));
        assert_eq!(
            (other.cabac, other.quality_vs_speed, other.key_qp_finer),
            (None, None, None)
        );
    }

    #[test]
    fn key_frames_are_spaced_in_frames() {
        let ten = settings_for(EncoderFamily::Nvidia);
        assert_eq!(key_interval(&ten, 24.0), 240);
        assert_eq!(key_interval(&ten, 29.97), 300);
        assert_eq!(key_interval(&ten, 240.0), 1200);
        // An unknown rate counts as 30.
        assert_eq!(key_interval(&ten, 0.0), 300);
        assert_eq!(key_interval(&ten, f64::NAN), 300);
        let two = settings_for(EncoderFamily::MicrosoftHevc);
        assert_eq!(key_interval(&two, 24.0), 48);
        assert_eq!(key_interval(&two, 0.25), 1);
        let other = settings_for(EncoderFamily::Other);
        assert_eq!(key_interval(&other, 25.0), 50);
    }

    #[test]
    fn quality_maps_to_media_foundations_qp() {
        // Points read from the encoders' slice headers.
        for (q, qp) in [
            (0, 44),
            (20, 39),
            (45, 32),
            (65, 26),
            (80, 22),
            (90, 19),
            (100, 16),
        ] {
            assert_eq!(quality_qp(q), qp, "quality {q}");
        }
        assert_eq!(quality_qp(255), 16);
        assert!(QUALITY_QP.windows(2).all(|w| w[1] <= w[0]));
    }

    #[test]
    fn key_frames_get_the_finer_qp_on_quality_steps() {
        let s = settings_for(EncoderFamily::MicrosoftHevc);
        assert_eq!(frame_qp(&s, 45, 12, 0), Some(28));
        assert_eq!(frame_qp(&s, 45, 12, 1), Some(32));
        assert_eq!(frame_qp(&s, 45, 12, 11), Some(32));
        assert_eq!(frame_qp(&s, 45, 12, 12), Some(28));
        assert_eq!(frame_qp(&s, 45, 0, 5), Some(28));
        assert_eq!(frame_qp(&s, 100, 12, 0), Some(12));
        assert_eq!(
            frame_qp(&settings_for(EncoderFamily::Nvidia), 45, 12, 0),
            None
        );
    }
}
